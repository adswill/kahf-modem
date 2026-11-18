//! Chase combining for repeated codewords (MESH-SPEC.md 8.5).
//!
//! When a codeword fails to decode, its soft bits are kept. If the sender repeats the frame, the
//! repeat is first tried alone (so a good repeat costs nothing and a different frame can never
//! be spoiled by an old one), and if that fails its soft bits are added to what was kept and the
//! sum is tried. The decoder's own check (the frame CRC, applied by the caller through `accept`)
//! decides what counts as decoded, so combining two unrelated frames just fails. What is kept is
//! the sum, so a third repeat adds to the first two.
//!
//! Entries are keyed by the codeword's position in its burst and the mode, and expire.

use crate::ldpc;

/// Kept entries wait this long (ms) for a repeat.
pub const KEEP_MS: u64 = 60_000;
const SLOTS: usize = 8;

struct Entry {
    mode: u8,
    at_ms: u64,
    llr: Vec<f32>,
    /// How many looks are in the sum.
    looks: u32,
}

#[derive(Default)]
pub struct Harq {
    slots: [Option<Entry>; SLOTS],
}

/// Result of a try: the info bits when it decoded, and how many looks it took.
#[derive(Debug, PartialEq)]
pub struct Decoded {
    pub info: Vec<u8>,
    pub looks: u32,
}

impl Harq {
    /// Try codeword `idx` of a burst in mode `mode` (any number identifying the mode). `accept`
    /// says whether decoded info bits are a real frame (the CRC check). Failed soft bits are kept.
    pub fn try_decode(
        &mut self,
        idx: usize,
        mode: u8,
        llr: &[f32],
        now_ms: u64,
        accept: impl Fn(&[u8]) -> bool,
    ) -> Option<Decoded> {
        let slot = idx.min(SLOTS - 1);
        if let Some(info) = ldpc::decode(llr, 50).filter(|i| accept(i)) {
            self.slots[slot] = None;
            return Some(Decoded { info, looks: 1 });
        }
        let prev = self.slots[slot].take().filter(|e| e.mode == mode && now_ms.saturating_sub(e.at_ms) <= KEEP_MS && e.llr.len() == llr.len());
        let Some(prev) = prev else {
            self.slots[slot] = Some(Entry { mode, at_ms: now_ms, llr: llr.to_vec(), looks: 1 });
            return None;
        };
        let sum: Vec<f32> = prev.llr.iter().zip(llr).map(|(a, b)| a + b).collect();
        let looks = prev.looks + 1;
        match ldpc::decode(&sum, 50).filter(|i| accept(i)) {
            Some(info) => Some(Decoded { info, looks }),
            None => {
                self.slots[slot] = Some(Entry { mode, at_ms: now_ms, llr: sum, looks });
                None
            }
        }
    }

    pub fn clear(&mut self) {
        self.slots = Default::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kami::{self, Shape};

    struct Rng(u64);
    impl Rng {
        fn u(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        }
        fn gauss(&mut self) -> f32 {
            let (a, b) = (self.u(), self.u());
            ((-2.0 * a.ln()).sqrt() * (2.0 * std::f64::consts::PI * b).cos()) as f32
        }
    }

    const SH: Shape = Shape { wb: 3, lb: 0 };

    fn info(seed: u64) -> Vec<u8> {
        let mut r = Rng(seed * 977 + 3);
        (0..ldpc::K).map(|_| (r.u() > 0.5) as u8).collect()
    }

    fn look(air: &[f32], snr_db: f64, seed: u64) -> Option<Vec<f32>> {
        let mut r = Rng(seed);
        let body = &air[1600..air.len() - 800];
        let rms = (body.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / body.len() as f64).sqrt();
        let sigma = (rms * 10f64.powf(-snr_db / 20.0)) as f32;
        let mut a = vec![0f32; 3000];
        a.extend(air.iter().map(|v| v + sigma * r.gauss()));
        a.extend((0..3000).map(|_| sigma * r.gauss()));
        let (start, hz, tau) = kami::find_preamble(&a)?;
        kami::demodulate_at(&a, start, hz, tau, ldpc::N, &[SH]).into_iter().next().map(|(_, l)| l)
    }

    /// Attempts needed for each frame, up to 4, with and without combining; (without, with).
    fn attempts(snr: f64, trials: u64) -> (Vec<u32>, Vec<u32>) {
        let (mut plain, mut chase) = (vec![], vec![]);
        for t in 0..trials {
            let sent = info(t + 1);
            let air = kami::modulate_frame_shape(&ldpc::encode(&sent), SH);
            let ok = |i: &[u8]| i == sent.as_slice();
            let mut h = Harq::default();
            let (mut p, mut c) = (0, 0);
            for n in 1..=4u32 {
                let Some(l) = look(&air, snr, t * 100 + n as u64) else { continue };
                if p == 0 && ldpc::decode(&l, 50).is_some_and(|i| ok(&i)) {
                    p = n;
                }
                if c == 0 && h.try_decode(0, 1, &l, 0, ok).is_some() {
                    c = n;
                }
            }
            plain.push(p);
            chase.push(c);
        }
        (plain, chase)
    }

    #[test]
    fn repeats_are_combined_and_need_fewer_tries_than_fresh_ones() {
        // KAMI-8 needs about -2.3 dB per sample; at -7.5 dB a single look mostly fails.
        let (plain, chase) = attempts(-7.5, 24);
        let done = |v: &[u32]| v.iter().filter(|n| **n > 0).count();
        assert!(done(&chase) > done(&plain), "combining should rescue frames fresh tries do not: {} vs {}", done(&chase), done(&plain));
        // Measured when written: fresh tries decode 0 of 24 within four looks, combining 17 of 24.
        assert!(done(&chase) >= 14, "{} of 24 within four tries", done(&chase));
    }

    #[test]
    fn a_good_repeat_decodes_alone_and_an_unrelated_frame_is_not_spoiled() {
        let a = info(1);
        let b = info(2);
        let (air_a, air_b) = (kami::modulate_frame_shape(&ldpc::encode(&a), SH), kami::modulate_frame_shape(&ldpc::encode(&b), SH));
        let mut h = Harq::default();
        // A bad look at A is kept ...
        assert!(h.try_decode(0, 1, &look(&air_a, -9.0, 1).unwrap(), 0, |i| i == a.as_slice()).is_none());
        // ... then a clean look at a different frame B decodes by itself.
        let got = h.try_decode(0, 1, &look(&air_b, 20.0, 2).unwrap(), 1000, |i| i == b.as_slice()).unwrap();
        assert_eq!((got.info, got.looks), (b, 1));
    }

    #[test]
    fn old_entries_and_other_modes_are_not_combined() {
        let a = info(3);
        let air = kami::modulate_frame_shape(&ldpc::encode(&a), SH);
        let bad = look(&air, -9.0, 5).unwrap();
        let mut h = Harq::default();
        assert!(h.try_decode(0, 1, &bad, 0, |i| i == a.as_slice()).is_none());
        // Same bits again after the keep time: it starts over (and fails alone).
        assert!(h.try_decode(0, 1, &bad, KEEP_MS + 1, |i| i == a.as_slice()).is_none());
        let mut h = Harq::default();
        assert!(h.try_decode(0, 1, &bad, 0, |i| i == a.as_slice()).is_none());
        assert!(h.try_decode(0, 2, &bad, 10, |i| i == a.as_slice()).is_none(), "another mode starts over");
    }
}
