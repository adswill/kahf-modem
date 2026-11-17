//! Driver for the Airspy HF+ and HF+ Discovery. The vendor library is loaded at run time.
//!
//! The radio streams 32-bit float I/Q pairs through a callback. The callback
//! puts them in a queue; `read_iq` takes them out and waits at most 200 ms.

use std::collections::VecDeque;
use std::ffi::{c_int, c_void};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use libloading::Library;

use crate::drivers::common::load_library;
use crate::{DeviceInfo, GainSetting, GainStage, RxRadio};

#[cfg(target_os = "macos")]
const LIB_NAMES: &[&str] = &["libairspyhf.dylib"];
#[cfg(target_os = "windows")]
const LIB_NAMES: &[&str] = &["airspyhf.dll"];
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const LIB_NAMES: &[&str] = &["libairspyhf.so.0", "libairspyhf.so"];

const LABEL: &str = "Airspy HF+";
const MIN_FREQ_HZ: u64 = 9_000;
const MAX_FREQ_HZ: u64 = 260_000_000;
/// Attenuator range in dB. The top of the gain slider means no attenuation.
const GAIN_MAX_DB: f32 = 48.0;
const GAIN_STEP_DB: f32 = 6.0;
/// The preamp adds this much gain when it is on.
const PREAMP_DB: f32 = 6.0;
/// A rate fits when it is at least this share of the wanted rate (absorbs rounding).
const RATE_FIT: f64 = 0.999;
/// Most samples kept while the reader is late. Older ones are dropped and counted.
const MAX_QUEUED: usize = 1 << 20;
const READ_WAIT: Duration = Duration::from_millis(200);
const MAX_DEVICES: usize = 16;
const MAX_RATES: usize = 16;

type Device = *mut c_void;

/// The callback block the library fills in for each buffer of samples.
#[repr(C)]
struct Transfer {
    device: Device,
    ctx: *mut c_void,
    samples: *const [f32; 2],
    sample_count: c_int,
    dropped_samples: u64,
}

type SampleCallback = unsafe extern "C" fn(*mut Transfer) -> c_int;
type ListDevices = unsafe extern "C" fn(*mut u64, c_int) -> c_int;
type OpenSerial = unsafe extern "C" fn(*mut Device, u64) -> c_int;
type CloseDevice = unsafe extern "C" fn(Device) -> c_int;
type GetRates = unsafe extern "C" fn(Device, *mut u32, u32) -> c_int;
type SetU32 = unsafe extern "C" fn(Device, u32) -> c_int;
type SetU8 = unsafe extern "C" fn(Device, u8) -> c_int;
type StartStream = unsafe extern "C" fn(Device, SampleCallback, *mut c_void) -> c_int;
type DeviceOnly = unsafe extern "C" fn(Device) -> c_int;

/// Function pointers from the library. They are valid while the `Library` they came from is loaded.
struct Api {
    list_devices: ListDevices,
    open_serial: OpenSerial,
    close: CloseDevice,
    get_rates: GetRates,
    set_rate: SetU32,
    set_freq: SetU32,
    set_agc: SetU8,
    set_att: SetU8,
    start: StartStream,
    stop: DeviceOnly,
    is_streaming: DeviceOnly,
    /// Optional: not in the oldest libraries.
    set_lna: Option<SetU8>,
    /// Optional: turns on the library's own correction and fine tuning.
    set_lib_dsp: Option<SetU8>,
}

fn required<T: Copy>(lib: &Library, name: &'static [u8]) -> Result<T, String> {
    // SAFETY: the symbol is copied out. The driver keeps the library loaded while the copy is used.
    let found = unsafe { lib.get::<T>(name).map(|s| *s) };
    found.map_err(|e| {
        let shown = String::from_utf8_lossy(name).trim_end_matches('\0').to_string();
        format!("the library has no {shown} ({e})")
    })
}

fn optional<T: Copy>(lib: &Library, name: &'static [u8]) -> Option<T> {
    // SAFETY: as in `required`.
    unsafe { lib.get::<T>(name).ok().map(|s| *s) }
}

impl Api {
    fn load(lib: &Library) -> Result<Api, String> {
        Ok(Api {
            list_devices: required(lib, b"airspyhf_list_devices\0")?,
            open_serial: required(lib, b"airspyhf_open_sn\0")?,
            close: required(lib, b"airspyhf_close\0")?,
            get_rates: required(lib, b"airspyhf_get_samplerates\0")?,
            set_rate: required(lib, b"airspyhf_set_samplerate\0")?,
            set_freq: required(lib, b"airspyhf_set_freq\0")?,
            set_agc: required(lib, b"airspyhf_set_hf_agc\0")?,
            set_att: required(lib, b"airspyhf_set_hf_att\0")?,
            start: required(lib, b"airspyhf_start\0")?,
            stop: required(lib, b"airspyhf_stop\0")?,
            is_streaming: required(lib, b"airspyhf_is_streaming\0")?,
            set_lna: optional(lib, b"airspyhf_set_hf_lna\0"),
            set_lib_dsp: optional(lib, b"airspyhf_set_lib_dsp\0"),
        })
    }
}

/// Sample queue shared with the callback. The driver owns it for as long as the device is open.
struct Shared {
    queue: Mutex<VecDeque<[f32; 2]>>,
    ready: Condvar,
    running: AtomicBool,
    dropped: AtomicU64,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, VecDeque<[f32; 2]>> {
        self.queue.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn push(&self, raw: &[[f32; 2]]) {
        {
            let mut q = self.lock();
            for s in raw {
                if q.len() >= MAX_QUEUED {
                    q.pop_front();
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                }
                q.push_back([clean_sample(s[0]), clean_sample(s[1])]);
            }
        }
        self.ready.notify_all();
    }
}

/// Called by the library on its own thread for each buffer of samples.
unsafe extern "C" fn on_samples(t: *mut Transfer) -> c_int {
    // SAFETY: the library passes a valid transfer for the length of this call. The context is
    // the shared block, which the driver keeps alive until the device is closed.
    unsafe {
        let Some(t) = t.as_ref() else { return 0 };
        if t.ctx.is_null() || t.samples.is_null() || t.sample_count <= 0 {
            return 0;
        }
        let shared = &*(t.ctx as *const Shared);
        if !shared.running.load(Ordering::Acquire) {
            return 0;
        }
        let raw = std::slice::from_raw_parts(t.samples, t.sample_count as usize);
        shared.push(raw);
    }
    0
}

pub struct AirspyHf {
    shared: Arc<Shared>,
    api: Api,
    dev: Device,
    rates: Vec<u32>,
    rate: u32,
    freq: Option<u64>,
    streaming: bool,
    // Keep the library loaded for as long as the function pointers exist. Declared last.
    _lib: Library,
}

// SAFETY: the device handle is used from one thread at a time, as the trait requires.
// The callback only touches `Shared`, which is thread-safe.
unsafe impl Send for AirspyHf {}

impl Drop for AirspyHf {
    fn drop(&mut self) {
        if self.dev.is_null() {
            return;
        }
        self.stop();
        // SAFETY: the handle came from a successful open and is closed once.
        unsafe { (self.api.close)(self.dev) };
        self.dev = null_mut();
    }
}

fn check(code: c_int, what: &str) -> Result<(), String> {
    if code == 0 {
        Ok(())
    } else {
        Err(format!("Airspy HF+: {what} failed (library code {code})"))
    }
}

impl AirspyHf {
    fn set_radio_agc(&mut self, on: bool) -> Result<(), String> {
        // SAFETY: the device is open (checked by the caller's construction path).
        let code = unsafe { (self.api.set_agc)(self.dev, on as u8) };
        check(code, "setting the radio's own gain control")
    }
}

impl RxRadio for AirspyHf {
    fn label(&self) -> String {
        LABEL.to_string()
    }

    fn freq_range_hz(&self) -> (u64, u64) {
        (MIN_FREQ_HZ, MAX_FREQ_HZ)
    }

    fn set_frequency_hz(&mut self, hz: u64) -> Result<(), String> {
        if !(MIN_FREQ_HZ..=MAX_FREQ_HZ).contains(&hz) {
            return Err(format!("{hz} Hz is outside the Airspy HF+ range of 9 kHz to 260 MHz"));
        }
        // SAFETY: the device is open. The range check keeps the value within 32 bits.
        let code = unsafe { (self.api.set_freq)(self.dev, hz as u32) };
        check(code, "setting the frequency")?;
        self.freq = Some(hz);
        Ok(())
    }

    fn sample_rates(&self) -> Vec<u32> {
        self.rates.clone()
    }

    fn set_sample_rate(&mut self, hz: u32) -> Result<u32, String> {
        if self.streaming {
            return Err("stop the Airspy HF+ before changing the sample rate".into());
        }
        let rate = choose_rate(&self.rates, hz).ok_or("the radio reports no sample rates")?;
        // SAFETY: the device is open.
        let code = unsafe { (self.api.set_rate)(self.dev, rate) };
        check(code, &format!("setting the sample rate to {rate} Hz"))?;
        self.rate = rate;
        Ok(rate)
    }

    fn gain_stages(&self) -> Vec<GainStage> {
        let mut out = vec![GainStage {
            name: "gain".into(),
            min_db: 0.0,
            max_db: GAIN_MAX_DB,
            step_db: GAIN_STEP_DB,
            auto: true,
        }];
        if self.api.set_lna.is_some() {
            out.push(GainStage {
                name: "preamp".into(),
                min_db: 0.0,
                max_db: PREAMP_DB,
                step_db: PREAMP_DB,
                auto: false,
            });
        }
        out
    }

    fn set_gain(&mut self, stage: &str, setting: GainSetting) -> Result<(), String> {
        match stage {
            "gain" => match setting {
                // The radio's own control: the attenuator is left as it is.
                GainSetting::Auto => self.set_radio_agc(true),
                // A fixed value: the radio's control goes off and the attenuator is set.
                GainSetting::Manual(db) => {
                    self.set_radio_agc(false)?;
                    let step = attenuation_step(db);
                    // SAFETY: the device is open.
                    let code = unsafe { (self.api.set_att)(self.dev, step) };
                    check(code, "setting the attenuator")
                }
            },
            "preamp" => {
                let Some(set_lna) = self.api.set_lna else {
                    return Err("this Airspy HF+ library has no preamp control".into());
                };
                match setting {
                    GainSetting::Auto => Err("the preamp has no automatic setting; give it a value".into()),
                    GainSetting::Manual(db) => {
                        // SAFETY: the device is open.
                        let code = unsafe { set_lna(self.dev, preamp_on(db) as u8) };
                        check(code, "setting the preamp")
                    }
                }
            }
            other => Err(format!("the Airspy HF+ has no gain stage named {other:?}; stages: gain, preamp")),
        }
    }

    fn start(&mut self) -> Result<(), String> {
        if self.streaming {
            return Ok(());
        }
        if self.freq.is_none() {
            return Err("set a frequency before starting the Airspy HF+".into());
        }
        self.shared.lock().clear();
        self.shared.running.store(true, Ordering::Release);
        let ctx = Arc::as_ptr(&self.shared) as *mut c_void;
        // SAFETY: the device is open. `ctx` stays valid until the device is closed in `drop`.
        let code = unsafe { (self.api.start)(self.dev, on_samples, ctx) };
        if code != 0 {
            self.shared.running.store(false, Ordering::Release);
            return Err(format!("Airspy HF+: starting the stream failed (library code {code})"));
        }
        self.streaming = true;
        Ok(())
    }

    fn read_iq(&mut self, out: &mut [(f32, f32)]) -> Result<usize, String> {
        if !self.streaming {
            return Err("the Airspy HF+ is not streaming; start it first".into());
        }
        if out.is_empty() {
            return Ok(0);
        }
        let guard = self.shared.lock();
        let (mut q, _) = self
            .shared
            .ready
            .wait_timeout_while(guard, READ_WAIT, |q| q.is_empty())
            .unwrap_or_else(|p| p.into_inner());
        let n = out.len().min(q.len());
        for (slot, s) in out.iter_mut().zip(q.drain(..n)) {
            *slot = (s[0], s[1]);
        }
        drop(q);
        // SAFETY: the device is open.
        if n == 0 && unsafe { (self.api.is_streaming)(self.dev) } <= 0 {
            return Err("the Airspy HF+ stopped streaming; check the USB cable and try again".into());
        }
        Ok(n)
    }

    fn stop(&mut self) {
        if !self.streaming {
            return;
        }
        // The callback drops samples from here on, even while the library winds down.
        self.shared.running.store(false, Ordering::Release);
        // SAFETY: the device is open.
        unsafe { (self.api.stop)(self.dev) };
        self.streaming = false;
    }

    fn dropped_samples(&self) -> u64 {
        self.shared.dropped.load(Ordering::Relaxed)
    }
}

/// The error for a missing vendor library. `detail` is what the loader tried.
pub fn missing_library_message(detail: &str) -> String {
    format!(
        "Airspy HF+ support needs the vendor library libairspyhf, which was not found ({detail}). \
         Install libairspyhf, then start again."
    )
}

fn no_device_message() -> String {
    "no Airspy HF+ found; connect it with a USB cable and try again".into()
}

/// Reads a serial number written in hexadecimal, with or without a 0x prefix.
pub fn parse_serial(text: &str) -> Result<u64, String> {
    let t = text.trim();
    let hex = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")).unwrap_or(t);
    u64::from_str_radix(hex, 16).map_err(|_| {
        format!("{text:?} is not an Airspy HF+ serial number; give the hexadecimal serial from the device list")
    })
}

/// The smallest rate that is at least `wanted`. The fastest rate when none is fast enough.
/// Returns None when the list is empty.
pub fn choose_rate(rates: &[u32], wanted: u32) -> Option<u32> {
    let top = rates.iter().copied().max()?;
    let wanted_fit = wanted as f64 * RATE_FIT;
    let fit = rates.iter().copied().filter(|&r| r as f64 >= wanted_fit).min();
    Some(fit.unwrap_or(top))
}

fn clamp_db(value: f32, max: f32) -> f32 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, max)
    }
}

/// A gain value kept within 0 to 48 dB. NaN counts as 0 dB.
pub fn clamp_gain_db(value: f32) -> f32 {
    clamp_db(value, GAIN_MAX_DB)
}

/// The attenuator step for a gain value. 0 is no attenuation, 8 is 48 dB.
pub fn attenuation_step(gain_db: f32) -> u8 {
    let gain = clamp_gain_db(gain_db);
    ((GAIN_MAX_DB - gain) / GAIN_STEP_DB).round() as u8
}

/// The preamp is on for a value in the upper half of its range.
pub fn preamp_on(value_db: f32) -> bool {
    clamp_db(value_db, PREAMP_DB) >= PREAMP_DB / 2.0
}

/// A sample kept within -1 to 1. Values that are not finite become 0.
pub fn clean_sample(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

fn list_serials(api: &Api) -> Vec<u64> {
    let mut buf = [0u64; MAX_DEVICES];
    // SAFETY: the buffer holds MAX_DEVICES entries, and the count passed matches it.
    let n = unsafe { (api.list_devices)(buf.as_mut_ptr(), MAX_DEVICES as c_int) };
    if n <= 0 {
        return Vec::new();
    }
    buf[..(n as usize).min(MAX_DEVICES)].to_vec()
}

fn read_rates(api: &Api, dev: Device) -> Result<Vec<u32>, String> {
    // A zero-length call first, which returns how many rates there are.
    let mut count: u32 = 0;
    // SAFETY: the device is open. A zero length means the count is written and nothing else.
    check(unsafe { (api.get_rates)(dev, &mut count, 0) }, "reading the sample rates")?;
    let len = (count as usize).min(MAX_RATES);
    let mut rates = vec![0u32; len];
    if len > 0 {
        // SAFETY: the buffer holds `len` entries.
        check(unsafe { (api.get_rates)(dev, rates.as_mut_ptr(), len as u32) }, "reading the sample rates")?;
    }
    rates.retain(|&r| r > 0);
    rates.sort_unstable();
    rates.dedup();
    if rates.is_empty() {
        return Err("Airspy HF+: the radio reports no sample rates".into());
    }
    Ok(rates)
}

/// Lists the radios the library can see. Returns nothing when the library is missing.
pub fn probe() -> Vec<DeviceInfo> {
    let Ok(lib) = load_library(LIB_NAMES) else {
        return Vec::new();
    };
    let Ok(api) = Api::load(&lib) else {
        return Vec::new();
    };
    list_serials(&api)
        .into_iter()
        .map(|serial| DeviceInfo {
            driver: "airspyhf",
            label: LABEL.to_string(),
            id: format!("{serial:016X}"),
        })
        .collect()
}

/// Opens the radio with serial `id` (hexadecimal), or the first one found when `id` is None.
pub fn open(id: Option<&str>) -> Result<Box<dyn RxRadio>, String> {
    let lib = load_library(LIB_NAMES).map_err(|e| missing_library_message(&e))?;
    let api = Api::load(&lib).map_err(|e| missing_library_message(&e))?;
    let serial = match id {
        Some(text) => parse_serial(text)?,
        None => *list_serials(&api).first().ok_or_else(no_device_message)?,
    };

    let mut dev: Device = null_mut();
    // SAFETY: `dev` is a valid out pointer.
    let code = unsafe { (api.open_serial)(&mut dev, serial) };
    if code != 0 || dev.is_null() {
        return Err(format!(
            "Airspy HF+ with serial {serial:016X} cannot be opened (not connected, or in use by another program)"
        ));
    }

    // From here on the struct owns the handle, so a failure below closes it in `drop`.
    let mut radio = AirspyHf {
        shared: Arc::new(Shared {
            queue: Mutex::new(VecDeque::new()),
            ready: Condvar::new(),
            running: AtomicBool::new(false),
            dropped: AtomicU64::new(0),
        }),
        api,
        dev,
        rates: Vec::new(),
        rate: 0,
        freq: None,
        streaming: false,
        _lib: lib,
    };
    if let Some(set_dsp) = radio.api.set_lib_dsp {
        // SAFETY: the device is open.
        unsafe { set_dsp(radio.dev, 1) };
    }
    radio.rates = read_rates(&radio.api, radio.dev)?;
    let top = *radio.rates.last().ok_or("the radio reports no sample rates")?;
    radio.set_sample_rate(top)?;
    Ok(Box::new(radio))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATES: &[u32] = &[192_000, 256_000, 384_000, 456_000, 768_000, 912_000];

    #[test]
    fn rate_choice_takes_the_smallest_rate_that_fits() {
        assert_eq!(choose_rate(RATES, 250_000), Some(256_000));
        assert_eq!(choose_rate(RATES, 256_000), Some(256_000));
        assert_eq!(choose_rate(RATES, 912_000), Some(912_000));
        assert_eq!(choose_rate(RATES, 0), Some(192_000));
    }

    #[test]
    fn rate_choice_falls_back_to_the_fastest_rate() {
        assert_eq!(choose_rate(RATES, 2_000_000), Some(912_000));
    }

    #[test]
    fn rate_choice_handles_unsorted_and_empty_lists() {
        assert_eq!(choose_rate(&[912_000, 192_000, 456_000], 300_000), Some(456_000));
        assert_eq!(choose_rate(&[], 300_000), None);
    }

    #[test]
    fn gain_is_clamped_to_the_attenuator_range() {
        assert_eq!(clamp_gain_db(-5.0), 0.0);
        assert_eq!(clamp_gain_db(100.0), 48.0);
        assert_eq!(clamp_gain_db(f32::NAN), 0.0);
        assert_eq!(clamp_gain_db(30.0), 30.0);
    }

    #[test]
    fn attenuation_steps_follow_the_gain_slider() {
        assert_eq!(attenuation_step(48.0), 0);
        assert_eq!(attenuation_step(0.0), 8);
        assert_eq!(attenuation_step(45.0), 1);
        assert_eq!(attenuation_step(1000.0), 0);
        assert_eq!(attenuation_step(-20.0), 8);
        assert_eq!(attenuation_step(f32::NAN), 8);
    }

    #[test]
    fn preamp_turns_on_in_the_upper_half() {
        assert!(!preamp_on(0.0));
        assert!(!preamp_on(2.9));
        assert!(preamp_on(3.0));
        assert!(preamp_on(50.0));
    }

    #[test]
    fn samples_are_kept_within_unit_range() {
        assert_eq!(clean_sample(0.5), 0.5);
        assert_eq!(clean_sample(2.0), 1.0);
        assert_eq!(clean_sample(-3.0), -1.0);
        assert_eq!(clean_sample(f32::NAN), 0.0);
        assert_eq!(clean_sample(f32::INFINITY), 0.0);
    }

    #[test]
    fn serial_numbers_parse_as_hex() {
        assert_eq!(parse_serial("8D7A3C1B2F4E5A60"), Ok(0x8D7A_3C1B_2F4E_5A60));
        assert_eq!(parse_serial(" 0x1f "), Ok(0x1f));
        assert!(parse_serial("").is_err());
        assert!(parse_serial("not-hex").is_err());
    }

    #[test]
    fn missing_library_error_says_what_to_install() {
        let err = load_library(&["kahf-no-such-library-airspyhf.so"]).unwrap_err();
        let msg = missing_library_message(&err);
        assert!(msg.contains("Airspy HF+"));
        assert!(msg.contains("libairspyhf"));
        assert!(msg.contains("not found"));
        assert!(msg.contains("kahf-no-such-library-airspyhf.so"));
    }

    #[test]
    fn open_without_the_library_gives_the_install_message() {
        // Only meaningful on a machine without the library, so accept either outcome.
        if let Err(msg) = open(None) {
            assert!(msg.contains("Airspy HF+"));
        }
    }
}
