//! Speed ladder: multi-codeword bursts at a range of rates, from tens of bit/s that survive
//! about -9 dB (2500 Hz noise bandwidth) to over 5 kbit/s on a clean channel.
//!
//! A burst carries `n` codewords of 36 payload bytes each (own length and CRC, so a damaged
//! codeword does not cost the others). Rate comes from three knobs:
//! - modulation (QPSK, 16QAM, 64QAM) on the OFDM carriers,
//! - puncturing of the rate-1/2 LDPC code (send only the first `keep` of 648 coded bits;
//!   the decoder sees the dropped bits as zero LLRs),
//! - repetition (`reps` copies of the coded bits, soft-combined at the receiver).
//!
//! Several codewords share one frame, so the sync and channel-estimation overhead is paid once.
//! Most modes use the weak-signal preamble ([`phy::modulate_weak`]), the fastest the normal sync.
//! Both ends must agree on mode and codeword count (the link layer negotiates them).

use crate::phy::{self, Modulation};
use crate::{frame, ldpc};

/// Payload bytes per codeword.
pub const CW_BYTES: usize = frame::MAX_DATA;
/// Most codewords in one burst (about 1150 bytes).
pub const MAX_CODEWORDS: usize = 32;

/// One rung of the ladder.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mode {
    pub name: &'static str,
    pub modulation: Modulation,
    /// Coded bits sent per codeword, 648 down to 432 (rate 1/2 to 3/4).
    pub keep: usize,
    /// Copies of the coded bits.
    pub reps: usize,
    /// Use the weak-signal preamble (long correlation sync). The normal one-symbol sync
    /// needs about 6 dB, and misses a few percent of bursts even on a clean channel.
    pub weak: bool,
    /// Lowest SNR (dB, 2500 Hz) at which the mode is usable on an AWGN channel
    /// (codeword error rate under 10 percent), from `ladder_bench`.
    pub min_snr_db: f32,
}

macro_rules! m {
    ($n:expr, $m:ident, $k:expr, $r:expr, $w:expr, $s:expr) => {
        Mode {
            name: $n,
            modulation: Modulation::$m,
            keep: $k,
            reps: $r,
            weak: $w,
            min_snr_db: $s,
        }
    };
}

/// All modes, slowest and most robust first.
pub const LADDER: [Mode; 11] = [
    m!("R32", Qpsk, 648, 32, true, -9.0),
    m!("R16", Qpsk, 648, 16, true, -8.0),
    m!("R8", Qpsk, 648, 8, true, -6.0),
    m!("R4", Qpsk, 648, 4, true, -3.0),
    m!("Q12", Qpsk, 648, 1, true, 3.0),
    m!("Q23", Qpsk, 486, 1, true, 6.0),
    m!("S12", Qam16, 648, 1, true, 9.0),
    m!("S23", Qam16, 486, 1, true, 12.0),
    m!("T12", Qam64, 648, 1, true, 13.0),
    m!("T34", Qam64, 432, 1, true, 18.0),
    m!("T56", Qam64, 389, 1, true, 21.0),
];

/// Highest-rate mode usable at `snr_db` (2500 Hz noise bandwidth), with `margin_db` in hand.
/// Falls back to the most robust mode.
pub fn pick(snr_db: f32, margin_db: f32) -> &'static Mode {
    LADDER
        .iter()
        .rev()
        .find(|m| snr_db - margin_db >= m.min_snr_db)
        .unwrap_or(&LADDER[0])
}

fn bits_per_symbol(m: Modulation) -> usize {
    match m {
        Modulation::Qpsk => 2,
        Modulation::Qam16 => 4,
        Modulation::Qam64 => 6,
    }
}

impl Mode {
    fn coded_bits(&self, n_cw: usize) -> usize {
        self.keep * self.reps * n_cw
    }

    /// OFDM data symbols in a burst of `n_cw` codewords.
    fn data_symbols(&self, n_cw: usize) -> usize {
        // 114 data carriers per symbol, see phy.
        self.coded_bits(n_cw)
            .div_ceil(114 * bits_per_symbol(self.modulation))
    }

    /// Airtime in seconds of a burst of `n_cw` codewords.
    pub fn airtime_s(&self, n_cw: usize) -> f64 {
        let sym = (2 + self.data_symbols(n_cw)) * 576;
        let pre = if self.weak { phy::WEAK_PRE_LEN } else { 0 };
        (sym + pre) as f64 / 8000.0
    }

    /// Payload bit/s of a full burst, ignoring turnaround and ARQ.
    pub fn rate_bps(&self, n_cw: usize) -> f64 {
        (n_cw * CW_BYTES * 8) as f64 / self.airtime_s(n_cw)
    }
}

/// Codewords in a "long" burst of `mode`. Bursts carry either 1 codeword or this many, so the
/// receiver can find the length by trying two sizes per mode (no header is sent).
pub fn long_codewords(mode: &Mode) -> usize {
    match mode.modulation {
        Modulation::Qpsk if mode.reps > 1 => 1,
        // 5, not 4: four codewords of Q12 are the same waveform as one codeword of R4.
        Modulation::Qpsk => 5,
        Modulation::Qam16 => 8,
        Modulation::Qam64 => 16,
    }
}

/// Position of `mode` in [`LADDER`].
pub fn index_of(mode: &Mode) -> usize {
    LADDER.iter().position(|m| m == mode).unwrap_or(0)
}

/// What every codeword of a [`modulate_frames`] burst carries in the frame's id byte, covered
/// by its CRC: a flag, the ladder index and short or long. The receiver of [`scan`] has to
/// try several (mode, size) guesses, and a guess that is a prefix of the real burst, or the same
/// waveform (one codeword of R4 is four of Q12), would otherwise decode to the wrong thing.
fn descriptor(mode: &Mode, long: bool, kind: FrameKind) -> u8 {
    0x80 | ((index_of(mode) as u8) << 3) | ((kind as u8) << 1) | long as u8
}

/// What a burst's frames are for, so a receiver can route them without a link session. Two bits
/// of the codeword id byte, the same for every codeword of the burst.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FrameKind {
    /// Link frames (connect, data, ack): the default.
    #[default]
    Link = 0,
    /// A beacon (position, grid, presence), sent unconnected.
    Beacon = 1,
    /// A delivery receipt, sent unconnected.
    Receipt = 2,
    /// A relay-held item.
    Relay = 3,
}

impl FrameKind {
    fn from_bits(b: u8) -> FrameKind {
        match b & 3 {
            0 => FrameKind::Link,
            1 => FrameKind::Beacon,
            2 => FrameKind::Receipt,
            _ => FrameKind::Relay,
        }
    }
}

/// Frames (each up to [`CW_BYTES`] bytes) as one burst. One frame, or a mode with long bursts of
/// one codeword, sends a short burst; more frames fill a long burst, padded with empty
/// codewords. Panics if there are more frames than [`long_codewords`].
pub fn modulate_frames(frames: &[Vec<u8>], mode: &Mode, kind: FrameKind) -> Vec<f32> {
    let long = long_codewords(mode);
    assert!(frames.len() <= long, "too many frames for one burst");
    let n_cw = if frames.len() <= 1 { 1 } else { long };
    let mut cws: Vec<Vec<u8>> = frames.to_vec();
    cws.resize(n_cw, Vec::new());
    modulate_codewords(&cws, mode, descriptor(mode, n_cw > 1, kind))
}

/// A burst heard by [`scan`].
#[derive(Debug, PartialEq)]
pub struct Heard {
    /// Index in [`LADDER`] of the mode it was sent with.
    pub mode: usize,
    /// What the burst's frames are for.
    pub kind: FrameKind,
    /// Payloads of the codewords that decoded and were not empty.
    pub frames: Vec<Vec<u8>>,
    /// Sample index just past the burst in the scanned buffer.
    pub end: usize,
}

/// Result of [`scan`].
#[derive(Debug, PartialEq)]
pub enum Scan {
    /// No preamble in the buffer.
    Nothing,
    /// A preamble was found but the burst is not all here yet, or did not decode.
    /// `from` is where the preamble starts, so the caller can drop earlier audio.
    Pending {
        from: usize,
    },
    Heard(Heard),
}

/// Fewest samples worth scanning: a preamble and a minimal burst.
pub const MIN_SCAN_SAMPLES: usize = 4000;

/// Decoder iterations of the cheap first pass of [`scan`].
const QUICK_ITERS: usize = 10;

/// Decodes the burst as `n` codewords of ladder mode `i`, or `None` when that guess is wrong.
fn try_hypothesis(
    wf: &phy::WeakFrame,
    i: usize,
    n: usize,
    end: usize,
    variants: &[Option<usize>],
    iters: usize,
) -> Option<Heard> {
    let mode = &LADDER[i];
    let sets = wf.llrs(mode.modulation, mode.coded_bits(n), variants)?;
    let cws: Vec<Option<(Vec<u8>, u8)>> = (0..n)
        .map(|c| {
            sets.iter().find_map(|llr| {
                let (data, id) = ldpc::decode(&combine(llr, mode, n, c), iters)
                    .and_then(|b| frame::parse_payload_id(&b))?;
                // The kind bits (2..1) are free; everything else must match this guess.
                let want = descriptor(mode, n > 1, FrameKind::Link);
                (id & !0b110 == want).then_some((data, id))
            })
        })
        .collect();
    // Every decoded codeword carries the burst's kind; the first one speaks for it.
    let kind = cws
        .iter()
        .flatten()
        .map(|(_, id)| FrameKind::from_bits(id >> 1))
        .next()?;
    let frames = cws
        .into_iter()
        .flatten()
        .map(|(d, _)| d)
        .filter(|f| !f.is_empty())
        .collect();
    Some(Heard {
        mode: i,
        kind,
        frames,
        end,
    })
}

/// Look for a burst of any mode in `samples` (about the last 16 s of audio). `prefer` is the
/// ladder index tried first (the mode last heard). Cost is about 0.2 s of CPU per call when a
/// preamble is present or not, a little more after a hit.
pub fn scan(samples: &[f32], prefer: usize) -> Scan {
    let Some(wf) = phy::weak_acquire(samples) else {
        return Scan::Nothing;
    };
    let from = wf.frame_start().saturating_sub(phy::WEAK_PRE_LEN + 32);
    // Every (mode, size) hypothesis whose audio is already here, shortest first, the preferred mode first.
    let mut hyps: Vec<(usize, usize, usize)> = Vec::new(); // (mode, n_cw, end)
    for (i, m) in LADDER.iter().enumerate() {
        let mut sizes = vec![1usize];
        if long_codewords(m) > 1 {
            sizes.push(long_codewords(m));
        }
        for n in sizes {
            let end = wf.end_of(m.modulation, m.coded_bits(n));
            if end <= wf.frame_start() + wf.available() {
                hyps.push((i, n, end));
            }
        }
    }
    hyps.sort_by_key(|&(i, _, end)| (i != prefer, end));
    // A wrong guess costs a full demodulation and a failed decode, about a second for all of them,
    // which the receiver cannot afford while the peer sits on another rung. So first a cheap pass
    // (static channel estimate, few decoder iterations) over every guess; only when nothing in it
    // decodes, the thorough pass with tracked estimates and the full iteration count.
    for (variants, iters) in [(&VARIANTS[..1], QUICK_ITERS), (&VARIANTS[..], 50)] {
        for &(i, n, end) in &hyps {
            if let Some(heard) = try_hypothesis(&wf, i, n, end, variants, iters) {
                return Scan::Heard(heard);
            }
        }
    }
    Scan::Pending { from }
}

/// Encode up to `MAX_CODEWORDS * CW_BYTES` bytes of `payload` as one burst of audio samples.
/// Returns the samples and the codeword count the receiver must be told.
pub fn modulate(payload: &[u8], mode: &Mode) -> (Vec<f32>, usize) {
    assert!(
        payload.len() <= MAX_CODEWORDS * CW_BYTES,
        "burst payload too long"
    );
    let n_cw = payload.len().div_ceil(CW_BYTES).max(1);
    let cws: Vec<Vec<u8>> = (0..n_cw)
        .map(|c| {
            let lo = c * CW_BYTES;
            payload[lo.min(payload.len())..(lo + CW_BYTES).min(payload.len())].to_vec()
        })
        .collect();
    (modulate_codewords(&cws, mode, 0), n_cw)
}

fn modulate_codewords(cws: &[Vec<u8>], mode: &Mode, id: u8) -> Vec<f32> {
    let sent: Vec<Vec<u8>> = cws
        .iter()
        .map(|c| ldpc::encode(&frame::build_payload_as(c, id))[..mode.keep].to_vec())
        .collect();
    let mut coded = Vec::with_capacity(mode.coded_bits(cws.len()));
    for _ in 0..mode.reps {
        for cw in &sent {
            coded.extend_from_slice(cw);
        }
    }
    if mode.weak {
        phy::modulate_weak(&coded, mode.modulation)
    } else {
        phy::modulate_frame(&coded, mode.modulation)
    }
}

/// Channel-estimate variants tried in turn: static, then tracked over 7 symbols (about 0.5 s),
/// then over 3 symbols (about 0.2 s). A codeword is taken from the first that decodes.
const VARIANTS: [Option<usize>; 3] = [None, Some(3), Some(1)];

/// Combine repetitions of codeword `c` from one set of LLRs.
fn combine(llr: &[f32], mode: &Mode, n_cw: usize, c: usize) -> Vec<f32> {
    let mut sum = vec![0f32; ldpc::N];
    for r in 0..mode.reps {
        let off = (r * n_cw + c) * mode.keep;
        for (s, l) in sum.iter_mut().zip(&llr[off..off + mode.keep]) {
            *s += l;
        }
    }
    sum
}

/// Decode a burst of `n_cw` codewords. Entry `c` is that codeword's payload bytes, or None if
/// it did not decode. The whole result is None only if the burst could not be found.
pub fn demodulate(samples: &[f32], mode: &Mode, n_cw: usize) -> Option<Vec<Option<Vec<u8>>>> {
    let n_bits = mode.coded_bits(n_cw);
    let sets: Vec<Vec<f32>> = if mode.weak {
        phy::demodulate_weak_variants(samples, mode.modulation, n_bits, &VARIANTS)?
    } else {
        vec![phy::demodulate_frame(samples, mode.modulation, n_bits)?]
    };
    Some(
        (0..n_cw)
            .map(|c| {
                sets.iter().find_map(|llr| {
                    let (data, id) = ldpc::decode(&combine(llr, mode, n_cw, c), 50)
                        .and_then(|b| frame::parse_payload_id(&b))?;
                    (id == 0).then_some(data)
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mode_roundtrips_clean() {
        let payload: Vec<u8> = (0..100u32).map(|i| (i * 7 + 3) as u8).collect();
        for mode in LADDER.iter().filter(|m| m.reps <= 8) {
            let (tx, n) = modulate(&payload, mode);
            assert_eq!(n, 3);
            let mut s = vec![0f32; 3000];
            s.extend_from_slice(&tx);
            s.extend(std::iter::repeat_n(0f32, 3000));
            let got = demodulate(&s, mode, n).unwrap_or_else(|| panic!("{} not found", mode.name));
            let out: Vec<u8> = got
                .into_iter()
                .flat_map(|c| c.unwrap_or_else(|| panic!("{} cw lost", mode.name)))
                .collect();
            assert_eq!(out, payload, "{}", mode.name);
        }
    }

    #[test]
    fn ladder_is_ordered_and_picks_sensibly() {
        for w in LADDER.windows(2) {
            assert!(w[0].min_snr_db <= w[1].min_snr_db);
            assert!(
                w[0].rate_bps(32) < w[1].rate_bps(32),
                "{} vs {}",
                w[0].name,
                w[1].name
            );
        }
        assert_eq!(pick(-30.0, 0.0).name, "R32");
        assert_eq!(pick(40.0, 0.0).name, "T56");
        assert_eq!(pick(11.0, 2.0).name, "S12");
    }

    fn on_air(tx: &[f32]) -> Vec<f32> {
        let mut s = vec![0f32; 5000];
        s.extend_from_slice(tx);
        s.extend(std::iter::repeat_n(0f32, 4000));
        s
    }

    #[test]
    fn scan_finds_any_mode_and_size_without_a_header() {
        for (i, mode) in LADDER.iter().enumerate().filter(|(_, m)| m.reps <= 4) {
            let long = long_codewords(mode);
            for n in [1, long.min(3)] {
                let frames: Vec<Vec<u8>> = (0..n).map(|k| vec![(i * 16 + k) as u8; 5 + k]).collect();
                let air = on_air(&modulate_frames(&frames, mode, FrameKind::Link));
                // Deliberately prefer the wrong mode: the scan must still find it.
                match scan(&air, (i + 3) % LADDER.len()) {
                    Scan::Heard(h) => {
                        assert_eq!(h.mode, i, "{}", mode.name);
                        assert_eq!(h.frames, frames, "{} n={n}", mode.name);
                        assert!(h.end <= air.len());
                    }
                    other => panic!("{} n={n}: {other:?}", mode.name),
                }
            }
        }
    }

    #[test]
    fn scan_on_silence_or_noise_finds_nothing() {
        assert_eq!(scan(&vec![0f32; 20_000], 0), Scan::Nothing);
        let noise: Vec<f32> = (0..30_000u32)
            .map(|i| ((i.wrapping_mul(2654435761) >> 16) as f32 / 32768.0) - 1.0)
            .collect();
        assert_eq!(scan(&noise, 0), Scan::Nothing);
    }

    #[test]
    fn scan_waits_for_a_burst_that_is_not_all_here() {
        let mode = &LADDER[index_of(&LADDER.iter().find(|m| m.name == "R4").copied().unwrap())];
        let air = on_air(&modulate_frames(&[vec![1, 2, 3]], mode, FrameKind::Link));
        let cut = &air[..air.len() - 4000 - 2000];
        assert!(matches!(scan(cut, 0), Scan::Pending { .. } | Scan::Nothing));
    }

    #[test]
    fn frame_kind_travels_with_the_burst() {
        let q12 = LADDER.iter().find(|m| m.name == "Q12").unwrap();
        for kind in [
            FrameKind::Link,
            FrameKind::Beacon,
            FrameKind::Receipt,
            FrameKind::Relay,
        ] {
            let air = on_air(&modulate_frames(&[vec![42; 9]], q12, kind));
            match scan(&air, 0) {
                Scan::Heard(h) => {
                    assert_eq!(h.kind, kind);
                    assert_eq!(h.frames, vec![vec![42; 9]]);
                }
                other => panic!("{kind:?}: {other:?}"),
            }
        }
    }
}
