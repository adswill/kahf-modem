//! Channel simulator + BLER harness.
//!
//! Named channel profiles (plan section 1.3) are applied with [`apply`]:
//!
//! ```ignore
//! use kahf_sim::{apply, Profile};
//! // tx: modulated frame at 8000 Hz; snr_db is measured against the impaired signal.
//! let rx = apply(&tx, Profile::TwoPath, 10.0, seed);
//! ```
//!
//! Profiles (all at the PHY sample rate of 8000 Hz, see [`SAMPLE_RATE_HZ`]):
//! - `Awgn`: white noise only; identical to [`awgn`].
//! - `TwoPath`: a copy 2 ms later (16 samples) at -6 dB, added to the signal.
//! - `FreqOffset`: a shift of up to +-5 Hz (seed-derived, uniform). Exact: the
//!   FFT-based analytic signal is multiplied by `exp(i 2 pi shift t)`, real part kept.
//! - `ClockOffset`: 20 ppm sample clock error, read at `n * (1 + 20e-6)` with a
//!   16-tap Blackman-windowed sinc interpolator.
//! - `Fade`: slow amplitude fade at 0.5 Hz, 6 dB peak-to-trough depth (raised cosine).
//!
//! The impairment is applied first, then AWGN at `snr_db` is added, so every
//! profile takes the same SNR argument.
//!
//! The BLER harness (`run_point`) still uses plain AWGN. To sweep a profile in the
//! CLI, call `apply(&tx, profile, snr_db, seed)` in place of `awgn(&tx, snr_db, seed)`.

use kahf_core::phy;
use rayon::prelude::*;
use std::f64::consts::PI;

/// Small xorshift64* generator, seeded through splitmix64 so nearby seeds diverge.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        Rng(if z == 0 { 0x2545_F491_4F6C_DD1D } else { z })
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in (0, 1], so the logarithm in Box-Muller is always finite.
    fn uniform_open(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 1.0) * (1.0 / (1u64 << 53) as f64)
    }

    /// Standard normal sample via Box-Muller (one value per call).
    fn gaussian(&mut self) -> f64 {
        let u1 = self.uniform_open();
        let u2 = self.uniform_open();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
}

/// Add white Gaussian noise so that the SNR is `snr_db`, measured against the
/// mean power of `samples` (the whole buffer). Deterministic for a given seed.
pub fn awgn(samples: &[f32], snr_db: f32, seed: u64) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }
    let n = samples.len() as f64;
    let signal_power = samples.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>() / n;
    let noise_power = signal_power / 10f64.powf(snr_db as f64 / 10.0);
    let sigma = noise_power.sqrt();
    let mut rng = Rng::new(seed);
    samples
        .iter()
        .map(|&s| s + (sigma * rng.gaussian()) as f32)
        .collect()
}

/// PHY audio sample rate (Hz). Matches `phy.rs` FS.
pub const SAMPLE_RATE_HZ: f64 = 8000.0;

const TWO_PATH_DELAY_S: f64 = 0.002;
const TWO_PATH_ECHO_DB: f64 = -6.0;
const FREQ_OFFSET_MAX_HZ: f64 = 5.0;
const CLOCK_PPM: f64 = 20.0;
const FADE_HZ: f64 = 0.5;
const FADE_DEPTH_DB: f64 = 6.0;

/// Named channel profiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    Awgn,
    TwoPath,
    FreqOffset,
    ClockOffset,
    Fade,
    /// ITU-R F.1487 / CCIR 520 Watterson "good" channel: 2 paths, 0.5 ms, 0.1 Hz spread.
    WattersonGood,
    /// Watterson "moderate": 1 ms, 0.5 Hz spread.
    WattersonModerate,
    /// Watterson "poor": 2 ms, 1 Hz spread.
    WattersonPoor,
    /// Watterson "flutter" (high-latitude): 0.5 ms, 10 Hz spread.
    WattersonFlutter,
}

/// Apply a named channel profile, then AWGN at `snr_db` (measured against the
/// impaired signal). `Profile::Awgn` returns exactly `awgn(samples, snr_db, seed)`.
/// Deterministic for a given seed. Output length equals input length.
pub fn apply(samples: &[f32], profile: Profile, snr_db: f32, seed: u64) -> Vec<f32> {
    let impaired = match profile {
        Profile::Awgn => return awgn(samples, snr_db, seed),
        Profile::TwoPath => two_path(samples),
        Profile::FreqOffset => {
            // Offset in [-5, 5] Hz, derived from the seed on its own stream.
            let u = Rng::new(seed ^ 0xF0E1_D2C3_B4A5_9687).uniform_open();
            let shift = (2.0 * u - 1.0) * FREQ_OFFSET_MAX_HZ;
            freq_shift(samples, shift)
        }
        Profile::ClockOffset => clock_offset(samples),
        Profile::Fade => fade(samples),
        Profile::WattersonGood => watterson(samples, 0.0005, 0.1, seed),
        Profile::WattersonModerate => watterson(samples, 0.001, 0.5, seed),
        Profile::WattersonPoor => watterson(samples, 0.002, 1.0, seed),
        Profile::WattersonFlutter => watterson(samples, 0.0005, 10.0, seed),
    };
    awgn(&impaired, snr_db, seed)
}

/// Direct path plus a copy 2 ms later at -6 dB.
fn two_path(x: &[f32]) -> Vec<f32> {
    let d = (TWO_PATH_DELAY_S * SAMPLE_RATE_HZ).round() as usize;
    let g = 10f64.powf(TWO_PATH_ECHO_DB / 20.0) as f32;
    (0..x.len())
        .map(|n| if n >= d { x[n] + g * x[n - d] } else { x[n] })
        .collect()
}

/// In-place iterative radix-2 FFT. `buf.len()` must be a power of two. The
/// forward transform uses exp(-i...), the inverse exp(+i...); neither is scaled.
fn fft(buf: &mut [(f64, f64)], inverse: bool) {
    let n = buf.len();
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            buf.swap(i, j);
        }
    }
    let sign = if inverse { 1.0 } else { -1.0 };
    let mut len = 2;
    while len <= n {
        let half = len / 2;
        let step = sign * 2.0 * PI / len as f64;
        let twiddle: Vec<(f64, f64)> = (0..half)
            .map(|k| ((step * k as f64).cos(), (step * k as f64).sin()))
            .collect();
        for start in (0..n).step_by(len) {
            for k in 0..half {
                let (wr, wi) = twiddle[k];
                let (br, bi) = buf[start + k + half];
                let (tr, ti) = (br * wr - bi * wi, br * wi + bi * wr);
                let (ar, ai) = buf[start + k];
                buf[start + k] = (ar + tr, ai + ti);
                buf[start + k + half] = (ar - tr, ai - ti);
            }
        }
        len <<= 1;
    }
}

/// Real-signal frequency shift by `shift_hz`, exact at every frequency: the
/// FFT-based analytic signal z = x + i*H{x} is multiplied by exp(i*2*pi*shift*t),
/// and the real part is returned, so the output is `Re(z * e^{i w t})`.
///
/// The input is zero-padded to at least twice its length so the circular
/// Hilbert transform does not wrap around the buffer.
fn freq_shift(x: &[f32], shift_hz: f64) -> Vec<f32> {
    let z = analytic(x);
    z.iter()
        .enumerate()
        .map(|(i, &(zr, zi))| {
            let theta = 2.0 * PI * shift_hz * i as f64 / SAMPLE_RATE_HZ;
            (zr * theta.cos() - zi * theta.sin()) as f32
        })
        .collect()
}

/// Analytic signal z = x + i*H{x} (FFT method, zero-padded against wrap-around).
fn analytic(x: &[f32]) -> Vec<(f64, f64)> {
    let n = x.len();
    if n == 0 {
        return Vec::new();
    }
    let big = (2 * n).next_power_of_two();
    let mut spec = vec![(0.0f64, 0.0f64); big];
    for (s, &v) in spec.iter_mut().zip(x) {
        *s = (v as f64, 0.0);
    }
    fft(&mut spec, false);
    // Analytic signal: keep DC and Nyquist, double positive frequencies, drop negative.
    let half = big / 2;
    for s in spec.iter_mut().take(half).skip(1) {
        *s = (s.0 * 2.0, s.1 * 2.0);
    }
    for s in spec.iter_mut().skip(half + 1) {
        *s = (0.0, 0.0);
    }
    fft(&mut spec, true);
    let scale = 1.0 / big as f64;
    (0..n).map(|i| (spec[i].0 * scale, spec[i].1 * scale)).collect()
}

/// Number of sinusoids per Rayleigh tap in the sum-of-sinusoids fading generator.
const FADE_SINUSOIDS: usize = 64;

/// Complex Rayleigh gain process with a Gaussian Doppler power spectrum. `spread_hz`
/// is the two-sigma Doppler spread (ITU-R F.1487 definition). Unit mean power.
/// Sum of sinusoids at Gaussian-distributed frequencies with random phases.
fn rayleigh_gain(len: usize, spread_hz: f64, rng: &mut Rng) -> Vec<(f64, f64)> {
    let sigma = spread_hz / 2.0;
    let comps: Vec<(f64, f64)> = (0..FADE_SINUSOIDS)
        .map(|_| (2.0 * PI * sigma * rng.gaussian(), 2.0 * PI * rng.uniform_open()))
        .collect();
    let a = (1.0 / FADE_SINUSOIDS as f64).sqrt();
    (0..len)
        .map(|n| {
            let t = n as f64 / SAMPLE_RATE_HZ;
            comps.iter().fold((0.0, 0.0), |(re, im), &(w, ph)| {
                let th = w * t + ph;
                (re + a * th.cos(), im + a * th.sin())
            })
        })
        .collect()
}

/// Watterson two-path channel: two independent Rayleigh paths of equal power, the second
/// delayed by `delay_s`, each with Gaussian Doppler spread `spread_hz` (two-sigma).
/// Applied to the analytic signal; the real part is returned. Output power averages
/// to the input power.
fn watterson(x: &[f32], delay_s: f64, spread_hz: f64, seed: u64) -> Vec<f32> {
    let z = analytic(x);
    let n = z.len();
    let d = (delay_s * SAMPLE_RATE_HZ).round() as usize;
    let mut rng = Rng::new(seed ^ 0x57A7_7E55_0000_0001);
    let g1 = rayleigh_gain(n, spread_hz, &mut rng);
    let g2 = rayleigh_gain(n, spread_hz, &mut rng);
    let h = std::f64::consts::FRAC_1_SQRT_2;
    (0..n)
        .map(|i| {
            let (zr, zi) = z[i];
            let mut re = h * (g1[i].0 * zr - g1[i].1 * zi);
            if i >= d {
                let (dr, di) = z[i - d];
                re += h * (g2[i].0 * dr - g2[i].1 * di);
            }
            re as f32
        })
        .collect()
}

/// Half-length of the windowed-sinc interpolator in input samples (16 taps).
const SINC_HALF: isize = 8;

/// Blackman-windowed sinc at offset `d` (input samples), zero outside +-SINC_HALF.
fn windowed_sinc(d: f64) -> f64 {
    let h = SINC_HALF as f64;
    if d.abs() >= h {
        return 0.0;
    }
    let sinc = if d == 0.0 { 1.0 } else { (PI * d).sin() / (PI * d) };
    let w = 0.42 + 0.5 * (PI * d / h).cos() + 0.08 * (2.0 * PI * d / h).cos();
    sinc * w
}

/// Sample clock error of `CLOCK_PPM`: output n reads the input at position
/// n * (1 + ppm*1e-6), interpolated with a 16-tap Blackman-windowed sinc. Taps
/// that fall outside the buffer are dropped and the rest renormalised, so
/// constant signals stay constant. Past the end of the input the output is zero.
fn clock_offset(x: &[f32]) -> Vec<f32> {
    let n = x.len();
    let ratio = 1.0 + CLOCK_PPM * 1e-6;
    (0..n)
        .map(|i| {
            let pos = i as f64 * ratio;
            if pos >= n as f64 {
                return 0.0;
            }
            let i0 = pos.floor() as isize;
            let (mut acc, mut wsum) = (0.0f64, 0.0f64);
            for k in (-SINC_HALF + 1)..=SINC_HALF {
                let j = i0 + k;
                if j < 0 || j as usize >= n {
                    continue;
                }
                let w = windowed_sinc(pos - j as f64);
                acc += w * x[j as usize] as f64;
                wsum += w;
            }
            (if wsum != 0.0 { acc / wsum } else { 0.0 }) as f32
        })
        .collect()
}

/// Slow fade at `FADE_HZ`: gain goes from 1 down to 10^(-depth/20) and back,
/// starting at full gain at sample 0.
fn fade(x: &[f32]) -> Vec<f32> {
    let floor = 10f64.powf(-FADE_DEPTH_DB / 20.0);
    x.iter()
        .enumerate()
        .map(|(i, &s)| {
            let t = i as f64 / SAMPLE_RATE_HZ;
            let g = floor + (1.0 - floor) * 0.5 * (1.0 + (2.0 * PI * FADE_HZ * t).cos());
            (s as f64 * g) as f32
        })
        .collect()
}

/// Modulation used by the BLER harness (same as the CLI).
const BLER_MOD: phy::Modulation = phy::Modulation::Qpsk;
/// LDPC decoder iteration cap (same as the CLI).
const BLER_MAX_ITER: usize = 50;

/// Effective Eb/N0 (dB) for a point whose `snr_db` is the `awgn` SNR.
///
/// `awgn` sets noise variance per real sample against the mean sample power P.
/// For real white noise at sample rate fs, N0 = 2*sigma^2/fs, and the energy per
/// info bit is Eb = P * (M/fs) / K, where M is samples per frame and K info bits.
/// So Eb/N0 = SNR + 10*log10(M / (2*K)). M is measured from the modulator, so the
/// figure is exact for the current PHY. (The naive "SNR - 10*log10(2)" only holds
/// when M = 2K, which is not true here.)
pub fn eb_n0_db(snr_db: f32, samples_per_frame: usize, info_bits: usize) -> f32 {
    snr_db + 10.0 * ((samples_per_frame as f32) / (2.0 * info_bits as f32)).log10()
}

/// Monte-Carlo BLER point at awgn SNR `snr_db` (per real sample, see `awgn`).
///
/// Each trial: random 36-byte payload -> `frame::build_payload` -> `ldpc::encode`
/// -> `phy::modulate_frame` (QPSK) -> `awgn` -> `phy::demodulate_frame` ->
/// `ldpc::decode` (50 iterations) -> `frame::parse_payload`, and it is an error
/// unless the recovered bytes equal the original payload. Trials run in parallel,
/// and each trial's RNG is derived from `seed` and its index, so results are
/// deterministic for a given (snr_db, frames, seed).
///
/// Returns (frame errors, frames).
pub fn run_point(snr_db: f32, frames: usize, seed: u64) -> (usize, usize) {
    run_point_profile(Profile::Awgn, snr_db, frames, seed)
}

/// Monte-Carlo frame error point on a named channel profile. Same trial as `run_point`,
/// but the modulated frame goes through `apply(profile, snr_db)` instead of plain AWGN.
/// For `Profile::Awgn` the result equals `run_point`.
///
/// Returns (frame errors, frames).
pub fn run_point_profile(profile: Profile, snr_db: f32, frames: usize, seed: u64) -> (usize, usize) {
    const PAYLOAD_BYTES: usize = 36;
    let errors = (0..frames)
        .into_par_iter()
        .filter(|&i| {
            let trial_seed = seed ^ (i as u64).wrapping_mul(0xD1B5_4A32_D192_ED03);
            let mut rng = Rng::new(trial_seed);
            let payload: Vec<u8> = (0..PAYLOAD_BYTES).map(|_| (rng.next_u64() >> 56) as u8).collect();
            let info = kahf_core::frame::build_payload(&payload);
            let coded = kahf_core::ldpc::encode(&info);
            let tx = kahf_core::phy::modulate_frame(&coded, BLER_MOD);
            let rx = apply(&tx, profile, snr_db, trial_seed);
            let decoded = kahf_core::phy::demodulate_frame(&rx, BLER_MOD, kahf_core::ldpc::N)
                .and_then(|llr| kahf_core::ldpc::decode(&llr, BLER_MAX_ITER))
                .and_then(|bits| kahf_core::frame::parse_payload(&bits));
            decoded.as_deref() != Some(payload.as_slice())
        })
        .count();
    (errors, frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_power_matches_target_snr() {
        // Unit-power square wave (+1/-1 in runs of 3), so signal power is exactly 1.
        let n = 200_000usize;
        let sig: Vec<f32> = (0..n)
            .map(|i| if (i / 3) % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        for &snr in &[0.0f32, 10.0, 20.0] {
            let out = awgn(&sig, snr, 42);
            let noise_power: f64 = out
                .iter()
                .zip(&sig)
                .map(|(&o, &s)| {
                    let d = (o - s) as f64;
                    d * d
                })
                .sum::<f64>()
                / n as f64;
            let target = 10f64.powf(-snr as f64 / 10.0);
            let err_db = 10.0 * (noise_power / target).log10();
            assert!(
                err_db.abs() < 0.5,
                "snr {snr} dB: measured noise off by {err_db:.3} dB"
            );
        }
    }

    fn tone(n: usize, hz: f64) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * PI * hz * i as f64 / SAMPLE_RATE_HZ).sin() as f32)
            .collect()
    }

    const ALL: [Profile; 5] = [
        Profile::Awgn,
        Profile::TwoPath,
        Profile::FreqOffset,
        Profile::ClockOffset,
        Profile::Fade,
    ];

    #[test]
    fn awgn_profile_matches_awgn() {
        let sig = tone(4000, 1000.0);
        assert_eq!(apply(&sig, Profile::Awgn, 10.0, 5), awgn(&sig, 10.0, 5));
    }

    #[test]
    fn every_profile_keeps_power_finite() {
        let sig = tone(16_000, 1000.0);
        for p in ALL {
            let out = apply(&sig, p, 12.0, 9);
            assert_eq!(out.len(), sig.len(), "{p:?} length");
            let power: f64 = out.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / out.len() as f64;
            assert!(power.is_finite() && power > 0.0, "{p:?} power {power}");
        }
    }

    #[test]
    fn empty_input_gives_empty_output() {
        for p in ALL {
            assert!(apply(&[], p, 10.0, 1).is_empty(), "{p:?}");
        }
    }

    #[test]
    fn two_path_echo_is_16_samples_at_minus_6_db() {
        // Impulse at 0; SNR 200 dB makes the added noise negligible.
        let mut sig = vec![0.0f32; 64];
        sig[0] = 1.0;
        let out = apply(&sig, Profile::TwoPath, 200.0, 3);
        assert!((out[0] - 1.0).abs() < 1e-4, "direct {}", out[0]);
        assert!((out[16] - 0.501).abs() < 1e-3, "echo {}", out[16]);
        assert!(out[15].abs() < 1e-4 && out[17].abs() < 1e-4);
    }

    #[test]
    fn fade_depth_is_6_db_over_one_period() {
        // 0.5 Hz period = 16000 samples at 8 kHz.
        let sig = vec![1.0f32; 16_001];
        let out = apply(&sig, Profile::Fade, 200.0, 3);
        let max = out.iter().cloned().fold(f32::MIN, f32::max);
        let min = out.iter().cloned().fold(f32::MAX, f32::min);
        assert!((max - 1.0).abs() < 1e-3, "max {max}");
        assert!((min - 0.501).abs() < 1e-2, "min {min}");
    }

    #[test]
    fn freq_shift_preserves_tone_amplitude() {
        // 2 kHz tone (a quarter of the sample rate) sits well inside the Hilbert band.
        let sig = tone(8000, 2000.0);
        let out = freq_shift(&sig, 5.0);
        let rms = |v: &[f32]| (v.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / v.len() as f64).sqrt();
        let inner = 100..7900;
        let r_in = rms(&sig[inner.clone()]);
        let r_out = rms(&out[inner]);
        assert!((r_out / r_in - 1.0).abs() < 0.05, "ratio {}", r_out / r_in);
    }

    #[test]
    fn clock_offset_keeps_constant_signal() {
        let sig = vec![0.25f32; 1000];
        let out = clock_offset(&sig);
        for &v in &out[..999] {
            assert!((v - 0.25).abs() < 1e-6);
        }
    }

    /// Mean power of `v` over `range`.
    fn power(v: &[f32], range: std::ops::Range<usize>) -> f64 {
        let s = &v[range];
        s.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>() / s.len() as f64
    }

    /// Amplitude of a `hz` tone in `v` over `range`, by projection onto sin and cos.
    fn amplitude_at(v: &[f32], hz: f64, range: std::ops::Range<usize>) -> f64 {
        let len = range.len() as f64;
        let (mut a, mut b) = (0.0f64, 0.0f64);
        for i in range {
            let ph = 2.0 * PI * hz * i as f64 / SAMPLE_RATE_HZ;
            a += v[i] as f64 * ph.sin();
            b += v[i] as f64 * ph.cos();
        }
        2.0 * (a * a + b * b).sqrt() / len
    }

    #[test]
    fn freq_shift_1khz_tone_by_5hz_within_half_percent_power() {
        // A 1 kHz tone shifted by +5 Hz must be a 1005 Hz tone. The residual
        // (output minus expected, phase included) must be under 0.5 % of the
        // expected tone's power. A plain power comparison cannot see phase error.
        let sig = tone(8000, 1000.0);
        let out = freq_shift(&sig, 5.0);
        let expected = tone(8000, 1005.0);
        let inner = 100..7900;
        let resid: Vec<f32> = out.iter().zip(&expected).map(|(&o, &e)| o - e).collect();
        let err = power(&resid, inner.clone()) / power(&expected, inner.clone());
        assert!(
            err < 0.005,
            "residual power {:.4} % of tone (limit 0.5 %)",
            err * 100.0
        );
    }

    #[test]
    fn clock_offset_2khz_tone_amplitude_within_half_percent() {
        // 20 ppm clock error turns a 2 kHz tone into 2 kHz * (1 + 20e-6) at amplitude 1.
        let sig = tone(8000, 2000.0);
        let out = clock_offset(&sig);
        let expected_hz = 2000.0 * (1.0 + CLOCK_PPM * 1e-6);
        let amp = amplitude_at(&out, expected_hz, 100..7900);
        let err = (amp - 1.0).abs();
        assert!(
            err < 0.005,
            "amplitude residual {:.4} % (limit 0.5 %)",
            err * 100.0
        );
    }

    #[test]
    fn same_seed_same_output() {
        let sig = vec![0.5f32; 64];
        assert_eq!(awgn(&sig, 6.0, 7), awgn(&sig, 6.0, 7));
        assert_ne!(awgn(&sig, 6.0, 7), awgn(&sig, 6.0, 8));
    }

    /// Manual measurement, not run in CI. Set KAHF_FER_PROFILE (awgn, two-path, freq-offset,
    /// clock-offset, fade), KAHF_FER_SNR and KAHF_FER_FRAMES, then run with --ignored.
    #[test]
    #[ignore]
    fn measure_profile_fer() {
        let name = std::env::var("KAHF_FER_PROFILE").unwrap_or_else(|_| "two-path".into());
        let snr: f32 = std::env::var("KAHF_FER_SNR")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.0);
        let frames: usize = std::env::var("KAHF_FER_FRAMES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2000);
        let seed: u64 = std::env::var("KAHF_FER_SEED")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);
        let profile = match name.as_str() {
            "awgn" => Profile::Awgn,
            "two-path" => Profile::TwoPath,
            "freq-offset" => Profile::FreqOffset,
            "clock-offset" => Profile::ClockOffset,
            "fade" => Profile::Fade,
            other => panic!("unknown profile {other}"),
        };
        let (errors, n) = run_point_profile(profile, snr, frames, seed);
        println!(
            "FER profile={name} snr={snr} dB frames={n} errors={errors} fer={:.4}",
            errors as f64 / n as f64
        );
    }

    #[test]
    fn watterson_keeps_length_is_deterministic_and_unit_mean_power() {
        let n = 80_000usize;
        let sig: Vec<f32> = (0..n)
            .map(|i| (2.0 * PI * 1500.0 * i as f64 / SAMPLE_RATE_HZ).sin() as f32)
            .collect();
        let sp = sig.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / n as f64;
        // Mean over many independent fades is the input power (unit gain channel).
        let mut acc = 0.0;
        let runs = 60u64;
        for seed in 0..runs {
            let y = apply(&sig, Profile::WattersonModerate, 200.0, seed);
            assert_eq!(y.len(), n);
            acc += y.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / n as f64;
        }
        let ratio = acc / runs as f64 / sp;
        assert!((0.6..1.6).contains(&ratio), "mean power ratio {ratio}");
        let a = apply(&sig, Profile::WattersonPoor, 10.0, 5);
        let b = apply(&sig, Profile::WattersonPoor, 10.0, 5);
        assert_eq!(a, b);
    }
}
