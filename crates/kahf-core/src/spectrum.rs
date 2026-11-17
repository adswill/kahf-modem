//! High-resolution spectrum analyser for the GUI and engine.
//!
//! Hann-windowed power spectrum, 50 % overlap between frames, averaged over
//! the last four frames. Uses its own radix-2 FFT; no external dependencies.

use std::collections::VecDeque;
use std::f64::consts::PI;

/// Default FFT length used by the GUI and engine.
pub const DEFAULT_FFT_SIZE: usize = 2048;
/// Number of frames averaged in `spectrum_db`.
const AVG_FRAMES: usize = 4;
/// Lowest dB value returned by `spectrum_db`.
const FLOOR_DB: f32 = -140.0;
/// Waterfall dB range mapped onto 0..=255.
const WF_MIN_DB: f32 = -120.0;
const WF_MAX_DB: f32 = -20.0;

#[derive(Clone, Copy, Debug, Default)]
struct Cpx {
    re: f64,
    im: f64,
}

/// Iterative radix-2 forward FFT plan with precomputed twiddles and bit reversal.
struct Fft {
    n: usize,
    tw: Vec<Cpx>,
    rev: Vec<usize>,
}

impl Fft {
    fn new(n: usize) -> Fft {
        let bits = n.trailing_zeros();
        let tw = (0..n / 2)
            .map(|k| {
                let a = -2.0 * PI * k as f64 / n as f64;
                Cpx {
                    re: a.cos(),
                    im: a.sin(),
                }
            })
            .collect();
        let rev = (0..n)
            .map(|i| {
                if bits == 0 {
                    0
                } else {
                    i.reverse_bits() >> (usize::BITS - bits)
                }
            })
            .collect();
        Fft { n, tw, rev }
    }

    /// In-place forward transform. `a.len()` must equal the plan size.
    fn forward(&self, a: &mut [Cpx]) {
        let n = self.n;
        for (i, &j) in self.rev[..n].iter().enumerate() {
            if i < j {
                a.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let step = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..half {
                    let w = self.tw[k * step];
                    let x = a[start + k + half];
                    let v = Cpx {
                        re: x.re * w.re - x.im * w.im,
                        im: x.re * w.im + x.im * w.re,
                    };
                    let u = a[start + k];
                    a[start + k] = Cpx {
                        re: u.re + v.re,
                        im: u.im + v.im,
                    };
                    a[start + k + half] = Cpx {
                        re: u.re - v.re,
                        im: u.im - v.im,
                    };
                }
            }
            len <<= 1;
        }
    }
}

/// Streaming spectrum analyser. Feed samples with `push`, read with `spectrum_db`.
pub struct Analyser {
    fft_size: usize,
    sample_rate: f32,
    hop: usize,
    window: Vec<f64>,
    /// Power normalisation so a full-scale sine in one bin reads 0 dB.
    scale: f64,
    fft: Fft,
    /// Most recent `fft_size` samples.
    buf: VecDeque<f32>,
    /// Total samples received.
    seen: usize,
    /// Power spectra of the last `AVG_FRAMES` frames (fft_size / 2 bins each).
    frames: VecDeque<Vec<f64>>,
    work: Vec<Cpx>,
}

impl Analyser {
    /// `fft_size` must be a power of two, at least 4.
    pub fn new(fft_size: usize, sample_rate: f32) -> Analyser {
        assert!(
            fft_size >= 4 && fft_size.is_power_of_two(),
            "fft_size must be a power of two >= 4"
        );
        let window: Vec<f64> = (0..fft_size)
            .map(|n| 0.5 - 0.5 * (2.0 * PI * n as f64 / fft_size as f64).cos())
            .collect();
        let wsum: f64 = window.iter().sum();
        Analyser {
            fft_size,
            sample_rate,
            hop: fft_size / 2,
            window,
            scale: 4.0 / (wsum * wsum),
            fft: Fft::new(fft_size),
            buf: VecDeque::with_capacity(fft_size),
            seen: 0,
            frames: VecDeque::with_capacity(AVG_FRAMES),
            work: vec![Cpx::default(); fft_size],
        }
    }

    /// Append samples. A new frame is computed every `fft_size / 2` samples once
    /// a full frame has been received.
    pub fn push(&mut self, samples: &[f32]) {
        for &s in samples {
            if self.buf.len() == self.fft_size {
                self.buf.pop_front();
            }
            self.buf.push_back(s);
            self.seen += 1;
            if self.seen >= self.fft_size && (self.seen - self.fft_size).is_multiple_of(self.hop) {
                self.process_frame();
            }
        }
    }

    fn process_frame(&mut self) {
        for (i, (x, w)) in self.buf.iter().zip(self.window.iter()).enumerate() {
            self.work[i] = Cpx {
                re: *x as f64 * w,
                im: 0.0,
            };
        }
        self.fft.forward(&mut self.work);
        let p: Vec<f64> = self.work[..self.fft_size / 2]
            .iter()
            .map(|c| (c.re * c.re + c.im * c.im) * self.scale)
            .collect();
        if self.frames.len() == AVG_FRAMES {
            self.frames.pop_front();
        }
        self.frames.push_back(p);
    }

    /// Averaged power spectrum in dB, `fft_size / 2` bins, floored at -140 dB.
    /// Returns None until a full frame has been received.
    pub fn spectrum_db(&mut self) -> Option<Vec<f32>> {
        if self.frames.is_empty() {
            return None;
        }
        let half = self.fft_size / 2;
        let mut acc = vec![0.0f64; half];
        for f in &self.frames {
            for (a, p) in acc.iter_mut().zip(f.iter()) {
                *a += p;
            }
        }
        let n = self.frames.len() as f64;
        Some(
            acc.iter()
                .map(|a| ((10.0 * (a / n).max(1e-14).log10()) as f32).max(FLOOR_DB))
                .collect(),
        )
    }

    /// Centre frequency of a bin in Hz.
    pub fn bin_to_hz(&self, bin: usize) -> f32 {
        bin as f32 * self.sample_rate / self.fft_size as f32
    }

    /// Index and value (dB) of the largest bin. Returns (0, -140.0) for an empty slice.
    pub fn peak(&self, spectrum: &[f32]) -> (usize, f32) {
        let mut best = (0usize, FLOOR_DB);
        for (i, &v) in spectrum.iter().enumerate() {
            if i == 0 || v > best.1 {
                best = (i, v);
            }
        }
        best
    }
}

/// Reduce a spectrum to `width` columns (max of each group of bins) and map
/// -120 dB..-20 dB onto 0..=255. Returns `width` bytes.
pub fn waterfall_row(spectrum: &[f32], width: usize) -> Vec<u8> {
    if width == 0 {
        return Vec::new();
    }
    let n = spectrum.len();
    (0..width)
        .map(|c| {
            let (lo, hi) = if n == 0 {
                (0, 0)
            } else {
                let lo = (c * n / width).min(n - 1);
                let hi = ((c + 1) * n / width).max(lo + 1).min(n);
                (lo, hi)
            };
            let db = spectrum[lo..hi].iter().fold(f32::NEG_INFINITY, |m, &v| m.max(v));
            let db = if db.is_finite() { db } else { WF_MIN_DB };
            let t = (db - WF_MIN_DB) / (WF_MAX_DB - WF_MIN_DB);
            (t * 255.0).round().clamp(0.0, 255.0) as u8
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
        }
    }

    #[test]
    fn spectrum_none_before_full_frame() {
        let mut a = Analyser::new(2048, 8000.0);
        assert!(a.spectrum_db().is_none());
        a.push(&vec![0.1f32; 2047]);
        assert!(a.spectrum_db().is_none());
        a.push(&[0.1]);
        let s = a.spectrum_db().expect("frame ready at fft_size samples");
        assert_eq!(s.len(), 1024);
    }

    #[test]
    fn sine_1khz_peaks_at_1khz_bin() {
        let fs = 8000.0f32;
        let mut a = Analyser::new(2048, fs);
        let x: Vec<f32> = (0..8192)
            .map(|n| (2.0 * PI as f32 * 1000.0 * n as f32 / fs).sin() * 0.5)
            .collect();
        a.push(&x);
        let s = a.spectrum_db().unwrap();
        let (idx, db) = a.peak(&s);
        assert_eq!(idx, 256, "1 kHz at 8 kHz / 2048 point should be bin 256");
        assert!((a.bin_to_hz(idx) - 1000.0).abs() <= fs / 2048.0);
        // Amplitude 0.5 sine: about -6 dB at the peak.
        assert!((db + 6.0).abs() < 1.5, "peak {db} dB");
    }

    #[test]
    fn white_noise_floor_stable_over_frames() {
        let mut a = Analyser::new(2048, 8000.0);
        let mut rng = Lcg(12345);
        let mut block = |n: usize| (0..n).map(|_| rng.next() * 0.3).collect::<Vec<f32>>();
        a.push(&block(2048));
        let mut means = Vec::new();
        for _ in 0..10 {
            let s = a.spectrum_db().unwrap();
            let m: f32 = s[1..].iter().sum::<f32>() / (s.len() - 1) as f32;
            means.push(m);
            a.push(&block(1024));
        }
        let max = means.iter().cloned().fold(f32::MIN, f32::max);
        let min = means.iter().cloned().fold(f32::MAX, f32::min);
        assert!(max - min <= 3.0, "floor drifted {:.2} dB: {means:?}", max - min);
    }

    #[test]
    fn waterfall_row_width_and_mapping() {
        let spec: Vec<f32> = (0..1024).map(|i| -140.0 + i as f32 * 0.2).collect();
        assert_eq!(waterfall_row(&spec, 64).len(), 64);
        assert_eq!(waterfall_row(&spec, 1000).len(), 1000);
        assert_eq!(waterfall_row(&spec, 0).len(), 0);
        let row = waterfall_row(&[-120.0, -20.0, -200.0, 0.0], 4);
        assert_eq!(row, vec![0, 255, 0, 255]);
    }
}
