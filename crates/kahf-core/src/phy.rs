//! OFDM PHY at 8000 Hz audio rate (real samples).
//!
//! Numerology: FFT size 512 (15.625 Hz carrier spacing), cyclic prefix 64,
//! so one symbol is 576 samples. Active carriers are bins 20..=172
//! (312.5 Hz to 2687.5 Hz), 153 in total. Every 4th active carrier is a BPSK
//! pilot, leaving 114 data carriers. Data carriers carry Gray-mapped
//! QPSK / 16QAM / 64QAM (unit average power).
//!
//! A frame is sync symbol A (pilot carriers only, so its waveform repeats every
//! 128 samples), channel symbol B (all active carriers, known BPSK), then the
//! data symbols.
//!
//! Receiver: the analytic signal (FFT Hilbert) feeds a complex lag-128
//! Schmidl-Cox metric (coarse timing) and the CFO estimate (unambiguous to
//! +-31.25 Hz). The CFO is removed by mixing. Fine timing is a normalised
//! correlation with symbol A inside one search window. Per symbol, a weighted
//! pilot fit of common phase and linear phase slope absorbs residual CFO and
//! sample-clock drift.
use std::f64::consts::PI;
use std::ops::{Add, Mul, Sub};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modulation { Qpsk, Qam16, Qam64 }

const FS: f64 = 8000.0;
const N: usize = 512;
const CP: usize = 64;
const SYM: usize = N + CP;
/// Repeat period of sync symbol A (N / 4). Active bins start on a multiple of 4.
const PER: usize = N / 4;
const FIRST_BIN: usize = 20;
const LAST_BIN: usize = 172;
const NACT: usize = LAST_BIN - FIRST_BIN + 1;
const PILOT_EVERY: usize = 4;
const MAX_BPS: usize = 6;
const MIN_CORR: f64 = 0.5;
const PEAK: f64 = 0.95;

#[derive(Clone, Copy, Debug, PartialEq)]
struct C { re: f64, im: f64 }

impl C {
    const ZERO: C = C { re: 0.0, im: 0.0 };
    fn new(re: f64, im: f64) -> C { C { re, im } }
    fn conj(self) -> C { C::new(self.re, -self.im) }
    fn norm2(self) -> f64 { self.re * self.re + self.im * self.im }
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

/// Iterative radix-2 FFT plan (forward, e^{-j}), twiddles and bit reversal precomputed.
struct Fft { n: usize, tw: Vec<C>, rev: Vec<usize> }

impl Fft {
    fn new(n: usize) -> Fft {
        let bits = n.trailing_zeros();
        let tw = (0..n / 2)
            .map(|k| {
                let a = -2.0 * PI * k as f64 / n as f64;
                C::new(a.cos(), a.sin())
            })
            .collect();
        let rev = (0..n).map(|i| i.reverse_bits() >> (usize::BITS - bits)).collect();
        Fft { n, tw, rev }
    }

    /// In-place forward transform. `a.len()` must equal the plan size.
    fn forward(&self, a: &mut [C]) {
        let n = self.n;
        for i in 0..n {
            let j = self.rev[i];
            if i < j {
                a.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let step = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..half {
                    let u = a[start + k];
                    let v = a[start + k + half] * self.tw[k * step];
                    a[start + k] = u + v;
                    a[start + k + half] = u - v;
                }
            }
            len <<= 1;
        }
    }
}

/// Analytic signal x + j*Hilbert(x), computed by FFT over the whole buffer.
fn analytic(x: &[f64]) -> Vec<C> {
    let m = x.len().next_power_of_two().max(2);
    let plan = Fft::new(m);
    let mut buf = vec![C::ZERO; m];
    for (b, &v) in buf.iter_mut().zip(x) {
        *b = C::new(v, 0.0);
    }
    plan.forward(&mut buf);
    for k in 1..m / 2 {
        buf[k] = buf[k] * 2.0;
    }
    for c in buf[m / 2 + 1..].iter_mut() {
        *c = C::ZERO;
    }
    // Inverse transform: IFFT(X) = conj(FFT(conj(X))) / m.
    for c in buf.iter_mut() {
        *c = c.conj();
    }
    plan.forward(&mut buf);
    let s = 1.0 / m as f64;
    buf.truncate(x.len());
    for c in buf.iter_mut() {
        *c = c.conj() * s;
    }
    buf
}

/// Known pseudo-random BPSK sign for index `i` (fixed, deterministic).
fn pn_sign(i: u64) -> f64 {
    let mut x = i.wrapping_add(1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 29;
    if x & 1 == 1 { 1.0 } else { -1.0 }
}

fn pilot_value(i: usize) -> f64 { pn_sign(i as u64 + 1000) }

fn is_pilot(i: usize) -> bool { i % PILOT_EVERY == 0 }

fn bits_per_axis(m: Modulation) -> usize {
    match m { Modulation::Qpsk => 1, Modulation::Qam16 => 2, Modulation::Qam64 => 3 }
}

fn mod_idx(m: Modulation) -> usize {
    match m { Modulation::Qpsk => 0, Modulation::Qam16 => 1, Modulation::Qam64 => 2 }
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

/// Precomputed, shared PHY state: FFT plan, carrier roles, known symbols, constellations.
struct Phy {
    fft: Fft,
    /// Active-carrier indices (0..NACT) that are pilots.
    pilots: Vec<usize>,
    /// Active-carrier indices that carry data.
    data: Vec<usize>,
    /// Known BPSK values on every active carrier (channel symbol B).
    pre: Vec<f64>,
    /// Time-domain sync symbol A (CP + body), 576 samples.
    sym_a: Vec<f64>,
    /// Time-domain channel symbol B (CP + body).
    sym_b: Vec<f64>,
    cons: [Vec<(usize, C)>; 3],
}

static PHY: OnceLock<Phy> = OnceLock::new();

fn phy() -> &'static Phy {
    PHY.get_or_init(Phy::new)
}

impl Phy {
    fn new() -> Phy {
        let pilots: Vec<usize> = (0..NACT).filter(|&i| is_pilot(i)).collect();
        let data: Vec<usize> = (0..NACT).filter(|&i| !is_pilot(i)).collect();
        let pre: Vec<f64> = (0..NACT).map(|i| pn_sign(i as u64)).collect();
        let mut sync_vals = vec![C::ZERO; NACT];
        for &i in &pilots {
            sync_vals[i] = C::new(pilot_value(i), 0.0);
        }
        let b_vals: Vec<C> = pre.iter().map(|&s| C::new(s, 0.0)).collect();
        let mut p = Phy {
            fft: Fft::new(N),
            pilots,
            data,
            pre,
            sym_a: Vec::new(),
            sym_b: Vec::new(),
            cons: [
                constellation(Modulation::Qpsk),
                constellation(Modulation::Qam16),
                constellation(Modulation::Qam64),
            ],
        };
        let mut buf = vec![C::ZERO; N];
        let mut a = Vec::with_capacity(SYM);
        let mut b = Vec::with_capacity(SYM);
        p.synth_into(&sync_vals, &mut buf, &mut a);
        p.synth_into(&b_vals, &mut buf, &mut b);
        p.sym_a = a;
        p.sym_b = b;
        p
    }

    /// Append one CP + body time-domain symbol for active-carrier values `vals`.
    fn synth_into(&self, vals: &[C], buf: &mut [C], out: &mut Vec<f64>) {
        buf.iter_mut().for_each(|c| *c = C::ZERO);
        for (i, &v) in vals.iter().enumerate() {
            let k = FIRST_BIN + i;
            // Conjugate spectrum, so the forward FFT yields the real inverse transform.
            buf[k] = v.conj();
            buf[N - k] = v;
        }
        self.fft.forward(buf);
        let scale = 1.0 / N as f64;
        out.extend(buf[N - CP..].iter().map(|c| c.re * scale));
        out.extend(buf[..N].iter().map(|c| c.re * scale));
    }

    /// Active-carrier spectrum of the symbol whose CP starts at `start` in `y`.
    fn analyse(&self, y: &[C], start: usize, buf: &mut [C], out: &mut [C]) {
        for (n, b) in buf.iter_mut().enumerate() {
            *b = y[start + CP + n];
        }
        self.fft.forward(buf);
        for (i, o) in out.iter_mut().enumerate() {
            *o = buf[FIRST_BIN + i];
        }
    }
}

/// Map coded bits to audio samples (frame = sync A + channel B + data symbols).
pub fn modulate_frame(coded: &[u8], m: Modulation) -> Vec<f32> {
    let p = phy();
    let bps = 2 * bits_per_axis(m);
    let per_sym = p.data.len() * bps;
    let n_sym = (coded.len() + per_sym - 1) / per_sym;

    let mut out: Vec<f64> = Vec::with_capacity(SYM * (2 + n_sym));
    out.extend_from_slice(&p.sym_a);
    out.extend_from_slice(&p.sym_b);

    let mut buf = vec![C::ZERO; N];
    let mut vals = vec![C::ZERO; NACT];
    for &i in &p.pilots {
        vals[i] = C::new(pilot_value(i), 0.0);
    }
    let mut bits = [0u8; MAX_BPS];
    for s in 0..n_sym {
        for (d, &ci) in p.data.iter().enumerate() {
            let base = s * per_sym + d * bps;
            for j in 0..bps {
                bits[j] = coded.get(base + j).map_or(0, |b| b & 1);
            }
            vals[ci] = map_symbol(&bits[..bps], m);
        }
        p.synth_into(&vals, &mut buf, &mut out);
    }

    let peak = out.iter().fold(0f64, |a, v| a.max(v.abs()));
    let g = if peak > 0.0 { PEAK / peak } else { 1.0 };
    out.iter().map(|&v| (v * g) as f32).collect()
}

/// Find the frame in `samples`, equalise, return coded-bit LLRs (positive = 0), or None.
pub fn demodulate_frame(samples: &[f32], m: Modulation, n_bits: usize) -> Option<Vec<f32>> {
    let p = phy();
    let x: Vec<f64> = samples.iter().map(|&v| v as f64).collect();
    if x.len() < SYM {
        return None;
    }
    let power = x.iter().map(|v| v * v).sum::<f64>() / x.len() as f64;
    if power <= 0.0 {
        return None;
    }

    // 1. Coarse timing: complex Schmidl-Cox metric with lag PER and window PER.
    //    M is about 1 from the symbol A start on, and ramps up over the PER samples before it.
    //    The first offset that clears MIN_CORR (on a non-silent window) is taken.
    let z = analytic(&x);
    let last = x.len().checked_sub(2 * PER)?;
    let floor = 0.5 * PER as f64 * power;
    let mut pc = C::ZERO;
    let mut rc = 0f64;
    for n in 0..PER {
        pc = pc + z[n].conj() * z[n + PER];
        rc += z[n + PER].norm2();
    }
    let mut coarse = None;
    for d in 0..=last {
        if rc > floor && pc.norm2() >= MIN_CORR * rc * rc {
            coarse = Some(d);
            break;
        }
        if d == last {
            break;
        }
        pc = pc + z[d + PER].conj() * z[d + 2 * PER] - z[d].conj() * z[d + PER];
        rc += z[d + 2 * PER].norm2() - z[d + PER].norm2();
    }
    let d_c = coarse?;

    // 2. CFO from the phase of the lag-PER product over symbol A.
    //    Unambiguous to +-FS / (2 PER) = +-31.25 Hz.
    let span = (SYM - PER).min(z.len() - d_c - PER);
    let mut acc = C::ZERO;
    for n in 0..span {
        acc = acc + z[d_c + n].conj() * z[d_c + n + PER];
    }
    let cfo = acc.im.atan2(acc.re) * FS / (2.0 * PI * PER as f64);
    drop(z);

    // 3. Mix the real signal down by the CFO. Carriers land on their nominal
    //    bins; the mirror image lands far outside the active bins.
    let w0 = -2.0 * PI * cfo / FS;
    let step = C::new(w0.cos(), w0.sin());
    let mut ph = C::new(1.0, 0.0);
    let y: Vec<C> = x
        .iter()
        .map(|&v| {
            let s = ph * v;
            ph = ph * step;
            s
        })
        .collect();

    // 4. Fine timing: magnitude of the correlation with symbol A over one PER window.
    let tpl = &p.sym_a;
    let tpl_e: f64 = tpl.iter().map(|v| v * v).sum();
    let mut start = d_c;
    let mut best = -1.0;
    for t in d_c..=d_c + PER {
        if t + SYM > y.len() {
            break;
        }
        let mut acc = C::ZERO;
        let mut e = 0f64;
        for (n, &tv) in tpl.iter().enumerate() {
            let v = y[t + n];
            acc = acc + v * tv;
            e += v.norm2();
        }
        let r = acc.norm2() / (e * tpl_e).max(1e-30);
        if r > best {
            best = r;
            start = t;
        }
    }

    if n_bits == 0 {
        return Some(Vec::new());
    }
    let bps = 2 * bits_per_axis(m);
    let nd = p.data.len();
    let per_sym = nd * bps;
    let n_sym = (n_bits + per_sym - 1) / per_sym;
    if start + SYM * (2 + n_sym) > y.len() {
        return None;
    }

    let mut fb = vec![C::ZERO; N];
    let mut yv = vec![C::ZERO; NACT];

    // 5. Channel estimate from symbol B (known BPSK on every active carrier).
    p.analyse(&y, start + SYM, &mut fb, &mut yv);
    let h: Vec<C> = yv.iter().zip(&p.pre).map(|(v, &s)| *v * s).collect();
    let hw: Vec<f64> = h.iter().map(|c| c.norm2()).collect();

    // 6. Equalise each data symbol. Fit phase = a + b*i over the pilots
    //    (|H|^2 weighted): a is the common phase, b the sample-clock slope.
    //    Remove it from every carrier and accumulate a pilot noise estimate.
    let mut zs: Vec<C> = Vec::with_capacity(n_sym * nd);
    let mut resid = 0f64;
    let mut cnt = 0usize;
    let mut z = vec![C::ZERO; NACT];
    for s in 0..n_sym {
        p.analyse(&y, start + SYM * (2 + s), &mut fb, &mut yv);
        for i in 0..NACT {
            z[i] = yv[i].div(h[i]);
        }
        let (mut sw, mut sx, mut sxx, mut st, mut sxt) = (0f64, 0f64, 0f64, 0f64, 0f64);
        for &i in &p.pilots {
            let u = z[i] * pilot_value(i);
            let th = u.im.atan2(u.re);
            let xi = i as f64;
            let wi = hw[i];
            sw += wi;
            sx += wi * xi;
            sxx += wi * xi * xi;
            st += wi * th;
            sxt += wi * xi * th;
        }
        let den = sw * sxx - sx * sx;
        let (a, b) = if sw > 0.0 && den.abs() > 1e-12 {
            let b = (sw * sxt - sx * st) / den;
            ((st - b * sx) / sw, b)
        } else {
            (0.0, 0.0)
        };
        for (i, zi) in z.iter_mut().enumerate() {
            let ang = a + b * i as f64;
            *zi = *zi * C::new(ang.cos(), -ang.sin());
        }
        for &i in &p.pilots {
            let e = z[i] - C::new(pilot_value(i), 0.0);
            resid += e.norm2() * hw[i];
            cnt += 1;
        }
        for &i in &p.data {
            zs.push(z[i]);
        }
    }
    let sigma2 = (resid / cnt as f64).max(1e-12);

    // 7. Max-log LLRs, scaled by channel gain over noise: s2_k = sigma2 / |H_k|^2.
    let table = &p.cons[mod_idx(m)];
    let mut llr: Vec<f32> = Vec::with_capacity(n_sym * per_sym);
    for (k, &zk) in zs.iter().enumerate() {
        let i = p.data[k % nd];
        let s2 = sigma2 / hw[i].max(1e-12);
        let mut d0 = [f64::INFINITY; MAX_BPS];
        let mut d1 = [f64::INFINITY; MAX_BPS];
        for &(v, pt) in table.iter() {
            let dist = (zk - pt).norm2();
            for j in 0..bps {
                let t = if ((v >> (bps - 1 - j)) & 1) == 0 { &mut d0 } else { &mut d1 };
                if dist < t[j] {
                    t[j] = dist;
                }
            }
        }
        for j in 0..bps {
            llr.push(((d1[j] - d0[j]) / s2) as f32);
        }
    }
    llr.truncate(n_bits);
    Some(llr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

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

    /// Real carrier frequency offset: shift the analytic signal by `hz`.
    fn cfo(x: &[f32], hz: f64) -> Vec<f32> {
        let x64: Vec<f64> = x.iter().map(|&v| v as f64).collect();
        analytic(&x64)
            .iter()
            .enumerate()
            .map(|(n, c)| {
                let ph = 2.0 * PI * hz * n as f64 / FS;
                (c.re * ph.cos() - c.im * ph.sin()) as f32
            })
            .collect()
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

    #[test]
    fn phy_cfo_20hz_qpsk_20db() {
        let mut rng = Rng(2024);
        let bits = rng.bits(2000);
        let tx = modulate_frame(&bits, Modulation::Qpsk);
        let mut rx = vec![0f32; 100];
        rx.extend(cfo(&tx, 20.0));
        rx.extend(vec![0f32; 100]);
        let sig = &rx[100..100 + tx.len()];
        let power = sig.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / sig.len() as f64;
        let sigma = (power / 100.0).sqrt(); // 20 dB SNR
        for v in rx.iter_mut() {
            *v += (sigma * rng.gauss()) as f32;
        }
        let llr = demodulate_frame(&rx, Modulation::Qpsk, bits.len()).expect("preamble not found");
        let errs = hard(&llr).iter().zip(&bits).filter(|(a, b)| a != b).count();
        eprintln!("QPSK at 20 Hz CFO, 20 dB SNR: {errs} bit errors / {}", bits.len());
        assert_eq!(errs, 0);
    }

    #[test]
    fn phy_sample_clock_50ppm_qpsk() {
        let mut rng = Rng(31337);
        let bits = rng.bits(4000);
        let tx = modulate_frame(&bits, Modulation::Qpsk);
        // Receiver sample clock 50 ppm fast: rx[k] = tx(k * ratio), cubic Lagrange interpolation.
        let ratio = 1.0 + 50e-6;
        let mut pad = vec![0f32; 16];
        pad.extend_from_slice(&tx);
        pad.extend(vec![0f32; 16]);
        let at = |i: usize| pad[i] as f64;
        let mut rx: Vec<f32> = Vec::new();
        for k in 0usize.. {
            let p = k as f64 * ratio;
            let i = p.floor() as usize;
            if i + 2 >= pad.len() {
                break;
            }
            if i == 0 {
                rx.push(0.0);
                continue;
            }
            let mu = p - i as f64;
            let (x0, x1, x2, x3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
            let v = x0 * (-mu * (mu - 1.0) * (mu - 2.0) / 6.0)
                + x1 * ((mu + 1.0) * (mu - 1.0) * (mu - 2.0) / 2.0)
                + x2 * (-(mu + 1.0) * mu * (mu - 2.0) / 2.0)
                + x3 * ((mu + 1.0) * mu * (mu - 1.0) / 6.0);
            rx.push(v as f32);
        }
        let llr = demodulate_frame(&rx, Modulation::Qpsk, bits.len()).expect("preamble not found");
        let errs = hard(&llr).iter().zip(&bits).filter(|(a, b)| a != b).count();
        eprintln!("QPSK at 50 ppm sample clock offset: {errs} bit errors / {}", bits.len());
        assert_eq!(errs, 0);
    }

    #[test]
    fn phy_frame_timing() {
        let mut rng = Rng(99);
        let bits = rng.bits(2000);
        let tx = modulate_frame(&bits, Modulation::Qpsk);
        let rounds: u32 = 50;
        let t0 = Instant::now();
        let mut ok = 0u32;
        for _ in 0..rounds {
            let llr = demodulate_frame(&tx, Modulation::Qpsk, bits.len()).expect("preamble not found");
            if hard(&llr) == bits {
                ok += 1;
            }
        }
        let demod = t0.elapsed() / rounds;
        let t1 = Instant::now();
        for _ in 0..rounds {
            let _ = modulate_frame(&bits, Modulation::Qpsk);
        }
        let modt = t1.elapsed() / rounds;
        eprintln!(
            "PHY timing (QPSK, 2000 bits, {} samples/frame): demod {:?}/frame, modulate {:?}/frame, {ok}/{rounds} exact",
            tx.len(),
            demod,
            modt
        );
        assert_eq!(ok, rounds);
    }
}
