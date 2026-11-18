//! LDPC code with K = 324 info bits and N = 648 coded bits, rate 1/2.
//!
//! The parity-check structure is the IEEE 802.11 (Wi-Fi) LDPC code of length 648
//! at rate 1/2, lifting size Z = 27. The base matrix and its source and licence
//! are in `ldpc_std` (MIT-licensed open-source project, see that file's header).
//!
//! The encoder is generic: Gauss-Jordan elimination on the parity columns, done
//! once. Output is systematic. Decoding is layered offset min-sum, one block
//! row at a time. Buffers are allocated once per call, and decoding stops as soon
//! as every parity check holds.

use std::sync::OnceLock;

use crate::ldpc_std::{BASE, MB, NB, Z};

pub const K: usize = 324;
/// Coded length.
pub const N: usize = 648;

const _: () = assert!(
    MB * Z == N - K && NB * Z == N,
    "base matrix does not match K and N"
);

/// Offset subtracted from every check-to-variable magnitude (offset min-sum).
/// Damping on variable-to-check messages: 1.0 = off.
const DAMP: f32 = 0.8;
/// Largest check degree the decoder's stack buffer can hold.
const MAX_DEG: usize = 32;
/// Clamp on half-LLRs before tanh, and on the tanh product before atanh (keeps values finite).
const TANH_CLAMP: f32 = 15.0;
const PROD_CLAMP: f32 = 0.999_999;
/// 64-bit words per info vector.
const WORDS: usize = K.div_ceil(64);

fn bit(words: &[u64], i: usize) -> bool {
    (words[i >> 6] >> (i & 63)) & 1 == 1
}

fn flip_row(dst: &mut [u64], src: &[u64]) {
    for (d, s) in dst.iter_mut().zip(src) {
        *d ^= *s;
    }
}

/// For each parity bit, the row of K info bits whose parity gives that bit.
/// Built by Gauss-Jordan on the parity columns: after elimination, the row
/// that pivots on parity column p reads p = sum(row_info AND info) mod 2.
fn build_encoder() -> Vec<[u64; WORDS]> {
    let rows = MB * Z;
    let w = N.div_ceil(64);
    let mut h = vec![vec![0u64; w]; rows];
    for i in 0..MB {
        for r in 0..Z {
            for (j, &s) in BASE[i].iter().enumerate() {
                if s >= 0 {
                    let v = j * Z + (r + s as usize) % Z;
                    h[i * Z + r][v >> 6] |= 1u64 << (v & 63);
                }
            }
        }
    }
    let mut pivot_row = vec![usize::MAX; N - K];
    let mut used = vec![false; rows];
    for pc in K..N {
        let r = (0..rows)
            .find(|&r| !used[r] && bit(&h[r], pc))
            .expect("LDPC parity part is singular");
        used[r] = true;
        pivot_row[pc - K] = r;
        let pivot = h[r].clone();
        for (r2, row) in h.iter_mut().enumerate() {
            if r2 != r && bit(row, pc) {
                flip_row(row, &pivot);
            }
        }
    }
    let last_mask = (1u64 << (K % 64)) - 1;
    pivot_row
        .iter()
        .map(|&r| {
            let mut out = [0u64; WORDS];
            out.copy_from_slice(&h[r][..WORDS]);
            out[WORDS - 1] &= last_mask;
            out
        })
        .collect()
}

fn encoder() -> &'static Vec<[u64; WORDS]> {
    static E: OnceLock<Vec<[u64; WORDS]>> = OnceLock::new();
    E.get_or_init(build_encoder)
}

/// Encode K info bits (values 0/1) into N coded bits. Systematic.
pub fn encode(info: &[u8]) -> Vec<u8> {
    assert_eq!(info.len(), K, "encode expects {K} info bits");
    let mut s = [0u64; WORDS];
    for (k, &b) in info.iter().enumerate() {
        if b & 1 == 1 {
            s[k >> 6] |= 1u64 << (k & 63);
        }
    }
    let mut cw = vec![0u8; N];
    for (k, &b) in info.iter().enumerate() {
        cw[k] = b & 1;
    }
    for (p, row) in encoder().iter().enumerate() {
        let mut acc = 0u32;
        for w in 0..WORDS {
            acc ^= (row[w] & s[w]).count_ones();
        }
        cw[K + p] = (acc & 1) as u8;
    }
    cw
}

/// Edge layout for the layered decoder. Block row `i` has `deg[i]` nonzero
/// blocks. Its Z checks are stored consecutively, check `r` at
/// `layer_start[i] + r * deg[i]`, and each check lists its variables in order.
struct Graph {
    vars: Vec<usize>,
    layer_start: Vec<usize>,
    deg: Vec<usize>,
}

fn build_graph() -> Graph {
    let mut vars = Vec::new();
    let mut layer_start = vec![0usize; MB + 1];
    let mut deg = vec![0usize; MB];
    for i in 0..MB {
        let cols: Vec<usize> = (0..NB).filter(|&j| BASE[i][j] >= 0).collect();
        deg[i] = cols.len();
        assert!(deg[i] <= MAX_DEG, "check degree exceeds MAX_DEG");
        layer_start[i + 1] = layer_start[i] + Z * deg[i];
        for r in 0..Z {
            for &j in &cols {
                let s = BASE[i][j] as usize;
                vars.push(j * Z + (r + s) % Z);
            }
        }
    }
    Graph {
        vars,
        layer_start,
        deg,
    }
}

fn graph() -> &'static Graph {
    static G: OnceLock<Graph> = OnceLock::new();
    G.get_or_init(build_graph)
}

/// True when every parity check holds for the hard decisions `post < 0`.
fn syndrome_ok(g: &Graph, post: &[f32]) -> bool {
    for i in 0..g.deg.len() {
        let d = g.deg[i];
        for r in 0..Z {
            let base = g.layer_start[i] + r * d;
            let mut x = false;
            for &v in &g.vars[base..base + d] {
                x ^= post[v] < 0.0;
            }
            if x {
                return false;
            }
        }
    }
    true
}

/// Layered offset min-sum on LLRs (positive = bit 0).
/// Returns the first K bits once all parity checks hold, or None when
/// `max_iter` iterations pass first, or when `llr.len() != N`.
pub fn decode(llr: &[f32], max_iter: usize) -> Option<Vec<u8>> {
    if llr.len() != N {
        return None;
    }
    let g = graph();
    // Everything the iteration loop touches is allocated here, once per call.
    let mut post = llr.to_vec();
    let mut msg = vec![0f32; g.vars.len()];
    let mut q = [0f32; MAX_DEG];
    let mut tanh_half = [0f32; MAX_DEG];
    let mut pre = [0f32; MAX_DEG + 1];
    let mut suf = [0f32; MAX_DEG + 1];
    let mut qlast: Vec<f32> = g.vars.iter().map(|&v| llr[v]).collect();

    for _ in 0..max_iter {
        for i in 0..g.deg.len() {
            let d = g.deg[i];
            for r in 0..Z {
                let base = g.layer_start[i] + r * d;
                let vars = &g.vars[base..base + d];
                let msgs = &mut msg[base..base + d];
                let qs = &mut q[..d];
                for k in 0..d {
                    let raw = post[vars[k]] - msgs[k];
                    let qk = DAMP * raw + (1.0 - DAMP) * qlast[base + k];
                    qlast[base + k] = qk;
                    qs[k] = qk;
                    tanh_half[k] = (0.5 * qk).clamp(-TANH_CLAMP, TANH_CLAMP).tanh();
                }
                // Sum-product check update: product of the other tanh terms, via prefix
                // and suffix products (no division, so zero terms are safe).
                pre[0] = 1.0;
                for k in 0..d {
                    pre[k + 1] = pre[k] * tanh_half[k];
                }
                suf[d] = 1.0;
                for k in (0..d).rev() {
                    suf[k] = suf[k + 1] * tanh_half[k];
                }
                for k in 0..d {
                    let prod = (pre[k] * suf[k + 1]).clamp(-PROD_CLAMP, PROD_CLAMP);
                    let new = 2.0 * prod.atanh();
                    post[vars[k]] += new - msgs[k];
                    msgs[k] = new;
                }
            }
        }
        // Early exit: every parity check holds on the current hard decisions.
        if syndrome_ok(g, &post) {
            return Some(post[..K].iter().map(|&p| u8::from(p < 0.0)).collect());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference codeword 0 from the IEEE 802.11 N = 648, R = 1/2 test vectors in
    /// the MIT-licensed project named in `ldpc_std`. Its first K bits are the info.
    const STD_CODEWORD_0: &str = "100110000101011110011010011111101100110101001101011001100111001000011010101101010100110000110111000101001101110101100010110110001100110010111111001110110110111110111110110110001111000111001000110111111000100111011000101000001010000001110010011001011011110001100000010100111001001101110001110111110110010011100010011011110110010000011101111111110100111111110110011110011100100000011011111001010000000101111110000000000111100100110111001100010101100000100011100100001110000100000111110011001011100111010010010110010111111100001110110010101011101011100110100010010111111011001011001111000110011111001010101000010011001011011111101100101010101011111110";

    /// Small deterministic PRNG for test data.
    struct Rng(u64);
    impl Rng {
        fn next_u64(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0 >> 11
        }
        /// Uniform in (0, 1).
        fn uniform(&mut self) -> f64 {
            (self.next_u64() as f64 + 0.5) / (1u64 << 53) as f64
        }
        fn gaussian(&mut self) -> f64 {
            let u1 = self.uniform();
            let u2 = self.uniform();
            (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
        }
        fn info(&mut self) -> Vec<u8> {
            (0..K).map(|_| (self.next_u64() & 1) as u8).collect()
        }
    }

    /// Independent parity check written from the base matrix, not from the graph.
    fn parity_ok(cw: &[u8]) -> bool {
        for row in BASE.iter().take(MB) {
            for r in 0..Z {
                let mut acc = 0u8;
                for j in 0..NB {
                    let s = row[j];
                    if s >= 0 {
                        acc ^= cw[j * Z + (r + s as usize) % Z];
                    }
                }
                if acc != 0 {
                    return false;
                }
            }
        }
        true
    }

    fn llr_from_bits(cw: &[u8], mag: f32) -> Vec<f32> {
        cw.iter().map(|&b| if b == 0 { mag } else { -mag }).collect()
    }

    /// One AWGN frame at the given Eb/N0 (dB), unit-energy BPSK.
    /// Convention: Eb is energy per info bit, so Ec/N0 = R * Eb/N0 with R = K/N,
    /// sigma^2 = N0/2 = 1 / (2 * R * Eb/N0), LLR = 2y / sigma^2.
    fn awgn_llr(rng: &mut Rng, cw: &[u8], ebn0_db: f64) -> Vec<f32> {
        let ebn0 = 10f64.powf(ebn0_db / 10.0);
        let r = K as f64 / N as f64;
        let sigma2 = 1.0 / (2.0 * r * ebn0);
        let sigma = sigma2.sqrt();
        cw.iter()
            .map(|&b| {
                let x = if b == 0 { 1.0 } else { -1.0 };
                let y = x + sigma * rng.gaussian();
                (2.0 * y / sigma2) as f32
            })
            .collect()
    }

    #[test]
    fn base_matrix_size_matches_code() {
        assert_eq!(BASE.len() * Z, N - K);
        assert!(BASE.iter().all(|row| row.len() == NB));
    }

    #[test]
    fn encode_satisfies_all_checks() {
        let zero = encode(&vec![0u8; K]);
        assert!(zero.iter().all(|&b| b == 0));

        let mut rng = Rng(1);
        for _ in 0..20 {
            let info = rng.info();
            let cw = encode(&info);
            assert_eq!(cw.len(), N);
            assert_eq!(&cw[..K], &info[..], "code must be systematic");
            assert!(parity_ok(&cw), "encoder output fails parity checks");
        }
    }

    /// The encoder reproduces the standard's reference codeword exactly.
    #[test]
    fn matches_standard_test_vector() {
        let cw: Vec<u8> = STD_CODEWORD_0.bytes().map(|c| c - b'0').collect();
        assert_eq!(cw.len(), N);
        assert!(parity_ok(&cw));
        assert_eq!(encode(&cw[..K]), cw);
    }

    #[test]
    fn noiseless_round_trip() {
        let mut rng = Rng(2);
        for _ in 0..10 {
            let info = rng.info();
            let cw = encode(&info);
            let llr = llr_from_bits(&cw, 8.0);
            assert_eq!(decode(&llr, 20), Some(info));
        }
    }

    #[test]
    fn wrong_length_is_rejected() {
        let llr = vec![1.0f32; N - 1];
        assert!(decode(&llr, 5).is_none());
    }

    #[test]
    fn corrects_flipped_bits() {
        let mut rng = Rng(3);
        let info = rng.info();
        let cw = encode(&info);
        let mut llr = llr_from_bits(&cw, 4.0);
        // Five bits flipped in sign but with low confidence.
        for &p in &[3usize, 100, 211, 400, 600] {
            llr[p] = if cw[p] == 0 { -0.5 } else { 0.5 };
        }
        assert_eq!(decode(&llr, 50), Some(info));
    }

    /// FER table: Eb/N0 -1 to 3 dB in 0.5 steps, 500 frames each, max_iter 50,
    /// fixed seeds. Prints the lowest grid point with FER below 1 percent and
    /// the mean decode time per frame.
    /// Run with `cargo test -p kahf-core ldpc -- --ignored --nocapture --release`.
    #[test]
    #[ignore]
    fn fer_table() {
        let trials = 500;
        let max_iter = 50;
        let points: Vec<f64> = (0..=8).map(|s| -1.0 + 0.5 * s as f64).collect();
        println!("\nN = {N}  K = {K}  R = {:.3}  Z = {Z}", K as f64 / N as f64);
        println!("Eb/N0 (dB)   FER        frames   mean decode (us)");
        let mut threshold: Option<f64> = None;
        let mut total_us = 0f64;
        for (si, &db) in points.iter().enumerate() {
            let mut rng = Rng(1000 + si as u64);
            let mut errors = 0usize;
            let mut decode_time = std::time::Duration::ZERO;
            for _ in 0..trials {
                let info = rng.info();
                let cw = encode(&info);
                let llr = awgn_llr(&mut rng, &cw, db);
                let t0 = std::time::Instant::now();
                let out = decode(&llr, max_iter);
                decode_time += t0.elapsed();
                if out != Some(info) {
                    errors += 1;
                }
            }
            let fer = errors as f64 / trials as f64;
            if fer < 0.01 && threshold.is_none() {
                threshold = Some(db);
            }
            let us = decode_time.as_secs_f64() * 1e6 / trials as f64;
            total_us += decode_time.as_secs_f64() * 1e6;
            println!("{db:>10.1}   {fer:<10.4} {trials:>6}   {us:>10.1}");
        }
        let mean = total_us / (trials * points.len()) as f64;
        match threshold {
            Some(t) => println!("lowest Eb/N0 with FER < 1%: {t:.1} dB"),
            None => println!("lowest Eb/N0 with FER < 1%: none in -1..3 dB"),
        }
        println!("mean decode over all points: {mean:.1} us per frame (max_iter {max_iter})");
    }
}
