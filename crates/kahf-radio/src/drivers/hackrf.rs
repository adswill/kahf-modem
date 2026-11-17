//! Driver for HackRF One, HackRF Pro, Jawbreaker and rad1o receivers over libhackrf (receive only).
//!
//! The vendor library is loaded at run time, so the program starts without it. Samples arrive as
//! signed 8-bit I/Q pairs and are scaled to -1..1 by dividing by 128.
//!
//! Order of setup: sample rate, then baseband filter (the widest filter at or below half the rate),
//! then frequency, then LNA, VGA and the amplifier. Streaming starts last.

use std::collections::VecDeque;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use libloading::Library;

use crate::drivers::common::load_library;
use crate::{DeviceInfo, GainSetting, GainStage, RxRadio};

const SUCCESS: c_int = 0;
/// libhackrf's code for "no device found".
const ERROR_NOT_FOUND: c_int = -5;
/// Longest wait of one `read_iq` call.
const READ_WAIT: Duration = Duration::from_millis(200);
/// Backlog cap in samples (about 0.4 s at 10 Msps). The oldest samples go first on overflow.
const BACKLOG_LIMIT: usize = 1 << 22;
/// Sample rates offered, ascending. The radio takes 2 to 20 Msps.
const RATES_HZ: [u32; 8] = [
    2_000_000, 4_000_000, 5_000_000, 8_000_000, 10_000_000, 12_500_000, 16_000_000, 20_000_000,
];
const DEFAULT_RATE_HZ: u32 = 10_000_000;
const DEFAULT_FREQ_HZ: u64 = 100_000_000;
const DEFAULT_LNA_DB: u32 = 16;
const DEFAULT_VGA_DB: u32 = 20;
/// Baseband filter widths of the MAX2837 chip, ascending.
const FILTERS_HZ: [u32; 16] = [
    1_750_000, 2_500_000, 3_500_000, 5_000_000, 5_500_000, 6_000_000, 7_000_000, 8_000_000, 9_000_000,
    10_000_000, 12_000_000, 14_000_000, 15_000_000, 20_000_000, 24_000_000, 28_000_000,
];
/// Board id for "not known" as libhackrf reports it.
const BOARD_UNKNOWN: u8 = 0xff;

#[cfg(target_os = "macos")]
const LIBRARY_NAMES: &[&str] = &[
    "libhackrf.dylib",
    "/opt/homebrew/lib/libhackrf.dylib",
    "/usr/local/lib/libhackrf.dylib",
];
#[cfg(all(unix, not(target_os = "macos")))]
const LIBRARY_NAMES: &[&str] = &["libhackrf.so.0", "libhackrf.so"];
#[cfg(windows)]
const LIBRARY_NAMES: &[&str] = &["hackrf.dll"];

type Device = *mut c_void;
type RxCallback = unsafe extern "C" fn(*mut Transfer) -> c_int;

/// libhackrf's `hackrf_transfer`, passed to the receive callback.
#[repr(C)]
struct Transfer {
    device: *mut c_void,
    buffer: *mut u8,
    buffer_length: c_int,
    valid_length: c_int,
    rx_context: *mut c_void,
    tx_context: *mut c_void,
}

/// libhackrf's `hackrf_device_list_t`. Only the serial numbers and board ids are read.
#[repr(C)]
struct DeviceList {
    serial_numbers: *mut *mut c_char,
    usb_board_ids: *mut c_int,
    usb_device_index: *mut c_int,
    device_count: c_int,
    usb_devices: *mut *mut c_void,
    usb_device_count: c_int,
}

/// Functions of libhackrf, copied out of the loaded library. The library stays loaded for the whole run.
struct Api {
    _library: Library,
    open: unsafe extern "C" fn(*mut Device) -> c_int,
    open_by_serial: unsafe extern "C" fn(*const c_char, *mut Device) -> c_int,
    close: unsafe extern "C" fn(Device) -> c_int,
    board_id_read: unsafe extern "C" fn(Device, *mut u8) -> c_int,
    set_sample_rate: unsafe extern "C" fn(Device, f64) -> c_int,
    set_filter_bandwidth: unsafe extern "C" fn(Device, u32) -> c_int,
    set_freq: unsafe extern "C" fn(Device, u64) -> c_int,
    set_lna_gain: unsafe extern "C" fn(Device, u32) -> c_int,
    set_vga_gain: unsafe extern "C" fn(Device, u32) -> c_int,
    set_amp_enable: unsafe extern "C" fn(Device, u8) -> c_int,
    start_rx: unsafe extern "C" fn(Device, RxCallback, *mut c_void) -> c_int,
    stop_rx: unsafe extern "C" fn(Device) -> c_int,
    is_streaming: unsafe extern "C" fn(Device) -> c_int,
    device_list: unsafe extern "C" fn() -> *mut DeviceList,
    device_list_free: unsafe extern "C" fn(*mut DeviceList),
    error_name: unsafe extern "C" fn(c_int) -> *const c_char,
}

/// Loads one function by name. The function pointer is copied out; the library must stay loaded.
fn symbol<T: Copy>(library: &Library, name: &str) -> Result<T, String> {
    let mut bytes = name.as_bytes().to_vec();
    bytes.push(0);
    // SAFETY: the name is a libhackrf function; the caller gives the matching function type.
    let found = unsafe { library.get::<T>(&bytes) };
    match found {
        Ok(sym) => Ok(*sym),
        Err(e) => Err(format!("libhackrf has no function {name} ({e})")),
    }
}

fn load_api() -> Result<Api, String> {
    let library = load_library(LIBRARY_NAMES)
        .map_err(|e| format!("HackRF library not found; install libhackrf ({e})"))?;
    let api = Api {
        open: symbol(&library, "hackrf_open")?,
        open_by_serial: symbol(&library, "hackrf_open_by_serial")?,
        close: symbol(&library, "hackrf_close")?,
        board_id_read: symbol(&library, "hackrf_board_id_read")?,
        set_sample_rate: symbol(&library, "hackrf_set_sample_rate")?,
        set_filter_bandwidth: symbol(&library, "hackrf_set_baseband_filter_bandwidth")?,
        set_freq: symbol(&library, "hackrf_set_freq")?,
        set_lna_gain: symbol(&library, "hackrf_set_lna_gain")?,
        set_vga_gain: symbol(&library, "hackrf_set_vga_gain")?,
        set_amp_enable: symbol(&library, "hackrf_set_amp_enable")?,
        start_rx: symbol(&library, "hackrf_start_rx")?,
        stop_rx: symbol(&library, "hackrf_stop_rx")?,
        is_streaming: symbol(&library, "hackrf_is_streaming")?,
        device_list: symbol(&library, "hackrf_device_list")?,
        device_list_free: symbol(&library, "hackrf_device_list_free")?,
        error_name: symbol(&library, "hackrf_error_name")?,
        _library: library,
    };
    // SAFETY: plain library initialiser with no arguments.
    let code = unsafe { symbol::<unsafe extern "C" fn() -> c_int>(&api._library, "hackrf_init")?() };
    if code != SUCCESS {
        return Err(format!("hackrf_init failed ({})", error_text(&api, code)));
    }
    Ok(api)
}

/// The library is loaded and initialised once per process. A failure is kept and reported each time.
static API: OnceLock<Result<Api, String>> = OnceLock::new();

fn api() -> Result<&'static Api, String> {
    API.get_or_init(load_api).as_ref().map_err(Clone::clone)
}

fn error_text(api: &Api, code: c_int) -> String {
    // SAFETY: libhackrf returns a static string for every code.
    let p = unsafe { (api.error_name)(code) };
    if p.is_null() {
        return format!("error {code}");
    }
    // SAFETY: the pointer is non-null and points to a NUL-terminated string.
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

fn check(api: &Api, what: &str, code: c_int) -> Result<(), String> {
    if code == SUCCESS {
        Ok(())
    } else {
        Err(format!("{what}: {}", error_text(api, code)))
    }
}

/// Name of a board id as libhackrf reports it.
fn board_name(board: u8) -> &'static str {
    match board {
        0 => "HackRF Jellybean",
        1 => "HackRF Jawbreaker",
        2 | 4 => "HackRF One",
        3 => "rad1o",
        5 => "HackRF Pro",
        _ => "HackRF",
    }
}

/// Tuning range in Hz for a board. Unknown boards get the One's range.
fn board_range_hz(board: u8) -> (u64, u64) {
    match board {
        1 => (10_000_000, 6_000_000_000),
        3 => (50_000_000, 4_000_000_000),
        5 => (100_000, 6_000_000_000),
        _ => (1_000_000, 6_000_000_000),
    }
}

/// The offered rate closest to `hz`.
fn closest_rate(hz: u32) -> u32 {
    RATES_HZ
        .iter()
        .copied()
        .min_by_key(|r| (i64::from(*r) - i64::from(hz)).abs())
        .unwrap_or(DEFAULT_RATE_HZ)
}

/// Widest baseband filter at or below half the sample rate. At 10 Msps this is 5 MHz.
fn filter_for_rate(rate_hz: u32) -> u32 {
    let half = rate_hz / 2;
    FILTERS_HZ
        .iter()
        .copied()
        .filter(|f| *f <= half)
        .max()
        .unwrap_or(FILTERS_HZ[0])
}

/// Signed 8-bit sample to -1..1.
fn sample_to_f32(v: i8) -> f32 {
    f32::from(v) / 128.0
}

/// Turns a manual gain request into a value the stage accepts: clamped, then rounded to the stage step.
fn manual_gain(stage: &GainStage, setting: GainSetting) -> Result<f32, String> {
    let value = match setting {
        GainSetting::Auto => {
            return Err(format!("gain stage {} has no automatic setting", stage.name));
        }
        GainSetting::Manual(v) => v,
    };
    if !value.is_finite() {
        return Err(format!("gain for stage {} is not a number", stage.name));
    }
    let clamped = value.clamp(stage.min_db, stage.max_db);
    if stage.step_db <= 0.0 {
        return Ok(clamped);
    }
    let snapped = (clamped / stage.step_db).round() * stage.step_db;
    Ok(snapped.clamp(stage.min_db, stage.max_db))
}

/// Samples waiting for the reader, filled by the libhackrf thread.
struct Queue {
    state: Mutex<QueueState>,
    ready: Condvar,
}

struct QueueState {
    items: VecDeque<(f32, f32)>,
    closed: bool,
    dropped: u64,
}

impl Queue {
    fn new() -> Self {
        Self {
            state: Mutex::new(QueueState {
                items: VecDeque::new(),
                closed: false,
                dropped: 0,
            }),
            ready: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, QueueState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Receive callback. The context is a pointer to the `Queue` that `start` keeps alive until after `stop_rx`.
unsafe extern "C" fn receive_block(transfer: *mut Transfer) -> c_int {
    if transfer.is_null() {
        return -1;
    }
    // SAFETY: libhackrf passes a valid transfer for the duration of this call.
    let transfer = unsafe { &*transfer };
    if transfer.buffer.is_null() || transfer.valid_length <= 0 {
        return 0;
    }
    // SAFETY: the context was set by `start` to a live queue, and it is not freed while streaming.
    let queue = unsafe { &*(transfer.rx_context as *const Queue) };
    // SAFETY: the buffer holds `valid_length` bytes for this call.
    let bytes =
        unsafe { std::slice::from_raw_parts(transfer.buffer as *const i8, transfer.valid_length as usize) };
    let mut st = queue.lock();
    if st.closed {
        return -1;
    }
    for pair in bytes.as_chunks::<2>().0 {
        if st.items.len() >= BACKLOG_LIMIT {
            st.items.pop_front();
            st.dropped += 1;
        }
        st.items
            .push_back((sample_to_f32(pair[0]), sample_to_f32(pair[1])));
    }
    drop(st);
    queue.ready.notify_all();
    0
}

/// An open HackRF. Streaming stops and the device closes on drop.
pub struct HackRf {
    api: &'static Api,
    dev: Device,
    label: String,
    board: u8,
    rate_hz: u32,
    freq_hz: u64,
    lna_db: u32,
    vga_db: u32,
    amp_on: bool,
    queue: Arc<Queue>,
    streaming: bool,
}

// SAFETY: the device handle is used only through libhackrf, which allows calls from any thread,
// and a radio is used from one thread at a time.
unsafe impl Send for HackRf {}

impl HackRf {
    fn freq_range(&self) -> (u64, u64) {
        board_range_hz(self.board)
    }

    /// Sends the sample rate, then the filter that goes with it.
    fn send_rate(&self, rate_hz: u32) -> Result<(), String> {
        let api = self.api;
        check(api, "set sample rate", unsafe {
            (api.set_sample_rate)(self.dev, f64::from(rate_hz))
        })?;
        check(api, "set baseband filter", unsafe {
            (api.set_filter_bandwidth)(self.dev, filter_for_rate(rate_hz))
        })
    }

    fn send_freq(&self, freq_hz: u64) -> Result<(), String> {
        check(self.api, "set frequency", unsafe {
            (self.api.set_freq)(self.dev, freq_hz)
        })
    }

    fn send_lna(&self, db: u32) -> Result<(), String> {
        check(self.api, "set LNA gain", unsafe {
            (self.api.set_lna_gain)(self.dev, db)
        })
    }

    fn send_vga(&self, db: u32) -> Result<(), String> {
        check(self.api, "set VGA gain", unsafe {
            (self.api.set_vga_gain)(self.dev, db)
        })
    }

    fn send_amp(&self, on: bool) -> Result<(), String> {
        check(self.api, "set amplifier", unsafe {
            (self.api.set_amp_enable)(self.dev, u8::from(on))
        })
    }

    /// Writes the stored settings to the device, in the order the radio expects.
    fn apply_all(&self) -> Result<(), String> {
        self.send_rate(self.rate_hz)?;
        self.send_freq(self.freq_hz)?;
        self.send_lna(self.lna_db)?;
        self.send_vga(self.vga_db)?;
        self.send_amp(self.amp_on)
    }

    fn stop_streaming(&mut self) {
        if !self.streaming {
            return;
        }
        // Close the queue first so the callback returns, then stop; libhackrf waits for its own thread.
        self.queue.lock().closed = true;
        self.queue.ready.notify_all();
        // SAFETY: the device is open for as long as this struct exists.
        unsafe {
            (self.api.stop_rx)(self.dev);
        }
        self.streaming = false;
    }
}

impl Drop for HackRf {
    fn drop(&mut self) {
        self.stop_streaming();
        // SAFETY: the device was opened by `open` and is closed only here.
        unsafe {
            (self.api.close)(self.dev);
        }
    }
}

impl RxRadio for HackRf {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn freq_range_hz(&self) -> (u64, u64) {
        self.freq_range()
    }

    fn set_frequency_hz(&mut self, hz: u64) -> Result<(), String> {
        let (lo, hi) = self.freq_range();
        if hz < lo || hz > hi {
            return Err(format!(
                "frequency {} MHz is outside this HackRF's range {} to {} MHz",
                hz as f64 / 1e6,
                lo as f64 / 1e6,
                hi as f64 / 1e6
            ));
        }
        self.send_freq(hz)?;
        self.freq_hz = hz;
        Ok(())
    }

    fn sample_rates(&self) -> Vec<u32> {
        RATES_HZ.to_vec()
    }

    fn set_sample_rate(&mut self, hz: u32) -> Result<u32, String> {
        let rate = closest_rate(hz);
        self.send_rate(rate)?;
        self.rate_hz = rate;
        Ok(rate)
    }

    fn gain_stages(&self) -> Vec<GainStage> {
        vec![
            GainStage {
                name: "LNA".into(),
                min_db: 0.0,
                max_db: 40.0,
                step_db: 8.0,
                auto: false,
            },
            GainStage {
                name: "VGA".into(),
                min_db: 0.0,
                max_db: 62.0,
                step_db: 2.0,
                auto: false,
            },
            GainStage {
                name: "amp".into(),
                min_db: 0.0,
                max_db: 1.0,
                step_db: 1.0,
                auto: false,
            },
        ]
    }

    /// The amplifier is on/off: a manual value of 0.5 or more turns it on.
    fn set_gain(&mut self, stage: &str, setting: GainSetting) -> Result<(), String> {
        let stages = self.gain_stages();
        let Some(info) = stages.iter().find(|s| s.name == stage) else {
            return Err(format!("no gain stage named {stage:?}; stages are LNA, VGA, amp"));
        };
        let value = manual_gain(info, setting)?;
        match stage {
            "LNA" => {
                let db = value as u32;
                self.send_lna(db)?;
                self.lna_db = db;
            }
            "VGA" => {
                let db = value as u32;
                self.send_vga(db)?;
                self.vga_db = db;
            }
            _ => {
                let on = value >= 0.5;
                self.send_amp(on)?;
                self.amp_on = on;
            }
        }
        Ok(())
    }

    fn start(&mut self) -> Result<(), String> {
        if self.streaming {
            return Ok(());
        }
        // A fresh queue for each start. The previous one is no longer used once streaming has stopped.
        self.queue = Arc::new(Queue::new());
        let context = Arc::as_ptr(&self.queue) as *mut c_void;
        check(self.api, "start receiving", unsafe {
            (self.api.start_rx)(self.dev, receive_block, context)
        })?;
        self.streaming = true;
        Ok(())
    }

    fn read_iq(&mut self, out: &mut [(f32, f32)]) -> Result<usize, String> {
        if out.is_empty() {
            return Ok(0);
        }
        if !self.streaming {
            return Err("HackRF is not receiving; call start first".into());
        }
        let deadline = Instant::now() + READ_WAIT;
        let queue = Arc::clone(&self.queue);
        let mut st = queue.lock();
        loop {
            if !st.items.is_empty() {
                let n = out.len().min(st.items.len());
                for (slot, s) in out.iter_mut().zip(st.items.drain(..n)) {
                    *slot = s;
                }
                return Ok(n);
            }
            if st.closed {
                return Err("HackRF receive stopped".into());
            }
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let (guard, _) = queue
                .ready
                .wait_timeout(st, deadline - now)
                .unwrap_or_else(|e| e.into_inner());
            st = guard;
        }
        drop(st);
        // SAFETY: the device is open and streaming was started.
        if unsafe { (self.api.is_streaming)(self.dev) } != 1 {
            self.streaming = false;
            return Err("HackRF stopped streaming".into());
        }
        Ok(0)
    }

    fn stop(&mut self) {
        self.stop_streaming();
    }

    fn dropped_samples(&self) -> u64 {
        self.queue.lock().dropped
    }
}

/// Finds attached HackRF radios. Returns an empty list when the library or the device is missing.
pub fn probe() -> Vec<DeviceInfo> {
    let Ok(api) = api() else {
        return Vec::new();
    };
    // SAFETY: libhackrf returns a list it owns; it is freed below.
    let list = unsafe { (api.device_list)() };
    if list.is_null() {
        return Vec::new();
    }
    let mut out = Vec::new();
    // SAFETY: the list is valid until `device_list_free`; the counts come from libhackrf.
    unsafe {
        let l = &*list;
        let count = usize::try_from(l.device_count).unwrap_or(0);
        for i in 0..count {
            let serial_ptr = if l.serial_numbers.is_null() {
                std::ptr::null_mut()
            } else {
                *l.serial_numbers.add(i)
            };
            let serial = if serial_ptr.is_null() {
                String::new()
            } else {
                CStr::from_ptr(serial_ptr).to_string_lossy().into_owned()
            };
            let board = if l.usb_board_ids.is_null() {
                BOARD_UNKNOWN
            } else {
                u8::try_from(*l.usb_board_ids.add(i)).unwrap_or(BOARD_UNKNOWN)
            };
            let tail: String = serial
                .chars()
                .rev()
                .take(8)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            let label = if tail.is_empty() {
                board_name(board).to_string()
            } else {
                format!("{} ({tail})", board_name(board))
            };
            out.push(DeviceInfo {
                driver: "hackrf",
                label,
                id: serial,
            });
        }
        (api.device_list_free)(list);
    }
    out
}

/// Opens a HackRF: the first one found, or the one with the given serial number.
/// The radio is set to the defaults (10 Msps, 100 MHz, LNA 16 dB, VGA 20 dB, amplifier off).
pub fn open(id: Option<&str>) -> Result<Box<dyn RxRadio>, String> {
    let api = api()?;
    let mut dev: Device = std::ptr::null_mut();
    let code = match id {
        Some(serial) => {
            let c = CString::new(serial).map_err(|_| "HackRF serial contains a NUL byte".to_string())?;
            // SAFETY: the serial is a valid NUL-terminated string for the call.
            unsafe { (api.open_by_serial)(c.as_ptr(), &mut dev) }
        }
        None => {
            // SAFETY: `dev` is a valid out pointer.
            unsafe { (api.open)(&mut dev) }
        }
    };
    if code == ERROR_NOT_FOUND {
        return Err(match id {
            Some(serial) => format!("no HackRF with serial {serial:?} found; check the USB cable and that no other application is using it"),
            None => "no HackRF found; check the USB cable and that no other application is using it".into(),
        });
    }
    check(api, "open HackRF", code)?;
    if dev.is_null() {
        return Err("HackRF open returned no device".into());
    }
    let mut board = BOARD_UNKNOWN;
    // SAFETY: `board` is a valid one-byte out pointer; a failure leaves the board unknown.
    if unsafe { (api.board_id_read)(dev, &mut board) } != SUCCESS {
        board = BOARD_UNKNOWN;
    }
    let radio = HackRf {
        api,
        dev,
        label: board_name(board).to_string(),
        board,
        rate_hz: DEFAULT_RATE_HZ,
        freq_hz: DEFAULT_FREQ_HZ,
        lna_db: DEFAULT_LNA_DB,
        vga_db: DEFAULT_VGA_DB,
        amp_on: false,
        queue: Arc::new(Queue::new()),
        streaming: false,
    };
    // Dropping `radio` on an error closes the device again.
    radio.apply_all()?;
    Ok(Box::new(radio))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_scale_maps_full_range() {
        assert_eq!(sample_to_f32(0), 0.0);
        assert_eq!(sample_to_f32(64), 0.5);
        assert_eq!(sample_to_f32(-128), -1.0);
    }

    #[test]
    fn rate_is_closest_offered() {
        assert_eq!(closest_rate(10_000_000), 10_000_000);
        assert_eq!(closest_rate(9_000_000), 8_000_000);
        assert_eq!(closest_rate(1), 2_000_000);
        assert_eq!(closest_rate(u32::MAX), 20_000_000);
    }

    #[test]
    fn filter_is_at_or_below_half_rate() {
        assert_eq!(filter_for_rate(10_000_000), 5_000_000);
        assert_eq!(filter_for_rate(2_000_000), 1_750_000);
        assert_eq!(filter_for_rate(20_000_000), 10_000_000);
    }

    #[test]
    fn board_ranges() {
        assert_eq!(board_range_hz(5), (100_000, 6_000_000_000));
        assert_eq!(board_range_hz(3), (50_000_000, 4_000_000_000));
        assert_eq!(board_range_hz(BOARD_UNKNOWN), (1_000_000, 6_000_000_000));
    }

    fn lna() -> GainStage {
        GainStage {
            name: "LNA".into(),
            min_db: 0.0,
            max_db: 40.0,
            step_db: 8.0,
            auto: false,
        }
    }

    #[test]
    fn manual_gain_snaps_and_clamps() {
        let s = lna();
        assert_eq!(manual_gain(&s, GainSetting::Manual(17.0)).unwrap(), 16.0);
        assert_eq!(manual_gain(&s, GainSetting::Manual(99.0)).unwrap(), 40.0);
        assert_eq!(manual_gain(&s, GainSetting::Manual(-5.0)).unwrap(), 0.0);
        assert!(manual_gain(&s, GainSetting::Manual(f32::NAN)).is_err());
        assert!(manual_gain(&s, GainSetting::Auto).is_err());
    }

    #[test]
    fn probe_does_not_panic_without_library() {
        let _ = probe();
    }
}
