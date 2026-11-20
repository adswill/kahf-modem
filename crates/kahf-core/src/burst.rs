//! Speed ladder: multi-codeword bursts at a range of rates, from tens of bit/s that survive
//! about -9 dB (2500 Hz noise bandwidth) to over 5 kbit/s on a clean channel.
//!
//! A burst carries `n` codewords of 36 payload bytes each (own length and CRC, so a damaged
//! codeword does not cost the others). Rate comes from three knobs:
//! - modulation (QPSK, 16QAM, 64QAM) on the OFDM carriers,
//! - puncturing of the rate-1/2 LDPC code (send only the first `keep` of 648 coded bits;
//!   the decoder sees the dropped bits as zero LLRs),
//! - repetition (`reps` copies of the coded bits, soft-combined at the receiver).
//!
//! Several codewords share one frame, so the sync and channel-estimation overhead is paid once.
//! Most modes use the weak-signal preamble ([`phy::modulate_weak`]), the fastest the normal sync.
//! Both ends must agree on mode and codeword count (the link layer negotiates them).

use crate::phy::{self, Modulation};
use crate::{frame, ldpc};

/// Payload bytes per codeword.
pub const CW_BYTES: usize = frame::MAX_DATA;
/// Most codewords in one burst (about 1150 bytes).
pub const MAX_CODEWORDS: usize = 32;

/// One rung of the ladder.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mode {
    pub name: &'static str,
    pub modulation: Modulation,
    /// Coded bits sent per codeword, 648 down to 432 (rate 1/2 to 3/4).
    pub keep: usize,
    /// Copies of the coded bits.
    pub reps: usize,
    /// Use the weak-signal preamble (long correlation sync). The normal one-symbol sync
    /// needs about 6 dB, and misses a few percent of bursts even on a clean channel.
    pub weak: bool,
    /// Lowest SNR (dB, 2500 Hz) at which the mode is usable on an AWGN channel
    /// (codeword error rate under 10 percent), from `ladder_bench`.
    pub min_snr_db: f32,
}

macro_rules! m {
    ($n:expr, $m:ident, $k:expr, $r:expr, $w:expr, $s:expr) => {
        Mode {
            name: $n,
            modulation: Modulation::$m,
            keep: $k,
            reps: $r,
            weak: $w,
            min_snr_db: $s,
        }
    };
}

/// All modes, slowest and most robust first.
pub const LADDER: [Mode; 11] = [
    m!("R32", Qpsk, 648, 32, true, -9.0),
    m!("R16", Qpsk, 648, 16, true, -8.0),
    m!("R8", Qpsk, 648, 8, true, -6.0),
    m!("R4", Qpsk, 648, 4, true, -3.0),
    m!("Q12", Qpsk, 648, 1, true, 3.0),
    m!("Q23", Qpsk, 486, 1, true, 6.0),
    m!("S12", Qam16, 648, 1, true, 9.0),
    m!("S23", Qam16, 486, 1, true, 12.0),
    m!("T12", Qam64, 648, 1, true, 13.0),
    m!("T34", Qam64, 432, 1, true, 18.0),
    m!("T56", Qam64, 389, 1, true, 21.0),
];

/// Highest-rate mode usable at `snr_db` (2500 Hz noise bandwidth), with `margin_db` in hand.
/// Falls back to the most robust mode.
pub fn pick(snr_db: f32, margin_db: f32) -> &'static Mode {
    LADDER
        .iter()
        .rev()
        .find(|m| snr_db - margin_db >= m.min_snr_db)
        .unwrap_or(&LADDER[0])
}

fn bits_per_symbol(m: Modulation) -> usize {
    match m {
        Modulation::Qpsk => 2,
        Modulation::Qam16 => 4,
        Modulation::Qam64 => 6,
    }
}

impl Mode {
    fn coded_bits(&self, n_cw: usize) -> usize {
        self.keep * self.reps * n_cw
    }

    /// OFDM data symbols in a burst of `n_cw` codewords.
    fn data_symbols(&self, n_cw: usize) -> usize {
        // 114 data carriers per symbol, see phy.
        self.coded_bits(n_cw)
            .div_ceil(114 * bits_per_symbol(self.modulation))
    }

    /// Airtime in seconds of a burst of `n_cw` codewords.
    pub fn airtime_s(&self, n_cw: usize) -> f64 {
        let sym = (2 + self.data_symbols(n_cw)) * 576;
        let pre = if self.weak { phy::WEAK_PRE_LEN } else { 0 };
        (sym + pre) as f64 / 8000.0
    }

    /// Payload bit/s of a full burst, ignoring turnaround and ARQ.
    pub fn rate_bps(&self, n_cw: usize) -> f64 {
        (n_cw * CW_BYTES * 8) as f64 / self.airtime_s(n_cw)
    }
}

/// Encode up to `MAX_CODEWORDS * CW_BYTES` bytes of `payload` as one burst of audio samples.
/// Returns the samples and the codeword count the receiver must be told.
pub fn modulate(payload: &[u8], mode: &Mode) -> (Vec<f32>, usize) {
    assert!(
        payload.len() <= MAX_CODEWORDS * CW_BYTES,
        "burst payload too long"
    );
    let n_cw = payload.len().div_ceil(CW_BYTES).max(1);
    let mut coded = Vec::with_capacity(mode.coded_bits(n_cw));
    let mut cws: Vec<Vec<u8>> = Vec::with_capacity(n_cw);
    for c in 0..n_cw {
        let lo = c * CW_BYTES;
        let chunk = &payload[lo.min(payload.len())..(lo + CW_BYTES).min(payload.len())];
        let cw = ldpc::encode(&frame::build_payload(chunk));
        cws.push(cw[..mode.keep].to_vec());
    }
    for _ in 0..mode.reps {
        for cw in &cws {
            coded.extend_from_slice(cw);
        }
    }
    let samples = if mode.weak {
        phy::modulate_weak(&coded, mode.modulation)
    } else {
        phy::modulate_frame(&coded, mode.modulation)
    };
    (samples, n_cw)
}

/// Channel-estimate variants tried in turn: static, then tracked over 7 symbols (about 0.5 s),
/// then over 3 symbols (about 0.2 s). A codeword is taken from the first that decodes.
const VARIANTS: [Option<usize>; 3] = [None, Some(3), Some(1)];

/// Combine repetitions of codeword `c` from one set of LLRs.
fn combine(llr: &[f32], mode: &Mode, n_cw: usize, c: usize) -> Vec<f32> {
    let mut sum = vec![0f32; ldpc::N];
    for r in 0..mode.reps {
        let off = (r * n_cw + c) * mode.keep;
        for (s, l) in sum.iter_mut().zip(&llr[off..off + mode.keep]) {
            *s += l;
        }
    }
    sum
}

/// Decode a burst of `n_cw` codewords. Entry `c` is that codeword's payload bytes, or None if
/// it did not decode. The whole result is None only if the burst could not be found.
pub fn demodulate(samples: &[f32], mode: &Mode, n_cw: usize) -> Option<Vec<Option<Vec<u8>>>> {
    let n_bits = mode.coded_bits(n_cw);
    let sets: Vec<Vec<f32>> = if mode.weak {
        phy::demodulate_weak_variants(samples, mode.modulation, n_bits, &VARIANTS)?
    } else {
        vec![phy::demodulate_frame(samples, mode.modulation, n_bits)?]
    };
    Some(
        (0..n_cw)
            .map(|c| {
                sets.iter().find_map(|llr| {
                    ldpc::decode(&combine(llr, mode, n_cw, c), 50).and_then(|b| frame::parse_payload(&b))
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mode_roundtrips_clean() {
        let payload: Vec<u8> = (0..100u32).map(|i| (i * 7 + 3) as u8).collect();
        for mode in LADDER.iter().filter(|m| m.reps <= 8) {
            let (tx, n) = modulate(&payload, mode);
            assert_eq!(n, 3);
            let mut s = vec![0f32; 3000];
            s.extend_from_slice(&tx);
            s.extend(std::iter::repeat_n(0f32, 3000));
            let got = demodulate(&s, mode, n).unwrap_or_else(|| panic!("{} not found", mode.name));
            let out: Vec<u8> = got
                .into_iter()
                .flat_map(|c| c.unwrap_or_else(|| panic!("{} cw lost", mode.name)))
                .collect();
            assert_eq!(out, payload, "{}", mode.name);
        }
    }

    #[test]
    fn ladder_is_ordered_and_picks_sensibly() {
        for w in LADDER.windows(2) {
            assert!(w[0].min_snr_db <= w[1].min_snr_db);
            assert!(
                w[0].rate_bps(32) < w[1].rate_bps(32),
                "{} vs {}",
                w[0].name,
                w[1].name
            );
        }
        assert_eq!(pick(-30.0, 0.0).name, "R32");
        assert_eq!(pick(40.0, 0.0).name, "T56");
        assert_eq!(pick(11.0, 2.0).name, "S12");
    }
}
