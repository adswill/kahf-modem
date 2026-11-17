//! Driver for the RX888 MkII (receive side).
//!
//! The radio has a USB 3 chip (Cypress FX3) that runs an application image. The
//! radio shows as USB id 04b4:00f1 with the product string "RX888mk2" once that
//! image runs. Right after power-up it shows as 04b4:00f3 (loader mode); this
//! driver does not load the image yet and says so.
//!
//! USB access goes through the system USB library (libusb 1.0), loaded at run
//! time with `drivers::common::load_library`. Nothing is linked at build time.
//! The samples arrive as 16-bit real values at the ADC clock (64 or 128 MHz).
//! This driver mixes them to baseband and averages them down to the rate asked
//! for. The averaging is a plain box filter, so weak signals near strong ones
//! may alias more than they should.
//!
//! Not tested on a real radio. The unit tests cover the pure parts only.

use std::ffi::c_void;
use std::f64::consts::PI;
use std::os::raw::c_int;
use std::ptr;
use std::sync::Arc;
use std::time::Duration;

use libloading::Library;

use crate::drivers::common::load_library;
use crate::{DeviceInfo, GainSetting, GainStage, RxRadio};

const VENDOR_ID: u16 = 0x04b4;
const PID_APP: u16 = 0x00f1;
const PID_LOADER: u16 = 0x00f3;
const PRODUCT_MARK: &str = "RX888mk2";
const MODEL_MK2: u8 = 4;

const LIB_NAMES: &[&str] = &[
    "libusb-1.0.dylib",
    "/opt/homebrew/lib/libusb-1.0.dylib",
    "/usr/local/lib/libusb-1.0.dylib",
    "libusb-1.0.so.0",
    "libusb-1.0.so",
    "libusb-1.0.dll",
];

// Vendor requests of the radio's application image.
const CMD_START_STREAM: u8 = 0xAA;
const CMD_STOP_STREAM: u8 = 0xAB;
const CMD_TEST: u8 = 0xAC;
const CMD_GPIO: u8 = 0xAD;
const CMD_ADC_CLOCK: u8 = 0xB2;
const CMD_TUNER_INIT: u8 = 0xB4;
const CMD_TUNE: u8 = 0xB5;
const CMD_SET_ARG: u8 = 0xB6;
const CMD_TUNER_STANDBY: u8 = 0xB8;

// Argument numbers for CMD_SET_ARG.
const ARG_LNA: u16 = 1;
const ARG_VGA: u16 = 2;
const ARG_HF_ATT: u16 = 10;
const ARG_HF_IF: u16 = 11;

// Bits of the GPIO word.
const GPIO_VHF_EN: u32 = 1 << 15;

const ADC_LOW_HZ: u32 = 64_000_000;
const ADC_HIGH_HZ: u32 = 128_000_000;
const RATE_MIN_HZ: u32 = 250_000;
const RATE_MAX_HZ: u32 = 64_000_000;
const MAX_DECIM: u32 = 4096;
const HF_MIN_HZ: u64 = 10_000;
const VHF_MAX_HZ: u64 = 1_750_000_000;
const TUNER_IF_HZ: f64 = 4_570_000.0;
const TUNER_REF_HZ: u32 = 16_000_000;
const GAIN_MAX_DB: f32 = 90.0;
const HF_ATT_MAX_DB: f32 = 31.5;
const DEFAULT_RATE_HZ: u32 = 8_000_000;
const DEFAULT_FREQ_HZ: u64 = 14_200_000;

const READ_CHUNK_BYTES: usize = 256 * 1024;
const READ_TIMEOUT_MS: u32 = 200;
const CONTROL_TIMEOUT_MS: u32 = 5000;
const RETRY_DELAY_MS: u64 = 8;

// libusb error codes (negative) and speed codes.
const ERR_IO: i32 = -1;
const ERR_NO_DEVICE: i32 = -4;
const ERR_BUSY: i32 = -6;
const ERR_TIMEOUT: i32 = -7;
const ERR_PIPE: i32 = -9;
const SPEED_LOW: c_int = 1;
const SPEED_FULL: c_int = 2;
const SPEED_HIGH: c_int = 3;

// ------------------------------------------------------------------ pure parts

/// Text for a USB library error code. Plain English.
pub fn usb_error_text(code: i32) -> String {
    match code {
        -1 => "input/output error on the USB link".into(),
        -2 => "invalid argument to the USB library".into(),
        -3 => "access denied: another program may have the radio open, or this user may not be allowed to use the USB device".into(),
        -4 => "the radio is not connected (unplugged?)".into(),
        -5 => "the USB device was not found".into(),
        -6 => "the USB device is busy".into(),
        -7 => "the radio did not answer in time".into(),
        -8 => "the radio sent more data than expected (overflow)".into(),
        -9 => "the radio refused the request (pipe stall)".into(),
        -10 => "the USB call was interrupted".into(),
        -11 => "out of memory in the USB library".into(),
        -12 => "this operation is not supported on this system".into(),
        -99 => "unknown USB error".into(),
        other => format!("USB error {other}"),
    }
}

fn step_error(what: &str, code: i32) -> String {
    format!("RX888 MkII: {what} ({})", usb_error_text(code))
}

/// The rates the driver offers, ascending. They come from the two ADC clocks divided by whole numbers.
pub fn rate_list() -> Vec<u32> {
    let mut v: Vec<u32> = (2..=256u32).map(|d| round_div(ADC_LOW_HZ, d)).collect();
    v.extend((2..=4u32).map(|d| round_div(ADC_HIGH_HZ, d)));
    v.retain(|&r| (RATE_MIN_HZ..=RATE_MAX_HZ).contains(&r));
    v.sort_unstable();
    v.dedup();
    v
}

fn round_div(clock: u32, d: u32) -> u32 {
    (clock as f64 / d as f64).round() as u32
}

/// The entry of `list` nearest to `hz`.
pub fn choose_rate(list: &[u32], hz: u32) -> u32 {
    list.iter().copied().min_by_key(|&r| (r as i64 - hz as i64).abs()).unwrap_or(DEFAULT_RATE_HZ)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RatePlan {
    /// ADC clock in Hz.
    pub adc_hz: u32,
    /// Whole-number divider of the ADC clock.
    pub decim: u32,
    /// Output rate in Hz (adc / decim). Never below the rate asked for.
    pub out_hz: u32,
}

/// The ADC clock and divider for a rate. The clock is 128 MHz above 32 MHz, else 64 MHz.
pub fn plan_rate(rate_hz: u32) -> RatePlan {
    let want = rate_hz.clamp(RATE_MIN_HZ, RATE_MAX_HZ);
    let adc = if want > 32_000_000 { ADC_HIGH_HZ } else { ADC_LOW_HZ };
    let decim = ((adc as f64 / want as f64 * 1.001).floor() as u32).clamp(2, MAX_DECIM);
    RatePlan { adc_hz: adc, decim, out_hz: round_div(adc, decim) }
}

/// Gain slider in dB, clamped to 0..90.
pub fn clamp_slider(db: f32) -> f32 {
    if db.is_nan() { 0.0 } else { db.clamp(0.0, GAIN_MAX_DB) }
}

/// HF attenuator in dB, clamped to 0..31.5 and rounded to half dB.
pub fn clamp_hf_att(db: f32) -> f32 {
    if db.is_nan() { return 0.0; }
    ((db.clamp(0.0, HF_ATT_MAX_DB) * 2.0).round() / 2.0).clamp(0.0, HF_ATT_MAX_DB)
}

// R828D tuner steps (dB), the LNA + mixer and the VGA.
const VHF_RF_DB: [f64; 29] = [
    0.0, 0.9, 1.4, 2.7, 3.7, 7.7, 8.7, 12.5, 14.4, 15.7, 16.6, 19.7, 20.7, 22.9, 25.4, 28.0, 29.7, 32.8,
    33.8, 36.4, 37.2, 38.6, 40.2, 42.1, 43.4, 43.9, 44.5, 48.0, 49.6,
];
const VHF_IF_DB: [f64; 16] = [-4.7, -2.1, 0.5, 3.5, 7.7, 11.2, 13.6, 14.9, 16.3, 19.5, 23.1, 26.5, 30.0, 33.7, 37.2, 40.8];
const HF_IF_STEPS: usize = 127;
const HF_IF_SWEET_POINT: usize = 18;

/// Register values for one gain setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GainPlan {
    /// HF attenuator code, 0..63 (half dB steps).
    pub att_code: u16,
    /// HF IF gain code as the radio takes it: 1..19 (low range) or 0x80 | 3..111 (high range).
    pub hf_if_code: u16,
    /// R828D LNA and mixer step, 0..28.
    pub lna: u16,
    /// R828D VGA step, 0..15.
    pub vga: u16,
}

fn hf_if_gain_db(index: usize) -> f64 {
    let i = index.min(HF_IF_STEPS - 1);
    if i > HF_IF_SWEET_POINT {
        20.0 * (0.409 * (i - HF_IF_SWEET_POINT + 3) as f64).log10()
    } else {
        20.0 * (0.059 * (i + 1) as f64).log10()
    }
}

fn nearest(table: &[f64], want: f64) -> usize {
    let mut best = 0;
    for (i, &v) in table.iter().enumerate() {
        if (v - want).abs() < (table[best] - want).abs() {
            best = i;
        }
    }
    best
}

/// The register values for a slider (dB) and an HF attenuator (dB). Values are clamped first.
pub fn gain_plan(slider_db: f32, hf_att_db: f32) -> GainPlan {
    let slider = clamp_slider(slider_db) as f64;
    let att = clamp_hf_att(hf_att_db) as f64;
    let att_code = ((att * 2.0).round() as u16).min(63);

    let target = hf_if_gain_db(0) + slider;
    let mut index = 0;
    let mut best = f64::MAX;
    for i in 0..HF_IF_STEPS {
        let d = (hf_if_gain_db(i) - target).abs();
        if d < best {
            best = d;
            index = i;
        }
    }
    let hf_if_code = if index > HF_IF_SWEET_POINT {
        0x80 | (index - HF_IF_SWEET_POINT + 3) as u16
    } else {
        (index + 1) as u16
    };

    let lna = nearest(&VHF_RF_DB, slider.min(VHF_RF_DB[28]));
    let vga = nearest(&VHF_IF_DB, VHF_IF_DB[0] + (slider - VHF_RF_DB[28]).max(0.0));
    GainPlan { att_code, hf_if_code, lna: lna as u16, vga: vga as u16 }
}

fn gpio_word(vhf: bool) -> u32 {
    if vhf { GPIO_VHF_EN } else { 0 }
}

/// Little-endian 16-bit samples from raw bytes. A trailing odd byte is ignored.
pub fn le_samples(bytes: &[u8], out: &mut Vec<i16>) {
    out.clear();
    out.extend(bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])));
}

/// Mixes real samples to baseband and averages them down. Keeps its state between calls.
#[derive(Debug, Clone)]
pub struct Ddc {
    decim: u32,
    step_cos: f64,
    step_sin: f64,
    cos: f64,
    sin: f64,
    /// -1 for the normal spectrum, +1 when the output is conjugated (the tuner's IF path).
    sign: f64,
    acc_i: f64,
    acc_q: f64,
    count: u32,
    since_norm: u32,
}

impl Ddc {
    pub fn new() -> Ddc {
        Ddc { decim: 2, step_cos: 1.0, step_sin: 0.0, cos: 1.0, sin: 0.0, sign: -1.0, acc_i: 0.0, acc_q: 0.0, count: 0, since_norm: 0 }
    }

    /// Sets the ADC clock, the divider, the mixing offset and whether the output is conjugated.
    pub fn configure(&mut self, adc_hz: u32, decim: u32, offset_hz: f64, mirror: bool) {
        let step = 2.0 * PI * offset_hz / adc_hz as f64;
        self.step_cos = step.cos();
        self.step_sin = step.sin();
        self.decim = decim.max(1);
        self.sign = if mirror { 1.0 } else { -1.0 };
        self.acc_i = 0.0;
        self.acc_q = 0.0;
        self.count = 0;
    }

    /// Converts raw samples (full scale 32768) into complex values in -1..1.
    pub fn process(&mut self, raw: &[i16], out: &mut Vec<(f32, f32)>) {
        for &v in raw {
            let x = v as f64 / 32768.0;
            self.acc_i += x * self.cos;
            self.acc_q += self.sign * x * self.sin;
            self.count += 1;

            let nc = self.cos * self.step_cos - self.sin * self.step_sin;
            let ns = self.sin * self.step_cos + self.cos * self.step_sin;
            self.cos = nc;
            self.sin = ns;
            self.since_norm += 1;
            if self.since_norm >= 1024 {
                let r = (self.cos * self.cos + self.sin * self.sin).sqrt();
                self.cos /= r;
                self.sin /= r;
                self.since_norm = 0;
            }

            if self.count == self.decim {
                let d = self.decim as f64;
                out.push(((self.acc_i / d) as f32, (self.acc_q / d) as f32));
                self.acc_i = 0.0;
                self.acc_q = 0.0;
                self.count = 0;
            }
        }
    }
}

// ------------------------------------------------------------- libusb access

type FnInit = unsafe extern "C" fn(*mut *mut c_void) -> c_int;
type FnExit = unsafe extern "C" fn(*mut c_void);
type FnGetList = unsafe extern "C" fn(*mut c_void, *mut *mut *mut c_void) -> isize;
type FnFreeList = unsafe extern "C" fn(*mut *mut c_void, c_int);
type FnGetDesc = unsafe extern "C" fn(*mut c_void, *mut DeviceDesc) -> c_int;
type FnDevNum = unsafe extern "C" fn(*mut c_void) -> u8;
type FnSpeed = unsafe extern "C" fn(*mut c_void) -> c_int;
type FnOpen = unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> c_int;
type FnClose = unsafe extern "C" fn(*mut c_void);
type FnString = unsafe extern "C" fn(*mut c_void, u8, *mut u8, c_int) -> c_int;
type FnIface = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
type FnControl = unsafe extern "C" fn(*mut c_void, u8, u8, u16, u16, *mut u8, u16, u32) -> c_int;
type FnBulk = unsafe extern "C" fn(*mut c_void, u8, *mut u8, c_int, *mut c_int, u32) -> c_int;
type FnGetCfg = unsafe extern "C" fn(*mut c_void, *mut *const ConfigDesc) -> c_int;
type FnFreeCfg = unsafe extern "C" fn(*const ConfigDesc);

/// The USB device descriptor, 18 bytes, as the library lays it out.
#[repr(C)]
#[derive(Default)]
struct DeviceDesc {
    b_length: u8,
    b_descriptor_type: u8,
    bcd_usb: u16,
    device_class: u8,
    device_sub_class: u8,
    device_protocol: u8,
    max_packet_size0: u8,
    vendor_id: u16,
    product_id: u16,
    bcd_device: u16,
    manufacturer_idx: u8,
    product_idx: u8,
    serial_idx: u8,
    num_configurations: u8,
}

#[repr(C)]
struct ConfigDesc {
    b_length: u8,
    b_descriptor_type: u8,
    w_total_length: u16,
    b_num_interfaces: u8,
    b_configuration_value: u8,
    i_configuration: u8,
    bm_attributes: u8,
    b_max_power: u8,
    interface: *const InterfaceGroup,
}

#[repr(C)]
struct InterfaceGroup {
    altsetting: *const AltSetting,
    num_altsetting: c_int,
}

#[repr(C)]
struct AltSetting {
    b_length: u8,
    b_descriptor_type: u8,
    b_interface_number: u8,
    b_alternate_setting: u8,
    b_num_endpoints: u8,
    b_interface_class: u8,
    b_interface_sub_class: u8,
    b_interface_protocol: u8,
    i_interface: u8,
    endpoint: *const EndpointDesc,
    extra: *const u8,
    extra_length: c_int,
}

#[repr(C)]
struct EndpointDesc {
    b_length: u8,
    b_descriptor_type: u8,
    b_endpoint_address: u8,
    bm_attributes: u8,
    w_max_packet_size: u16,
    b_interval: u8,
    b_refresh: u8,
    b_synch_address: u8,
    extra: *const u8,
    extra_length: c_int,
}

/// The library functions this driver calls, loaded once. The library stays loaded while this lives.
struct Api {
    _lib: Library,
    init: FnInit,
    exit: FnExit,
    get_list: FnGetList,
    free_list: FnFreeList,
    get_desc: FnGetDesc,
    bus: FnDevNum,
    address: FnDevNum,
    speed: FnSpeed,
    open: FnOpen,
    close: FnClose,
    string: FnString,
    kernel_active: FnIface,
    detach: FnIface,
    claim: FnIface,
    release: FnIface,
    control: FnControl,
    bulk: FnBulk,
    get_cfg: FnGetCfg,
    free_cfg: FnFreeCfg,
}

impl Api {
    fn load() -> Result<Api, String> {
        let lib = load_library(LIB_NAMES).map_err(|e| format!("the USB library (libusb 1.0) is not installed or cannot be loaded: {e}"))?;
        macro_rules! sym {
            ($name:literal, $ty:ty) => {{
                let s = unsafe { lib.get::<$ty>(concat!($name, "\0").as_bytes()) }
                    .map_err(|e| format!("the USB library is missing {}: {e}", $name))?;
                *s
            }};
        }
        let api = Api {
            init: sym!("libusb_init", FnInit),
            exit: sym!("libusb_exit", FnExit),
            get_list: sym!("libusb_get_device_list", FnGetList),
            free_list: sym!("libusb_free_device_list", FnFreeList),
            get_desc: sym!("libusb_get_device_descriptor", FnGetDesc),
            bus: sym!("libusb_get_bus_number", FnDevNum),
            address: sym!("libusb_get_device_address", FnDevNum),
            speed: sym!("libusb_get_device_speed", FnSpeed),
            open: sym!("libusb_open", FnOpen),
            close: sym!("libusb_close", FnClose),
            string: sym!("libusb_get_string_descriptor_ascii", FnString),
            kernel_active: sym!("libusb_kernel_driver_active", FnIface),
            detach: sym!("libusb_detach_kernel_driver", FnIface),
            claim: sym!("libusb_claim_interface", FnIface),
            release: sym!("libusb_release_interface", FnIface),
            control: sym!("libusb_control_transfer", FnControl),
            bulk: sym!("libusb_bulk_transfer", FnBulk),
            get_cfg: sym!("libusb_get_active_config_descriptor", FnGetCfg),
            free_cfg: sym!("libusb_free_config_descriptor", FnFreeCfg),
            _lib: lib,
        };
        Ok(api)
    }
}

/// One libusb context. Handles must be closed before it is dropped.
struct Session {
    api: Arc<Api>,
    ctx: *mut c_void,
}

/// A radio found on the bus.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Found {
    bus: u8,
    address: u8,
    product_id: u16,
    serial: String,
}

impl Found {
    fn id(&self) -> String {
        if self.serial.is_empty() { format!("{}:{}", self.bus, self.address) } else { self.serial.clone() }
    }
}

impl Session {
    fn new(api: Arc<Api>) -> Result<Session, String> {
        let mut ctx: *mut c_void = ptr::null_mut();
        let r = unsafe { (api.init)(&mut ctx) };
        if r != 0 || ctx.is_null() {
            return Err(format!("the USB library would not start ({})", usb_error_text(r)));
        }
        Ok(Session { api, ctx })
    }

    fn read_string(&self, handle: *mut c_void, index: u8) -> String {
        if index == 0 {
            return String::new();
        }
        let mut buf = [0u8; 256];
        let n = unsafe { (self.api.string)(handle, index, buf.as_mut_ptr(), buf.len() as c_int) };
        if n <= 0 {
            return String::new();
        }
        let n = (n as usize).min(buf.len());
        String::from_utf8_lossy(&buf[..n]).trim_end_matches('\0').to_string()
    }

    /// Every radio of the bus that is an RX888 MkII (running image) or in loader mode.
    fn devices(&self) -> Result<Vec<Found>, String> {
        let api = &*self.api;
        let mut list: *mut *mut c_void = ptr::null_mut();
        let n = unsafe { (api.get_list)(self.ctx, &mut list) };
        if n < 0 {
            return Err(format!("could not list the USB devices ({})", usb_error_text(n as i32)));
        }
        let mut out = Vec::new();
        for i in 0..n as usize {
            let dev = unsafe { *list.add(i) };
            let mut d = DeviceDesc::default();
            if unsafe { (api.get_desc)(dev, &mut d) } != 0 || d.vendor_id != VENDOR_ID {
                continue;
            }
            let bus = unsafe { (api.bus)(dev) };
            let address = unsafe { (api.address)(dev) };
            if d.product_id == PID_LOADER {
                out.push(Found { bus, address, product_id: PID_LOADER, serial: String::new() });
            } else if d.product_id == PID_APP {
                let mut h: *mut c_void = ptr::null_mut();
                if unsafe { (api.open)(dev, &mut h) } != 0 || h.is_null() {
                    continue;
                }
                let product = self.read_string(h, d.product_idx);
                let serial = self.read_string(h, d.serial_idx);
                unsafe { (api.close)(h) };
                if product.contains(PRODUCT_MARK) {
                    out.push(Found { bus, address, product_id: PID_APP, serial });
                }
            }
        }
        unsafe { (api.free_list)(list, 1) };
        Ok(out)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        unsafe { (self.api.exit)(self.ctx) };
    }
}

/// An open handle with the bulk input endpoint known.
struct Link {
    api: Arc<Api>,
    handle: *mut c_void,
    endpoint: u8,
    claimed: bool,
}

impl Link {
    fn open(session: &Session, bus: u8, address: u8) -> Result<Link, String> {
        let api = session.api.clone();
        let mut list: *mut *mut c_void = ptr::null_mut();
        let n = unsafe { (api.get_list)(session.ctx, &mut list) };
        if n < 0 {
            return Err(format!("could not list the USB devices ({})", usb_error_text(n as i32)));
        }
        let mut result: Result<Link, String> = Err("the radio is no longer connected (unplugged?)".into());
        for i in 0..n as usize {
            let dev = unsafe { *list.add(i) };
            let mut d = DeviceDesc::default();
            if unsafe { (api.get_desc)(dev, &mut d) } != 0 || d.vendor_id != VENDOR_ID {
                continue;
            }
            if unsafe { (api.bus)(dev) } != bus || unsafe { (api.address)(dev) } != address {
                continue;
            }
            let mut h: *mut c_void = ptr::null_mut();
            let r = unsafe { (api.open)(dev, &mut h) };
            if r != 0 || h.is_null() {
                result = Err(step_error("cannot open the radio", r));
                break;
            }
            let mut link = Link { api: api.clone(), handle: h, endpoint: 0, claimed: false };
            if unsafe { (api.kernel_active)(h, 0) } == 1 {
                unsafe { (api.detach)(h, 0) };
            }
            let r = unsafe { (api.claim)(h, 0) };
            if r != 0 {
                result = Err(step_error("cannot claim the radio's USB interface", r));
                break;
            }
            link.claimed = true;
            let speed = unsafe { (api.speed)(dev) };
            if speed == SPEED_LOW || speed == SPEED_FULL || speed == SPEED_HIGH {
                result = Err("the radio is on a USB 2 port; it needs a USB 3 port to carry its samples".into());
                break;
            }
            match find_bulk_in(&api, dev) {
                Some(ep) => link.endpoint = ep,
                None => {
                    result = Err("RX888 MkII: no bulk input endpoint found on the radio".into());
                    break;
                }
            }
            result = Ok(link);
            break;
        }
        unsafe { (api.free_list)(list, 1) };
        result
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        unsafe {
            if self.claimed {
                (self.api.release)(self.handle, 0);
            }
            (self.api.close)(self.handle);
        }
    }
}

fn find_bulk_in(api: &Api, dev: *mut c_void) -> Option<u8> {
    let mut cfg: *const ConfigDesc = ptr::null();
    if unsafe { (api.get_cfg)(dev, &mut cfg) } != 0 || cfg.is_null() {
        return None;
    }
    let mut found = None;
    unsafe {
        let c = &*cfg;
        let groups = if c.interface.is_null() { &[][..] } else { std::slice::from_raw_parts(c.interface, c.b_num_interfaces as usize) };
        'search: for group in groups {
            if group.altsetting.is_null() || group.num_altsetting <= 0 {
                continue;
            }
            let alts = std::slice::from_raw_parts(group.altsetting, group.num_altsetting as usize);
            for alt in alts {
                if alt.endpoint.is_null() {
                    continue;
                }
                let eps = std::slice::from_raw_parts(alt.endpoint, alt.b_num_endpoints as usize);
                for ep in eps {
                    // IN direction (bit 7) and bulk transfer type (2 in the low two bits of bmAttributes)
                    if ep.b_endpoint_address & 0x80 != 0 && ep.bm_attributes & 0x03 == 0x02 {
                        found = Some(ep.b_endpoint_address);
                        break 'search;
                    }
                }
            }
        }
        (api.free_cfg)(cfg);
    }
    found
}

// ----------------------------------------------------------------- the radio

/// Last values sent to the radio, so unchanged settings are not sent again.
#[derive(Debug, Clone, Default)]
struct Applied {
    vhf: Option<bool>,
    adc_hz: u32,
    gpio: Option<u32>,
    tune_hz: Option<u64>,
    att: Option<u16>,
    hf_if: Option<u16>,
    lna: Option<u16>,
    vga: Option<u16>,
}

pub struct Rx888 {
    // Field order matters: the link is closed before the session ends the libusb context.
    link: Link,
    _session: Session,
    serial: String,
    running: bool,
    rate_hz: u32,
    freq_hz: u64,
    slider_db: f32,
    hf_att_db: f32,
    applied: Applied,
    ddc: Ddc,
    pending: Vec<(f32, f32)>,
    pending_pos: usize,
}

// The libusb handles are used from one thread at a time (the trait contract).
unsafe impl Send for Rx888 {}

impl Rx888 {
    fn raw_ctl(&self, input: bool, request: u8, value: u16, index: u16, buf: &mut [u8]) -> i32 {
        let api = &*self.link.api;
        let direction = if input { 0xC0 } else { 0x40 };
        let mut r = 0;
        for attempt in 0..3 {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(RETRY_DELAY_MS));
            }
            r = unsafe {
                (api.control)(self.link.handle, direction, request, value, index, buf.as_mut_ptr(), buf.len() as u16, CONTROL_TIMEOUT_MS)
            };
            if r >= 0 {
                return r;
            }
            if ![ERR_PIPE, ERR_IO, ERR_TIMEOUT, ERR_BUSY].contains(&r) {
                break;
            }
        }
        r
    }

    /// Sends one command with data. `what` names the step in the error.
    fn cmd(&self, what: &str, request: u8, value: u16, index: u16, data: &[u8]) -> Result<(), String> {
        let mut buf = data.to_vec();
        let r = self.raw_ctl(false, request, value, index, &mut buf);
        if r == data.len() as i32 { Ok(()) } else { Err(step_error(what, r)) }
    }

    fn set_arg(&self, arg: u16, value: u16, what: &str) -> Result<(), String> {
        self.cmd(what, CMD_SET_ARG, value, arg, &[0])
    }

    fn write_gpio(&mut self, word: u32) -> Result<(), String> {
        self.cmd("the antenna switch could not be set", CMD_GPIO, 0, 0, &word.to_le_bytes())?;
        self.applied.gpio = Some(word);
        Ok(())
    }

    fn tuner_standby(&self) -> Result<(), String> {
        let mut b = [0u8; 1];
        let r = self.raw_ctl(false, CMD_TUNER_STANDBY, 0, 0, &mut b);
        // Some stacks refuse this on the first request after opening; the tuner is in standby by default.
        if r == 1 || r == ERR_PIPE { Ok(()) } else { Err(step_error("the tuner could not be put on standby", r)) }
    }

    fn switch_input(&mut self, vhf: bool) -> Result<(), String> {
        if vhf {
            self.set_arg(ARG_HF_ATT, 63, "the VHF input could not be switched on (HF attenuator)")?;
            self.set_arg(ARG_HF_IF, 1, "the VHF input could not be switched on (HF gain)")?;
            self.write_gpio(gpio_word(true))?;
            self.set_arg(ARG_HF_IF, 0x80 | 3, "the VHF input could not be switched on (HF gain)")?;
            self.cmd("the VHF input could not be switched on (tuner reference)", CMD_TUNER_INIT, 0, 0, &TUNER_REF_HZ.to_le_bytes())?;
            self.applied.att = Some(63);
            self.applied.hf_if = Some(0x80 | 3);
            self.applied.lna = None;
            self.applied.vga = None;
            self.applied.tune_hz = None;
        } else {
            self.tuner_standby()?;
            self.write_gpio(gpio_word(false))?;
            self.applied.att = None;
            self.applied.hf_if = None;
            self.applied.lna = None;
            self.applied.vga = None;
        }
        self.applied.vhf = Some(vhf);
        Ok(())
    }

    /// Checks that the radio is an RX888 MkII.
    fn handshake(&mut self) -> Result<(), String> {
        let mut info = [0u8; 4];
        let r = self.raw_ctl(true, CMD_TEST, 0, 0, &mut info);
        if r == 4 {
            if info[0] != MODEL_MK2 {
                return Err(format!("the radio reports model {} and is not an RX888 MkII", info[0]));
            }
        } else if r != ERR_PIPE {
            return Err(step_error("the radio did not answer", r));
        }
        Ok(())
    }

    /// Sends every setting that changed since the last send.
    fn apply(&mut self) -> Result<(), String> {
        let plan = plan_rate(self.rate_hz);
        let vhf = self.freq_hz >= (plan.adc_hz / 2) as u64;

        // The HF path goes first, as the radio needs it before the ADC starts.
        if self.applied.vhf.is_none() {
            self.switch_input(false)?;
        }
        if self.applied.adc_hz != plan.adc_hz {
            self.cmd("the ADC clock was refused", CMD_ADC_CLOCK, 0, 0, &plan.adc_hz.to_le_bytes())?;
            self.applied.adc_hz = plan.adc_hz;
        }
        if self.applied.vhf != Some(vhf) {
            self.switch_input(vhf)?;
        }
        let word = gpio_word(vhf);
        if self.applied.gpio != Some(word) {
            self.write_gpio(word)?;
        }

        let g = gain_plan(self.slider_db, self.hf_att_db);
        if vhf {
            if self.applied.tune_hz != Some(self.freq_hz) {
                self.cmd("the tuner was not told the frequency", CMD_TUNE, 0, 0, &self.freq_hz.to_le_bytes())?;
                self.applied.tune_hz = Some(self.freq_hz);
            }
            if self.applied.lna != Some(g.lna) {
                self.set_arg(ARG_LNA, g.lna, "the tuner LNA could not be set")?;
                self.applied.lna = Some(g.lna);
            }
            if self.applied.vga != Some(g.vga) {
                self.set_arg(ARG_VGA, g.vga, "the tuner VGA could not be set")?;
                self.applied.vga = Some(g.vga);
            }
        } else {
            if self.applied.att != Some(g.att_code) {
                self.set_arg(ARG_HF_ATT, g.att_code, "the HF attenuator could not be set")?;
                self.applied.att = Some(g.att_code);
            }
            if self.applied.hf_if != Some(g.hf_if_code) {
                self.set_arg(ARG_HF_IF, g.hf_if_code, "the HF gain could not be set")?;
                self.applied.hf_if = Some(g.hf_if_code);
            }
        }

        // On the tuner path the converter output sits on the IF and is conjugated.
        let offset = if vhf { TUNER_IF_HZ } else { self.freq_hz as f64 };
        self.ddc.configure(plan.adc_hz, plan.decim, offset, vhf);
        Ok(())
    }
}

impl Drop for Rx888 {
    fn drop(&mut self) {
        self.stop();
    }
}

fn pick_device(found: &[Found], id: Option<&str>) -> Result<Found, String> {
    match id.map(str::trim).filter(|s| !s.is_empty()) {
        Some(want) => found
            .iter()
            .find(|f| f.serial == want || format!("{}:{}", f.bus, f.address) == want)
            .cloned()
            .ok_or_else(|| format!("no RX888 MkII with id {want:?} is connected")),
        None => found
            .iter()
            .find(|f| f.product_id == PID_APP)
            .or_else(|| found.first())
            .cloned()
            .ok_or_else(|| "no RX888 MkII found: check the USB cable and that no other program has the radio open".to_string()),
    }
}

/// Finds the RX888 MkII radios on the bus. Quick; returns nothing when the USB library is missing.
pub fn probe() -> Vec<DeviceInfo> {
    let Ok(api) = Api::load() else {
        return Vec::new();
    };
    let Ok(session) = Session::new(Arc::new(api)) else {
        return Vec::new();
    };
    let Ok(found) = session.devices() else {
        return Vec::new();
    };
    found
        .iter()
        .map(|f| DeviceInfo {
            driver: "rx888",
            label: if f.product_id == PID_LOADER {
                "RX888 MkII (loader mode: loading its firmware is not written yet)".into()
            } else {
                "RX888 MkII".into()
            },
            id: f.id(),
        })
        .collect()
}

/// Opens an RX888 MkII. `id` is a serial number or "bus:address"; None takes the first one found.
pub fn open(id: Option<&str>) -> Result<Box<dyn RxRadio>, String> {
    let api = Arc::new(Api::load()?);
    let session = Session::new(api)?;
    let found = session.devices()?;
    let pick = pick_device(&found, id)?;
    if pick.product_id == PID_LOADER {
        return Err("the RX888 MkII is in loader mode; loading its firmware is not written yet".into());
    }
    let link = Link::open(&session, pick.bus, pick.address)?;
    let mut radio = Rx888 {
        link,
        _session: session,
        serial: pick.serial.clone(),
        running: false,
        rate_hz: DEFAULT_RATE_HZ,
        freq_hz: DEFAULT_FREQ_HZ,
        slider_db: 0.0,
        hf_att_db: 0.0,
        applied: Applied::default(),
        ddc: Ddc::new(),
        pending: Vec::new(),
        pending_pos: 0,
    };
    radio.handshake()?;
    radio.apply()?;
    Ok(Box::new(radio))
}

impl RxRadio for Rx888 {
    fn label(&self) -> String {
        let tail: String = self.serial.chars().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect();
        if tail.is_empty() { "RX888 MkII".into() } else { format!("RX888 MkII {tail}") }
    }

    fn freq_range_hz(&self) -> (u64, u64) {
        (HF_MIN_HZ, VHF_MAX_HZ)
    }

    fn set_frequency_hz(&mut self, hz: u64) -> Result<(), String> {
        if !(HF_MIN_HZ..=VHF_MAX_HZ).contains(&hz) {
            return Err(format!("{hz} Hz is outside the RX888 MkII tuning range of 10 kHz to 1750 MHz"));
        }
        let old = self.freq_hz;
        self.freq_hz = hz;
        let r = self.apply();
        if r.is_err() {
            self.freq_hz = old;
        }
        r
    }

    fn sample_rates(&self) -> Vec<u32> {
        rate_list()
    }

    fn set_sample_rate(&mut self, hz: u32) -> Result<u32, String> {
        let old = self.rate_hz;
        self.rate_hz = choose_rate(&rate_list(), hz);
        let r = self.apply();
        match r {
            Ok(()) => Ok(plan_rate(self.rate_hz).out_hz),
            Err(e) => {
                self.rate_hz = old;
                Err(e)
            }
        }
    }

    fn gain_stages(&self) -> Vec<GainStage> {
        vec![
            GainStage { name: "gain".into(), min_db: 0.0, max_db: GAIN_MAX_DB, step_db: 0.0, auto: false },
            GainStage { name: "hf_attenuator".into(), min_db: 0.0, max_db: HF_ATT_MAX_DB, step_db: 0.5, auto: false },
        ]
    }

    fn set_gain(&mut self, stage: &str, setting: GainSetting) -> Result<(), String> {
        let value = match setting {
            GainSetting::Auto => return Err(format!("gain stage {stage:?} has no automatic mode")),
            GainSetting::Manual(v) => v,
        };
        match stage {
            "gain" => self.slider_db = clamp_slider(value),
            "hf_attenuator" => self.hf_att_db = clamp_hf_att(value),
            other => return Err(format!("no gain stage named {other:?}; the stages are gain and hf_attenuator")),
        }
        self.apply()
    }

    fn start(&mut self) -> Result<(), String> {
        if self.running {
            return Ok(());
        }
        let mut b = [0u8; 1];
        let r = self.raw_ctl(false, CMD_START_STREAM, 0, 0, &mut b);
        if r != 1 {
            return Err(step_error("the radio did not start streaming", r));
        }
        self.pending.clear();
        self.pending_pos = 0;
        self.running = true;
        Ok(())
    }

    fn read_iq(&mut self, out: &mut [(f32, f32)]) -> Result<usize, String> {
        if !self.running {
            return Err("RX888 MkII: not streaming; start it before reading samples".into());
        }
        let mut n = 0;
        while n < out.len() && self.pending_pos < self.pending.len() {
            out[n] = self.pending[self.pending_pos];
            self.pending_pos += 1;
            n += 1;
        }
        if self.pending_pos >= self.pending.len() {
            self.pending.clear();
            self.pending_pos = 0;
        }
        if n > 0 {
            return Ok(n);
        }

        let api = self.link.api.clone();
        let mut raw = vec![0u8; READ_CHUNK_BYTES];
        let mut got: c_int = 0;
        let r = unsafe {
            (api.bulk)(self.link.handle, self.link.endpoint, raw.as_mut_ptr(), raw.len() as c_int, &mut got, READ_TIMEOUT_MS)
        };
        let got = got.max(0) as usize;
        if r != 0 && r != ERR_TIMEOUT {
            if r == ERR_NO_DEVICE {
                return Err("the RX888 MkII was unplugged".into());
            }
            return Err(step_error("reading samples failed", r));
        }
        if got == 0 {
            return Ok(0);
        }

        let mut samples = Vec::with_capacity(got / 2);
        le_samples(&raw[..got], &mut samples);
        self.pending.clear();
        self.pending_pos = 0;
        self.ddc.process(&samples, &mut self.pending);

        while n < out.len() && self.pending_pos < self.pending.len() {
            out[n] = self.pending[self.pending_pos];
            self.pending_pos += 1;
            n += 1;
        }
        if self.pending_pos >= self.pending.len() {
            self.pending.clear();
            self.pending_pos = 0;
        }
        Ok(n)
    }

    fn stop(&mut self) {
        if self.running {
            let mut b = [0u8; 1];
            let _ = self.raw_ctl(false, CMD_STOP_STREAM, 0, 0, &mut b);
            self.running = false;
        }
    }
}

// ------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_choice_picks_nearest_offered_rate() {
        let list = rate_list();
        assert!(list.windows(2).all(|w| w[0] < w[1]), "list must be ascending");
        assert_eq!(choose_rate(&list, 2_000_000), 2_000_000);
        assert_eq!(choose_rate(&list, 0), RATE_MIN_HZ);
        assert_eq!(choose_rate(&list, u32::MAX), RATE_MAX_HZ);
        assert_eq!(choose_rate(&list, 2_000_100), 2_000_000);
    }

    #[test]
    fn rate_plan_never_goes_below_the_request() {
        assert_eq!(plan_rate(2_000_000), RatePlan { adc_hz: 64_000_000, decim: 32, out_hz: 2_000_000 });
        assert_eq!(plan_rate(50_000_000), RatePlan { adc_hz: 128_000_000, decim: 2, out_hz: 64_000_000 });
        assert_eq!(plan_rate(1), RatePlan { adc_hz: 64_000_000, decim: 256, out_hz: 250_000 });
        let p = plan_rate(21_333_333);
        assert_eq!((p.adc_hz, p.decim), (64_000_000, 3));
    }

    #[test]
    fn gain_slider_and_attenuator_are_clamped() {
        assert_eq!(clamp_slider(-5.0), 0.0);
        assert_eq!(clamp_slider(200.0), GAIN_MAX_DB);
        assert_eq!(clamp_slider(f32::NAN), 0.0);
        assert_eq!(clamp_hf_att(-1.0), 0.0);
        assert_eq!(clamp_hf_att(40.0), HF_ATT_MAX_DB);
        assert_eq!(clamp_hf_att(10.3), 10.5);
    }

    #[test]
    fn gain_plan_values_follow_the_tables() {
        let zero = gain_plan(0.0, 0.0);
        assert_eq!(zero.hf_if_code, 1);
        assert_eq!((zero.lna, zero.vga), (0, 0));
        assert_eq!(gain_plan(0.0, 10.0).att_code, 20);
        assert_eq!(gain_plan(0.0, 40.0).att_code, 63);
        let top = gain_plan(90.0, 0.0);
        assert_eq!(top.lna, 28);
        assert_eq!(top.vga, 14);
        assert_eq!(gain_plan(200.0, 0.0), gain_plan(90.0, 0.0), "out of range values are clamped first");
    }

    #[test]
    fn hf_if_gain_rises_with_the_index() {
        let mut last = f64::MIN;
        for i in 0..HF_IF_STEPS {
            let g = hf_if_gain_db(i);
            assert!(g >= last, "gain fell at index {i}");
            last = g;
        }
    }

    #[test]
    fn gpio_word_sets_the_vhf_bit_only() {
        assert_eq!(gpio_word(false), 0);
        assert_eq!(gpio_word(true), GPIO_VHF_EN);
    }

    #[test]
    fn samples_convert_little_endian_and_drop_odd_byte() {
        let mut out = Vec::new();
        le_samples(&[0x01, 0x00, 0xFF, 0xFF, 0x00, 0x80, 0x7F], &mut out);
        assert_eq!(out, vec![1, -1, i16::MIN]);
    }

    #[test]
    fn ddc_averages_and_mixes() {
        // DC input 0.5: the offset is zero, so the output is 0.5 on I and 0 on Q.
        let mut ddc = Ddc::new();
        ddc.configure(64_000_000, 2, 0.0, false);
        let mut out = Vec::new();
        ddc.process(&[16384, 16384, 16384, 16384], &mut out);
        assert_eq!(out.len(), 2);
        assert!((out[0].0 - 0.5).abs() < 1e-4 && out[0].1.abs() < 1e-4);

        // Offset of a quarter of the clock: the mixer turns 90 degrees per sample.
        let raw = [16384i16, 16384];
        let mut normal = Ddc::new();
        normal.configure(64_000_000, 2, 16_000_000.0, false);
        let mut a = Vec::new();
        normal.process(&raw, &mut a);
        let mut mirror = Ddc::new();
        mirror.configure(64_000_000, 2, 16_000_000.0, true);
        let mut b = Vec::new();
        mirror.process(&raw, &mut b);
        assert!((a[0].0 - 0.25).abs() < 1e-4 && (a[0].1 + 0.25).abs() < 1e-4, "normal: {:?}", a[0]);
        assert!((b[0].0 - 0.25).abs() < 1e-4 && (b[0].1 - 0.25).abs() < 1e-4, "mirror: {:?}", b[0]);
    }

    #[test]
    fn ddc_keeps_partial_blocks_between_calls() {
        let mut ddc = Ddc::new();
        ddc.configure(64_000_000, 4, 0.0, false);
        let mut out = Vec::new();
        ddc.process(&[1000, 1000, 1000], &mut out);
        assert!(out.is_empty());
        ddc.process(&[1000], &mut out);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn error_text_is_plain_and_covers_the_codes() {
        assert!(usb_error_text(-3).contains("access denied"));
        assert!(usb_error_text(-4).contains("not connected"));
        assert_eq!(usb_error_text(-42), "USB error -42");
        assert!(step_error("the radio did not answer", ERR_TIMEOUT).contains("did not answer in time"));
    }

    #[test]
    fn pick_prefers_running_radio_and_matches_ids() {
        let found = vec![
            Found { bus: 1, address: 4, product_id: PID_LOADER, serial: String::new() },
            Found { bus: 2, address: 7, product_id: PID_APP, serial: "ABC12345".into() },
        ];
        assert_eq!(pick_device(&found, None).unwrap().serial, "ABC12345");
        assert_eq!(pick_device(&found, Some("ABC12345")).unwrap().address, 7);
        assert_eq!(pick_device(&found, Some("1:4")).unwrap().product_id, PID_LOADER);
        assert!(pick_device(&found, Some("nope")).is_err());
        assert!(pick_device(&[], None).is_err());
    }
}
