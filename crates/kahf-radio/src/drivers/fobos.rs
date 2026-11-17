//! Driver for the RigExpert Fobos SDR with the agile firmware.
//!
//! The vendor library is loaded at run time. This driver uses the RF input only
//! (50 MHz to 6 GHz). The HF inputs below 50 MHz are not supported yet.
//! The radio gives complex samples as float pairs at 14-bit scale, so full scale
//! arrives as 0.25. The driver multiplies by 4 so that full scale reads as 1.0.

use std::collections::VecDeque;
use std::ffi::{c_char, c_int, c_void, CStr};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use libloading::{Library, Symbol};

use crate::{DeviceInfo, GainSetting, GainStage, RxRadio};

const DRIVER: &str = "fobos";
/// The serial the library reports for a radio it cannot open (in use, or no access).
const IN_USE: &str = "XXXXXXXXXXXX";
const RF_MIN_HZ: u64 = 50_000_000;
const RF_MAX_HZ: u64 = 6_000_000_000;
const LNA_MAX_DB: f32 = 33.0;
const VGA_MAX_DB: f32 = 62.0;
const VGA_STEP_DB: f32 = 2.0;
const VGA_MAX_STEP: u32 = 31;
/// (gain in dB, step number the library takes). The LNA has three steps only.
const LNA_TABLE: [(f32, u32); 3] = [(0.0, 0), (16.0, 2), (33.0, 3)];
/// Full scale from the library is 0.25; multiplying by 4 makes it 1.0.
const SCALE: f32 = 4.0;
/// Rates used when the radio cannot be asked.
const DEFAULT_RATES: [u32; 9] = [
    8_000_000, 10_000_000, 12_500_000, 16_000_000, 20_000_000, 25_000_000, 32_000_000, 40_000_000,
    50_000_000,
];
/// Buffers the library keeps in flight, and samples per buffer.
const BUF_COUNT: u32 = 16;
const BUF_LEN: u32 = 65_536;
/// Buffers waiting between the library's thread and the reader.
const QUEUE_DEPTH: usize = 16;
/// How long read_iq waits for samples.
const WAIT: Duration = Duration::from_millis(200);
/// How long stop keeps asking the library to end the stream.
const STOP_WAIT: Duration = Duration::from_secs(3);
const LIST_BYTES: usize = 16_384;
const MAX_RADIOS: usize = 64;

#[cfg(target_os = "macos")]
const LIB_NAMES: &[&str] = &["libfobos_sdr.dylib"];
#[cfg(all(unix, not(target_os = "macos")))]
const LIB_NAMES: &[&str] = &["libfobos_sdr.so.0", "libfobos_sdr.so"];
#[cfg(windows)]
const LIB_NAMES: &[&str] = &["fobos_sdr.dll"];

type ListDevicesFn = unsafe extern "C" fn(*mut c_char) -> c_int;
type OpenFn = unsafe extern "C" fn(*mut *mut c_void, u32) -> c_int;
type CloseFn = unsafe extern "C" fn(*mut c_void) -> c_int;
type SetFrequencyFn = unsafe extern "C" fn(*mut c_void, f64) -> c_int;
type SetStepFn = unsafe extern "C" fn(*mut c_void, u32) -> c_int;
type GetRatesFn = unsafe extern "C" fn(*mut c_void, *mut f64, *mut u32) -> c_int;
type SetRateFn = unsafe extern "C" fn(*mut c_void, f64) -> c_int;
type SampleCallback = unsafe extern "C" fn(*mut f32, u32, *mut c_void, *mut c_void);
type ReadAsyncFn = unsafe extern "C" fn(*mut c_void, SampleCallback, *mut c_void, u32, u32) -> c_int;
type CancelFn = unsafe extern "C" fn(*mut c_void) -> c_int;
type ErrorNameFn = unsafe extern "C" fn(c_int) -> *const c_char;

/// The functions of the loaded library. The library stays loaded while this exists.
struct Api {
    _lib: Library,
    list_devices: ListDevicesFn,
    open: OpenFn,
    close: CloseFn,
    set_frequency: SetFrequencyFn,
    set_lna_gain: SetStepFn,
    set_vga_gain: SetStepFn,
    get_samplerates: GetRatesFn,
    set_samplerate: SetRateFn,
    read_async: ReadAsyncFn,
    cancel_async: CancelFn,
    error_name: Option<ErrorNameFn>,
}

fn symbol<T: Copy>(lib: &Library, name: &str) -> Result<T, String> {
    let mut bytes = name.as_bytes().to_vec();
    bytes.push(0);
    // SAFETY: the caller names each function with its signature from the library's interface.
    let sym: Symbol<T> = unsafe { lib.get(&bytes) }.map_err(|_| {
        format!("Fobos SDR: the library has no function {name}; it may be too old or not the Fobos SDR library")
    })?;
    Ok(*sym)
}

fn load_api(names: &[&str]) -> Result<Api, String> {
    let lib = crate::drivers::common::load_library(names).map_err(|d| missing_message(&d))?;
    let api = Api {
        list_devices: symbol(&lib, "fobos_sdr_list_devices")?,
        open: symbol(&lib, "fobos_sdr_open")?,
        close: symbol(&lib, "fobos_sdr_close")?,
        set_frequency: symbol(&lib, "fobos_sdr_set_frequency")?,
        set_lna_gain: symbol(&lib, "fobos_sdr_set_lna_gain")?,
        set_vga_gain: symbol(&lib, "fobos_sdr_set_vga_gain")?,
        get_samplerates: symbol(&lib, "fobos_sdr_get_samplerates")?,
        set_samplerate: symbol(&lib, "fobos_sdr_set_samplerate")?,
        read_async: symbol(&lib, "fobos_sdr_read_async")?,
        cancel_async: symbol(&lib, "fobos_sdr_cancel_async")?,
        error_name: symbol(&lib, "fobos_sdr_error_name").ok(),
        _lib: lib,
    };
    Ok(api)
}

fn missing_message(detail: &str) -> String {
    format!(
        "Fobos SDR: the radio library (fobos_sdr) is not installed or cannot be loaded ({detail}). \
         Install the Fobos SDR library for this system, then try again."
    )
}

fn error_text(name: Option<&str>, code: i32) -> String {
    match name {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => format!("error code {code}"),
    }
}

fn describe(api: &Api, code: i32) -> String {
    let name = api.error_name.and_then(|f| {
        // SAFETY: the library returns a static text for a code, or a null pointer.
        let p = unsafe { f(code) };
        if p.is_null() {
            None
        } else {
            // SAFETY: the pointer is non-null and points to a NUL-terminated text.
            Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
        }
    });
    error_text(name.as_deref(), code)
}

/// Splits the serial list the library writes (each serial followed by a space).
/// An empty entry keeps its place in the list. `count` is what the library returned.
fn parse_serials(text: &str, count: i32) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        if ch == ' ' {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(ch);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out.truncate(count.clamp(0, MAX_RADIOS as i32) as usize);
    out
}

/// Chooses the radio: the one with the serial `id`, or the first usable one when `id` is None.
/// The index is the radio's position in the library's list, which is the index it opens by.
fn pick_index(serials: &[String], id: Option<&str>) -> Result<usize, String> {
    match id {
        None => {
            if serials.is_empty() {
                return Err("Fobos SDR: no radio found. Check the USB cable and that the radio has the agile firmware".into());
            }
            serials
                .iter()
                .position(|s| s.as_str() != IN_USE)
                .ok_or_else(|| "Fobos SDR: every radio found is in use by another program or has no USB access".to_string())
        }
        Some(want) => match serials.iter().position(|s| s.as_str() == want) {
            Some(i) if serials[i] == IN_USE => Err(format!(
                "Fobos SDR: the radio {want} is in use by another program or has no USB access"
            )),
            Some(i) => Ok(i),
            None => {
                let found: Vec<&str> = serials.iter().map(|s| s.as_str()).filter(|s| *s != IN_USE).collect();
                let list = if found.is_empty() { "none".to_string() } else { found.join(", ") };
                Err(format!("Fobos SDR: no radio with serial {want}; found: {list}"))
            }
        },
    }
}

/// Makes the rate list sorted and unique, without zero or bad values. Uses the default list when empty.
fn clean_rates(values: &[f64]) -> Vec<u32> {
    let mut out: Vec<u32> = values
        .iter()
        .filter(|v| v.is_finite() && **v >= 1.0 && **v <= u32::MAX as f64)
        .map(|v| v.round() as u32)
        .collect();
    out.sort_unstable();
    out.dedup();
    if out.is_empty() {
        DEFAULT_RATES.to_vec()
    } else {
        out
    }
}

/// The offered rate closest to `want`. On a tie the higher rate wins.
fn nearest_rate(rates: &[u32], want: u32) -> Option<u32> {
    rates
        .iter()
        .copied()
        .min_by(|a, b| a.abs_diff(want).cmp(&b.abs_diff(want)).then(b.cmp(a)))
}

/// The LNA step for a gain in dB. The gain is clamped to 0..33 dB and snapped to 0, 16 or 33.
/// On a tie the lower step wins.
fn lna_step_for(db: f32) -> u32 {
    let db = db.clamp(0.0, LNA_MAX_DB);
    let mut best = LNA_TABLE[0];
    for entry in LNA_TABLE {
        if (entry.0 - db).abs() < (best.0 - db).abs() {
            best = entry;
        }
    }
    best.1
}

/// The VGA step for a gain in dB. The gain is clamped to 0..62 dB, in 2 dB steps.
fn vga_step_for(db: f32) -> u32 {
    let db = db.clamp(0.0, VGA_MAX_DB);
    ((db / VGA_STEP_DB).round() as u32).min(VGA_MAX_STEP)
}

/// Moves complete I/Q pairs from `pending` into `out`, scaled to full scale 1.0.
/// Returns how many pairs were written. A lone value stays in `pending`.
fn convert_pairs(pending: &mut VecDeque<f32>, out: &mut [(f32, f32)]) -> usize {
    let mut n = 0;
    while n < out.len() && pending.len() >= 2 {
        if let (Some(i), Some(q)) = (pending.pop_front(), pending.pop_front()) {
            out[n] = (i * SCALE, q * SCALE);
            n += 1;
        } else {
            break;
        }
    }
    n
}

fn mhz_text(hz: u64) -> String {
    format!("{} MHz", hz as f64 / 1e6)
}

fn label_for(serial: &str) -> String {
    if serial.is_empty() {
        "Fobos SDR".to_string()
    } else {
        format!("Fobos SDR {serial}")
    }
}

fn list_serials(api: &Api) -> Vec<String> {
    let mut buf = vec![0u8; LIST_BYTES];
    // SAFETY: the buffer is LIST_BYTES long, and the library writes no more than its listing.
    let count = unsafe { (api.list_devices)(buf.as_mut_ptr() as *mut c_char) };
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let text = String::from_utf8_lossy(&buf[..end]).into_owned();
    parse_serials(&text, count)
}

fn query_rates(api: &Api, handle: *mut c_void) -> Vec<f64> {
    let mut count: u32 = 0;
    // SAFETY: a null list asks only for the count.
    let rc = unsafe { (api.get_samplerates)(handle, ptr::null_mut(), &mut count) };
    if rc != 0 || count == 0 || count as usize > MAX_RADIOS {
        return Vec::new();
    }
    let mut values = vec![0f64; count as usize];
    // SAFETY: the list holds `count` values, as the count says.
    let rc = unsafe { (api.get_samplerates)(handle, values.as_mut_ptr(), &mut count) };
    if rc != 0 {
        return Vec::new();
    }
    values.truncate(count as usize);
    values
}

/// Data from the library's stream thread. The callback owns a sender; the stream ends when it is dropped.
struct CallbackContext {
    tx: SyncSender<Vec<f32>>,
    running: Arc<AtomicBool>,
    dropped: Arc<AtomicU64>,
}

/// Called by the library for each buffer of `n` complex samples (float I/Q pairs).
extern "C" fn on_samples(buf: *mut f32, n: u32, _sender: *mut c_void, user: *mut c_void) {
    if buf.is_null() || user.is_null() || n == 0 {
        return;
    }
    // SAFETY: `user` is the context the stream thread made, and it lives until read_async returns.
    let ctx = unsafe { &*(user as *const CallbackContext) };
    if !ctx.running.load(Ordering::SeqCst) {
        return;
    }
    // SAFETY: the library gives `n` complex samples, which is 2 * n floats.
    let samples = unsafe { std::slice::from_raw_parts(buf, n as usize * 2) }.to_vec();
    if let Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) = ctx.tx.try_send(samples) {
        ctx.dropped.fetch_add(n as u64, Ordering::SeqCst);
    }
}

struct Stream {
    thread: JoinHandle<()>,
    rx: Receiver<Vec<f32>>,
    running: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    end_code: Arc<Mutex<Option<i32>>>,
}

/// An open Fobos SDR. Used from one thread at a time.
pub struct FobosRadio {
    api: Arc<Api>,
    /// The library's device handle, kept as a number so the stream thread can use it.
    handle: usize,
    serial: String,
    rates: Vec<u32>,
    lna_step: u32,
    vga_step: u32,
    stream: Option<Stream>,
    dropped: Arc<AtomicU64>,
    pending: VecDeque<f32>,
}

impl FobosRadio {
    fn ptr(&self) -> *mut c_void {
        self.handle as *mut c_void
    }

    fn check(&self, rc: c_int, what: &str) -> Result<(), String> {
        if rc == 0 {
            Ok(())
        } else {
            Err(format!("Fobos SDR: {what} was refused ({})", describe(&self.api, rc)))
        }
    }
}

pub fn probe() -> Vec<DeviceInfo> {
    let Ok(api) = load_api(LIB_NAMES) else {
        return Vec::new();
    };
    list_serials(&api)
        .into_iter()
        .filter(|s| !s.is_empty() && s != IN_USE)
        .map(|serial| DeviceInfo { driver: DRIVER, label: label_for(&serial), id: serial })
        .collect()
}

pub fn open(id: Option<&str>) -> Result<Box<dyn RxRadio>, String> {
    open_with(LIB_NAMES, id)
}

fn open_with(names: &[&str], id: Option<&str>) -> Result<Box<dyn RxRadio>, String> {
    let api = Arc::new(load_api(names)?);
    let serials = list_serials(&api);
    let index = pick_index(&serials, id)?;
    let serial = serials[index].clone();

    let mut handle: *mut c_void = ptr::null_mut();
    // SAFETY: `handle` is a valid place for the library to write the device handle.
    let rc = unsafe { (api.open)(&mut handle, index as u32) };
    if rc != 0 || handle.is_null() {
        return Err(format!(
            "Fobos SDR: cannot open the radio {serial} ({}): it may be in use by another program, unplugged, or without USB access",
            describe(&api, rc)
        ));
    }

    let mut radio = FobosRadio {
        api: Arc::clone(&api),
        handle: handle as usize,
        serial,
        rates: Vec::new(),
        lna_step: 0,
        vga_step: 0,
        stream: None,
        dropped: Arc::new(AtomicU64::new(0)),
        pending: VecDeque::new(),
    };
    // From here on, Drop closes the radio if anything below fails.
    radio.rates = clean_rates(&query_rates(&api, radio.ptr()));
    // SAFETY: the handle is open; the step is one the library takes.
    let rc = unsafe { (api.set_lna_gain)(radio.ptr(), 0) };
    radio.check(rc, "the LNA gain at start")?;
    // SAFETY: as above.
    let rc = unsafe { (api.set_vga_gain)(radio.ptr(), 0) };
    radio.check(rc, "the VGA gain at start")?;
    Ok(Box::new(radio))
}

impl RxRadio for FobosRadio {
    fn label(&self) -> String {
        label_for(&self.serial)
    }

    fn freq_range_hz(&self) -> (u64, u64) {
        (RF_MIN_HZ, RF_MAX_HZ)
    }

    fn set_frequency_hz(&mut self, hz: u64) -> Result<(), String> {
        if !(RF_MIN_HZ..=RF_MAX_HZ).contains(&hz) {
            return Err(format!(
                "Fobos SDR: {} is outside the RF input range of 50 MHz to 6 GHz. Lower frequencies need the HF inputs, which this driver does not use yet",
                mhz_text(hz)
            ));
        }
        // SAFETY: the handle is open; the frequency is in the RF range.
        let rc = unsafe { (self.api.set_frequency)(self.ptr(), hz as f64) };
        self.check(rc, &format!("the frequency {}", mhz_text(hz)))
    }

    fn sample_rates(&self) -> Vec<u32> {
        self.rates.clone()
    }

    fn set_sample_rate(&mut self, hz: u32) -> Result<u32, String> {
        if self.stream.is_some() {
            return Err("Fobos SDR: the sample rate can only change while the radio is stopped; stop it first".into());
        }
        let rate = nearest_rate(&self.rates, hz)
            .ok_or_else(|| "Fobos SDR: the radio offers no sample rates".to_string())?;
        // SAFETY: the handle is open; the rate is one the radio offered.
        let rc = unsafe { (self.api.set_samplerate)(self.ptr(), rate as f64) };
        self.check(rc, &format!("the sample rate of {rate} samples per second"))?;
        Ok(rate)
    }

    fn gain_stages(&self) -> Vec<GainStage> {
        vec![
            GainStage { name: "LNA".into(), min_db: 0.0, max_db: LNA_MAX_DB, step_db: 0.0, auto: false },
            GainStage { name: "VGA".into(), min_db: 0.0, max_db: VGA_MAX_DB, step_db: VGA_STEP_DB, auto: false },
        ]
    }

    fn set_gain(&mut self, stage: &str, setting: GainSetting) -> Result<(), String> {
        let db = match setting {
            GainSetting::Auto => {
                return Err(format!("Fobos SDR: the {stage} stage has no automatic mode; give a fixed value in dB"))
            }
            GainSetting::Manual(v) if !v.is_finite() => {
                return Err(format!("Fobos SDR: the {stage} gain must be a number of dB"))
            }
            GainSetting::Manual(v) => v,
        };
        match stage {
            "LNA" => {
                let step = lna_step_for(db);
                if step != self.lna_step {
                    // SAFETY: the handle is open; the step is one the library takes.
                    let rc = unsafe { (self.api.set_lna_gain)(self.ptr(), step) };
                    self.check(rc, "the LNA gain")?;
                    self.lna_step = step;
                }
                Ok(())
            }
            "VGA" => {
                let step = vga_step_for(db);
                if step != self.vga_step {
                    // SAFETY: the handle is open; the step is in the library's range.
                    let rc = unsafe { (self.api.set_vga_gain)(self.ptr(), step) };
                    self.check(rc, "the VGA gain")?;
                    self.vga_step = step;
                }
                Ok(())
            }
            other => Err(format!("Fobos SDR: no gain stage named {other:?}; the stages are LNA and VGA")),
        }
    }

    fn start(&mut self) -> Result<(), String> {
        if self.stream.is_some() {
            return Ok(());
        }
        let (tx, rx) = mpsc::sync_channel::<Vec<f32>>(QUEUE_DEPTH);
        let running = Arc::new(AtomicBool::new(true));
        let finished = Arc::new(AtomicBool::new(false));
        let end_code: Arc<Mutex<Option<i32>>> = Arc::new(Mutex::new(None));
        let ctx = CallbackContext {
            tx,
            running: Arc::clone(&running),
            dropped: Arc::clone(&self.dropped),
        };
        let api = Arc::clone(&self.api);
        let handle = self.handle;
        let fin = Arc::clone(&finished);
        let end = Arc::clone(&end_code);
        let thread = thread::Builder::new()
            .name("fobos-stream".into())
            .spawn(move || {
                let ctx_ptr = Box::into_raw(Box::new(ctx));
                // SAFETY: the handle is open until the driver closes it after this thread is joined.
                // The context lives until read_async returns.
                let rc = unsafe {
                    (api.read_async)(handle as *mut c_void, on_samples, ctx_ptr as *mut c_void, BUF_COUNT, BUF_LEN)
                };
                // SAFETY: read_async has returned, so the library makes no more callbacks.
                drop(unsafe { Box::from_raw(ctx_ptr) });
                if rc != 0 {
                    if let Ok(mut g) = end.lock() {
                        *g = Some(rc);
                    }
                }
                fin.store(true, Ordering::SeqCst);
            })
            .map_err(|e| format!("Fobos SDR: cannot start the stream thread: {e}"))?;
        self.stream = Some(Stream { thread, rx, running, finished, end_code });
        Ok(())
    }

    fn read_iq(&mut self, out: &mut [(f32, f32)]) -> Result<usize, String> {
        if out.is_empty() {
            return Ok(0);
        }
        let Some(stream) = self.stream.as_ref() else {
            return Err("Fobos SDR: the radio is not started; call start first".into());
        };
        let n = convert_pairs(&mut self.pending, out);
        if n > 0 {
            return Ok(n);
        }
        match stream.rx.recv_timeout(WAIT) {
            Ok(buf) => {
                self.pending.extend(buf);
                Ok(convert_pairs(&mut self.pending, out))
            }
            Err(RecvTimeoutError::Timeout) => Ok(0),
            Err(RecvTimeoutError::Disconnected) => {
                let code = stream.end_code.lock().ok().and_then(|g| *g);
                match code {
                    Some(c) => Err(format!("Fobos SDR: the stream ended ({})", describe(&self.api, c))),
                    None => Err("Fobos SDR: the stream ended".into()),
                }
            }
        }
    }

    fn stop(&mut self) {
        let Some(stream) = self.stream.take() else {
            return;
        };
        stream.running.store(false, Ordering::SeqCst);
        let began = Instant::now();
        // The library ignores a cancel that comes before its stream runs, so ask again until the thread ends.
        while !stream.finished.load(Ordering::SeqCst) && began.elapsed() < STOP_WAIT {
            // SAFETY: the handle is open until Drop; cancelling a stream is allowed at any time.
            unsafe { (self.api.cancel_async)(self.ptr()) };
            thread::sleep(Duration::from_millis(10));
        }
        let _ = stream.thread.join();
        self.pending.clear();
    }

    fn dropped_samples(&self) -> u64 {
        self.dropped.load(Ordering::SeqCst)
    }
}

impl Drop for FobosRadio {
    fn drop(&mut self) {
        self.stop();
        // SAFETY: the handle came from a successful open and is closed only here.
        unsafe { (self.api.close)(self.ptr()) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_choice_takes_the_closest_offered_rate() {
        let rates = clean_rates(&[]);
        assert_eq!(rates.first(), Some(&8_000_000));
        assert_eq!(nearest_rate(&rates, 2_400_000), Some(8_000_000));
        assert_eq!(nearest_rate(&rates, 60_000_000), Some(50_000_000));
        assert_eq!(nearest_rate(&rates, 12_000_000), Some(12_500_000));
        // a tie between 8 and 10 MHz goes to the higher rate
        assert_eq!(nearest_rate(&rates, 9_000_000), Some(10_000_000));
        assert_eq!(nearest_rate(&[], 9_000_000), None);
    }

    #[test]
    fn rate_list_is_cleaned() {
        assert_eq!(clean_rates(&[50e6, 8e6, 8e6, -1.0, f64::NAN, 0.0]), vec![8_000_000, 50_000_000]);
        assert_eq!(clean_rates(&[64e6]), vec![64_000_000]);
    }

    #[test]
    fn gain_is_clamped_and_snapped() {
        assert_eq!(lna_step_for(-5.0), 0);
        assert_eq!(lna_step_for(10.0), 2);
        assert_eq!(lna_step_for(24.0), 2);
        assert_eq!(lna_step_for(25.0), 3);
        assert_eq!(lna_step_for(40.0), 3);
        assert_eq!(vga_step_for(-1.0), 0);
        assert_eq!(vga_step_for(3.0), 2);
        assert_eq!(vga_step_for(61.0), 31);
        assert_eq!(vga_step_for(100.0), 31);
    }

    #[test]
    fn samples_are_scaled_to_full_scale_one() {
        let mut pending: VecDeque<f32> = [0.25, -0.25, 0.5, 0.0, 0.1].into_iter().collect();
        let mut out = [(0.0f32, 0.0f32); 3];
        let n = convert_pairs(&mut pending, &mut out);
        assert_eq!(n, 2);
        assert_eq!(out[0], (1.0, -1.0));
        assert_eq!(out[1], (2.0, 0.0));
        assert_eq!(pending.len(), 1);
    }

    #[test]
    fn serial_list_keeps_its_places() {
        assert_eq!(parse_serials("ABC DEF ", 2), vec!["ABC".to_string(), "DEF".to_string()]);
        assert_eq!(parse_serials("ABC DEF ", 1), vec!["ABC".to_string()]);
        assert_eq!(parse_serials("ABC  DEF ", 3), vec!["ABC".to_string(), String::new(), "DEF".to_string()]);
        assert!(parse_serials("", 0).is_empty());
    }

    #[test]
    fn radio_choice_by_serial() {
        let serials = vec!["AAA".to_string(), IN_USE.to_string(), "BBB".to_string()];
        assert_eq!(pick_index(&serials, None), Ok(0));
        assert_eq!(pick_index(&serials, Some("BBB")), Ok(2));
        assert!(pick_index(&serials, Some(IN_USE)).unwrap_err().contains("in use"));
        assert!(pick_index(&serials, Some("ZZZ")).unwrap_err().contains("no radio with serial ZZZ"));
        assert!(pick_index(&[], None).unwrap_err().contains("no radio found"));
        let busy = vec![IN_USE.to_string()];
        assert!(pick_index(&busy, None).unwrap_err().contains("in use"));
    }

    #[test]
    fn error_text_uses_library_name_or_code() {
        assert_eq!(error_text(Some("FOBOS_ERR_NO_DEV"), -3), "FOBOS_ERR_NO_DEV");
        assert_eq!(error_text(None, -3), "error code -3");
        assert_eq!(error_text(Some(""), 7), "error code 7");
    }

    #[test]
    fn missing_library_gives_a_plain_message() {
        let msg = missing_message("library not found");
        assert!(msg.starts_with("Fobos SDR:"));
        assert!(msg.contains("not installed"));
        assert!(msg.contains("fobos_sdr"));
        assert!(msg.contains("library not found"));
    }

    #[test]
    fn open_reports_missing_library() {
        let err = open_with(&["/nonexistent/kahf-test/libfobos_sdr.so"], None)
            .err()
            .expect("open must fail when the library is absent");
        assert!(err.starts_with("Fobos SDR:"), "{err}");
        assert!(err.contains("not installed"), "{err}");
    }
}
