//! Driver for the bladeRF 1 and 2.0. The vendor host library is loaded at run time.
//!
//! Receive path: open the radio, check the FPGA (bladeRF 1 only), read its ranges,
//! set the rate and bandwidth, start a synchronous stream of 16-bit I/Q pairs
//! (Q11 format) and convert them to floats.
//!
//! Not tested with hardware. The unit tests cover the pure helpers only.

use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::path::PathBuf;
use std::ptr::{null, null_mut};

use libloading::Library;

use crate::drivers::common::load_library;
use crate::{DeviceInfo, GainSetting, GainStage, RxRadio};

#[cfg(target_os = "macos")]
const LIB_NAMES: &[&str] = &["libbladeRF.dylib"];
#[cfg(all(unix, not(target_os = "macos")))]
const LIB_NAMES: &[&str] = &["libbladeRF.so.2", "libbladeRF.so"];
#[cfg(windows)]
const LIB_NAMES: &[&str] = &["bladeRF.dll"];

/// Rate used when the caller asks for 0.
const DEFAULT_RATE: u32 = 10_000_000;
/// Lowest bandwidth the filter is set to, in Hz.
const MIN_BANDWIDTH: u32 = 200_000;
/// Most samples taken in one call, so one call stays near the read timeout.
const MAX_BLOCK_SAMPLES: usize = 8192;
/// Longest a single read waits, in milliseconds.
const READ_TIMEOUT_MS: c_uint = 200;
/// Stream setup: 16 buffers of 16384 samples, 8 transfers in flight, 3500 ms stream timeout.
const STREAM_BUFFERS: c_uint = 16;
const STREAM_BUFFER_SAMPLES: c_uint = 16384;
const STREAM_TRANSFERS: c_uint = 8;
const STREAM_TIMEOUT_MS: c_uint = 3500;
/// Q11 samples: full scale is 2048.
const FULL_SCALE: f32 = 2048.0;

const LAYOUT_RX_X1: c_int = 0;
const FORMAT_SC16_Q11: c_int = 0;
const GAIN_MODE_DEFAULT: c_int = 0;
const GAIN_MODE_MANUAL: c_int = 1;
/// Error code the library returns when a read times out.
const TIMEOUT_CODE: c_int = -6;
/// Error codes that usually mean the USB link or its access rights are the problem.
const USB_ACCESS_CODES: &[c_int] = &[-5, -7, -8, -17];
/// The one gain stage this driver offers.
const GAIN_NAME: &str = "gain";
/// Most devices listed by one probe.
const MAX_PROBE: usize = 16;

type Dev = *mut c_void;

#[repr(C)]
struct DevInfo {
    backend: c_int,
    serial: [c_char; 33],
    usb_bus: u8,
    usb_addr: u8,
    instance: c_uint,
    manufacturer: [c_char; 33],
    product: [c_char; 33],
}

/// Min, max and step of a range, with a scale that turns the integers into real values.
#[repr(C)]
#[derive(Clone, Copy)]
struct Range {
    min: i64,
    max: i64,
    step: i64,
    scale: f32,
}

type FnDeviceList = unsafe extern "C" fn(*mut *mut DevInfo) -> c_int;
type FnFreeDeviceList = unsafe extern "C" fn(*mut DevInfo);
type FnOpen = unsafe extern "C" fn(*mut Dev, *const c_char) -> c_int;
type FnClose = unsafe extern "C" fn(Dev);
type FnSetFrequency = unsafe extern "C" fn(Dev, c_int, u64) -> c_int;
type FnSetRate = unsafe extern "C" fn(Dev, c_int, c_uint, *mut c_uint) -> c_int;
type FnSetInt = unsafe extern "C" fn(Dev, c_int, c_int) -> c_int;
type FnGetRange = unsafe extern "C" fn(Dev, c_int, *mut *const Range) -> c_int;
type FnEnable = unsafe extern "C" fn(Dev, c_int, bool) -> c_int;
type FnSyncConfig = unsafe extern "C" fn(Dev, c_int, c_int, c_uint, c_uint, c_uint, c_uint) -> c_int;
type FnSyncRx = unsafe extern "C" fn(Dev, *mut c_void, c_uint, *mut c_void, c_uint) -> c_int;
type FnStrerror = unsafe extern "C" fn(c_int) -> *const c_char;
type FnDevInt = unsafe extern "C" fn(Dev) -> c_int;
type FnFpgaSize = unsafe extern "C" fn(Dev, *mut c_int) -> c_int;
type FnLoadFpga = unsafe extern "C" fn(Dev, *const c_char) -> c_int;
type FnBoardName = unsafe extern "C" fn(Dev) -> *const c_char;

/// Function pointers from the vendor library. `_lib` keeps the library loaded while they are used.
struct Api {
    get_device_list: FnDeviceList,
    free_device_list: FnFreeDeviceList,
    open: FnOpen,
    close: FnClose,
    set_frequency: FnSetFrequency,
    set_sample_rate: FnSetRate,
    set_bandwidth: FnSetRate,
    set_gain: FnSetInt,
    set_gain_mode: FnSetInt,
    get_gain_range: FnGetRange,
    enable_module: FnEnable,
    sync_config: FnSyncConfig,
    sync_rx: FnSyncRx,
    strerror: Option<FnStrerror>,
    is_fpga_configured: Option<FnDevInt>,
    get_fpga_size: Option<FnFpgaSize>,
    load_fpga: Option<FnLoadFpga>,
    get_board_name: Option<FnBoardName>,
    get_frequency_range: Option<FnGetRange>,
    get_sample_rate_range: Option<FnGetRange>,
    _lib: Library,
}

impl Api {
    fn strerror_text(&self, code: c_int) -> Option<String> {
        let f = self.strerror?;
        // SAFETY: the library returns a pointer to a static text for any code.
        let p = unsafe { f(code) };
        if p.is_null() {
            None
        } else {
            // SAFETY: the pointer is a valid NUL-terminated string from the library.
            Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
        }
    }
}

/// Loads the vendor library and finds every function the driver needs.
fn load_api() -> Result<Api, String> {
    let lib = load_library(LIB_NAMES).map_err(|e| library_missing_text(&e))?;
    // SAFETY: each symbol is looked up by name and typed with the signature the library documents.
    unsafe {
        Ok(Api {
            get_device_list: required(&lib, "bladerf_get_device_list")?,
            free_device_list: required(&lib, "bladerf_free_device_list")?,
            open: required(&lib, "bladerf_open")?,
            close: required(&lib, "bladerf_close")?,
            set_frequency: required(&lib, "bladerf_set_frequency")?,
            set_sample_rate: required(&lib, "bladerf_set_sample_rate")?,
            set_bandwidth: required(&lib, "bladerf_set_bandwidth")?,
            set_gain: required(&lib, "bladerf_set_gain")?,
            set_gain_mode: required(&lib, "bladerf_set_gain_mode")?,
            get_gain_range: required(&lib, "bladerf_get_gain_range")?,
            enable_module: required(&lib, "bladerf_enable_module")?,
            sync_config: required(&lib, "bladerf_sync_config")?,
            sync_rx: required(&lib, "bladerf_sync_rx")?,
            strerror: optional(&lib, "bladerf_strerror"),
            is_fpga_configured: optional(&lib, "bladerf_is_fpga_configured"),
            get_fpga_size: optional(&lib, "bladerf_get_fpga_size"),
            load_fpga: optional(&lib, "bladerf_load_fpga"),
            get_board_name: optional(&lib, "bladerf_get_board_name"),
            get_frequency_range: optional(&lib, "bladerf_get_frequency_range"),
            get_sample_rate_range: optional(&lib, "bladerf_get_sample_rate_range"),
            _lib: lib,
        })
    }
}

/// Looks up a symbol that the driver cannot work without.
///
/// # Safety
/// `T` must be a function pointer type with the signature the library uses for `name`.
unsafe fn required<T: Copy>(lib: &Library, name: &str) -> Result<T, String> {
    let symbol = format!("{name}\0");
    match unsafe { lib.get::<T>(symbol.as_bytes()) } {
        Ok(f) => Ok(*f),
        Err(_) => Err(format!(
            "bladeRF: the host library has no {name}; it is too old or not the bladeRF library"
        )),
    }
}

/// Looks up a symbol that is used only when present.
///
/// # Safety
/// Same rule as `required`.
unsafe fn optional<T: Copy>(lib: &Library, name: &str) -> Option<T> {
    let symbol = format!("{name}\0");
    unsafe { lib.get::<T>(symbol.as_bytes()) }.ok().map(|f| *f)
}

/// The message when the host library cannot be loaded.
pub(crate) fn library_missing_text(detail: &str) -> String {
    format!("bladeRF: cannot load the bladeRF host library ({detail}). Install the bladeRF host library and try again.")
}

/// Error text from the library's own message, or the bare code.
pub(crate) fn describe_error(text: Option<String>, code: c_int) -> String {
    match text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
        Some(t) => format!("{t} (code {code})"),
        None => format!("error code {code}"),
    }
}

/// The rate the radio is set to: the request, or the default for 0, held inside the range.
pub(crate) fn choose_rate(requested: u32, lo: u32, hi: u32) -> u32 {
    let want = if requested == 0 { DEFAULT_RATE } else { requested };
    want.max(lo).min(hi)
}

/// The filter bandwidth for a rate: 95 % of it, never below the minimum.
pub(crate) fn choose_bandwidth(rate: u32) -> u32 {
    (rate as u64 * 95 / 100).max(MIN_BANDWIDTH as u64) as u32
}

const COMMON_RATES: &[u32] = &[
    520_834, 1_000_000, 2_000_000, 2_400_000, 4_000_000, 5_000_000, 8_000_000, 10_000_000, 12_000_000,
    16_000_000, 20_000_000, 25_000_000, 30_000_000, 40_000_000, 50_000_000, 61_440_000,
];

/// Rates offered in the range, ascending. The two ends of the range are always included.
pub(crate) fn offered_rates(lo: u32, hi: u32) -> Vec<u32> {
    let mut v: Vec<u32> = COMMON_RATES
        .iter()
        .copied()
        .chain([lo, hi])
        .filter(|r| *r >= lo && *r <= hi)
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Sample-rate range when the library cannot report one. Depends on the board.
pub(crate) fn default_rate_range(board: &str) -> (u32, u32) {
    if board.to_lowercase().contains("bladerf1") {
        (80_000, 40_000_000)
    } else {
        (520_834, 61_440_000)
    }
}

/// Tuning range when the library cannot report one. Depends on the board.
pub(crate) fn default_freq_range(board: &str) -> (u64, u64) {
    if board.to_lowercase().contains("bladerf1") {
        (237_500_000, 3_800_000_000)
    } else {
        (70_000_000, 6_000_000_000)
    }
}

/// Gain in whole dB inside the stage range. NaN goes to the bottom of the range.
pub(crate) fn clamp_gain_db(db: f32, lo: f32, hi: f32) -> i32 {
    let v = if db.is_nan() { lo } else { db.max(lo).min(hi) };
    v.round() as i32
}

/// Turns raw Q11 pairs (I, Q) into floats in -1..1.
#[allow(clippy::chunks_exact_to_as_chunks)]
pub(crate) fn convert_samples(raw: &[i16], out: &mut [(f32, f32)]) {
    for (dst, pair) in out.iter_mut().zip(raw.chunks_exact(2)) {
        *dst = (pair[0] as f32 / FULL_SCALE, pair[1] as f32 / FULL_SCALE);
    }
}

/// Splits "serial" or "serial#port" (port 0 = RX1, 1 = RX2). Empty means the first radio.
pub(crate) fn parse_id(id: Option<&str>) -> Result<(Option<String>, u8), String> {
    let text = id.unwrap_or("").trim();
    if text.is_empty() {
        return Ok((None, 0));
    }
    let (serial, port_text) = match text.split_once('#') {
        Some((s, p)) => (s.trim(), Some(p.trim())),
        None => (text, None),
    };
    let port = match port_text {
        None => 0,
        Some(p) => match p.parse::<u8>() {
            Ok(v @ 0..=1) => v,
            _ => {
                return Err(format!(
                    "bladeRF: the input must be 0 (RX1) or 1 (RX2), not {p:?}"
                ))
            }
        },
    };
    let serial = if serial.is_empty() {
        None
    } else {
        Some(serial.to_string())
    };
    Ok((serial, port))
}

/// Name for the operator, for example "bladeRF 2.0 RX2" or "bladeRF 1".
pub(crate) fn board_label(name: &str, port: u8) -> String {
    let n = name.to_lowercase();
    if n.contains("bladerf2") || n.contains("2.0") {
        let input = if port == 1 { "RX2" } else { "RX1" };
        format!("bladeRF 2.0 {input}")
    } else if n.contains("bladerf1") {
        "bladeRF 1".to_string()
    } else {
        "bladeRF".to_string()
    }
}

/// Last eight characters of a long serial number, for telling two radios apart.
fn serial_tail(serial: &str) -> String {
    let n = serial.chars().count();
    if n > 8 {
        serial.chars().skip(n - 8).collect()
    } else {
        String::new()
    }
}

/// Reads a fixed-size C text field up to its NUL.
fn c_field(bytes: &[c_char]) -> String {
    let v: Vec<u8> = bytes.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    String::from_utf8_lossy(&v).trim().to_string()
}

/// Folders searched for FPGA images: the program folder and the library's search variable.
fn fpga_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.to_path_buf());
        }
    }
    if let Some(d) = std::env::var_os("BLADERF_SEARCH_DIR") {
        dirs.push(PathBuf::from(d));
    }
    dirs
}

/// Lists the radios the library can see. Each bladeRF 2.0 gives two entries, one per input.
pub fn probe() -> Vec<DeviceInfo> {
    let api = match load_api() {
        Ok(a) => a,
        Err(_) => return Vec::new(),
    };
    let mut list: *mut DevInfo = null_mut();
    // SAFETY: the library fills `list` with an array it owns; it is freed below.
    let n = unsafe { (api.get_device_list)(&mut list) };
    if n <= 0 || list.is_null() {
        return Vec::new();
    }
    let count = (n as usize).min(MAX_PROBE);
    let mut out = Vec::new();
    // SAFETY: the library reported `n` entries at `list`; `count` is no more than that.
    let devices = unsafe { std::slice::from_raw_parts(list, count) };
    for d in devices {
        let serial = c_field(&d.serial);
        let product = c_field(&d.product);
        let ports: u8 = if product.contains("2.0") { 2 } else { 1 };
        for port in 0..ports {
            let id = if port == 0 {
                serial.clone()
            } else {
                format!("{serial}#{port}")
            };
            let mut label = board_label(&product, port);
            let tail = serial_tail(&serial);
            if !tail.is_empty() {
                label = format!("{label} {tail}");
            }
            out.push(DeviceInfo {
                driver: "bladerf",
                label,
                id,
            });
        }
    }
    // SAFETY: `list` came from the library's list call above and is freed once.
    unsafe { (api.free_device_list)(list) };
    out
}

/// Opens the radio named by `id` ("serial" or "serial#port"), or the first one found.
pub fn open(id: Option<&str>) -> Result<Box<dyn RxRadio>, String> {
    let (serial, port) = parse_id(id)?;
    let api = load_api()?;
    let serial_arg = match &serial {
        Some(s) => Some(
            CString::new(format!("*:serial={s}"))
                .map_err(|_| "bladeRF: the serial number has a bad character".to_string())?,
        ),
        None => None,
    };
    let arg_ptr = serial_arg.as_ref().map_or(null(), |c| c.as_ptr());
    let mut dev: Dev = null_mut();
    // SAFETY: `dev` receives the handle; the argument is NULL or a valid C string.
    let r = unsafe { (api.open)(&mut dev, arg_ptr) };
    if r != 0 || dev.is_null() {
        let what = match &serial {
            Some(s) => format!("serial {s}"),
            None => "the first radio found".to_string(),
        };
        let mut msg = format!(
            "bladeRF: cannot open {what}: {}",
            describe_error(api.strerror_text(r), r)
        );
        if USB_ACCESS_CODES.contains(&r) {
            msg.push_str(". Check the USB cable and that this user may access the radio");
        }
        return Err(msg);
    }
    let mut rx = BladeRx {
        api,
        dev,
        ch: (port as c_int) << 1,
        label: String::new(),
        board: String::new(),
        freq_lo: 0,
        freq_hi: 0,
        rate_lo: 0,
        rate_hi: 0,
        gain_lo: -15.0,
        gain_hi: 60.0,
        gain_step: 0.0,
        rate: 0,
        streaming: false,
        raw: Vec::new(),
    };
    // Any error below drops `rx`, which closes the radio.
    rx.init()?;
    rx.label = board_label(&rx.board, port);
    Ok(Box::new(rx))
}

/// An open bladeRF input. Used from one thread at a time.
pub struct BladeRx {
    api: Api,
    dev: Dev,
    /// Channel number for the library: RX1 = 0, RX2 = 2.
    ch: c_int,
    label: String,
    board: String,
    freq_lo: u64,
    freq_hi: u64,
    rate_lo: u32,
    rate_hi: u32,
    gain_lo: f32,
    gain_hi: f32,
    gain_step: f32,
    rate: u32,
    streaming: bool,
    raw: Vec<i16>,
}

// SAFETY: the device handle is only used by the one thread that owns this value.
unsafe impl Send for BladeRx {}

impl BladeRx {
    fn init(&mut self) -> Result<(), String> {
        self.load_fpga_if_needed()?;
        self.board = self.board_name();
        let (flo, fhi) = default_freq_range(&self.board);
        self.freq_lo = flo;
        self.freq_hi = fhi;
        if let Some((lo, hi)) = self.read_range(self.api.get_frequency_range).and_then(usable) {
            self.freq_lo = lo.max(0.0) as u64;
            self.freq_hi = hi as u64;
        }
        let (rlo, rhi) = default_rate_range(&self.board);
        self.rate_lo = rlo;
        self.rate_hi = rhi;
        if let Some((lo, hi)) = self.read_range(self.api.get_sample_rate_range).and_then(usable) {
            self.rate_lo = to_u32(lo.ceil());
            self.rate_hi = to_u32(hi.floor());
        }
        if let Some((lo, hi, step)) = self
            .read_range(Some(self.api.get_gain_range))
            .and_then(usable_gain)
        {
            self.gain_lo = lo as f32;
            self.gain_hi = hi as f32;
            self.gain_step = step.max(0.0) as f32;
        }
        self.set_mode(GAIN_MODE_MANUAL)?;
        self.set_gain_value(0.0)?;
        self.set_sample_rate(DEFAULT_RATE)?;
        Ok(())
    }

    fn describe(&self, code: c_int) -> String {
        describe_error(self.api.strerror_text(code), code)
    }

    fn check(&self, code: c_int, what: &str) -> Result<(), String> {
        if code == 0 {
            Ok(())
        } else {
            Err(format!("bladeRF: {what}: {}", self.describe(code)))
        }
    }

    fn board_name(&self) -> String {
        let Some(f) = self.api.get_board_name else {
            return String::new();
        };
        // SAFETY: the library returns a static name or NULL for an open handle.
        let p = unsafe { f(self.dev) };
        if p.is_null() {
            String::new()
        } else {
            // SAFETY: the pointer is a valid NUL-terminated string from the library.
            unsafe { CStr::from_ptr(p) }.to_string_lossy().trim().to_string()
        }
    }

    /// Reads a range as (min, max, step) with the scale applied. None when the library has none.
    fn read_range(&self, f: Option<FnGetRange>) -> Option<(f64, f64, f64)> {
        let f = f?;
        let mut p: *const Range = null();
        // SAFETY: the library sets `p` to a range it owns, or leaves it NULL.
        let r = unsafe { f(self.dev, self.ch, &mut p) };
        if r != 0 || p.is_null() {
            return None;
        }
        // SAFETY: `p` was checked not to be NULL above.
        let range = unsafe { *p };
        let scale = if range.scale > 0.0 {
            range.scale as f64
        } else {
            1.0
        };
        Some((
            range.min as f64 * scale,
            range.max as f64 * scale,
            range.step as f64 * scale,
        ))
    }

    /// bladeRF 1 keeps its FPGA only until power-off. Load the image from the program folder when it is missing.
    fn load_fpga_if_needed(&self) -> Result<(), String> {
        let Some(configured) = self.api.is_fpga_configured else {
            return Ok(());
        };
        // SAFETY: the handle is open.
        if unsafe { configured(self.dev) } != 0 {
            return Ok(());
        }
        let mut size: c_int = 0;
        if let Some(f) = self.api.get_fpga_size {
            // SAFETY: `size` is a valid out value.
            unsafe { f(self.dev, &mut size) };
        }
        let names: &[&str] = match size {
            40 => &["hostedx40.rbf"],
            115 => &["hostedx115.rbf"],
            _ => &["hostedx40.rbf", "hostedx115.rbf"],
        };
        if let Some(load) = self.api.load_fpga {
            for dir in fpga_dirs() {
                for name in names {
                    let path = dir.join(name);
                    if !path.is_file() {
                        continue;
                    }
                    let c = CString::new(path.to_string_lossy().into_owned())
                        .map_err(|_| "bladeRF: the FPGA image path has a bad character".to_string())?;
                    // SAFETY: the path is a valid C string for the call.
                    let r = unsafe { load(self.dev, c.as_ptr()) };
                    if r != 0 {
                        return Err(format!(
                            "bladeRF: loading the FPGA image {} failed: {}",
                            path.display(),
                            self.describe(r)
                        ));
                    }
                    return Ok(());
                }
            }
        }
        Err("bladeRF: this radio needs its FPGA image (hostedx40.rbf or hostedx115.rbf). Get it from the vendor's site and put it in the program folder.".to_string())
    }

    fn set_mode(&self, mode: c_int) -> Result<(), String> {
        // SAFETY: the handle is open.
        let r = unsafe { (self.api.set_gain_mode)(self.dev, self.ch, mode) };
        self.check(r, "the gain mode was refused")
    }

    fn set_gain_value(&self, db: f32) -> Result<(), String> {
        let v = clamp_gain_db(db, self.gain_lo, self.gain_hi);
        // SAFETY: the handle is open.
        let r = unsafe { (self.api.set_gain)(self.dev, self.ch, v) };
        self.check(r, "the gain was refused")
    }
}

fn usable((lo, hi, _): (f64, f64, f64)) -> Option<(f64, f64)> {
    if lo.is_finite() && hi.is_finite() && lo >= 0.0 && hi > lo {
        Some((lo, hi))
    } else {
        None
    }
}

fn usable_gain((lo, hi, step): (f64, f64, f64)) -> Option<(f64, f64, f64)> {
    if lo.is_finite() && hi.is_finite() && step.is_finite() && hi > lo {
        Some((lo, hi, step))
    } else {
        None
    }
}

fn to_u32(v: f64) -> u32 {
    v.max(0.0).min(u32::MAX as f64) as u32
}

impl RxRadio for BladeRx {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn freq_range_hz(&self) -> (u64, u64) {
        (self.freq_lo, self.freq_hi)
    }

    fn set_frequency_hz(&mut self, hz: u64) -> Result<(), String> {
        // SAFETY: the handle is open.
        let r = unsafe { (self.api.set_frequency)(self.dev, self.ch, hz) };
        self.check(r, &format!("the radio refused {hz} Hz"))
    }

    fn sample_rates(&self) -> Vec<u32> {
        offered_rates(self.rate_lo, self.rate_hi)
    }

    fn set_sample_rate(&mut self, hz: u32) -> Result<u32, String> {
        let want = choose_rate(hz, self.rate_lo, self.rate_hi);
        let mut actual: c_uint = 0;
        // SAFETY: `actual` is a valid out value.
        let r = unsafe { (self.api.set_sample_rate)(self.dev, self.ch, want, &mut actual) };
        self.check(r, "the sample rate was refused")?;
        let rate = if actual > 0 { actual } else { want };
        let mut bw_actual: c_uint = 0;
        // SAFETY: `bw_actual` is a valid out value. A refused filter is left at the radio's own setting.
        let _ =
            unsafe { (self.api.set_bandwidth)(self.dev, self.ch, choose_bandwidth(rate), &mut bw_actual) };
        self.rate = rate;
        Ok(rate)
    }

    fn gain_stages(&self) -> Vec<GainStage> {
        vec![GainStage {
            name: GAIN_NAME.to_string(),
            min_db: self.gain_lo,
            max_db: self.gain_hi,
            step_db: self.gain_step,
            auto: true,
        }]
    }

    fn set_gain(&mut self, stage: &str, setting: GainSetting) -> Result<(), String> {
        if stage != GAIN_NAME {
            return Err(format!(
                "bladeRF has no gain stage named {stage:?}; it has {GAIN_NAME:?}"
            ));
        }
        match setting {
            GainSetting::Auto => self.set_mode(GAIN_MODE_DEFAULT),
            GainSetting::Manual(db) => {
                self.set_mode(GAIN_MODE_MANUAL)?;
                self.set_gain_value(db)
            }
        }
    }

    fn start(&mut self) -> Result<(), String> {
        if self.streaming {
            return Ok(());
        }
        // SAFETY: the handle is open; the arguments are the stream settings above.
        let r = unsafe {
            (self.api.sync_config)(
                self.dev,
                LAYOUT_RX_X1,
                FORMAT_SC16_Q11,
                STREAM_BUFFERS,
                STREAM_BUFFER_SAMPLES,
                STREAM_TRANSFERS,
                STREAM_TIMEOUT_MS,
            )
        };
        self.check(r, "cannot start the sample stream")?;
        // SAFETY: the handle is open.
        let r = unsafe { (self.api.enable_module)(self.dev, self.ch, true) };
        self.check(r, "cannot enable the receiver")?;
        self.streaming = true;
        Ok(())
    }

    fn read_iq(&mut self, out: &mut [(f32, f32)]) -> Result<usize, String> {
        if !self.streaming {
            return Err("bladeRF: the stream is not started".to_string());
        }
        let n = out.len().min(MAX_BLOCK_SAMPLES);
        if n == 0 {
            return Ok(0);
        }
        self.raw.resize(2 * n, 0);
        // SAFETY: `raw` holds 2 * n i16 values, which is what the library writes for n complex samples.
        let r = unsafe {
            (self.api.sync_rx)(
                self.dev,
                self.raw.as_mut_ptr().cast::<c_void>(),
                n as c_uint,
                null_mut(),
                READ_TIMEOUT_MS,
            )
        };
        if r == 0 {
            convert_samples(&self.raw[..2 * n], &mut out[..n]);
            return Ok(n);
        }
        if r == TIMEOUT_CODE {
            return Ok(0);
        }
        Err(format!("bladeRF: the stream failed: {}", self.describe(r)))
    }

    fn stop(&mut self) {
        if self.streaming {
            // SAFETY: the handle is open. The result is not needed when stopping.
            let _ = unsafe { (self.api.enable_module)(self.dev, self.ch, false) };
            self.streaming = false;
        }
    }
}

impl Drop for BladeRx {
    fn drop(&mut self) {
        self.stop();
        if !self.dev.is_null() {
            // SAFETY: the handle came from a successful open and is closed once.
            unsafe { (self.api.close)(self.dev) };
            self.dev = null_mut();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_is_default_for_zero_and_held_in_range() {
        assert_eq!(choose_rate(0, 520_834, 61_440_000), DEFAULT_RATE);
        assert_eq!(choose_rate(2_000_000, 520_834, 61_440_000), 2_000_000);
        assert_eq!(choose_rate(100, 520_834, 61_440_000), 520_834);
        assert_eq!(choose_rate(90_000_000, 80_000, 40_000_000), 40_000_000);
    }

    #[test]
    fn bandwidth_is_95_percent_with_floor() {
        assert_eq!(choose_bandwidth(10_000_000), 9_500_000);
        assert_eq!(choose_bandwidth(100_000), MIN_BANDWIDTH);
    }

    #[test]
    fn offered_rates_are_sorted_and_hold_range_ends() {
        let v = offered_rates(80_000, 40_000_000);
        assert_eq!(v.first(), Some(&80_000));
        assert_eq!(v.last(), Some(&40_000_000));
        assert!(v.contains(&10_000_000));
        assert!(!v.contains(&50_000_000));
        assert!(v.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn gain_is_clamped_and_rounded() {
        assert_eq!(clamp_gain_db(-30.0, -15.0, 60.0), -15);
        assert_eq!(clamp_gain_db(70.0, -15.0, 60.0), 60);
        assert_eq!(clamp_gain_db(12.4, -15.0, 60.0), 12);
        assert_eq!(clamp_gain_db(12.6, -15.0, 60.0), 13);
        assert_eq!(clamp_gain_db(f32::NAN, -15.0, 60.0), -15);
    }

    #[test]
    fn samples_convert_to_unit_range() {
        let raw = [2047i16, -2048, 0, 1024];
        let mut out = [(0.0f32, 0.0f32); 2];
        convert_samples(&raw, &mut out);
        assert!((out[0].0 - 2047.0 / 2048.0).abs() < 1e-6);
        assert_eq!(out[0].1, -1.0);
        assert_eq!(out[1], (0.0, 0.5));
    }

    #[test]
    fn missing_library_error_says_what_to_do() {
        let err = load_library(&["/nonexistent/kahf-test-libbladeRF.so"])
            .map_err(|e| library_missing_text(&e))
            .unwrap_err();
        assert!(err.contains("library not found"), "{err}");
        assert!(err.starts_with("bladeRF: cannot load"), "{err}");
        assert!(err.contains("Install the bladeRF host library"), "{err}");
    }

    #[test]
    fn error_text_uses_library_message_or_code() {
        assert_eq!(
            describe_error(Some("I/O error".into()), -5),
            "I/O error (code -5)"
        );
        assert_eq!(describe_error(None, -5), "error code -5");
        assert_eq!(describe_error(Some("  ".into()), -7), "error code -7");
    }

    #[test]
    fn id_parsing_accepts_serial_and_input() {
        assert_eq!(parse_id(None).unwrap(), (None, 0));
        assert_eq!(parse_id(Some("  ")).unwrap(), (None, 0));
        assert_eq!(parse_id(Some("ABC123")).unwrap(), (Some("ABC123".into()), 0));
        assert_eq!(parse_id(Some("ABC123#1")).unwrap(), (Some("ABC123".into()), 1));
        assert!(parse_id(Some("ABC123#2")).is_err());
    }

    #[test]
    fn board_defaults_and_labels() {
        assert_eq!(default_rate_range("bladerf1"), (80_000, 40_000_000));
        assert_eq!(default_rate_range("bladerf2"), (520_834, 61_440_000));
        assert_eq!(default_freq_range("bladerf1").0, 237_500_000);
        assert_eq!(default_freq_range("bladerf2").1, 6_000_000_000);
        assert_eq!(board_label("bladerf2", 1), "bladeRF 2.0 RX2");
        assert_eq!(board_label("bladeRF 2.0 micro", 0), "bladeRF 2.0 RX1");
        assert_eq!(board_label("bladerf1", 0), "bladeRF 1");
    }
}
