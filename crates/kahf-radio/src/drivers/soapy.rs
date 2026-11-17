//! Driver for every radio that has a SoapySDR module, and for remote SoapySDR servers.
//!
//! The SoapySDR C library is loaded at run time. Each function is looked up by
//! name; a library that lacks one is reported by that name. Without the library
//! the driver lists nothing and `open` says how to install it.

use crate::{DeviceInfo, GainSetting, GainStage, RxRadio};
use libloading::Library;
use std::collections::BTreeMap;
use std::ffi::{c_char, c_int, c_long, c_void, CStr, CString};

const DIR_RX: c_int = 1;
const FORMAT_CF32: &str = "CF32";
const ERR_TIMEOUT: c_int = -1;
const ERR_OVERFLOW: c_int = -4;
const ERR_UNDERFLOW: c_int = -7;
/// Longest wait of one `read_iq` call, in microseconds.
const READ_TIMEOUT_US: c_long = 150_000;
/// Read failures in a row before the radio counts as lost.
const MAX_STREAM_FAILURES: u32 = 50;
/// Name of the overall gain stage in `gain_stages`.
const OVERALL: &str = "overall";
/// Remote servers that do not answer within this time count as lost (microseconds).
const REMOTE_TIMEOUT_US: &str = "5000000";
/// Rates offered when the radio's ranges allow them.
const COMMON_RATES: &[u32] = &[
    250_000, 1_000_000, 1_024_000, 1_200_000, 1_536_000, 1_800_000, 1_920_000, 2_000_000, 2_048_000,
    2_400_000, 2_500_000, 2_800_000, 3_000_000, 3_200_000, 4_000_000, 5_000_000, 6_000_000, 8_000_000,
    10_000_000, 12_000_000, 16_000_000, 20_000_000, 24_000_000, 25_000_000, 32_000_000, 40_000_000,
    48_000_000, 50_000_000, 56_000_000, 61_440_000,
];

// ---- C library types and functions (SoapySDR C API, version 0.8) ----

/// One range of values: minimum, maximum and step (step 0 = continuous).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct SRange {
    minimum: f64,
    maximum: f64,
    step: f64,
}

/// A list of key/value pairs as the library passes it.
#[repr(C)]
struct Kwargs {
    keys: *mut *mut c_char,
    vals: *mut *mut c_char,
    size: usize,
}

/// Opaque handles.
enum Dev {}
enum Stream {}

type EnumerateFn = unsafe extern "C" fn(*const Kwargs, *mut usize) -> *mut Kwargs;
type KwargsListClearFn = unsafe extern "C" fn(*mut Kwargs, usize);
type MakeFn = unsafe extern "C" fn(*const Kwargs) -> *mut Dev;
type UnmakeFn = unsafe extern "C" fn(*mut Dev) -> c_int;
type SetFreqFn = unsafe extern "C" fn(*mut Dev, c_int, usize, f64, *const Kwargs) -> c_int;
type RangeListFn = unsafe extern "C" fn(*mut Dev, c_int, usize, *mut usize) -> *mut SRange;
type SetValueFn = unsafe extern "C" fn(*mut Dev, c_int, usize, f64) -> c_int;
type GetValueFn = unsafe extern "C" fn(*mut Dev, c_int, usize) -> f64;
type ListGainsFn = unsafe extern "C" fn(*mut Dev, c_int, usize, *mut usize) -> *mut *mut c_char;
type GainRangeFn = unsafe extern "C" fn(*mut Dev, c_int, usize) -> SRange;
type GainElemRangeFn = unsafe extern "C" fn(*mut Dev, c_int, usize, *const c_char) -> SRange;
type SetGainElemFn = unsafe extern "C" fn(*mut Dev, c_int, usize, *const c_char, f64) -> c_int;
type HasGainModeFn = unsafe extern "C" fn(*mut Dev, c_int, usize) -> bool;
type SetGainModeFn = unsafe extern "C" fn(*mut Dev, c_int, usize, bool) -> c_int;
type SetupStreamFn =
    unsafe extern "C" fn(*mut Dev, c_int, *const c_char, *const usize, usize, *const Kwargs) -> *mut Stream;
type ActivateFn = unsafe extern "C" fn(*mut Dev, *mut Stream, c_int, i64, usize) -> c_int;
type DeactivateFn = unsafe extern "C" fn(*mut Dev, *mut Stream, c_int, i64) -> c_int;
type CloseStreamFn = unsafe extern "C" fn(*mut Dev, *mut Stream) -> c_int;
type ReadStreamFn = unsafe extern "C" fn(
    *mut Dev,
    *mut Stream,
    *const *mut c_void,
    usize,
    *mut c_int,
    *mut i64,
    c_long,
) -> c_int;
type ErrToStrFn = unsafe extern "C" fn(c_int) -> *const c_char;
type HardwareKeyFn = unsafe extern "C" fn(*mut Dev) -> *mut c_char;
type LastErrorFn = unsafe extern "C" fn() -> *const c_char;

unsafe extern "C" {
    fn free(p: *mut c_void);
}

/// Looks up one function the driver must have. Returns an error that names it when it is missing.
macro_rules! sym {
    ($lib:expr, $name:literal, $ty:ty) => {{
        // SAFETY: the name and type match the SoapySDR C header this driver was written against.
        match unsafe { $lib.get::<$ty>(concat!($name, "\0").as_bytes()) } {
            Ok(s) => *s,
            Err(_) => {
                return Err(format!(
                    "the SoapySDR library has no function {}; its version is too old or the file is damaged",
                    $name
                ))
            }
        }
    }};
}

/// Looks up a function the driver can do without.
fn optional<T: Copy>(lib: &Library, name: &[u8]) -> Option<T> {
    // SAFETY: callers pass the name of a function with the matching type.
    unsafe { lib.get::<T>(name).ok().map(|s| *s) }
}

/// The loaded library and its functions. The functions stay valid while `_lib` lives.
struct Api {
    enumerate: EnumerateFn,
    list_clear: KwargsListClearFn,
    make: MakeFn,
    unmake: UnmakeFn,
    set_freq: SetFreqFn,
    freq_range: RangeListFn,
    rate_range: RangeListFn,
    set_rate: SetValueFn,
    get_rate: GetValueFn,
    bw_range: RangeListFn,
    set_bw: SetValueFn,
    list_gains: ListGainsFn,
    gain_range: GainRangeFn,
    gain_elem_range: GainElemRangeFn,
    set_gain: SetValueFn,
    set_gain_elem: SetGainElemFn,
    has_gain_mode: HasGainModeFn,
    set_gain_mode: SetGainModeFn,
    setup_stream: SetupStreamFn,
    activate: ActivateFn,
    deactivate: DeactivateFn,
    close_stream: CloseStreamFn,
    read_stream: ReadStreamFn,
    err_to_str: Option<ErrToStrFn>,
    hardware_key: Option<HardwareKeyFn>,
    last_error: Option<LastErrorFn>,
    _lib: Library,
}

#[cfg(target_os = "macos")]
fn library_names() -> &'static [&'static str] {
    &["libSoapySDR.dylib"]
}

#[cfg(all(unix, not(target_os = "macos")))]
fn library_names() -> &'static [&'static str] {
    &["libSoapySDR.so.0.8", "libSoapySDR.so"]
}

#[cfg(windows)]
fn library_names() -> &'static [&'static str] {
    &["SoapySDR.dll"]
}

/// The message when the library cannot be loaded. `tried` is the loader's own list of failures.
fn missing_library_text(tried: &str) -> String {
    format!("SoapySDR is not available ({tried}). Install the SoapySDR library for this system, then start KAHF again. A radio on another computer can be used through a remote SoapySDR server.")
}

impl Api {
    fn load() -> Result<Api, String> {
        Self::load_from(library_names())
    }

    fn load_from(names: &[&str]) -> Result<Api, String> {
        let lib = crate::drivers::common::load_library(names).map_err(|e| missing_library_text(&e))?;
        let api = Api {
            enumerate: sym!(lib, "SoapySDRDevice_enumerate", EnumerateFn),
            list_clear: sym!(lib, "SoapySDRKwargsList_clear", KwargsListClearFn),
            make: sym!(lib, "SoapySDRDevice_make", MakeFn),
            unmake: sym!(lib, "SoapySDRDevice_unmake", UnmakeFn),
            set_freq: sym!(lib, "SoapySDRDevice_setFrequency", SetFreqFn),
            freq_range: sym!(lib, "SoapySDRDevice_getFrequencyRange", RangeListFn),
            rate_range: sym!(lib, "SoapySDRDevice_getSampleRateRange", RangeListFn),
            set_rate: sym!(lib, "SoapySDRDevice_setSampleRate", SetValueFn),
            get_rate: sym!(lib, "SoapySDRDevice_getSampleRate", GetValueFn),
            bw_range: sym!(lib, "SoapySDRDevice_getBandwidthRange", RangeListFn),
            set_bw: sym!(lib, "SoapySDRDevice_setBandwidth", SetValueFn),
            list_gains: sym!(lib, "SoapySDRDevice_listGains", ListGainsFn),
            gain_range: sym!(lib, "SoapySDRDevice_getGainRange", GainRangeFn),
            gain_elem_range: sym!(lib, "SoapySDRDevice_getGainElementRange", GainElemRangeFn),
            set_gain: sym!(lib, "SoapySDRDevice_setGain", SetValueFn),
            set_gain_elem: sym!(lib, "SoapySDRDevice_setGainElement", SetGainElemFn),
            has_gain_mode: sym!(lib, "SoapySDRDevice_hasGainMode", HasGainModeFn),
            set_gain_mode: sym!(lib, "SoapySDRDevice_setGainMode", SetGainModeFn),
            setup_stream: sym!(lib, "SoapySDRDevice_setupStream", SetupStreamFn),
            activate: sym!(lib, "SoapySDRDevice_activateStream", ActivateFn),
            deactivate: sym!(lib, "SoapySDRDevice_deactivateStream", DeactivateFn),
            close_stream: sym!(lib, "SoapySDRDevice_closeStream", CloseStreamFn),
            read_stream: sym!(lib, "SoapySDRDevice_readStream", ReadStreamFn),
            err_to_str: optional(&lib, b"SoapySDR_errToStr\0"),
            hardware_key: optional(&lib, b"SoapySDRDevice_getHardwareKey\0"),
            last_error: optional(&lib, b"SoapySDRDevice_lastError\0"),
            _lib: lib,
        };
        Ok(api)
    }

    /// The library's own words for the last failure, or a plain code when it has none.
    fn detail(&self, rc: c_int) -> String {
        // SAFETY: both functions return static text or null; null is checked.
        unsafe {
            if let Some(f) = self.last_error {
                let p = f();
                if !p.is_null() {
                    let s = CStr::from_ptr(p).to_string_lossy().into_owned();
                    if !s.is_empty() {
                        return s;
                    }
                }
            }
            if let Some(f) = self.err_to_str {
                let p = f(rc);
                if !p.is_null() {
                    return CStr::from_ptr(p).to_string_lossy().into_owned();
                }
            }
        }
        if rc == 0 {
            return "the library gave no reason".to_string();
        }
        format!("error code {rc}")
    }
}

// ---- pure helpers (no library needed) ----

/// Splits "key=value,key=value" into a map. Values may contain '=' after the first one.
fn parse_args(text: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for part in text.split(',') {
        if let Some((k, v)) = part.split_once('=') {
            let k = k.trim();
            if !k.is_empty() {
                m.insert(k.to_string(), v.trim().to_string());
            }
        }
    }
    m
}

/// Turns what the operator typed into library arguments.
/// A bare word is a serial number. A remote address gets the remote driver and a timeout when they are missing.
fn normalise_args(id: &str) -> String {
    let id = id.trim();
    if id.is_empty() {
        return String::new();
    }
    if !id.contains('=') {
        return format!("serial={id}");
    }
    let m = parse_args(id);
    let mut out = id.to_string();
    if m.contains_key("remote") {
        if !m.contains_key("driver") {
            out = format!("driver=remote,{out}");
        }
        if !m.contains_key("remote:timeout") {
            out.push_str(&format!(",remote:timeout={REMOTE_TIMEOUT_US}"));
        }
    }
    out
}

/// Rounds a value onto a range's step grid, never past its ends.
/// `up` rounds up to the next step (used for rates: the slowest rate at or above the wanted one).
fn snap(r: &SRange, v: f64, up: bool) -> f64 {
    let mut c = v.clamp(r.minimum, r.maximum);
    if r.step > 0.0 && c > r.minimum {
        let q = (c - r.minimum) / r.step;
        let k = if up { (q - 1e-9).ceil() } else { q.round() };
        c = r.minimum + k * r.step;
        if c > r.maximum {
            c = r.minimum + ((r.maximum - r.minimum) / r.step + 1e-9).floor() * r.step;
        }
    }
    c
}

/// The sample rate to ask for: the slowest offered rate at or above `want`, else the fastest offered.
fn rate_at_least(ranges: &[SRange], want: f64) -> f64 {
    let mut above = 0.0_f64;
    let mut fastest = 0.0_f64;
    for r in ranges {
        let c = snap(r, want, true);
        fastest = fastest.max(c);
        if c >= want - 1.0 && (above == 0.0 || c < above) {
            above = c;
        }
    }
    if above > 0.0 {
        above
    } else {
        fastest
    }
}

/// The offered value nearest `want`, over all ranges (used for the analogue bandwidth).
fn nearest_in_ranges(ranges: &[SRange], want: f64) -> f64 {
    let mut best = 0.0;
    let mut best_d = f64::MAX;
    for r in ranges {
        let c = snap(r, want, false);
        if (c - want).abs() < best_d {
            best_d = (c - want).abs();
            best = c;
        }
    }
    best
}

/// The rates to list: the common rates that fall on a range's grid, plus each range's ends.
fn offered_rates(ranges: &[SRange]) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    for r in ranges {
        for &hz in COMMON_RATES {
            let v = hz as f64;
            if v >= r.minimum - 1.0
                && v <= r.maximum + 1.0
                && (r.step <= 0.0 || (snap(r, v, false) - v).abs() <= 1.0)
            {
                out.push(hz);
            }
        }
        for end in [r.minimum, r.maximum] {
            let c = snap(r, end, false);
            if c >= 1.0 {
                out.push(c.round() as u32);
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// The span of frequencies the ranges cover together. No ranges gives the whole u64 span.
fn union_range(ranges: &[SRange]) -> (u64, u64) {
    if ranges.is_empty() {
        return (0, u64::MAX);
    }
    let lo = ranges.iter().map(|r| r.minimum.max(0.0)).fold(f64::MAX, f64::min);
    let hi = ranges.iter().map(|r| r.maximum.max(0.0)).fold(0.0, f64::max);
    (lo.round() as u64, hi.round() as u64)
}

/// Maps a gain setting onto a stage. `Ok(None)` means "the radio's automatic control".
fn gain_value(setting: GainSetting, r: &SRange, auto_ok: bool) -> Result<Option<f64>, String> {
    match setting {
        GainSetting::Auto if auto_ok => Ok(None),
        GainSetting::Auto => Err("this gain stage has no automatic setting".into()),
        GainSetting::Manual(v) if v.is_finite() => Ok(Some(snap(r, v as f64, false))),
        GainSetting::Manual(_) => Err("the gain value is not a number".into()),
    }
}

/// Some drivers list only gain reductions (names ending in "GR"). Their overall value runs the other way.
fn is_gain_reduction(names: &[String]) -> bool {
    !names.is_empty()
        && names.iter().all(|n| {
            let u = n.to_ascii_uppercase();
            u.len() >= 3 && u.ends_with("GR")
        })
}

/// The value to hand the driver for a reduction-type overall gain.
fn turned_around(g: f64, r: &SRange) -> f64 {
    (r.maximum - (g - r.minimum)).clamp(r.minimum, r.maximum)
}

#[derive(Debug, PartialEq)]
enum ReadOutcome {
    Samples(usize),
    Idle,
    Overflow,
    Failed(c_int),
}

fn classify_read(rc: c_int) -> ReadOutcome {
    if rc > 0 {
        ReadOutcome::Samples(rc as usize)
    } else if rc == 0 || rc == ERR_TIMEOUT || rc == ERR_UNDERFLOW {
        ReadOutcome::Idle
    } else if rc == ERR_OVERFLOW {
        ReadOutcome::Overflow
    } else {
        ReadOutcome::Failed(rc)
    }
}

/// A plain sentence for errors that mean another program holds the radio. None otherwise.
fn busy_hint(text: &str) -> Option<&'static str> {
    let l = text.to_ascii_lowercase();
    const KEYS: &[&str] = &[
        "busy",
        "in use",
        "claim",
        "resource temporarily",
        "already open",
        "access denied",
    ];
    if KEYS.iter().any(|k| l.contains(k)) {
        Some("the radio is in use by another program: close it and try again")
    } else {
        None
    }
}

/// The text for a failed open. Remote addresses get their own hint.
fn open_failure_text(detail: &str, args: &str) -> String {
    let mut msg = format!("could not open the radio ({args}): {detail}");
    if let Some(h) = busy_hint(detail) {
        msg.push_str(". ");
        msg.push_str(h);
    } else if args.contains("remote=") {
        msg.push_str(". The remote server did not answer: check the address and port, that the server is running on that computer, and that its radio is connected");
    } else if args.is_empty() {
        msg.push_str(". No SoapySDR radio is connected to this computer");
    }
    msg
}

/// The device list for the operator: the driver's own label, and the arguments that open that radio.
fn device_info(m: &BTreeMap<String, String>) -> DeviceInfo {
    let driver = m.get("driver").cloned().unwrap_or_default();
    let label = m
        .get("label")
        .cloned()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if driver.is_empty() {
                "SoapySDR radio".to_string()
            } else {
                format!("SoapySDR {driver}")
            }
        });
    let id = m
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",");
    DeviceInfo {
        driver: "soapy",
        label,
        id,
    }
}

// ---- C memory handling ----

/// Copies a list the library allocated, then frees it.
///
/// # Safety
/// `p` must be null or a list of `len` items allocated by the library with malloc.
unsafe fn take_array<T: Copy>(p: *mut T, len: usize) -> Vec<T> {
    if p.is_null() {
        return Vec::new();
    }
    let v = std::slice::from_raw_parts(p, len).to_vec();
    free(p as *mut c_void);
    v
}

/// Copies a list of strings the library allocated, then frees each one and the list.
///
/// # Safety
/// `p` must be null or a list of `len` malloc'd C strings.
unsafe fn take_names(p: *mut *mut c_char, len: usize) -> Vec<String> {
    if p.is_null() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        let s = *p.add(i);
        if !s.is_null() {
            out.push(CStr::from_ptr(s).to_string_lossy().into_owned());
            free(s as *mut c_void);
        }
    }
    free(p as *mut c_void);
    out
}

/// Builds a key/value list for the library, keeps it alive while `f` runs.
fn with_kwargs<R>(pairs: &BTreeMap<String, String>, f: impl FnOnce(*const Kwargs) -> R) -> Result<R, String> {
    let mut keys = Vec::with_capacity(pairs.len());
    let mut vals = Vec::with_capacity(pairs.len());
    for (k, v) in pairs {
        keys.push(
            CString::new(k.as_str()).map_err(|_| format!("the argument name {k:?} holds a zero byte"))?,
        );
        vals.push(
            CString::new(v.as_str()).map_err(|_| format!("the argument value for {k} holds a zero byte"))?,
        );
    }
    let mut kp: Vec<*mut c_char> = keys.iter().map(|k| k.as_ptr() as *mut c_char).collect();
    let mut vp: Vec<*mut c_char> = vals.iter().map(|v| v.as_ptr() as *mut c_char).collect();
    let kw = Kwargs {
        keys: kp.as_mut_ptr(),
        vals: vp.as_mut_ptr(),
        size: kp.len(),
    };
    Ok(f(&kw))
}

/// Reads a key/value list the library returned.
///
/// # Safety
/// `kw` must describe valid C strings.
unsafe fn read_kwargs(kw: &Kwargs) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for i in 0..kw.size {
        let k = *kw.keys.add(i);
        let v = *kw.vals.add(i);
        if !k.is_null() && !v.is_null() {
            m.insert(
                CStr::from_ptr(k).to_string_lossy().into_owned(),
                CStr::from_ptr(v).to_string_lossy().into_owned(),
            );
        }
    }
    m
}

// ---- driver entry points ----

/// Lists the radios SoapySDR can see on this computer. Quick: no radio is opened.
pub fn probe() -> Vec<DeviceInfo> {
    let Ok(api) = Api::load() else { return Vec::new() };
    let mut out = Vec::new();
    let empty = BTreeMap::new();
    // SAFETY: the list comes from the library and is freed with its own clear function.
    let _ = with_kwargs(&empty, |kw| unsafe {
        let mut len = 0usize;
        let list = (api.enumerate)(kw, &mut len);
        if !list.is_null() {
            for i in 0..len {
                out.push(device_info(&read_kwargs(&*list.add(i))));
            }
            (api.list_clear)(list, len);
        }
    });
    out
}

/// Opens a radio. `id` is a list such as "driver=rtlsdr,serial=..." or "remote=tcp://host:port".
/// A bare word is a serial number. None opens the first radio found.
pub fn open(id: Option<&str>) -> Result<Box<dyn RxRadio>, String> {
    let api = Api::load()?;
    let args = normalise_args(id.unwrap_or(""));
    let pairs = parse_args(&args);
    let dev = with_kwargs(&pairs, |kw| unsafe { (api.make)(kw) })?;
    if dev.is_null() {
        let detail = api.detail(0);
        return Err(open_failure_text(&detail, &args));
    }
    let mut radio = SoapyRadio {
        api,
        dev,
        stream: std::ptr::null_mut(),
        streaming: false,
        label: String::new(),
        freq: (0, u64::MAX),
        buf: Vec::new(),
        failures: 0,
        dropped: 0,
    };
    radio.label = radio.read_label(&args);
    radio.freq = union_range(&radio.ranges(|api, dev| unsafe { take_ranges(api.freq_range, dev) }));
    Ok(Box::new(radio))
}

/// Calls a range-list function and copies the result.
///
/// # Safety
/// `f` must be one of the range-list functions of the library, `dev` a live device.
unsafe fn take_ranges(f: RangeListFn, dev: *mut Dev) -> Vec<SRange> {
    let mut len = 0usize;
    let p = f(dev, DIR_RX, 0, &mut len);
    take_array(p, len)
}

struct SoapyRadio {
    api: Api,
    dev: *mut Dev,
    stream: *mut Stream,
    streaming: bool,
    label: String,
    freq: (u64, u64),
    buf: Vec<f32>,
    failures: u32,
    dropped: u64,
}

// SAFETY: the handles are used from one thread at a time (see the trait), and the library allows a device to move between threads.
unsafe impl Send for SoapyRadio {}

impl SoapyRadio {
    fn ranges(&self, f: impl FnOnce(&Api, *mut Dev) -> Vec<SRange>) -> Vec<SRange> {
        f(&self.api, self.dev)
    }

    fn read_label(&self, args: &str) -> String {
        let mut label = String::new();
        if let Some(f) = self.api.hardware_key {
            // SAFETY: the library returns a malloc'd C string or null.
            let p = unsafe { f(self.dev) };
            if !p.is_null() {
                label = unsafe { CStr::from_ptr(p).to_string_lossy().into_owned() };
                unsafe { free(p as *mut c_void) };
            }
        }
        if label.is_empty() {
            label = parse_args(args)
                .get("driver")
                .map(|d| format!("SoapySDR {d}"))
                .unwrap_or_else(|| "SoapySDR radio".into());
        }
        label
    }

    fn rates(&self) -> Vec<SRange> {
        // SAFETY: the device is live for the life of the radio.
        self.ranges(|api, dev| unsafe { take_ranges(api.rate_range, dev) })
    }

    fn check(&self, rc: c_int, what: &str) -> Result<(), String> {
        if rc == 0 {
            Ok(())
        } else {
            Err(format!("{what}: {}", self.api.detail(rc)))
        }
    }

    fn gain_names(&self) -> Vec<String> {
        let mut len = 0usize;
        // SAFETY: the list is freed by take_names.
        unsafe {
            let p = (self.api.list_gains)(self.dev, DIR_RX, 0, &mut len);
            take_names(p, len)
        }
    }

    fn overall_range(&self) -> SRange {
        // SAFETY: live device, valid direction and channel.
        unsafe { (self.api.gain_range)(self.dev, DIR_RX, 0) }
    }

    fn has_gain_mode(&self) -> bool {
        // SAFETY: live device, valid direction and channel.
        unsafe { (self.api.has_gain_mode)(self.dev, DIR_RX, 0) }
    }

    fn element_range(&self, name: &str) -> Result<SRange, String> {
        let c = CString::new(name).map_err(|_| "the gain stage name holds a zero byte".to_string())?;
        // SAFETY: the name is a valid C string for the call.
        Ok(unsafe { (self.api.gain_elem_range)(self.dev, DIR_RX, 0, c.as_ptr()) })
    }
}

impl RxRadio for SoapyRadio {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn freq_range_hz(&self) -> (u64, u64) {
        self.freq
    }

    fn set_frequency_hz(&mut self, hz: u64) -> Result<(), String> {
        let api = &self.api;
        let dev = self.dev;
        let rc = with_kwargs(&BTreeMap::new(), |kw| unsafe {
            (api.set_freq)(dev, DIR_RX, 0, hz as f64, kw)
        })?;
        self.check(rc, "could not tune the radio")
    }

    fn sample_rates(&self) -> Vec<u32> {
        offered_rates(&self.rates())
    }

    fn set_sample_rate(&mut self, hz: u32) -> Result<u32, String> {
        let ranges = self.rates();
        let want = hz as f64;
        let target = if ranges.is_empty() {
            want
        } else {
            rate_at_least(&ranges, want)
        };
        // SAFETY: live device.
        let rc = unsafe { (self.api.set_rate)(self.dev, DIR_RX, 0, target) };
        self.check(rc, "could not set the sample rate")?;
        // SAFETY: live device.
        let got = unsafe { (self.api.get_rate)(self.dev, DIR_RX, 0) };
        // The analogue filter is as wide as the signal, within what the radio offers. Failure here is not fatal.
        // SAFETY: live device.
        let bws = unsafe { take_ranges(self.api.bw_range, self.dev) };
        if !bws.is_empty() {
            let b = nearest_in_ranges(&bws, got * 0.95);
            // SAFETY: live device.
            let _ = unsafe { (self.api.set_bw)(self.dev, DIR_RX, 0, b) };
        }
        Ok(got.round() as u32)
    }

    fn gain_stages(&self) -> Vec<GainStage> {
        let mut out = Vec::new();
        let r = self.overall_range();
        out.push(GainStage {
            name: OVERALL.to_string(),
            min_db: r.minimum as f32,
            max_db: r.maximum as f32,
            step_db: r.step as f32,
            auto: self.has_gain_mode(),
        });
        for name in self.gain_names() {
            if let Ok(er) = self.element_range(&name) {
                out.push(GainStage {
                    name,
                    min_db: er.minimum as f32,
                    max_db: er.maximum as f32,
                    step_db: er.step as f32,
                    auto: false,
                });
            }
        }
        out
    }

    fn set_gain(&mut self, stage: &str, setting: GainSetting) -> Result<(), String> {
        if stage == OVERALL {
            let r = self.overall_range();
            let auto_ok = self.has_gain_mode();
            match gain_value(setting, &r, auto_ok)? {
                None => {
                    // SAFETY: live device.
                    let rc = unsafe { (self.api.set_gain_mode)(self.dev, DIR_RX, 0, true) };
                    self.check(rc, "could not turn on automatic gain")
                }
                Some(g) => {
                    if auto_ok {
                        // SAFETY: live device.
                        let rc = unsafe { (self.api.set_gain_mode)(self.dev, DIR_RX, 0, false) };
                        self.check(rc, "could not turn off automatic gain")?;
                    }
                    let v = if is_gain_reduction(&self.gain_names()) {
                        turned_around(g, &r)
                    } else {
                        g
                    };
                    // SAFETY: live device.
                    let rc = unsafe { (self.api.set_gain)(self.dev, DIR_RX, 0, v) };
                    self.check(rc, "could not set the gain")
                }
            }
        } else {
            if !self.gain_names().iter().any(|n| n == stage) {
                return Err(format!("the radio has no gain stage named {stage:?}"));
            }
            let r = self.element_range(stage)?;
            match gain_value(setting, &r, false)? {
                None => Err("this gain stage has no automatic setting".into()),
                Some(v) => {
                    let c = CString::new(stage)
                        .map_err(|_| "the gain stage name holds a zero byte".to_string())?;
                    // SAFETY: the name is a valid C string for the call; live device.
                    let rc = unsafe { (self.api.set_gain_elem)(self.dev, DIR_RX, 0, c.as_ptr(), v) };
                    self.check(rc, &format!("could not set the {stage} gain"))
                }
            }
        }
    }

    fn start(&mut self) -> Result<(), String> {
        if self.streaming {
            return Ok(());
        }
        let chan = [0usize];
        let fmt = CString::new(FORMAT_CF32).map_err(|_| "bad format name".to_string())?;
        let api = &self.api;
        let dev = self.dev;
        let stream = with_kwargs(&BTreeMap::new(), |kw| unsafe {
            (api.setup_stream)(dev, DIR_RX, fmt.as_ptr(), chan.as_ptr(), 1, kw)
        })?;
        if stream.is_null() {
            return Err(format!(
                "could not start the sample stream: {}",
                self.api.detail(0)
            ));
        }
        // SAFETY: live device and stream.
        let rc = unsafe { (self.api.activate)(self.dev, stream, 0, 0, 0) };
        if rc != 0 {
            let msg = self.check(rc, "could not start the sample stream").unwrap_err();
            // SAFETY: the stream was just made and is not used again.
            unsafe { (self.api.close_stream)(self.dev, stream) };
            return Err(msg);
        }
        self.stream = stream;
        self.streaming = true;
        self.failures = 0;
        Ok(())
    }

    fn read_iq(&mut self, out: &mut [(f32, f32)]) -> Result<usize, String> {
        if !self.streaming {
            return Err("the radio is not streaming".into());
        }
        if out.is_empty() {
            return Ok(0);
        }
        self.buf.resize(out.len() * 2, 0.0);
        let bufs = [self.buf.as_mut_ptr() as *mut c_void];
        let mut flags: c_int = 0;
        let mut ts: i64 = 0;
        // SAFETY: the buffer holds out.len() complex CF32 samples, and the stream is live.
        let rc = unsafe {
            (self.api.read_stream)(
                self.dev,
                self.stream,
                bufs.as_ptr(),
                out.len(),
                &mut flags,
                &mut ts,
                READ_TIMEOUT_US,
            )
        };
        match classify_read(rc) {
            ReadOutcome::Samples(n) => {
                for (i, s) in out.iter_mut().enumerate().take(n) {
                    *s = (self.buf[2 * i], self.buf[2 * i + 1]);
                }
                self.failures = 0;
                Ok(n)
            }
            ReadOutcome::Idle => {
                self.failures = 0;
                Ok(0)
            }
            ReadOutcome::Overflow => {
                self.dropped += 1;
                Ok(0)
            }
            ReadOutcome::Failed(code) => {
                self.failures += 1;
                if self.failures > MAX_STREAM_FAILURES {
                    Err(format!(
                        "the radio stopped delivering samples: {}",
                        self.api.detail(code)
                    ))
                } else {
                    Ok(0)
                }
            }
        }
    }

    fn stop(&mut self) {
        if !self.streaming {
            return;
        }
        // SAFETY: the stream was made by start and is closed once here.
        unsafe {
            (self.api.deactivate)(self.dev, self.stream, 0, 0);
            (self.api.close_stream)(self.dev, self.stream);
        }
        self.stream = std::ptr::null_mut();
        self.streaming = false;
    }

    fn dropped_samples(&self) -> u64 {
        self.dropped
    }
}

impl Drop for SoapyRadio {
    fn drop(&mut self) {
        self.stop();
        if !self.dev.is_null() {
            // SAFETY: the device was made by open and is released once here.
            unsafe { (self.api.unmake)(self.dev) };
            self.dev = std::ptr::null_mut();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(lo: f64, hi: f64, step: f64) -> SRange {
        SRange {
            minimum: lo,
            maximum: hi,
            step,
        }
    }

    #[test]
    fn args_split_on_first_equals() {
        let m = parse_args("driver=rtlsdr,serial=AB=CD");
        assert_eq!(m.get("driver").map(String::as_str), Some("rtlsdr"));
        assert_eq!(m.get("serial").map(String::as_str), Some("AB=CD"));
        assert!(parse_args("").is_empty());
    }

    #[test]
    fn typed_ids_become_arguments() {
        assert_eq!(normalise_args(""), "");
        assert_eq!(normalise_args("ABC123"), "serial=ABC123");
        assert_eq!(normalise_args("driver=rtlsdr,serial=1"), "driver=rtlsdr,serial=1");
        assert_eq!(
            normalise_args("remote=tcp://10.0.0.5:55132"),
            "driver=remote,remote=tcp://10.0.0.5:55132,remote:timeout=5000000"
        );
        assert_eq!(
            normalise_args("remote=tcp://h:1,remote:driver=rtlsdr,remote:timeout=7"),
            "driver=remote,remote=tcp://h:1,remote:driver=rtlsdr,remote:timeout=7"
        );
    }

    #[test]
    fn rate_is_slowest_offered_at_or_above() {
        let ranges = [
            r(2_500_000.0, 2_500_000.0, 0.0),
            r(10_000_000.0, 10_000_000.0, 0.0),
        ];
        assert_eq!(rate_at_least(&ranges, 4_000_000.0), 10_000_000.0);
        assert_eq!(rate_at_least(&ranges, 2_000_000.0), 2_500_000.0);
        let cont = [r(250_000.0, 61_440_000.0, 0.0)];
        assert_eq!(rate_at_least(&cont, 2_048_000.0), 2_048_000.0);
        // above every offered rate: the fastest is used
        assert_eq!(rate_at_least(&ranges, 50_000_000.0), 10_000_000.0);
    }

    #[test]
    fn rates_snap_to_step_grid() {
        let stepped = [r(1_000_000.0, 3_200_000.0, 100_000.0)];
        assert_eq!(rate_at_least(&stepped, 2_050_000.0), 2_100_000.0);
        let offered = offered_rates(&stepped);
        assert!(offered.contains(&2_000_000) && offered.contains(&3_200_000));
        assert!(!offered.contains(&2_048_000));
        let fixed = offered_rates(&[r(2_500_000.0, 2_500_000.0, 0.0)]);
        assert_eq!(fixed, vec![2_500_000]);
    }

    #[test]
    fn bandwidth_is_nearest_offered() {
        let bws = [r(200_000.0, 200_000.0, 0.0), r(1_000_000.0, 8_000_000.0, 0.0)];
        assert_eq!(nearest_in_ranges(&bws, 1_900_000.0), 1_900_000.0);
        assert_eq!(nearest_in_ranges(&bws, 100_000.0), 200_000.0);
    }

    #[test]
    fn gain_is_clamped_and_snapped() {
        let stage = r(0.0, 49.6, 1.0);
        assert_eq!(
            gain_value(GainSetting::Manual(60.0), &stage, false),
            Ok(Some(49.0))
        );
        assert_eq!(
            gain_value(GainSetting::Manual(-3.0), &stage, false),
            Ok(Some(0.0))
        );
        assert_eq!(
            gain_value(GainSetting::Manual(12.4), &stage, false),
            Ok(Some(12.0))
        );
        assert_eq!(gain_value(GainSetting::Auto, &stage, true), Ok(None));
        assert!(gain_value(GainSetting::Auto, &stage, false).is_err());
        assert!(gain_value(GainSetting::Manual(f32::NAN), &stage, false).is_err());
    }

    #[test]
    fn reduction_gains_run_the_other_way() {
        let names = vec!["IFGR".to_string(), "RFGR".to_string()];
        assert!(is_gain_reduction(&names));
        assert!(!is_gain_reduction(&["LNA".to_string()]));
        assert!(!is_gain_reduction(&[]));
        let overall = r(0.0, 59.0, 0.0);
        assert_eq!(turned_around(0.0, &overall), 59.0);
        assert_eq!(turned_around(59.0, &overall), 0.0);
    }

    #[test]
    fn read_codes_are_sorted() {
        assert_eq!(classify_read(4096), ReadOutcome::Samples(4096));
        assert_eq!(classify_read(0), ReadOutcome::Idle);
        assert_eq!(classify_read(ERR_TIMEOUT), ReadOutcome::Idle);
        assert_eq!(classify_read(ERR_OVERFLOW), ReadOutcome::Overflow);
        assert_eq!(classify_read(-2), ReadOutcome::Failed(-2));
    }

    #[test]
    fn missing_library_says_what_to_do() {
        let err = Api::load_from(&["kahf-no-such-soapysdr-library.so"])
            .err()
            .expect("must fail");
        assert!(err.contains("SoapySDR is not available"), "{err}");
        assert!(err.contains("kahf-no-such-soapysdr-library.so"), "{err}");
        assert!(err.contains("Install the SoapySDR library"), "{err}");
    }

    #[test]
    fn open_errors_give_hints() {
        assert!(open_failure_text("device busy", "driver=rtlsdr").contains("in use by another program"));
        assert!(open_failure_text("no route", "remote=tcp://h:1").contains("remote server did not answer"));
        assert!(open_failure_text("x", "").contains("No SoapySDR radio"));
    }

    #[test]
    fn union_and_labels() {
        assert_eq!(
            union_range(&[r(24e6, 1.7e9, 0.0), r(2e9, 3e9, 0.0)]),
            (24_000_000, 3_000_000_000)
        );
        assert_eq!(union_range(&[]), (0, u64::MAX));
        let mut m = BTreeMap::new();
        m.insert("driver".to_string(), "rtlsdr".to_string());
        let d = device_info(&m);
        assert_eq!(d.label, "SoapySDR rtlsdr");
        assert_eq!(d.id, "driver=rtlsdr");
    }
}
