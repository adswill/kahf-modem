//! Robustness sweeps: random and adversarial input to the decoding entry points.
//!
//! Every entry point runs ITERS iterations from a fixed seed. Each call is wrapped in
//! catch_unwind so that a panic is reported together with its iteration number and
//! input shape, and every returned value is checked for basic well-formedness.
//! No panics were found, so no #[ignore]d reproducers are needed.

use kahf_core::frame;
use kahf_core::ldpc::{self, K, N};
use kahf_core::phy::{self, Modulation};
use kahf_core::session;
use std::panic::{self, AssertUnwindSafe};

const ITERS: usize = 2000;
const SEED: u64 = 0x9E37_79B9_7F4A_7C15;
const MAX_FRAME_DATA: usize = 36;

/// xorshift64 generator. Never seeded with zero.
struct XorShift(u64);

impl XorShift {
    fn new(seed: u64) -> Self {
        assert_ne!(seed, 0);
        XorShift(seed)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniform in 0..n, n > 0.
    fn below(&mut self, n: usize) -> usize {
        assert!(n > 0);
        (self.next_u64() % n as u64) as usize
    }

    fn byte(&mut self) -> u8 {
        self.next_u64() as u8
    }

    /// Uniform in [-1, 1).
    fn signed_unit(&mut self) -> f32 {
        let u = (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32;
        u * 2.0 - 1.0
    }
}

/// Run `f`, turning a panic into an Err with the panic message.
fn guarded<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    panic::catch_unwind(AssertUnwindSafe(f)).map_err(|e| {
        e.downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| e.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "non-string panic payload".to_string())
    })
}

fn report(failures: &[String]) {
    assert!(
        failures.is_empty(),
        "{} failure(s), first few:\n{}",
        failures.len(),
        failures.iter().take(5).cloned().collect::<Vec<_>>().join("\n")
    );
}

/// Bits for frame::parse_payload. Mix of random bits, valid frames with a few
/// flipped bits, and out-of-domain byte values.
fn frame_input(rng: &mut XorShift) -> Vec<u8> {
    match rng.below(4) {
        0 => (0..K).map(|_| rng.below(2) as u8).collect(),
        1 => (0..K).map(|_| rng.byte()).collect(),
        _ => {
            let len = rng.below(MAX_FRAME_DATA + 1);
            let data: Vec<u8> = (0..len).map(|_| rng.byte()).collect();
            let mut bits = frame::build_payload(&data);
            for _ in 0..rng.below(4) {
                let i = rng.below(K);
                bits[i] ^= 1;
            }
            bits
        }
    }
}

/// Lengths for frame::parse_payload that are not K.
fn wrong_frame_len(rng: &mut XorShift) -> usize {
    const FIXED: [usize; 7] = [0, 1, 8, 296, 323, 325, 648];
    if rng.below(2) == 0 {
        FIXED[rng.below(FIXED.len())]
    } else {
        let n = rng.below(1024);
        if n == K {
            K + 1
        } else {
            n
        }
    }
}

#[test]
fn frame_parse_payload_random_bits_never_panics() {
    let mut rng = XorShift::new(SEED);
    let mut failures = Vec::new();
    for it in 0..ITERS {
        let bits = if it % 5 == 4 {
            let n = wrong_frame_len(&mut rng);
            (0..n).map(|_| rng.below(2) as u8).collect()
        } else {
            frame_input(&mut rng)
        };
        let shape = format!("iter {it}, len {}", bits.len());
        match guarded(|| frame::parse_payload(&bits)) {
            Err(p) => failures.push(format!("{shape}: panic: {p}")),
            Ok(Some(v)) if v.len() > MAX_FRAME_DATA => {
                failures.push(format!("{shape}: Some with {} bytes", v.len()))
            }
            Ok(Some(_)) if bits.len() != K => failures.push(format!("{shape}: Some for wrong-length input")),
            Ok(_) => {}
        }
    }
    report(&failures);
}

/// CRC-16/CCITT-FALSE, as used by session chunks.
fn crc16(bytes: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in bytes {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// One random chunk list: random lengths 0..=36, random bytes, and with some
/// probability chunks with a valid CRC and a random header (id, index, total),
/// so that they reach the reassembly code. Chunk counts range from 0 upward.
fn session_chunks(rng: &mut XorShift) -> Vec<Vec<u8>> {
    let count = match rng.below(4) {
        0 => 0,
        1 => rng.below(4),
        2 => rng.below(40),
        _ => rng.below(300),
    };
    let crafted = rng.below(2) == 0;
    (0..count)
        .map(|_| {
            if crafted && rng.below(3) != 0 {
                // Header plus data of 0..=31 bytes plus 2 CRC bytes: 5..=36 bytes.
                let len = 5 + rng.below(32);
                let mut c = Vec::with_capacity(len);
                c.push(rng.below(4) as u8);
                c.push(if rng.below(2) == 0 {
                    rng.below(4) as u8
                } else {
                    rng.byte()
                });
                c.push(if rng.below(2) == 0 {
                    rng.below(8) as u8
                } else {
                    rng.byte()
                });
                for _ in 0..len - 5 {
                    c.push(rng.byte());
                }
                let crc = crc16(&c);
                c.extend_from_slice(&crc.to_be_bytes());
                c
            } else {
                let len = rng.below(MAX_FRAME_DATA + 1);
                (0..len).map(|_| rng.byte()).collect()
            }
        })
        .collect()
}

#[test]
fn session_decode_random_chunks_never_panics() {
    let mut rng = XorShift::new(SEED);
    let mut failures = Vec::new();
    for it in 0..ITERS {
        let chunks = session_chunks(&mut rng);
        let lens: Vec<usize> = chunks.iter().map(|c| c.len()).collect();
        let shape = format!("iter {it}, {} chunks, lens {:?}", chunks.len(), lens);
        match guarded(|| session::decode(&chunks)) {
            Err(p) => failures.push(format!("{shape}: panic: {p}")),
            Ok(Some(msg)) => {
                // A decoded message must survive a re-encode and decode.
                let again = guarded(|| session::decode(&session::encode(&msg)));
                match again {
                    Err(p) => failures.push(format!("{shape}: re-encode panic: {p}")),
                    Ok(back) if back.as_ref() != Some(&msg) => {
                        failures.push(format!("{shape}: decoded message does not round-trip"))
                    }
                    Ok(_) => {}
                }
            }
            Ok(None) => {}
        }
    }
    report(&failures);
}

/// LLRs for ldpc::decode. Finite values, or a mix that includes NaN, +/-inf, zero,
/// and extreme magnitudes.
fn llr_input(rng: &mut XorShift, specials: bool) -> Vec<f32> {
    const SPECIAL: [f32; 7] = [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        0.0,
        -0.0,
        f32::MAX,
        f32::MIN_POSITIVE,
    ];
    (0..N)
        .map(|_| {
            if specials && rng.below(4) == 0 {
                SPECIAL[rng.below(SPECIAL.len())]
            } else {
                rng.signed_unit() * 12.0
            }
        })
        .collect()
}

fn check_ldpc(it: usize, llr: &[f32], label: &str, failures: &mut Vec<String>) {
    let max_iter = (it % 21) as usize;
    let shape = format!("{label} iter {it}, max_iter {max_iter}");
    match guarded(|| ldpc::decode(llr, max_iter)) {
        Err(p) => failures.push(format!("{shape}: panic: {p}")),
        Ok(Some(bits)) if bits.len() != K => {
            failures.push(format!("{shape}: Some with {} bits, expected {K}", bits.len()))
        }
        Ok(_) => {}
    }
}

#[test]
fn ldpc_decode_random_llrs_never_panics() {
    let mut rng = XorShift::new(SEED);
    let mut failures = Vec::new();
    for it in 0..ITERS {
        let llr = llr_input(&mut rng, false);
        check_ldpc(it, &llr, "finite", &mut failures);
    }
    report(&failures);
}

#[test]
fn ldpc_decode_nan_and_inf_llrs_never_panics() {
    let mut rng = XorShift::new(SEED ^ 0xA5A5);
    let mut failures = Vec::new();
    for it in 0..ITERS {
        let llr = llr_input(&mut rng, true);
        check_ldpc(it, &llr, "special", &mut failures);
    }
    report(&failures);
}

#[test]
fn phy_demodulate_frame_random_samples_never_panics() {
    const LENGTHS: [usize; 4] = [0, 1, 100, 10_000];
    const N_BITS: [usize; 5] = [0, 1, N, K, 4096];
    const MODS: [Modulation; 3] = [Modulation::Qpsk, Modulation::Qam16, Modulation::Qam64];
    let mut rng = XorShift::new(SEED);
    let mut failures = Vec::new();
    for it in 0..ITERS {
        let len = LENGTHS[rng.below(LENGTHS.len())];
        let kind = rng.below(4);
        let samples: Vec<f32> = (0..len)
            .map(|_| match kind {
                // Silence: exercises the zero-power path.
                0 => 0.0,
                // Mostly NaN and infinities.
                1 if rng.below(2) == 0 => f32::NAN,
                1 => {
                    if rng.below(2) == 0 {
                        f32::INFINITY
                    } else {
                        f32::NEG_INFINITY
                    }
                }
                // Finite audio in [-1, 1).
                _ => rng.signed_unit(),
            })
            .collect();
        let m = MODS[rng.below(MODS.len())];
        let n_bits = N_BITS[rng.below(N_BITS.len())];
        let shape = format!("iter {it}, {len} samples (kind {kind}), {m:?}, n_bits {n_bits}");
        if let Err(p) = guarded(|| phy::demodulate_frame(&samples, m, n_bits)) {
            failures.push(format!("{shape}: panic: {p}"));
        }
    }
    report(&failures);
}
