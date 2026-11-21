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
    /// Alias of `WattersonGood`, kept for the lane and ladder examples.
    HfGood,
    /// Alias of `WattersonModerate`.
    HfModerate,
    /// Alias of `WattersonPoor`.
    HfPoor,
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
        Profile::HfGood | Profile::WattersonGood => watterson(samples, 0.0005, 0.1, seed),
        Profile::HfModerate | Profile::WattersonModerate => watterson(samples, 0.001, 0.5, seed),
        Profile::HfPoor | Profile::WattersonPoor => watterson(samples, 0.002, 1.0, seed),
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
/// The analytic signal of `x` (the real signal plus i times its Hilbert transform).
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
    let half = big / 2;
    for s in spec.iter_mut().take(half).skip(1) {
        *s = (s.0 * 2.0, s.1 * 2.0);
    }
    for s in spec.iter_mut().skip(half + 1) {
        *s = (0.0, 0.0);
    }
    fft(&mut spec, true);
    let scale = 1.0 / big as f64;
    spec.into_iter().take(n).map(|(r, i)| (r * scale, i * scale)).collect()
}

/// A complex fading process of `len` samples at 8 kHz with a Gaussian Doppler spectrum of
/// total width `spread_hz` (the ITU "Doppler spread") and unit mean power. It is made at a low
/// rate (white complex noise through a Gaussian filter) and interpolated up.
fn fading_process(len: usize, spread_hz: f64, rng: &mut Rng) -> Vec<(f64, f64)> {
    const LOW_HZ: f64 = 50.0;
    let step = SAMPLE_RATE_HZ / LOW_HZ;
    let sigma_f = spread_hz / 2.0;
    // Filter impulse response h(t) ~ exp(-2 pi^2 sigma_f^2 t^2), out to four time constants.
    let tau = 1.0 / (2.0 * PI * sigma_f);
    let half = (4.0 * tau * LOW_HZ).ceil() as isize;
    let taps: Vec<f64> = (-half..=half).map(|k| (-((k as f64 / LOW_HZ) / tau).powi(2) / 2.0).exp()).collect();
    let norm = taps.iter().map(|t| t * t).sum::<f64>().sqrt();
    let n_low = (len as f64 / step).ceil() as usize + 2;
    let noise: Vec<(f64, f64)> = (0..n_low + 2 * half as usize)
        .map(|_| (rng.gaussian() / 2f64.sqrt(), rng.gaussian() / 2f64.sqrt()))
        .collect();
    let low: Vec<(f64, f64)> = (0..n_low)
        .map(|i| {
            let (mut re, mut im) = (0.0, 0.0);
            for (j, t) in taps.iter().enumerate() {
                re += t * noise[i + j].0;
                im += t * noise[i + j].1;
            }
            (re / norm, im / norm)
        })
        .collect();
    (0..len)
        .map(|n| {
            let pos = n as f64 / step;
            let i = pos.floor() as usize;
            let f = pos - i as f64;
            (low[i].0 * (1.0 - f) + low[i + 1].0 * f, low[i].1 * (1.0 - f) + low[i + 1].1 * f)
        })
        .collect()
}

/// The Watterson HF channel (ITU-R F.1487): two independent Rayleigh-fading paths of equal
/// mean power, `delay_s` apart, each with a Gaussian Doppler spectrum of width `spread_hz`.
fn watterson(x: &[f32], delay_s: f64, spread_hz: f64, seed: u64) -> Vec<f32> {
    let n = x.len();
    if n == 0 {
        return Vec::new();
    }
    let a = analytic(x);
    let d = (delay_s * SAMPLE_RATE_HZ).round() as usize;
    let mut rng = Rng::new(seed ^ 0x5741_5454_4552_534F);
    let g1 = fading_process(n, spread_hz, &mut rng);
    let g2 = fading_process(n, spread_hz, &mut rng);
    let s = 1.0 / 2f64.sqrt();
    (0..n)
        .map(|i| {
            let p1 = (g1[i].0 * a[i].0 - g1[i].1 * a[i].1, g1[i].0 * a[i].1 + g1[i].1 * a[i].0);
            let p2 = if i >= d {
                let b = a[i - d];
                (g2[i].0 * b.0 - g2[i].1 * b.1, g2[i].0 * b.1 + g2[i].1 * b.0)
            } else {
                (0.0, 0.0)
            };
            ((p1.0 + p2.0) * s) as f32
        })
        .collect()
}

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
    run_point_mod(BLER_MOD, profile, snr_db, frames, seed)
}

/// As `run_point_profile`, for any modulation (the frame carries that modulation's id).
pub fn run_point_mod(m: phy::Modulation, profile: Profile, snr_db: f32, frames: usize, seed: u64) -> (usize, usize) {
    run_point_with(m, frames, seed, |tx, trial_seed| apply(tx, profile, snr_db, trial_seed))
}

/// The most general sweep: `channel` turns each transmitted frame into what the receiver gets.
pub fn run_point_with<F>(m: phy::Modulation, frames: usize, seed: u64, channel: F) -> (usize, usize)
where
    F: Fn(&[f32], u64) -> Vec<f32> + Sync,
{
    let id = match m {
        phy::Modulation::Qpsk => 0,
        phy::Modulation::Qam16 => 1,
        phy::Modulation::Qam64 => 2,
        phy::Modulation::Kami32 => kahf_core::kami::ID,
        phy::Modulation::Kami16 => kahf_core::kami::ID16,
        phy::Modulation::Kami8 => kahf_core::kami::ID8,
        phy::Modulation::Kami8H => kahf_core::kami::ID8H,
        phy::Modulation::Kami64 => kahf_core::kami::ID64,
        phy::Modulation::Kami128 => kahf_core::kami::ID128,
        phy::Modulation::Kami256 => kahf_core::kami::ID256,
        phy::Modulation::Kami4 => kahf_core::kami::ID4,
        phy::Modulation::Kami2 => kahf_core::kami::ID2,
        phy::Modulation::Kami1a => kahf_core::kami::ID1A,
        phy::Modulation::Kami1b => kahf_core::kami::ID1B,
        phy::Modulation::Kami1c => kahf_core::kami::ID1C,
        phy::Modulation::Kami1d => kahf_core::kami::ID1D,
    };
    const PAYLOAD_BYTES: usize = 36;
    let errors = (0..frames)
        .into_par_iter()
        .filter(|&i| {
            let trial_seed = seed ^ (i as u64).wrapping_mul(0xD1B5_4A32_D192_ED03);
            let mut rng = Rng::new(trial_seed);
            let payload: Vec<u8> = (0..PAYLOAD_BYTES).map(|_| (rng.next_u64() >> 56) as u8).collect();
            let info = kahf_core::frame::build_payload_as(&payload, id);
            let coded = kahf_core::ldpc::encode(&info);
            let tx = kahf_core::phy::modulate_frame(&coded, m);
            let rx = channel(&tx, trial_seed);
            let decoded = kahf_core::phy::demodulate_frame(&rx, m, kahf_core::ldpc::N)
                .and_then(|llr| kahf_core::ldpc::decode(&llr, BLER_MAX_ITER))
                .and_then(|bits| kahf_core::frame::parse_payload(&bits));
            decoded.as_deref() != Some(payload.as_slice())
        })
        .count();
    (errors, frames)
}

/// The Watterson channel with explicit parameters, then AWGN: for probing which impairment a
/// receiver cannot take. `delay_s` 0 makes the second path add in phase (a flat fade).
pub fn hf_channel(samples: &[f32], delay_s: f64, spread_hz: f64, snr_db: f32, seed: u64) -> Vec<f32> {
    awgn(&watterson(samples, delay_s, spread_hz, seed), snr_db, seed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two equal-strength Rayleigh paths 1 ms apart (a normal HF echo) must not break KAMI-32 at a
    /// good SNR. Before the receiver had an equalizer this lost more than 70 percent of the frames.
    #[test]
    fn kami_32_survives_an_hf_echo() {
        let (err, n) = run_point_with(phy::Modulation::Kami32, 20, 5, |tx, s| hf_channel(tx, 0.001, 0.05, 30.0, s));
        assert!(err * 10 <= n, "{err} of {n} frames lost through a 1 ms echo");
        let (err, n) = run_point_with(phy::Modulation::Kami32, 20, 6, |tx, s| hf_channel(tx, 0.002, 0.05, 30.0, s));
        assert!(err * 10 <= n, "{err} of {n} frames lost through a 2 ms echo");
    }

    #[test]
    fn the_hf_channel_keeps_mean_power_and_fades() {
        let n = 8000 * 120;
        let tone: Vec<f32> = (0..n).map(|i| (2.0 * std::f64::consts::PI * 1500.0 * i as f64 / 8000.0).sin() as f32).collect();
        let p_in: f64 = tone.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / n as f64;
        let out = watterson(&tone, 0.001, 0.5, 3);
        let p_out: f64 = out.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / n as f64;
        let db = 10.0 * (p_out / p_in).log10();
        assert!(db.abs() < 3.0, "mean power changed by {db:.1} dB");
        // Power in one-second blocks must vary: that is the fading.
        let blocks: Vec<f64> = out.chunks(8000).map(|c| c.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / 8000.0).collect();
        let (lo, hi) = blocks.iter().fold((f64::MAX, 0.0f64), |(l, h), &b| (l.min(b), h.max(b)));
        assert!(10.0 * (hi / lo).log10() > 6.0, "no fading: {lo:.3} to {hi:.3}");
    }

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
