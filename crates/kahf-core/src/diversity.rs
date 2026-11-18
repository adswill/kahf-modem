//! Receive diversity (MESH-SPEC.md 10.1): several relays hear the same burst, each sends the soft
//! bits of the codeword it could not decode to the server, and the server adds them and decodes.
//!
//! Soft bits (LLRs) are quantised to 4 bits (two per byte, 324 bytes for a 648 bit codeword).
//! Adding LLRs of independent looks at the same bits is maximal-ratio combining when each LLR
//! is already scaled by its own noise level, which the demodulator does. Only a codeword that
//! failed locally is forwarded. Nothing here is wired to the daemon or the server yet.

use crate::ldpc;
use std::collections::HashMap;

/// LLR magnitude that maps to the largest 4 bit level.
pub const CLIP: f32 = 16.0;
const LEVEL: f32 = 7.0;

/// 4 bit signed levels, two per byte (high nibble first), for `llr.len()` bits.
pub fn pack(llr: &[f32]) -> Vec<u8> {
    let q = |l: f32| ((l.clamp(-CLIP, CLIP) * LEVEL / CLIP).round() as i8 + 8) as u8 & 0x0F;
    llr.chunks(2).map(|c| (q(c[0]) << 4) | c.get(1).map_or(8, |l| q(*l))).collect()
}

pub fn unpack(bytes: &[u8], n: usize) -> Vec<f32> {
    let d = |nib: u8| (nib as i8 - 8) as f32 * CLIP / LEVEL;
    bytes.iter().flat_map(|b| [d(b >> 4), d(b & 0x0F)]).take(n).collect()
}

/// What a relay sends for a codeword it could not decode.
#[derive(Clone, Debug, PartialEq)]
pub struct Forward {
    /// Names the burst and codeword: the same for every relay that heard it (lane, slot,
    /// source and codeword index make it, chosen by the caller).
    pub frame: u64,
    pub relay: u32,
    pub llr4: Vec<u8>,
}

/// The server's side: collects what relays forward and decodes the sum.
#[derive(Default)]
pub struct Combiner {
    parts: HashMap<u64, Vec<(u32, Vec<f32>)>>,
}

impl Combiner {
    /// Add a forward. A second forward from the same relay for the same frame replaces its first.
    pub fn add(&mut self, f: &Forward) {
        let llr = unpack(&f.llr4, ldpc::N);
        let v = self.parts.entry(f.frame).or_default();
        v.retain(|(r, _)| *r != f.relay);
        v.push((f.relay, llr));
    }

    pub fn relays(&self, frame: u64) -> usize {
        self.parts.get(&frame).map_or(0, |v| v.len())
    }

    /// Decode the sum of everything held for `frame`. The info bits, or None.
    pub fn decode(&self, frame: u64) -> Option<Vec<u8>> {
        let v = self.parts.get(&frame)?;
        let mut sum = vec![0f32; ldpc::N];
        for (_, l) in v {
            for (s, x) in sum.iter_mut().zip(l) {
                *s += x;
            }
        }
        ldpc::decode(&sum, 50)
    }

    /// The server is done with this frame (decoded or given up): drop what it holds.
    pub fn forget(&mut self, frame: u64) {
        self.parts.remove(&frame);
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

    /// One relay's look: the soft bits of the frame after its own noise at `snr_db` (per sample).
    fn look(air: &[f32], snr_db: f64, seed: u64) -> Option<Vec<f32>> {
        let mut r = Rng(seed);
        let body: &[f32] = &air[1600..air.len() - 800];
        let rms = (body.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / body.len() as f64).sqrt();
        let sigma = (rms * 10f64.powf(-snr_db / 20.0)) as f32;
        let mut a = vec![0f32; 3000];
        a.extend(air.iter().map(|v| v + sigma * r.gauss()));
        a.extend((0..3000).map(|_| sigma * r.gauss()));
        let (start, hz, tau) = kami::find_preamble(&a)?;
        kami::demodulate_at(&a, start, hz, tau, ldpc::N, &[SH]).into_iter().next().map(|(_, l)| l)
    }

    #[test]
    fn four_bit_packing_round_trips_signs_and_keeps_the_size() {
        let llr: Vec<f32> = (0..ldpc::N).map(|i| ((i as f32 * 0.37).sin()) * 30.0).collect();
        let p = pack(&llr);
        assert_eq!(p.len(), 324);
        let u = unpack(&p, ldpc::N);
        for (a, b) in llr.iter().zip(&u) {
            assert!(a.signum() == b.signum() || b.abs() < 1.2, "sign lost: {a} -> {b}");
            assert!((a.clamp(-CLIP, CLIP) - b).abs() <= CLIP / LEVEL / 2.0 + 1e-3);
        }
    }

    #[test]
    fn three_relays_that_each_fail_decode_together() {
        // KAMI-8 needs about -2.3 dB per sample; each relay hears it at -6.5 dB. Three
        // independent looks add about 4.8 dB.
        let snr = -6.5;
        let mut single_ok = 0;
        let mut combined_ok = 0;
        let mut each_failed_and_combined_ok = 0;
        let trials = 24;
        for t in 0..trials {
            let sent = info(t + 1);
            let air = kami::modulate_frame_shape(&ldpc::encode(&sent), SH);
            let mut c = Combiner::default();
            let mut failed = 0;
            for relay in 0..3u32 {
                let Some(llr) = look(&air, snr, t * 10 + relay as u64 + 1) else {
                    failed += 1;
                    continue;
                };
                let ok = ldpc::decode(&llr, 50).as_deref() == Some(sent.as_slice());
                if ok {
                    single_ok += 1;
                } else {
                    failed += 1;
                    // Only what failed locally is forwarded.
                    c.add(&Forward { frame: t, relay, llr4: pack(&llr) });
                }
            }
            if c.relays(t) > 0 || failed == 3 {
                let got = c.decode(t);
                if got.as_deref() == Some(sent.as_slice()) {
                    combined_ok += 1;
                    if failed == 3 {
                        each_failed_and_combined_ok += 1;
                    }
                }
            }
        }
        // Measured when written: 0 of 72 single looks decode, 15 of 24 frames lost at all three relays are decoded by the sum.
        assert!(single_ok * 100 <= trials as usize * 3 * 40, "single decodes {single_ok} of {}", trials * 3);
        assert!(each_failed_and_combined_ok >= 10, "frames lost at all three relays but decoded by the server: {each_failed_and_combined_ok} (combined {combined_ok})");
    }

    #[test]
    fn a_relay_that_decoded_nothing_new_adds_no_harm_and_replacing_works() {
        let sent = info(5);
        let air = kami::modulate_frame_shape(&ldpc::encode(&sent), SH);
        let mut c = Combiner::default();
        let l = look(&air, 20.0, 1).unwrap();
        c.add(&Forward { frame: 1, relay: 7, llr4: pack(&l) });
        assert_eq!(c.decode(1).as_deref(), Some(sent.as_slice()));
        // The same relay sending again replaces, not doubles.
        c.add(&Forward { frame: 1, relay: 7, llr4: pack(&l) });
        assert_eq!(c.relays(1), 1);
        c.forget(1);
        assert_eq!(c.decode(1), None);
    }
}
