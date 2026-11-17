//! Driver for the RTL-SDR (RTL2832U dongles, including the RTL-SDR Blog V4), through librtlsdr loaded at run time.
//!
//! Receive only. Most dongles drop samples above about 2.56 Msps, so the offered rates stop there.
//! On macOS the samples are read in blocks (rtlsdr_read_sync) instead of rtlsdr_read_async: cancelling an
//! asynchronous read there can leave a USB transfer in flight, and the next control call then crashes.
//! Frequencies below 24 MHz (HF) are not handled here: the tuner cannot reach them without direct sampling.

use std::ffi::{c_char, c_int, c_void, CStr};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use libloading::Library;

use super::common::load_library;
use crate::{DeviceInfo, GainSetting, GainStage, RxRadio};

const DRIVER: &str = "rtlsdr";
/// The one gain stage the dongle has (the tuner's own gain steps).
const STAGE: &str = "tuner";
/// How long read_iq waits for samples before it returns 0.
const READ_WAIT: Duration = Duration::from_millis(200);
/// Blocks queued between the reader thread and read_iq: 32 x 256 KiB, about 2 s at 2.048 Msps.
const QUEUED_BLOCKS: usize = 32;
const DEFAULT_RATE: u32 = 2_048_000;
/// Rates the dongle offers. 0.3 to 0.9 Msps is refused by librtlsdr, so 900001 Hz is the lowest.
const RATES: [u32; 8] = [900_001, 1_024_000, 1_200_000, 1_536_000, 1_800_000, 2_048_000, 2_400_000, 2_560_000];
const MAX_DEVICES: u32 = 16;
/// Gain set when the radio opens, in dB. The operator can change it with set_gain.
const OPEN_GAIN_DB: f32 = 30.0;
const LOST: &str = "the RTL-SDR stopped sending samples. It may have been unplugged: plug it in and start again.";

#[cfg(target_os = "macos")]
const LIB_NAMES: &[&str] = &["librtlsdr.dylib", "/opt/homebrew/lib/librtlsdr.dylib", "/usr/local/lib/librtlsdr.dylib"];
#[cfg(windows)]
const LIB_NAMES: &[&str] = &["rtlsdr.dll", "librtlsdr.dll"];
#[cfg(not(any(target_os = "macos", windows)))]
const LIB_NAMES: &[&str] = &["librtlsdr.so.2", "librtlsdr.so"];
/// True on macOS: the stream is read in blocks there (see the module comment).
const BLOCK_READS: bool = cfg!(target_os = "macos");

type GetCount = unsafe extern "C" fn() -> u32;
type GetName = unsafe extern "C" fn(u32) -> *const c_char;
type GetStrings = unsafe extern "C" fn(u32, *mut c_char, *mut c_char, *mut c_char) -> c_int;
type OpenFn = unsafe extern "C" fn(*mut *mut c_void, u32) -> c_int;
type CloseFn = unsafe extern "C" fn(*mut c_void) -> c_int;
type SetFreq = unsafe extern "C" fn(*mut c_void, u32) -> c_int;
type SetRate = unsafe extern "C" fn(*mut c_void, u32) -> c_int;
type GetRate = unsafe extern "C" fn(*mut c_void) -> u32;
type SetGainMode = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
type GetGains = unsafe extern "C" fn(*mut c_void, *mut c_int) -> c_int;
type SetGain = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
type SetAgc = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
type ResetBuffer = unsafe extern "C" fn(*mut c_void) -> c_int;
type BlockFn = unsafe extern "C" fn(*mut u8, u32, *mut c_void);
type ReadAsync = unsafe extern "C" fn(*mut c_void, BlockFn, *mut c_void, u32, u32) -> c_int;
type CancelAsync = unsafe extern "C" fn(*mut c_void) -> c_int;
type ReadSync = unsafe extern "C" fn(*mut c_void, *mut c_void, c_int, *mut c_int) -> c_int;
type TunerType = unsafe extern "C" fn(*mut c_void) -> c_int;
type SetPpm = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;

/// The librtlsdr functions this driver uses. `_lib` keeps the library loaded while they are in use.
struct Api {
    count: GetCount,
    name: GetName,
    strings: GetStrings,
    open: OpenFn,
    close: CloseFn,
    set_freq: SetFreq,
    set_rate: SetRate,
    get_rate: GetRate,
    set_gain_mode: SetGainMode,
    get_gains: GetGains,
    set_gain: SetGain,
    set_agc: SetAgc,
    reset_buffer: ResetBuffer,
    read_async: ReadAsync,
    cancel_async: CancelAsync,
    read_sync: Option<ReadSync>,
    tuner_type: Option<TunerType>,
    set_ppm: Option<SetPpm>,
    _lib: Library,
}

/// An open dongle. The pointer is only used through librtlsdr, which allows control calls from one thread
/// while the reader thread streams.
#[derive(Clone, Copy)]
struct Handle(*mut c_void);
// SAFETY: see the type comment; the library takes care of its own locking for control calls.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

unsafe fn sym<T: Copy>(lib: &Library, name: &'static [u8]) -> Result<T, String> {
    match lib.get::<T>(name) {
        Ok(s) => Ok(*s),
        Err(e) => Err(format!("the RTL-SDR library has no {}: {e}", String::from_utf8_lossy(&name[..name.len() - 1]))),
    }
}

unsafe fn sym_opt<T: Copy>(lib: &Library, name: &'static [u8]) -> Option<T> {
    lib.get::<T>(name).ok().map(|s| *s)
}

fn load_api() -> Result<Api, String> {
    let lib = load_library(LIB_NAMES)
        .map_err(|e| format!("{e}. Install the RTL-SDR driver library (librtlsdr) and try again."))?;
    // SAFETY: each symbol is looked up with the signature librtlsdr documents.
    unsafe {
        Ok(Api {
            count: sym(&lib, b"rtlsdr_get_device_count\0")?,
            name: sym(&lib, b"rtlsdr_get_device_name\0")?,
            strings: sym(&lib, b"rtlsdr_get_device_usb_strings\0")?,
            open: sym(&lib, b"rtlsdr_open\0")?,
            close: sym(&lib, b"rtlsdr_close\0")?,
            set_freq: sym(&lib, b"rtlsdr_set_center_freq\0")?,
            set_rate: sym(&lib, b"rtlsdr_set_sample_rate\0")?,
            get_rate: sym(&lib, b"rtlsdr_get_sample_rate\0")?,
            set_gain_mode: sym(&lib, b"rtlsdr_set_tuner_gain_mode\0")?,
            get_gains: sym(&lib, b"rtlsdr_get_tuner_gains\0")?,
            set_gain: sym(&lib, b"rtlsdr_set_tuner_gain\0")?,
            set_agc: sym(&lib, b"rtlsdr_set_agc_mode\0")?,
            reset_buffer: sym(&lib, b"rtlsdr_reset_buffer\0")?,
            read_async: sym(&lib, b"rtlsdr_read_async\0")?,
            cancel_async: sym(&lib, b"rtlsdr_cancel_async\0")?,
            read_sync: sym_opt(&lib, b"rtlsdr_read_sync\0"),
            tuner_type: sym_opt(&lib, b"rtlsdr_get_tuner_type\0"),
            set_ppm: sym_opt(&lib, b"rtlsdr_set_freq_correction\0"),
            _lib: lib,
        })
    }
}

fn c_text(buf: &[u8]) -> String {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).trim().to_string()
}

/// Manufacturer, product and serial of device `index`, or None when the library cannot read them.
fn usb_strings(api: &Api, index: u32) -> Option<(String, String, String)> {
    let mut manufacturer = [0u8; 256];
    let mut product = [0u8; 256];
    let mut serial = [0u8; 256];
    // SAFETY: each buffer is 256 bytes, which is more than the library writes.
    let rc = unsafe {
        (api.strings)(index, manufacturer.as_mut_ptr().cast(), product.as_mut_ptr().cast(), serial.as_mut_ptr().cast())
    };
    if rc != 0 {
        return None;
    }
    Some((c_text(&manufacturer), c_text(&product), c_text(&serial)))
}

/// Devices in the library's order. The position is the index librtlsdr_open takes.
fn list(api: &Api) -> Vec<DeviceInfo> {
    // SAFETY: plain query with no arguments.
    let count = unsafe { (api.count)() }.min(MAX_DEVICES);
    (0..count)
        .map(|index| {
            let (product, serial) = match usb_strings(api, index) {
                Some((_, product, serial)) => (product, serial),
                None => (String::new(), String::new()),
            };
            let label = if product.is_empty() { fallback_name(api, index) } else { product };
            DeviceInfo { driver: DRIVER, label, id: serial }
        })
        .collect()
}

fn fallback_name(api: &Api, index: u32) -> String {
    // SAFETY: the library returns a pointer to a static name, or null.
    let p = unsafe { (api.name)(index) };
    if p.is_null() {
        return "RTL-SDR".into();
    }
    // SAFETY: checked for null above; the library's name is a C string.
    let name = unsafe { CStr::from_ptr(p) }.to_string_lossy().trim().to_string();
    if name.is_empty() { "RTL-SDR".into() } else { name }
}

/// Finds the radios this computer can use. Returns quickly; an absent library gives an empty list.
pub fn probe() -> Vec<DeviceInfo> {
    match load_api() {
        Ok(api) => list(&api),
        Err(_) => Vec::new(),
    }
}

/// Opens the RTL-SDR with serial `id`, or the first one when `id` is empty.
pub fn open(id: Option<&str>) -> Result<Box<dyn RxRadio>, String> {
    let api = Arc::new(load_api()?);
    let devices = list(&api);
    let plugged = if devices.is_empty() {
        "none".to_string()
    } else {
        devices.iter().map(|d| d.id.clone()).collect::<Vec<_>>().join(", ")
    };
    let index = match id.map(str::trim).filter(|s| !s.is_empty()) {
        Some(serial) => devices.iter().position(|d| d.id == serial).ok_or_else(|| {
            format!("no RTL-SDR with serial {serial:?} is plugged in (plugged in: {plugged}). Use one of those serials, or leave the id empty for the first radio.")
        })?,
        None => {
            if devices.is_empty() {
                return Err("no RTL-SDR found. Plug the radio into a USB port and try again.".into());
            }
            0
        }
    };
    let label = devices[index].label.clone();
    let index = index as u32;

    let mut dev: *mut c_void = ptr::null_mut();
    // SAFETY: the library writes the device pointer into `dev` on success.
    let rc = unsafe { (api.open)(&mut dev, index) };
    if rc != 0 || dev.is_null() {
        return Err(format!(
            "cannot open the RTL-SDR (error {rc}). Unplug it and plug it in again. If it still fails, another program may be using the radio: close that program and retry. On Linux, check the USB rules for RTL-SDR devices."
        ));
    }
    let handle = Handle(dev);
    // SAFETY: the handle is open; control calls only.
    let tuner = unsafe {
        (api.set_agc)(dev, 0);          // the chip's own AGC stays off: the operator sets the gain
        (api.set_gain_mode)(dev, 1);    // manual tuner gain
        if let Some(ppm) = api.set_ppm {
            ppm(dev, 0);
        }
        api.tuner_type.map_or(0, |f| f(dev))
    };
    let mut radio = RtlRadio {
        api: api.clone(),
        dev: handle,
        label,
        range: tuner_range(tuner),
        gains: read_gains(&api, dev),
        rate: 0,
        stream: None,
        queue: SampleQueue::default(),
        dropped: Arc::new(AtomicU64::new(0)),
    };
    radio.set_sample_rate(DEFAULT_RATE)?;
    if !radio.gains.is_empty() {
        radio.set_gain(STAGE, GainSetting::Manual(OPEN_GAIN_DB))?;
    }
    Ok(Box::new(radio))
}

fn read_gains(api: &Api, dev: *mut c_void) -> Vec<i32> {
    // SAFETY: a null buffer asks for the count only.
    let n = unsafe { (api.get_gains)(dev, ptr::null_mut()) };
    if n <= 0 {
        return Vec::new();
    }
    let mut gains = vec![0i32; n as usize];
    // SAFETY: the buffer holds `n` values, as the count said.
    unsafe { (api.get_gains)(dev, gains.as_mut_ptr()) };
    gains.sort_unstable();
    gains
}

/// Tuning range in Hz for a tuner type (librtlsdr's enum rtlsdr_tuner). Unknown tuners get the R820T range.
fn tuner_range(tuner: i32) -> (u64, u64) {
    match tuner {
        1 => (52_000_000, 2_200_000_000),  // E4000
        2 => (22_000_000, 948_600_000),    // FC0012
        3 => (22_000_000, 1_100_000_000),  // FC0013
        4 => (146_000_000, 924_000_000),   // FC2580
        _ => (24_000_000, 1_766_000_000),  // R820T / R828D (below 24 MHz needs direct sampling, not written)
    }
}

fn nearest_rate(hz: u32) -> u32 {
    RATES.iter().copied().min_by_key(|r| r.abs_diff(hz)).unwrap_or(DEFAULT_RATE)
}

/// Gain in tenths of a dB, the closest the tuner offers to `db`. `gains` must not be empty.
fn nearest_gain(gains: &[i32], db: f32) -> i32 {
    let want = (db * 10.0).round() as i32;
    gains.iter().copied().min_by_key(|g| (g - want).abs()).unwrap_or(0)
}

/// Bytes per read in the macOS block mode: about 0.1 s of samples, in whole 16 KiB units.
fn block_bytes(rate: u32) -> usize {
    let blocks = ((rate as usize * 2) / 10 + 8192) / 16384;
    blocks.clamp(1, 16) * 16384
}

fn to_f32(b: u8) -> f32 {
    (b as f32 - 127.4) / 128.0
}

fn mhz(hz: u64) -> String {
    format!("{:.3}", hz as f64 / 1e6)
}

/// Sends one block to the reader. A full queue drops the block and counts its samples.
/// Returns false when nobody reads any more.
fn send_block(tx: &SyncSender<Vec<u8>>, dropped: &AtomicU64, data: &[u8]) -> bool {
    match tx.try_send(data.to_vec()) {
        Ok(()) => true,
        Err(TrySendError::Full(_)) => {
            dropped.fetch_add((data.len() / 2) as u64, Ordering::Relaxed);
            true
        }
        Err(TrySendError::Disconnected(_)) => false,
    }
}

/// What the library's asynchronous callback needs. Lives on the reader thread's stack frame for the whole read.
struct CallbackCtx {
    tx: SyncSender<Vec<u8>>,
    dropped: Arc<AtomicU64>,
    running: Arc<AtomicBool>,
    dev: Handle,
    cancel: CancelAsync,
}

unsafe extern "C" fn on_block(buf: *mut u8, len: u32, ctx: *mut c_void) {
    // SAFETY: ctx is the CallbackCtx that run_async passed to read_async, alive until read_async returns.
    let c = &*(ctx as *const CallbackCtx);
    if !c.running.load(Ordering::Relaxed) {
        (c.cancel)(c.dev.0);
        return;
    }
    // SAFETY: the library gives `len` valid bytes at `buf` for the duration of this call.
    let data = std::slice::from_raw_parts(buf, len as usize);
    if !send_block(&c.tx, &c.dropped, data) {
        (c.cancel)(c.dev.0);
    }
}

struct StreamJob {
    api: Arc<Api>,
    dev: Handle,
    tx: SyncSender<Vec<u8>>,
    running: Arc<AtomicBool>,
    dropped: Arc<AtomicU64>,
    rate: u32,
}

impl StreamJob {
    fn run(self) {
        match (BLOCK_READS, self.api.read_sync) {
            (true, Some(read_sync)) => self.run_blocks(read_sync),
            _ => self.run_async(),
        }
    }

    fn run_blocks(&self, read_sync: ReadSync) {
        let bytes = block_bytes(self.rate);
        let mut buf = vec![0u8; bytes];
        while self.running.load(Ordering::Relaxed) {
            let mut got: c_int = 0;
            // SAFETY: buf holds `bytes` bytes; the handle stays open until the reader thread has been joined.
            let rc = unsafe { read_sync(self.dev.0, buf.as_mut_ptr().cast(), bytes as c_int, &mut got) };
            if rc < 0 {
                break;
            }
            if got > 0 && !send_block(&self.tx, &self.dropped, &buf[..got as usize]) {
                break;
            }
        }
    }

    fn run_async(self) {
        let ctx = Box::into_raw(Box::new(CallbackCtx {
            tx: self.tx.clone(),
            dropped: self.dropped.clone(),
            running: self.running.clone(),
            dev: self.dev,
            cancel: self.api.cancel_async,
        }));
        // SAFETY: the library calls on_block only inside read_async, so ctx is freed after it returns.
        unsafe {
            (self.api.read_async)(self.dev.0, on_block, ctx.cast(), 0, 0);
            drop(Box::from_raw(ctx));
        }
    }
}

struct Stream {
    running: Arc<AtomicBool>,
    rx: Receiver<Vec<u8>>,
    thread: JoinHandle<()>,
}

/// Bytes taken from the queue but not yet turned into samples. Holds at most one byte between calls.
#[derive(Default)]
struct SampleQueue {
    chunk: Vec<u8>,
    at: usize,
}

impl SampleQueue {
    /// Fills `out` with complex samples. Waits up to READ_WAIT for the first block, then takes what is ready.
    fn drain(&mut self, rx: &Receiver<Vec<u8>>, out: &mut [(f32, f32)]) -> Result<usize, String> {
        let mut n = 0;
        while n < out.len() {
            if self.at + 2 > self.chunk.len() {
                let next = if n == 0 {
                    rx.recv_timeout(READ_WAIT)
                } else {
                    rx.try_recv().map_err(|e| match e {
                        mpsc::TryRecvError::Empty => RecvTimeoutError::Timeout,
                        mpsc::TryRecvError::Disconnected => RecvTimeoutError::Disconnected,
                    })
                };
                match next {
                    Ok(mut block) => {
                        if self.at < self.chunk.len() {
                            // one byte left over from the last block: it goes in front of the new one
                            let mut joined = self.chunk[self.at..].to_vec();
                            joined.append(&mut block);
                            block = joined;
                        }
                        self.chunk = block;
                        self.at = 0;
                    }
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => {
                        if n == 0 {
                            return Err(LOST.into());
                        }
                        break;
                    }
                }
                continue;
            }
            let take = ((self.chunk.len() - self.at) / 2).min(out.len() - n);
            for k in 0..take {
                let i = self.at + 2 * k;
                out[n + k] = (to_f32(self.chunk[i]), to_f32(self.chunk[i + 1]));
            }
            self.at += 2 * take;
            n += take;
        }
        Ok(n)
    }
}

pub struct RtlRadio {
    api: Arc<Api>,
    dev: Handle,
    label: String,
    range: (u64, u64),
    /// Tuner gain steps in tenths of a dB, ascending. Empty when the library gives none.
    gains: Vec<i32>,
    rate: u32,
    stream: Option<Stream>,
    queue: SampleQueue,
    dropped: Arc<AtomicU64>,
}

impl RtlRadio {
    fn uses_blocks(&self) -> bool {
        BLOCK_READS && self.api.read_sync.is_some()
    }

    fn stop_stream(&mut self) {
        let Some(stream) = self.stream.take() else { return };
        stream.running.store(false, Ordering::Relaxed);
        // Wait for the reader to finish. Cancelling again on each pass covers a read that started just after the first cancel.
        while !stream.thread.is_finished() {
            if !self.uses_blocks() {
                // SAFETY: the handle is still open (close happens only in Drop, after this).
                unsafe { (self.api.cancel_async)(self.dev.0) };
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = stream.thread.join();
        self.queue = SampleQueue::default();
    }
}

impl Drop for RtlRadio {
    fn drop(&mut self) {
        self.stop_stream();
        // SAFETY: the reader has finished, so nothing else uses the handle.
        unsafe { (self.api.close)(self.dev.0) };
    }
}

impl RxRadio for RtlRadio {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn freq_range_hz(&self) -> (u64, u64) {
        self.range
    }

    fn set_frequency_hz(&mut self, hz: u64) -> Result<(), String> {
        let (lo, hi) = self.range;
        if hz < lo || hz > hi {
            return Err(format!(
                "{} MHz is outside what this RTL-SDR tunes ({} to {} MHz). Pick a frequency in that range.",
                mhz(hz),
                mhz(lo),
                mhz(hi)
            ));
        }
        // SAFETY: hz is within the tuner range, so it fits the library's 32-bit frequency.
        let rc = unsafe { (self.api.set_freq)(self.dev.0, hz as u32) };
        if rc != 0 {
            return Err(format!("the RTL-SDR refused {} MHz (error {rc}). Check the frequency and try again.", mhz(hz)));
        }
        Ok(())
    }

    fn sample_rates(&self) -> Vec<u32> {
        RATES.to_vec()
    }

    fn set_sample_rate(&mut self, hz: u32) -> Result<u32, String> {
        let want = nearest_rate(hz);
        // SAFETY: control call on the open handle.
        let rc = unsafe { (self.api.set_rate)(self.dev.0, want) };
        if rc != 0 {
            return Err(format!("the RTL-SDR refused the sample rate {want} Hz (error {rc}). Use one of the offered rates."));
        }
        // SAFETY: plain query on the open handle.
        let got = unsafe { (self.api.get_rate)(self.dev.0) };
        self.rate = if got == 0 { want } else { got };
        if self.stream.is_some() {
            // SAFETY: control call on the open handle.
            unsafe { (self.api.reset_buffer)(self.dev.0) };
        }
        Ok(self.rate)
    }

    fn gain_stages(&self) -> Vec<GainStage> {
        match (self.gains.iter().min(), self.gains.iter().max()) {
            (Some(lo), Some(hi)) => vec![GainStage {
                name: STAGE.into(),
                min_db: *lo as f32 / 10.0,
                max_db: *hi as f32 / 10.0,
                step_db: 0.0,
                auto: true,
            }],
            _ => Vec::new(),
        }
    }

    fn set_gain(&mut self, stage: &str, setting: GainSetting) -> Result<(), String> {
        if stage != STAGE {
            return Err(format!("this radio has no gain stage named {stage:?}; it has: {STAGE}"));
        }
        let (mode, tenths) = match setting {
            GainSetting::Auto => (0, None),
            GainSetting::Manual(db) => {
                if self.gains.is_empty() {
                    return Err("this RTL-SDR's tuner reports no gain steps, so a manual gain cannot be set. Use auto.".into());
                }
                (1, Some(nearest_gain(&self.gains, db)))
            }
        };
        // SAFETY: control calls on the open handle.
        let rc = unsafe { (self.api.set_gain_mode)(self.dev.0, mode) };
        if rc != 0 {
            return Err(format!("the RTL-SDR refused the gain mode (error {rc})."));
        }
        if let Some(t) = tenths {
            let rc = unsafe { (self.api.set_gain)(self.dev.0, t) };
            if rc != 0 {
                return Err(format!("the RTL-SDR refused the gain of {:.1} dB (error {rc}).", t as f32 / 10.0));
            }
        }
        Ok(())
    }

    fn start(&mut self) -> Result<(), String> {
        if self.stream.is_some() {
            return Ok(());
        }
        // SAFETY: control call on the open handle; the reader starts after it.
        let rc = unsafe { (self.api.reset_buffer)(self.dev.0) };
        if rc != 0 {
            return Err(format!("the RTL-SDR would not reset its buffer (error {rc}). Unplug it and plug it in again."));
        }
        let (tx, rx) = mpsc::sync_channel(QUEUED_BLOCKS);
        let running = Arc::new(AtomicBool::new(true));
        let job = StreamJob {
            api: self.api.clone(),
            dev: self.dev,
            tx,
            running: running.clone(),
            dropped: self.dropped.clone(),
            rate: self.rate,
        };
        let thread = std::thread::Builder::new()
            .name("rtlsdr-read".into())
            .spawn(move || job.run())
            .map_err(|e| format!("could not start the sample reader thread: {e}"))?;
        self.queue = SampleQueue::default();
        self.stream = Some(Stream { running, rx, thread });
        Ok(())
    }

    fn read_iq(&mut self, out: &mut [(f32, f32)]) -> Result<usize, String> {
        match self.stream.as_ref() {
            Some(stream) => self.queue.drain(&stream.rx, out),
            None => Err("the RTL-SDR is not streaming. Call start first.".into()),
        }
    }

    fn stop(&mut self) {
        self.stop_stream();
    }

    fn dropped_samples(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_is_the_closest_offered() {
        assert_eq!(nearest_rate(2_000_000), 2_048_000);
        assert_eq!(nearest_rate(100), 900_001);
        assert_eq!(nearest_rate(9_000_000), 2_560_000);
    }

    #[test]
    fn gain_is_the_closest_step() {
        let gains = [0, 100, 200, 496];
        assert_eq!(nearest_gain(&gains, 20.0), 200);
        assert_eq!(nearest_gain(&gains, 49.6), 496);
        assert_eq!(nearest_gain(&gains, 99.0), 496);
    }

    #[test]
    fn block_is_about_a_tenth_of_a_second() {
        assert_eq!(block_bytes(2_048_000), 262_144);
        assert_eq!(block_bytes(900_001), 180_224);
        assert_eq!(block_bytes(1), 16_384);
    }

    #[test]
    fn tuner_ranges() {
        assert_eq!(tuner_range(5), (24_000_000, 1_766_000_000));
        assert_eq!(tuner_range(0), (24_000_000, 1_766_000_000));
        assert_eq!(tuner_range(1), (52_000_000, 2_200_000_000));
    }

    #[test]
    fn c_strings_stop_at_nul_and_trim() {
        assert_eq!(c_text(b"Blog V4\0\0junk"), "Blog V4");
        assert_eq!(c_text(b"  00000001 \0"), "00000001");
    }

    #[test]
    fn odd_byte_is_kept_for_the_next_block() {
        let (tx, rx) = mpsc::sync_channel(4);
        tx.send(vec![1, 2, 3]).unwrap();
        tx.send(vec![4, 5]).unwrap();
        let mut q = SampleQueue::default();
        let mut out = [(0.0f32, 0.0f32); 3];
        let n = q.drain(&rx, &mut out).unwrap();
        assert_eq!(n, 2);
        assert_eq!(out[0], (to_f32(1), to_f32(2)));
        assert_eq!(out[1], (to_f32(3), to_f32(4)));
    }

    #[test]
    fn empty_stream_returns_zero_after_the_wait() {
        let (_tx, rx) = mpsc::sync_channel::<Vec<u8>>(4);
        let mut q = SampleQueue::default();
        let mut out = [(0.0f32, 0.0f32); 8];
        assert_eq!(q.drain(&rx, &mut out).unwrap(), 0);
    }

    #[test]
    fn unknown_serial_is_an_error_and_probe_does_not_panic() {
        let _ = probe();
        assert!(open(Some("no-such-serial-zz")).is_err());
    }
}
