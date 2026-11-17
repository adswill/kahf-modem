//! Receive driver for the PlutoSDR (an Analog Devices AD9361 radio) through libiio.
//!
//! libiio is loaded when the radio is listed or opened. Without it the list is
//! empty and open() says what to install. Only the receive side is here.

use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::io;
use std::ptr::null_mut;

use libloading::Library;

use crate::drivers::common::load_library;
use crate::{DeviceInfo, GainSetting, GainStage, RxRadio};

const LIB_NAMES: &[&str] = if cfg!(target_os = "macos") {
    &["libiio.dylib", "/Library/Frameworks/iio.framework/iio"]
} else if cfg!(windows) {
    &["libiio.dll"]
} else {
    &["libiio.so.0", "libiio.so"]
};

/// The lowest rate the converter runs without the optional decimating filter.
const RATE_MIN_HZ: u32 = 2_100_000;
const RATE_MAX_HZ: u32 = 61_440_000;
const RATE_STEPS_HZ: &[u32] = &[
    2_100_000,
    2_400_000,
    3_000_000,
    4_000_000,
    5_000_000,
    8_000_000,
    10_000_000,
    12_000_000,
    16_000_000,
    20_000_000,
    30_000_000,
    40_000_000,
    50_000_000,
    RATE_MAX_HZ,
];
const DEFAULT_RATE_HZ: u32 = 10_000_000;
/// A radio on USB 2 keeps up with about 4 Msps without losing samples.
const USB_CABLE_CAP_HZ: u32 = 4_000_000;
/// The default network address of the radio runs over the same USB cable: about 3 Msps.
const DEFAULT_NET_CAP_HZ: u32 = 3_000_000;
const DEFAULT_ADDRESSES: [&str; 2] = ["ip:192.168.2.1", "ip:pluto.local"];
/// The tuning range when the radio does not report its own.
const DEFAULT_FREQ_RANGE_HZ: (u64, u64) = (70_000_000, 6_000_000_000);
const DEFAULT_FREQ_HZ: u64 = 100_000_000;
const GAIN_STAGE: &str = "hardwaregain";
/// Longest wait of one read, in milliseconds.
const READ_TIMEOUT_MS: c_uint = 150;
/// Samples per buffer: about 16 ms at 2 Msps.
const BUFFER_SAMPLES: usize = 32_768;
const SCAN_LIMIT: usize = 32;

type Ptr = *mut c_void;

/// The libiio functions this driver uses, read from the loaded library.
struct Iio {
    _lib: Library,
    create_context_from_uri: unsafe extern "C" fn(*const c_char) -> Ptr,
    context_destroy: unsafe extern "C" fn(Ptr),
    context_set_timeout: unsafe extern "C" fn(Ptr, c_uint) -> c_int,
    context_find_device: unsafe extern "C" fn(Ptr, *const c_char) -> Ptr,
    device_find_channel: unsafe extern "C" fn(Ptr, *const c_char, bool) -> Ptr,
    channel_attr_read: unsafe extern "C" fn(Ptr, *const c_char, *mut c_char, usize) -> isize,
    channel_attr_write: unsafe extern "C" fn(Ptr, *const c_char, *const c_char) -> isize,
    channel_attr_write_longlong: unsafe extern "C" fn(Ptr, *const c_char, i64) -> c_int,
    channel_enable: unsafe extern "C" fn(Ptr),
    channel_disable: unsafe extern "C" fn(Ptr),
    device_create_buffer: unsafe extern "C" fn(Ptr, usize, bool) -> Ptr,
    buffer_destroy: unsafe extern "C" fn(Ptr),
    buffer_refill: unsafe extern "C" fn(Ptr) -> isize,
    buffer_cancel: unsafe extern "C" fn(Ptr),
    buffer_start: unsafe extern "C" fn(Ptr) -> Ptr,
    buffer_end: unsafe extern "C" fn(Ptr) -> Ptr,
    create_scan_context: unsafe extern "C" fn(*const c_char, c_uint) -> Ptr,
    scan_context_destroy: unsafe extern "C" fn(Ptr),
    scan_context_get_info_list: unsafe extern "C" fn(Ptr, *mut *mut Ptr) -> isize,
    context_info_list_free: unsafe extern "C" fn(*mut Ptr),
    context_info_get_description: unsafe extern "C" fn(Ptr) -> *const c_char,
    context_info_get_uri: unsafe extern "C" fn(Ptr) -> *const c_char,
}

/// Reads one symbol. The caller gives the function type it was built for.
fn sym<T: Copy>(lib: &Library, name: &[u8]) -> Result<T, String> {
    // SAFETY: the type T is the C signature of the named libiio function.
    match unsafe { lib.get::<T>(name) } {
        Ok(s) => Ok(*s),
        Err(e) => Err(format!(
            "PlutoSDR: libiio lacks {} ({e})",
            String::from_utf8_lossy(name)
        )),
    }
}

impl Iio {
    fn bind(lib: Library) -> Result<Iio, String> {
        // SAFETY: the signature matches libiio's iio_library_get_version.
        let version = unsafe {
            lib.get::<unsafe extern "C" fn(*mut c_uint, *mut c_uint, *mut c_char) -> c_int>(
                b"iio_library_get_version",
            )
            .ok()
            .map(|s| *s)
        };
        if let Some(get_version) = version {
            let (mut major, mut minor) = (0u32, 0u32);
            let mut tag = [0 as c_char; 8];
            // SAFETY: the pointers are to local variables of the right size.
            unsafe { get_version(&mut major, &mut minor, tag.as_mut_ptr()) };
            if major >= 1 {
                return Err("PlutoSDR: libiio 1.x is not supported yet; install libiio 0.x".into());
            }
        }
        Ok(Iio {
            create_context_from_uri: sym(&lib, b"iio_create_context_from_uri")?,
            context_destroy: sym(&lib, b"iio_context_destroy")?,
            context_set_timeout: sym(&lib, b"iio_context_set_timeout")?,
            context_find_device: sym(&lib, b"iio_context_find_device")?,
            device_find_channel: sym(&lib, b"iio_device_find_channel")?,
            channel_attr_read: sym(&lib, b"iio_channel_attr_read")?,
            channel_attr_write: sym(&lib, b"iio_channel_attr_write")?,
            channel_attr_write_longlong: sym(&lib, b"iio_channel_attr_write_longlong")?,
            channel_enable: sym(&lib, b"iio_channel_enable")?,
            channel_disable: sym(&lib, b"iio_channel_disable")?,
            device_create_buffer: sym(&lib, b"iio_device_create_buffer")?,
            buffer_destroy: sym(&lib, b"iio_buffer_destroy")?,
            buffer_refill: sym(&lib, b"iio_buffer_refill")?,
            buffer_cancel: sym(&lib, b"iio_buffer_cancel")?,
            buffer_start: sym(&lib, b"iio_buffer_start")?,
            buffer_end: sym(&lib, b"iio_buffer_end")?,
            create_scan_context: sym(&lib, b"iio_create_scan_context")?,
            scan_context_destroy: sym(&lib, b"iio_scan_context_destroy")?,
            scan_context_get_info_list: sym(&lib, b"iio_scan_context_get_info_list")?,
            context_info_list_free: sym(&lib, b"iio_context_info_list_free")?,
            context_info_get_description: sym(&lib, b"iio_context_info_get_description")?,
            context_info_get_uri: sym(&lib, b"iio_context_info_get_uri")?,
            _lib: lib,
        })
    }
}

/// Loads libiio from the standard names for this system.
fn load_iio() -> Result<Iio, String> {
    load_from(LIB_NAMES)
}

fn load_from(names: &[&str]) -> Result<Iio, String> {
    let lib = load_library(names).map_err(|e| {
        format!("PlutoSDR: libiio is not installed ({e}). Install libiio 0.x (on macOS, the Analog Devices package), then try again.")
    })?;
    Iio::bind(lib)
}

/// The highest rate the link to this address carries without losing samples. None: no limit set here.
fn link_cap_hz(uri: &str) -> Option<u32> {
    if uri.starts_with("usb:") {
        Some(USB_CABLE_CAP_HZ)
    } else if DEFAULT_ADDRESSES.contains(&uri) {
        Some(DEFAULT_NET_CAP_HZ)
    } else {
        None
    }
}

/// The rates offered under a link cap, in ascending order.
fn offered_rates(cap: Option<u32>) -> Vec<u32> {
    let top = cap.unwrap_or(u32::MAX);
    RATE_STEPS_HZ.iter().copied().filter(|&r| r <= top).collect()
}

/// The offered rate closest to `wanted`.
fn pick_rate(wanted: u32, cap: Option<u32>) -> u32 {
    offered_rates(cap)
        .into_iter()
        .min_by_key(|&r| (i64::from(r) - i64::from(wanted)).abs())
        .unwrap_or(RATE_MIN_HZ)
}

/// The RF bandwidth set for a rate: 95 percent of it, kept between 0.2 and 56 MHz.
fn bandwidth_for_rate(rate: u32) -> u32 {
    ((f64::from(rate) * 0.95).round() as u32).clamp(200_000, 56_000_000)
}

/// Highest RF gain in dB at a frequency (the radio's gain tables).
fn max_gain_db(freq_hz: u64) -> f32 {
    if freq_hz < 1_300_000_000 {
        73.0
    } else if freq_hz < 4_000_000_000 {
        71.0
    } else {
        62.0
    }
}

/// The gain in whole dB, kept between 0 and the highest gain at that frequency.
fn clamp_gain_db(db: f32, freq_hz: u64) -> f32 {
    db.clamp(0.0, max_gain_db(freq_hz)).round()
}

/// "[70000000 1 6000000000]" (first, step, last) -> (first, last).
fn parse_range(text: &str) -> Option<(u64, u64)> {
    let inner = text.trim().strip_prefix('[')?.strip_suffix(']')?;
    let parts = inner
        .split_whitespace()
        .map(|p| p.parse::<u64>().ok())
        .collect::<Option<Vec<u64>>>()?;
    if parts.len() != 3 || parts[2] <= parts[0] {
        return None;
    }
    Some((parts[0], parts[2]))
}

/// One 12-bit sample held in 16 bits, scaled to -1..1.
fn sample_f32(raw: i16) -> f32 {
    f32::from(raw) / 2048.0
}

/// Interleaved 16-bit little-endian I and Q pairs (4 bytes per sample) to complex samples.
/// A partial sample at the end is left out.
fn convert_bytes(bytes: &[u8], out: &mut Vec<(f32, f32)>) {
    out.extend(bytes.as_chunks::<4>().0.iter().map(|c| {
        (
            sample_f32(i16::from_le_bytes([c[0], c[1]])),
            sample_f32(i16::from_le_bytes([c[2], c[3]])),
        )
    }));
}

/// "0456:b673 (Analog Devices Inc. PlutoSDR (ADALM-PLUTO)), serial=104473" -> "104473"
fn serial_from_description(desc: &str) -> String {
    let Some(pos) = desc.find("serial=") else {
        return String::new();
    };
    desc[pos + 7..]
        .chars()
        .take_while(|c| !matches!(c, ' ' | ',' | ')'))
        .collect()
}

fn is_pluto_description(desc: &str) -> bool {
    ["PlutoSDR", "ADALM-PLUTO", "AD9361"]
        .iter()
        .any(|k| desc.contains(k))
}

/// The address to open. `found` is (address, serial) of the USB radios present.
fn pick_uri(id: Option<&str>, found: &[(String, String)]) -> Result<String, String> {
    match id {
        None => found.first().map(|(uri, _)| uri.clone()).ok_or_else(|| {
            "PlutoSDR: no radio found on USB. Connect it, or name its address (plutosdr:ip:ADDRESS)"
                .to_string()
        }),
        Some(s) if s.starts_with("usb:") || s.starts_with("ip:") => Ok(s.to_string()),
        Some(serial) => found
            .iter()
            .find(|(_, sn)| sn == serial)
            .map(|(uri, _)| uri.clone())
            .ok_or_else(|| format!("PlutoSDR: no radio with serial {serial} found on USB")),
    }
}

fn label_for(uri: &str, serial: &str) -> String {
    if serial.is_empty() {
        format!("PlutoSDR at {uri}")
    } else {
        format!("PlutoSDR serial {serial}")
    }
}

/// Text of a negative libiio error code (a negated errno value).
fn os_text(code: i64) -> String {
    io::Error::from_raw_os_error(code.unsigned_abs() as i32).to_string()
}

fn cstring(s: &str) -> Result<CString, String> {
    CString::new(s).map_err(|_| format!("PlutoSDR: {s:?} contains a zero byte"))
}

/// Text of a C string the library owns. Empty for a null pointer.
fn c_text(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: libiio returns NUL-terminated strings that live as long as the scan result.
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

struct Found {
    uri: String,
    serial: String,
}

/// The PlutoSDR radios on USB, from a scan.
fn scan_usb(iio: &Iio) -> Vec<Found> {
    let mut out = Vec::new();
    // SAFETY: each pointer comes from the library and is used only while the scan exists.
    unsafe {
        let sc = (iio.create_scan_context)(c"usb".as_ptr(), 0);
        if sc.is_null() {
            return out;
        }
        let mut list: *mut Ptr = null_mut();
        let n = (iio.scan_context_get_info_list)(sc, &mut list);
        if n > 0 && !list.is_null() {
            for i in 0..(n as usize).min(SCAN_LIMIT) {
                let info = *list.add(i);
                if info.is_null() {
                    continue;
                }
                let desc = c_text((iio.context_info_get_description)(info));
                let uri = c_text((iio.context_info_get_uri)(info));
                if !uri.is_empty() && is_pluto_description(&desc) {
                    out.push(Found {
                        uri,
                        serial: serial_from_description(&desc),
                    });
                }
            }
        }
        if !list.is_null() {
            (iio.context_info_list_free)(list);
        }
        (iio.scan_context_destroy)(sc);
    }
    out
}

/// Lists the PlutoSDR radios on USB. Empty when libiio is missing.
pub fn probe() -> Vec<DeviceInfo> {
    let Ok(iio) = load_iio() else {
        return Vec::new();
    };
    scan_usb(&iio)
        .into_iter()
        .map(|f| DeviceInfo {
            driver: "plutosdr",
            label: label_for(&f.uri, &f.serial),
            id: if f.serial.is_empty() {
                f.uri.clone()
            } else {
                f.serial.clone()
            },
        })
        .collect()
}

/// Opens a radio by serial number, or by address ("usb:..." or "ip:..."). With no id, the first USB radio.
pub fn open(id: Option<&str>) -> Result<Box<dyn RxRadio>, String> {
    let iio = load_iio()?;
    let found: Vec<(String, String)> = scan_usb(&iio).into_iter().map(|f| (f.uri, f.serial)).collect();
    let uri = pick_uri(id, &found)?;
    let serial = found
        .iter()
        .find(|(u, _)| *u == uri)
        .map(|(_, s)| s.clone())
        .unwrap_or_default();
    let radio = Pluto::connect(iio, &uri, &serial)?;
    Ok(Box::new(radio))
}

/// An open PlutoSDR. The channels are found once; the stream is built by start().
pub struct Pluto {
    iio: Iio,
    ctx: Ptr,
    rx_dev: Ptr,
    rf: Ptr,
    lo: Ptr,
    i_ch: Ptr,
    q_ch: Ptr,
    buf: Ptr,
    label: String,
    cap: Option<u32>,
    rate: u32,
    freq_hz: u64,
    freq_range: (u64, u64),
    running: bool,
    pending: Vec<(f32, f32)>,
    pending_pos: usize,
}

// SAFETY: the library handles are used from one thread at a time, as the trait requires; libiio
// does not tie a context to the thread that made it.
unsafe impl Send for Pluto {}

fn find_device(iio: &Iio, ctx: Ptr, name: &str) -> Result<Ptr, String> {
    let c = cstring(name)?;
    // SAFETY: ctx is a live context; the name is a valid C string.
    let dev = unsafe { (iio.context_find_device)(ctx, c.as_ptr()) };
    if dev.is_null() {
        Err(format!(
            "PlutoSDR: device {name} not found; check the radio's firmware"
        ))
    } else {
        Ok(dev)
    }
}

fn find_channel(iio: &Iio, dev: Ptr, name: &str, output: bool) -> Result<Ptr, String> {
    let c = cstring(name)?;
    // SAFETY: dev is a live device; the name is a valid C string.
    let ch = unsafe { (iio.device_find_channel)(dev, c.as_ptr(), output) };
    if ch.is_null() {
        Err(format!(
            "PlutoSDR: channel {name} not found; check the radio's firmware"
        ))
    } else {
        Ok(ch)
    }
}

impl Pluto {
    fn connect(iio: Iio, uri: &str, serial: &str) -> Result<Pluto, String> {
        let c_uri = cstring(uri)?;
        // SAFETY: the address is a valid C string; the result is checked for null below.
        let ctx = unsafe { (iio.create_context_from_uri)(c_uri.as_ptr()) };
        if ctx.is_null() {
            let e = io::Error::last_os_error();
            return Err(format!("PlutoSDR: cannot connect ({uri}): {e}"));
        }
        // From here on the context belongs to the struct, so Drop releases it on every error path.
        let mut radio = Pluto {
            freq_range: DEFAULT_FREQ_RANGE_HZ,
            freq_hz: DEFAULT_FREQ_HZ,
            rate: 0,
            cap: link_cap_hz(uri),
            label: label_for(uri, serial),
            iio,
            ctx,
            rx_dev: null_mut(),
            rf: null_mut(),
            lo: null_mut(),
            i_ch: null_mut(),
            q_ch: null_mut(),
            buf: null_mut(),
            running: false,
            pending: Vec::new(),
            pending_pos: 0,
        };
        // SAFETY: ctx is live.
        if unsafe { (radio.iio.context_set_timeout)(ctx, READ_TIMEOUT_MS) } < 0 {
            return Err("PlutoSDR: the radio refused the read timeout".into());
        }
        let phy = find_device(&radio.iio, ctx, "ad9361-phy")?;
        radio.rx_dev = find_device(&radio.iio, ctx, "cf-ad9361-lpc")?;
        radio.rf = find_channel(&radio.iio, phy, "voltage0", false)?;
        radio.lo = find_channel(&radio.iio, phy, "altvoltage0", true)?;
        radio.i_ch = find_channel(&radio.iio, radio.rx_dev, "voltage0", false)?;
        radio.q_ch = find_channel(&radio.iio, radio.rx_dev, "voltage1", false)?;

        if let Some(range) = radio
            .read_attr(radio.lo, "frequency_available")
            .and_then(|t| parse_range(&t))
        {
            radio.freq_range = range;
        }
        if let Some(f) = radio
            .read_attr(radio.lo, "frequency")
            .and_then(|t| t.parse::<u64>().ok())
        {
            radio.freq_hz = f;
        }
        radio.write_str(radio.rf, "gain_control_mode", "manual")?;
        let rate = pick_rate(DEFAULT_RATE_HZ, radio.cap);
        radio.apply_rate(rate)?;
        Ok(radio)
    }

    fn read_attr(&self, chn: Ptr, attr: &str) -> Option<String> {
        let c = cstring(attr).ok()?;
        let mut buf = [0u8; 128];
        // SAFETY: the buffer is 128 bytes and the length passed is the buffer length.
        let n = unsafe { (self.iio.channel_attr_read)(chn, c.as_ptr(), buf.as_mut_ptr().cast(), buf.len()) };
        if n <= 0 {
            return None;
        }
        let n = (n as usize).min(buf.len());
        Some(
            String::from_utf8_lossy(&buf[..n])
                .trim_matches(|c: char| c == '\0' || c.is_whitespace())
                .to_string(),
        )
    }

    fn write_i64(&self, chn: Ptr, attr: &str, value: i64) -> Result<(), String> {
        let c = cstring(attr)?;
        // SAFETY: the channel is live and the attribute name is a valid C string.
        let r = unsafe { (self.iio.channel_attr_write_longlong)(chn, c.as_ptr(), value) };
        if r < 0 {
            return Err(format!(
                "PlutoSDR: the radio refused {attr} = {value} ({})",
                os_text(i64::from(r))
            ));
        }
        Ok(())
    }

    fn write_str(&self, chn: Ptr, attr: &str, value: &str) -> Result<(), String> {
        let c = cstring(attr)?;
        let v = cstring(value)?;
        // SAFETY: the channel is live and both strings are valid C strings.
        let r = unsafe { (self.iio.channel_attr_write)(chn, c.as_ptr(), v.as_ptr()) };
        if r < 0 {
            return Err(format!(
                "PlutoSDR: the radio refused {attr} = {value} ({})",
                os_text(r as i64)
            ));
        }
        Ok(())
    }

    /// Sets the converter rate and the RF bandwidth that goes with it.
    fn apply_rate(&mut self, rate: u32) -> Result<(), String> {
        self.write_i64(self.rf, "sampling_frequency", i64::from(rate))?;
        self.write_i64(self.rf, "rf_bandwidth", i64::from(bandwidth_for_rate(rate)))?;
        self.rate = rate;
        Ok(())
    }

    /// Reads the buffer once. A timeout is not an error: the buffer is left empty.
    fn refill(&mut self) -> Result<(), String> {
        self.pending.clear();
        self.pending_pos = 0;
        // SAFETY: the buffer was created by start() and is live while running is set.
        let r = unsafe { (self.iio.buffer_refill)(self.buf) };
        if r < 0 {
            let kind = io::Error::from_raw_os_error(r.unsigned_abs() as i32).kind();
            if matches!(kind, io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) {
                return Ok(());
            }
            return Err(format!("PlutoSDR: the stream stopped ({})", os_text(r as i64)));
        }
        // SAFETY: start and end bound the buffer; only the first r bytes (at most the buffer size) are read.
        let (start, end) = unsafe {
            (
                (self.iio.buffer_start)(self.buf) as *const u8,
                (self.iio.buffer_end)(self.buf) as *const u8,
            )
        };
        let size = (end as usize).saturating_sub(start as usize);
        let n = (r as usize).min(size);
        // SAFETY: the n bytes lie inside the buffer, which libiio keeps alive until the next refill.
        let bytes = unsafe { std::slice::from_raw_parts(start, n) };
        convert_bytes(bytes, &mut self.pending);
        Ok(())
    }
}

impl RxRadio for Pluto {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn freq_range_hz(&self) -> (u64, u64) {
        self.freq_range
    }

    fn set_frequency_hz(&mut self, hz: u64) -> Result<(), String> {
        let (lo, hi) = self.freq_range;
        if hz < lo || hz > hi {
            return Err(format!(
                "PlutoSDR: {:.3} MHz is outside the radio's range of {:.3} to {:.3} MHz",
                hz as f64 / 1e6,
                lo as f64 / 1e6,
                hi as f64 / 1e6
            ));
        }
        self.write_i64(self.lo, "frequency", hz as i64)?;
        self.freq_hz = hz;
        Ok(())
    }

    fn sample_rates(&self) -> Vec<u32> {
        offered_rates(self.cap)
    }

    fn set_sample_rate(&mut self, hz: u32) -> Result<u32, String> {
        if self.running {
            return Err("PlutoSDR: stop the stream before changing the sample rate".into());
        }
        let rate = pick_rate(hz, self.cap);
        self.apply_rate(rate)?;
        Ok(self.rate)
    }

    fn gain_stages(&self) -> Vec<GainStage> {
        vec![GainStage {
            name: GAIN_STAGE.into(),
            min_db: 0.0,
            max_db: max_gain_db(self.freq_hz),
            step_db: 1.0,
            auto: true,
        }]
    }

    fn set_gain(&mut self, stage: &str, setting: GainSetting) -> Result<(), String> {
        if stage != GAIN_STAGE {
            return Err(format!("PlutoSDR has no gain stage {stage:?}"));
        }
        match setting {
            GainSetting::Auto => self.write_str(self.rf, "gain_control_mode", "slow_attack"),
            GainSetting::Manual(db) => {
                if !db.is_finite() {
                    return Err("PlutoSDR: the gain must be a number of dB".into());
                }
                let g = clamp_gain_db(db, self.freq_hz);
                self.write_str(self.rf, "gain_control_mode", "manual")?;
                self.write_i64(self.rf, GAIN_STAGE, g as i64)
            }
        }
    }

    fn start(&mut self) -> Result<(), String> {
        if self.running {
            return Ok(());
        }
        // SAFETY: the channels and the receive device were found at open and are live.
        unsafe {
            (self.iio.channel_enable)(self.i_ch);
            (self.iio.channel_enable)(self.q_ch);
            self.buf = (self.iio.device_create_buffer)(self.rx_dev, BUFFER_SAMPLES, false);
            if self.buf.is_null() {
                (self.iio.channel_disable)(self.i_ch);
                (self.iio.channel_disable)(self.q_ch);
                return Err(
                    "PlutoSDR: cannot create the sample buffer (is another program using the radio?)".into(),
                );
            }
        }
        self.pending.clear();
        self.pending_pos = 0;
        self.running = true;
        Ok(())
    }

    fn read_iq(&mut self, out: &mut [(f32, f32)]) -> Result<usize, String> {
        if out.is_empty() {
            return Ok(0);
        }
        if self.pending_pos >= self.pending.len() {
            if !self.running {
                return Err("PlutoSDR: the stream is not running".into());
            }
            self.refill()?;
        }
        let avail = &self.pending[self.pending_pos..];
        let n = avail.len().min(out.len());
        out[..n].copy_from_slice(&avail[..n]);
        self.pending_pos += n;
        Ok(n)
    }

    fn stop(&mut self) {
        if !self.buf.is_null() {
            // SAFETY: the buffer was created by start() and is destroyed only once.
            unsafe {
                (self.iio.buffer_cancel)(self.buf);
                (self.iio.buffer_destroy)(self.buf);
            }
            self.buf = null_mut();
        }
        if self.running {
            // SAFETY: the channels were enabled by start().
            unsafe {
                (self.iio.channel_disable)(self.i_ch);
                (self.iio.channel_disable)(self.q_ch);
            }
        }
        self.running = false;
        self.pending.clear();
        self.pending_pos = 0;
    }
}

impl Drop for Pluto {
    fn drop(&mut self) {
        self.stop();
        if !self.ctx.is_null() {
            // SAFETY: the context was created by connect and is destroyed only here.
            unsafe { (self.iio.context_destroy)(self.ctx) };
            self.ctx = null_mut();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_choice_follows_the_offered_list() {
        assert_eq!(pick_rate(1_000_000, None), RATE_MIN_HZ);
        assert_eq!(pick_rate(10_200_000, None), 10_000_000);
        assert_eq!(pick_rate(100_000_000, None), RATE_MAX_HZ);
        assert_eq!(pick_rate(10_000_000, Some(USB_CABLE_CAP_HZ)), 4_000_000);
        assert_eq!(pick_rate(10_000_000, Some(DEFAULT_NET_CAP_HZ)), 3_000_000);
        let all = offered_rates(None);
        assert!(all.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(all.first(), Some(&RATE_MIN_HZ));
        assert_eq!(all.last(), Some(&RATE_MAX_HZ));
    }

    #[test]
    fn link_caps_by_address() {
        assert_eq!(link_cap_hz("usb:1.5.5"), Some(4_000_000));
        assert_eq!(link_cap_hz("ip:192.168.2.1"), Some(3_000_000));
        assert_eq!(link_cap_hz("ip:pluto.local"), Some(3_000_000));
        assert_eq!(link_cap_hz("ip:10.0.0.4"), None);
    }

    #[test]
    fn bandwidth_is_95_percent_within_limits() {
        assert_eq!(bandwidth_for_rate(10_000_000), 9_500_000);
        assert_eq!(bandwidth_for_rate(100_000), 200_000);
        assert_eq!(bandwidth_for_rate(RATE_MAX_HZ), 56_000_000);
    }

    #[test]
    fn gain_is_clamped_by_frequency() {
        assert_eq!(max_gain_db(900_000_000), 73.0);
        assert_eq!(max_gain_db(1_300_000_000), 71.0);
        assert_eq!(max_gain_db(5_000_000_000), 62.0);
        assert_eq!(clamp_gain_db(-3.0, 900_000_000), 0.0);
        assert_eq!(clamp_gain_db(40.4, 900_000_000), 40.0);
        assert_eq!(clamp_gain_db(80.0, 900_000_000), 73.0);
        assert_eq!(clamp_gain_db(80.0, 2_000_000_000), 71.0);
        assert_eq!(clamp_gain_db(80.0, 5_000_000_000), 62.0);
    }

    #[test]
    fn samples_convert_to_unit_range() {
        let mut out = Vec::new();
        convert_bytes(&[0x00, 0x08, 0x00, 0xF8], &mut out);
        assert_eq!(out, vec![(1.0, -1.0)]);

        out.clear();
        convert_bytes(&[0, 0, 0, 0, 0xFF, 0x07, 0, 0, 0xAA], &mut out);
        assert_eq!(out.len(), 2);
        assert!((out[1].0 - 2047.0 / 2048.0).abs() < 1e-6);
        assert_eq!(out[1].1, 0.0);
    }

    #[test]
    fn missing_library_says_what_to_install() {
        match load_from(&["/nonexistent/kahf-test/libiio-missing.so"]) {
            Ok(_) => panic!("a missing library must not load"),
            Err(e) => {
                assert!(e.contains("libiio is not installed"), "{e}");
                assert!(e.contains("libiio-missing.so"), "{e}");
            }
        }
    }

    #[test]
    fn frequency_range_text_is_read() {
        assert_eq!(
            parse_range("[70000000 1 6000000000]"),
            Some((70_000_000, 6_000_000_000))
        );
        assert_eq!(parse_range("[3 1 5]"), Some((3, 5)));
        assert_eq!(parse_range("[5 1 3]"), None);
        assert_eq!(parse_range("[1 1 1]"), None);
        assert_eq!(parse_range("garbage"), None);
    }

    #[test]
    fn serial_is_taken_from_the_description() {
        assert_eq!(
            serial_from_description(
                "0456:b673 (Analog Devices Inc. PlutoSDR (ADALM-PLUTO)), serial=104473abc"
            ),
            "104473abc"
        );
        assert_eq!(serial_from_description("PlutoSDR"), "");
    }

    #[test]
    fn address_is_picked_for_each_kind_of_id() {
        let found = vec![("usb:1.5.5".to_string(), "104473".to_string())];
        assert_eq!(pick_uri(None, &found).unwrap(), "usb:1.5.5");
        assert!(pick_uri(None, &[]).is_err());
        assert_eq!(pick_uri(Some("usb:1.2.3"), &[]).unwrap(), "usb:1.2.3");
        assert_eq!(pick_uri(Some("ip:192.168.2.1"), &[]).unwrap(), "ip:192.168.2.1");
        assert_eq!(pick_uri(Some("104473"), &found).unwrap(), "usb:1.5.5");
        assert!(pick_uri(Some("999"), &found).is_err());
    }
}
