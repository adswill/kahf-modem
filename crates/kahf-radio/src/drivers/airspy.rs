//! Driver for the Airspy R2 (and the Airspy Mini at its listed rates).
//!
//! The radio's host library is loaded at run time. Without it, `probe` finds
//! nothing and `open` says what is missing. Samples arrive as complex float
//! pairs from the library's stream callback and wait in a bounded queue.
//!
//! Not checked against hardware: the code paths that talk to the library are
//! written from the library's interface and are only covered by unit tests on
//! the pure helpers.

use std::collections::VecDeque;
use std::ffi::{c_char, c_int, c_void, CStr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use libloading::{Library, Symbol};

use crate::drivers::common::load_library;
use crate::{DeviceInfo, GainSetting, GainStage, RxRadio};

/// Tuning range of the R820T tuner in Hz.
const FREQ_MIN_HZ: u64 = 24_000_000;
const FREQ_MAX_HZ: u64 = 1_800_000_000;
/// Frequency used at open, until the caller sets one.
const DEFAULT_FREQ_HZ: u64 = 100_000_000;
/// Linearity gain steps offered by the library (0 to 21).
const LINEARITY_MAX: f32 = 21.0;
/// Manual stage steps (0 to 15 each).
const STAGE_MAX: f32 = 15.0;
/// Largest number of complex samples kept while the reader is late (about 4 million).
const QUEUE_CAP: usize = 1 << 22;
/// Longest time one `read_iq` call waits for samples.
const READ_WAIT: Duration = Duration::from_millis(200);
/// Most entries read from the library in one call.
const MAX_RATES: usize = 16;
/// Sample type asked for: float pairs (I, Q), each -1..1.
const SAMPLE_TYPE_FLOAT32_IQ: c_int = 0;

#[cfg(target_os = "macos")]
const LIBRARY_NAMES: &[&str] = &["libairspy.dylib"];
#[cfg(all(unix, not(target_os = "macos")))]
const LIBRARY_NAMES: &[&str] = &["libairspy.so.0", "libairspy.so"];
#[cfg(windows)]
const LIBRARY_NAMES: &[&str] = &["airspy.dll"];
#[cfg(not(any(unix, windows)))]
const LIBRARY_NAMES: &[&str] = &[];

// Function types of the library's interface. Each one is used through a raw pointer.
type Dev = *mut c_void;
type CallbackFn = unsafe extern "C" fn(*mut Transfer) -> c_int;
type InitFn = unsafe extern "C" fn() -> c_int;
type ListFn = unsafe extern "C" fn(*mut u64, c_int) -> c_int;
type OpenSnFn = unsafe extern "C" fn(*mut Dev, u64) -> c_int;
type OpenFn = unsafe extern "C" fn(*mut Dev) -> c_int;
type GetRatesFn = unsafe extern "C" fn(Dev, *mut u32, u32) -> c_int;
type SetU32Fn = unsafe extern "C" fn(Dev, u32) -> c_int;
type SetSampleTypeFn = unsafe extern "C" fn(Dev, c_int) -> c_int;
type SetGainFn = unsafe extern "C" fn(Dev, u8) -> c_int;
type StartFn = unsafe extern "C" fn(Dev, CallbackFn, *mut c_void) -> c_int;
type DevFn = unsafe extern "C" fn(Dev) -> c_int;
type ErrorNameFn = unsafe extern "C" fn(c_int) -> *const c_char;

/// What the library passes to the stream callback.
#[repr(C)]
pub struct Transfer {
    device: Dev,
    ctx: *mut c_void,
    samples: *mut c_void,
    sample_count: c_int,
    dropped_samples: u64,
    sample_type: c_int,
}

/// Functions of the loaded library. The library stays loaded while this value lives.
struct Api {
    _lib: Library,
    init: InitFn,
    list_devices: Option<ListFn>,
    open_sn: OpenSnFn,
    open: OpenFn,
    close: DevFn,
    get_samplerates: GetRatesFn,
    set_samplerate: SetU32Fn,
    set_sample_type: SetSampleTypeFn,
    set_freq: SetU32Fn,
    set_linearity_gain: SetGainFn,
    set_lna_gain: Option<SetGainFn>,
    set_mixer_gain: Option<SetGainFn>,
    set_vga_gain: Option<SetGainFn>,
    start_rx: StartFn,
    stop_rx: DevFn,
    is_streaming: Option<DevFn>,
    error_name: Option<ErrorNameFn>,
}

/// Loads one symbol that must exist.
///
/// # Safety
/// `T` must be the exact function type the library exports under `name`.
unsafe fn required<T: Copy>(lib: &Library, name: &str) -> Result<T, String> {
    let sym: Symbol<T> = lib
        .get(name.as_bytes())
        .map_err(|_| format!("Airspy: the installed libairspy has no {name}; update libairspy"))?;
    Ok(*sym)
}

/// Loads one symbol that may be missing in older libraries.
///
/// # Safety
/// `T` must be the exact function type the library exports under `name`, if it exports it.
unsafe fn optional<T: Copy>(lib: &Library, name: &str) -> Option<T> {
    lib.get::<T>(name.as_bytes()).ok().map(|sym| *sym)
}

impl Api {
    fn load() -> Result<Api, String> {
        load_with_names(LIBRARY_NAMES).and_then(Api::from_library)
    }

    fn from_library(lib: Library) -> Result<Api, String> {
        // SAFETY: each type below matches the library's documented signature for that name.
        unsafe {
            Ok(Api {
                init: required(&lib, "airspy_init")?,
                list_devices: optional(&lib, "airspy_list_devices"),
                open_sn: required(&lib, "airspy_open_sn")?,
                open: required(&lib, "airspy_open")?,
                close: required(&lib, "airspy_close")?,
                get_samplerates: required(&lib, "airspy_get_samplerates")?,
                set_samplerate: required(&lib, "airspy_set_samplerate")?,
                set_sample_type: required(&lib, "airspy_set_sample_type")?,
                set_freq: required(&lib, "airspy_set_freq")?,
                set_linearity_gain: required(&lib, "airspy_set_linearity_gain")?,
                set_lna_gain: optional(&lib, "airspy_set_lna_gain"),
                set_mixer_gain: optional(&lib, "airspy_set_mixer_gain"),
                set_vga_gain: optional(&lib, "airspy_set_vga_gain"),
                start_rx: required(&lib, "airspy_start_rx")?,
                stop_rx: required(&lib, "airspy_stop_rx")?,
                is_streaming: optional(&lib, "airspy_is_streaming"),
                error_name: optional(&lib, "airspy_error_name"),
                _lib: lib,
            })
        }
    }

    /// Text for a library error code, with the library's own name for it when it has one.
    fn describe(&self, rc: c_int) -> String {
        let name = self.error_name.and_then(|f| {
            // SAFETY: the library returns a pointer to a static string or null.
            let p = unsafe { f(rc) };
            if p.is_null() {
                None
            } else {
                // SAFETY: non-null, and the library keeps the string alive.
                Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
            }
        });
        error_text(rc, name.as_deref())
    }

    fn check(&self, rc: c_int, what: &str) -> Result<(), String> {
        if rc == 0 {
            Ok(())
        } else {
            Err(format!("Airspy: {what} failed: {}", self.describe(rc)))
        }
    }
}

/// Loads the first library in `names` that the system has.
fn load_with_names(names: &[&str]) -> Result<Library, String> {
    if names.is_empty() {
        return Err(missing_library_text("no library name is known for this system"));
    }
    load_library(names).map_err(|e| missing_library_text(&e))
}

/// The message when the host library cannot be loaded.
pub fn missing_library_text(detail: &str) -> String {
    format!("Airspy: the libairspy host library was not found ({detail}). Install libairspy and try again.")
}

/// The message for a library error code: the code, then the library's name for it when given.
pub fn error_text(rc: c_int, name: Option<&str>) -> String {
    match name.filter(|n| !n.is_empty()) {
        Some(n) => format!("libairspy error {rc} {n}"),
        None => format!("libairspy error {rc}"),
    }
}

/// Picks the listed rate closest to `want` that is not below it. When none is high
/// enough, the highest listed rate. None when the radio lists no rate.
pub fn pick_rate(listed: &[u32], want: u32) -> Option<u32> {
    let mut rates: Vec<u32> = listed.iter().copied().filter(|&r| r > 0).collect();
    rates.sort_unstable();
    rates.dedup();
    if rates.is_empty() {
        return None;
    }
    let floor = want as f64 * 0.999;
    rates
        .iter()
        .copied()
        .find(|&r| r as f64 >= floor)
        .or_else(|| rates.last().copied())
}

/// Turns a dB (or step) value into the whole step the library takes, clamped to the range.
/// A value that is not a number gives the lowest step.
pub fn clamp_gain(value: f32, min: f32, max: f32) -> u8 {
    if value.is_nan() {
        return min.round().max(0.0) as u8;
    }
    value.clamp(min, max).round().clamp(0.0, 255.0) as u8
}

/// Converts raw interleaved float pairs to complex samples, limited to -1..1.
/// A trailing lone value is ignored. A value that is not a number becomes 0.
pub fn convert_iq(raw: &[f32]) -> Vec<(f32, f32)> {
    raw.as_chunks::<2>()
        .0
        .iter()
        .map(|p| (clean(p[0]), clean(p[1])))
        .collect()
}

fn clean(v: f32) -> f32 {
    if v.is_nan() {
        0.0
    } else {
        v.clamp(-1.0, 1.0)
    }
}

/// Adds samples to the queue without going past `cap`. Returns how many were dropped.
fn push_capped(queue: &mut VecDeque<(f32, f32)>, samples: &[(f32, f32)], cap: usize) -> u64 {
    let room = cap.saturating_sub(queue.len());
    let keep = room.min(samples.len());
    queue.extend(samples[..keep].iter().copied());
    (samples.len() - keep) as u64
}

/// State shared with the stream callback.
struct Shared {
    queue: Mutex<VecDeque<(f32, f32)>>,
    ready: Condvar,
    dropped: AtomicU64,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, VecDeque<(f32, f32)>> {
        self.queue.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The stream callback. Called by the library's thread with a block of float pairs.
///
/// # Safety
/// `t` is the transfer the library passes in. `ctx` is the pointer given to `start_rx`,
/// which points at a `Shared` that outlives the streaming.
unsafe extern "C" fn stream_callback(t: *mut Transfer) -> c_int {
    if t.is_null() {
        return 0;
    }
    // SAFETY: see the function's safety note.
    let t = unsafe { &*t };
    if t.ctx.is_null() || t.samples.is_null() || t.sample_count <= 0 {
        return 0;
    }
    // SAFETY: the library gives `sample_count` complex float pairs at `samples`.
    let shared = unsafe { &*(t.ctx as *const Shared) };
    let raw = unsafe { std::slice::from_raw_parts(t.samples as *const f32, t.sample_count as usize * 2) };
    let samples = convert_iq(raw);
    let mut q = shared.lock();
    let dropped = push_capped(&mut q, &samples, QUEUE_CAP);
    drop(q);
    if dropped > 0 {
        shared.dropped.fetch_add(dropped, Ordering::Relaxed);
    }
    shared.ready.notify_one();
    0
}

/// Reads a serial number written as hex, with or without a 0x prefix.
fn parse_serial(text: &str) -> Result<u64, String> {
    let t = text.trim();
    let hex = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")).unwrap_or(t);
    u64::from_str_radix(hex, 16).map_err(|_| format!("Airspy: serial {t:?} is not a hex number"))
}

/// The gain stages the library offers. The linearity step is the one gain control;
/// lna, mixer and vga are optional manual stages in libraries that have them.
fn stage_list(api: &Api) -> Vec<GainStage> {
    let mut out = vec![GainStage {
        name: "gain".into(),
        min_db: 0.0,
        max_db: LINEARITY_MAX,
        step_db: 1.0,
        auto: false,
    }];
    let manual = [
        ("lna", api.set_lna_gain.is_some()),
        ("mixer", api.set_mixer_gain.is_some()),
        ("vga", api.set_vga_gain.is_some()),
    ];
    for (name, present) in manual {
        if present {
            out.push(GainStage {
                name: name.into(),
                min_db: 0.0,
                max_db: STAGE_MAX,
                step_db: 1.0,
                auto: false,
            });
        }
    }
    out
}

/// An open Airspy. Used from one thread at a time.
pub struct AirspyRadio {
    api: Api,
    dev: Dev,
    shared: Arc<Shared>,
    rates: Vec<u32>,
    rate: u32,
    streaming: bool,
    stages: Vec<GainStage>,
}

// SAFETY: the device handle is only used from the thread that owns this value
// (the trait is used from one thread at a time). The callback touches only `Shared`.
unsafe impl Send for AirspyRadio {}

impl AirspyRadio {
    fn set_device_frequency(&mut self, hz: u64) -> Result<(), String> {
        // SAFETY: the handle is open for the life of this value.
        let rc = unsafe { (self.api.set_freq)(self.dev, hz as u32) };
        self.api.check(rc, "setting the frequency")
    }

    fn apply_rate(&mut self, rate: u32) -> Result<(), String> {
        // SAFETY: the handle is open for the life of this value.
        let rc = unsafe { (self.api.set_samplerate)(self.dev, rate) };
        self.api.check(rc, "setting the sample rate")?;
        self.rate = rate;
        Ok(())
    }

    fn stop_stream(&mut self) {
        if self.streaming {
            // SAFETY: the handle is open. The library returns once its threads are gone.
            unsafe { (self.api.stop_rx)(self.dev) };
            self.streaming = false;
        }
    }
}

impl RxRadio for AirspyRadio {
    fn label(&self) -> String {
        "Airspy".into()
    }

    fn freq_range_hz(&self) -> (u64, u64) {
        (FREQ_MIN_HZ, FREQ_MAX_HZ)
    }

    fn set_frequency_hz(&mut self, hz: u64) -> Result<(), String> {
        if !(FREQ_MIN_HZ..=FREQ_MAX_HZ).contains(&hz) {
            return Err(format!(
                "Airspy: {} kHz is outside the tuning range of 24000 kHz to 1800000 kHz",
                hz / 1000
            ));
        }
        self.set_device_frequency(hz)
    }

    fn sample_rates(&self) -> Vec<u32> {
        self.rates.clone()
    }

    fn set_sample_rate(&mut self, hz: u32) -> Result<u32, String> {
        let rate = pick_rate(&self.rates, hz)
            .ok_or_else(|| "Airspy: the radio lists no sample rates".to_string())?;
        if self.streaming {
            self.stop_stream();
            self.apply_rate(rate)?;
            self.start()?;
        } else {
            self.apply_rate(rate)?;
        }
        Ok(rate)
    }

    fn gain_stages(&self) -> Vec<GainStage> {
        self.stages.clone()
    }

    fn set_gain(&mut self, stage: &str, setting: GainSetting) -> Result<(), String> {
        let info = self
            .stages
            .iter()
            .find(|s| s.name == stage)
            .cloned()
            .ok_or_else(|| format!("Airspy: no gain stage named {stage:?}"))?;
        let value = match setting {
            GainSetting::Auto => {
                return Err(format!("Airspy: gain stage {stage:?} has no automatic setting"));
            }
            GainSetting::Manual(v) => clamp_gain(v, info.min_db, info.max_db),
        };
        let func = match stage {
            "gain" => Some(self.api.set_linearity_gain),
            "lna" => self.api.set_lna_gain,
            "mixer" => self.api.set_mixer_gain,
            "vga" => self.api.set_vga_gain,
            _ => None,
        }
        .ok_or_else(|| format!("Airspy: gain stage {stage:?} is not available"))?;
        // SAFETY: the handle is open and the function matches the library's signature.
        let rc = unsafe { func(self.dev, value) };
        self.api.check(rc, &format!("setting the {stage} gain"))
    }

    fn start(&mut self) -> Result<(), String> {
        if self.streaming {
            return Ok(());
        }
        {
            let mut q = self.shared.lock();
            q.clear();
        }
        let ctx = Arc::as_ptr(&self.shared) as *mut c_void;
        // SAFETY: the handle is open; `ctx` stays valid until `stop_stream` returns (see Drop).
        let rc = unsafe { (self.api.start_rx)(self.dev, stream_callback, ctx) };
        self.api.check(rc, "starting the stream")?;
        self.streaming = true;
        Ok(())
    }

    fn read_iq(&mut self, out: &mut [(f32, f32)]) -> Result<usize, String> {
        if out.is_empty() {
            return Ok(0);
        }
        let mut q = self.shared.lock();
        if q.is_empty() {
            let (guard, _) = self
                .shared
                .ready
                .wait_timeout(q, READ_WAIT)
                .unwrap_or_else(|e| e.into_inner());
            q = guard;
        }
        let n = out.len().min(q.len());
        for (slot, s) in out.iter_mut().zip(q.drain(..n)) {
            *slot = s;
        }
        drop(q);
        if n == 0 && self.streaming {
            if let Some(is_streaming) = self.api.is_streaming {
                // SAFETY: the handle is open.
                if unsafe { is_streaming(self.dev) } == 0 {
                    self.streaming = false;
                    return Err("Airspy: the radio stopped streaming (unplugged?)".into());
                }
            }
        }
        Ok(n)
    }

    fn stop(&mut self) {
        self.stop_stream();
    }

    fn dropped_samples(&self) -> u64 {
        self.shared.dropped.load(Ordering::Relaxed)
    }
}

impl Drop for AirspyRadio {
    fn drop(&mut self) {
        // The stream stops before the handle closes; the callback context is still alive here.
        self.stop_stream();
        if !self.dev.is_null() {
            // SAFETY: the handle came from a successful open and is closed once.
            unsafe { (self.api.close)(self.dev) };
            self.dev = std::ptr::null_mut();
        }
    }
}

/// Lists the Airspy radios the library can see. Empty when the library is missing or
/// lists nothing. Each id is the serial number in hex.
pub fn probe() -> Vec<DeviceInfo> {
    let Ok(api) = Api::load() else {
        return Vec::new();
    };
    let Some(list) = api.list_devices else {
        return Vec::new();
    };
    // SAFETY: init takes no arguments and returns a status code.
    if unsafe { (api.init)() } != 0 {
        return Vec::new();
    }
    let mut serials = [0u64; MAX_RATES];
    // SAFETY: the buffer holds MAX_RATES entries and the length says so.
    let n = unsafe { list(serials.as_mut_ptr(), MAX_RATES as c_int) };
    if n <= 0 {
        return Vec::new();
    }
    serials[..(n as usize).min(MAX_RATES)]
        .iter()
        .map(|sn| DeviceInfo {
            driver: "airspy",
            label: "Airspy".into(),
            id: format!("{sn:016X}"),
        })
        .collect()
}

/// Opens an Airspy. With no id, the first radio found. With an id, the radio with that serial.
/// The radio starts tuned to 100 MHz at its highest listed rate.
pub fn open(id: Option<&str>) -> Result<Box<dyn RxRadio>, String> {
    let api = Api::load()?;
    // SAFETY: init takes no arguments and returns a status code.
    api.check(unsafe { (api.init)() }, "starting the library")?;

    let serial = match id.map(str::trim).filter(|s| !s.is_empty()) {
        Some(text) => Some(parse_serial(text)?),
        None => None,
    };
    let mut dev: Dev = std::ptr::null_mut();
    // SAFETY: `dev` is a valid place for the handle the library writes.
    let rc = unsafe {
        match serial {
            Some(sn) => (api.open_sn)(&mut dev, sn),
            None => (api.open)(&mut dev),
        }
    };
    if rc != 0 || dev.is_null() {
        let which = match serial {
            Some(sn) => format!("serial {sn:016X}"),
            None => "the first radio".to_string(),
        };
        return Err(format!(
            "Airspy: cannot open {which}: {}. Check the USB cable, and that no other program is using the radio",
            api.describe(rc)
        ));
    }

    let mut radio = AirspyRadio {
        dev,
        shared: Arc::new(Shared {
            queue: Mutex::new(VecDeque::new()),
            ready: Condvar::new(),
            dropped: AtomicU64::new(0),
        }),
        rates: Vec::new(),
        rate: 0,
        streaming: false,
        stages: stage_list(&api),
        api,
    };

    // SAFETY: the handle is open and the value is the library's float pair type.
    let rc = unsafe { (radio.api.set_sample_type)(dev, SAMPLE_TYPE_FLOAT32_IQ) };
    radio.api.check(rc, "selecting the sample format")?;

    // SAFETY: with length 0 the library writes the count to the first argument.
    let mut count: u32 = 0;
    let rc = unsafe { (radio.api.get_samplerates)(dev, &mut count, 0) };
    radio.api.check(rc, "reading the sample rates")?;
    let mut rates = vec![0u32; (count as usize).min(MAX_RATES)];
    if !rates.is_empty() {
        // SAFETY: the buffer holds `rates.len()` entries.
        let rc = unsafe { (radio.api.get_samplerates)(dev, rates.as_mut_ptr(), rates.len() as u32) };
        radio.api.check(rc, "reading the sample rates")?;
    }
    rates.retain(|&r| r > 0);
    rates.sort_unstable();
    rates.dedup();
    if rates.is_empty() {
        return Err("Airspy: the radio lists no sample rates".into());
    }
    radio.rates = rates;

    radio.set_device_frequency(DEFAULT_FREQ_HZ)?;
    let top = *radio.rates.last().unwrap_or(&0);
    radio.apply_rate(top)?;
    Ok(Box::new(radio))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_choice_takes_the_closest_rate_not_below() {
        assert_eq!(pick_rate(&[2_500_000, 10_000_000], 2_000_000), Some(2_500_000));
        assert_eq!(pick_rate(&[10_000_000, 2_500_000], 5_000_000), Some(10_000_000));
        assert_eq!(pick_rate(&[3_000_000, 6_000_000], 6_000_000), Some(6_000_000));
    }

    #[test]
    fn rate_choice_takes_the_highest_when_none_is_high_enough() {
        assert_eq!(pick_rate(&[3_000_000, 6_000_000], 10_000_000), Some(6_000_000));
    }

    #[test]
    fn rate_choice_ignores_zero_and_duplicates() {
        assert_eq!(pick_rate(&[0, 6_000_000, 6_000_000], 1), Some(6_000_000));
        assert_eq!(pick_rate(&[], 1_000_000), None);
        assert_eq!(pick_rate(&[0], 1_000_000), None);
    }

    #[test]
    fn gain_is_clamped_and_rounded() {
        assert_eq!(clamp_gain(-5.0, 0.0, LINEARITY_MAX), 0);
        assert_eq!(clamp_gain(99.0, 0.0, LINEARITY_MAX), 21);
        assert_eq!(clamp_gain(7.4, 0.0, LINEARITY_MAX), 7);
        assert_eq!(clamp_gain(7.6, 0.0, LINEARITY_MAX), 8);
        assert_eq!(clamp_gain(15.0, 0.0, STAGE_MAX), 15);
        assert_eq!(clamp_gain(16.0, 0.0, STAGE_MAX), 15);
        assert_eq!(clamp_gain(f32::NAN, 0.0, LINEARITY_MAX), 0);
    }

    #[test]
    fn samples_convert_to_pairs_within_range() {
        let raw = [0.5, -0.25, 2.0, f32::NAN, -3.0, 0.0, 0.1];
        assert_eq!(convert_iq(&raw), vec![(0.5, -0.25), (1.0, 0.0), (-1.0, 0.0)]);
        assert!(convert_iq(&[]).is_empty());
    }

    #[test]
    fn queue_drops_what_does_not_fit() {
        let mut q = VecDeque::new();
        let block: Vec<(f32, f32)> = (0..5).map(|i| (i as f32, 0.0)).collect();
        assert_eq!(push_capped(&mut q, &block, 3), 2);
        assert_eq!(q.len(), 3);
        assert_eq!(push_capped(&mut q, &block, 3), 5);
    }

    #[test]
    fn missing_library_says_what_to_install() {
        let err = load_with_names(&["/nonexistent/path/libairspy-test.dylib"])
            .expect_err("the library must not load");
        assert!(
            err.starts_with("Airspy: the libairspy host library was not found"),
            "{err}"
        );
        assert!(err.contains("Install libairspy"), "{err}");
        assert!(err.contains("libairspy-test.dylib"), "{err}");
    }

    #[test]
    fn empty_library_list_is_reported() {
        let err = load_with_names(&[]).expect_err("no names, no library");
        assert!(err.contains("no library name is known"), "{err}");
    }

    #[test]
    fn error_text_names_the_code_and_the_library_name() {
        assert_eq!(
            error_text(-2, Some("AIRSPY_ERROR_INVALID_PARAM")),
            "libairspy error -2 AIRSPY_ERROR_INVALID_PARAM"
        );
        assert_eq!(error_text(-1, None), "libairspy error -1");
        assert_eq!(error_text(-1, Some("")), "libairspy error -1");
    }

    #[test]
    fn serial_parses_hex_only() {
        assert_eq!(parse_serial("0x1F"), Ok(0x1F));
        assert_eq!(parse_serial("1f"), Ok(0x1F));
        assert!(parse_serial("zz").is_err());
    }

    #[test]
    fn frequency_range_is_the_tuner_range() {
        assert_eq!(FREQ_MIN_HZ, 24_000_000);
        assert_eq!(FREQ_MAX_HZ, 1_800_000_000);
    }
}
