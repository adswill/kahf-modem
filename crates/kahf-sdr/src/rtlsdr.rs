//! RTL-SDR receiver over librtlsdr (Homebrew on macOS, system library elsewhere).
//! Reference: OnAir's core/src/source/source_rtlsdr.cpp.
//!
//! The device is opened and tuned on the caller's thread. A worker thread then
//! reads blocks with rtlsdr_read_sync (the same choice OnAir makes on macOS, where
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

unsafe extern "C" {
    fn rtlsdr_get_device_count() -> u32;
    fn rtlsdr_get_device_name(index: u32) -> *const c_char;
    fn rtlsdr_open(dev: *mut *mut RtlDev, index: u32) -> c_int;
    fn rtlsdr_close(dev: *mut RtlDev) -> c_int;
    fn rtlsdr_set_center_freq(dev: *mut RtlDev, freq: u32) -> c_int;
    fn rtlsdr_set_sample_rate(dev: *mut RtlDev, rate: u32) -> c_int;
    fn rtlsdr_set_tuner_gain_mode(dev: *mut RtlDev, manual: c_int) -> c_int;
    fn rtlsdr_set_tuner_gain(dev: *mut RtlDev, gain: c_int) -> c_int;
    fn rtlsdr_set_freq_correction(dev: *mut RtlDev, ppm: c_int) -> c_int;
    fn rtlsdr_reset_buffer(dev: *mut RtlDev) -> c_int;
    fn rtlsdr_read_sync(dev: *mut RtlDev, buf: *mut c_void, len: c_int, n_read: *mut c_int) -> c_int;
}

/// librtlsdr returns -2 from set_freq_correction when the value is already set.
const ALREADY_SET: c_int = -2;
/// Bytes per read: 256 kB, about 55 ms at 2.4 Msps (16384 is the block size the driver wants).
const BLOCK_BYTES: usize = 262_144;
/// Blocks queued between the worker and read_iq before new blocks are dropped.
const QUEUE_BLOCKS: usize = 64;
/// How long read_iq waits for samples before it reports a stalled radio.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Converts one RTL-SDR byte to a float in [-1, 1): (value - 127.5) / 127.5.
#[inline]
pub fn u8_to_f32(v: u8) -> f32 {
    (v as f32 - 127.5) / 127.5
}

/// Inverse of [`u8_to_f32`], rounded and clamped to 0..=255.
#[inline]
pub fn f32_to_u8(x: f32) -> u8 {
    (x * 127.5 + 127.5).round().clamp(0.0, 255.0) as u8
}

/// Converts interleaved uint8 I/Q bytes to complex samples. A trailing odd byte is ignored.
pub fn convert_iq(bytes: &[u8]) -> Vec<(f32, f32)> {
    bytes
        .chunks_exact(2)
        .map(|p| (u8_to_f32(p[0]), u8_to_f32(p[1])))
        .collect()
}

/// Names of the RTL-SDR devices found on the USB bus, in index order.
pub fn list_devices() -> Vec<String> {
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
    /// `gain_tenths_db` = `None` leaves the tuner in automatic gain; `Some(g)` sets manual
    /// gain in tenths of a dB (e.g. 496 = 49.6 dB). `ppm` is the frequency correction.
    /// Streaming starts immediately; samples arrive through [`IqSource::read_iq`].
    pub fn open(
        index: u32,
        freq_hz: u32,
        sample_rate: u32,
        gain_tenths_db: Option<i32>,
        ppm: i32,
    ) -> Result<Self, String> {
        let mut dev: *mut RtlDev = std::ptr::null_mut();
        if unsafe { rtlsdr_open(&mut dev, index) } != 0 || dev.is_null() {
            return Err(format!("RTL-SDR: cannot open device {index}"));
        }
        let handle = Handle(dev);

        let configure = || -> Result<(), String> {
            if unsafe { rtlsdr_set_sample_rate(dev, sample_rate) } < 0 {
                return Err(format!("RTL-SDR: sample rate {sample_rate} Hz refused"));
            }
            match gain_tenths_db {
                None => {
                    unsafe { rtlsdr_set_tuner_gain_mode(dev, 0) };
                }
                Some(g) => {
                    if unsafe { rtlsdr_set_tuner_gain_mode(dev, 1) } < 0
                        || unsafe { rtlsdr_set_tuner_gain(dev, g) } < 0
                    {
                        return Err(format!("RTL-SDR: manual gain {g} refused"));
                    }
                }
            }
            let r = unsafe { rtlsdr_set_freq_correction(dev, ppm) };
            if r < 0 && r != ALREADY_SET {
                return Err(format!("RTL-SDR: frequency correction {ppm} ppm refused ({r})"));
            }
            if unsafe { rtlsdr_set_center_freq(dev, freq_hz) } < 0 {
                return Err(format!("RTL-SDR: driver refused {freq_hz} Hz"));
            }
            if unsafe { rtlsdr_reset_buffer(dev) } < 0 {
                return Err("RTL-SDR: reset buffer failed".to_string());
            }
            Ok(())
        };
        if let Err(e) = configure() {
            unsafe { rtlsdr_close(dev) };
            return Err(e);
        }

        let (tx, rx) = mpsc::sync_channel(QUEUE_BLOCKS);
        let running = Arc::new(AtomicBool::new(true));
        let worker = {
            let running = Arc::clone(&running);
            std::thread::Builder::new()
                .name("rtlsdr-rx".into())
                .spawn(move || {
                    let handle = handle;
                    worker_loop(handle.0, tx, running);
                })
                .map_err(|e| {
                    unsafe { rtlsdr_close(dev) };
                    format!("RTL-SDR: cannot start reader thread: {e}")
                })?
        };

        Ok(Self {
            dev: handle,
            rate: sample_rate as f64,
            rx,
            running,
            worker: Some(worker),
            pending: Vec::new(),
            pos: 0,
        })
    }
}

fn worker_loop(dev: *mut RtlDev, tx: SyncSender<Vec<(f32, f32)>>, running: Arc<AtomicBool>) {
    if unsafe { rtlsdr_reset_buffer(dev) } < 0 {
        return;
    }
    let mut raw = vec![0u8; BLOCK_BYTES];
    while running.load(Ordering::Relaxed) {
        let mut got: c_int = 0;
        let rc = unsafe {
            rtlsdr_read_sync(dev, raw.as_mut_ptr() as *mut c_void, raw.len() as c_int, &mut got)
        };
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
        assert_eq!(u8_to_f32(0), -1.0);
        assert_eq!(u8_to_f32(255), 255.0 / 127.5 - 1.0);
        assert!((u8_to_f32(255) - 1.0).abs() < 0.01);
    }

    #[test]
    fn rtlsdr_u8_midpoint_is_near_zero() {
        assert_eq!(u8_to_f32(127), -0.5 / 127.5);
        assert_eq!(u8_to_f32(128), 0.5 / 127.5);
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
