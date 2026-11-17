//! Blind SNR estimator for received QPSK-style data symbols (no known symbols needed).
//!
//! Uses the M2M4 moment method on the real and imaginary parts of the received
//! constellation points. For a QPSK component `a = +-A` plus Gaussian noise of
//! variance `N/2` (where `S = 2A^2` is the signal power and `N` the noise power):
//!
//! ```text
//! m2 = E[x^2]  = (S + N) / 2
//! m4 = E[x^4]  = S^2/4 + 3SN/2 + 3N^2/4
//! ```
//!
//! Eliminating `N` gives `S^2 = 6*m2^2 - 2*m4`, and `N = 2*m2 - S`.

/// Minimum number of symbols needed for a meaningful estimate.
const MIN_SYMBOLS: usize = 50;

/// Estimate SNR in dB from received constellation points `(re, im)`.
///
/// Returns `None` for fewer than 50 symbols or when the moments are degenerate
/// (for example all-zero input, or noise so large the signal term is not positive).
/// A noiseless input returns a large finite value (capped near 60 dB) rather than infinity.
pub fn estimate_snr_db(symbols: &[(f32, f32)]) -> Option<f32> {
    if symbols.len() < MIN_SYMBOLS {
        return None;
    }

    // Pool the real and imaginary parts: 2*n samples.
    let mut s2 = 0.0f64;
    let mut s4 = 0.0f64;
    for &(re, im) in symbols {
        for x in [re as f64, im as f64] {
            let x2 = x * x;
            s2 += x2;
            s4 += x2 * x2;
        }
    }
    let count = (2 * symbols.len()) as f64;
    let m2 = s2 / count;
    let m4 = s4 / count;

    if m2.is_nan() || m2 <= 0.0 {
        return None;
    }

    let signal = 6.0 * m2 * m2 - 2.0 * m4;
    if signal.is_nan() || signal <= 0.0 {
        return None;
    }
    let signal = signal.sqrt();

    // Noise power, floored so a noiseless input gives a finite (capped) SNR.
    let noise = (2.0 * m2 - signal).max(m2 * 1e-6);
    let snr_db = 10.0 * (signal / noise).log10();
    Some(snr_db as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Small deterministic PRNG (xorshift64*) so tests need no extra crates.
    struct Rng(u64);
    impl Rng {
        fn next_u(&mut self) -> f64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            let v = self.0.wrapping_mul(0x2545_F491_4F6C_DD1D);
            ((v >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        }
        /// Standard normal via Box-Muller.
        fn gauss(&mut self) -> f64 {
            let u1 = self.next_u();
            let u2 = self.next_u();
            (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
        }
    }

    /// Unit-power QPSK symbols with complex Gaussian noise of total power `n_power`.
    fn qpsk_with_noise(count: usize, snr_db: f64, seed: u64) -> Vec<(f32, f32)> {
        let mut rng = Rng(seed);
        let n_power = 10f64.powf(-snr_db / 10.0);
        let per_axis_std = (n_power / 2.0).sqrt();
        let a = std::f64::consts::FRAC_1_SQRT_2;
        (0..count)
            .map(|_| {
                let re_s = if rng.next_u() < 0.5 { -a } else { a };
                let im_s = if rng.next_u() < 0.5 { -a } else { a };
                let re = re_s + per_axis_std * rng.gauss();
                let im = im_s + per_axis_std * rng.gauss();
                (re as f32, im as f32)
            })
            .collect()
    }

    #[test]
    fn noiseless_qpsk_is_high_snr() {
        let a = std::f32::consts::FRAC_1_SQRT_2;
        let pts: Vec<(f32, f32)> = (0..400)
            .map(|i| {
                let re = if i & 1 == 0 { a } else { -a };
                let im = if i & 2 == 0 { a } else { -a };
                (re, im)
            })
            .collect();
        let snr = estimate_snr_db(&pts).expect("should estimate");
        assert!(snr > 30.0, "noiseless SNR was {snr} dB");
    }

    #[test]
    fn qpsk_at_10db_is_within_1_5db() {
        let pts = qpsk_with_noise(20_000, 10.0, 0x1234_5678_9ABC_DEF1);
        let snr = estimate_snr_db(&pts).expect("should estimate");
        assert!((snr - 10.0).abs() < 1.5, "estimated {snr} dB, expected 10");
    }

    #[test]
    fn fewer_than_50_symbols_is_none() {
        let pts = qpsk_with_noise(49, 10.0, 42);
        assert!(estimate_snr_db(&pts).is_none());
        assert!(estimate_snr_db(&[]).is_none());
        let pts50 = qpsk_with_noise(50, 10.0, 42);
        assert!(estimate_snr_db(&pts50).is_some());
    }
}
