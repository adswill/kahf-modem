//! Narrowband interference canceller for audio ahead of demodulation.
//!
//! Each pass takes the Hann-windowed spectrum of the current frame and looks for
//! a bin in the 300 to 2700 Hz band that is a local maximum and more than 15 dB
//! above the band median. The peak frequency is refined on the unwindowed frame,
//! a sinusoid (amplitude and phase) is fitted to the whole frame by least
//! squares and subtracted. The search repeats up to `max_tones` times.

use std::f64::consts::PI;

const BAND_LO_HZ: f64 = 300.0;
const BAND_HI_HZ: f64 = 2700.0;
/// A peak must exceed the band median by this many dB (in power).
const PEAK_THRESHOLD_DB: f64 = 15.0;
/// Frequency refinement grid: steps per DFT bin, searched over +/- 1 bin.
const REFINE_STEPS_PER_BIN: i32 = 64;
/// Absolute power floor so that silent frames never produce a detection.
const POWER_FLOOR: f64 = 1e-18;

/// Removes up to `max_tones` narrowband tones from a frame.
///
/// Returns the cleaned samples. When no tone is detected the input is returned
/// unchanged. Empty or very short input is returned as is.
pub fn cancel_tones(samples: &[f32], sample_rate: f32, max_tones: usize) -> Vec<f32> {
    let fs = sample_rate as f64;
    if samples.len() < 16 || max_tones == 0 || !fs.is_finite() || fs <= 0.0 {
        return samples.to_vec();
    }
    let mut x: Vec<f64> = samples.iter().map(|&s| s as f64).collect();
    let mut found = false;
    for _ in 0..max_tones {
        let Some(omega) = find_tone(&x, fs) else {
            break;
        };
        remove_sinusoid(&mut x, omega);
        found = true;
    }
    if !found {
        return samples.to_vec();
    }
    x.iter().map(|&v| v as f32).collect()
}

/// Returns the angular frequency (radians per sample) of the strongest qualifying
/// in-band tone, or `None` if no bin clears the threshold.
fn find_tone(x: &[f64], fs: f64) -> Option<f64> {
    let n = x.len();
    let bin_hz = fs / n as f64;
    let k_lo = (BAND_LO_HZ / bin_hz).ceil() as usize;
    let k_hi = ((BAND_HI_HZ.min(fs / 2.0)) / bin_hz).floor() as usize;
    let k_hi = k_hi.min(n / 2 - 1);
    if k_lo < 1 || k_hi < k_lo {
        return None;
    }

    let win = hann(n);
    let windowed: Vec<f64> = x.iter().zip(&win).map(|(a, b)| a * b).collect();

    // p[i] is the power at bin lo + i, covering the band plus one neighbour per side.
    let lo = k_lo - 1;
    let p: Vec<f64> = (lo..=k_hi + 1)
        .map(|k| goertzel_power(&windowed, 2.0 * PI * k as f64 / n as f64))
        .collect();

    let mut band: Vec<f64> = p[1..=(k_hi - lo)].to_vec();
    let mid = band.len() / 2;
    band.select_nth_unstable_by(mid, |a, b| a.total_cmp(b));
    let median = band[mid];
    let threshold = (median * 10f64.powf(PEAK_THRESHOLD_DB / 10.0)).max(POWER_FLOOR);

    let mut best: Option<(usize, f64)> = None;
    for i in 1..p.len() - 1 {
        let v = p[i];
        if v > threshold && v >= p[i - 1] && v >= p[i + 1] && best.is_none_or(|(_, b)| v > b) {
            best = Some((i, v));
        }
    }
    let (i, _) = best?;

    // Parabolic interpolation on log power for a sub-bin starting estimate.
    let k = (lo + i) as f64;
    let a = p[i - 1].max(POWER_FLOOR).ln();
    let b = p[i].max(POWER_FLOOR).ln();
    let c = p[i + 1].max(POWER_FLOOR).ln();
    let denom = a - 2.0 * b + c;
    let delta = if denom.abs() > 1e-12 {
        (0.5 * (a - c) / denom).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    let centre = 2.0 * PI * (k + delta) / n as f64;

    // Refine on the unwindowed frame: maximise the projection energy.
    let step = 2.0 * PI / n as f64 / REFINE_STEPS_PER_BIN as f64;
    let mut best_w = centre;
    let mut best_p = f64::NEG_INFINITY;
    for j in -REFINE_STEPS_PER_BIN..=REFINE_STEPS_PER_BIN {
        let w = centre + j as f64 * step;
        let pw = goertzel_power(x, w);
        if pw > best_p {
            best_p = pw;
            best_w = w;
        }
    }
    Some(best_w)
}

/// Least-squares fit of `a cos(wn) + b sin(wn)` over the frame, subtracted in place.
fn remove_sinusoid(x: &mut [f64], omega: f64) {
    let (mut scc, mut sss, mut scs, mut xc, mut xs) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (n, &v) in x.iter().enumerate() {
        let (s, c) = (omega * n as f64).sin_cos();
        scc += c * c;
        sss += s * s;
        scs += c * s;
        xc += v * c;
        xs += v * s;
    }
    let det = scc * sss - scs * scs;
    if det.is_nan() || det <= 1e-12 * scc * sss {
        return;
    }
    let a = (sss * xc - scs * xs) / det;
    let b = (scc * xs - scs * xc) / det;
    for (n, v) in x.iter_mut().enumerate() {
        let (s, c) = (omega * n as f64).sin_cos();
        *v -= a * c + b * s;
    }
}

/// Power |sum x[n] e^{-j omega n}|^2 at an arbitrary angular frequency (Goertzel).
fn goertzel_power(x: &[f64], omega: f64) -> f64 {
    let c = 2.0 * omega.cos();
    let (mut s1, mut s2) = (0.0, 0.0);
    for &v in x {
        let s0 = v + c * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    s1 * s1 + s2 * s2 - c * s1 * s2
}

fn hann(n: usize) -> Vec<f64> {
    let denom = (n - 1) as f64;
    (0..n)
        .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f64 / denom).cos())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f32 = 8000.0;
    const N: usize = 4000;

    /// Deterministic uniform noise source in [-1, 1).
    struct Lcg(u64);

    impl Lcg {
        fn next_unit(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
        }
    }

    /// Summed power over the 300 to 2700 Hz band (2 Hz bins at N = 4000, fs = 8000).
    fn band_power(x: &[f32]) -> f64 {
        let v: Vec<f64> = x.iter().map(|&s| s as f64).collect();
        let n = v.len();
        (150..=1350)
            .map(|k| goertzel_power(&v, 2.0 * PI * k as f64 / n as f64))
            .sum()
    }

    #[test]
    fn interf_clean_frame_unchanged() {
        let mut rng = Lcg(1);
        let frame: Vec<f32> = (0..N).map(|_| (0.1 * rng.next_unit()) as f32).collect();
        let out = cancel_tones(&frame, FS, 4);
        assert_eq!(out.len(), N);
        let rms = (frame.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / N as f64).sqrt();
        let max_diff = frame
            .iter()
            .zip(&out)
            .map(|(a, b)| (a - b).abs() as f64)
            .fold(0.0, f64::max);
        assert!(max_diff <= 0.01 * rms, "max_diff {max_diff} rms {rms}");
    }

    #[test]
    fn interf_tone_1500_reduced_15db() {
        let mut rng = Lcg(7);
        let noise: Vec<f32> = (0..N).map(|_| (0.02 * rng.next_unit()) as f32).collect();
        let frame: Vec<f32> = (0..N)
            .map(|n| {
                let t = n as f64 / FS as f64;
                (0.5 * (2.0 * PI * 1500.0 * t).sin()) as f32 + noise[n]
            })
            .collect();
        let out = cancel_tones(&frame, FS, 4);
        let reduction_db = 10.0 * (band_power(&frame) / band_power(&out)).log10();
        assert!(reduction_db >= 15.0, "reduction {reduction_db:.1} dB");
    }

    #[test]
    fn interf_empty_input_no_panic() {
        assert!(cancel_tones(&[], FS, 4).is_empty());
        assert_eq!(cancel_tones(&[0.0; 10], FS, 4).len(), 10);
        assert_eq!(cancel_tones(&[0.5; 4000], FS, 0).len(), 4000);
    }
}
