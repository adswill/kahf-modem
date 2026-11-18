//! Lane sensing (MESH-SPEC.md 5.5): listen before every transmission, per lane.
//!
//! The audio is cut into 100 ms frames (10 Hz resolution). For each lane the level is the
//! power within +-240 Hz of its centre. The lane's noise floor is a slow minimum of its level
//! (30 s). A lane is busy when its level is more than 6 dB over its own floor. A station may
//! transmit on a lane only when the lane and its two neighbours are quiet.
//!
//! The classifier names what sits on a lane (voice, CW, a steady carrier, an FT8-like signal
//! with a 15 s cycle) from the last 40 s of levels. It is a set of simple rules tested on
//! synthetic signals only; it has not heard a real band. `Quarantine` keeps a lane out of
//! use for longer each time it is found occupied.

use crate::{kami, narrow};
use std::collections::{HashMap, VecDeque};
use std::f64::consts::PI;

const FS: f64 = 8000.0;
const FRAME: usize = 800;
const BIN_HZ: f64 = 10.0;
/// Bins from -300 to +300 Hz around a centre; the lane itself is -240 to +240.
const BINS: usize = 61;
const LANE_BINS: std::ops::RangeInclusive<usize> = 6..=54;
const HISTORY: usize = 400;
const FLOOR_FRAMES: usize = 300;
/// The noise floor is the lowest half-second level of the last 10 minutes, kept as 20 minima of 30 s
/// each, so a foreign signal that stays on the lane is not mistaken for the noise floor.
const SLOW_BUCKETS: usize = 20;
pub const BUSY_OVER_DB: f32 = 6.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Quiet,
    Voice,
    Cw,
    Carrier,
    /// Periodic 15 s transmissions of a narrow signal: FT8-like.
    Digital15s,
    /// Busy, but none of the above.
    Unknown,
}

#[derive(Clone, Copy)]
struct Frame {
    db: f32,
    /// Share of the lane's power in its three strongest bins.
    peak: f32,
}

pub struct LaneSense {
    centres: Vec<f64>,
    cos: Vec<Vec<f32>>,
    sin: Vec<Vec<f32>>,
    window: Vec<f32>,
    pending: Vec<f32>,
    hist: Vec<VecDeque<Frame>>,
    slow: Vec<Slow>,
}

#[derive(Default)]
struct Slow {
    cur: Option<f32>,
    frames: usize,
    done: VecDeque<f32>,
}

impl LaneSense {
    /// One lane per audio centre (Hz), in order of frequency. Neighbours are the adjacent entries.
    pub fn new(centres: &[f64]) -> Self {
        let window = (0..FRAME).map(|i| (0.5 - 0.5 * (2.0 * PI * i as f64 / (FRAME - 1) as f64).cos()) as f32).collect();
        let tables = |f: fn(f64) -> f64| -> Vec<Vec<f32>> {
            centres
                .iter()
                .flat_map(|c| {
                    (0..BINS).map(move |b| {
                        let hz = c - 300.0 + b as f64 * BIN_HZ;
                        (0..FRAME).map(|i| f(2.0 * PI * hz * i as f64 / FS) as f32).collect::<Vec<f32>>()
                    })
                })
                .collect()
        };
        LaneSense {
            centres: centres.to_vec(),
            cos: tables(f64::cos),
            sin: tables(f64::sin),
            window,
            pending: Vec::new(),
            hist: centres.iter().map(|_| VecDeque::new()).collect(),
            slow: centres.iter().map(|_| Slow::default()).collect(),
        }
    }

    pub fn lanes(&self) -> usize {
        self.centres.len()
    }

    /// Feed audio. Whole frames are processed, the rest waits.
    pub fn push(&mut self, samples: &[f32]) {
        self.pending.extend_from_slice(samples);
        while self.pending.len() >= FRAME {
            let frame: Vec<f32> = self.pending.drain(..FRAME).zip(&self.window).map(|(a, w)| a * w).collect();
            for k in 0..self.centres.len() {
                let mut p = [0f64; BINS];
                for (b, pb) in p.iter_mut().enumerate() {
                    let (c, s) = (&self.cos[k * BINS + b], &self.sin[k * BINS + b]);
                    let (mut re, mut im) = (0f64, 0f64);
                    for i in 0..FRAME {
                        re += (frame[i] * c[i]) as f64;
                        im += (frame[i] * s[i]) as f64;
                    }
                    *pb = re * re + im * im;
                }
                let lane: Vec<f64> = LANE_BINS.map(|b| p[b]).collect();
                let total: f64 = lane.iter().sum::<f64>().max(1e-30);
                let mut top = lane.clone();
                top.sort_by(|a, b| b.total_cmp(a));
                let peak = (top[0] + top[1] + top[2]) / total;
                let h = &mut self.hist[k];
                h.push_back(Frame { db: (10.0 * total.log10()) as f32, peak: peak as f32 });
                if h.len() > HISTORY {
                    h.pop_front();
                }
                if h.len() >= 5 {
                    let m = mean(h.iter().rev().take(5).map(|f| f.db));
                    let sl = &mut self.slow[k];
                    sl.cur = Some(sl.cur.map_or(m, |c| c.min(m)));
                    sl.frames += 1;
                    if sl.frames >= FLOOR_FRAMES {
                        sl.done.push_back(sl.cur.take().unwrap_or(m));
                        sl.frames = 0;
                        while sl.done.len() > SLOW_BUCKETS {
                            sl.done.pop_front();
                        }
                    }
                }
            }
        }
    }

    /// Level of lane `k` over the last half second (dB, arbitrary reference).
    pub fn level_db(&self, k: usize) -> Option<f32> {
        let h = &self.hist[k];
        (h.len() >= 5).then(|| mean(h.iter().rev().take(5).map(|f| f.db)))
    }

    /// Noise floor of lane `k`: the lowest half-second level of the last 10 minutes.
    pub fn floor_db(&self, k: usize) -> Option<f32> {
        let sl = &self.slow[k];
        sl.done.iter().copied().chain(sl.cur).reduce(f32::min)
    }

    /// How far above its floor lane `k` is, when that is over the busy limit.
    pub fn over_db(&self, k: usize) -> Option<f32> {
        let over = self.level_db(k)? - self.floor_db(k)?;
        (over > BUSY_OVER_DB).then_some(over)
    }

    pub fn busy(&self, k: usize) -> bool {
        self.over_db(k).is_some()
    }

    /// May a station transmit on lane `k` now? The lane and both neighbours must be quiet.
    /// On refusal: the lane that is busy and how far over its floor it is.
    pub fn clear_to_send(&self, k: usize) -> Result<(), (usize, f32)> {
        let lo = k.saturating_sub(1);
        let hi = (k + 1).min(self.centres.len() - 1);
        for j in lo..=hi {
            if let Some(o) = self.over_db(j) {
                return Err((j, o));
            }
        }
        Ok(())
    }

    /// What sits on lane `k`, judged from the last 40 s (the periodic class needs about that).
    pub fn classify(&self, k: usize) -> Class {
        let h = &self.hist[k];
        let Some(floor) = self.floor_db(k) else { return Class::Quiet };
        let on = |f: &Frame| f.db > floor + BUSY_OVER_DB;
        let recent: Vec<&Frame> = h.iter().rev().take(30).collect();
        if recent.iter().filter(|f| on(f)).count() * 2 < recent.len().max(1) && !self.periodic(k, floor) {
            return Class::Quiet;
        }
        // Several adjacent busy lanes with a varying level: speech.
        let run = self.busy_run(k);
        if run >= 3 {
            let m = mean(recent.iter().map(|f| f.db));
            let sd = (recent.iter().map(|f| (f.db - m).powi(2)).sum::<f32>() / recent.len() as f32).sqrt();
            return if sd > 3.0 { Class::Voice } else { Class::Unknown };
        }
        if self.periodic(k, floor) {
            return Class::Digital15s;
        }
        // Narrowness is judged on the frames where something is on the lane.
        let peak = mean(h.iter().filter(|f| on(f)).map(|f| f.peak));
        if peak > 0.8 {
            let runs = on_runs(h.iter().map(on));
            let short = runs.iter().filter(|(_, len)| *len < 20).count();
            return if short >= 4 { Class::Cw } else { Class::Carrier };
        }
        Class::Unknown
    }

    /// Length of the run of adjacent busy lanes that contains lane `k` (0 when `k` is quiet).
    fn busy_run(&self, k: usize) -> usize {
        let on = |j: usize| {
            let h = &self.hist[j];
            let Some(fl) = self.floor_db(j) else { return false };
            let r: Vec<&Frame> = h.iter().rev().take(30).collect();
            r.iter().filter(|f| f.db > fl + BUSY_OVER_DB).count() * 2 >= r.len().max(1) && !r.is_empty()
        };
        if !on(k) {
            return 0;
        }
        let (mut a, mut b) = (k, k);
        while a > 0 && on(a - 1) {
            a -= 1;
        }
        while b + 1 < self.centres.len() && on(b + 1) {
            b += 1;
        }
        b - a + 1
    }

    /// Two or more long transmissions about 15 s apart, with gaps between.
    fn periodic(&self, k: usize, floor: f32) -> bool {
        let runs = on_runs(self.hist[k].iter().map(|f| f.db > floor + BUSY_OVER_DB));
        let long: Vec<usize> = runs.iter().filter(|(_, len)| *len >= 80).map(|(s, _)| *s).collect();
        long.windows(2).any(|w| (130..=170).contains(&(w[1] - w[0])))
    }
}

fn mean(it: impl Iterator<Item = f32>) -> f32 {
    let (s, n) = it.fold((0.0, 0), |(s, n), v| (s + v, n + 1));
    s / n.max(1) as f32
}

/// (start, length) of each run of true values, in frames.
fn on_runs(it: impl Iterator<Item = bool>) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = None;
    let mut n = 0;
    for (i, v) in it.enumerate() {
        n = i + 1;
        match (v, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                out.push((s, i - s));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push((s, n - s));
    }
    out
}

/// True when a KAHF lane signal (a KAMI preamble at one fifth the rate) is in `audio`, the last
/// few seconds of 8 kHz audio, on the lane centred at `centre_hz`.
pub fn hears_kahf(audio: &[f32], centre_hz: f64) -> bool {
    kami::find_preamble(&narrow::from_lane(audio, centre_hz)).is_some()
}

/// Keeps lanes out of use after they were found occupied (MESH-SPEC 5.5): 10 min, then 30 min,
/// then 2 h for the same lane. Voice or CW blocks the lane and both neighbours for 20 min.
#[derive(Default)]
pub struct Quarantine {
    until: HashMap<usize, u64>,
    strikes: HashMap<usize, u32>,
}

impl Quarantine {
    /// Lane `k` was found occupied at `now_s`.
    pub fn occupied(&mut self, k: usize, now_s: u64) {
        let n = self.strikes.entry(k).or_insert(0);
        let wait = [600, 1800, 7200][(*n as usize).min(2)];
        *n += 1;
        self.block(k, now_s + wait);
    }

    /// A human conversation (voice or CW) on lane `k`: it and its neighbours for 20 minutes.
    pub fn human(&mut self, k: usize, now_s: u64) {
        for j in k.saturating_sub(1)..=k + 1 {
            self.block(j, now_s + 1200);
        }
    }

    fn block(&mut self, k: usize, until: u64) {
        let u = self.until.entry(k).or_insert(0);
        *u = (*u).max(until);
    }

    pub fn blocked(&self, k: usize, now_s: u64) -> bool {
        self.until.get(&k).is_some_and(|u| now_s < *u)
    }

    /// The first of `candidates` that is not blocked.
    pub fn pick(&self, candidates: &[usize], now_s: u64) -> Option<usize> {
        candidates.iter().copied().find(|k| !self.blocked(*k, now_s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CENTRES: [f64; 5] = [800.0, 1500.0, 2200.0, 2900.0, 3600.0];

    struct Rng(u64);
    impl Rng {
        fn u(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        }
        fn gauss(&mut self) -> f32 {
            let (a, b) = (self.u(), self.u());
            ((-2.0 * a.ln()).sqrt() * (2.0 * PI * b).cos()) as f32
        }
    }

    const SIGMA: f32 = 0.01;

    fn noise(r: &mut Rng, seconds: f64) -> Vec<f32> {
        (0..(seconds * FS) as usize).map(|_| SIGMA * r.gauss()).collect()
    }

    fn tone(hz: f64, amp: f32, t0: usize, n: usize) -> Vec<f32> {
        (0..n).map(|i| amp * (2.0 * PI * hz * (t0 + i) as f64 / FS).sin() as f32).collect()
    }

    fn add(a: &mut [f32], b: &[f32]) {
        for (x, y) in a.iter_mut().zip(b) {
            *x += y;
        }
    }

    fn sense_of(audio: &[f32]) -> LaneSense {
        let mut s = LaneSense::new(&CENTRES);
        // Feed in odd sized pieces, as a sound card does.
        for c in audio.chunks(1000) {
            s.push(c);
        }
        s
    }

    fn voice(r: &mut Rng, seconds: f64) -> Vec<f32> {
        let n = (seconds * FS) as usize;
        let mut v = vec![0f32; n];
        for h in 1..=24 {
            add(&mut v, &tone(110.0 * h as f64, 0.2 / (h as f32).sqrt(), 0, n));
        }
        // Syllables: about 4 per second, with pauses, as speech does.
        let mut gate = vec![1f32; n];
        let mut i = 0;
        while i < n {
            let len = (FS * (0.12 + 0.2 * r.u())) as usize;
            let on = r.u() > 0.25;
            for g in gate.iter_mut().skip(i).take(len) {
                *g = if on { 1.0 } else { 0.03 };
            }
            i += len;
        }
        v.iter().zip(&gate).map(|(a, g)| a * g).collect()
    }

    #[test]
    fn a_quiet_band_is_clear_and_nothing_is_found() {
        let mut r = Rng(1);
        let s = sense_of(&noise(&mut r, 45.0));
        for k in 0..5 {
            assert!(!s.busy(k), "lane {k}");
            assert_eq!(s.classify(k), Class::Quiet);
            assert!(s.clear_to_send(k).is_ok());
        }
    }

    #[test]
    fn the_six_db_rule_is_a_rule() {
        let mut r = Rng(2);
        let mut a = noise(&mut r, 20.0);
        // Raise every bin by x dB with more white noise, for the last 6 s.
        let up = |x: f64, a: &mut Vec<f32>, r: &mut Rng| {
            let extra = SIGMA * ((10f64.powf(x / 10.0) - 1.0).sqrt() as f32);
            let n = a.len();
            for v in a[n - 6 * 8000..].iter_mut() {
                *v += extra * r.gauss();
            }
        };
        let mut b = a.clone();
        up(4.0, &mut a, &mut r);
        up(8.5, &mut b, &mut r);
        assert!(!sense_of(&a).busy(1), "+4 dB must not count");
        assert!(sense_of(&b).busy(1), "+8.5 dB must count");
    }

    #[test]
    fn speech_is_found_blocks_the_neighbours_and_is_avoided() {
        let mut r = Rng(3);
        let mut a = noise(&mut r, 45.0);
        let v = voice(&mut r, 20.0);
        let n = a.len();
        add(&mut a[n - v.len()..], &v);
        let s = sense_of(&a);
        for k in 0..3 {
            assert!(s.busy(k), "lane {k} should hear the voice");
            assert_eq!(s.classify(k), Class::Voice, "lane {k}");
        }
        // A lane next to the voice is refused even if it is quiet itself.
        assert!(s.clear_to_send(1).is_err());
        let mut q = Quarantine::default();
        q.human(1, 1000);
        assert!(q.blocked(0, 1100) && q.blocked(1, 1100) && q.blocked(2, 1100));
        assert!(!q.blocked(3, 1100) && !q.blocked(1, 2300));
        assert_eq!(q.pick(&[1, 2, 3], 1100), Some(3));
    }

    #[test]
    fn an_ft8_like_signal_is_found_by_its_15_second_cycle() {
        let mut r = Rng(4);
        let secs = 60.0;
        let mut a = noise(&mut r, secs);
        // 12.64 s of eight tones hopping every 160 ms in a 50 Hz span, then a pause, every 15 s.
        for cycle in 0..4 {
            let start = (cycle as f64 * 15.0 * FS) as usize;
            let symbols = (12.64 / 0.16) as usize;
            for sy in 0..symbols {
                let hz = 1480.0 + 6.25 * (r.u() * 8.0).floor();
                let t0 = start + (sy as f64 * 0.16 * FS) as usize;
                let n = (0.16 * FS) as usize;
                if t0 + n <= a.len() {
                    let t = tone(hz, 0.1, t0, n);
                    add(&mut a[t0..t0 + n], &t);
                }
            }
        }
        let s = sense_of(&a);
        assert_eq!(s.classify(1), Class::Digital15s);
        assert!(!s.busy(3));
    }

    #[test]
    fn a_steady_carrier_and_keyed_cw_are_told_apart() {
        let mut r = Rng(5);
        let mut carrier = noise(&mut r, 45.0);
        let n = carrier.len();
        add(&mut carrier[n - 20 * 8000..], &tone(1500.0, 0.1, 0, 20 * 8000));
        assert_eq!(sense_of(&carrier).classify(1), Class::Carrier);

        let mut cw = noise(&mut r, 45.0);
        // Dots and dashes, 0.1 s units, for 20 s.
        let mut t = cw.len() - 20 * 8000;
        let pattern = [1, 1, 1, 3, 1, 1, 3, 1, 3, 3, 1, 1];
        'outer: loop {
            for (j, u) in pattern.iter().enumerate() {
                let on = (0.1 * FS) as usize * u;
                let gap = (0.1 * FS) as usize * if j % 4 == 3 { 3 } else { 1 };
                if t + on >= cw.len() {
                    break 'outer;
                }
                let k = tone(1500.0, 0.1, t, on);
                add(&mut cw[t..t + on], &k);
                t += on + gap;
            }
        }
        assert_eq!(sense_of(&cw).classify(1), Class::Cw);
    }

    #[test]
    fn a_kahf_lane_frame_is_recognised_and_noise_is_not() {
        use crate::{kami::Shape, ldpc};
        let info: Vec<u8> = (0..ldpc::K).map(|i| (i % 3 == 0) as u8).collect();
        let air = kami::modulate_frame_shape(&ldpc::encode(&info), Shape { wb: 0, lb: 3 });
        let mut audio = vec![0f32; 4000];
        audio.extend(narrow::to_lane(&air, 2200.0));
        audio.extend(vec![0f32; 4000]);
        let mut r = Rng(6);
        let n = audio.len();
        add(&mut audio, &noise(&mut r, n as f64 / FS));
        assert!(hears_kahf(&audio, 2200.0));
        assert!(!hears_kahf(&noise(&mut r, n as f64 / FS), 2200.0));
    }

    #[test]
    fn a_lane_found_occupied_stays_out_longer_each_time() {
        let mut q = Quarantine::default();
        q.occupied(2, 0);
        assert!(q.blocked(2, 599) && !q.blocked(2, 600));
        q.occupied(2, 600);
        assert!(q.blocked(2, 600 + 1799) && !q.blocked(2, 600 + 1800));
        q.occupied(2, 3000);
        assert!(q.blocked(2, 3000 + 7199) && !q.blocked(2, 3000 + 7200));
        q.occupied(2, 20000);
        assert!(q.blocked(2, 20000 + 7199), "stays at two hours");
    }
}
