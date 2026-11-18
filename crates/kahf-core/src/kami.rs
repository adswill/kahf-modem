//! KAMI: Walsh signalling in the same 2.4 kHz channel as the OFDM modes.
//!
//! Every mode sends complex chips at 2000 chips/s, shaped with a root-raised-cosine pulse
//! (roll-off 0.2) and carried on a 1500 Hz audio carrier, so the signal occupies 300 to 2700 Hz
//! whatever the mode. A mode is a symbol shape: `wb` bits pick one of `1 << wb` Walsh rows
//! (`1 << wb` chips per symbol), sent independently on the in-phase and quadrature branches, so a
//! symbol slot carries `2 * wb` coded bits (`lb` more per branch for amplitude levels).
//! KAMI-2 to KAMI-256 are `wb` 1 to 8: the bigger the number, the lower the rate and the lower
//! the SNR it needs.
//!
//! Frame: a 512-chip LFSR preamble (x^9 + x^5 + 1, in-phase only), then slots of `1 << wb` chips.
//! Slot 0 and every `pilot_every`-th slot after it is a pilot (all-ones on the in-phase branch).
//!
//! Receiver: joint start and frequency search (FFT correlation of the baseband signal with the
//! preamble waveform, +-60 Hz), a fine frequency pass on the matched-filter chips, pilot phase and
//! amplitude interpolation, correlation with every Walsh row on each branch, per-bit max-log
//! decisions scaled by the pilot amplitude squared. LLR sign convention is the same as `phy`:
//! positive means bit 0. The coded bits are the 648-bit LDPC codeword, zero-padded to whole slots.

use std::f64::consts::PI;
use std::sync::OnceLock;

/// Frame-layer modulation ids, one per mode. 0 to 2 are the OFDM modes.
pub const ID: u8 = 3; // KAMI-32
pub const ID16: u8 = 4;
pub const ID8: u8 = 5;
pub const ID8H: u8 = 6;
pub const ID64: u8 = 7;
pub const ID128: u8 = 8;
pub const ID4: u8 = 9;
pub const ID2: u8 = 11;
pub const ID256: u8 = 12;
pub const ID1A: u8 = 13;
pub const ID1B: u8 = 14;
pub const ID1C: u8 = 15;
pub const ID1D: u8 = 16;
/// Chips per symbol of KAMI-32.
pub const L: usize = 32;
/// Bits per symbol (per branch) of KAMI-32.
pub const BPS: usize = 5;
/// Symbol shape of KAMI-8H: 8-chip Walsh words with four amplitude levels per branch.
pub const SHAPE_8H: Shape = Shape { wb: 3, lb: 2 };

/// Frame-layer id for a plain Walsh size of `wb` bits per branch (1 to 8).
pub fn id_for(wb: usize) -> u8 {
    match wb {
        1 => ID2,
        2 => ID4,
        3 => ID8,
        4 => ID16,
        6 => ID64,
        7 => ID128,
        8 => ID256,
        _ => ID,
    }
}

/// Shapes with `wb` at or above this carry training chips in their pilot slots. 9 means none:
/// tracking works in the simulator (3 here) but broke the daemon end-to-end tests, so it is off
/// until that is understood.
const TRACK_MIN_WB: usize = 9;
/// Training chips in one pilot group, at least.
const TRAIN_MIN_CHIPS: usize = 64;

/// Chips per second.
pub const CHIP_RATE: f64 = 2000.0;
/// Audio samples per chip.
const SPS: usize = 4;
/// Carrier centre in the audio band, Hz.
pub const CARRIER_HZ: f64 = 1500.0;
const ROLLOFF: f64 = 0.2;
/// Pulse half-length in samples (6 chips) and taps.
const HALF: usize = 6 * SPS;
const NT: usize = 2 * HALF + 1;
/// Preamble length in chips.
pub const PRE_CHIPS: usize = 512;
/// Frequency search range, Hz either side of nominal.
pub const MAX_OFFSET_HZ: f64 = 60.0;
const FS: f64 = 8000.0;
const PEAK: f64 = 0.95;
/// The pilot noise estimate reads this many dB above the SNR the simulator and the ladder use
/// (white noise over the whole 4 kHz audio band, per real sample): the receiver only sees the
/// noise inside its 2.4 kHz filter. Measured with `kami_snr_est`, within 0.5 dB from -12 dB up.
const SNR_OFFSET_DB: f64 = 4.0;
/// Largest soft-decision magnitude handed to the LDPC decoder.
const LLR_MAX: f64 = 40.0;

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


/// Root-raised-cosine pulse at time `x` samples from its centre, before normalisation.
fn rrc_raw(x: f64) -> f64 {
    let b = ROLLOFF;
    let t = x / SPS as f64;
    if t.abs() < 1e-9 {
        1.0 - b + 4.0 * b / PI
    } else if (t.abs() - 1.0 / (4.0 * b)).abs() < 1e-9 {
        b / 2f64.sqrt() * ((1.0 + 2.0 / PI) * (PI / (4.0 * b)).sin() + (1.0 - 2.0 / PI) * (PI / (4.0 * b)).cos())
    } else {
        ((PI * t * (1.0 - b)).sin() + 4.0 * b * t * (PI * t * (1.0 + b)).cos())
            / (PI * t * (1.0 - (4.0 * b * t).powi(2)))
    }
}

/// Normalisation that gives the `NT`-tap pulse unit energy.
fn rrc_norm() -> f64 {
    static N: OnceLock<f64> = OnceLock::new();
    *N.get_or_init(|| (0..NT).map(|n| rrc_raw(n as f64 - HALF as f64).powi(2)).sum::<f64>().sqrt())
}

/// Root-raised-cosine pulse, `NT` taps, unit energy.
fn rrc() -> &'static [f64] {
    static H: OnceLock<Vec<f64>> = OnceLock::new();
    H.get_or_init(|| (0..NT).map(|n| rrc_raw(n as f64 - HALF as f64) / rrc_norm()).collect())
}

/// Symbol shape: `wb` bits pick one of `1 << wb` Walsh rows (so `1 << wb` chips per symbol);
/// `lb` more bits pick one of `1 << lb` amplitude levels (Gray coded, sign-preserving). The same
/// shape is sent on both the in-phase and the quadrature branch. `lb == 0` is plain KAMI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shape {
    pub wb: usize,
    pub lb: usize,
}

impl Shape {
    /// Plain Walsh signalling with `wb` bits per branch.
    pub const fn walsh(wb: usize) -> Shape {
        Shape { wb, lb: 0 }
    }
    /// Coded bits carried by one branch of one symbol.
    pub const fn bits(self) -> usize {
        self.wb + self.lb
    }
    /// Coded bits carried by one symbol slot (both branches).
    pub const fn slot_bits(self) -> usize {
        2 * self.bits()
    }
    /// Chips per symbol.
    pub const fn chips(self) -> usize {
        1 << self.wb
    }
    /// Preamble chips: the full 512 for Walsh modes, shorter for the single-chip amplitude
    /// modes, which only run at SNRs where a short preamble finds the frame. Every preamble is a
    /// prefix of the same sequence, so one detector finds them all.
    pub const fn pre_chips(self) -> usize {
        if self.wb > 0 {
            PRE_CHIPS
        } else if self.lb <= 1 {
            256
        } else {
            128
        }
    }
    /// Chips in one slot: a symbol, or several symbols for very short ones, so that a pilot
    /// slot always averages at least 8 chips.
    pub const fn slot_chips(self) -> usize {
        if self.chips() < 8 {
            8
        } else {
            self.chips()
        }
    }
    /// Symbols in one data slot.
    pub const fn syms_per_slot(self) -> usize {
        self.slot_chips() / self.chips()
    }
    /// Slots between pilots: pilots stay about 512 chips (0.26 s) apart, 3 to 64 slots.
    pub const fn pilot_every(self) -> usize {
        let p = 512 / self.slot_chips();
        if p < 3 {
            3
        } else if p > 64 {
            64
        } else {
            p
        }
    }
    /// Slots in one pilot group. Slow modes (long bursts, which outlast an HF channel) carry
    /// known training chips in their pilot slots, at least `TRAIN_MIN_CHIPS` of them, so the
    /// receiver can follow the channel through the burst. Fast modes keep the single all-ones
    /// pilot slot.
    pub const fn pilot_group(self) -> usize {
        if self.wb >= TRACK_MIN_WB {
            TRAIN_MIN_CHIPS.div_ceil(self.slot_chips())
        } else {
            1
        }
    }
    /// True when the pilot slots carry training chips.
    pub const fn tracked(self) -> bool {
        self.wb >= TRACK_MIN_WB
    }
    /// Raw coded bits per second while sending data slots.
    pub fn raw_bps(self) -> f64 {
        self.slot_bits() as f64 / self.chips() as f64 * CHIP_RATE
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

/// Walsh row `k` of the size with `wb` bits: `1 << wb` chips of +-1.
pub fn walsh_bps(k: usize, wb: usize) -> Vec<f64> {
    (0..1usize << wb).map(|j| if (k & j).count_ones().is_multiple_of(2) { 1.0 } else { -1.0 }).collect()
}

/// The 512-chip +-1 preamble from the 9-stage LFSR x^9 + x^5 + 1.
pub fn preamble() -> Vec<f64> {
    let mut st: u16 = 0x1FF;
    (0..PRE_CHIPS)
        .map(|_| {
            let bit = ((st >> 8) ^ (st >> 4)) & 1;
            st = ((st << 1) | bit) & 0x1FF;
            if st & 1 == 1 { 1.0 } else { -1.0 }
        })
        .collect()
}

fn n_syms(n_bits: usize, sh: Shape) -> usize {
    n_bits.div_ceil(sh.slot_bits())
}

/// Total slots (pilots plus data) for `nsym` data symbols.
fn n_slots(nsym: usize, pe: usize, g: usize) -> usize {
    let (mut slot, mut count) = (0, 0);
    while count < nsym {
        if slot % pe >= g {
            count += 1;
        }
        slot += 1;
    }
    slot
}

/// Data slots needed for `n_bits` coded bits.
fn n_data_slots(n_bits: usize, sh: Shape) -> usize {
    n_syms(n_bits, sh).div_ceil(sh.syms_per_slot())
}

fn n_chips(n_bits: usize, sh: Shape) -> usize {
    sh.pre_chips() + n_slots(n_data_slots(n_bits, sh), sh.pilot_every(), sh.pilot_group()) * sh.slot_chips()
}

/// The chips of one pilot group: all ones for the fast modes, a piece of the preamble's
/// m-sequence (flat autocorrelation, so it measures the channel) for the tracked ones.
fn train_pattern(sh: Shape) -> Vec<f64> {
    let n = sh.pilot_group() * sh.slot_chips();
    if sh.tracked() {
        preamble().into_iter().take(n).collect()
    } else {
        vec![1.0; n]
    }
}

/// Audio samples a frame of `n_bits` coded bits occupies for a plain Walsh size of `wb` bits.
pub fn frame_len_bps(n_bits: usize, wb: usize) -> usize {
    frame_len_shape(n_bits, Shape::walsh(wb))
}

/// Audio samples a frame of `n_bits` coded bits occupies for any symbol shape.
pub fn frame_len_shape(n_bits: usize, sh: Shape) -> usize {
    SPS * (n_chips(n_bits, sh) - 1) + NT
}

/// Shape complex chips into passband audio: RRC pulses at `SPS` samples per chip, on the carrier.
fn shape(ci: &[f64], cq: &[f64]) -> Vec<f64> {
    let h = rrc();
    let len = SPS * (ci.len() - 1) + NT;
    let (mut yi, mut yq) = (vec![0.0; len], vec![0.0; len]);
    for k in 0..ci.len() {
        for (t, &ht) in h.iter().enumerate() {
            yi[SPS * k + t] += ci[k] * ht;
            yq[SPS * k + t] += cq[k] * ht;
        }
    }
    let w = 2.0 * PI * CARRIER_HZ / FS;
    (0..len).map(|n| yi[n] * (w * n as f64).cos() - yq[n] * (w * n as f64).sin()).collect()
}

/// Length of the preamble waveform in audio samples.
pub fn preamble_len() -> usize {
    SPS * (PRE_CHIPS - 1) + NT
}

/// Baseband waveform of the first `n_chips` preamble chips, the template of the start search.
fn preamble_wave_of(n_chips: usize) -> Vec<f64> {
    let ci = preamble();
    let h = rrc();
    let mut y = vec![0.0; SPS * (n_chips - 1) + NT];
    for (k, &c) in ci.iter().take(n_chips).enumerate() {
        for (t, &ht) in h.iter().enumerate() {
            y[SPS * k + t] += c * ht;
        }
    }
    y
}

/// The long template (the full preamble, for every Walsh mode) and the short one (the first 128
/// chips, which the fast single-chip modes use as their whole preamble).
fn preamble_wave(short: bool) -> &'static [f64] {
    static LONG: OnceLock<Vec<f64>> = OnceLock::new();
    static SHORT: OnceLock<Vec<f64>> = OnceLock::new();
    if short {
        SHORT.get_or_init(|| preamble_wave_of(128))
    } else {
        LONG.get_or_init(|| preamble_wave_of(PRE_CHIPS))
    }
}

/// Map coded bits (0/1) to audio samples for a plain Walsh size of `wb` bits per branch.
pub fn modulate_frame_bps(coded: &[u8], wb: usize) -> Vec<f32> {
    modulate_frame_shape(coded, Shape::walsh(wb))
}

/// Coded bits one data slot carries (all its symbols, both branches).
fn data_slot_bits(sh: Shape) -> usize {
    sh.syms_per_slot() * sh.slot_bits()
}

/// Length in coded bits of one codeword of `n_bits` once padded to whole data slots.
fn padded_bits(n_bits: usize, sh: Shape) -> usize {
    n_data_slots(n_bits, sh) * data_slot_bits(sh)
}

/// Audio samples of a burst of `n_cw` codewords of `n_bits` coded bits each, sharing one
/// preamble. `burst_len_shape(n, 1, sh)` is `frame_len_shape(n, sh)`.
pub fn burst_len_shape(n_bits: usize, n_cw: usize, sh: Shape) -> usize {
    frame_len_shape(n_cw * padded_bits(n_bits, sh), sh)
}

/// Map several codewords to audio samples with one preamble: the codewords follow each other
/// in whole data slots, with pilots interleaved as in a single frame. Sharing the preamble is
/// what lets the fast modes reach their full rate.
pub fn modulate_burst_shape(codewords: &[Vec<u8>], sh: Shape) -> Vec<f32> {
    let n_bits = codewords.first().map_or(0, |c| c.len());
    let pb = padded_bits(n_bits, sh);
    let mut all = Vec::with_capacity(codewords.len() * pb);
    for c in codewords {
        all.extend_from_slice(c);
        all.resize(all.len() + pb - c.len(), 0);
    }
    modulate_frame_shape(&all, sh)
}

/// A demodulated burst: one LLR vector per codeword and the SNR estimated from its pilots.
pub struct BurstRx {
    pub llrs: Vec<Vec<f32>>,
    /// Estimated SNR in dB per real audio sample (clamped to -30 to 45).
    pub snr_db: f64,
}

/// Demodulate the burst of `n_cw` codewords that starts at `start` in `samples`. One LLR vector
/// of `n_bits` per codeword, in order, or None when the burst does not fit in `samples`.
pub fn demodulate_burst_at(
    samples: &[f32],
    start: usize,
    hz: f64,
    tau: f64,
    n_bits: usize,
    n_cw: usize,
    sh: Shape,
) -> Option<Vec<Vec<f32>>> {
    demodulate_burst_rx(samples, start, hz, tau, n_bits, n_cw, sh).map(|b| b.llrs)
}

/// As `demodulate_burst_at`, also returning the SNR estimate.
pub fn demodulate_burst_rx(
    samples: &[f32],
    start: usize,
    hz: f64,
    tau: f64,
    n_bits: usize,
    n_cw: usize,
    sh: Shape,
) -> Option<BurstRx> {
    let len = burst_len_shape(n_bits, n_cw, sh);
    if n_cw == 0 || start + len > samples.len() {
        return None;
    }
    let z = baseband(samples)?;
    let pb = padded_bits(n_bits, sh);
    let (llr, snr_db) = demod_at(&z[start..start + len], hz, tau, n_cw * pb, sh)?;
    Some(BurstRx { llrs: (0..n_cw).map(|j| llr[j * pb..j * pb + n_bits].to_vec()).collect(), snr_db })
}

/// Map coded bits (0/1) to audio samples: preamble, then pilots and Walsh symbols, on the
/// carrier and band-limited to 2.4 kHz. Scaled so the largest sample is PEAK.
pub fn modulate_frame_shape(coded: &[u8], sh: Shape) -> Vec<f32> {
    let nd = n_data_slots(coded.len(), sh);
    let pe = sh.pilot_every();
    let sc = sh.slot_chips();
    let mut ci = preamble()[..sh.pre_chips()].to_vec();
    let mut cq = vec![0.0; sh.pre_chips()];
    let bit = |i: usize| coded.get(i).map_or(0, |b| (b & 1) as usize);
    let (mut done, mut slot, mut sym_i) = (0usize, 0usize, 0usize);
    let g = sh.pilot_group();
    let train = train_pattern(sh);
    while done < nd {
        if slot % pe < g {
            let at = (slot % pe) * sc;
            ci.extend((0..sc).map(|t| train[at + t]));
            cq.extend(std::iter::repeat_n(0.0, sc));
        } else {
            for _ in 0..sh.syms_per_slot() {
                for branch in 0..2 {
                    let base = sym_i * sh.slot_bits() + branch * sh.bits();
                    let mut idx = 0usize;
                    for j in 0..sh.wb {
                        idx = (idx << 1) | bit(base + j);
                    }
                    let mut g = 0usize;
                    for j in 0..sh.lb {
                        g = (g << 1) | bit(base + sh.wb + j);
                    }
                    let amp = gray_amp(g, sh.lb);
                    let chips = walsh_bps(idx, sh.wb).into_iter().map(|w| w * amp);
                    if branch == 0 {
                        ci.extend(chips);
                    } else {
                        cq.extend(chips);
                    }
                }
                sym_i += 1;
            }
            done += 1;
        }
        slot += 1;
    }
    let x = shape(&ci, &cq);
    let top = x.iter().fold(1e-12f64, |m, v| m.max(v.abs()));
    x.iter().map(|&v| (v * PEAK / top) as f32).collect()
}

/// Find the frame in `samples` (plain Walsh size `wb`), return `n_bits` coded-bit LLRs, or None.
pub fn demodulate_frame_bps(samples: &[f32], n_bits: usize, wb: usize) -> Option<Vec<f32>> {
    demodulate_frame_shape(samples, n_bits, Shape::walsh(wb))
}

/// `demodulate_frame` for any symbol shape.
pub fn demodulate_frame_shape(samples: &[f32], n_bits: usize, sh: Shape) -> Option<Vec<f32>> {
    locate_and_demodulate_shape(samples, n_bits, sh).map(|(_, llr)| llr)
}

/// As `demodulate_frame_shape`, also returning the sample index where the frame starts.
pub fn locate_and_demodulate_shape(samples: &[f32], n_bits: usize, sh: Shape) -> Option<(usize, Vec<f32>)> {
    let (start, mut all) = locate_and_demodulate_multi(samples, n_bits, &[sh])?;
    Some((start, all.pop()?.1))
}

/// LLRs of one frame under one symbol shape.
pub type SizeLlrs = (Shape, Vec<f32>);

/// Find one frame whose shape is any of `sizes` (all share the preamble) and demodulate it
/// under each shape, returning the start and the LLRs per shape, in the order of `sizes`.
/// Shapes whose frame does not fit inside `samples` are left out.
pub fn locate_and_demodulate_multi(
    samples: &[f32],
    n_bits: usize,
    sizes: &[Shape],
) -> Option<(usize, Vec<SizeLlrs>)> {
    let total = sizes.iter().map(|&s| frame_len_shape(n_bits, s)).min()?;
    if n_bits == 0 || samples.len() < total {
        return None;
    }
    let z = baseband(samples)?;
    let (start, hz, _, tau) = search(&z, total, preamble_wave(false));
    Some((start, demod_shapes(&z, start, hz, tau, n_bits, sizes)))
}

/// Baseband (carrier removed) analytic signal of `samples`, None if they are all zero.
fn baseband(samples: &[f32]) -> Option<Vec<C>> {
    let x: Vec<f64> = samples.iter().map(|&v| if v.is_finite() { v as f64 } else { 0.0 }).collect();
    if x.iter().all(|&v| v == 0.0) {
        return None;
    }
    let a = analytic(&x);
    let w = 2.0 * PI * CARRIER_HZ / FS;
    Some(a.iter().enumerate().map(|(n, &v)| cmul(v, ((w * n as f64).cos(), -(w * n as f64).sin()))).collect())
}

/// Joint start and frequency search for the preamble over a window of `z`. `total` is the least
/// number of samples a frame needs after its start. Returns the start, the frequency and the peak
/// to mean ratio of the correlation at the best frequency.
fn search(z: &[C], total: usize, t: &[f64]) -> (usize, f64, f64, f64) {
    let last = z.len() - total;
    let nfft = (z.len() + t.len()).next_power_of_two();
    let mut pf: Vec<C> = vec![(0.0, 0.0); nfft];
    for (i, &v) in t.iter().enumerate() {
        pf[i] = (v, 0.0);
    }
    fft(&mut pf, false);
    let mut best = (f64::MIN, 0usize, 0.0f64, 0.0f64);
    let try_hz = |hz: f64, best: &mut (f64, usize, f64, f64)| {
        let mut zf = vec![(0.0, 0.0); nfft];
        for (n, zz) in zf.iter_mut().enumerate().take(z.len()) {
            let th = -2.0 * PI * hz * n as f64 / FS;
            *zz = cmul(z[n], (th.cos(), th.sin()));
        }
        fft(&mut zf, false);
        let mut rf: Vec<C> = zf.iter().zip(&pf).map(|(&q, &pv)| cmul(q, (pv.0, -pv.1))).collect();
        fft(&mut rf, true);
        let (mut peak, mut at, mut sum) = (f64::MIN, 0usize, 0.0);
        for (st, r) in rf.iter().enumerate().take(last + 1) {
            let m = r.0 * r.0 + r.1 * r.1;
            sum += m;
            if m > peak {
                peak = m;
                at = st;
            }
        }
        if peak > best.0 {
            *best = (peak, at, hz, peak / (sum / (last + 1) as f64).max(1e-30));
        }
    };
    // Coarse pass every 6 Hz, then +-4 Hz around the winner.
    let mut hz = -MAX_OFFSET_HZ;
    while hz <= MAX_OFFSET_HZ {
        try_hz(hz, &mut best);
        hz += 6.0;
    }
    let centre = best.2;
    for d in [-4.0, -2.0, 2.0, 4.0] {
        try_hz(centre + d, &mut best);
    }
    // The strongest peak may belong to a later frame of a burst. Take the earliest peak that is
    // nearly as strong, and refine it to the local maximum, at the best frequency.
    let mut zf = vec![(0.0, 0.0); nfft];
    for (n, zz) in zf.iter_mut().enumerate().take(z.len()) {
        let th = -2.0 * PI * best.2 * n as f64 / FS;
        *zz = cmul(z[n], (th.cos(), th.sin()));
    }
    fft(&mut zf, false);
    let mut rf: Vec<C> = zf.iter().zip(&pf).map(|(&q, &pv)| cmul(q, (pv.0, -pv.1))).collect();
    fft(&mut rf, true);
    let mag = |st: usize| rf[st].0 * rf[st].0 + rf[st].1 * rf[st].1;
    let first = (0..=last).find(|&st| mag(st) >= 0.4 * best.0).unwrap_or(best.1);
    let start = (first..=(first + 12).min(last)).max_by(|&a, &b| mag(a).total_cmp(&mag(b))).unwrap_or(first);
    // Fractional timing: parabola through the correlation magnitude around the peak.
    let (ml, m0, mr) = (
        if start > 0 { mag(start - 1).sqrt() } else { mag(start).sqrt() },
        mag(start).sqrt(),
        if start < last { mag(start + 1).sqrt() } else { mag(start).sqrt() },
    );
    let den = ml - 2.0 * m0 + mr;
    let tau = if start > 0 && start < last && den.abs() > 1e-12 { (0.5 * (ml - mr) / den).clamp(-0.5, 0.5) } else { 0.0 };
    (start, best.2, best.3, tau)
}

/// Peak-to-mean ratio of the preamble correlation above which a frame is declared found.
const DETECT_RATIO: f64 = 25.0;

/// Look for a KAMI preamble that lies wholly inside `samples`. Returns where it starts and the
/// coarse frequency offset and the fractional timing in samples. Cheap enough to run on a short window of recent audio.
pub fn find_preamble(samples: &[f32]) -> Option<(usize, f64, f64)> {
    if samples.len() < preamble_wave(true).len() {
        return None;
    }
    let z = baseband(samples)?;
    // Both templates; the one that matches better wins (a long preamble matches the long
    // template far better, a short one the short template).
    let mut best: Option<(f64, usize, f64, f64)> = None;
    for short in [false, true] {
        let t = preamble_wave(short);
        if samples.len() < t.len() {
            continue;
        }
        let (start, hz, ratio, tau) = search(&z, t.len(), t);
        if ratio >= DETECT_RATIO && best.is_none_or(|b| ratio > b.0) {
            best = Some((ratio, start, hz, tau));
        }
    }
    best.map(|(_, start, hz, tau)| (start, hz, tau))
}

/// Demodulate the frame that starts at `start` in `samples` under each of `sizes`, given the
/// frequency from `find_preamble`. Shapes whose frame does not fit are left out.
pub fn demodulate_at(samples: &[f32], start: usize, hz: f64, tau: f64, n_bits: usize, sizes: &[Shape]) -> Vec<SizeLlrs> {
    match baseband(samples) {
        Some(z) => demod_shapes(&z, start, hz, tau, n_bits, sizes),
        None => Vec::new(),
    }
}

fn demod_shapes(z: &[C], start: usize, hz: f64, tau: f64, n_bits: usize, sizes: &[Shape]) -> Vec<SizeLlrs> {
    let mut out = Vec::new();
    for &sh in sizes {
        let len = frame_len_shape(n_bits, sh);
        if start + len <= z.len() {
            if let Some((llr, _)) = demod_at(&z[start..start + len], hz, tau, n_bits, sh) {
                out.push((sh, llr));
            }
        }
    }
    out
}

/// Echoes that arrive before the main path, in chips.
const EQ_PRE: isize = 3;
/// Echoes that arrive after it, in chips (8 chips are 4 ms).
const EQ_POST: isize = 8;
/// Equalizer reach either side of the centre, in chips.
const EQ_HALF: isize = 8;

/// One known stretch of the frame: where it starts (in chips) and the +-1 chips sent there.
struct Training {
    start: usize,
    pattern: Vec<f64>,
}

/// Fits a linear MMSE equalizer from one known stretch `r[t.start..]` of the received chips.
/// The stretch is a +-1 sequence with a flat autocorrelation, so correlating what arrived with it
/// at each delay gives the channel's impulse response, one complex tap per chip delay. `None`
/// when the channel shows no echo worth undoing (any plain noise channel): the caller then leaves
/// the chips alone, so such a channel behaves exactly as without an equalizer.
#[allow(clippy::needless_range_loop)]
fn fit_equaliser(r: &[C], t: &Training) -> Option<Vec<C>> {
    let p = t.pattern.len();
    let pn = &t.pattern;
    let margin = EQ_POST.max(EQ_PRE);
    if (p as isize) < 4 * margin || t.start + p > r.len() {
        return None;
    }
    // Work on a copy of the stretch. A little frequency error left over from the preamble turns
    // the phase during the stretch (a slow mode's stretch is a quarter of a second long), which
    // spreads the correlation over other delays and looks like echoes; it is measured and taken
    // out first.
    let mut win: Vec<C> = r[t.start..t.start + p].to_vec();
    let (k0, k1) = (margin, p as isize - margin);
    let taps_of = |win: &[C]| -> Vec<C> {
        let n_taps = (EQ_PRE + EQ_POST + 1) as usize;
        let mut h = vec![(0.0f64, 0.0f64); n_taps];
        for (i, d) in (-EQ_PRE..=EQ_POST).enumerate() {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            let mut cnt = 0.0f64;
            for k in k0..k1 {
                let j = k + d;
                if j >= 0 && (j as usize) < win.len() {
                    let v = win[j as usize];
                    re += v.0 * pn[k as usize];
                    im += v.1 * pn[k as usize];
                    cnt += 1.0;
                }
            }
            h[i] = (re / cnt.max(1.0), im / cnt.max(1.0));
        }
        h
    };
    let mut h = taps_of(&win);
    let main = h.iter().enumerate().max_by(|a, b| (a.1 .0.hypot(a.1 .1)).total_cmp(&b.1 .0.hypot(b.1 .1))).map_or(0, |(i, _)| i);
    let d0 = main as isize - EQ_PRE;
    let mid = (k0 + k1) / 2;
    let half = |from: isize, to: isize| -> C {
        let (mut re, mut im) = (0.0, 0.0);
        for k in from..to {
            let j = (k + d0) as usize;
            if j < win.len() {
                re += win[j].0 * pn[k as usize];
                im += win[j].1 * pn[k as usize];
            }
        }
        (re, im)
    };
    let (c1, c2) = (half(k0, mid), half(mid, k1));
    let sep = (k1 - k0) as f64 / 2.0;
    let omega = (c2.1 * c1.0 - c2.0 * c1.1).atan2(c2.0 * c1.0 + c2.1 * c1.1) / sep;
    if omega.abs() * (p as f64) > 0.1 {
        let centre = p as f64 / 2.0;
        for (k, v) in win.iter_mut().enumerate() {
            let th = -omega * (k as f64 - centre);
            *v = cmul(*v, (th.cos(), th.sin()));
        }
        h = taps_of(&win);
    }
    let at = |k: isize| -> Option<C> { (k >= 0 && (k as usize) < win.len()).then(|| win[k as usize]) };
    let (mut peak_i, mut peak_m) = (0usize, 0.0f64);
    for (i, tap) in h.iter().enumerate() {
        let m = tap.0 * tap.0 + tap.1 * tap.1;
        if m > peak_m {
            peak_m = m;
            peak_i = i;
        }
    }
    if peak_m <= 0.0 {
        return None;
    }
    // Noise in the estimate: what the stretch leaves after the taps are removed.
    let mut resid = 0.0f64;
    let mut cnt = 0.0f64;
    for k in k0..k1 {
        let Some(mut e) = at(k) else { continue };
        for (i, d) in (-EQ_PRE..=EQ_POST).enumerate() {
            let kk = k - d;
            if kk >= 0 && (kk as usize) < p {
                e.0 -= h[i].0 * pn[kk as usize];
                e.1 -= h[i].1 * pn[kk as usize];
            }
        }
        resid += e.0 * e.0 + e.1 * e.1;
        cnt += 1.0;
    }
    let sigma2 = if cnt > 0.0 { resid / cnt } else { 0.0 };
    // Each tap's own estimation noise is about sigma2 / n; a cut of an m-sequence also leaves
    // sidelobes of about 1 / p in power at the other delays. An echo must clearly exceed both.
    let n_eff = (k1 - k0).max(1) as f64;
    // 20 times: with a dozen taps in each of fifty stretches a lower bar (9 times) lets a noise
    // tap through now and then, and one false echo skews the filter at that stretch.
    let floor = 20.0 * sigma2 / n_eff;
    let mut kept = 0;
    for (i, tap) in h.iter_mut().enumerate() {
        if i != peak_i && tap.0 * tap.0 + tap.1 * tap.1 < floor.max(6.0 / p as f64 * peak_m) {
            *tap = (0.0, 0.0);
        } else if i != peak_i {
            kept += 1;
        }
    }
    if kept == 0 {
        return None;
    }
    // Linear MMSE equalizer w[m], m in -EQ_HALF..=EQ_HALF, so that sum_m w[m] h[j-m] ~ delta[j].
    let nw = (2 * EQ_HALF + 1) as usize;
    let lo = -EQ_PRE - EQ_HALF;
    let hi = EQ_POST + EQ_HALF;
    let nj = (hi - lo + 1) as usize;
    let hval = |d: isize| -> C {
        if (-EQ_PRE..=EQ_POST).contains(&d) {
            h[(d + EQ_PRE) as usize]
        } else {
            (0.0, 0.0)
        }
    };
    // Normal equations A w = b with A = H^H H + lambda I, b = H^H e0.
    let lambda = sigma2.max(1e-9);
    let mut a = vec![vec![(0.0f64, 0.0f64); nw + 1]; nw];
    for m1 in 0..nw {
        for m2 in 0..nw {
            let (mut re, mut im) = (0.0, 0.0);
            for j in 0..nj {
                let jj = lo + j as isize;
                let x = hval(jj - (m1 as isize - EQ_HALF));
                let y = hval(jj - (m2 as isize - EQ_HALF));
                // conj(x) * y
                re += x.0 * y.0 + x.1 * y.1;
                im += x.0 * y.1 - x.1 * y.0;
            }
            if m1 == m2 {
                re += lambda;
            }
            a[m1][m2] = (re, im);
        }
        let x = hval(-(m1 as isize - EQ_HALF));
        a[m1][nw] = (x.0, -x.1);
    }
    // Gaussian elimination with partial pivoting.
    for col in 0..nw {
        let mut piv = col;
        for row in col + 1..nw {
            if a[row][col].0.hypot(a[row][col].1) > a[piv][col].0.hypot(a[piv][col].1) {
                piv = row;
            }
        }
        a.swap(col, piv);
        let d = a[col][col];
        let dn = d.0 * d.0 + d.1 * d.1;
        if dn < 1e-18 {
            return None;
        }
        for row in 0..nw {
            if row != col {
                let f = cmul(a[row][col], (d.0 / dn, -d.1 / dn));
                for c in col..=nw {
                    let tt = cmul(f, a[col][c]);
                    a[row][c] = (a[row][c].0 - tt.0, a[row][c].1 - tt.1);
                }
            }
        }
    }
    Some(
        (0..nw)
            .map(|i| {
                let d = a[i][i];
                let dn = d.0 * d.0 + d.1 * d.1;
                cmul(a[i][nw], (d.0 / dn, -d.1 / dn))
            })
            .collect(),
    )
}

/// Undo the echoes of a multipath channel (HF has them: 0.5 to 2 ms is ordinary, and one chip is
/// 0.5 ms) before the Walsh correlation, which needs the chips of a slot to arrive unmixed, and
/// follow the channel as it changes. An equalizer is fitted at the preamble and at every
/// training stretch of the frame; between two of them the filter is blended linearly with chip
/// position. A stretch that shows no echo counts as a plain pass-through, and when none shows
/// one the chips are returned untouched.
fn equalise(r: &mut [C], stretches: &[Training]) {
    let fits: Vec<(f64, Option<Vec<C>>)> =
        stretches.iter().map(|t| (t.start as f64 + t.pattern.len() as f64 / 2.0, fit_equaliser(r, t))).collect();
    if fits.iter().all(|(_, w)| w.is_none()) {
        return;
    }
    let nw = (2 * EQ_HALF + 1) as usize;
    let identity: Vec<C> = (0..nw).map(|i| if i as isize == EQ_HALF { (1.0, 0.0) } else { (0.0, 0.0) }).collect();
    let taps: Vec<&Vec<C>> = fits.iter().map(|(_, w)| w.as_ref().unwrap_or(&identity)).collect();
    let src = r.to_vec();
    let mut seg = 0usize;
    for (k, out) in r.iter_mut().enumerate() {
        let kf = k as f64;
        while seg + 1 < fits.len() && fits[seg + 1].0 <= kf {
            seg += 1;
        }
        let (w0, w1, t) = if seg + 1 >= fits.len() || kf <= fits[seg].0 {
            (taps[seg.min(taps.len() - 1)], taps[seg.min(taps.len() - 1)], 0.0)
        } else {
            (taps[seg], taps[seg + 1], (kf - fits[seg].0) / (fits[seg + 1].0 - fits[seg].0))
        };
        let (mut re, mut im) = (0.0, 0.0);
        for i in 0..nw {
            let idx = k as isize - (i as isize - EQ_HALF);
            if idx < 0 || idx as usize >= src.len() {
                continue;
            }
            let w = ((1.0 - t) * w0[i].0 + t * w1[i].0, (1.0 - t) * w0[i].1 + t * w1[i].1);
            let v = cmul(w, src[idx as usize]);
            re += v.0;
            im += v.1;
        }
        *out = (re, im);
    }
}

/// Demodulate the frame that starts at `zs[0]` (baseband), given the coarse frequency `coarse`.
/// Soft decisions and the SNR estimate of one demodulated frame or burst.
fn demod_at(zs: &[C], coarse: f64, tau: f64, n_bits: usize, sh: Shape) -> Option<(Vec<f32>, f64)> {
    let l = sh.chips();
    // Matched filter centred tau samples after the integer start estimate.
    let norm = rrc_norm();
    let h: Vec<f64> = (0..NT).map(|t| rrc_raw(t as f64 - HALF as f64 - tau) / norm).collect();
    let nch = n_chips(n_bits, sh);
    // Matched filter, sampled at the chip centres, with the coarse offset removed.
    let u: Vec<C> = zs
        .iter()
        .enumerate()
        .map(|(n, &v)| {
            let th = -2.0 * PI * coarse * n as f64 / FS;
            cmul(v, (th.cos(), th.sin()))
        })
        .collect();
    let mut r: Vec<C> = (0..nch)
        .map(|k| {
            let (mut re, mut im) = (0.0, 0.0);
            for (t, &ht) in h.iter().enumerate() {
                let v = u[SPS * k + t];
                re += ht * v.0;
                im += ht * v.1;
            }
            (re, im)
        })
        .collect();
    // Fine frequency from the preamble chips.
    let pn = &preamble()[..sh.pre_chips()];
    let mut fbest = (f64::MIN, 0.0f64);
    let mut df = -8.0;
    while df <= 8.0 {
        let mut acc = (0.0, 0.0);
        for (k, &p) in pn.iter().enumerate() {
            let th = -2.0 * PI * df * k as f64 / CHIP_RATE;
            let v = cmul(r[k], (p * th.cos(), p * th.sin()));
            acc.0 += v.0;
            acc.1 += v.1;
        }
        let mag = acc.0 * acc.0 + acc.1 * acc.1;
        if mag > fbest.0 {
            fbest = (mag, df);
        }
        df += 0.25;
    }
    for (k, v) in r.iter_mut().enumerate() {
        let th = -2.0 * PI * fbest.1 * k as f64 / CHIP_RATE;
        *v = cmul(*v, (th.cos(), th.sin()));
    }
    let sc = sh.slot_chips();
    let pre = sh.pre_chips();
    let nsym = n_syms(n_bits, sh);
    let pe = sh.pilot_every();
    let g = sh.pilot_group();
    let slots = n_slots(n_data_slots(n_bits, sh), pe, g);
    let train = train_pattern(sh);
    // Equalizer: from the preamble, and for the slow modes from every training stretch too.
    let mut stretches = vec![Training { start: 0, pattern: pn.to_vec() }];
    if sh.tracked() {
        for s0 in (0..slots).step_by(pe) {
            if pre + (s0 + g) * sc <= r.len() {
                stretches.push(Training { start: pre + s0 * sc, pattern: train.clone() });
            }
        }
    }
    equalise(&mut r, &stretches);
    let block = |slot: usize| &r[pre + slot * sc..pre + (slot + 1) * sc];

    // Pilots: phase and amplitude per pilot group (known chips, so correlate with them).
    let mut pil_slots = Vec::new();
    let mut pil_phase: Vec<f64> = Vec::new();
    let mut pil_amp: Vec<f64> = Vec::new();
    for s in (0..slots).step_by(pe) {
        let (mut re, mut im) = (0.0, 0.0);
        for (i, slot) in (s..s + g).enumerate() {
            for (t, v) in block(slot).iter().enumerate() {
                let pt = train[i * sc + t];
                re += v.0 * pt;
                im += v.1 * pt;
            }
        }
        pil_slots.push(s as f64 + (g as f64 - 1.0) / 2.0);
        pil_phase.push(im.atan2(re));
        pil_amp.push((re * re + im * im).sqrt() / (g * sc) as f64);
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

    // Noise variance per normalised chip component. The pilot is purely real after phase
    // correction, so what is left on the quadrature axis is noise (plus a little phase error).
    // Scaling the metrics by 1/(2 sigma^2) makes them true max-log LLRs, which dense amplitude
    // constellations need.
    let (mut q, mut qn) = (0.0, 0usize);
    for (i, &ps) in pil_slots.iter().enumerate() {
        let (c, sn) = ((-pil_phase[i]).cos(), (-pil_phase[i]).sin());
        let amp = pil_amp[i].max(1e-9);
        let first = (ps - (g as f64 - 1.0) / 2.0).round() as usize;
        for (gi, slot) in (first..first + g).enumerate() {
            for (t, v) in block(slot).iter().enumerate() {
                let pt = train[gi * sc + t];
                let im = (v.0 * pt * sn + v.1 * pt * c) / amp;
                q += im * im;
                qn += 1;
            }
        }
    }
    let sigma2 = if qn > 0 { (q / qn as f64).max(1e-4) } else { 1.0 };
    let rows: Vec<Vec<f64>> = (0..l).map(|k| walsh_bps(k, sh.wb)).collect();
    let nvals = 1usize << sh.bits();
    let syms: Vec<(usize, f64)> =
        (0..nvals).map(|v| (v >> sh.lb, gray_amp(v & ((1 << sh.lb) - 1), sh.lb))).collect();
    let mut llr = Vec::with_capacity(nsym * sh.slot_bits());
    for s in (0..slots).filter(|s| s % pe >= g) {
        let ph = interp(&pil_phase, s as f64);
        let amp = interp(&pil_amp, s as f64).max(1e-9);
        let (c, sn) = ((-ph).cos(), (-ph).sin());
        let rot: Vec<C> = block(s).iter().map(|&v| ((v.0 * c - v.1 * sn) / amp, (v.0 * sn + v.1 * c) / amp)).collect();
        for sym in rot.chunks(l) {
            for branch in 0..2 {
                let real: Vec<f64> = sym.iter().map(|v| if branch == 0 { v.0 } else { v.1 }).collect();
                let corr: Vec<f64> = rows.iter().map(|row| real.iter().zip(row).map(|(x, y)| x * y).sum()).collect();
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
                    llr.push(((b0 - b1) / (2.0 * sigma2)).clamp(-LLR_MAX, LLR_MAX) as f32);
                }
            }
        }
    }
    llr.truncate(n_bits);
    // SNR per real audio sample: chip amplitudes are normalised to the pilot, so the mean signal
    // power per real sample is 1 and the noise power is sigma2.
    Some((llr, (-10.0 * sigma2.log10() - SNR_OFFSET_DB).clamp(-30.0, 45.0)))
}

/// One row of the mode table shown to the operator.
#[derive(Clone, Debug, PartialEq)]
pub struct ModeRow {
    pub name: &'static str,
    /// SNR (dB, per real audio sample) at which under 5 percent of frames are lost.
    pub need_snr: f32,
    /// Raw coded bits per second while data slots are on the air.
    pub raw_bps: u32,
    /// Net information bits per second in a burst of 8 codewords, preamble and pilots included.
    pub net_bps: u32,
}

/// The modes the app can use, slowest and most robust first. The SNR figures are the measured
/// 5 percent frame-loss points from `docs/KAMI-MODES.md`.
pub fn mode_table() -> Vec<ModeRow> {
    let rows: [(&str, Shape, f32); 11] = [
        ("KAMI-256", Shape::walsh(8), -16.0),
        ("KAMI-128", Shape::walsh(7), -13.2),
        ("KAMI-64", Shape::walsh(6), -10.5),
        ("KAMI-32", Shape::walsh(5), -8.1),
        ("KAMI-16", Shape::walsh(4), -5.3),
        ("KAMI-8", Shape::walsh(3), -2.3),
        ("KAMI-4", Shape::walsh(2), -1.8),
        ("KAMI-1a", Shape { wb: 0, lb: 1 }, -0.2),
        ("KAMI-1b", Shape { wb: 0, lb: 2 }, 4.8),
        ("KAMI-1c", Shape { wb: 0, lb: 3 }, 8.9),
        ("KAMI-1d", Shape { wb: 0, lb: 4 }, 13.0),
    ];
    rows.iter()
        .map(|&(name, sh, need_snr)| ModeRow {
            name,
            need_snr,
            raw_bps: sh.raw_bps().round() as u32,
            net_bps: (8.0 * 324.0 * FS / burst_len_shape(648, 8, sh) as f64).round() as u32,
        })
        .collect()
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
        v.extend(std::iter::repeat_n(0.0, 400));
        v
    }

    fn noise_rms(x: &[f32], snr_db: f64) -> f64 {
        let p = x.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / x.len() as f64;
        (p / 10f64.powf(snr_db / 10.0)).sqrt()
    }

    #[test]
    fn walsh_rows_orthogonal_for_every_size() {
        for wb in 1..=8 {
            let l = 1usize << wb;
            for a in 0..l.min(16) {
                for b in 0..l.min(16) {
                    let d: f64 = walsh_bps(a, wb).iter().zip(&walsh_bps(b, wb)).map(|(x, y)| x * y).sum();
                    assert_eq!(d, if a == b { l as f64 } else { 0.0 });
                }
            }
        }
    }

    #[test]
    fn pulse_has_unit_energy_and_no_intersymbol_interference() {
        let h = rrc();
        let e: f64 = h.iter().map(|v| v * v).sum();
        assert!((e - 1.0).abs() < 1e-9);
        // Matched-filter autocorrelation at multiples of one chip is about zero (Nyquist).
        for k in 1..5 {
            let c: f64 = (0..NT - SPS * k).map(|i| h[i] * h[i + SPS * k]).sum();
            assert!(c.abs() < 0.02, "isi {c} at {k} chips");
        }
    }

    #[test]
    fn every_size_stays_inside_the_ofdm_channel() {
        let mut rng = Rng(5);
        let coded: Vec<u8> = (0..648).map(|_| (rng.uniform() > 0.5) as u8).collect();
        for sh in [Shape::walsh(1), Shape::walsh(3), Shape::walsh(5), Shape::walsh(8), SHAPE_8H] {
            let tx = modulate_frame_shape(&coded, sh);
            assert_eq!(tx.len(), frame_len_shape(coded.len(), sh));
            let n = 2048usize;
            let mut psd = vec![0.0f64; n / 2];
            let mut cnt = 0.0;
            for seg in tx.chunks_exact(n / 2).collect::<Vec<_>>().windows(2) {
                let mut buf: Vec<C> = (0..n)
                    .map(|i| {
                        let v = if i < n / 2 { seg[0][i] } else { seg[1][i - n / 2] } as f64;
                        (v * (0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos()), 0.0)
                    })
                    .collect();
                fft(&mut buf, false);
                for (b, p) in buf.iter().take(n / 2).zip(psd.iter_mut()) {
                    *p += b.0 * b.0 + b.1 * b.1;
                }
                cnt += 1.0;
            }
            assert!(cnt > 0.0);
            let total: f64 = psd.iter().sum();
            let inband: f64 = psd
                .iter()
                .enumerate()
                .filter(|(i, _)| {
                    let f = *i as f64 * FS / n as f64;
                    (250.0..=2750.0).contains(&f)
                })
                .map(|(_, p)| p)
                .sum();
            assert!(inband / total > 0.99, "{sh:?}: only {:.4} of the power is in 250-2750 Hz", inband / total);
        }
    }

    #[test]
    fn noiseless_roundtrip_exact_for_every_size() {
        for wb in 1..=8 {
            let sh = Shape::walsh(wb);
            let d = data(wb);
            let coded = ldpc::encode(&frame::build_payload_as(&d, id_for(wb)));
            let tx = modulate_frame_shape(&coded, sh);
            let llr = demodulate_frame_shape(&pad(&tx, 137 + wb * 11), coded.len(), sh).expect("frame not found");
            let hard: Vec<u8> = llr.iter().map(|&l| u8::from(l < 0.0)).collect();
            assert_eq!(hard, coded, "KAMI-{} hard decisions differ without noise", sh.chips());
        }
        let coded = ldpc::encode(&frame::build_payload_as(&data(9), ID8H));
        let tx = modulate_frame_shape(&coded, SHAPE_8H);
        let llr = demodulate_frame_shape(&pad(&tx, 77), coded.len(), SHAPE_8H).expect("8H not found");
        let hard: Vec<u8> = llr.iter().map(|&l| u8::from(l < 0.0)).collect();
        assert_eq!(hard, coded);
    }

    #[test]
    #[ignore]
    fn scratch_matrix() {
        let mut rng = Rng(777);
        for (wb, snr) in [(2usize, 14.0), (5, 6.0), (8, 0.0), (5, -6.0), (8, -12.0)] {
            for (f, &hz) in [-40.0f64, 7.7, 25.0, 55.0].iter().enumerate() {
                let sh = Shape::walsh(wb);
                let d = data(f);
                let coded = ldpc::encode(&frame::build_payload_as(&d, id_for(wb)));
                let tx = modulate_frame_shape(&coded, sh);
                let sigma = noise_rms(&tx, snr);
                let mut rx = shift(&pad(&tx, 50 + f * 23), hz);
                for v in rx.iter_mut() {
                    *v += (sigma * rng.gauss()) as f32;
                }
                let llr = demodulate_frame_shape(&rx, coded.len(), sh);
                let ok = llr.and_then(|l| ldpc::decode(&l, 50)).and_then(|b| frame::parse_payload_id(&b)) == Some((d, id_for(wb)));
                println!("wb {wb} snr {snr} hz {hz}: {}", if ok { "ok" } else { "FAIL" });
            }
        }
    }

    #[test]
    fn offset_and_noise_roundtrip() {
        let mut rng = Rng(777);
        for (wb, snr) in [(2usize, 14.0), (5, 6.0), (8, 0.0)] {
            for (f, &hz) in [-40.0f64, 7.7, 25.0, 55.0].iter().enumerate() {
                let sh = Shape::walsh(wb);
                let d = data(f);
                let coded = ldpc::encode(&frame::build_payload_as(&d, id_for(wb)));
                let tx = modulate_frame_shape(&coded, sh);
                let sigma = noise_rms(&tx, snr);
                let mut rx = shift(&pad(&tx, 50 + f * 23), hz);
                for v in rx.iter_mut() {
                    *v += (sigma * rng.gauss()) as f32;
                }
                let llr = demodulate_frame_shape(&rx, coded.len(), sh).expect("frame not found");
                let bits = ldpc::decode(&llr, 50).expect("decodes");
                assert_eq!(frame::parse_payload_id(&bits), Some((d, id_for(wb))), "KAMI-{} offset {hz}", sh.chips());
            }
        }
    }

    #[test]
    fn all_shapes_are_found_together() {
        let mut rng = Rng(4242);
        let shapes = [Shape::walsh(8), Shape::walsh(5), Shape::walsh(3), Shape::walsh(1), SHAPE_8H];
        let ids = [ID256, ID, ID8, ID2, ID8H];
        for (k, &sh) in shapes.iter().enumerate() {
            let d = data(k);
            let coded = ldpc::encode(&frame::build_payload_as(&d, ids[k]));
            let tx = modulate_frame_shape(&coded, sh);
            let sigma = noise_rms(&tx, 12.0);
            let mut rx = shift(&pad(&tx, 90), 18.0);
            for v in rx.iter_mut() {
                *v += (sigma * rng.gauss()) as f32;
            }
            let (_, tries) = locate_and_demodulate_multi(&rx, coded.len(), &shapes).expect("frame not found");
            let (_, llr) = tries.iter().find(|(b, _)| *b == sh).expect("shape demodulated");
            let bits = ldpc::decode(llr, 50).expect("decodes");
            assert_eq!(frame::parse_payload_id(&bits), Some((d, ids[k])), "shape {sh:?}");
        }
        assert!(frame_len_bps(648, 1) < frame_len_bps(648, 5) && frame_len_bps(648, 5) < frame_len_bps(648, 8));
    }

    #[test]
    fn mode_table_matches_the_ladder_and_rises_in_speed() {
        let t = mode_table();
        assert_eq!(t.len(), 11);
        for w in t.windows(2) {
            assert!(w[1].need_snr > w[0].need_snr, "{} to {}", w[0].name, w[1].name);
            assert!(w[1].net_bps > w[0].net_bps, "{} to {}", w[0].name, w[1].name);
        }
        let d = t.iter().find(|m| m.name == "KAMI-1d").unwrap();
        assert!(d.net_bps > 4000 && d.raw_bps == 16000, "{d:?}");
        assert_eq!(t[3].name, "KAMI-32");
        assert_eq!(t[3].raw_bps, 625);
    }

    #[test]
    fn burst_of_codewords_shares_one_preamble_and_decodes() {
        let mut rng = Rng(99);
        for (sh, id) in [(Shape::walsh(1), ID2), (Shape { wb: 0, lb: 2 }, ID1B), (Shape { wb: 0, lb: 3 }, ID1C)] {
            let cws: Vec<Vec<u8>> =
                (0..5).map(|j| ldpc::encode(&frame::build_payload_as(&data(j), id))).collect();
            let tx = modulate_burst_shape(&cws, sh);
            assert_eq!(tx.len(), burst_len_shape(ldpc::N, 5, sh));
            // Sharing the preamble beats five separate frames.
            assert!(tx.len() < 5 * frame_len_shape(ldpc::N, sh));
            let mut rx = shift(&pad(&tx, 211), 9.0);
            let sigma = noise_rms(&rx, 30.0);
            for v in rx.iter_mut() {
                *v += (sigma * rng.gauss()) as f32;
            }
            let (start, hz, tau) = find_preamble(&rx).expect("burst preamble");
            let llrs = demodulate_burst_at(&rx, start, hz, tau, ldpc::N, 5, sh).expect("burst fits");
            for (j, llr) in llrs.iter().enumerate() {
                let bits = ldpc::decode(llr, 50).expect("codeword decodes");
                assert_eq!(frame::parse_payload_id(&bits), Some((data(j), id)), "{sh:?} codeword {j}");
            }
        }
    }
}
