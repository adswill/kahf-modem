//! RTL-SDR receiver over librtlsdr (Homebrew on macOS, system library elsewhere).
//!
//! The device is opened and tuned on the caller's thread. A worker thread then
//! reads blocks with rtlsdr_read_sync (the same choice the makes on macOS, where
//! rtlsdr_read_async can free a transfer that is still in flight) and sends the
//! converted samples over a bounded channel. read_iq drains that channel.

use std::ffi::{c_char, c_int, c_void, CStr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use kahf_core::io::IqSource;

/// Opaque librtlsdr device handle (`rtlsdr_dev_t`).
#[repr(C)]
pub struct RtlDev {
    _private: [u8; 0],
}

crate::dl::foreign_fns! {
rtlsdr;
fn rtlsdr_get_device_count() -> u32;
fn rtlsdr_get_device_name(index: u32) -> *const c_char;
fn rtlsdr_open(dev: *mut *mut RtlDev, index: u32) -> c_int;
fn rtlsdr_close(dev: *mut RtlDev) -> c_int;
fn rtlsdr_set_center_freq(dev: *mut RtlDev, freq: u32) -> c_int;
fn rtlsdr_set_sample_rate(dev: *mut RtlDev, rate: u32) -> c_int;
fn rtlsdr_set_tuner_gain_mode(dev: *mut RtlDev, manual: c_int) -> c_int;
fn rtlsdr_set_tuner_gain(dev: *mut RtlDev, gain: c_int) -> c_int;
fn rtlsdr_get_tuner_gains(dev: *mut RtlDev, gains: *mut c_int) -> c_int;
fn rtlsdr_set_agc_mode(dev: *mut RtlDev, on: c_int) -> c_int;
fn rtlsdr_get_sample_rate(dev: *mut RtlDev) -> u32;
fn rtlsdr_set_freq_correction(dev: *mut RtlDev, ppm: c_int) -> c_int;
fn rtlsdr_reset_buffer(dev: *mut RtlDev) -> c_int;
fn rtlsdr_read_sync(dev: *mut RtlDev, buf: *mut c_void, len: c_int, n_read: *mut c_int) -> c_int;}

/// librtlsdr returns -2 from set_freq_correction when the value is already set.
const ALREADY_SET: c_int = -2;
/// Highest sample rate the dongle delivers (the refuses anything above it).
const MAX_RATE: u32 = 3_200_000;
/// Rate ceiling for a stable stream (the caps the request here; above it most dongles drop samples).
const STABLE_MAX_RATE: u32 = 2_560_000;
/// Rate used when the caller asks for 0 (the default).
const DEFAULT_RATE: u32 = 2_048_000;
/// Blocks queued between the worker and read_iq before new blocks are dropped.
const QUEUE_BLOCKS: usize = 64;
/// How long read_iq waits for samples before it reports a stalled radio.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Converts one RTL-SDR byte to a float: (value - 127.4) / 128, the same mapping as the earlier driver's
/// source_rtlsdr.cpp convert() (its (buf - 127.4f) / 128.f). The old (v - 127.5) / 127.5 differed slightly.
#[inline]
pub fn u8_to_f32(v: u8) -> f32 {
    (v as f32 - 127.4) / 128.0
}

/// Inverse of [`u8_to_f32`], rounded and clamped to 0..=255.
#[inline]
pub fn f32_to_u8(x: f32) -> u8 {
    (x * 128.0 + 127.4).round().clamp(0.0, 255.0) as u8
}

/// Sample rate actually requested from librtlsdr, from the caller's rate. Mirrors the source_rtlsdr.cpp configure():
/// refuse above 3.2 Msps, cap at 2.56 Msps, 0 means 2.048 Msps. Rates at or below 225 kHz, and those from 300 kHz up to 900 kHz,
/// are raised to 900001 Hz (librtlsdr refuses the first; the second is not a stable rate). 225-300 kHz is kept.
pub fn stream_rate(requested: u32) -> Result<u32, String> {
    if requested > MAX_RATE {
        return Err(format!(
            "RTL-SDR gives at most 3.2 Msps; {:.0} Msps needs a faster radio",
            requested as f64 / 1e6
        ));
    }
    let base = if requested > 0 { requested } else { DEFAULT_RATE };
    let want = base.min(STABLE_MAX_RATE);
    Ok(if want < 900_001 && (want > 300_000 || want <= 225_000) {
        900_001
    } else {
        want
    })
}

/// Bytes per read_sync call, from the rate: about 100 ms of samples, a multiple of 16 kB, from 16 kB to 256 kB (the macOS
/// read size; it bounds how long stop() waits for a read). At 2.4 Msps this is 256 kB, about 55 ms.
pub fn read_bytes(rate: u32) -> usize {
    let blocks = ((rate as f64 * 2.0 * 0.1) / 16384.0).round() as usize;
    (blocks.max(1) * 16_384).clamp(16_384, 262_144)
}

/// Converts interleaved uint8 I/Q bytes to complex samples. A trailing odd byte is ignored.
pub fn convert_iq(bytes: &[u8]) -> Vec<(f32, f32)> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| (u8_to_f32(p[0]), u8_to_f32(p[1])))
        .collect()
}

/// Names of the RTL-SDR devices found on the USB bus, in index order.
/// What to tell the operator when the RTL-SDR library cannot be found.
pub(crate) const RTLSDR_MISSING: &str = "The RTL-SDR library is not installed. Install librtlsdr: Homebrew \"brew install librtlsdr\" on macOS, the librtlsdr package on Linux, or the RTL-SDR driver package on Windows.";

pub fn list_devices() -> Vec<String> {
    if crate::dl::rtlsdr().is_none() {
        return Vec::new();
    }
    let n = unsafe { rtlsdr_get_device_count() };
    (0..n)
        .map(|i| {
            let p = unsafe { rtlsdr_get_device_name(i) };
            if p.is_null() {
                format!("RTL-SDR {i}")
            } else {
                unsafe { CStr::from_ptr(p) }.to_string_lossy().trim().to_string()
            }
        })
        .collect()
}

/// Raw device handle that may move to the worker thread. Only the worker reads from it
/// while it runs; the owner closes it after the worker has been joined.
#[derive(Clone, Copy)]
struct Handle(*mut RtlDev);
unsafe impl Send for Handle {}

pub struct RtlsdrRx {
    dev: Handle,
    rate: f64,
    rx: Receiver<Vec<(f32, f32)>>,
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    pending: Vec<(f32, f32)>,
    pos: usize,
}

impl RtlsdrRx {
    /// Opens device `index` and tunes it.
    ///
    /// `gain_tenths_db` = `Some(g)` sets manual gain in tenths of a dB (e.g. 496 = 49.6 dB), snapped to the
    /// nearest gain the tuner offers, as the does. The tuner is always in manual gain mode (the runs its own
    /// AGC, with the RTL2832U's AGC off); `None` keeps the library's current gain. `ppm` is the frequency correction.
    /// Streaming starts immediately; samples arrive through [`IqSource::read_iq`].
    pub fn open(
        index: u32,
        freq_hz: u32,
        sample_rate: u32,
        gain_tenths_db: Option<i32>,
        ppm: i32,
    ) -> Result<Self, String> {
        if crate::dl::rtlsdr().is_none() {
            return Err(RTLSDR_MISSING.to_string());
        }
        let mut dev: *mut RtlDev = std::ptr::null_mut();
        if unsafe { rtlsdr_open(&mut dev, index) } != 0 || dev.is_null() {
            return Err(format!("RTL-SDR: cannot open device {index}"));
        }
        let handle = Handle(dev);

        // Same order as the openDevice + configure: AGC off, manual gain mode, rate, ppm, frequency, gain.
        // The buffer reset is done by the worker before its first read (the streamLoop), not here.
        let configure = || -> Result<u32, String> {
            unsafe { rtlsdr_set_agc_mode(dev, 0) };
            if unsafe { rtlsdr_set_tuner_gain_mode(dev, 1) } < 0 {
                return Err("RTL-SDR: manual gain mode refused".to_string());
            }
            let want = stream_rate(sample_rate)?;
            if unsafe { rtlsdr_set_sample_rate(dev, want) } < 0 {
                return Err(format!("RTL-SDR: sample rate {want} Hz refused"));
            }
            // the rate the dongle actually runs at (the keeps this, not the request)
            let actual = match unsafe { rtlsdr_get_sample_rate(dev) } {
                0 => want,
                r => r,
            };
            let r = unsafe { rtlsdr_set_freq_correction(dev, ppm) };
            if r < 0 && r != ALREADY_SET {
                return Err(format!("RTL-SDR: frequency correction {ppm} ppm refused ({r})"));
            }
            if unsafe { rtlsdr_set_center_freq(dev, freq_hz) } < 0 {
                return Err(format!("RTL-SDR: driver refused {freq_hz} Hz"));
            }
            if let Some(g) = gain_tenths_db {
                set_nearest_gain(dev, g)?;
            }
            Ok(actual)
        };
        let rate = match configure() {
            Ok(rate) => rate,
            Err(e) => {
                unsafe { rtlsdr_close(dev) };
                return Err(e);
            }
        };

        let (tx, rx) = mpsc::sync_channel(QUEUE_BLOCKS);
        let running = Arc::new(AtomicBool::new(true));
        let block_bytes = read_bytes(rate);
        let worker = {
            let running = Arc::clone(&running);
            std::thread::Builder::new()
                .name("rtlsdr-rx".into())
                .spawn(move || {
                    let handle = handle;
                    worker_loop(handle.0, tx, running, block_bytes);
                })
                .map_err(|e| {
                    unsafe { rtlsdr_close(dev) };
                    format!("RTL-SDR: cannot start reader thread: {e}")
                })?
        };

        Ok(Self {
            dev: handle,
            rate: rate as f64,
            rx,
            running,
            worker: Some(worker),
            pending: Vec::new(),
            pos: 0,
        })
    }
}

/// Sets the tuner gain to the one the tuner offers closest to `want_tenths_db` (the configure() does the same).
fn set_nearest_gain(dev: *mut RtlDev, want_tenths_db: i32) -> Result<(), String> {
    let n = unsafe { rtlsdr_get_tuner_gains(dev, std::ptr::null_mut()) };
    if n <= 0 {
        return Ok(()); // no list: nothing to snap to
    }
    let mut gains = vec![0 as c_int; n as usize];
    let got = unsafe { rtlsdr_get_tuner_gains(dev, gains.as_mut_ptr()) };
    if got <= 0 {
        return Ok(());
    }
    let best = gains[..got as usize]
        .iter()
        .copied()
        .min_by_key(|g| (g - want_tenths_db).abs())
        .unwrap_or(want_tenths_db);
    if unsafe { rtlsdr_set_tuner_gain(dev, best) } < 0 {
        return Err(format!("RTL-SDR: manual gain {best} refused"));
    }
    Ok(())
}

fn worker_loop(
    dev: *mut RtlDev,
    tx: SyncSender<Vec<(f32, f32)>>,
    running: Arc<AtomicBool>,
    block_bytes: usize,
) {
    // the streamLoop: reset the buffer before the first read
    if unsafe { rtlsdr_reset_buffer(dev) } < 0 {
        return;
    }
    let mut raw = vec![0u8; block_bytes];
    while running.load(Ordering::Relaxed) {
        let mut got: c_int = 0;
        let rc =
            unsafe { rtlsdr_read_sync(dev, raw.as_mut_ptr() as *mut c_void, raw.len() as c_int, &mut got) };
        if rc < 0 {
            break; // unplugged or the stream died
        }
        if got <= 0 {
            continue;
        }
        let samples = convert_iq(&raw[..got as usize]);
        match tx.try_send(samples) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {} // consumer is behind: drop this block, keep the radio running
            Err(TrySendError::Disconnected(_)) => break,
        }
    }
}

impl IqSource for RtlsdrRx {
    fn read_iq(&mut self, out: &mut [(f32, f32)]) -> Result<usize, String> {
        if out.is_empty() {
            return Ok(0);
        }
        while self.pos >= self.pending.len() {
            self.pending = self.rx.recv_timeout(READ_TIMEOUT).map_err(|e| match e {
                RecvTimeoutError::Timeout => "RTL-SDR: no samples for 5 s".to_string(),
                RecvTimeoutError::Disconnected => "RTL-SDR: stream stopped".to_string(),
            })?;
            self.pos = 0;
        }
        let mut n = 0;
        loop {
            while self.pos < self.pending.len() && n < out.len() {
                out[n] = self.pending[self.pos];
                n += 1;
                self.pos += 1;
            }
            if n == out.len() {
                break;
            }
            match self.rx.try_recv() {
                Ok(block) => {
                    self.pending = block;
                    self.pos = 0;
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        Ok(n)
    }

    fn sample_rate(&self) -> f64 {
        self.rate
    }
}

impl Drop for RtlsdrRx {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(w) = self.worker.take() {
            let _ = w.join(); // read_sync returns within one block, so this is quick
        }
        unsafe { rtlsdr_close(self.dev.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtlsdr_u8_extremes_map_to_unit_range() {
        assert!((u8_to_f32(0) - (-127.4 / 128.0)).abs() < 1e-6);
        assert!((u8_to_f32(255) - (127.6 / 128.0)).abs() < 1e-6);
        assert!(u8_to_f32(0) >= -1.0 && u8_to_f32(255) < 1.0);
    }

    #[test]
    fn rtlsdr_u8_midpoint_is_near_zero() {
        assert!((u8_to_f32(127) - (-0.4 / 128.0)).abs() < 1e-6);
        assert!((u8_to_f32(128) - (0.6 / 128.0)).abs() < 1e-6);
    }

    #[test]
    fn rtlsdr_stream_rate_rules() {
        assert_eq!(stream_rate(2_400_000), Ok(2_400_000));
        assert_eq!(stream_rate(0), Ok(2_048_000));
        assert_eq!(stream_rate(3_000_000), Ok(2_560_000));
        assert_eq!(stream_rate(250_000), Ok(250_000));
        assert_eq!(stream_rate(200_000), Ok(900_001));
        assert_eq!(stream_rate(500_000), Ok(900_001));
        assert!(stream_rate(3_200_001).is_err());
    }

    #[test]
    fn rtlsdr_read_size_follows_rate() {
        assert_eq!(read_bytes(2_400_000), 262_144);
        assert_eq!(read_bytes(900_001), 180_224);
        assert_eq!(read_bytes(1), 16_384);
    }

    #[test]
    fn rtlsdr_u8_round_trips_every_value() {
        for v in 0..=255u8 {
            assert_eq!(f32_to_u8(u8_to_f32(v)), v, "value {v}");
        }
    }

    #[test]
    fn rtlsdr_f32_to_u8_clamps_out_of_range() {
        assert_eq!(f32_to_u8(-5.0), 0);
        assert_eq!(f32_to_u8(5.0), 255);
    }

    #[test]
    fn rtlsdr_convert_pairs_bytes_as_iq() {
        let s = convert_iq(&[0, 255, 128, 127, 42]);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0], (u8_to_f32(0), u8_to_f32(255)));
        assert_eq!(s[1], (u8_to_f32(128), u8_to_f32(127)));
    }
}
