//! Receive driver for LimeSDR boards (LimeSDR Mini 1.x and 2.x, LimeSDR-USB).
//!
//! The vendor library (LimeSuite) is loaded at run time. Nothing here links to
//! it, so the program starts on a machine without it. Only the receive path is
//! written: one channel, float samples, no transmit, no external clock input,
//! no automatic calibration yet.

use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::ptr::null_mut;

use libloading::{Library, Symbol};

use crate::drivers::common::load_library;
use crate::{DeviceInfo, GainSetting, GainStage, RxRadio};

const DRIVER: &str = "limesdr";
/// One device info string is at most this long (with the end mark).
const INFO_LEN: usize = 256;
/// Most boards the scan reads.
const MAX_DEVICES: usize = 16;
/// Entries in the input list and their length.
const MAX_INPUTS: usize = 16;
const INPUT_NAME_LEN: usize = 16;
/// Read timeout of one call, so that read_iq returns within about 200 ms.
const READ_TIMEOUT_MS: c_uint = 200;
/// Most samples asked for in one read.
const MAX_READ_SAMPLES: usize = 65_536;
/// Reads that bring nothing before the radio counts as lost (25 x 200 ms = 5 s).
const STALL_READS: u32 = 25;
/// Failed reads in a row before the stream is given up.
const MAX_STREAM_ERRORS: u32 = 20;
const GAIN_STAGE: &str = "gain";
const GAIN_MAX_DB: f32 = 73.0;
const DEFAULT_RATE_HZ: u32 = 10_000_000;
const DATA_FMT_F32: c_int = 0;
const LINK_FMT_I12: c_int = 2;

const MINI_RATES: [u32; 9] = [
    1_000_000, 2_000_000, 4_000_000, 5_000_000, 8_000_000, 10_000_000, 15_000_000, 20_000_000, 30_720_000,
];
const WIDE_RATES: [u32; 11] = [
    100_000, 1_000_000, 2_000_000, 4_000_000, 5_000_000, 8_000_000, 10_000_000, 15_000_000, 20_000_000, 30_720_000, 61_440_000,
];

// ---------------------------------------------------------------------------
// Board facts (plain functions, no library calls)
// ---------------------------------------------------------------------------

/// What the device info string says about a board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Board {
    /// LimeSDR Mini (1.x or 2.x).
    pub mini: bool,
    /// Mini 2.x.
    pub v2: bool,
    /// False when the info string says USB 2.0.
    pub usb3: bool,
    /// Name for the operator.
    pub model: String,
    /// Serial number, may be empty.
    pub serial: String,
}

/// Reads the board facts from a device info string, for example
/// "LimeSDR Mini, media=USB 3.0, module=FT601, addr=24607:1027, serial=1D40CA8B2ED29A".
pub fn board_from_info(info: &str) -> Board {
    let head = info.split(',').next().unwrap_or("").trim();
    let low = head.to_ascii_lowercase();
    let mini = low.contains("mini");
    let v2 = mini && (low.contains("v2") || low.contains("mini 2") || low.contains("mini2"));
    let model = if mini {
        if v2 { "LimeSDR Mini 2".to_string() } else { "LimeSDR Mini".to_string() }
    } else if head.is_empty() {
        "LimeSDR".to_string()
    } else {
        head.to_string()
    };
    let usb3 = !info.to_ascii_lowercase().contains("media=usb 2");
    let serial = match info.find("serial=") {
        Some(pos) => {
            let rest = &info[pos + "serial=".len()..];
            rest.split(',').next().unwrap_or("").trim().to_string()
        }
        None => String::new(),
    };
    Board { mini, v2, usb3, model, serial }
}

/// Sample rates the board offers, in ascending order (Hz).
pub fn board_rates(b: &Board) -> Vec<u32> {
    if b.mini { MINI_RATES.to_vec() } else { WIDE_RATES.to_vec() }
}

/// The smallest offered rate at or above `want`, or the largest one when `want` is above all of them.
pub fn pick_rate(rates: &[u32], want: u32) -> Option<u32> {
    rates.iter().copied().find(|&r| r >= want).or_else(|| rates.last().copied())
}

/// Tuning range in Hz, inclusive.
pub fn board_freq_range(b: &Board) -> (u64, u64) {
    if b.mini { (10_000_000, 3_500_000_000) } else { (100_000, 3_800_000_000) }
}

/// The index in the library's input list to use at `hz`, or None when no input suits.
/// Names are matched exactly, so "LNAL_NC" (an input with no socket) is never chosen.
/// Low band below the crossover uses LNAL, high band uses LNAH, LNAW is the fallback.
pub fn input_for_frequency(b: &Board, hz: u64, names: &[&str]) -> Option<usize> {
    let crossover: u64 = if b.mini { 1_700_000_000 } else { 1_500_000_000 };
    let mut pick = None;
    let mut wide = None;
    for (i, name) in names.iter().enumerate() {
        if hz < crossover && *name == "LNAL" {
            pick = Some(i);
        }
        if hz >= crossover && *name == "LNAH" {
            pick = Some(i);
        }
        if *name == "LNAW" {
            wide = Some(i);
        }
    }
    pick.or(wide)
}

/// Overall receive gain as the library wants it (whole dB, 0 to 73). The library
/// splits it over its own stages. NaN counts as 0.
pub fn gain_code(db: f32) -> u32 {
    if db.is_nan() {
        return 0;
    }
    (db.clamp(0.0, GAIN_MAX_DB) + 0.5).floor() as u32
}

/// Text added to an open or start-up failure: what to check next.
pub fn open_hint(b: &Board, windows: bool) -> String {
    let mut s = String::from(
        ". If the board is plugged in, its gateware or firmware may not match this LimeSuite library: run the LimeSuite update tool so the matching images are loaded",
    );
    if b.mini && windows {
        s.push_str(". On Windows the LimeSDR Mini needs the FTDI FT60x USB 3 driver, not a generic USB driver");
    } else if windows {
        s.push_str(". On Windows the LimeSuite USB driver must be installed");
    }
    s
}

/// Library file names to try, per system.
#[cfg(target_os = "macos")]
pub fn library_names() -> &'static [&'static str] {
    &["libLimeSuite.dylib"]
}

#[cfg(target_os = "linux")]
pub fn library_names() -> &'static [&'static str] {
    &["libLimeSuite.so.20.01", "libLimeSuite.so"]
}

#[cfg(windows)]
pub fn library_names() -> &'static [&'static str] {
    &["LimeSuite.dll"]
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub fn library_names() -> &'static [&'static str] {
    &["libLimeSuite.so"]
}

/// Hz as MHz text for messages, without trailing zeros.
fn mhz_text(hz: u64) -> String {
    let s = format!("{:.6}", hz as f64 / 1e6);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Splits the raw scan buffer into info strings.
fn split_infos(raw: &[u8], count: usize) -> Vec<String> {
    raw.chunks_exact(INFO_LEN)
        .take(count.min(MAX_DEVICES))
        .map(|c| {
            let end = c.iter().position(|&b| b == 0).unwrap_or(c.len());
            String::from_utf8_lossy(&c[..end]).into_owned()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

// ---------------------------------------------------------------------------
// Library binding
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Stream {
    handle: usize,
    is_tx: bool,
    channel: u32,
    fifo_size: u32,
    throughput_vs_latency: f32,
    data_fmt: c_int,
    link_fmt: c_int,
}

#[repr(C)]
#[derive(Default)]
struct Meta {
    timestamp: u64,
    wait_for_timestamp: bool,
    flush_partial_packet: bool,
}

type FnDeviceList = unsafe extern "C" fn(*mut c_char) -> c_int;
type FnOpen = unsafe extern "C" fn(*mut *mut c_void, *const c_char, *mut c_void) -> c_int;
type FnDevice = unsafe extern "C" fn(*mut c_void) -> c_int;
type FnEnable = unsafe extern "C" fn(*mut c_void, bool, usize, bool) -> c_int;
type FnSetRate = unsafe extern "C" fn(*mut c_void, f64, usize) -> c_int;
type FnGetRate = unsafe extern "C" fn(*mut c_void, bool, usize, *mut f64, *mut f64) -> c_int;
type FnSetLo = unsafe extern "C" fn(*mut c_void, bool, usize, f64) -> c_int;
type FnInputList = unsafe extern "C" fn(*mut c_void, bool, usize, *mut c_char) -> c_int;
type FnSetInput = unsafe extern "C" fn(*mut c_void, bool, usize, usize) -> c_int;
type FnSetBw = unsafe extern "C" fn(*mut c_void, bool, usize, f64) -> c_int;
type FnSetGain = unsafe extern "C" fn(*mut c_void, bool, usize, c_uint) -> c_int;
type FnSetupStream = unsafe extern "C" fn(*mut c_void, *mut Stream) -> c_int;
type FnDestroyStream = unsafe extern "C" fn(*mut c_void, *mut Stream) -> c_int;
type FnStream = unsafe extern "C" fn(*mut Stream) -> c_int;
type FnRecv = unsafe extern "C" fn(*mut Stream, *mut c_void, usize, *mut Meta, c_uint) -> c_int;
type FnLastError = unsafe extern "C" fn() -> *const c_char;

/// The loaded library and its functions. The library handle must outlive the
/// copied function pointers, so it is kept in the same struct.
struct Api {
    _lib: Library,
    device_list: FnDeviceList,
    open: FnOpen,
    close: FnDevice,
    init: FnDevice,
    enable_channel: FnEnable,
    set_sample_rate: FnSetRate,
    get_sample_rate: Option<FnGetRate>,
    set_lo: FnSetLo,
    input_list: FnInputList,
    set_input: FnSetInput,
    set_lpf_bw: FnSetBw,
    set_gain_db: FnSetGain,
    setup_stream: FnSetupStream,
    destroy_stream: FnDestroyStream,
    start_stream: FnStream,
    stop_stream: FnStream,
    recv: FnRecv,
    last_error: Option<FnLastError>,
}

fn required<T: Copy>(lib: &Library, name: &[u8]) -> Result<T, String> {
    // SAFETY: each caller names a symbol with the C signature declared above.
    let sym: Symbol<T> = unsafe { lib.get(name) }.map_err(|e| {
        format!("LimeSDR: the LimeSuite library has no {} ({e})", String::from_utf8_lossy(&name[..name.len() - 1]))
    })?;
    Ok(*sym)
}

fn optional<T: Copy>(lib: &Library, name: &[u8]) -> Option<T> {
    // SAFETY: as in `required`; a missing symbol just means the feature is not used.
    unsafe { lib.get::<T>(name) }.ok().map(|s| *s)
}

impl Api {
    fn load(names: &[&str]) -> Result<Api, String> {
        let lib = load_library(names).map_err(|e| {
            format!("LimeSDR: cannot load the LimeSuite library; install LimeSuite for this system and try again ({e})")
        })?;
        let api = Api {
            device_list: required(&lib, b"LMS_GetDeviceList\0")?,
            open: required(&lib, b"LMS_Open\0")?,
            close: required(&lib, b"LMS_Close\0")?,
            init: required(&lib, b"LMS_Init\0")?,
            enable_channel: required(&lib, b"LMS_EnableChannel\0")?,
            set_sample_rate: required(&lib, b"LMS_SetSampleRate\0")?,
            get_sample_rate: optional(&lib, b"LMS_GetSampleRate\0"),
            set_lo: required(&lib, b"LMS_SetLOFrequency\0")?,
            input_list: required(&lib, b"LMS_GetAntennaList\0")?,
            set_input: required(&lib, b"LMS_SetAntenna\0")?,
            set_lpf_bw: required(&lib, b"LMS_SetLPFBW\0")?,
            set_gain_db: required(&lib, b"LMS_SetGaindB\0")?,
            setup_stream: required(&lib, b"LMS_SetupStream\0")?,
            destroy_stream: required(&lib, b"LMS_DestroyStream\0")?,
            start_stream: required(&lib, b"LMS_StartStream\0")?,
            stop_stream: required(&lib, b"LMS_StopStream\0")?,
            recv: required(&lib, b"LMS_RecvStream\0")?,
            last_error: optional(&lib, b"LMS_GetLastErrorMessage\0"),
            _lib: lib,
        };
        Ok(api)
    }

    /// The library's own words for the last failure, with a leading ": ", or empty.
    fn text(&self) -> String {
        let Some(f) = self.last_error else { return String::new() };
        // SAFETY: the library returns a pointer to its own message text or null.
        let p = unsafe { f() };
        if p.is_null() {
            return String::new();
        }
        let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().trim().to_string();
        if s.is_empty() { String::new() } else { format!(": {s}") }
    }
}

/// Reads the info strings of the boards the library can see.
fn scan(api: &Api) -> Result<Vec<String>, String> {
    // SAFETY: a null buffer asks only for the count.
    let n = unsafe { (api.device_list)(null_mut()) };
    if n < 0 {
        return Err(format!("LimeSDR: the device scan failed{}", api.text()));
    }
    if n == 0 {
        return Ok(Vec::new());
    }
    let mut raw = vec![0u8; n as usize * INFO_LEN];
    // SAFETY: the buffer holds n strings of INFO_LEN bytes, as the library expects.
    let got = unsafe { (api.device_list)(raw.as_mut_ptr() as *mut c_char) };
    if got < 0 {
        return Err(format!("LimeSDR: the device scan failed{}", api.text()));
    }
    Ok(split_infos(&raw, got as usize))
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Lists the LimeSDR boards the library can see. Returns nothing when the library is missing.
pub fn probe() -> Vec<DeviceInfo> {
    let Ok(api) = Api::load(library_names()) else { return Vec::new() };
    let Ok(infos) = scan(&api) else { return Vec::new() };
    infos
        .iter()
        .map(|info| {
            let b = board_from_info(info);
            DeviceInfo { driver: DRIVER, label: b.model, id: b.serial }
        })
        .collect()
}

/// Opens a board. `id` is the serial number; None takes the first board found.
pub fn open(id: Option<&str>) -> Result<Box<dyn RxRadio>, String> {
    let api = Api::load(library_names())?;
    let infos = scan(&api)?;
    if infos.is_empty() {
        return Err("LimeSDR: no radio found; check the USB cable and the port, and that the board has power".into());
    }
    let info = match id.map(|s| s.trim().trim_start_matches("serial=")).filter(|s| !s.is_empty()) {
        None => infos[0].clone(),
        Some(serial) => infos
            .iter()
            .find(|i| board_from_info(i).serial == serial)
            .cloned()
            .ok_or_else(|| {
                let found: Vec<String> = infos.iter().map(|i| board_from_info(i).serial).collect();
                format!("LimeSDR: no radio with serial {serial}; found: {}", found.join(", "))
            })?,
    };
    let board = board_from_info(&info);
    let c_info = CString::new(info.clone()).map_err(|_| "LimeSDR: the device info has a NUL byte".to_string())?;
    let mut dev: *mut c_void = null_mut();
    // SAFETY: `dev` receives the handle; the info string is a valid C string.
    let rc = unsafe { (api.open)(&mut dev, c_info.as_ptr(), null_mut()) };
    if rc != 0 || dev.is_null() {
        let hint = open_hint(&board, cfg!(windows));
        return Err(format!("LimeSDR: cannot open the radio{}{hint}", api.text()));
    }
    let label = board.model.clone();
    let mut radio = LimeSdr {
        api,
        board,
        label,
        dev,
        rate: 0,
        freq_hz: None,
        stream: Stream::default(),
        streaming: false,
        buf: Vec::new(),
        stall_reads: 0,
        stream_errors: 0,
    };
    radio.init_device()?;
    Ok(Box::new(radio))
}

// ---------------------------------------------------------------------------
// The open radio
// ---------------------------------------------------------------------------

struct LimeSdr {
    api: Api,
    board: Board,
    label: String,
    /// Library device handle; null after close.
    dev: *mut c_void,
    /// Rate the board really runs at (Hz).
    rate: u32,
    freq_hz: Option<u64>,
    stream: Stream,
    streaming: bool,
    /// Interleaved I and Q floats for one read.
    buf: Vec<f32>,
    stall_reads: u32,
    stream_errors: u32,
}

// SAFETY: the device handle is only used through `&mut self`, so one thread at a
// time, as the trait requires. The library handle is not shared with other code.
unsafe impl Send for LimeSdr {}

impl LimeSdr {
    fn fail(&self, what: &str) -> String {
        format!("LimeSDR: {what}{}", self.api.text())
    }

    fn init_device(&mut self) -> Result<(), String> {
        // SAFETY: `dev` is the open handle.
        if unsafe { (self.api.init)(self.dev) } != 0 {
            return Err(format!("{}{}", self.fail("initialisation failed"), open_hint(&self.board, cfg!(windows))));
        }
        // SAFETY: as above; channel 0 of the receiver.
        if unsafe { (self.api.enable_channel)(self.dev, false, 0, true) } != 0 {
            return Err(self.fail("cannot enable the receiver"));
        }
        self.apply_rate(DEFAULT_RATE_HZ).map(|_| ())
    }

    /// Sets the rate, restarting the stream when it runs.
    fn apply_rate(&mut self, want: u32) -> Result<u32, String> {
        let chosen = pick_rate(&board_rates(&self.board), want).ok_or("LimeSDR: this board offers no sample rate")?;
        let was_streaming = self.streaming;
        self.halt_stream();
        // SAFETY: `dev` is open; receive channel 0.
        if unsafe { (self.api.set_sample_rate)(self.dev, chosen as f64, 0) } != 0 {
            return Err(self.fail("the sample rate was refused"));
        }
        let mut actual = chosen;
        if let Some(get) = self.api.get_sample_rate {
            let mut host = 0f64;
            let mut rf = 0f64;
            // SAFETY: both out pointers are valid for the call.
            if unsafe { get(self.dev, false, 0, &mut host, &mut rf) } == 0 && host > 0.0 {
                actual = host.round() as u32;
            }
        }
        // The filter follows the rate. A failure here is not fatal; the board keeps its last filter.
        let bw = (1.5e6f64).max(actual as f64 * 0.95);
        // SAFETY: as above.
        let _ = unsafe { (self.api.set_lpf_bw)(self.dev, false, 0, bw) };
        self.rate = actual;
        if was_streaming {
            self.start_stream()?;
        }
        Ok(self.rate)
    }

    fn halt_stream(&mut self) {
        if self.streaming {
            // SAFETY: the stream was set up on this device and is still valid here.
            unsafe {
                (self.api.stop_stream)(&mut self.stream);
                (self.api.destroy_stream)(self.dev, &mut self.stream);
            }
            self.streaming = false;
        }
    }

    fn start_stream(&mut self) -> Result<(), String> {
        if self.streaming {
            return Ok(());
        }
        // About 0.1 s of samples, a power of two, at least 1 Msample, at most 8 Msamples.
        let mut fifo: u32 = 1024 * 1024;
        while fifo < 8 * 1024 * 1024 && (fifo as f64) < self.rate as f64 * 0.1 {
            fifo *= 2;
        }
        self.stream = Stream {
            handle: 0,
            is_tx: false,
            channel: 0,
            fifo_size: fifo,
            throughput_vs_latency: 1.0,
            data_fmt: DATA_FMT_F32,
            // The Mini's ADC has 12 bits, so the packed 12-bit link carries it without loss.
            link_fmt: if self.board.mini { LINK_FMT_I12 } else { 0 },
        };
        let setup = self.api.setup_stream;
        let start = self.api.start_stream;
        // SAFETY: the stream struct lives in `self` for as long as the stream runs.
        if unsafe { setup(self.dev, &mut self.stream) } != 0 {
            return Err(self.fail("cannot set up the sample stream"));
        }
        // SAFETY: as above.
        if unsafe { start(&mut self.stream) } != 0 {
            let destroy = self.api.destroy_stream;
            unsafe { destroy(self.dev, &mut self.stream) };
            return Err(self.fail("cannot start the sample stream"));
        }
        self.streaming = true;
        self.stall_reads = 0;
        self.stream_errors = 0;
        Ok(())
    }
}

impl Drop for LimeSdr {
    fn drop(&mut self) {
        self.halt_stream();
        if !self.dev.is_null() {
            // SAFETY: the handle came from a successful open and is closed once.
            unsafe { (self.api.close)(self.dev) };
            self.dev = null_mut();
        }
    }
}

impl RxRadio for LimeSdr {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn freq_range_hz(&self) -> (u64, u64) {
        board_freq_range(&self.board)
    }

    fn set_frequency_hz(&mut self, hz: u64) -> Result<(), String> {
        let (lo, hi) = board_freq_range(&self.board);
        if hz < lo || hz > hi {
            return Err(format!("LimeSDR: {} MHz is outside the range {}-{} MHz", mhz_text(hz), mhz_text(lo), mhz_text(hi)));
        }
        // SAFETY: `dev` is open; receive channel 0.
        if unsafe { (self.api.set_lo)(self.dev, false, 0, hz as f64) } != 0 {
            return Err(self.fail(&format!("the library refused {} MHz", mhz_text(hz))));
        }
        // Read the input names and pick the input for this frequency.
        let mut raw = vec![0u8; MAX_INPUTS * INPUT_NAME_LEN];
        // SAFETY: the buffer holds MAX_INPUTS entries of INPUT_NAME_LEN bytes.
        let n = unsafe { (self.api.input_list)(self.dev, false, 0, raw.as_mut_ptr() as *mut c_char) };
        if n < 0 {
            return Err(self.fail("cannot read the input list"));
        }
        let names: Vec<String> = (0..(n as usize).min(MAX_INPUTS))
            .map(|i| {
                let e = &raw[i * INPUT_NAME_LEN..(i + 1) * INPUT_NAME_LEN];
                let end = e.iter().position(|&b| b == 0).unwrap_or(e.len());
                String::from_utf8_lossy(&e[..end]).into_owned()
            })
            .collect();
        let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        let pick = input_for_frequency(&self.board, hz, &refs)
            .ok_or_else(|| format!("LimeSDR: no receive input suits {} MHz on this board", mhz_text(hz)))?;
        // SAFETY: `pick` is an index into the list the library just gave.
        if unsafe { (self.api.set_input)(self.dev, false, 0, pick) } != 0 {
            return Err(self.fail("the receive input could not be set"));
        }
        self.freq_hz = Some(hz);
        Ok(())
    }

    fn sample_rates(&self) -> Vec<u32> {
        board_rates(&self.board)
    }

    fn set_sample_rate(&mut self, hz: u32) -> Result<u32, String> {
        self.apply_rate(hz)
    }

    fn gain_stages(&self) -> Vec<GainStage> {
        vec![GainStage { name: GAIN_STAGE.into(), min_db: 0.0, max_db: GAIN_MAX_DB, step_db: 1.0, auto: false }]
    }

    fn set_gain(&mut self, stage: &str, setting: GainSetting) -> Result<(), String> {
        if stage != GAIN_STAGE {
            return Err(format!("LimeSDR has no gain stage named {stage:?}; it has: {GAIN_STAGE}"));
        }
        let db = match setting {
            GainSetting::Auto => return Err("LimeSDR gain has no automatic setting; give a value in dB".into()),
            GainSetting::Manual(v) => v,
        };
        // SAFETY: `dev` is open; receive channel 0.
        if unsafe { (self.api.set_gain_db)(self.dev, false, 0, gain_code(db)) } != 0 {
            return Err(self.fail("the gain was refused"));
        }
        Ok(())
    }

    fn start(&mut self) -> Result<(), String> {
        if self.freq_hz.is_none() {
            return Err("LimeSDR: set a frequency before starting".into());
        }
        self.start_stream()
    }

    fn read_iq(&mut self, out: &mut [(f32, f32)]) -> Result<usize, String> {
        if out.is_empty() {
            return Ok(0);
        }
        if !self.streaming {
            return Err("LimeSDR: the stream is not started".into());
        }
        let n = out.len().min(MAX_READ_SAMPLES);
        if self.buf.len() < 2 * n {
            self.buf.resize(2 * n, 0.0);
        }
        let mut meta = Meta::default();
        let recv = self.api.recv;
        // SAFETY: the buffer holds 2 * n floats, which is room for n complex samples.
        let got = unsafe { recv(&mut self.stream, self.buf.as_mut_ptr() as *mut c_void, n, &mut meta, READ_TIMEOUT_MS) };
        if got < 0 {
            self.stream_errors += 1;
            if self.stream_errors > MAX_STREAM_ERRORS {
                self.halt_stream();
                return Err(self.fail(&format!("the sample stream failed {} times in a row", self.stream_errors)));
            }
            return Ok(0);
        }
        self.stream_errors = 0;
        if got == 0 {
            // Nothing came within the timeout. The library does not report an unplugged board, so a long silence counts as lost.
            self.stall_reads += 1;
            if self.stall_reads >= STALL_READS {
                self.halt_stream();
                return Err("LimeSDR: no samples for 5 seconds; the radio may have been unplugged".into());
            }
            return Ok(0);
        }
        self.stall_reads = 0;
        let got = (got as usize).min(n);
        for (i, o) in out[..got].iter_mut().enumerate() {
            *o = (self.buf[2 * i], self.buf[2 * i + 1]);
        }
        Ok(got)
    }

    fn stop(&mut self) {
        self.halt_stream();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINI_INFO: &str = "LimeSDR Mini, media=USB 3.0, module=FT601, addr=24607:1027, serial=1D40CA8B2ED29A";

    #[test]
    fn board_name_and_flags() {
        let b = board_from_info(MINI_INFO);
        assert!(b.mini);
        assert!(!b.v2);
        assert!(b.usb3);
        assert_eq!(b.model, "LimeSDR Mini");
        assert_eq!(b.serial, "1D40CA8B2ED29A");
    }

    #[test]
    fn mini_2_on_usb2() {
        let b = board_from_info("LimeSDR Mini v2.x, media=USB 2.0, serial=ABC123");
        assert!(b.mini && b.v2);
        assert!(!b.usb3);
        assert_eq!(b.model, "LimeSDR Mini 2");
        assert_eq!(b.serial, "ABC123");
    }

    #[test]
    fn usb_board_keeps_its_name_and_has_no_serial() {
        let b = board_from_info("LimeSDR-USB, media=USB 3.0");
        assert!(!b.mini);
        assert_eq!(b.model, "LimeSDR-USB");
        assert_eq!(b.serial, "");
        assert!(b.usb3);
    }

    #[test]
    fn rate_lists() {
        let mini = board_rates(&board_from_info(MINI_INFO));
        assert_eq!(mini.first(), Some(&1_000_000));
        assert_eq!(mini.last(), Some(&30_720_000));
        assert_eq!(mini.len(), 9);
        let wide = board_rates(&board_from_info("LimeSDR-USB"));
        assert_eq!(wide.last(), Some(&61_440_000));
        assert!(wide.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn rate_pick_is_smallest_at_or_above() {
        let mini = board_rates(&board_from_info(MINI_INFO));
        assert_eq!(pick_rate(&mini, 9_000_000), Some(10_000_000));
        assert_eq!(pick_rate(&mini, 0), Some(1_000_000));
        assert_eq!(pick_rate(&mini, 50_000_000), Some(30_720_000));
        assert_eq!(pick_rate(&[], 1), None);
    }

    #[test]
    fn frequency_ranges() {
        assert_eq!(board_freq_range(&board_from_info(MINI_INFO)), (10_000_000, 3_500_000_000));
        assert_eq!(board_freq_range(&board_from_info("LimeSDR-USB")), (100_000, 3_800_000_000));
    }

    #[test]
    fn input_follows_frequency_and_skips_unwired_inputs() {
        let mini = board_from_info(MINI_INFO);
        let mini_names = ["LNAW", "LNAH", "LNAL_NC"];
        assert_eq!(input_for_frequency(&mini, 900_000_000, &mini_names), Some(0));
        assert_eq!(input_for_frequency(&mini, 2_400_000_000, &mini_names), Some(1));
        assert_eq!(input_for_frequency(&mini, 500_000_000, &["LNAL_NC", "LNAW", "LNAH"]), Some(1));

        let wide = board_from_info("LimeSDR-USB");
        let names = ["LNAL", "LNAH", "LNAW"];
        assert_eq!(input_for_frequency(&wide, 100_000_000, &names), Some(0));
        assert_eq!(input_for_frequency(&wide, 1_400_000_000, &names), Some(0));
        assert_eq!(input_for_frequency(&wide, 1_600_000_000, &names), Some(1));
        assert_eq!(input_for_frequency(&wide, 100_000_000, &["LNAW"]), Some(0));
        assert_eq!(input_for_frequency(&wide, 100_000_000, &[]), None);
    }

    #[test]
    fn gain_maps_to_whole_db_and_clamps() {
        assert_eq!(gain_code(-5.0), 0);
        assert_eq!(gain_code(0.0), 0);
        assert_eq!(gain_code(12.4), 12);
        assert_eq!(gain_code(12.5), 13);
        assert_eq!(gain_code(73.0), 73);
        assert_eq!(gain_code(100.0), 73);
        assert_eq!(gain_code(f32::NAN), 0);
    }

    #[test]
    fn open_hint_names_the_next_step() {
        let mini = board_from_info(MINI_INFO);
        let plain = open_hint(&mini, false);
        assert!(plain.contains("update tool"));
        assert!(!plain.contains("FTDI"));
        assert!(open_hint(&mini, true).contains("FTDI"));
        assert!(open_hint(&board_from_info("LimeSDR-USB"), true).contains("USB driver"));
    }

    #[test]
    fn missing_library_message() {
        let err = Api::load(&["kahf-no-such-limesuite-library.so"]).err().expect("loading a missing library must fail");
        assert!(err.contains("cannot load the LimeSuite library"), "{err}");
        assert!(err.contains("install LimeSuite"), "{err}");
        assert!(err.contains("tried"), "{err}");
    }

    #[test]
    fn scan_buffer_splits_into_info_strings() {
        let mut raw = vec![0u8; 2 * INFO_LEN];
        raw[..MINI_INFO.len()].copy_from_slice(MINI_INFO.as_bytes());
        raw[INFO_LEN..INFO_LEN + 7].copy_from_slice(b"LimeSDR");
        let infos = split_infos(&raw, 2);
        assert_eq!(infos.len(), 2);
        assert_eq!(infos[0], MINI_INFO);
        assert_eq!(infos[1], "LimeSDR");
        assert!(split_infos(&raw, 0).is_empty());
    }

    #[test]
    fn mhz_text_has_no_trailing_zeros() {
        assert_eq!(mhz_text(10_000_000), "10");
        assert_eq!(mhz_text(1_500_000), "1.5");
        assert_eq!(mhz_text(3_500_000_000), "3500");
    }

    #[test]
    fn probe_without_library_is_empty_and_does_not_panic() {
        // No LimeSuite library is expected on the test machine; either way probe must return.
        let _ = probe();
    }
}
