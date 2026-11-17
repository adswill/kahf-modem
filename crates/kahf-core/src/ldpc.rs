//! Own quasi-cyclic LDPC, rate 1/2, K = 324 info bits, N = 648 coded bits.
//! Lifting size Z = 54: 12 block columns (6 info, 6 parity), 6 block rows.
//! The base matrix is our own: it is built at compile time by a fixed seeded
//! xorshift construction (`build_base`), not copied from any standard.
//! The parity part is a staircase (dual-diagonal), so encoding is a single
//! forward pass over the parity blocks.

use std::sync::OnceLock;

pub const K: usize = 324;
pub const N: usize = 648;

const Z: usize = 54;
const MB: usize = 6;
const NB: usize = 12;
const KB: usize = 6;
const ALPHA: f32 = 0.75;
const SEED: u64 = 0x9E37_79B9_7F4A_7C15;

const fn xorshift(mut x: u64) -> u64 {
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x
}

/// Base matrix: -1 = zero block, otherwise cyclic shift in 0..Z.
/// Info blocks: each present with probability 3/5 and a random shift.
/// Parity blocks: staircase, check i has shift 0 on p_i and on p_{i-1}.
const fn build_base() -> [[i32; NB]; MB] {
    let mut b = [[-1i32; NB]; MB];
    let mut st = SEED;
    let mut i = 0;
    while i < MB {
        let mut j = 0;
        while j < KB {
            st = xorshift(st);
            if (st >> 33) % 5 < 3 {
                b[i][j] = ((st >> 8) % Z as u64) as i32;
            }
            j += 1;
        }
        i += 1;
    }
    let mut i = 0;
    while i < MB {
        b[i][KB + i] = 0;
        if i > 0 {
            b[i][KB + i - 1] = 0;
        }
        i += 1;
    }
    b
}

const BASE: [[i32; NB]; MB] = build_base();

/// Check-node adjacency: for each of the MB*Z checks, the variable indices it touches.
/// Check (i, r) connects block column j to variable j*Z + (r + shift) % Z.
fn checks() -> &'static [Vec<usize>] {
    static G: OnceLock<Vec<Vec<usize>>> = OnceLock::new();
    G.get_or_init(|| {
        let mut out = Vec::with_capacity(MB * Z);
        for i in 0..MB {
            for r in 0..Z {
                let mut vars = Vec::new();
                for j in 0..NB {
                    let s = BASE[i][j];
                    if s >= 0 {
                        vars.push(j * Z + (r + s as usize) % Z);
                    }
                }
                out.push(vars);
            }
        }
        out
    })
    .as_slice()
}

fn all_checks_hold(bits: &[u8]) -> bool {
    checks()
        .iter()
        .all(|vars| vars.iter().fold(0u8, |a, &v| a ^ bits[v]) == 0)
}

/// Encode K info bits (values 0/1) into N coded bits (0/1). Systematic.
///
/// Info blocks come first. For block row i, s_i is the XOR of the info blocks
/// on that row, and the parity block is p_i = s_i ^ p_{i-1} (p_{-1} = 0).
pub fn encode(info: &[u8]) -> Vec<u8> {
    assert_eq!(info.len(), K, "encode expects {K} info bits");
    let mut cw = vec![0u8; N];
    for (k, &b) in info.iter().enumerate() {
        cw[k] = b & 1;
    }
    for i in 0..MB {
        for r in 0..Z {
            let mut p = 0u8;
            for j in 0..KB {
                let s = BASE[i][j];
                if s >= 0 {
                    p ^= cw[j * Z + (r + s as usize) % Z];
                }
            }
            if i > 0 {
                p ^= cw[(KB + i - 1) * Z + r];
            }
            cw[(KB + i) * Z + r] = p;
        }
    }
    cw
}

/// Normalised min-sum belief propagation on LLRs (positive = bit 0).
/// Checks all parity equations after each iteration. Returns the first K bits
/// once they are satisfied, or None if `max_iter` is reached first.
pub fn decode(llr: &[f32], max_iter: usize) -> Option<Vec<u8>> {
    if llr.len() != N {
        return None;
    }
    let checks = checks();

    // Flat edge list: edge e joins check `check_of(e)` and variable edge_var[e].
    let mut edge_var = Vec::new();
    let mut check_start = Vec::with_capacity(checks.len() + 1);
    let mut var_edges: Vec<Vec<usize>> = vec![Vec::new(); N];
    for vars in checks {
        check_start.push(edge_var.len());
        for &v in vars {
            var_edges[v].push(edge_var.len());
            edge_var.push(v);
        }
    }
    check_start.push(edge_var.len());
    let ne = edge_var.len();

    let mut c2v = vec![0f32; ne];
    let mut v2c: Vec<f32> = edge_var.iter().map(|&v| llr[v]).collect();
    let mut bits = vec![0u8; N];

    for _ in 0..max_iter {
        // Check-node update.
        for c in 0..checks.len() {
            let (s, t) = (check_start[c], check_start[c + 1]);
            let mut min1 = f32::INFINITY;
            let mut min2 = f32::INFINITY;
            let mut idx1 = usize::MAX;
            let mut neg = false;
            for e in s..t {
                let m = v2c[e].abs();
                if v2c[e] < 0.0 {
                    neg = !neg;
                }
                if m < min1 {
                    min2 = min1;
                    min1 = m;
                    idx1 = e;
                } else if m < min2 {
                    min2 = m;
                }
            }
            for e in s..t {
                let mag = if e == idx1 { min2 } else { min1 };
                let negative = neg ^ (v2c[e] < 0.0);
                c2v[e] = if negative { -ALPHA * mag } else { ALPHA * mag };
            }
        }

        // Variable-node update and hard decision.
        for v in 0..N {
            let mut total = llr[v];
            for &e in &var_edges[v] {
                total += c2v[e];
            }
            bits[v] = u8::from(total < 0.0);
            for &e in &var_edges[v] {
                v2c[e] = total - c2v[e];
            }
        }

        if all_checks_hold(&bits) {
            return Some(bits[..K].to_vec());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

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
        for i in 0..MB {
            for r in 0..Z {
                let mut acc = 0u8;
                for j in 0..NB {
                    let s = BASE[i][j];
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

    /// Not a hard assert: prints the success rate at Eb/N0 = 2.5 dB over 200 trials.
    /// Run with `-- --nocapture` to see the line.
    #[test]
    fn success_rate_at_2_5_db() {
        let trials = 200;
        let ebn0 = 10f64.powf(2.5 / 10.0);
        // Rate 1/2, unit-energy BPSK: sigma^2 = 1 / (2 R Eb/N0).
        let sigma2 = 1.0 / (2.0 * 0.5 * ebn0);
        let sigma = sigma2.sqrt();
        let mut rng = Rng(4);
        let mut ok = 0usize;
        for _ in 0..trials {
            let info = rng.info();
            let cw = encode(&info);
            let llr: Vec<f32> = cw
                .iter()
                .map(|&b| {
                    let x = if b == 0 { 1.0 } else { -1.0 };
                    let y = x + sigma * rng.gaussian();
                    (2.0 * y / sigma2) as f32
                })
                .collect();
            if decode(&llr, 50) == Some(info) {
                ok += 1;
            }
        }
        println!(
            "LDPC Eb/N0 2.5 dB: {ok}/{trials} decoded ({:.1}%)",
            100.0 * ok as f64 / trials as f64
        );
    }
}
