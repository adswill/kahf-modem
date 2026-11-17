//! Driver for SDRplay radios (RSP1, RSP1A, RSP1B, RSP2, RSPduo, RSPdx, RSPdx-R2).
//!
//! The radios work through the SDRplay API 3.x library, which the user installs
//! from the SDRplay site. The library is loaded at run time. The layouts below
//! follow the vendor's C interface for API 3.07 to 3.x. This code has not been
//! tested with hardware.

use std::collections::VecDeque;
use std::ffi::{c_char, c_int, c_uint, c_void, CStr};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::Duration;

use libloading::Library;

use crate::drivers::common::load_library;
use crate::{DeviceInfo, GainSetting, GainStage, RxRadio};

const DRIVER: &str = "sdrplay";
/// Longest wait in `read_iq` for samples.
const READ_WAIT: Duration = Duration::from_millis(200);
/// Most samples held between the API thread and the reader (about 8 MB).
const SAMPLE_CAP: usize = 1 << 20;
const TUNE_MIN_HZ: u64 = 1_000;
const TUNE_MIN_RSP1_HZ: u64 = 10_000;
const TUNE_MAX_HZ: u64 = 2_000_000_000;
const DEFAULT_RATE: u32 = 2_000_000;
const DEFAULT_FREQ_HZ: u64 = 100_000_000;
const DEFAULT_GAIN_DB: f64 = 40.0;
const GAIN_STAGE: &str = "gain";
const MAX_DEVICES: usize = 16;
const SERIAL_LEN: usize = 64;
const DEVICE_STRUCT_SIZE: usize = 96;

/// Sample rates the driver offers, in hertz, ascending.
const RATES: &[u32] = &[
    62_500, 125_000, 250_000, 500_000, 1_000_000, 2_000_000, 2_048_000, 2_500_000, 3_000_000, 4_000_000, 5_000_000,
    6_000_000, 8_000_000, 10_000_000,
];

// Return codes and values from the vendor interface.
const SUCCESS: c_int = 0;
const ERR_GAIN_UPDATE: c_int = 4;
const ERR_RF_UPDATE: c_int = 5;
const ERR_FS_UPDATE: c_int = 6;
const HW_RSP1: u8 = 1;
const HW_RSP2: u8 = 2;
const HW_RSPDUO: u8 = 3;
const HW_RSPDX: u8 = 4;
const HW_RSP1B: u8 = 6;
const HW_RSPDX_R2: u8 = 7;
const HW_RSP1A: u8 = 255;
const TUNER_A: c_int = 1;
const TUNER_B: c_int = 2;
const DUO_SINGLE_TUNER: c_uint = 1;
const IF_ZERO: c_int = 0;
const LO_AUTO: c_int = 1;
const AGC_DISABLE: c_int = 0;
const NORMAL_MIN_GR: c_int = 20;
const UPDATE_TUNER_GR: c_uint = 0x0000_8000;
const UPDATE_TUNER_FRF: c_uint = 0x0002_0000;
const UPDATE_CTRL_OVERLOAD_ACK: c_uint = 0x0400_0000;
const EVENT_POWER_OVERLOAD: c_int = 1;
const EVENT_DEVICE_REMOVED: c_int = 2;
const EVENT_DEVICE_FAILURE: c_int = 4;

// ---------------------------------------------------------------- gain tables

/// Gain reduction in dB of each LNA state, for frequencies below `up_to_hz`.
struct LnaBand {
    up_to_hz: f64,
    gr: &'static [i32],
}

const LNA_RSP1: &[LnaBand] = &[
    LnaBand { up_to_hz: 420e6, gr: &[0, 24, 19, 43] },
    LnaBand { up_to_hz: 1000e6, gr: &[0, 7, 19, 26] },
    LnaBand { up_to_hz: 1e12, gr: &[0, 5, 19, 24] },
];
const LNA_RSP1A: &[LnaBand] = &[
    LnaBand { up_to_hz: 60e6, gr: &[0, 6, 12, 18, 37, 42, 61] },
    LnaBand { up_to_hz: 420e6, gr: &[0, 6, 12, 18, 20, 26, 32, 38, 57, 62] },
    LnaBand { up_to_hz: 1000e6, gr: &[0, 7, 13, 19, 20, 27, 33, 39, 45, 64] },
    LnaBand { up_to_hz: 1e12, gr: &[0, 6, 12, 20, 26, 32, 38, 43, 62] },
];
const LNA_RSP1B: &[LnaBand] = &[
    LnaBand { up_to_hz: 50e6, gr: &[0, 6, 12, 18, 37, 42, 61] },
    LnaBand { up_to_hz: 420e6, gr: &[0, 6, 12, 18, 20, 26, 32, 38, 57, 62] },
    LnaBand { up_to_hz: 1000e6, gr: &[0, 7, 13, 19, 20, 27, 33, 39, 45, 64] },
    LnaBand { up_to_hz: 1e12, gr: &[0, 6, 12, 20, 26, 32, 38, 43, 62] },
];
const LNA_RSP2: &[LnaBand] = &[
    LnaBand { up_to_hz: 420e6, gr: &[0, 10, 15, 21, 24, 34, 39, 45, 64] },
    LnaBand { up_to_hz: 1000e6, gr: &[0, 7, 10, 17, 22, 41] },
    LnaBand { up_to_hz: 1e12, gr: &[0, 5, 21, 15, 15, 34] },
];
const LNA_RSPDX: &[LnaBand] = &[
    LnaBand { up_to_hz: 12e6, gr: &[0, 3, 6, 9, 12, 15, 24, 27, 30, 33, 36, 39, 42, 45, 48, 51, 54, 57, 60] },
    LnaBand { up_to_hz: 50e6, gr: &[0, 3, 6, 9, 12, 15, 18, 24, 27, 30, 33, 36, 39, 42, 45, 48, 51, 54, 57, 60] },
    LnaBand {
        up_to_hz: 60e6,
        gr: &[0, 3, 6, 9, 12, 20, 23, 26, 29, 32, 35, 38, 44, 47, 50, 53, 56, 59, 62, 65, 68, 71, 74, 77, 80],
    },
    LnaBand {
        up_to_hz: 250e6,
        gr: &[0, 3, 6, 9, 12, 15, 24, 27, 30, 33, 36, 39, 42, 45, 48, 51, 54, 57, 60, 63, 66, 69, 72, 75, 78, 81, 84],
    },
    LnaBand {
        up_to_hz: 420e6,
        gr: &[0, 3, 6, 9, 12, 15, 18, 24, 27, 30, 33, 36, 39, 42, 45, 48, 51, 54, 57, 60, 63, 66, 69, 72, 75, 78, 81, 84],
    },
    LnaBand { up_to_hz: 1000e6, gr: &[0, 7, 10, 13, 16, 19, 22, 25, 31, 34, 37, 40, 43, 46, 49, 52, 55, 58, 61, 64, 67] },
    LnaBand { up_to_hz: 1e12, gr: &[0, 5, 8, 11, 14, 17, 20, 32, 35, 38, 41, 44, 47, 50, 53, 56, 59, 62, 65] },
];

/// The IF gain reduction runs from 20 to 59 dB.
const GR_IF_MIN: i32 = 20;
const GR_IF_MAX: i32 = 59;
/// Above this total reduction the LNA steps in, so the IF stage stays at 50 dB or less.
const GR_IF_SOFT: i32 = 50;

fn lna_bands(hw: u8) -> &'static [LnaBand] {
    match hw {
        HW_RSP1 => LNA_RSP1,
        HW_RSP1B => LNA_RSP1B,
        HW_RSP2 => LNA_RSP2,
        HW_RSPDX | HW_RSPDX_R2 => LNA_RSPDX,
        // RSP1A, RSPduo (same table) and newer models
        _ => LNA_RSP1A,
    }
}

fn lna_band(hw: u8, hz: f64) -> &'static LnaBand {
    let bands = lna_bands(hw);
    bands.iter().find(|b| hz < b.up_to_hz).unwrap_or(&bands[bands.len() - 1])
}

/// Most gain in dB the single gain stage offers: no reduction at all.
fn max_gain(hw: u8) -> i32 {
    let top = lna_bands(hw).iter().flat_map(|b| b.gr.iter().copied()).max().unwrap_or(0);
    top + (GR_IF_MAX - GR_IF_MIN)
}

/// Maps one gain value in dB (0 to `max_gain`, more is more gain) to the LNA state
/// and the IF gain reduction for frequency `hz`.
fn gain_split(hw: u8, hz: f64, gain_db: f64) -> (u8, i32) {
    let gmax = max_gain(hw);
    let g = gain_db.clamp(0.0, gmax as f64).round() as i32;
    let reduce = gmax - g + GR_IF_MIN;
    let band = lna_band(hw, hz);
    let mut pick: Option<usize> = None;
    for (k, &gr) in band.gr.iter().enumerate() {
        if gr >= reduce - GR_IF_SOFT && pick.map_or(true, |p| gr < band.gr[p]) {
            pick = Some(k);
        }
    }
    let pick = pick.unwrap_or_else(|| {
        // no LNA state is low enough: use the one with the most reduction
        let mut best = 0;
        for (k, &gr) in band.gr.iter().enumerate() {
            if gr > band.gr[best] {
                best = k;
            }
        }
        best
    });
    let gr_db = (reduce - band.gr[pick]).clamp(GR_IF_MIN, GR_IF_MAX);
    (pick as u8, gr_db)
}

// ---------------------------------------------------------------- rates

/// Nearest rate in the table to `hz`.
fn closest_rate(hz: u32) -> u32 {
    RATES.iter().copied().min_by_key(|r| r.abs_diff(hz)).unwrap_or(DEFAULT_RATE)
}

/// The analogue ADC rate and decimation factor (1, 2, ... 32) that give `rate`.
/// The ADC runs at 2 MHz or more.
fn rate_plan(rate: u32) -> (f64, u8) {
    let mut dec: u32 = 1;
    while dec < 32 && (rate as f64) * (dec as f64) < 2_000_000.0 - 1.0 {
        dec *= 2;
    }
    let fs = (rate as f64 * dec as f64).max(2_000_000.0);
    (fs, dec as u8)
}

/// The widest analogue IF filter (kHz) that fits inside the sample rate.
fn if_bandwidth_khz(rate_hz: u32) -> c_int {
    const BWS: [c_int; 8] = [200, 300, 600, 1536, 5000, 6000, 7000, 8000];
    let want = rate_hz as f64;
    let mut i = 0;
    while i + 1 < BWS.len() && (BWS[i] as f64) * 1e3 < want * 0.999 {
        i += 1;
    }
    while i > 0 && (BWS[i] as f64) * 1e3 > want * 1.001 {
        i -= 1;
    }
    BWS[i]
}

// ---------------------------------------------------------------- samples

const SAMPLE_SCALE: f32 = 1.0 / 32768.0;

/// Adds I and Q blocks to `buf` as complex samples in -1..1. Returns how many were
/// dropped because `buf` already held `cap` samples.
fn push_iq(buf: &mut VecDeque<(f32, f32)>, xi: &[i16], xq: &[i16], cap: usize) -> u64 {
    let n = xi.len().min(xq.len());
    let take = n.min(cap.saturating_sub(buf.len()));
    buf.extend(xi.iter().zip(xq.iter()).take(take).map(|(&i, &q)| (i as f32 * SAMPLE_SCALE, q as f32 * SAMPLE_SCALE)));
    (n - take) as u64
}

// ---------------------------------------------------------------- vendor layouts

#[repr(C)]
#[derive(Clone, Copy)]
struct DeviceT {
    ser_no: [c_char; SERIAL_LEN],
    hw_ver: u8,
    tuner: c_int,
    rsp_duo_mode: c_uint,
    valid: u8,
    rsp_duo_sample_freq: f64,
    dev: *mut c_void,
}

#[repr(C)]
struct DeviceParamsT {
    dev_params: *mut DevParamsT,
    rx_a: *mut RxChannelParamsT,
    rx_b: *mut RxChannelParamsT,
}

#[repr(C)]
struct FsFreqT {
    fs_hz: f64,
    sync_update: u8,
    re_cal: u8,
}

#[repr(C)]
struct SyncUpdateT {
    sample_num: u32,
    period: u32,
}

#[repr(C)]
struct ResetFlagsT {
    gain: u8,
    rf: u8,
    fs: u8,
}

#[repr(C)]
struct Rsp1aParamsT {
    rf_notch: u8,
    rf_dab_notch: u8,
}

#[repr(C)]
struct Rsp2ParamsT {
    ext_ref: u8,
}

#[repr(C)]
struct RspDuoParamsT {
    ext_ref: c_int,
}

#[repr(C)]
struct RspDxParamsT {
    hdr_enable: u8,
    bias_t: u8,
    antenna_sel: c_int,
    rf_notch: u8,
    rf_dab_notch: u8,
}

#[repr(C)]
struct DevParamsT {
    ppm: f64,
    fs_freq: FsFreqT,
    sync_update: SyncUpdateT,
    reset_flags: ResetFlagsT,
    mode: c_int,
    samples_per_pkt: u32,
    rsp1a: Rsp1aParamsT,
    rsp2: Rsp2ParamsT,
    rsp_duo: RspDuoParamsT,
    rsp_dx: RspDxParamsT,
}

#[repr(C)]
struct GainParamsT {
    gr_db: c_int,
    lna_state: u8,
    sync_update: u8,
    min_gr: c_int,
    gain_vals: [f32; 3],
}

#[repr(C)]
struct RfFreqT {
    rf_hz: f64,
    sync_update: u8,
}

#[repr(C)]
struct DcOffsetTunerT {
    dc_cal: u8,
    speed_up: u8,
    track_time: c_int,
    refresh_rate_time: c_int,
}

#[repr(C)]
struct TunerParamsT {
    bw_type: c_int,
    if_type: c_int,
    lo_mode: c_int,
    gain: GainParamsT,
    rf_freq: RfFreqT,
    dc_offset_tuner: DcOffsetTunerT,
}

#[repr(C)]
struct DcOffsetT {
    dc_enable: u8,
    iq_enable: u8,
}

#[repr(C)]
struct DecimationT {
    enable: u8,
    factor: u8,
    wide_band: u8,
}

#[repr(C)]
struct AgcT {
    enable: c_int,
    set_point_dbfs: c_int,
    attack_ms: u16,
    decay_ms: u16,
    decay_delay_ms: u16,
    decay_threshold_db: u16,
    sync_update: c_int,
}

#[repr(C)]
struct ControlParamsT {
    dc_offset: DcOffsetT,
    decimation: DecimationT,
    agc: AgcT,
    adsb_mode: c_int,
}

#[repr(C)]
struct Rsp1aTunerParamsT {
    bias_t: u8,
}

#[repr(C)]
struct Rsp2TunerParamsT {
    bias_t: u8,
    am_port_sel: c_int,
    antenna_sel: c_int,
    rf_notch: u8,
}

#[repr(C)]
struct RspDuoTunerParamsT {
    bias_t: u8,
    tuner1_am_port_sel: c_int,
    tuner1_am_notch: u8,
    rf_notch: u8,
    rf_dab_notch: u8,
    reset_slave_flags: [u8; 2],
}

#[repr(C)]
struct RspDxTunerParamsT {
    hdr_bw: c_int,
}

#[repr(C)]
struct RxChannelParamsT {
    tuner: TunerParamsT,
    ctrl: ControlParamsT,
    rsp1a_tuner: Rsp1aTunerParamsT,
    rsp2_tuner: Rsp2TunerParamsT,
    rsp_duo_tuner: RspDuoTunerParamsT,
    rsp_dx_tuner: RspDxTunerParamsT,
}

// The vendor's layouts are fixed by the C interface. These checks catch a bad edit.
const _: () = assert!(std::mem::size_of::<DeviceT>() == DEVICE_STRUCT_SIZE);
const _: () = assert!(std::mem::size_of::<DevParamsT>() == 64);
const _: () = assert!(std::mem::size_of::<TunerParamsT>() == 72);
const _: () = assert!(std::mem::size_of::<RxChannelParamsT>() == 144);

#[repr(C)]
struct CallbackFnsT {
    stream_a: StreamCb,
    stream_b: StreamCb,
    event: EventCb,
}

type StreamCb = unsafe extern "C" fn(*mut i16, *mut i16, *mut c_void, c_uint, c_uint, *mut c_void);
type EventCb = unsafe extern "C" fn(c_int, c_int, *mut c_void, *mut c_void);

type OpenFn = unsafe extern "C" fn() -> c_int;
type VersionFn = unsafe extern "C" fn(*mut f32) -> c_int;
type GetDevicesFn = unsafe extern "C" fn(*mut DeviceT, *mut c_uint, c_uint) -> c_int;
type DeviceFn = unsafe extern "C" fn(*mut DeviceT) -> c_int;
type ErrorStringFn = unsafe extern "C" fn(c_int) -> *const c_char;
type DeviceParamsFn = unsafe extern "C" fn(*mut c_void, *mut *mut DeviceParamsT) -> c_int;
type InitFn = unsafe extern "C" fn(*mut c_void, *mut CallbackFnsT, *mut c_void) -> c_int;
type UninitFn = unsafe extern "C" fn(*mut c_void) -> c_int;
type UpdateFn = unsafe extern "C" fn(*mut c_void, c_int, c_uint, c_uint) -> c_int;

// ---------------------------------------------------------------- library

/// Library names to try, in order. The macOS list also tries the Unix names,
/// because the vendor's Unix installer may place the library under those.
fn library_names() -> &'static [&'static str] {
    if cfg!(target_os = "windows") {
        &[r"C:\Program Files\SDRplay\API\x64\sdrplay_api.dll", "sdrplay_api.dll"]
    } else if cfg!(target_os = "macos") {
        &["libsdrplay_api.dylib", "libsdrplay_api.so.3", "libsdrplay_api.so"]
    } else {
        &["libsdrplay_api.so.3", "libsdrplay_api.so"]
    }
}

fn missing_library_message(detail: &str) -> String {
    format!("SDRplay API is not installed ({detail}). Install the SDRplay API from sdrplay.com, then try again")
}

/// Reads one function from the library.
fn symbol<T: Copy>(lib: &Library, name: &str) -> Result<T, String> {
    let with_nul = format!("{name}\0");
    // SAFETY: the library is the vendor's own, and T is the function type the
    // vendor documents for this name. The pointer is copied out and used only
    // while the library stays loaded (the library lives as long as the process).
    match unsafe { lib.get::<T>(with_nul.as_bytes()) } {
        Ok(sym) => Ok(*sym),
        Err(_) => Err(format!("the SDRplay API library has no function {name}; it may be too old")),
    }
}

struct Api {
    _lib: Library,
    open: OpenFn,
    close: OpenFn,
    api_version: VersionFn,
    lock_devices: OpenFn,
    unlock_devices: OpenFn,
    get_devices: GetDevicesFn,
    select_device: DeviceFn,
    release_device: DeviceFn,
    error_string: ErrorStringFn,
    get_device_params: DeviceParamsFn,
    init: InitFn,
    uninit: UninitFn,
    update: UpdateFn,
    /// Set to the API version once the service answers; None before that.
    service: Mutex<Option<f32>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Api {
    fn load() -> Result<Api, String> {
        let lib = load_library(library_names()).map_err(|d| missing_library_message(&d))?;
        Ok(Api {
            open: symbol(&lib, "sdrplay_api_Open")?,
            close: symbol(&lib, "sdrplay_api_Close")?,
            api_version: symbol(&lib, "sdrplay_api_ApiVersion")?,
            lock_devices: symbol(&lib, "sdrplay_api_LockDeviceApi")?,
            unlock_devices: symbol(&lib, "sdrplay_api_UnlockDeviceApi")?,
            get_devices: symbol(&lib, "sdrplay_api_GetDevices")?,
            select_device: symbol(&lib, "sdrplay_api_SelectDevice")?,
            release_device: symbol(&lib, "sdrplay_api_ReleaseDevice")?,
            error_string: symbol(&lib, "sdrplay_api_GetErrorString")?,
            get_device_params: symbol(&lib, "sdrplay_api_GetDeviceParams")?,
            init: symbol(&lib, "sdrplay_api_Init")?,
            uninit: symbol(&lib, "sdrplay_api_Uninit")?,
            update: symbol(&lib, "sdrplay_api_Update")?,
            _lib: lib,
            service: Mutex::new(None),
        })
    }

    fn err_text(&self, code: c_int) -> String {
        // SAFETY: the vendor returns a static, NUL-terminated string or null.
        let p = unsafe { (self.error_string)(code) };
        if p.is_null() {
            return format!("error code {code}");
        }
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }

    /// Opens the service once and checks the API version. Returns the version.
    fn ensure_service(&self) -> Result<f32, String> {
        let mut slot = lock(&self.service);
        if let Some(v) = *slot {
            return Ok(v);
        }
        // SAFETY: plain vendor call with no arguments.
        let code = unsafe { (self.open)() };
        if code != SUCCESS {
            return Err(format!(
                "SDRplay API service is not running ({}): start the SDRplay API service or reinstall the API from sdrplay.com",
                self.err_text(code)
            ));
        }
        let mut v: f32 = 0.0;
        // SAFETY: the pointer refers to a live f32 on this stack frame.
        let code = unsafe { (self.api_version)(&mut v) };
        if code != SUCCESS {
            // SAFETY: the service was opened above.
            unsafe { (self.close)() };
            return Err(format!(
                "SDRplay API library does not work with the running service ({}): reinstall the API from sdrplay.com",
                self.err_text(code)
            ));
        }
        if !(3.07..4.0).contains(&v) {
            // SAFETY: the service was opened above.
            unsafe { (self.close)() };
            return Err(format!("SDRplay API {v:.2} is not supported: this driver needs API 3.07 or a newer 3.x"));
        }
        *slot = Some(v);
        Ok(v)
    }

    /// All devices the service lists, usable or not.
    fn list_devices(&self) -> Result<Vec<DeviceT>, String> {
        // SAFETY: DeviceT holds only integers, arrays and pointers, so all zeros is valid.
        let mut devs: [DeviceT; MAX_DEVICES] = unsafe { std::mem::zeroed() };
        let mut n: c_uint = 0;
        // SAFETY: the device list is written into `devs`, which holds MAX_DEVICES entries.
        let code = unsafe {
            (self.lock_devices)();
            let code = (self.get_devices)(devs.as_mut_ptr(), &mut n, MAX_DEVICES as c_uint);
            (self.unlock_devices)();
            code
        };
        if code != SUCCESS {
            return Err(format!("SDRplay API: cannot list the radios: {}", self.err_text(code)));
        }
        Ok(devs[..(n as usize).min(MAX_DEVICES)].to_vec())
    }
}

static API: OnceLock<Result<Api, String>> = OnceLock::new();

fn api() -> Result<&'static Api, String> {
    API.get_or_init(Api::load).as_ref().map_err(|e| e.clone())
}

fn serial_of(d: &DeviceT) -> String {
    let bytes: Vec<u8> = d.ser_no.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn model_name(hw: u8) -> String {
    match hw {
        HW_RSP1 => "RSP1".into(),
        HW_RSP1A => "RSP1A".into(),
        HW_RSP1B => "RSP1B".into(),
        HW_RSP2 => "RSP2".into(),
        HW_RSPDUO => "RSPduo".into(),
        HW_RSPDX => "RSPdx".into(),
        HW_RSPDX_R2 => "RSPdx-R2".into(),
        other => format!("RSP (model {other})"),
    }
}

/// True when the device can be used. The valid flag exists from API 3.08 on.
fn is_usable(version: f32, d: &DeviceT) -> bool {
    (version <= 3.075 || d.valid != 0) && !(d.hw_ver == HW_RSPDUO && d.rsp_duo_mode & DUO_SINGLE_TUNER == 0)
}

// ---------------------------------------------------------------- stream state

/// Shared between the reader and the API's callbacks.
struct Shared {
    samples: Mutex<VecDeque<(f32, f32)>>,
    ready: Condvar,
    removed: AtomicBool,
    dropped: AtomicU64,
    device: AtomicPtr<c_void>,
    tuner: AtomicI32,
}

unsafe extern "C" fn on_stream(xi: *mut i16, xq: *mut i16, _params: *mut c_void, count: c_uint, _reset: c_uint, ctx: *mut c_void) {
    if ctx.is_null() || xi.is_null() || xq.is_null() || count == 0 {
        return;
    }
    // SAFETY: ctx is the Shared behind the radio's Arc. It stays alive until Uninit has returned.
    let shared = unsafe { &*(ctx as *const Shared) };
    // SAFETY: the API gives `count` valid samples in each array for this call only.
    let (i, q) = unsafe {
        (std::slice::from_raw_parts(xi, count as usize), std::slice::from_raw_parts(xq, count as usize))
    };
    let mut buf = lock(&shared.samples);
    let dropped = push_iq(&mut buf, i, q, SAMPLE_CAP);
    drop(buf);
    if dropped > 0 {
        shared.dropped.fetch_add(dropped, Ordering::Relaxed);
    }
    shared.ready.notify_all();
}

unsafe extern "C" fn on_event(id: c_int, tuner: c_int, _params: *mut c_void, ctx: *mut c_void) {
    if ctx.is_null() {
        return;
    }
    // SAFETY: as in on_stream.
    let shared = unsafe { &*(ctx as *const Shared) };
    match id {
        EVENT_POWER_OVERLOAD => {
            // The vendor reports the next overload change only after an acknowledgement.
            if let Ok(a) = api() {
                // SAFETY: the device handle was stored before Init and is still selected.
                unsafe { (a.update)(shared.device.load(Ordering::Acquire), tuner, UPDATE_CTRL_OVERLOAD_ACK, 0) };
            }
        }
        EVENT_DEVICE_REMOVED | EVENT_DEVICE_FAILURE => shared.removed.store(true, Ordering::Release),
        _ => {}
    }
}

// ---------------------------------------------------------------- radio

pub struct SdrplayRadio {
    api: &'static Api,
    dev: DeviceT,
    hw: u8,
    label: String,
    dev_params: *mut DevParamsT,
    chan: *mut RxChannelParamsT,
    rate: u32,
    freq_hz: u64,
    gain_db: f64,
    shared: Arc<Shared>,
    running: bool,
}

// SAFETY: the raw pointers point into the vendor's per-device memory. They are
// used only through this struct, which the trait allows on one thread at a time.
unsafe impl Send for SdrplayRadio {}

impl SdrplayRadio {
    fn channel(&mut self) -> &mut RxChannelParamsT {
        // SAFETY: the pointer comes from GetDeviceParams for the device selected by this struct.
        unsafe { &mut *self.chan }
    }

    fn device_params(&mut self) -> &mut DevParamsT {
        // SAFETY: as for channel().
        unsafe { &mut *self.dev_params }
    }

    fn freq_range(&self) -> (u64, u64) {
        let lo = if self.hw == HW_RSP1 { TUNE_MIN_RSP1_HZ } else { TUNE_MIN_HZ };
        (lo, TUNE_MAX_HZ)
    }

    /// Writes the sample rate, the ADC rate, the decimation and the IF filter.
    fn write_rate(&mut self, rate: u32) {
        let (fs, dec) = rate_plan(rate);
        self.device_params().fs_freq.fs_hz = fs;
        let ch = self.channel();
        ch.ctrl.decimation.enable = u8::from(dec > 1);
        ch.ctrl.decimation.factor = dec;
        ch.ctrl.decimation.wide_band = 1;
        ch.tuner.bw_type = if_bandwidth_khz(rate);
        self.rate = rate;
    }

    /// Writes the gain stage at the current frequency. Returns nothing: the caller pushes an update.
    fn write_gain(&mut self) {
        let (lna, gr) = gain_split(self.hw, self.freq_hz as f64, self.gain_db);
        let g = &mut self.channel().tuner.gain;
        g.lna_state = lna;
        g.gr_db = gr;
    }

    /// Writes the frequency, then the gain (the LNA table depends on the band).
    fn write_freq(&mut self, hz: u64) {
        self.freq_hz = hz;
        self.channel().tuner.rf_freq.rf_hz = hz as f64;
        self.write_gain();
    }

    /// Sends the changed fields to a running radio.
    fn push_update(&self, reason: c_uint) -> Result<(), String> {
        let mut code = SUCCESS;
        for attempt in 0..5 {
            // SAFETY: the device is selected and streaming, so the handle is valid.
            code = unsafe { (self.api.update)(self.dev.dev, self.dev.tuner, reason, 0) };
            if ![ERR_GAIN_UPDATE, ERR_RF_UPDATE, ERR_FS_UPDATE].contains(&code) {
                break;
            }
            if attempt < 4 {
                thread::sleep(Duration::from_millis(20));
            }
        }
        if code != SUCCESS {
            return Err(format!("SDRplay: the radio did not take the new settings: {}", self.api.err_text(code)));
        }
        Ok(())
    }

    fn attach(&mut self) -> Result<(), String> {
        let mut params: *mut DeviceParamsT = ptr::null_mut();
        // SAFETY: the device handle comes from SelectDevice; the vendor fills `params`.
        let code = unsafe { (self.api.get_device_params)(self.dev.dev, &mut params) };
        if code != SUCCESS || params.is_null() {
            return Err(format!("SDRplay: cannot read the radio's settings: {}", self.api.err_text(code)));
        }
        // SAFETY: params was just returned by the vendor and is valid while the device is selected.
        let p = unsafe { &*params };
        let chan = if self.dev.tuner == TUNER_B { p.rx_b } else { p.rx_a };
        if chan.is_null() || p.dev_params.is_null() {
            return Err("SDRplay: cannot read the radio's settings for this antenna input".into());
        }
        self.chan = chan;
        self.dev_params = p.dev_params;
        Ok(())
    }
}

impl RxRadio for SdrplayRadio {
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
                "SDRplay {} tunes {} Hz to {} Hz; {} Hz is outside that",
                model_name(self.hw),
                lo,
                hi,
                hz
            ));
        }
        self.write_freq(hz);
        if self.running {
            self.push_update(UPDATE_TUNER_FRF | UPDATE_TUNER_GR)?;
        }
        Ok(())
    }

    fn sample_rates(&self) -> Vec<u32> {
        RATES.to_vec()
    }

    fn set_sample_rate(&mut self, hz: u32) -> Result<u32, String> {
        if self.running {
            return Err("SDRplay: stop the radio before changing the sample rate".into());
        }
        let rate = closest_rate(hz);
        self.write_rate(rate);
        Ok(rate)
    }

    fn gain_stages(&self) -> Vec<GainStage> {
        vec![GainStage {
            name: GAIN_STAGE.into(),
            min_db: 0.0,
            max_db: max_gain(self.hw) as f32,
            step_db: 1.0,
            auto: false,
        }]
    }

    fn set_gain(&mut self, stage: &str, setting: GainSetting) -> Result<(), String> {
        if stage != GAIN_STAGE {
            return Err(format!("SDRplay has no gain stage named {stage:?}; the only one is {GAIN_STAGE:?}"));
        }
        let db = match setting {
            GainSetting::Auto => return Err("the SDRplay gain stage has no automatic setting; give a value in dB".into()),
            GainSetting::Manual(v) => v as f64,
        };
        self.gain_db = db.clamp(0.0, max_gain(self.hw) as f64);
        self.write_gain();
        if self.running {
            self.push_update(UPDATE_TUNER_GR)?;
        }
        Ok(())
    }

    fn start(&mut self) -> Result<(), String> {
        if self.running {
            return Ok(());
        }
        self.shared.removed.store(false, Ordering::Release);
        self.shared.device.store(self.dev.dev, Ordering::Release);
        self.shared.tuner.store(self.dev.tuner, Ordering::Release);
        let mut cb = CallbackFnsT { stream_a: on_stream, stream_b: on_stream, event: on_event };
        let ctx = Arc::as_ptr(&self.shared) as *const c_void as *mut c_void;
        // SAFETY: the callbacks and the context stay valid until Uninit returns (see stop).
        let code = unsafe { (self.api.init)(self.dev.dev, &mut cb, ctx) };
        if code != SUCCESS {
            return Err(format!("SDRplay: the radio did not start: {}", self.api.err_text(code)));
        }
        self.running = true;
        Ok(())
    }

    fn read_iq(&mut self, out: &mut [(f32, f32)]) -> Result<usize, String> {
        let shared = Arc::clone(&self.shared);
        let mut buf = lock(&shared.samples);
        if buf.is_empty() && !shared.removed.load(Ordering::Acquire) {
            let (guard, _) = shared.ready.wait_timeout(buf, READ_WAIT).unwrap_or_else(|p| p.into_inner());
            buf = guard;
        }
        if buf.is_empty() {
            if shared.removed.load(Ordering::Acquire) {
                return Err("SDRplay: the radio was unplugged or stopped working".into());
            }
            return Ok(0);
        }
        let n = out.len().min(buf.len());
        for (slot, s) in out.iter_mut().zip(buf.drain(..n)) {
            *slot = s;
        }
        Ok(n)
    }

    fn stop(&mut self) {
        if !self.running {
            return;
        }
        // SAFETY: the stream was started with this handle; Uninit ends the callbacks.
        unsafe { (self.api.uninit)(self.dev.dev) };
        self.running = false;
    }

    fn dropped_samples(&self) -> u64 {
        self.shared.dropped.load(Ordering::Relaxed)
    }
}

impl Drop for SdrplayRadio {
    fn drop(&mut self) {
        self.stop();
        // SAFETY: the device was selected by open() and is released once here.
        unsafe { (self.api.release_device)(&mut self.dev) };
    }
}

// ---------------------------------------------------------------- driver entry points

/// Lists the radios that the service reports as usable. Returns nothing when the library is missing.
pub fn probe() -> Vec<DeviceInfo> {
    let Ok(api) = api() else { return Vec::new() };
    let Ok(version) = api.ensure_service() else { return Vec::new() };
    let Ok(devs) = api.list_devices() else { return Vec::new() };
    devs.iter()
        .filter(|d| is_usable(version, d))
        .map(|d| {
            let serial = serial_of(d);
            DeviceInfo { driver: DRIVER, label: format!("SDRplay {} {}", model_name(d.hw_ver), serial), id: serial }
        })
        .collect()
}

/// Opens the radio with serial `id`, or the first usable radio when `id` is empty.
pub fn open(id: Option<&str>) -> Result<Box<dyn RxRadio>, String> {
    let api = api()?;
    let version = api.ensure_service()?;
    let wanted = id.map(str::trim).filter(|s| !s.is_empty());
    let devs = api.list_devices()?;
    let mut dev = devs
        .into_iter()
        .filter(|d| is_usable(version, d))
        .find(|d| wanted.map_or(true, |w| serial_of(d) == w))
        .ok_or_else(|| match wanted {
            Some(w) => format!("SDRplay: no radio with serial {w} is connected, or it is in use by another program"),
            None => "SDRplay: no radio found; plug it in and check that the SDRplay API service is running".into(),
        })?;
    if dev.hw_ver == HW_RSPDUO {
        // use tuner 1 on its own, the way a single-tuner RSP works
        dev.tuner = TUNER_A;
        dev.rsp_duo_mode = DUO_SINGLE_TUNER;
        dev.rsp_duo_sample_freq = 0.0;
    }
    let serial = serial_of(&dev);
    // SAFETY: `dev` is a copy of a listed device; the vendor fills in its handle.
    let code = unsafe {
        (api.lock_devices)();
        let code = (api.select_device)(&mut dev);
        (api.unlock_devices)();
        code
    };
    if code != SUCCESS {
        return Err(format!("SDRplay: cannot open the radio {serial} (in use by another program?): {}", api.err_text(code)));
    }
    if dev.tuner != TUNER_A && dev.tuner != TUNER_B {
        dev.tuner = TUNER_A;
    }
    let mut radio = SdrplayRadio {
        api,
        label: format!("SDRplay {} {}", model_name(dev.hw_ver), serial),
        hw: dev.hw_ver,
        dev,
        dev_params: ptr::null_mut(),
        chan: ptr::null_mut(),
        rate: DEFAULT_RATE,
        freq_hz: DEFAULT_FREQ_HZ,
        gain_db: DEFAULT_GAIN_DB,
        shared: Arc::new(Shared {
            samples: Mutex::new(VecDeque::new()),
            ready: Condvar::new(),
            removed: AtomicBool::new(false),
            dropped: AtomicU64::new(0),
            device: AtomicPtr::new(ptr::null_mut()),
            tuner: AtomicI32::new(TUNER_A),
        }),
        running: false,
    };
    // Dropping `radio` on an error below releases the device.
    radio.attach()?;
    radio.write_rate(DEFAULT_RATE);
    radio.write_freq(DEFAULT_FREQ_HZ);
    {
        let ch = radio.channel();
        ch.tuner.if_type = IF_ZERO;
        ch.tuner.lo_mode = LO_AUTO;
        ch.tuner.gain.min_gr = NORMAL_MIN_GR;
        ch.ctrl.agc.enable = AGC_DISABLE;
        ch.ctrl.dc_offset.dc_enable = 1;
        ch.ctrl.dc_offset.iq_enable = 1;
    }
    Ok(Box::new(radio))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_choice_picks_nearest_table_value() {
        assert_eq!(closest_rate(1_000_000), 1_000_000);
        assert_eq!(closest_rate(2_040_000), 2_048_000);
        assert_eq!(closest_rate(0), 62_500);
        assert_eq!(closest_rate(1_000_000_000), 10_000_000);
        assert!(RATES.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn rate_plan_decimates_below_two_megasamples() {
        assert_eq!(rate_plan(62_500), (2_000_000.0, 32));
        assert_eq!(rate_plan(1_000_000), (2_000_000.0, 2));
        assert_eq!(rate_plan(2_000_000), (2_000_000.0, 1));
        assert_eq!(rate_plan(2_048_000), (2_048_000.0, 1));
        assert_eq!(rate_plan(10_000_000), (10_000_000.0, 1));
    }

    #[test]
    fn if_filter_fits_inside_rate() {
        assert_eq!(if_bandwidth_khz(62_500), 200);
        assert_eq!(if_bandwidth_khz(2_000_000), 1536);
        assert_eq!(if_bandwidth_khz(10_000_000), 8000);
    }

    #[test]
    fn gain_maps_to_lna_and_if_stages() {
        assert_eq!(max_gain(HW_RSP1A), 103);
        // full gain: no reduction at all
        assert_eq!(gain_split(HW_RSP1A, 100e6, 103.0), (0, 20));
        // no gain: the largest LNA reduction, IF stage clamped to its maximum
        assert_eq!(gain_split(HW_RSP1A, 100e6, 0.0), (9, 59));
        // 40 dB at 100 MHz: LNA state 7 (38 dB reduction), IF stage 45 dB
        assert_eq!(gain_split(HW_RSP1A, 100e6, 40.0), (7, 45));
        // out-of-range values are clamped
        assert_eq!(gain_split(HW_RSP1A, 100e6, 500.0), gain_split(HW_RSP1A, 100e6, 103.0));
        assert_eq!(gain_split(HW_RSP1A, 100e6, -5.0), gain_split(HW_RSP1A, 100e6, 0.0));
    }

    #[test]
    fn gain_range_follows_model() {
        assert_eq!(max_gain(HW_RSP2), 103);
        assert_eq!(max_gain(HW_RSPDX), 123);
        assert_eq!(max_gain(HW_RSP1), 82);
    }

    #[test]
    fn samples_convert_to_unit_range() {
        let mut buf = VecDeque::new();
        let dropped = push_iq(&mut buf, &[0, 32767, -32768], &[16384, 0, 0], 10);
        assert_eq!(dropped, 0);
        assert_eq!(buf.len(), 3);
        assert_eq!(buf[0], (0.0, 0.5));
        assert!((buf[1].0 - 32767.0 / 32768.0).abs() < 1e-6);
        assert_eq!(buf[2], (-1.0, 0.0));
    }

    #[test]
    fn samples_over_capacity_are_counted_as_dropped() {
        let mut buf = VecDeque::new();
        let dropped = push_iq(&mut buf, &[1, 2, 3, 4], &[1, 2, 3, 4], 3);
        assert_eq!(buf.len(), 3);
        assert_eq!(dropped, 1);
        assert_eq!(push_iq(&mut buf, &[5], &[5], 3), 1);
    }

    #[test]
    fn missing_library_message_says_what_to_do() {
        let msg = missing_library_message("library not found; tried x");
        assert!(msg.contains("not installed"));
        assert!(msg.contains("sdrplay.com"));
    }

    #[test]
    fn missing_library_is_reported_as_an_error() {
        let err = load_library(&["/nonexistent/dir/libsdrplay_api_test_only.so"]).unwrap_err();
        assert!(err.contains("library not found"));
        assert!(missing_library_message(&err).starts_with("SDRplay API is not installed"));
    }

    #[test]
    fn struct_sizes_match_the_vendor_layout() {
        assert_eq!(std::mem::size_of::<DeviceT>(), 96);
        assert_eq!(std::mem::size_of::<RxChannelParamsT>(), 144);
        assert_eq!(std::mem::size_of::<DevParamsT>(), 64);
    }
}
