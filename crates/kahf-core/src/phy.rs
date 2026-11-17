//! OFDM PHY at 8000 Hz audio rate (real samples).
//!
//! Numerology: FFT size 256 (31.25 Hz carrier spacing), cyclic prefix 32,
//! so one symbol is 288 samples. Active carriers are bins 10..=86
//! (about 312 Hz to 2687 Hz). Every 4th active carrier is a BPSK pilot,
//! the rest carry Gray-mapped QPSK / 16QAM / 64QAM (unit average power).
//! A frame is one known BPSK preamble symbol followed by data symbols.
use std::f64::consts::PI;
use std::ops::{Add, Mul, Sub};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modulation { Qpsk, Qam16, Qam64 }

const N: usize = 256;
const CP: usize = 32;
const SYM: usize = N + CP;
const FIRST_BIN: usize = 10;
const LAST_BIN: usize = 86;
const NACT: usize = LAST_BIN - FIRST_BIN + 1;
const PILOT_EVERY: usize = 4;
const MIN_CORR: f64 = 0.5;
const PEAK: f64 = 0.95;

#[derive(Clone, Copy, Debug, PartialEq)]
struct C { re: f64, im: f64 }

impl C {
    const ZERO: C = C { re: 0.0, im: 0.0 };
    fn new(re: f64, im: f64) -> C { C { re, im } }
    fn conj(self) -> C { C::new(self.re, -self.im) }
    fn norm2(self) -> f64 { self.re * self.re + self.im * self.im }
    fn abs(self) -> f64 { self.norm2().sqrt() }
    /// Division with a floor on |h|^2 to avoid blow-up on dead carriers.
    fn div(self, h: C) -> C { self * h.conj() * (1.0 / h.norm2().max(1e-12)) }
}
impl Add for C { type Output = C; fn add(self, o: C) -> C { C::new(self.re + o.re, self.im + o.im) } }
impl Sub for C { type Output = C; fn sub(self, o: C) -> C { C::new(self.re - o.re, self.im - o.im) } }
impl Mul for C {
    type Output = C;
    fn mul(self, o: C) -> C { C::new(self.re * o.re - self.im * o.im, self.re * o.im + self.im * o.re) }
}
impl Mul<f64> for C { type Output = C; fn mul(self, k: f64) -> C { C::new(self.re * k, self.im * k) } }

/// Iterative radix-2 FFT (forward, e^{-j}). Input length must be a power of two.
fn fft(input: &[C]) -> Vec<C> {
    let n = input.len();
    let bits = n.trailing_zeros();
    let mut a = vec![C::ZERO; n];
    for (i, &v) in input.iter().enumerate() {
        a[i.reverse_bits() >> (usize::BITS - bits)] = v;
    }
    let mut len = 2;
    while len <= n {
        let half = len / 2;
        let ang = -2.0 * PI / len as f64;
        for start in (0..n).step_by(len) {
            for k in 0..half {
                let w = C::new((ang * k as f64).cos(), (ang * k as f64).sin());
                let u = a[start + k];
                let v = a[start + k + half] * w;
                a[start + k] = u + v;
                a[start + k + half] = u - v;
            }
        }
        len <<= 1;
    }
    a
}

/// Known pseudo-random BPSK sign for index `i` (fixed, deterministic).
fn pn_sign(i: u64) -> f64 {
    let mut x = i.wrapping_add(1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 29;
    if x & 1 == 1 { 1.0 } else { -1.0 }
}

fn preamble_values() -> Vec<C> {
    (0..NACT).map(|i| C::new(pn_sign(i as u64), 0.0)).collect()
}

fn pilot_value(i: usize) -> f64 { pn_sign(i as u64 + 1000) }

fn is_pilot(i: usize) -> bool { i % PILOT_EVERY == 0 }

/// Active-carrier indices that carry data (non-pilot).
fn data_idx() -> Vec<usize> { (0..NACT).filter(|&i| !is_pilot(i)).collect() }

fn bits_per_axis(m: Modulation) -> usize {
    match m { Modulation::Qpsk => 1, Modulation::Qam16 => 2, Modulation::Qam64 => 3 }
}

/// Gray-coded PAM level for `k` bits on one axis (M = 2^k levels, odd, centred).
fn axis_level(bits: &[u8], mm: usize) -> f64 {
    let mut b = 0usize;
    for &x in bits { b = (b << 1) | (x & 1) as usize; }
    // inverse Gray code: binary index whose Gray code is b
    let mut i = b;
    let mut s = b >> 1;
    while s > 0 { i ^= s; s >>= 1; }
    2.0 * i as f64 - (mm as f64 - 1.0)
}

/// Map 2k bits (first k -> I, next k -> Q) to a unit-average-power point.
fn map_symbol(bits: &[u8], m: Modulation) -> C {
    let k = bits_per_axis(m);
    let mm = 1usize << k;
    let norm = (2.0 * ((mm * mm - 1) as f64) / 3.0).sqrt();
    C::new(axis_level(&bits[..k], mm) / norm, axis_level(&bits[k..2 * k], mm) / norm)
}

/// All constellation points as (bit pattern, point); bit j of the pattern is bits[j].
fn constellation(m: Modulation) -> Vec<(usize, C)> {
    let bps = 2 * bits_per_axis(m);
    (0..1usize << bps)
        .map(|v| {
            let bits: Vec<u8> = (0..bps).map(|j| ((v >> (bps - 1 - j)) & 1) as u8).collect();
            (v, map_symbol(&bits, m))
        })
        .collect()
}

/// Build one time-domain OFDM symbol (CP + body) from active-carrier values.
fn synth(vals: &[C]) -> Vec<f64> {
    let mut x = vec![C::ZERO; N];
    for (i, &v) in vals.iter().enumerate() {
        let k = FIRST_BIN + i;
        x[k] = v;
        x[N - k] = v.conj(); // Hermitian symmetry keeps the output real
    }
    // inverse FFT via forward FFT of the conjugate; real part is the signal
    let xc: Vec<C> = x.iter().map(|c| c.conj()).collect();
    let body: Vec<f64> = fft(&xc).iter().map(|c| c.re / N as f64).collect();
    let mut out = Vec::with_capacity(SYM);
    out.extend_from_slice(&body[N - CP..]);
    out.extend_from_slice(&body);
    out
}

/// FFT of the symbol whose CP starts at `start`; returns the active carriers.
fn analyse(x: &[f64], start: usize) -> Vec<C> {
    let buf: Vec<C> = (0..N).map(|n| C::new(x[start + CP + n], 0.0)).collect();
    let f = fft(&buf);
    (0..NACT).map(|i| f[FIRST_BIN + i]).collect()
}

/// Map coded bits to audio samples (frame = preamble + data symbols).
pub fn modulate_frame(coded: &[u8], m: Modulation) -> Vec<f32> {
    let bps = 2 * bits_per_axis(m);
    let data = data_idx();
    let per_sym = data.len() * bps;
    let n_sym = (coded.len() + per_sym - 1) / per_sym;
    let bit = |idx: usize| -> u8 { coded.get(idx).map_or(0, |b| b & 1) };

    let mut out = synth(&preamble_values());
    for s in 0..n_sym {
        let mut vals = vec![C::ZERO; NACT];
        for (i, v) in vals.iter_mut().enumerate() {
            if is_pilot(i) {
                *v = C::new(pilot_value(i), 0.0);
            }
        }
        for (d, &ci) in data.iter().enumerate() {
            let base = s * per_sym + d * bps;
            let bits: Vec<u8> = (0..bps).map(|j| bit(base + j)).collect();
            vals[ci] = map_symbol(&bits, m);
        }
        out.extend(synth(&vals));
    }

    let peak = out.iter().fold(0f64, |p, v| p.max(v.abs()));
    let g = if peak > 0.0 { PEAK / peak } else { 1.0 };
    out.iter().map(|&v| (v * g) as f32).collect()
}

/// Find frame in `samples`, equalise, return coded-bit LLRs (positive = 0), or None.
pub fn demodulate_frame(samples: &[f32], m: Modulation, n_bits: usize) -> Option<Vec<f32>> {
    let x: Vec<f64> = samples.iter().map(|&v| v as f64).collect();
    if x.len() < SYM {
        return None;
    }

    // Preamble search: normalised cross-correlation. Every frame starts with the
    // same preamble, so a stream holds several equal peaks. Take the first offset
    // that clears MIN_CORR, then the peak within one symbol after it. A global
    // maximum would pick a later frame whenever two peaks are about equal.
    let tpl = synth(&preamble_values());
    let tpl_e: f64 = tpl.iter().map(|v| v * v).sum();
    let last = x.len() - SYM;
    let corr_at = |t: usize| -> f64 {
        let w = &x[t..t + SYM];
        let dot: f64 = w.iter().zip(&tpl).map(|(a, b)| a * b).sum();
        let e: f64 = w.iter().map(|v| v * v).sum();
        dot / (tpl_e * e).sqrt().max(1e-30)
    };
    let t0 = (0..=last).find(|&t| corr_at(t) >= MIN_CORR)?;
    let mut best_t = t0;
    let mut best_r = corr_at(t0);
    for t in (t0 + 1)..=(t0 + SYM).min(last) {
        let r = corr_at(t);
        if r > best_r {
            best_r = r;
            best_t = t;
        }
    }
    if n_bits == 0 {
        return Some(Vec::new());
    }

    let start = best_t;
    let bps = 2 * bits_per_axis(m);
    let data = data_idx();
    let per_sym = data.len() * bps;
    let n_sym = (n_bits + per_sym - 1) / per_sym;
    if start + SYM * (1 + n_sym) > x.len() {
        return None;
    }

    // Channel estimate from the known preamble (values are +-1, so H = Y * P).
    let pre = preamble_values();
    let h: Vec<C> = analyse(&x, start).iter().zip(&pre).map(|(y, p)| *y * p.re).collect();

    // Equalise each data symbol, remove common phase using the pilots,
    // and accumulate a pilot-based noise estimate (referred to the raw input).
    let pilots: Vec<usize> = (0..NACT).filter(|&i| is_pilot(i)).collect();
    let mut eqs: Vec<Vec<C>> = Vec::with_capacity(n_sym);
    let mut resid = 0f64;
    let mut cnt = 0usize;
    for s in 0..n_sym {
        let y = analyse(&x, start + SYM * (1 + s));
        let mut z: Vec<C> = (0..NACT).map(|i| y[i].div(h[i])).collect();
        let acc = pilots.iter().fold(C::ZERO, |a, &i| a + z[i] * pilot_value(i));
        let mag = acc.abs();
        if mag > 1e-12 {
            let rot = acc.conj() * (1.0 / mag);
            for v in z.iter_mut() {
                *v = *v * rot;
            }
        }
        for &i in &pilots {
            let e = z[i] - C::new(pilot_value(i), 0.0);
            resid += e.norm2() * h[i].norm2();
            cnt += 1;
        }
        eqs.push(z);
    }
    let sigma2 = (resid / cnt as f64).max(1e-12);

    // Max-log LLRs, scaled by channel gain over noise: s2_k = sigma2 / |H_k|^2.
    let table = constellation(m);
    let mut llr: Vec<f32> = Vec::with_capacity(n_sym * per_sym);
    for s in 0..n_sym {
        for &i in &data {
            let z = eqs[s][i];
            let s2 = sigma2 / h[i].norm2().max(1e-12);
            let mut d0 = vec![f64::INFINITY; bps];
            let mut d1 = vec![f64::INFINITY; bps];
            for (v, pt) in &table {
                let dist = (z - *pt).norm2();
                for j in 0..bps {
                    let target = if ((v >> (bps - 1 - j)) & 1) == 0 { &mut d0 } else { &mut d1 };
                    if dist < target[j] {
                        target[j] = dist;
                    }
                }
            }
            for j in 0..bps {
                llr.push(((d1[j] - d0[j]) / s2) as f32);
            }
        }
    }
    llr.truncate(n_bits);
    Some(llr)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u64);
    impl Rng {
        fn next_u64(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn uniform(&mut self) -> f64 {
            ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        }
        fn gauss(&mut self) -> f64 {
            let u1 = self.uniform();
            let u2 = self.uniform();
            (-2.0 * u1.ln()).sqrt() * (2.0 * PI * u2).cos()
        }
        fn bits(&mut self, n: usize) -> Vec<u8> {
            (0..n).map(|_| (self.next_u64() & 1) as u8).collect()
        }
    }

    const ALL: [Modulation; 3] = [Modulation::Qpsk, Modulation::Qam16, Modulation::Qam64];

    fn hard(llr: &[f32]) -> Vec<u8> {
        llr.iter().map(|&l| u8::from(l < 0.0)).collect()
    }

    #[test]
    fn phy_noiseless_roundtrip_all_modulations() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for m in ALL {
            let bits = rng.bits(1000);
            let tx = modulate_frame(&bits, m);
            assert!(tx.iter().all(|v| v.abs() <= 1.0), "{m:?} peak above 1.0");
            let llr = demodulate_frame(&tx, m, bits.len()).expect("preamble not found");
            assert_eq!(llr.len(), bits.len());
            assert_eq!(hard(&llr), bits, "{m:?}");
        }
    }

    #[test]
    fn phy_delay_and_gain_roundtrip() {
        let mut rng = Rng(12345);
        for m in ALL {
            let bits = rng.bits(900);
            let tx = modulate_frame(&bits, m);
            let mut rx = vec![0f32; 37];
            rx.extend(tx.iter().map(|&v| 0.7 * v));
            rx.extend(vec![0f32; 50]);
            let llr = demodulate_frame(&rx, m, bits.len()).expect("preamble not found");
            assert_eq!(hard(&llr), bits, "{m:?}");
        }
    }

    #[test]
    fn phy_noisy_qpsk_hard_decisions() {
        let mut rng = Rng(777);
        let bits = rng.bits(2000);
        let tx = modulate_frame(&bits, Modulation::Qpsk);
        let power = tx.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / tx.len() as f64;
        let sigma = (power / 100.0).sqrt(); // 20 dB sample SNR
        let rx: Vec<f32> = tx.iter().map(|&v| v + (sigma * rng.gauss()) as f32).collect();
        let llr = demodulate_frame(&rx, Modulation::Qpsk, bits.len()).expect("preamble not found");
        let errs = hard(&llr).iter().zip(&bits).filter(|(a, b)| a != b).count();
        let ber = errs as f64 / bits.len() as f64;
        eprintln!("QPSK at 20 dB SNR: BER = {ber:.5} ({errs}/{})", bits.len());
        assert!(ber <= 0.01, "BER {ber} above 1%");
    }
}
