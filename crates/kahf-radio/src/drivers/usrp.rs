//! Driver for USRP radios through the UHD C library. The library is loaded at run
//! time, so the program starts on a machine without it. Only the receive side is here.
//!
//! Samples come out of UHD as interleaved complex floats (fc32); the wire format
//! between radio and computer is 16-bit (sc16).

use std::ffi::{c_char, c_int, c_void, CString};
use std::net::IpAddr;
use std::ptr::null_mut;

use libloading::Library;

use crate::drivers::common::load_library;
use crate::{DeviceInfo, GainSetting, GainStage, RxRadio};

/// Device hint for the probe: the USB radios of the B2xx family. Network radios need an address.
const USB_HINT: &str = "type=b200";
/// Longest wait of one read, in seconds.
const READ_WAIT_S: f64 = 0.2;
/// Reads with no samples in a row before the radio counts as lost (25 x 0.2 s = 5 s).
const SILENT_READS_LIMIT: u32 = 25;
/// Stream errors in a row before the radio counts as lost.
const FAILURE_LIMIT: u32 = 20;
const STREAM_START: c_int = 97;
const STREAM_STOP: c_int = 111;
const POLICY_AUTO: c_int = 65;
const ERR_NONE: c_int = 0;
const ERR_TIMEOUT: c_int = 1;
const ERR_OVERFLOW: c_int = 8;
const DEFAULT_BLOCK: usize = 16_384;
const MAX_BLOCK: usize = 65_536;
const DEFAULT_RATE: u32 = 10_000_000;
const FALLBACK_FREQ: (f64, f64) = (70e6, 6e9);
const FALLBACK_GAIN: (f32, f32) = (0.0, 76.0);
const FALLBACK_RATE_RANGE: (f64, f64) = (200e3, 56e6);
/// The one gain stage UHD reports for these radios.
const GAIN_STAGE: &str = "gain";
/// Common sample rates; the ones inside the radio's range are offered.
const RATE_STEPS: &[u32] = &[
    200_000, 500_000, 1_000_000, 2_000_000, 4_000_000, 5_000_000, 8_000_000, 10_000_000, 12_500_000,
    16_000_000, 20_000_000, 25_000_000, 40_000_000, 50_000_000, 56_000_000,
];

#[cfg(target_os = "macos")]
const LIB_NAMES: &[&str] = &["libuhd.dylib"];
#[cfg(windows)]
const LIB_NAMES: &[&str] = &["uhd.dll"];
#[cfg(all(unix, not(target_os = "macos")))]
const LIB_NAMES: &[&str] = &["libuhd.so.4", "libuhd.so"];

const CPU_FORMAT: &[u8] = b"fc32\0";
const WIRE_FORMAT: &[u8] = b"sc16\0";
const EMPTY: &[u8] = b"\0";

// ---- C types ----------------------------------------------------------------------------

#[repr(C)]
struct StreamArgs {
    cpu_format: *const c_char,
    otw_format: *const c_char,
    args: *const c_char,
    channel_list: *mut usize,
    n_channels: c_int,
}

#[repr(C)]
struct StreamCmd {
    stream_mode: c_int,
    num_samps: usize,
    stream_now: bool,
    time_spec_full_secs: i64,
    time_spec_frac_secs: f64,
}

#[repr(C)]
struct TuneRequest {
    target_freq: f64,
    rf_freq_policy: c_int,
    rf_freq: f64,
    dsp_freq_policy: c_int,
    dsp_freq: f64,
    args: *const c_char,
}

#[repr(C)]
#[derive(Default)]
struct TuneResult {
    clipped_rf_freq: f64,
    target_rf_freq: f64,
    actual_rf_freq: f64,
    target_dsp_freq: f64,
    actual_dsp_freq: f64,
}

// ---- function types (all return an error code, 0 = success) -----------------------------

type FnFind = unsafe extern "C" fn(*const c_char, *mut *mut c_void) -> c_int;
type FnHandleMake = unsafe extern "C" fn(*mut *mut c_void) -> c_int;
type FnHandleFree = unsafe extern "C" fn(*mut *mut c_void) -> c_int;
type FnSize = unsafe extern "C" fn(*mut c_void, *mut usize) -> c_int;
type FnStrAt = unsafe extern "C" fn(*mut c_void, usize, *mut c_char, usize) -> c_int;
type FnUsrpMake = unsafe extern "C" fn(*mut *mut c_void, *const c_char) -> c_int;
type FnSetRate = unsafe extern "C" fn(*mut c_void, f64, usize) -> c_int;
type FnGetRate = unsafe extern "C" fn(*mut c_void, usize, *mut f64) -> c_int;
type FnRange = unsafe extern "C" fn(*mut c_void, usize, *mut c_void) -> c_int;
type FnSetFreq = unsafe extern "C" fn(*mut c_void, *mut TuneRequest, usize, *mut TuneResult) -> c_int;
type FnSetGain = unsafe extern "C" fn(*mut c_void, f64, usize, *const c_char) -> c_int;
type FnGainRange = unsafe extern "C" fn(*mut c_void, *const c_char, usize, *mut c_void) -> c_int;
type FnGetStream = unsafe extern "C" fn(*mut c_void, *mut StreamArgs, *mut c_void) -> c_int;
type FnStreamCmd = unsafe extern "C" fn(*mut c_void, *const StreamCmd) -> c_int;
type FnRecv = unsafe extern "C" fn(
    *mut c_void,
    *mut *mut c_void,
    usize,
    *mut *mut c_void,
    f64,
    bool,
    *mut usize,
) -> c_int;
type FnMdErr = unsafe extern "C" fn(*mut c_void, *mut c_int) -> c_int;
type FnMetaVal = unsafe extern "C" fn(*mut c_void, *mut f64) -> c_int;
type FnUsrpErr = unsafe extern "C" fn(*mut c_void, *mut c_char, usize) -> c_int;
type FnGlobalErr = unsafe extern "C" fn(*mut c_char, usize) -> c_int;

/// The UHD library, loaded, with the functions this driver calls.
struct Uhd {
    _lib: Library,
    find: FnFind,
    strvec_make: FnHandleMake,
    strvec_free: FnHandleFree,
    strvec_size: FnSize,
    strvec_at: FnStrAt,
    usrp_make: FnUsrpMake,
    usrp_free: FnHandleFree,
    set_rx_rate: FnSetRate,
    get_rx_rate: FnGetRate,
    get_rx_rates: FnRange,
    set_rx_freq: FnSetFreq,
    set_rx_gain: FnSetGain,
    get_rx_gain_range: FnGainRange,
    get_rx_stream: FnGetStream,
    rx_streamer_make: FnHandleMake,
    rx_streamer_free: FnHandleFree,
    rx_streamer_max_num_samps: FnSize,
    rx_streamer_recv: FnRecv,
    rx_streamer_issue_stream_cmd: FnStreamCmd,
    rx_metadata_make: FnHandleMake,
    rx_metadata_free: FnHandleFree,
    rx_metadata_error_code: FnMdErr,
    meta_range_make: FnHandleMake,
    meta_range_free: FnHandleFree,
    meta_range_start: FnMetaVal,
    meta_range_stop: FnMetaVal,
    // optional: the library may be older without them
    get_rx_freq_range: Option<FnRange>,
    set_rx_bandwidth: Option<FnSetRate>,
    usrp_last_error: Option<FnUsrpErr>,
    get_last_error: Option<FnGlobalErr>,
}

/// Looks up one function by its C name. The caller keeps the library alive.
unsafe fn sym<T: Copy>(lib: &Library, name: &str) -> Result<T, String> {
    let mut bytes = name.as_bytes().to_vec();
    bytes.push(0);
    match lib.get::<T>(&bytes) {
        Ok(f) => Ok(*f),
        Err(_) => Err(format!("the UHD library has no function {name}; it is too old")),
    }
}

impl Uhd {
    fn from_library(lib: Library) -> Result<Uhd, String> {
        // SAFETY: each symbol is looked up with the function type UHD's C header gives it.
        unsafe {
            let uhd = Uhd {
                find: sym(&lib, "uhd_usrp_find")?,
                strvec_make: sym(&lib, "uhd_string_vector_make")?,
                strvec_free: sym(&lib, "uhd_string_vector_free")?,
                strvec_size: sym(&lib, "uhd_string_vector_size")?,
                strvec_at: sym(&lib, "uhd_string_vector_at")?,
                usrp_make: sym(&lib, "uhd_usrp_make")?,
                usrp_free: sym(&lib, "uhd_usrp_free")?,
                set_rx_rate: sym(&lib, "uhd_usrp_set_rx_rate")?,
                get_rx_rate: sym(&lib, "uhd_usrp_get_rx_rate")?,
                get_rx_rates: sym(&lib, "uhd_usrp_get_rx_rates")?,
                set_rx_freq: sym(&lib, "uhd_usrp_set_rx_freq")?,
                set_rx_gain: sym(&lib, "uhd_usrp_set_rx_gain")?,
                get_rx_gain_range: sym(&lib, "uhd_usrp_get_rx_gain_range")?,
                get_rx_stream: sym(&lib, "uhd_usrp_get_rx_stream")?,
                rx_streamer_make: sym(&lib, "uhd_rx_streamer_make")?,
                rx_streamer_free: sym(&lib, "uhd_rx_streamer_free")?,
                rx_streamer_max_num_samps: sym(&lib, "uhd_rx_streamer_max_num_samps")?,
                rx_streamer_recv: sym(&lib, "uhd_rx_streamer_recv")?,
                rx_streamer_issue_stream_cmd: sym(&lib, "uhd_rx_streamer_issue_stream_cmd")?,
                rx_metadata_make: sym(&lib, "uhd_rx_metadata_make")?,
                rx_metadata_free: sym(&lib, "uhd_rx_metadata_free")?,
                rx_metadata_error_code: sym(&lib, "uhd_rx_metadata_error_code")?,
                meta_range_make: sym(&lib, "uhd_meta_range_make")?,
                meta_range_free: sym(&lib, "uhd_meta_range_free")?,
                meta_range_start: sym(&lib, "uhd_meta_range_start")?,
                meta_range_stop: sym(&lib, "uhd_meta_range_stop")?,
                get_rx_freq_range: sym(&lib, "uhd_usrp_get_rx_freq_range").ok(),
                set_rx_bandwidth: sym(&lib, "uhd_usrp_set_rx_bandwidth").ok(),
                usrp_last_error: sym(&lib, "uhd_usrp_last_error").ok(),
                get_last_error: sym(&lib, "uhd_get_last_error").ok(),
                _lib: lib,
            };
            Ok(uhd)
        }
    }

    /// Loads the UHD library. The error says what to install.
    fn load() -> Result<Uhd, String> {
        load_uhd(LIB_NAMES)
    }

    /// UHD's text for the last failure, on one line. Empty when UHD says nothing.
    fn text(&self, dev: *mut c_void) -> String {
        let mut out = String::new();
        if !dev.is_null() {
            if let Some(f) = self.usrp_last_error {
                out = read_text(|p, n| unsafe { f(dev, p, n) });
            }
        }
        if out.is_empty() {
            if let Some(f) = self.get_last_error {
                out = read_text(|p, n| unsafe { f(p, n) });
            }
        }
        clean_text(&out)
    }

    /// Lists the radios UHD finds for `hint`, as its own comma-separated entries.
    fn find_entries(&self, hint: &str) -> Result<Vec<String>, String> {
        let chint = CString::new(hint).map_err(|_| "USRP: bad search hint".to_string())?;
        let mut v: *mut c_void = null_mut();
        // SAFETY: the handle is made here and freed below on every path.
        unsafe {
            if (self.strvec_make)(&mut v) != 0 || v.is_null() {
                return Err("USRP: cannot make a list for the radios".into());
            }
            let rc = (self.find)(chint.as_ptr(), &mut v);
            let mut out = Vec::new();
            if rc == 0 {
                let mut n = 0usize;
                (self.strvec_size)(v, &mut n);
                for i in 0..n.min(32) {
                    let mut b = [0 as c_char; 512];
                    if (self.strvec_at)(v, i, b.as_mut_ptr(), b.len()) == 0 {
                        out.push(c_buf_to_string(&b));
                    }
                }
            }
            if !v.is_null() {
                (self.strvec_free)(&mut v);
            }
            if rc != 0 {
                return Err(format!(
                    "USRP: the radio search failed: {}",
                    self.text(null_mut())
                ));
            }
            Ok(out)
        }
    }

    /// Reads a range that UHD fills into a meta range. None when UHD cannot give one.
    fn read_range(&self, f: impl FnOnce(*mut c_void) -> c_int) -> Option<(f64, f64)> {
        let mut rg: *mut c_void = null_mut();
        // SAFETY: the meta range is made here and freed before returning.
        unsafe {
            if (self.meta_range_make)(&mut rg) != 0 || rg.is_null() {
                return None;
            }
            let mut out = None;
            if f(rg) == 0 {
                let (mut a, mut b) = (0.0f64, 0.0f64);
                if (self.meta_range_start)(rg, &mut a) == 0
                    && (self.meta_range_stop)(rg, &mut b) == 0
                    && b > a
                {
                    out = Some((a, b));
                }
            }
            (self.meta_range_free)(&mut rg);
            out
        }
    }
}

/// Copies a NUL-ended C buffer into a String.
fn c_buf_to_string(b: &[c_char]) -> String {
    let bytes: Vec<u8> = b.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Runs a UHD text call into a 1024-byte buffer and returns the text.
fn read_text(f: impl FnOnce(*mut c_char, usize) -> c_int) -> String {
    let mut buf = [0 as c_char; 1024];
    f(buf.as_mut_ptr(), buf.len());
    c_buf_to_string(&buf)
}

/// Loads UHD from the first name that works. The error says what to install.
fn load_uhd(names: &[&str]) -> Result<Uhd, String> {
    let lib = load_library(names).map_err(|e| missing_library_message(&e))?;
    Uhd::from_library(lib)
}

/// The message when the UHD library cannot be loaded.
pub fn missing_library_message(detail: &str) -> String {
    format!(
        "USRP: the UHD library was not found, so USRP radios cannot be used. \
         Install UHD (the USRP driver library) and try again. Details: {detail}"
    )
}

/// Plain one-line text from a UHD message, at most 400 characters.
pub fn clean_text(s: &str) -> String {
    let one_line: String = s
        .chars()
        .map(|c| if matches!(c, '\n' | '\r' | '\t') { ' ' } else { c })
        .collect();
    let t = one_line.trim();
    if t.chars().count() > 400 {
        format!("{}...", t.chars().take(400).collect::<String>())
    } else {
        t.to_string()
    }
}

/// The message when a radio cannot be opened, from UHD's text.
pub fn open_error_message(text: &str) -> String {
    let t = text.to_ascii_lowercase();
    if t.contains("images") || t.contains("image not found") || t.contains("could not find path for image") {
        "USRP: the firmware and FPGA images were not found. Run the UHD image download \
         once, or set UHD_IMAGES_DIR to the folder that holds them."
            .to_string()
    } else if text.is_empty() {
        "USRP: cannot open the radio. Check that it is connected and not in use by another application."
            .to_string()
    } else {
        format!("USRP: cannot open the radio: {text}")
    }
}

/// Turns the operator's id into UHD device arguments.
/// Empty: any radio. "key=value,...": passed through. An IP address: a network radio.
/// Anything else: a serial number.
pub fn device_args(id: Option<&str>) -> Result<String, String> {
    let t = id.unwrap_or("").trim();
    if t.is_empty() {
        return Ok(String::new());
    }
    let bad = || {
        format!(
            "USRP: the radio id {t:?} is not a serial number, an IP address, or device \
             arguments such as type=b200,serial=ABC"
        )
    };
    if t.contains('=') {
        let chars_ok = t
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "=,.:-_".contains(c));
        let parts_ok = t.split(',').all(|p| match p.split_once('=') {
            Some((k, v)) => !k.is_empty() && !v.is_empty(),
            None => false,
        });
        return if chars_ok && parts_ok {
            Ok(t.to_string())
        } else {
            Err(bad())
        };
    }
    if t.parse::<IpAddr>().is_ok() {
        return Ok(format!("addr={t}"));
    }
    if t.chars().all(|c| c.is_ascii_alphanumeric()) {
        Ok(format!("serial={t}"))
    } else {
        Err(bad())
    }
}

/// Reads one UHD list entry such as "type=b200,name=,serial=30C6A1,product=B200".
pub fn parse_find_entry(entry: &str) -> DeviceInfo {
    let field = |key: &str| -> String {
        entry
            .split(',')
            .find_map(|p| {
                p.split_once('=')
                    .filter(|(k, _)| *k == key)
                    .map(|(_, v)| v.trim().to_string())
            })
            .unwrap_or_default()
    };
    let serial = field("serial");
    let mut product = field("product");
    if product.is_empty() {
        product = field("type");
    }
    let mut label = if product.is_empty() {
        "USRP".to_string()
    } else {
        format!("USRP {product}")
    };
    if !serial.is_empty() {
        label = format!("{label} serial {serial}");
    }
    DeviceInfo {
        driver: "usrp",
        label,
        id: serial,
    }
}

/// Common rates inside the range [lo, hi], plus the ends of the range, ascending.
pub fn rate_candidates(lo: f64, hi: f64) -> Vec<u32> {
    let cap = |x: f64| x.clamp(0.0, u32::MAX as f64) as u32;
    let mut v: Vec<u32> = RATE_STEPS
        .iter()
        .copied()
        .filter(|&r| r as f64 >= lo && r as f64 <= hi)
        .collect();
    if hi >= lo && hi > 0.0 {
        v.push(cap(lo.ceil()));
        v.push(cap(hi.floor()));
    }
    v.retain(|&r| r > 0);
    v.sort_unstable();
    v.dedup();
    v
}

/// The offered rate closest to `want`. None when nothing is offered.
pub fn pick_rate(offered: &[u32], want: u32) -> Option<u32> {
    offered.iter().copied().min_by_key(|&r| r.abs_diff(want))
}

/// A gain value held inside the stage's range.
pub fn clamp_gain(value: f32, min: f32, max: f32) -> f32 {
    value.max(min).min(max)
}

/// Converts interleaved fc32 samples (I, Q, I, Q, ...) into pairs. Values are held
/// to -1..1, and NaN becomes 0. Returns how many pairs were written.
#[allow(clippy::chunks_exact_to_as_chunks)]
pub fn fc32_to_pairs(src: &[f32], dst: &mut [(f32, f32)]) -> usize {
    fn clean(v: f32) -> f32 {
        if v.is_nan() {
            0.0
        } else {
            v.clamp(-1.0, 1.0)
        }
    }
    let n = (src.len() / 2).min(dst.len());
    for (d, s) in dst[..n].iter_mut().zip(src.chunks_exact(2)) {
        *d = (clean(s[0]), clean(s[1]));
    }
    n
}

// ---- the open radio ---------------------------------------------------------------------

pub struct UsrpRadio {
    uhd: Uhd,
    dev: *mut c_void,
    streamer: *mut c_void,
    md: *mut c_void,
    streaming: bool,
    label: String,
    rates: Vec<u32>,
    freq_lo: u64,
    freq_hi: u64,
    gain_min: f32,
    gain_max: f32,
    max_samps: usize,
    buf: Vec<f32>,
    failures: u32,
    silent: u32,
    dropped: u64,
}

// SAFETY: the raw UHD handles are only used from the one thread that owns the radio,
// and the trait promises one thread at a time.
unsafe impl Send for UsrpRadio {}

impl UsrpRadio {
    fn fault(&mut self, what: &str) -> Result<usize, String> {
        self.failures += 1;
        if self.failures > FAILURE_LIMIT {
            Err(format!("USRP: {what}: {}", self.uhd.text(self.dev)))
        } else {
            Ok(0)
        }
    }

    fn start_stream(&mut self) -> Result<(), String> {
        // SAFETY: each handle is made here, checked, and freed in release_stream.
        unsafe {
            if (self.uhd.rx_streamer_make)(&mut self.streamer) != 0 || self.streamer.is_null() {
                return Err(format!(
                    "USRP: cannot make the sample stream: {}",
                    self.uhd.text(self.dev)
                ));
            }
            if (self.uhd.rx_metadata_make)(&mut self.md) != 0 || self.md.is_null() {
                return Err("USRP: cannot make the sample packet header".into());
            }
            let mut chan: usize = 0;
            let mut sa = StreamArgs {
                cpu_format: CPU_FORMAT.as_ptr().cast(),
                otw_format: WIRE_FORMAT.as_ptr().cast(),
                args: EMPTY.as_ptr().cast(),
                channel_list: &mut chan as *mut usize,
                n_channels: 1,
            };
            if (self.uhd.get_rx_stream)(self.dev, &mut sa, self.streamer) != 0 {
                return Err(format!(
                    "USRP: cannot set up the sample stream: {}",
                    self.uhd.text(self.dev)
                ));
            }
            let mut n: usize = 0;
            (self.uhd.rx_streamer_max_num_samps)(self.streamer, &mut n);
            self.max_samps = if n == 0 { DEFAULT_BLOCK } else { n.min(MAX_BLOCK) };
            self.buf = vec![0.0; 2 * self.max_samps];
            let cmd = StreamCmd {
                stream_mode: STREAM_START,
                num_samps: 0,
                stream_now: true,
                time_spec_full_secs: 0,
                time_spec_frac_secs: 0.0,
            };
            if (self.uhd.rx_streamer_issue_stream_cmd)(self.streamer, &cmd) != 0 {
                return Err(format!(
                    "USRP: cannot start the sample stream: {}",
                    self.uhd.text(self.dev)
                ));
            }
        }
        self.failures = 0;
        self.silent = 0;
        Ok(())
    }

    fn release_stream(&mut self) {
        // SAFETY: only non-null handles are freed; UHD clears the pointer it frees.
        unsafe {
            if !self.streamer.is_null() {
                (self.uhd.rx_streamer_free)(&mut self.streamer);
            }
            if !self.md.is_null() {
                (self.uhd.rx_metadata_free)(&mut self.md);
            }
        }
        self.streamer = null_mut();
        self.md = null_mut();
    }
}

impl RxRadio for UsrpRadio {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn freq_range_hz(&self) -> (u64, u64) {
        (self.freq_lo, self.freq_hi)
    }

    fn set_frequency_hz(&mut self, hz: u64) -> Result<(), String> {
        if hz < self.freq_lo || hz > self.freq_hi {
            return Err(format!(
                "USRP: {hz} Hz is outside the tuning range {} to {} Hz",
                self.freq_lo, self.freq_hi
            ));
        }
        let mut req = TuneRequest {
            target_freq: hz as f64,
            rf_freq_policy: POLICY_AUTO,
            rf_freq: 0.0,
            dsp_freq_policy: POLICY_AUTO,
            dsp_freq: 0.0,
            args: EMPTY.as_ptr().cast(),
        };
        let mut res = TuneResult::default();
        // SAFETY: the request and result live on this stack frame during the call.
        let rc = unsafe { (self.uhd.set_rx_freq)(self.dev, &mut req, 0, &mut res) };
        if rc != 0 {
            return Err(format!(
                "USRP: the radio refused {hz} Hz: {}",
                self.uhd.text(self.dev)
            ));
        }
        Ok(())
    }

    fn sample_rates(&self) -> Vec<u32> {
        self.rates.clone()
    }

    fn set_sample_rate(&mut self, hz: u32) -> Result<u32, String> {
        let want = pick_rate(&self.rates, hz).ok_or("USRP: the radio offers no sample rates")?;
        // SAFETY: the device handle is valid for the life of the radio.
        unsafe {
            if (self.uhd.set_rx_rate)(self.dev, want as f64, 0) != 0 {
                return Err(format!(
                    "USRP: the sample rate {want} was refused: {}",
                    self.uhd.text(self.dev)
                ));
            }
            if let Some(f) = self.uhd.set_rx_bandwidth {
                let bw = (want as f64 * 0.95).max(0.2e6);
                f(self.dev, bw, 0);
            }
            let mut actual = want as f64;
            (self.uhd.get_rx_rate)(self.dev, 0, &mut actual);
            Ok(actual.round().max(0.0) as u32)
        }
    }

    fn gain_stages(&self) -> Vec<GainStage> {
        vec![GainStage {
            name: GAIN_STAGE.to_string(),
            min_db: self.gain_min,
            max_db: self.gain_max,
            step_db: 0.0,
            auto: false,
        }]
    }

    fn set_gain(&mut self, stage: &str, setting: GainSetting) -> Result<(), String> {
        if stage != GAIN_STAGE {
            return Err(format!(
                "USRP: no gain stage named {stage:?}; the radio has {GAIN_STAGE}"
            ));
        }
        let value = match setting {
            GainSetting::Auto => return Err("USRP: this radio has no automatic gain setting".into()),
            GainSetting::Manual(v) if !v.is_finite() => {
                return Err("USRP: the gain value is not a number".into())
            }
            GainSetting::Manual(v) => clamp_gain(v, self.gain_min, self.gain_max),
        };
        // SAFETY: the device handle is valid; the empty name asks for the overall gain.
        let rc = unsafe { (self.uhd.set_rx_gain)(self.dev, value as f64, 0, EMPTY.as_ptr().cast()) };
        if rc != 0 {
            return Err(format!(
                "USRP: the gain {value} dB was refused: {}",
                self.uhd.text(self.dev)
            ));
        }
        Ok(())
    }

    fn start(&mut self) -> Result<(), String> {
        if self.streaming {
            return Ok(());
        }
        match self.start_stream() {
            Ok(()) => {
                self.streaming = true;
                Ok(())
            }
            Err(e) => {
                self.release_stream();
                Err(e)
            }
        }
    }

    fn read_iq(&mut self, out: &mut [(f32, f32)]) -> Result<usize, String> {
        if out.is_empty() {
            return Ok(0);
        }
        if !self.streaming {
            return Err("USRP: the radio is not started".into());
        }
        let want = out.len().min(self.max_samps);
        let mut bufs = [self.buf.as_mut_ptr() as *mut c_void];
        let mut got: usize = 0;
        // SAFETY: bufs points at self.buf, which holds room for max_samps complex samples.
        let rc = unsafe {
            (self.uhd.rx_streamer_recv)(
                self.streamer,
                bufs.as_mut_ptr(),
                want,
                &mut self.md,
                READ_WAIT_S,
                false,
                &mut got,
            )
        };
        if rc != 0 {
            return self.fault("the stream failed");
        }
        let got = got.min(want);
        let mut code: c_int = ERR_NONE;
        // SAFETY: the metadata handle was made in start_stream.
        unsafe { (self.uhd.rx_metadata_error_code)(self.md, &mut code) };
        match code {
            ERR_NONE | ERR_OVERFLOW if got > 0 => {
                if code == ERR_OVERFLOW {
                    self.dropped += 1;
                }
                self.failures = 0;
                self.silent = 0;
                let n = got.min(out.len());
                Ok(fc32_to_pairs(&self.buf[..2 * n], &mut out[..n]))
            }
            ERR_NONE => {
                self.failures = 0;
                self.silent = self.silent.saturating_add(1);
                self.check_silence()
            }
            ERR_OVERFLOW => {
                self.dropped += 1;
                Ok(0)
            }
            ERR_TIMEOUT => {
                self.silent = self.silent.saturating_add(1);
                self.check_silence()
            }
            other => self.fault(&format!("the stream reported code {other}")),
        }
    }

    fn stop(&mut self) {
        if !self.streaming {
            self.release_stream();
            return;
        }
        let cmd = StreamCmd {
            stream_mode: STREAM_STOP,
            num_samps: 0,
            stream_now: true,
            time_spec_full_secs: 0,
            time_spec_frac_secs: 0.0,
        };
        // SAFETY: the streamer handle is valid while streaming.
        unsafe { (self.uhd.rx_streamer_issue_stream_cmd)(self.streamer, &cmd) };
        self.release_stream();
        self.streaming = false;
    }

    /// Overflow events seen so far. UHD does not give the count of lost samples here,
    /// so each overflow counts once.
    fn dropped_samples(&self) -> u64 {
        self.dropped
    }
}

impl UsrpRadio {
    fn check_silence(&mut self) -> Result<usize, String> {
        if self.silent >= SILENT_READS_LIMIT {
            Err("USRP: no samples for 5 seconds; check the cable and the power".into())
        } else {
            Ok(0)
        }
    }
}

impl Drop for UsrpRadio {
    fn drop(&mut self) {
        self.stop();
        self.release_stream();
        if !self.dev.is_null() {
            // SAFETY: the device handle came from usrp_make and is freed once.
            unsafe { (self.uhd.usrp_free)(&mut self.dev) };
            self.dev = null_mut();
        }
    }
}

// ---- probe and open ---------------------------------------------------------------------

/// Finds the USB radios of the B2xx family. Returns an empty list when UHD is missing.
pub fn probe() -> Vec<DeviceInfo> {
    let Ok(uhd) = Uhd::load() else {
        return Vec::new();
    };
    match uhd.find_entries(USB_HINT) {
        Ok(list) => list.iter().map(|e| parse_find_entry(e)).collect(),
        Err(_) => Vec::new(),
    }
}

/// Opens a radio by serial number, IP address or device arguments. None opens any radio.
pub fn open(id: Option<&str>) -> Result<Box<dyn RxRadio>, String> {
    let args = device_args(id)?;
    let uhd = Uhd::load()?;
    let cargs = CString::new(args.clone()).map_err(|_| "USRP: bad device address".to_string())?;
    let mut dev: *mut c_void = null_mut();
    // SAFETY: on failure the handle is freed if UHD made one.
    let rc = unsafe { (uhd.usrp_make)(&mut dev, cargs.as_ptr()) };
    if rc != 0 || dev.is_null() {
        let text = uhd.text(dev);
        if !dev.is_null() {
            unsafe { (uhd.usrp_free)(&mut dev) };
        }
        return Err(open_error_message(&text));
    }

    let rates = uhd
        .read_range(|rg| unsafe { (uhd.get_rx_rates)(dev, 0, rg) })
        .map(|(lo, hi)| rate_candidates(lo, hi))
        .unwrap_or_else(|| rate_candidates(FALLBACK_RATE_RANGE.0, FALLBACK_RATE_RANGE.1));
    let (freq_lo, freq_hi) = uhd
        .get_rx_freq_range
        .and_then(|f| uhd.read_range(|rg| unsafe { f(dev, 0, rg) }))
        .unwrap_or(FALLBACK_FREQ);
    let (gain_min, gain_max) = uhd
        .read_range(|rg| unsafe { (uhd.get_rx_gain_range)(dev, EMPTY.as_ptr().cast(), 0, rg) })
        .map(|(a, b)| (a as f32, b as f32))
        .unwrap_or(FALLBACK_GAIN);

    let label = if args.is_empty() {
        "USRP".to_string()
    } else {
        format!("USRP ({args})")
    };
    let mut radio = UsrpRadio {
        uhd,
        dev,
        streamer: null_mut(),
        md: null_mut(),
        streaming: false,
        label,
        rates,
        freq_lo: freq_lo.max(0.0) as u64,
        freq_hi: freq_hi.max(0.0) as u64,
        gain_min,
        gain_max,
        max_samps: DEFAULT_BLOCK,
        buf: Vec::new(),
        failures: 0,
        silent: 0,
        dropped: 0,
    };
    // Best effort: a default rate so the first start has one; the caller sets its own.
    let _ = radio.set_sample_rate(DEFAULT_RATE);
    Ok(Box::new(radio))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_address_forms() {
        assert_eq!(device_args(None).unwrap(), "");
        assert_eq!(device_args(Some("  ")).unwrap(), "");
        assert_eq!(device_args(Some("30C6A1")).unwrap(), "serial=30C6A1");
        assert_eq!(device_args(Some("192.168.10.2")).unwrap(), "addr=192.168.10.2");
        assert_eq!(
            device_args(Some("type=b200,serial=30C6A1")).unwrap(),
            "type=b200,serial=30C6A1"
        );
        assert!(device_args(Some("type=")).is_err());
        assert!(device_args(Some("serial=ab cd")).is_err());
        assert!(device_args(Some("bad id!")).is_err());
        assert!(device_args(Some("type=b200,,serial=1")).is_err());
    }

    #[test]
    fn list_entry_is_read() {
        let d = parse_find_entry("type=b200,name=,serial=30C6A1,product=B200");
        assert_eq!(d.driver, "usrp");
        assert_eq!(d.id, "30C6A1");
        assert_eq!(d.label, "USRP B200 serial 30C6A1");
        let bare = parse_find_entry("type=b200,name=,serial=,product=");
        assert_eq!(bare.id, "");
        assert_eq!(bare.label, "USRP b200");
    }

    #[test]
    fn rate_choice_takes_the_closest_offered() {
        assert_eq!(
            pick_rate(&[1_000_000, 2_000_000, 10_000_000], 4_000_000),
            Some(2_000_000)
        );
        assert_eq!(
            pick_rate(&[1_000_000, 10_000_000], 1_000_000_000),
            Some(10_000_000)
        );
        assert_eq!(pick_rate(&[], 1), None);
        let c = rate_candidates(200e3, 56e6);
        assert!(c.contains(&200_000) && c.contains(&56_000_000));
        assert!(c.windows(2).all(|w| w[0] < w[1]));
        let narrow = rate_candidates(3_000_000.0, 6_000_000.0);
        assert!(narrow.iter().all(|&r| (3_000_000..=6_000_000).contains(&r)));
    }

    #[test]
    fn gain_is_held_inside_the_range() {
        assert_eq!(clamp_gain(-5.0, 0.0, 76.0), 0.0);
        assert_eq!(clamp_gain(90.0, 0.0, 76.0), 76.0);
        assert_eq!(clamp_gain(30.5, 0.0, 76.0), 30.5);
    }

    #[test]
    fn samples_convert_to_pairs() {
        let src = [0.5f32, -0.25, 2.0, f32::NAN, -3.0, 0.1, 0.9];
        let mut dst = [(0.0f32, 0.0f32); 8];
        let n = fc32_to_pairs(&src, &mut dst);
        assert_eq!(n, 3, "an odd trailing value is not a pair");
        assert_eq!(dst[0], (0.5, -0.25));
        assert_eq!(dst[1], (1.0, 0.0), "over 1 is held to 1, NaN becomes 0");
        assert_eq!(dst[2], (-1.0, 0.1));
        let mut small = [(0.0f32, 0.0f32); 1];
        assert_eq!(fc32_to_pairs(&src, &mut small), 1);
    }

    #[test]
    fn missing_library_says_what_to_install() {
        let err = load_uhd(&["kahf-test-no-such-uhd-library.so"])
            .err()
            .expect("a missing library must fail");
        assert!(err.contains("the UHD library was not found"), "{err}");
        assert!(err.contains("kahf-test-no-such-uhd-library.so"), "{err}");
        assert!(err.contains("Install UHD"), "{err}");
    }

    #[test]
    fn open_errors_are_plain() {
        assert!(open_error_message("could not find path for images").contains("UHD_IMAGES_DIR"));
        assert!(open_error_message("").contains("Check that it is connected"));
        assert_eq!(open_error_message("boom"), "USRP: cannot open the radio: boom");
    }

    #[test]
    fn uhd_text_is_one_short_line() {
        assert_eq!(clean_text(" a\nb\tc\r "), "a b c");
        let long = "x".repeat(500);
        assert_eq!(clean_text(&long).chars().count(), 403);
    }
}
