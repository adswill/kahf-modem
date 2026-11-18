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
pub enum Modulation {
    /// KAMI-32 real Walsh signalling (see `kami`), the bottom rung of the ladder (frame id 3).
    /// It is not an OFDM mode: `modulate_frame` / `demodulate_frame` hand it to `kami`.
    Kami32,
    /// KAMI-16: 4 bits per 16-chip Walsh symbol, 2000 bit/s raw (frame id 4).
    Kami16,
    /// KAMI-8: 3 bits per 8-chip Walsh symbol, 3000 bit/s raw (frame id 5).
    Kami8,
    /// KAMI-8H: 8-chip Walsh words with four amplitude levels, 5 bits per symbol, 5000 bit/s raw
    /// (frame id 6). Needs about 3 dB; faster than QPSK.
    Kami8H,
    Qpsk,
    Qam16,
    Qam64,
}

impl Modulation {
    /// Symbol shape of the KAMI modes, None for the OFDM modes.
    pub fn kami_shape(self) -> Option<crate::kami::Shape> {
        use crate::kami::Shape;
        match self {
            Modulation::Kami32 => Some(Shape::walsh(5)),
            Modulation::Kami16 => Some(Shape::walsh(4)),
            Modulation::Kami8 => Some(Shape::walsh(3)),
            Modulation::Kami8H => Some(crate::kami::SHAPE_8H),
            _ => None,
        }
    }
}

/// SNR (dB, per real sample) below which KAMI-32 is chosen over QPSK. KAMI-32 clears 1 percent
/// frame loss near -5 dB, QPSK only above this point (1 percent loss near 1 dB on awgn, measured 100 frames per point, see the
/// integration report); above it QPSK is faster (about 0.36 s a frame against 0.65 s).
pub const QPSK_FLOOR_DB: f32 = 1.0;

/// SNR (dB) from which KAMI-16 (2000 bit/s, 0.46 s a frame) is chosen over KAMI-32: it clears
/// 1 percent frame loss near -2 dB (100 frames per point, awgn). KAMI-8 clears it near +1 dB and is
/// no faster than QPSK (0.35 s against 0.36 s), so the SNR chooser never picks it; it stays on
/// the link ladder.
pub const KAMI16_FLOOR_DB: f32 = -2.0;

/// SNR (dB) from which KAMI-8H (5000 bit/s raw, 0.26 s a frame against 0.36 s for QPSK) is chosen
/// over QPSK: 10 percent frame loss near 4 dB, 2 percent near 5 dB (100 frames per point, awgn).
pub const KAMI8H_FLOOR_DB: f32 = 5.5;

/// Modulation for a measured awgn SNR (dB, per real sample, as in `kahf-cli bler`).
/// The ladder, lowest rung first: KAMI-32, QPSK, 16QAM, 64QAM.
/// Thresholds are the lowest 0.5 dB points where the frame error rate of the mode
/// is below 1 percent (1000 frames per point, unit-power PHY, LDPC 50 iterations).
/// 16QAM first clears 1 percent at 6.5 dB; 64QAM at 10.5 dB.
pub fn choose_modulation(snr_db: f32) -> Modulation {
    if snr_db >= 10.5 {
        Modulation::Qam64
    } else if snr_db >= 6.5 {
        Modulation::Qam16
    } else if snr_db >= KAMI8H_FLOOR_DB {
        Modulation::Kami8H
    } else if snr_db >= QPSK_FLOOR_DB {
        Modulation::Qpsk
    } else if snr_db >= KAMI16_FLOOR_DB {
        Modulation::Kami16
    } else {
        Modulation::Kami32
    }
}

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
const MIN_CORR: f64 = 0.25;
const PEAK: f64 = 0.95;
/// Channel model for the estimate: an impulse response of this many samples.
/// Least-squares projection onto it averages noise over the carriers without
/// the bias of a boxcar, so echoes up to this delay are followed exactly.
const CIR_TAPS: usize = 24;
/// Scale on the max-log LLRs. 1.0 is the exact max-log value (distance difference over
/// the complex noise power). With the previous 0.4 the offset min-sum decoder gave
/// about 10 percent frame errors on the two-path echo at 1 dB.
const LLR_SCALE: f64 = 1.0;
/// Decision-directed channel refinement passes after the first equalisation.
const DD_PASSES: usize = 2;
/// Search room, in samples, beyond one frame for `demodulate_frame` (64 symbols, about 4.6 s).
/// Smaller margins change the power and edge estimates enough to flip a few marginal
/// frames in fade and clock-offset runs.
const SEARCH_MARGIN: usize = 64 * SYM;
/// Decay length (taps) of the exponential power-delay profile used as the channel prior.
const PDP_DECAY: f64 = 8.0;
/// Preamble correlation below which the aliased frequency hypotheses are also tried.
const ALIAS_CORR: f64 = 0.5;
/// Largest frequency offset (Hz) the joint search will consider.
const MAX_CFO: f64 = 100.0;

#[derive(Clone, Copy, Debug, PartialEq)]
struct C {
    re: f64,
    im: f64,
}

impl C {
    const ZERO: C = C { re: 0.0, im: 0.0 };
    fn new(re: f64, im: f64) -> C {
        C { re, im }
    }
    fn conj(self) -> C {
        C::new(self.re, -self.im)
    }
    fn norm2(self) -> f64 {
        self.re * self.re + self.im * self.im
    }
    /// Division with a floor on |h|^2 to avoid blow-up on dead carriers.
    fn div(self, h: C) -> C {
        self * h.conj() * (1.0 / h.norm2().max(1e-12))
    }
}
impl Add for C {
    type Output = C;
    fn add(self, o: C) -> C {
        C::new(self.re + o.re, self.im + o.im)
    }
}
impl Sub for C {
    type Output = C;
    fn sub(self, o: C) -> C {
        C::new(self.re - o.re, self.im - o.im)
    }
}
impl Mul for C {
    type Output = C;
    fn mul(self, o: C) -> C {
        C::new(self.re * o.re - self.im * o.im, self.re * o.im + self.im * o.re)
    }
}
impl Mul<f64> for C {
    type Output = C;
    fn mul(self, k: f64) -> C {
        C::new(self.re * k, self.im * k)
    }
}

/// Iterative radix-2 FFT plan (forward, e^{-j}), twiddles and bit reversal precomputed.
struct Fft {
    n: usize,
    tw: Vec<C>,
    rev: Vec<usize>,
}

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
        for (i, &j) in self.rev[..n].iter().enumerate() {
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
    for c in buf[1..m / 2].iter_mut() {
        *c = *c * 2.0;
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
    if x & 1 == 1 {
        1.0
    } else {
        -1.0
    }
}

fn pilot_value(i: usize) -> f64 {
    pn_sign(i as u64 + 1000)
}

fn is_pilot(i: usize) -> bool {
    i.is_multiple_of(PILOT_EVERY)
}

fn bits_per_axis(m: Modulation) -> usize {
    match m {
        Modulation::Kami32 | Modulation::Kami16 | Modulation::Kami8 | Modulation::Kami8H => {
            unreachable!("KAMI is not an OFDM mode")
        }
        Modulation::Qpsk => 1,
        Modulation::Qam16 => 2,
        Modulation::Qam64 => 3,
    }
}

fn mod_idx(m: Modulation) -> usize {
    match m {
        Modulation::Kami32 | Modulation::Kami16 | Modulation::Kami8 | Modulation::Kami8H => {
            unreachable!("KAMI is not an OFDM mode")
        }
        Modulation::Qpsk => 0,
        Modulation::Qam16 => 1,
        Modulation::Qam64 => 2,
    }
}

/// Gray-coded PAM level for `k` bits on one axis (M = 2^k levels, odd, centred).
fn axis_level(bits: &[u8], mm: usize) -> f64 {
    let mut b = 0usize;
    for &x in bits {
        b = (b << 1) | (x & 1) as usize;
    }
    // inverse Gray code: binary index whose Gray code is b
    let mut i = b;
    let mut s = b >> 1;
    while s > 0 {
        i ^= s;
        s >>= 1;
    }
    2.0 * i as f64 - (mm as f64 - 1.0)
}

/// Map 2k bits (first k -> I, next k -> Q) to a unit-average-power point.
fn map_symbol(bits: &[u8], m: Modulation) -> C {
    let k = bits_per_axis(m);
    let mm = 1usize << k;
    let norm = (2.0 * ((mm * mm - 1) as f64) / 3.0).sqrt();
    C::new(
        axis_level(&bits[..k], mm) / norm,
        axis_level(&bits[k..2 * k], mm) / norm,
    )
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
    /// DFT basis of the channel model: `cir[i * CIR_TAPS + n]` = e^{-j 2 pi k n / N}, k = FIRST_BIN + i.
    cir: Vec<C>,
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
        // Sync symbol A carries only the pilots, so boost them to the power of a
        // full data symbol. Otherwise its SNR sits about 5 dB below the frame's.
        let boost = (NACT as f64 / pilots.len() as f64).sqrt();
        let mut sync_vals = vec![C::ZERO; NACT];
        for &i in &pilots {
            sync_vals[i] = C::new(pilot_value(i) * boost, 0.0);
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
            cir: (0..NACT * CIR_TAPS)
                .map(|j| {
                    let (i, n) = (j / CIR_TAPS, j % CIR_TAPS);
                    let a = -2.0 * PI * ((FIRST_BIN + i) * n) as f64 / N as f64;
                    C::new(a.cos(), a.sin())
                })
                .collect(),
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

/// Solve a x = b for a small complex system by Gaussian elimination with partial pivoting.
fn solve(mut a: Vec<Vec<C>>, mut b: Vec<C>) -> Vec<C> {
    let n = b.len();
    for col in 0..n {
        let piv = (col..n)
            .max_by(|&i, &j| a[i][col].norm2().total_cmp(&a[j][col].norm2()))
            .unwrap_or(col);
        a.swap(col, piv);
        b.swap(col, piv);
        let d = a[col][col];
        if d.norm2() < 1e-30 {
            continue;
        }
        for row in col + 1..n {
            let f = a[row][col].div(d);
            let (upper, lower) = a.split_at_mut(row);
            for (dst, &src) in lower[0][col..n].iter_mut().zip(&upper[col][col..n]) {
                *dst = *dst - f * src;
            }
            b[row] = b[row] - f * b[col];
        }
    }
    let mut x = vec![C::ZERO; n];
    for row in (0..n).rev() {
        let mut s = b[row];
        for k in row + 1..n {
            s = s - a[row][k] * x[k];
        }
        x[row] = if a[row][row].norm2() < 1e-30 {
            C::ZERO
        } else {
            s.div(a[row][row])
        };
    }
    x
}

/// Weighted least-squares fit of the raw per-carrier estimates `est` (weights `wgt`)
/// to a channel impulse response of `CIR_TAPS` samples. Returns the fitted response
/// on every active carrier.
fn smooth_channel(p: &Phy, est: &[C], wgt: &[f64]) -> Vec<C> {
    let l = CIR_TAPS;
    let mut g = vec![vec![C::ZERO; l]; l];
    let mut r = vec![C::ZERO; l];
    for i in 0..NACT {
        let row = &p.cir[i * l..(i + 1) * l];
        for n in 0..l {
            let cn = row[n].conj() * wgt[i];
            r[n] = r[n] + cn * est[i];
            for m in 0..l {
                g[n][m] = g[n][m] + cn * row[m];
            }
        }
    }
    // Gaussian prior on the taps with an exponential power-delay profile: tap n has
    // power exp(-n / PDP_DECAY). The ridge term 1 / power pulls the late taps toward zero,
    // so the estimate does not carry noise from taps that carry no energy.
    for (n, row) in g.iter_mut().enumerate() {
        row[n] = row[n] + C::new((n as f64 / PDP_DECAY).exp(), 0.0);
    }
    let h = solve(g, r);
    (0..NACT)
        .map(|i| {
            p.cir[i * l..(i + 1) * l]
                .iter()
                .zip(&h)
                .fold(C::ZERO, |s, (&f, &hn)| s + f * hn)
        })
        .collect()
}

/// Output of one equalisation pass over the data symbols.
struct Equalised {
    /// Equalised data carriers, symbol by symbol.
    zs: Vec<C>,
    /// Weighted pilot residual sum and pilot count (noise estimate).
    resid: f64,
    cnt: usize,
}

/// Equalise `n_sym` data symbols with channel `h` (weights `hw` = |h|^2). Per symbol, a
/// weighted pilot fit of phase = a + b*i (common phase and sample-clock slope) is removed.
fn equalise(p: &Phy, y: &[C], start: usize, n_sym: usize, h: &[C], hw: &[f64]) -> Equalised {
    let mut fb = vec![C::ZERO; N];
    let mut yv = vec![C::ZERO; NACT];
    let mut out = Equalised {
        zs: Vec::with_capacity(n_sym * p.data.len()),
        resid: 0.0,
        cnt: 0,
    };
    let mut z = vec![C::ZERO; NACT];
    for s in 0..n_sym {
        p.analyse(y, start + SYM * (2 + s), &mut fb, &mut yv);
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
            out.resid += e.norm2() * hw[i];
            out.cnt += 1;
        }
        for &i in &p.data {
            out.zs.push(z[i]);
        }
    }
    out
}

/// Pseudo-random order of the transmitted bit slots, fixed for a given length. Slot t
/// carries coded bit `order[t]`, so a group of faded carriers does not hit one codeword.
fn slot_order(total: usize) -> Vec<usize> {
    let mut v: Vec<usize> = (0..total).collect();
    for i in (1..total).rev() {
        let mut x = (i as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        x ^= x >> 31;
        x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x ^= x >> 29;
        v.swap(i, (x % (i as u64 + 1)) as usize);
    }
    v
}

/// Map coded bits to audio samples (frame = sync A + channel B + data symbols).
pub fn modulate_frame(coded: &[u8], m: Modulation) -> Vec<f32> {
    if let Some(sh) = m.kami_shape() {
        return crate::kami::modulate_frame_shape(coded, sh);
    }
    let p = phy();
    let bps = 2 * bits_per_axis(m);
    let per_sym = p.data.len() * bps;
    let n_sym = coded.len().div_ceil(per_sym);

    let mut out: Vec<f64> = Vec::with_capacity(SYM * (2 + n_sym));
    out.extend_from_slice(&p.sym_a);
    out.extend_from_slice(&p.sym_b);

    let mut buf = vec![C::ZERO; N];
    let mut vals = vec![C::ZERO; NACT];
    for &i in &p.pilots {
        vals[i] = C::new(pilot_value(i), 0.0);
    }
    let order = slot_order(n_sym * per_sym);
    let mut bits = [0u8; MAX_BPS];
    for s in 0..n_sym {
        for (d, &ci) in p.data.iter().enumerate() {
            let base = s * per_sym + d * bps;
            for (j, slot) in bits[..bps].iter_mut().enumerate() {
                *slot = coded.get(order[base + j]).map_or(0, |b| b & 1);
            }
            vals[ci] = map_symbol(&bits[..bps], m);
        }
        p.synth_into(&vals, &mut buf, &mut out);
    }

    let peak = out.iter().fold(0f64, |a, v| a.max(v.abs()));
    let g = if peak > 0.0 { PEAK / peak } else { 1.0 };
    out.iter().map(|&v| (v * g) as f32).collect()
}

/// Mix the analytic signal by `-cfo` Hz and find the symbol-A start within one period of
/// `d_c`. Returns the mixed signal, the start and the normalised correlation there.
fn mix_and_time(p: &Phy, z: &[C], cfo: f64, d_c: usize) -> (Vec<C>, usize, f64) {
    let w0 = -2.0 * PI * cfo / FS;
    let step = C::new(w0.cos(), w0.sin());
    let mut ph = C::new(1.0, 0.0);
    let y: Vec<C> = z
        .iter()
        .map(|&v| {
            let s = ph * v;
            ph = ph * step;
            s
        })
        .collect();
    let tpl = &p.sym_a;
    let tpl_e: f64 = tpl.iter().map(|v| v * v).sum();
    let mut start = d_c;
    let mut best = -1.0;
    for t in d_c.saturating_sub(PER / 2)..d_c + PER / 2 {
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
    (y, start, best)
}

/// Find the frame in `samples`, equalise, return coded-bit LLRs (positive = 0), or None.
pub fn demodulate_frame(samples: &[f32], m: Modulation, n_bits: usize) -> Option<Vec<f32>> {
    if let Some(sh) = m.kami_shape() {
        return crate::kami::demodulate_frame_shape(samples, n_bits, sh);
    }
    let p = phy();
    // Only the region this frame can occupy is analysed: the frame itself plus
    // SEARCH_MARGIN samples of search room. Converting and transforming the whole
    // remaining stream on every call made stream decoding quadratic in its length.
    let frame_span = if n_bits == 0 {
        0
    } else {
        let bps = 2 * bits_per_axis(m);
        SYM * (2 + n_bits.div_ceil(p.data.len() * bps))
    };
    let win = samples.len().min(frame_span + SEARCH_MARGIN);
    let x: Vec<f64> = samples[..win].iter().map(|&v| v as f64).collect();
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
    // The window spans the whole periodic part of symbol A, so noise averages over
    // about SYM - PER samples. The first offset over MIN_CORR starts a search, which
    // keeps the best metric while it stays over MIN_CORR. The peak is the symbol start.
    let z = analytic(&x);
    let lw = SYM - PER;
    let floor = 0.5 * lw as f64 * power;
    let mut pc = C::ZERO;
    let mut rc = 0f64;
    for n in 0..lw {
        pc = pc + z[n].conj() * z[n + PER];
        rc += z[n + PER].norm2();
    }
    let mut coarse: Option<(f64, usize)> = None;
    let mut d = 0usize;
    loop {
        let met = if rc > floor { pc.norm2() / (rc * rc) } else { 0.0 };
        match coarse {
            None if met >= MIN_CORR * MIN_CORR => coarse = Some((met, d)),
            Some((best, _)) if met >= MIN_CORR * MIN_CORR => {
                if met > best {
                    coarse = Some((met, d));
                }
            }
            Some(_) => break,
            None => {}
        }
        if d + lw + PER >= z.len() {
            break;
        }
        pc = pc - z[d].conj() * z[d + PER] + z[d + lw].conj() * z[d + lw + PER];
        rc += z[d + lw + PER].norm2() - z[d + PER].norm2();
        d += 1;
    }
    let d_c = coarse?.1;

    // 2. CFO from the phase of the lag-PER product over symbol A.
    //    Unambiguous to +-FS / (2 PER) = +-31.25 Hz.
    let span = (SYM - PER).min(z.len() - d_c - PER);
    let mut acc = C::ZERO;
    for n in 0..span {
        acc = acc + z[d_c + n].conj() * z[d_c + n + PER];
    }
    let cfo = acc.im.atan2(acc.re) * FS / (2.0 * PI * PER as f64);

    // 3+4. Mix the analytic signal down by the CFO, then fine timing. Carriers land on
    //    their nominal bins. Mixing the real signal instead leaves a mirror image that
    //    rotates at twice the CFO; it corrupts the time-domain fine timing (wrong start).
    //    Fine timing is the magnitude of the correlation with symbol A over one PER window.
    //    Symbol A repeats every PER samples, so a window of one period holds exactly one
    //    copy of each phase. A wider window can pick a copy one period away from the frame.
    let (mut y, mut start, mut best) = mix_and_time(p, &z, cfo, d_c);
    // 4b. Joint time-frequency search. The lag-PER estimate wraps beyond +-31.25 Hz, so an
    //    offset outside that range gives a poor preamble correlation. Then the aliases
    //    cfo + k * FS / PER are tried (each with its own fine timing) and the best
    //    (start, frequency) pair is kept. Frames that already correlate well are untouched.
    if best < ALIAS_CORR {
        let alias = FS / PER as f64;
        for k in [-3i32, -2, -1, 1, 2, 3] {
            let c = cfo + k as f64 * alias;
            if c.abs() > MAX_CFO {
                continue;
            }
            let (y2, s2, r2) = mix_and_time(p, &z, c, d_c);
            if r2 > best {
                best = r2;
                y = y2;
                start = s2;
            }
        }
    }

    if n_bits == 0 {
        return Some(Vec::new());
    }
    let bps = 2 * bits_per_axis(m);
    let nd = p.data.len();
    let per_sym = nd * bps;
    let n_sym = n_bits.div_ceil(per_sym);
    if start + SYM * (2 + n_sym) > y.len() {
        return None;
    }

    let mut fb = vec![C::ZERO; N];
    let mut yv = vec![C::ZERO; NACT];

    // 5. Channel estimate from symbol B (known BPSK on every active carrier).
    p.analyse(&y, start + SYM, &mut fb, &mut yv);
    let eb: Vec<C> = yv.iter().zip(&p.pre).map(|(v, &s)| *v * s).collect();
    // Sync A (pilots only, boosted by `boost`) adds an estimate on each pilot.
    // Its weight is boost^2 (each pilot has that much more SNR than a data carrier).
    p.analyse(&y, start, &mut fb, &mut yv);
    let boost = (NACT as f64 / p.pilots.len() as f64).sqrt();
    let wa = boost * boost;
    let mut num = eb.clone();
    let mut den = vec![1.0f64; NACT];
    for &i in &p.pilots {
        let ea = yv[i] * (pilot_value(i) / boost);
        num[i] = eb[i] + ea * wa;
        den[i] = 1.0 + wa;
    }
    // 6. Smooth the estimate with the channel model, then equalise each data symbol.
    let raw: Vec<C> = num.iter().zip(&den).map(|(&n, &d)| n * (1.0 / d)).collect();
    let mut h = smooth_channel(p, &raw, &den);
    let mut hw: Vec<f64> = h.iter().map(|c| c.norm2()).collect();
    let mut eq = equalise(p, &y, start, n_sym, &h, &hw);
    // 6b. Decision-directed refinement: every data carrier of every data symbol adds an
    //     observation (its nearest constellation point from the previous pass), so the
    //     estimate averages over the whole frame. Repeated, each pass re-decides on the
    //     refined channel.
    let table = &p.cons[mod_idx(m)];
    for _ in 0..DD_PASSES {
        let mut num2 = num.clone();
        let mut den2 = den.clone();
        for (k, &zk) in eq.zs.iter().enumerate() {
            let i = p.data[k % nd];
            let dec = table
                .iter()
                .map(|&(_, pt)| pt)
                .min_by(|a, b| (zk - *a).norm2().total_cmp(&(zk - *b).norm2()))
                .unwrap_or(C::ZERO);
            num2[i] = num2[i] + (zk * h[i]) * dec.conj();
            den2[i] += dec.norm2();
        }
        let raw2: Vec<C> = num2.iter().zip(&den2).map(|(&n, &d)| n * (1.0 / d)).collect();
        h = smooth_channel(p, &raw2, &den2);
        hw = h.iter().map(|c| c.norm2()).collect();
        eq = equalise(p, &y, start, n_sym, &h, &hw);
    }
    let zs = eq.zs;
    let sigma2 = (eq.resid / eq.cnt as f64).max(1e-12);

    // 7. Max-log LLRs, scaled by channel gain over noise: s2_k = sigma2 / |H_k|^2.
    let mut llr: Vec<f32> = Vec::with_capacity(n_sym * per_sym);
    for (k, &zk) in zs.iter().enumerate() {
        let i = p.data[k % nd];
        let s2 = sigma2 / hw[i].max(1e-12);
        let mut d0 = [f64::INFINITY; MAX_BPS];
        let mut d1 = [f64::INFINITY; MAX_BPS];
        for &(v, pt) in table.iter() {
            let dist = (zk - pt).norm2();
            for j in 0..bps {
                let t = if ((v >> (bps - 1 - j)) & 1) == 0 {
                    &mut d0
                } else {
                    &mut d1
                };
                if dist < t[j] {
                    t[j] = dist;
                }
            }
        }
        for j in 0..bps {
            llr.push((LLR_SCALE * (d1[j] - d0[j]) / s2) as f32);
        }
    }
    // Undo the slot order: slot t holds coded bit order[t].
    let order = slot_order(llr.len());
    let mut out = vec![0f32; n_bits];
    for (t, &l) in llr.iter().enumerate() {
        if order[t] < n_bits {
            out[order[t]] = l;
        }
    }
    Some(out)
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
        eprintln!(
            "QPSK at 50 ppm sample clock offset: {errs} bit errors / {}",
            bits.len()
        );
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

    /// Two-path channel: the direct path plus one echo `delay` samples later at `gain`.
    fn two_path(x: &[f32], delay: usize, gain: f32) -> Vec<f32> {
        let mut y = x.to_vec();
        y.resize(x.len() + delay, 0.0);
        for (i, &v) in x.iter().enumerate() {
            y[i + delay] += gain * v;
        }
        y
    }

    /// AWGN at `snr_db` against the mean power of `x`, as `kahf_sim::awgn` does.
    fn awgn_db(x: &[f32], snr_db: f64, rng: &mut Rng) -> Vec<f32> {
        let p = x.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / x.len() as f64;
        let sigma = (p / 10f64.powf(snr_db / 10.0)).sqrt();
        x.iter()
            .map(|&v| (v as f64 + sigma * rng.gauss()) as f32)
            .collect()
    }

    /// Frame errors over `frames` LDPC-coded QPSK frames through the given channel.
    /// The chain matches `kahf_sim::run_point`: payload, LDPC, PHY, channel, PHY, LDPC, payload.
    fn frame_errors(frames: usize, snr_db: f64, seed: u64, chan: impl Fn(&[f32]) -> Vec<f32>) -> usize {
        (0..frames)
            .filter(|&i| {
                let mut rng = Rng(seed.wrapping_add(i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
                let payload: Vec<u8> = (0..36).map(|_| (rng.next_u64() >> 56) as u8).collect();
                let info = crate::frame::build_payload(&payload);
                let coded = crate::ldpc::encode(&info);
                let tx = modulate_frame(&coded, Modulation::Qpsk);
                let rx = awgn_db(&chan(&tx), snr_db, &mut rng);
                let decoded = demodulate_frame(&rx, Modulation::Qpsk, crate::ldpc::N)
                    .and_then(|llr| crate::ldpc::decode(&llr, 50))
                    .and_then(|bits| crate::frame::parse_payload(&bits));
                decoded.as_deref() != Some(payload.as_slice())
            })
            .count()
    }

    /// Zero-pad a frame by 100 samples each side, so the receiver sees silence before and after.
    fn padded(x: &[f32]) -> Vec<f32> {
        let mut p = vec![0f32; 100];
        p.extend_from_slice(x);
        p.extend(vec![0f32; 100]);
        p
    }

    /// Two-path channel, 2 ms echo (16 samples) at -6 dB, 100 frames at 2 dB.
    /// Frame error rate must stay at 1 percent or less.
    #[test]
    fn phy_two_path_echo_fer_100_frames() {
        let errors = frame_errors(100, 2.0, 4242, |x| two_path(x, 16, 0.5012));
        eprintln!("two-path 2 ms -6 dB, 2 dB SNR: {errors}/100 frame errors");
        assert!(
            errors <= 1,
            "{errors}/100 frames in error on the two-path channel"
        );
    }

    /// Carrier frequency offset of 5 Hz either way, 100 frames at 1 dB.
    /// Frame error rate must stay at 1 percent or less.
    #[test]
    fn phy_cfo_5hz_fer_100_frames() {
        for hz in [5.0, -5.0] {
            let errors = frame_errors(100, 1.0, 777, |x| cfo(&padded(x), hz));
            eprintln!("CFO {hz} Hz, 1 dB SNR: {errors}/100 frame errors");
            assert!(errors <= 1, "{errors}/100 frames in error at CFO {hz} Hz");
        }
    }

    /// 25 Hz offset with a start delay and noise, all modulations: recovered without errors.
    #[test]
    fn phy_cfo_25hz_delay_noise_all_modulations() {
        let mut rng = Rng(25);
        for m in ALL {
            for hz in [25.0, -25.0] {
                let bits = rng.bits(1500);
                let tx = modulate_frame(&bits, m);
                let mut rx = vec![0f32; 311];
                rx.extend(cfo(&tx, hz));
                rx.extend(vec![0f32; 100]);
                let power = tx.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / tx.len() as f64;
                let sigma = (power / 10f64.powf(3.2)).sqrt(); // 32 dB
                for v in rx.iter_mut() {
                    *v += (sigma * rng.gauss()) as f32;
                }
                let llr = demodulate_frame(&rx, m, bits.len()).expect("preamble not found");
                assert_eq!(hard(&llr), bits, "{m:?} at {hz} Hz");
            }
        }
    }

    /// Offsets beyond the +-31.25 Hz lag-estimate range: found by the alias search.
    #[test]
    fn phy_cfo_beyond_alias_range_qpsk() {
        let mut rng = Rng(45);
        for hz in [45.0, -45.0, 60.0, -60.0] {
            let bits = rng.bits(2000);
            let tx = modulate_frame(&bits, Modulation::Qpsk);
            let mut rx = vec![0f32; 150];
            rx.extend(cfo(&tx, hz));
            rx.extend(vec![0f32; 100]);
            let power = tx.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / tx.len() as f64;
            let sigma = (power / 100.0).sqrt(); // 20 dB
            for v in rx.iter_mut() {
                *v += (sigma * rng.gauss()) as f32;
            }
            let llr = demodulate_frame(&rx, Modulation::Qpsk, bits.len()).expect("preamble not found");
            let errs = hard(&llr).iter().zip(&bits).filter(|(a, b)| a != b).count();
            assert_eq!(errs, 0, "{hz} Hz: {errs} bit errors");
        }
    }
}
