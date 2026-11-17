//! Channel simulator + BLER harness.

use kahf_core::phy;
use rayon::prelude::*;

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
    const PAYLOAD_BYTES: usize = 36;
    let errors = (0..frames)
        .into_par_iter()
        .filter(|&i| {
            let trial_seed = seed ^ (i as u64).wrapping_mul(0xD1B5_4A32_D192_ED03);
            let mut rng = Rng::new(trial_seed);
            let payload: Vec<u8> = (0..PAYLOAD_BYTES)
                .map(|_| (rng.next_u64() >> 56) as u8)
                .collect();
            let info = kahf_core::frame::build_payload(&payload);
            let coded = kahf_core::ldpc::encode(&info);
            let tx = kahf_core::phy::modulate_frame(&coded, BLER_MOD);
            let rx = awgn(&tx, snr_db, trial_seed);
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

    #[test]
    fn same_seed_same_output() {
        let sig = vec![0.5f32; 64];
        assert_eq!(awgn(&sig, 6.0, 7), awgn(&sig, 6.0, 7));
        assert_ne!(awgn(&sig, 6.0, 7), awgn(&sig, 6.0, 8));
    }
}
