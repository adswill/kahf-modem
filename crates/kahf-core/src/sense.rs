//! Carrier sense: decides whether a channel is busy before transmitting.
//!
//! The input is the audio the station hears (real, mono, f32 in [-1, 1]).
//! Samples are cut into 50 ms frames and each frame gives a Hann-windowed
//! power spectrum per bin. The level of a channel is the mean bin power over
//! the last 500 ms. The noise floor of each bin is the 10th percentile of the
//! last 10 s of those 500 ms averages, so a signal that sits on a bin for a
//! while does not raise its floor. A channel is busy when its level is more
//! than 10 dB above the floor. Start markers are checked separately.

use crate::marker;
use std::collections::VecDeque;
use std::f64::consts::PI;

/// Analysis frame length in seconds.
const FRAME_S: f32 = 0.050;
/// Frames averaged for the channel level (500 ms).
const LEVEL_FRAMES: usize = 10;
/// Frames of level history kept for the noise floor (10 s).
const FLOOR_FRAMES: usize = 200;
/// Minimum history before a channel can be called busy (1 s).
const MIN_HISTORY: usize = 20;
/// Busy when the level is this many dB above the floor.
const BUSY_DB: f32 = 10.0;
/// Percentile used for the per-bin noise floor.
const FLOOR_PCT: f32 = 0.10;
/// Power values below this are treated as this (keeps dB finite).
const MIN_POWER: f32 = 1e-20;
/// The start marker is defined at 8000 Hz only.
const MARKER_RATE: f32 = 8000.0;

/// Carrier sense state for one audio input.
pub struct Sense {
    /// Broadband noise floor in dB (median of the per-bin floors).
    noise_floor_db: f32,
    sample_rate: f32,
    frame_len: usize,
    nbins: usize,
    window: Vec<f32>,
    window_power: f64,
    cos_t: Vec<f64>,
    sin_t: Vec<f64>,
    /// Samples not yet forming a whole frame.
    pending: Vec<f32>,
    /// Last second of raw samples, for the start marker search.
    raw: VecDeque<f32>,
    /// Last `LEVEL_FRAMES` raw frame spectra.
    recent: VecDeque<Vec<f32>>,
    /// Mean of `recent`, per bin (the 500 ms level).
    level: Vec<f32>,
    /// Last `FLOOR_FRAMES` values of `level`.
    history: VecDeque<Vec<f32>>,
    /// Per-bin noise floor (linear power).
    floor: Vec<f32>,
}

fn db(p: f32) -> f32 {
    10.0 * p.max(MIN_POWER).log10()
}

/// Median of a slice (the slice is reordered).
fn median(v: &mut [f32]) -> f32 {
    let mid = v.len() / 2;
    v.select_nth_unstable_by(mid, |a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v[mid]
}

impl Sense {
    /// New carrier sense for audio at `sample_rate` Hz.
    pub fn new(sample_rate: f32) -> Self {
        let frame_len = ((sample_rate * FRAME_S).round() as usize).max(16);
        let l = frame_len as f64;
        let window: Vec<f32> = (0..frame_len)
            .map(|n| (0.5 - 0.5 * (2.0 * PI * n as f64 / l).cos()) as f32)
            .collect();
        let window_power: f64 = window.iter().map(|&w| (w as f64) * (w as f64)).sum();
        let cos_t = (0..frame_len).map(|j| (2.0 * PI * j as f64 / l).cos()).collect();
        let sin_t = (0..frame_len).map(|j| (2.0 * PI * j as f64 / l).sin()).collect();
        let nbins = frame_len / 2 + 1;
        Sense {
            noise_floor_db: db(0.0),
            sample_rate,
            frame_len,
            nbins,
            window,
            window_power,
            cos_t,
            sin_t,
            pending: Vec::new(),
            raw: VecDeque::new(),
            recent: VecDeque::new(),
            level: vec![0.0; nbins],
            history: VecDeque::new(),
            floor: vec![0.0; nbins],
        }
    }

    /// Broadband noise floor in dB.
    pub fn noise_floor_db(&self) -> f32 {
        self.noise_floor_db
    }

    /// Feed audio samples. Any length is accepted.
    pub fn push(&mut self, samples: &[f32]) {
        let cap = self.sample_rate as usize;
        self.raw.extend(samples.iter().copied());
        while self.raw.len() > cap {
            self.raw.pop_front();
        }
        self.pending.extend_from_slice(samples);
        let mut start = 0;
        while self.pending.len() - start >= self.frame_len {
            let frame: Vec<f32> = self.pending[start..start + self.frame_len].to_vec();
            self.process_frame(&frame);
            start += self.frame_len;
        }
        self.pending.drain(..start);
    }

    fn process_frame(&mut self, frame: &[f32]) {
        let l = self.frame_len;
        let mut spec = vec![0.0f32; self.nbins];
        let windowed: Vec<f64> = frame
            .iter()
            .zip(&self.window)
            .map(|(&x, &w)| (x * w) as f64)
            .collect();
        for (k, out) in spec.iter_mut().enumerate() {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (n, &x) in windowed.iter().enumerate() {
                let idx = (k * n) % l;
                re += x * self.cos_t[idx];
                im -= x * self.sin_t[idx];
            }
            *out = ((re * re + im * im) / self.window_power) as f32;
        }

        self.recent.push_back(spec);
        if self.recent.len() > LEVEL_FRAMES {
            self.recent.pop_front();
        }
        let n = self.recent.len() as f32;
        for (k, lv) in self.level.iter_mut().enumerate() {
            *lv = self.recent.iter().map(|f| f[k]).sum::<f32>() / n;
        }

        self.history.push_back(self.level.clone());
        if self.history.len() > FLOOR_FRAMES {
            self.history.pop_front();
        }

        let mut col = Vec::with_capacity(self.history.len());
        for k in 0..self.nbins {
            col.clear();
            col.extend(self.history.iter().map(|h| h[k]));
            let idx = ((col.len() - 1) as f32 * FLOOR_PCT) as usize;
            col.select_nth_unstable_by(idx, |a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            self.floor[k] = col[idx];
        }
        let mut floors = self.floor.clone();
        self.noise_floor_db = db(median(&mut floors));
    }

    /// True when the mean level in the channel over the last 500 ms is more
    /// than 10 dB above the noise floor of that channel.
    ///
    /// `channel_khz` is the centre frequency in kHz at the audio input and
    /// `bandwidth_khz` the full width. Returns false until 1 s of audio has
    /// been seen.
    pub fn channel_busy(&self, channel_khz: f32, bandwidth_khz: f32) -> bool {
        if self.history.len() < MIN_HISTORY {
            return false;
        }
        let df = self.sample_rate as f64 / self.frame_len as f64;
        let centre = channel_khz as f64 * 1000.0;
        let half = bandwidth_khz as f64 * 500.0;
        let lo = ((centre - half) / df).round().max(1.0);
        let hi = ((centre + half) / df).round().min((self.nbins - 1) as f64);
        if lo > hi {
            return false;
        }
        let (lo, hi) = (lo as usize, hi as usize);
        let bins = &self.level[lo..=hi];
        let level = bins.iter().sum::<f32>() / bins.len() as f32;
        let mut floors: Vec<f32> = self.floor[lo..=hi].to_vec();
        let floor = median(&mut floors);
        db(level) > db(floor) + BUSY_DB
    }

    /// True when a start marker is found in the last second of audio.
    /// The marker is defined at 8000 Hz, so this is false at other rates.
    pub fn start_marker_seen(&self) -> bool {
        if (self.sample_rate - MARKER_RATE).abs() > 0.5 {
            return false;
        }
        let v: Vec<f32> = self.raw.iter().copied().collect();
        !marker::find_markers(&v, marker::DEFAULT_THRESHOLD).is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f32 = 8000.0;

    /// Deterministic uniform noise in [-amp, amp].
    fn noise(n: usize, amp: f32, seed: u64) -> Vec<f32> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s = s
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let u = (s >> 11) as f64 / (1u64 << 53) as f64;
                (amp as f64 * (2.0 * u - 1.0)) as f32
            })
            .collect()
    }

    fn tone(n: usize, hz: f32, amp: f32) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (2.0 * std::f32::consts::PI * hz * i as f32 / FS).sin())
            .collect()
    }

    /// A tone on top of background noise (a real input is never noise-free).
    fn tone_in_noise(n: usize, hz: f32, amp: f32, seed: u64) -> Vec<f32> {
        let t = tone(n, hz, amp);
        let w = noise(n, 0.01, seed);
        t.iter().zip(&w).map(|(a, b)| a + b).collect()
    }

    fn secs(s: f32) -> usize {
        (s * FS) as usize
    }

    #[test]
    fn sense_silence_is_not_busy() {
        let mut s = Sense::new(FS);
        s.push(&vec![0.0; secs(2.0)]);
        assert!(!s.channel_busy(1.5, 2.7));
    }

    #[test]
    fn sense_noise_is_not_busy() {
        let mut s = Sense::new(FS);
        s.push(&noise(secs(3.0), 0.01, 1));
        assert!(!s.channel_busy(1.5, 2.7));
    }

    #[test]
    fn sense_tone_in_channel_is_busy() {
        let mut s = Sense::new(FS);
        s.push(&noise(secs(1.5), 0.01, 2));
        assert!(!s.channel_busy(1.5, 2.7));
        s.push(&tone_in_noise(secs(1.0), 1500.0, 0.2, 7));
        assert!(s.channel_busy(1.5, 2.7));
    }

    #[test]
    fn sense_tone_outside_channel_is_not_busy() {
        let mut s = Sense::new(FS);
        s.push(&noise(secs(1.5), 0.01, 3));
        s.push(&tone_in_noise(secs(1.5), 3500.0, 0.2, 8));
        assert!(!s.channel_busy(1.5, 2.7));
    }

    #[test]
    fn sense_noise_floor_does_not_rise_after_long_signal() {
        let mut s = Sense::new(FS);
        s.push(&noise(secs(3.0), 0.01, 4));
        let before = s.noise_floor_db();
        s.push(&tone_in_noise(secs(2.0), 1500.0, 0.2, 6));
        s.push(&noise(secs(1.0), 0.01, 5));
        let after = s.noise_floor_db();
        assert!(
            (after - before).abs() < 1.0,
            "floor moved from {before} to {after} dB"
        );
    }
}
