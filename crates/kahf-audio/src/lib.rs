//! Sound-card audio I/O for rig audio (owner: audio agent).
//!
//! Everything the rest of KAHF sees is mono `f32` at [`RATE`] (8000 Hz).
//! Devices are asked for 8 kHz mono first. If the device refuses that
//! configuration, the stream runs at the device's own default rate and
//! channel count and is converted here (downmix + [`Resampler`]).

use std::collections::VecDeque;
use std::f64::consts::PI;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, FromSample, Sample, SampleFormat, SizedSample, StreamConfig};

/// Sample rate of every stream seen by callers, in Hz.
pub const RATE: u32 = 8000;

/// Ring capacity in samples at [`RATE`] (2 seconds).
const RING_CAP: usize = RATE as usize * 2;

type Ring = Arc<Mutex<VecDeque<f32>>>;
type ErrSlot = Arc<Mutex<Option<String>>>;

fn lock(ring: &Ring) -> MutexGuard<'_, VecDeque<f32>> {
    ring.lock().unwrap_or_else(|e| e.into_inner())
}

fn set_err(slot: &ErrSlot, msg: String) {
    if let Ok(mut s) = slot.lock() {
        s.get_or_insert(msg);
    }
}

// ---------------------------------------------------------------------------
// Resampler: streaming windowed-sinc FIR with a low-pass at 0.45 x the lower
// of the two rates (so decimation does not alias and interpolation does not
// image). Works for any ratio, integer or not (e.g. 48000 -> 8000, 44100 -> 8000).
// ---------------------------------------------------------------------------

/// Half the FIR length in input samples (filter is 2 * HALF taps long).
const HALF: i64 = 64;

/// Streaming sample-rate converter. Push input blocks with [`Resampler::process`].
pub struct Resampler {
    /// Input samples advanced per output sample (in_rate / out_rate).
    step: f64,
    /// Low-pass cutoff in cycles per input sample.
    cutoff: f64,
    /// Input samples still needed, starting at absolute index `base`.
    buf: Vec<f32>,
    base: i64,
    /// Absolute input index of the next output sample.
    pos: f64,
}

impl Resampler {
    pub fn new(in_rate: u32, out_rate: u32) -> Self {
        let lo = in_rate.min(out_rate) as f64;
        Self {
            step: in_rate as f64 / out_rate as f64,
            cutoff: 0.45 * lo / in_rate as f64,
            buf: Vec::new(),
            base: 0,
            pos: 0.0,
        }
    }

    fn sample(&self, n: i64) -> f64 {
        if n < self.base {
            0.0 // before the start of the stream: silence
        } else {
            self.buf[(n - self.base) as usize] as f64
        }
    }

    /// Convert `input` and append the produced samples to `out`.
    /// Output lags the input by about `HALF` input samples.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.buf.extend_from_slice(input);
        let end = self.base + self.buf.len() as i64; // one past the newest sample
        loop {
            let c = self.pos.floor() as i64;
            if c + HALF >= end {
                break; // need future input before this output can be formed
            }
            let mut acc = 0.0f64;
            let mut wsum = 0.0f64;
            for n in (c - HALF + 1)..=(c + HALF) {
                let d = n as f64 - self.pos;
                let window = 0.5 * (1.0 + (PI * d / HALF as f64).cos());
                let kernel = if d.abs() < 1e-9 {
                    2.0 * self.cutoff
                } else {
                    (2.0 * PI * self.cutoff * d).sin() / (PI * d)
                };
                let g = window * kernel;
                acc += g * self.sample(n);
                wsum += g;
            }
            out.push((acc / wsum) as f32); // normalise to unity DC gain
            self.pos += self.step;
        }
        // Drop input the next output can no longer reach.
        let keep = self.pos.floor() as i64 - HALF + 1;
        let drop = (keep - self.base).clamp(0, self.buf.len() as i64) as usize;
        self.buf.drain(..drop);
        self.base += drop as i64;
    }
}

// ---------------------------------------------------------------------------
// Device discovery and selection
// ---------------------------------------------------------------------------

/// RMS level of `samples` in dBFS (a full-scale sine is about -3 dB).
/// Silence and empty input return -120, which is also the floor.
pub fn level_db(samples: &[f32]) -> f32 {
    const FLOOR: f32 = -120.0;
    if samples.is_empty() {
        return FLOOR;
    }
    let mean_sq = samples.iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>() / samples.len() as f64;
    if mean_sq <= 0.0 {
        return FLOOR;
    }
    ((10.0 * mean_sq.log10()) as f32).max(FLOOR)
}

/// Name of the system's default input or output device, if there is one.
pub fn default_device_name(input: bool) -> Option<String> {
    let host = cpal::default_host();
    let dev = if input {
        host.default_input_device()
    } else {
        host.default_output_device()
    };
    dev.and_then(|d| d.description().ok().map(|x| x.name().to_string()))
}

/// Names of the devices that offer input (or output when `input` is false), in host order.
pub fn device_names(input: bool) -> Vec<String> {
    let host = cpal::default_host();
    let Ok(devices) = host.devices() else {
        return Vec::new();
    };
    let mut names: Vec<String> = Vec::new();
    for dev in devices {
        let ok = if input {
            dev.supports_input()
        } else {
            dev.supports_output()
        };
        if !ok {
            continue;
        }
        if let Ok(desc) = dev.description() {
            let name = desc.name().to_string();
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

/// Names of all sound devices that offer input or output, in host order.
/// Never panics; returns an empty list if the host cannot enumerate devices.
pub fn list_devices() -> Vec<String> {
    let host = cpal::default_host();
    let Ok(devices) = host.devices() else {
        return Vec::new();
    };
    let mut names: Vec<String> = Vec::new();
    for dev in devices {
        if !(dev.supports_input() || dev.supports_output()) {
            continue;
        }
        if let Ok(desc) = dev.description() {
            let name = desc.name().to_string();
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

fn find_device(name: Option<&str>, input: bool) -> Result<cpal::Device, String> {
    let dir = if input { "input" } else { "output" };
    let host = cpal::default_host();
    match name {
        None => {
            let dev = if input {
                host.default_input_device()
            } else {
                host.default_output_device()
            };
            dev.ok_or_else(|| format!("no default {dir} device"))
        }
        Some(want) => host
            .devices()
            .map_err(|e| e.to_string())?
            .find(|d| {
                let dir_ok = if input {
                    d.supports_input()
                } else {
                    d.supports_output()
                };
                dir_ok && d.description().map(|x| x.name() == want).unwrap_or(false)
            })
            .ok_or_else(|| format!("no {dir} device named {want:?}")),
    }
}

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

/// Sound-card microphone / rig receive audio, delivered as 8 kHz mono f32.
pub struct AudioIn {
    _stream: cpal::Stream,
    ring: Ring,
    err: ErrSlot,
    name: String,
}

impl AudioIn {
    /// Open an input device (`None` = system default). Requests 8 kHz mono;
    /// falls back to the device's default format and resamples internally.
    pub fn open(device: Option<&str>) -> Result<Self, String> {
        let dev = find_device(device, true)?;
        let name = dev
            .description()
            .map(|d| d.name().to_string())
            .unwrap_or_else(|_| device.unwrap_or("unknown").to_string());
        let ring: Ring = Arc::new(Mutex::new(VecDeque::new()));
        let err: ErrSlot = Arc::new(Mutex::new(None));

        let want = StreamConfig {
            channels: 1,
            sample_rate: RATE,
            buffer_size: BufferSize::Default,
        };
        let fmt = dev
            .default_input_config()
            .map_err(|e| e.to_string())?
            .sample_format();
        let stream = match build_input(&dev, want, fmt, 1, RATE, &ring, &err) {
            Ok(s) => s,
            Err(_) => {
                // Device refused 8 kHz: use its own rate/channels and resample.
                let def = dev.default_input_config().map_err(|e| e.to_string())?;
                let cfg = def.config();
                build_input(
                    &dev,
                    cfg,
                    def.sample_format(),
                    cfg.channels as usize,
                    cfg.sample_rate,
                    &ring,
                    &err,
                )?
            }
        };
        stream.play().map_err(|e| e.to_string())?;
        Ok(Self {
            _stream: stream,
            ring,
            err,
            name,
        })
    }

    /// Name of the device this stream was opened on.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Copy up to `out.len()` samples from the ring buffer. Non-blocking:
    /// returns how many were written (0 when nothing is buffered yet).
    pub fn read(&mut self, out: &mut [f32]) -> usize {
        let mut q = lock(&self.ring);
        let n = out.len().min(q.len());
        for (dst, src) in out.iter_mut().zip(q.drain(..n)) {
            *dst = src;
        }
        n
    }

    /// The first error reported by the audio stream, if any.
    pub fn error(&self) -> Option<String> {
        self.err.lock().ok().and_then(|e| e.clone())
    }
}

fn build_input(
    dev: &cpal::Device,
    cfg: StreamConfig,
    fmt: SampleFormat,
    channels: usize,
    rate_in: u32,
    ring: &Ring,
    err: &ErrSlot,
) -> Result<cpal::Stream, String> {
    match fmt {
        SampleFormat::F32 => input_stream::<f32>(dev, cfg, channels, rate_in, ring, err),
        SampleFormat::I16 => input_stream::<i16>(dev, cfg, channels, rate_in, ring, err),
        SampleFormat::U16 => input_stream::<u16>(dev, cfg, channels, rate_in, ring, err),
        SampleFormat::I32 => input_stream::<i32>(dev, cfg, channels, rate_in, ring, err),
        SampleFormat::U8 => input_stream::<u8>(dev, cfg, channels, rate_in, ring, err),
        SampleFormat::I8 => input_stream::<i8>(dev, cfg, channels, rate_in, ring, err),
        other => Err(format!("unsupported input sample format {other:?}")),
    }
}

fn input_stream<T>(
    dev: &cpal::Device,
    cfg: StreamConfig,
    channels: usize,
    rate_in: u32,
    ring: &Ring,
    err: &ErrSlot,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let mut rs = Resampler::new(rate_in, RATE);
    let mut mono: Vec<f32> = Vec::new();
    let mut out: Vec<f32> = Vec::new();
    let ring = ring.clone();
    let err_slot = err.clone();
    dev.build_input_stream(
        cfg,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            mono.clear();
            for frame in data.chunks(channels) {
                let sum: f32 = frame.iter().map(|&s| s.to_sample::<f32>()).sum();
                mono.push(sum / channels as f32);
            }
            out.clear();
            rs.process(&mono, &mut out);
            // Never block the audio thread: drop this block if the reader holds the lock.
            if let Ok(mut q) = ring.try_lock() {
                q.extend(out.iter().copied());
                while q.len() > RING_CAP {
                    q.pop_front();
                }
            }
        },
        move |e| set_err(&err_slot, e.to_string()),
        None,
    )
    .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

/// Sound-card speaker / rig transmit audio, fed as 8 kHz mono f32.
pub struct AudioOut {
    _stream: cpal::Stream,
    ring: Ring,
    err: ErrSlot,
}

impl AudioOut {
    /// Open an output device (`None` = system default). Requests 8 kHz mono;
    /// falls back to the device's default format and resamples internally.
    pub fn open(device: Option<&str>) -> Result<Self, String> {
        let dev = find_device(device, false)?;
        let ring: Ring = Arc::new(Mutex::new(VecDeque::new()));
        let err: ErrSlot = Arc::new(Mutex::new(None));

        let want = StreamConfig {
            channels: 1,
            sample_rate: RATE,
            buffer_size: BufferSize::Default,
        };
        let fmt = dev
            .default_output_config()
            .map_err(|e| e.to_string())?
            .sample_format();
        let stream = match build_output(&dev, want, fmt, 1, RATE, &ring, &err) {
            Ok(s) => s,
            Err(_) => {
                let def = dev.default_output_config().map_err(|e| e.to_string())?;
                let cfg = def.config();
                build_output(
                    &dev,
                    cfg,
                    def.sample_format(),
                    cfg.channels as usize,
                    cfg.sample_rate,
                    &ring,
                    &err,
                )?
            }
        };
        stream.play().map_err(|e| e.to_string())?;
        Ok(Self {
            _stream: stream,
            ring,
            err,
        })
    }

    /// Queue 8 kHz mono samples for playback. Blocks (sleeping) while the
    /// 2-second buffer is full, which paces the caller to real time.
    pub fn write(&mut self, samples: &[f32]) -> Result<(), String> {
        for chunk in samples.chunks(1024) {
            let started = Instant::now();
            loop {
                if let Some(e) = self.err.lock().ok().and_then(|e| e.clone()) {
                    return Err(e);
                }
                {
                    let mut q = lock(&self.ring);
                    if q.len() + chunk.len() <= RING_CAP {
                        q.extend(chunk.iter().copied());
                        break;
                    }
                }
                if started.elapsed() > Duration::from_secs(5) {
                    return Err("audio output stalled: no playback progress for 5 s".into());
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        Ok(())
    }
}

fn build_output(
    dev: &cpal::Device,
    cfg: StreamConfig,
    fmt: SampleFormat,
    channels: usize,
    rate_out: u32,
    ring: &Ring,
    err: &ErrSlot,
) -> Result<cpal::Stream, String> {
    match fmt {
        SampleFormat::F32 => output_stream::<f32>(dev, cfg, channels, rate_out, ring, err),
        SampleFormat::I16 => output_stream::<i16>(dev, cfg, channels, rate_out, ring, err),
        SampleFormat::U16 => output_stream::<u16>(dev, cfg, channels, rate_out, ring, err),
        SampleFormat::I32 => output_stream::<i32>(dev, cfg, channels, rate_out, ring, err),
        SampleFormat::U8 => output_stream::<u8>(dev, cfg, channels, rate_out, ring, err),
        SampleFormat::I8 => output_stream::<i8>(dev, cfg, channels, rate_out, ring, err),
        other => Err(format!("unsupported output sample format {other:?}")),
    }
}

fn output_stream<T>(
    dev: &cpal::Device,
    cfg: StreamConfig,
    channels: usize,
    rate_out: u32,
    ring: &Ring,
    err: &ErrSlot,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + Send + 'static,
    T: FromSample<f32>,
{
    let mut rs = Resampler::new(RATE, rate_out);
    let mut pending: VecDeque<f32> = VecDeque::new(); // device-rate mono
    let mut src: Vec<f32> = Vec::new();
    let mut res: Vec<f32> = Vec::new();
    let ring = ring.clone();
    let err_slot = err.clone();
    dev.build_output_stream(
        cfg,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            let frames = data.len() / channels;
            while pending.len() < frames {
                src.clear();
                if let Ok(mut q) = ring.try_lock() {
                    let n = q.len().min(4096);
                    src.extend(q.drain(..n));
                }
                if src.is_empty() {
                    src.resize(RATE as usize / 100, 0.0); // underrun: 10 ms of silence
                }
                res.clear();
                rs.process(&src, &mut res);
                pending.extend(res.iter().copied());
            }
            for frame in data.chunks_mut(channels) {
                let v = pending.pop_front().unwrap_or(0.0);
                let s: T = v.to_sample::<T>();
                for o in frame.iter_mut() {
                    *o = s;
                }
            }
        },
        move |e| set_err(&err_slot, e.to_string()),
        None,
    )
    .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Tests (no real devices are opened)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f64, rate: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * PI * freq * i as f64 / rate).sin() as f32)
            .collect()
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, &v| m.max(v.abs()))
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn list_devices_does_not_panic() {
        let _ = list_devices();
    }

    #[test]
    fn level_db_silence_and_full_scale_sine() {
        assert_eq!(level_db(&vec![0.0f32; 8000]), -120.0);
        assert_eq!(level_db(&[]), -120.0);
        // 1000 cycles of a unit sine over exactly 1 s: RMS = 1/sqrt(2), about -3.01 dB.
        let db = level_db(&tone(1000.0, RATE as f64, 8000));
        assert!((db + 3.01).abs() < 0.05, "sine level {db}");
    }

    #[test]
    fn decimate_48k_to_8k_keeps_voice_band_and_rejects_alias() {
        let mut rs = Resampler::new(48_000, RATE);

        // 1 kHz is inside the passband: amplitude preserved.
        let mut out = Vec::new();
        rs.process(&tone(1000.0, 48_000.0, 24_000), &mut out);
        assert!((3980..=4000).contains(&out.len()), "len {}", out.len());
        let pass = &out[400..];
        assert!((0.95..=1.05).contains(&peak(pass)), "1 kHz peak {}", peak(pass));

        // 7 kHz would alias to 1 kHz at 8 kHz: must be strongly attenuated.
        let mut rs = Resampler::new(48_000, RATE);
        let mut out = Vec::new();
        rs.process(&tone(7000.0, 48_000.0, 24_000), &mut out);
        assert!(rms(&out[400..]) < 0.02, "7 kHz rms {}", rms(&out[400..]));
    }

    #[test]
    fn dc_passes_unchanged() {
        let mut rs = Resampler::new(48_000, RATE);
        let mut out = Vec::new();
        rs.process(&vec![0.5f32; 4800], &mut out);
        for v in &out[100..] {
            assert!((v - 0.5).abs() < 1e-3, "dc sample {v}");
        }
    }

    #[test]
    fn chunked_processing_matches_one_shot() {
        let input = tone(800.0, 48_000.0, 9_600);

        let mut whole = Vec::new();
        Resampler::new(48_000, RATE).process(&input, &mut whole);

        let mut rs = Resampler::new(48_000, RATE);
        let mut chunked = Vec::new();
        for c in input.chunks(1000) {
            rs.process(c, &mut chunked);
        }
        assert_eq!(whole.len(), chunked.len());
        for (a, b) in whole.iter().zip(&chunked) {
            assert!((a - b).abs() < 1e-6);
        }
    }
}
