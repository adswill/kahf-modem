//! KAMI-32: real-valued Walsh signalling, the low-SNR mode at the top of the ladder.
//!
//! 5 coded bits select one of the 32 rows of the Walsh-Hadamard matrix
//! (row k, column j is +1 if popcount(k & j) is even, else -1). A row is sent
//! as 32 real chips at 8000 chips/s (the audio rate), so the output is a plain
//! audio sample vector like the OFDM modes.
//!
//! Frame: a 1024-chip LFSR preamble (x^9 + x^5 + 1) for start and frequency
//! search, then 32-chip slots. Slot 0 and every 32nd slot after it is a pilot
//! (all-ones row); the others carry data symbols.
//!
//! Receiver: joint start and frequency search (FFT correlation of the analytic
//! signal with the preamble, +-60 Hz), a fine frequency pass, pilot phase and
//! amplitude interpolation, real correlation with all 32 rows, per-bit max
//! difference scaled by pilot amplitude squared (as in the kami_fade_llrw
//! simulator). LLR sign convention is the same as `phy`: positive means bit 0.
//!
//! The coded bits are the 648-bit LDPC codeword, zero-padded to a multiple of 5.
//! The frame-layer modulation id for this mode is `ID`.

use std::f64::consts::PI;

/// Frame-layer modulation id (`frame::build_payload_as`) for KAMI-32.
pub const ID: u8 = 3;
/// Chips per symbol of KAMI-32.
pub const L: usize = 32;
/// Bits per symbol of KAMI-32.
pub const BPS: usize = 5;
/// Frame ids of the other Walsh sizes: KAMI-16 (4 bits per 16 chips) and KAMI-8 (3 bits per 8 chips).
pub const ID16: u8 = 4;
pub const ID8: u8 = 5;
/// Frame id of KAMI-8H: 8-chip Walsh words with four amplitude levels, 5 bits per symbol.
pub const ID8H: u8 = 6;
/// Symbol shape of KAMI-8H.
pub const SHAPE_8H: Shape = Shape { wb: 3, lb: 2 };

/// Frame-layer id for a Walsh size of `bps` bits per symbol (3, 4 or 5).
pub fn id_for(bps: usize) -> u8 {
    match bps {
        3 => ID8,
        4 => ID16,
        _ => ID,
    }
}
/// Preamble length in chips.
pub const PRE: usize = 1024;
/// One pilot every this many slots (the first slot is a pilot).
pub const PILOT_EVERY: usize = 32;
/// Frequency search range, Hz either side of nominal.
pub const MAX_OFFSET_HZ: f64 = 60.0;
const FS: f64 = 8000.0;
const PEAK: f64 = 0.95;

type C = (f64, f64);

fn cmul(a: C, b: C) -> C {
    (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0)
}

fn fft(x: &mut [C], inverse: bool) {
    let n = x.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            x.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = 2.0 * PI / len as f64 * if inverse { 1.0 } else { -1.0 };
        let tw: Vec<C> = (0..len / 2).map(|k| ((ang * k as f64).cos(), (ang * k as f64).sin())).collect();
        for i in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let u = x[i + k];
                let v = cmul(x[i + k + len / 2], tw[k]);
                x[i + k] = (u.0 + v.0, u.1 + v.1);
                x[i + k + len / 2] = (u.0 - v.0, u.1 - v.1);
            }
        }
        len <<= 1;
    }
    if inverse {
        for v in x.iter_mut() {
            v.0 /= n as f64;
            v.1 /= n as f64;
        }
    }
}

/// Analytic signal x + j*Hilbert(x) by FFT over the whole buffer.
fn analytic(x: &[f64]) -> Vec<C> {
    let n = x.len().next_power_of_two().max(2);
    let mut buf: Vec<C> = (0..n).map(|i| (x.get(i).copied().unwrap_or(0.0), 0.0)).collect();
    fft(&mut buf, false);
    for c in buf[1..n / 2].iter_mut() {
        c.0 *= 2.0;
        c.1 *= 2.0;
    }
    for c in buf[n / 2 + 1..].iter_mut() {
        *c = (0.0, 0.0);
    }
    fft(&mut buf, true);
    buf.truncate(x.len());
    buf
}

/// Walsh row `k`: 32 chips of +-1.
pub fn walsh(k: usize) -> [f64; L] {
    let mut r = [0.0; L];
    r.copy_from_slice(&walsh_bps(k, BPS));
    r
}

/// Walsh row `k` of the size with `bps` bits per symbol: `1 << bps` chips of +-1.
pub fn walsh_bps(k: usize, bps: usize) -> Vec<f64> {
    let mut r = vec![0.0; 1 << bps];
    for (j, v) in r.iter_mut().enumerate() {
        *v = if (k & j).count_ones().is_multiple_of(2) { 1.0 } else { -1.0 };
    }
    r
}

/// The 1024-chip +-1 preamble from the 9-stage LFSR x^9 + x^5 + 1.
pub fn preamble() -> Vec<f64> {
    let mut st: u16 = 0x1FF;
    (0..PRE)
        .map(|_| {
            let bit = ((st >> 8) ^ (st >> 4)) & 1;
            st = ((st << 1) | bit) & 0x1FF;
            if st & 1 == 1 { 1.0 } else { -1.0 }
        })
        .collect()
}

/// Symbol shape: `wb` bits pick one of `1 << wb` Walsh rows (so `1 << wb` chips per symbol);
/// `lb` more bits pick one of `1 << lb` amplitude levels (Gray coded, sign-preserving).
/// `lb == 0` is plain KAMI: the symbol has no amplitude information.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shape {
    pub wb: usize,
    pub lb: usize,
}

impl Shape {
    /// Plain Walsh signalling with `wb` bits per symbol.
    pub const fn walsh(wb: usize) -> Shape {
        Shape { wb, lb: 0 }
    }
    /// Coded bits carried by one symbol.
    pub const fn bits(self) -> usize {
        self.wb + self.lb
    }
    /// Chips per symbol.
    pub const fn chips(self) -> usize {
        1 << self.wb
    }
}

/// Amplitude of level index `li` of `1 << lb` levels: odd integers scaled to mean square 1.
fn level_amp(li: usize, lb: usize) -> f64 {
    let nl = 1usize << lb;
    if nl == 1 {
        return 1.0;
    }
    let ms: f64 = (0..nl).map(|i| (2.0 * i as f64 - (nl as f64 - 1.0)).powi(2)).sum::<f64>() / nl as f64;
    (2.0 * li as f64 - (nl as f64 - 1.0)) / ms.sqrt()
}

/// Amplitude for the Gray-coded level bits `g`.
fn gray_amp(g: usize, lb: usize) -> f64 {
    let mut i = g;
    let mut sh = g >> 1;
    while sh != 0 {
        i ^= sh;
        sh >>= 1;
    }
    level_amp(i, lb)
}

fn n_syms(n_bits: usize, sh: Shape) -> usize {
    n_bits.div_ceil(sh.bits())
}

/// Total slots (pilots plus data) for `nsym` data symbols.
fn n_slots(nsym: usize) -> usize {
    let (mut slot, mut count) = (0, 0);
    while count < nsym {
        if slot % PILOT_EVERY != 0 {
            count += 1;
        }
        slot += 1;
    }
    slot
}

/// Number of audio samples a frame of `n_bits` coded bits occupies.
pub fn frame_len(n_bits: usize) -> usize {
    frame_len_shape(n_bits, Shape::walsh(BPS))
}

/// `frame_len` for a Walsh size of `bps` bits per symbol.
pub fn frame_len_bps(n_bits: usize, bps: usize) -> usize {
    frame_len_shape(n_bits, Shape::walsh(bps))
}

/// `frame_len` for any symbol shape.
pub fn frame_len_shape(n_bits: usize, sh: Shape) -> usize {
    PRE + n_slots(n_syms(n_bits, sh)) * sh.chips()
}

/// Map coded bits (0/1) to audio samples: preamble, then pilots and Walsh symbols.
pub fn modulate_frame(coded: &[u8]) -> Vec<f32> {
    modulate_frame_bps(coded, BPS)
}

/// `modulate_frame` for a Walsh size of `bps` bits per symbol.
pub fn modulate_frame_bps(coded: &[u8], bps: usize) -> Vec<f32> {
    modulate_frame_shape(coded, Shape::walsh(bps))
}

/// `modulate_frame` for any symbol shape. Pilots stay at amplitude 1 (the mean level); the
/// whole frame is scaled so the largest level stays at PEAK.
pub fn modulate_frame_shape(coded: &[u8], sh: Shape) -> Vec<f32> {
    let nsym = n_syms(coded.len(), sh);
    let mut chips = preamble();
    let ones = vec![1.0f64; sh.chips()];
    let top = level_amp((1 << sh.lb) - 1, sh.lb).max(1.0);
    let bit = |i: usize| coded.get(i).map_or(0, |b| (b & 1) as usize);
    let (mut sym_i, mut slot) = (0usize, 0usize);
    while sym_i < nsym {
        if slot % PILOT_EVERY == 0 {
            chips.extend_from_slice(&ones);
        } else {
            let base = sym_i * sh.bits();
            let mut idx = 0usize;
            for j in 0..sh.wb {
                idx = (idx << 1) | bit(base + j);
            }
            let mut g = 0usize;
            for j in 0..sh.lb {
                g = (g << 1) | bit(base + sh.wb + j);
            }
            let amp = gray_amp(g, sh.lb);
            chips.extend(walsh_bps(idx, sh.wb).iter().map(|&w| w * amp));
            sym_i += 1;
        }
        slot += 1;
    }
    chips.iter().map(|&v| (v * PEAK / top) as f32).collect()
}

/// Find the frame in `samples`, return `n_bits` coded-bit LLRs (positive = 0), or None.
pub fn demodulate_frame(samples: &[f32], n_bits: usize) -> Option<Vec<f32>> {
    locate_and_demodulate(samples, n_bits).map(|(_, llr)| llr)
}

/// `demodulate_frame` for a Walsh size of `bps` bits per symbol.
pub fn demodulate_frame_bps(samples: &[f32], n_bits: usize, bps: usize) -> Option<Vec<f32>> {
    locate_and_demodulate_bps(samples, n_bits, bps).map(|(_, llr)| llr)
}

/// As `demodulate_frame`, also returning the sample index where the frame starts
/// (its preamble). Only frames lying wholly inside `samples` are considered.
pub fn locate_and_demodulate(samples: &[f32], n_bits: usize) -> Option<(usize, Vec<f32>)> {
    locate_and_demodulate_bps(samples, n_bits, BPS)
}

/// `locate_and_demodulate` for a Walsh size of `bps` bits per symbol.
pub fn locate_and_demodulate_bps(samples: &[f32], n_bits: usize, bps: usize) -> Option<(usize, Vec<f32>)> {
    locate_and_demodulate_shape(samples, n_bits, Shape::walsh(bps))
}

/// `locate_and_demodulate` for any symbol shape.
pub fn locate_and_demodulate_shape(samples: &[f32], n_bits: usize, sh: Shape) -> Option<(usize, Vec<f32>)> {
    let (start, mut all) = locate_and_demodulate_multi(samples, n_bits, &[sh])?;
    Some((start, all.pop()?.1))
}

/// `demodulate_frame` for any symbol shape.
pub fn demodulate_frame_shape(samples: &[f32], n_bits: usize, sh: Shape) -> Option<Vec<f32>> {
    locate_and_demodulate_shape(samples, n_bits, sh).map(|(_, llr)| llr)
}

/// LLRs of one frame under one symbol shape.
pub type SizeLlrs = (Shape, Vec<f32>);

/// Find one frame whose size is any of `sizes` (all share the preamble) and demodulate it
/// under each size, returning the start and the LLRs per size, in the order of `sizes`.
/// Sizes whose frame does not fit inside `samples` are left out.
pub fn locate_and_demodulate_multi(
    samples: &[f32],
    n_bits: usize,
    sizes: &[Shape],
) -> Option<(usize, Vec<SizeLlrs>)> {
    let total = sizes.iter().map(|&b| frame_len_shape(n_bits, b)).min()?;
    if n_bits == 0 || samples.len() < total {
        return None;
    }
    let x: Vec<f64> = samples.iter().map(|&v| if v.is_finite() { v as f64 } else { 0.0 }).collect();
    if x.iter().all(|&v| v == 0.0) {
        return None;
    }
    let p = preamble();
    let a_all = analytic(&x);

    // Joint start and frequency search: for each frequency hypothesis, FFT-correlate
    // the de-rotated analytic signal with the preamble and keep the best peak.
    let last = x.len() - total;
    let nfft = (x.len() + PRE).next_power_of_two();
    let mut pf: Vec<C> = vec![(0.0, 0.0); nfft];
    for i in 0..PRE {
        pf[i] = (p[i], 0.0);
    }
    fft(&mut pf, false);
    let mut best = (f64::MIN, 0usize, 0.0f64);
    // One frequency hypothesis: de-rotate, FFT-correlate with the preamble, keep the best peak.
    let try_hz = |hz: f64, best: &mut (f64, usize, f64)| {
        let mut zf = vec![(0.0, 0.0); nfft];
        for (n, z) in zf.iter_mut().enumerate().take(x.len()) {
            let th = -2.0 * PI * hz * n as f64 / FS;
            *z = cmul(a_all[n], (th.cos(), th.sin()));
        }
        fft(&mut zf, false);
        let mut rf: Vec<C> = zf.iter().zip(&pf).map(|(&z, &pv)| cmul(z, (pv.0, -pv.1))).collect();
        fft(&mut rf, true);
        for (st, r) in rf.iter().enumerate().take(last + 1) {
            let m = r.0 * r.0 + r.1 * r.1;
            if m > best.0 {
                *best = (m, st, hz);
            }
        }
    };
    // Coarse pass every 6 Hz (the preamble is 0.128 s long, so it tolerates a few Hz), then
    // a fine pass of +-4 Hz around the winner.
    let mut hz = -MAX_OFFSET_HZ;
    while hz <= MAX_OFFSET_HZ {
        try_hz(hz, &mut best);
        hz += 6.0;
    }
    let centre = best.2;
    for d in [-4.0, -2.0, 2.0, 4.0] {
        try_hz(centre + d, &mut best);
    }
    let start = best.1;
    let mut out = Vec::new();
    for &sh in sizes {
        let len = frame_len_shape(n_bits, sh);
        if start + len <= x.len() {
            if let Some(llr) = demod_at(&x[start..start + len], &p, best.2, n_bits, sh) {
                out.push((sh, llr));
            }
        }
    }
    Some((start, out))
}

/// Demodulate the frame that starts at `rx[0]`, given the coarse frequency estimate `coarse`.
fn demod_at(rx: &[f64], p: &[f64], coarse: f64, n_bits: usize, sh: Shape) -> Option<Vec<f32>> {
    let l = sh.chips();
    let a = analytic(rx);

    // Fine frequency pass around the coarse estimate.
    let mut fbest = (f64::MIN, coarse);
    let mut df = coarse - 2.0;
    while df <= coarse + 2.0 {
        let mut acc = (0.0, 0.0);
        for i in 0..PRE {
            let th = -2.0 * PI * df * i as f64 / FS;
            let v = cmul(a[i], (p[i] * th.cos(), p[i] * th.sin()));
            acc.0 += v.0;
            acc.1 += v.1;
        }
        let mag = acc.0 * acc.0 + acc.1 * acc.1;
        if mag > fbest.0 {
            fbest = (mag, df);
        }
        df += 0.25;
    }
    let est = fbest.1;
    let block = |slot: usize| -> Vec<C> {
        (0..l)
            .map(|j| {
                let n = PRE + slot * l + j;
                let th = -2.0 * PI * est * n as f64 / FS;
                cmul(a[n], (th.cos(), th.sin()))
            })
            .collect()
    };

    // Pilots: phase and amplitude per pilot slot.
    let nsym = n_syms(n_bits, sh);
    let slots = n_slots(nsym);
    let mut pil_slots = Vec::new();
    let mut pil_phase: Vec<f64> = Vec::new();
    let mut pil_amp: Vec<f64> = Vec::new();
    for s in (0..slots).step_by(PILOT_EVERY) {
        let (mut re, mut im) = (0.0, 0.0);
        for v in block(s) {
            re += v.0;
            im += v.1;
        }
        pil_slots.push(s as f64);
        pil_phase.push(im.atan2(re));
        pil_amp.push((re * re + im * im).sqrt() / l as f64);
    }
    for i in 1..pil_phase.len() {
        while pil_phase[i] - pil_phase[i - 1] > PI {
            pil_phase[i] -= 2.0 * PI;
        }
        while pil_phase[i] - pil_phase[i - 1] < -PI {
            pil_phase[i] += 2.0 * PI;
        }
    }
    let interp = |v: &[f64], s: f64| -> f64 {
        let mut k = 0;
        while k + 1 < pil_slots.len() && pil_slots[k + 1] < s {
            k += 1;
        }
        if k + 1 >= pil_slots.len() {
            return v[k];
        }
        let t = (s - pil_slots[k]) / (pil_slots[k + 1] - pil_slots[k]);
        v[k] + t * (v[k + 1] - v[k])
    };

    let rows: Vec<Vec<f64>> = (0..l).map(|k| walsh_bps(k, sh.wb)).collect();
    // Every symbol value (word bits then level bits) with its Walsh row and amplitude.
    let nvals = 1usize << sh.bits();
    let syms: Vec<(usize, f64)> =
        (0..nvals).map(|v| (v >> sh.lb, gray_amp(v & ((1 << sh.lb) - 1), sh.lb))).collect();
    let mut llr = Vec::with_capacity(nsym * sh.bits());
    for s in (0..slots).filter(|s| s % PILOT_EVERY != 0) {
        let ph = interp(&pil_phase, s as f64);
        let amp = interp(&pil_amp, s as f64).max(1e-6);
        let (c, sn) = ((-ph).cos(), (-ph).sin());
        let real: Vec<f64> = block(s).iter().map(|&v| (v.0 * c - v.1 * sn) / amp).collect();
        let corr: Vec<f64> =
            rows.iter().map(|r| real.iter().zip(r).map(|(x, y)| x * y).sum()).collect();
        // Max-log metric of every symbol value; plain Walsh reduces to the row correlation.
        let metric: Vec<f64> = syms.iter().map(|&(w, a)| 2.0 * a * corr[w] - a * a * l as f64).collect();
        for bit in (0..sh.bits()).rev() {
            let (mut b0, mut b1) = (f64::MIN, f64::MIN);
            for (v, &m) in metric.iter().enumerate() {
                if (v >> bit) & 1 == 0 {
                    b0 = b0.max(m)
                } else {
                    b1 = b1.max(m)
                }
            }
            llr.push(((b0 - b1) * amp * amp) as f32);
        }
    }
    llr.truncate(n_bits);
    Some(llr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{frame, ldpc};

    struct Rng(u64);
    impl Rng {
        fn uniform(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        }
        fn gauss(&mut self) -> f64 {
            let (a, b) = (self.uniform(), self.uniform());
            (-2.0 * a.ln()).sqrt() * (2.0 * PI * b).cos()
        }
    }

    fn data(f: usize) -> Vec<u8> {
        (0..36).map(|i| ((f * 37 + i * 11) % 251) as u8).collect()
    }

    fn encode(d: &[u8]) -> Vec<u8> {
        ldpc::encode(&frame::build_payload_as(d, ID))
    }

    fn decode(llr: &[f32]) -> Option<Vec<u8>> {
        let bits = ldpc::decode(llr, 50)?;
        let (d, id) = frame::parse_payload_id(&bits)?;
        (id == ID).then_some(d)
    }

    fn shift(x: &[f32], hz: f64) -> Vec<f32> {
        let x64: Vec<f64> = x.iter().map(|&v| v as f64).collect();
        analytic(&x64)
            .iter()
            .enumerate()
            .map(|(n, a)| {
                let th = 2.0 * PI * hz * n as f64 / FS;
                (a.0 * th.cos() - a.1 * th.sin()) as f32
            })
            .collect()
    }

    fn pad(tx: &[f32], lead: usize) -> Vec<f32> {
        let mut v = vec![0.0f32; lead];
        v.extend_from_slice(tx);
        v.extend(std::iter::repeat_n(0.0, 200));
        v
    }

    #[test]
    fn walsh_rows_orthogonal() {
        for a in 0..L {
            for b in 0..L {
                let d: f64 = walsh(a).iter().zip(&walsh(b)).map(|(x, y)| x * y).sum();
                assert_eq!(d, if a == b { L as f64 } else { 0.0 });
            }
        }
    }

    #[test]
    fn noiseless_roundtrip_exact() {
        for f in 0..3 {
            let d = data(f);
            let coded = encode(&d);
            let tx = modulate_frame(&coded);
            assert_eq!(tx.len(), frame_len(coded.len()));
            let llr = demodulate_frame(&pad(&tx, f * 131 + 37), coded.len()).expect("frame not found");
            assert_eq!(llr.len(), coded.len());
            let hard: Vec<u8> = llr.iter().map(|&l| u8::from(l < 0.0)).collect();
            assert_eq!(hard, coded, "hard decisions differ without noise");
            assert_eq!(decode(&llr), Some(d));
        }
    }

    #[test]
    fn roundtrip_with_noise_0db() {
        let mut rng = Rng(12345);
        for f in 0..5 {
            let d = data(f);
            let coded = encode(&d);
            let mut rx = pad(&modulate_frame(&coded), 100 + f * 17);
            for v in rx.iter_mut() {
                *v += (PEAK * rng.gauss()) as f32; // 0 dB per real sample
            }
            let llr = demodulate_frame(&rx, coded.len()).expect("frame not found");
            assert_eq!(decode(&llr), Some(d), "frame {f} lost at 0 dB");
        }
    }

    #[test]
    fn roundtrip_frequency_offset_and_noise() {
        let mut rng = Rng(777);
        for (f, &hz) in [-40.0f64, -13.3, 0.0, 7.7, 25.0, 55.0].iter().enumerate() {
            let d = data(f);
            let coded = encode(&d);
            let mut rx = shift(&pad(&modulate_frame(&coded), 50 + f * 23), hz);
            for v in rx.iter_mut() {
                *v += (0.5 * PEAK * rng.gauss()) as f32; // about +6 dB
            }
            let llr = demodulate_frame(&rx, coded.len()).expect("frame not found");
            assert_eq!(decode(&llr), Some(d), "frame lost at offset {hz} Hz");
        }
    }

    #[test]
    fn roundtrip_low_snr_with_offset() {
        // -3 dB, 25 Hz offset, slow amplitude swing: inside the simulator's zero-loss region.
        let mut rng = Rng(99);
        let mut ok = 0;
        for f in 0..4 {
            let d = data(f);
            let coded = encode(&d);
            let mut rx = shift(&pad(&modulate_frame(&coded), 300 + f * 31), 25.0);
            let sigma = PEAK * 10f64.powf(3.0 / 20.0);
            for (n, v) in rx.iter_mut().enumerate() {
                let g = 1.0 + 0.1 * (2.0 * PI * n as f64 / FS + 0.7).cos();
                *v = (*v as f64 * g + sigma * rng.gauss()) as f32;
            }
            if let Some(llr) = demodulate_frame(&rx, coded.len()) {
                if decode(&llr) == Some(d) {
                    ok += 1;
                }
            }
        }
        assert_eq!(ok, 4, "lost frames at -3 dB, 25 Hz");
    }

    #[test]
    fn smaller_walsh_sizes_roundtrip_and_are_found_together() {
        let mut rng = Rng(4242);
        let shapes = [Shape::walsh(5), Shape::walsh(4), Shape::walsh(3), SHAPE_8H];
        let ids = [ID, ID16, ID8, ID8H];
        for (k, &sh) in shapes.iter().enumerate() {
            for f in 0..3 {
                let d = data(f + k);
                let coded = ldpc::encode(&frame::build_payload_as(&d, ids[k]));
                let tx = modulate_frame_shape(&coded, sh);
                assert_eq!(tx.len(), frame_len_shape(coded.len(), sh));
                assert!(tx.iter().all(|v| v.abs() <= PEAK as f32 + 1e-6), "clipping");
                let mut rx = shift(&pad(&tx, 90 + f * 41), 18.0);
                for v in rx.iter_mut() {
                    *v += (0.4 * PEAK * rng.gauss()) as f32; // about +8 dB
                }
                let (_, tries) = locate_and_demodulate_multi(&rx, coded.len(), &shapes).expect("frame not found");
                let (_, llr) = tries.iter().find(|(b, _)| *b == sh).expect("shape demodulated");
                let bits = ldpc::decode(llr, 50).expect("decodes");
                assert_eq!(frame::parse_payload_id(&bits), Some((d, ids[k])), "shape {sh:?} frame {f}");
            }
        }
        assert!(frame_len_bps(648, 3) < frame_len_bps(648, 4) && frame_len_bps(648, 4) < frame_len(648));
        assert!(frame_len_shape(648, SHAPE_8H) < frame_len_bps(648, 3));
    }
}
