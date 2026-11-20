//! Listen before talk: the half-duplex rules of one station on one channel.
//!
//! A station never starts a transmission while it hears someone else, waits for a short quiet
//! after the last energy it heard (the gap inside a train of bursts is shorter than that), and
//! then waits a random extra time so that two stations that were both waiting do not start
//! together. Its own transmissions do not count: nothing is classified while it transmits, and
//! for a moment after it, because a sound path such as BlackHole or a monitoring radio plays our
//! own signal back with a delay.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Samples per measuring block (20 ms at 8 kHz).
const BLOCK: usize = 160;
/// Blocks kept to estimate the noise floor (30 s).
const HISTORY: usize = 1500;
/// A block is busy when it is this many dB above the noise floor.
const BUSY_OVER_FLOOR_DB: f32 = 10.0;
/// ...and above this level (dBFS), so digital silence and its rounding noise never count.
const BUSY_MIN_DB: f32 = -80.0;
/// How long the channel must have been quiet before a transmission may start. Longer than the
/// silence between the bursts of one train (0.3 s), so a station does not cut in on a train.
pub const LISTEN_QUIET: Duration = Duration::from_millis(400);
/// How long after our own transmission its playback may still arrive.
pub const ECHO_GUARD: Duration = Duration::from_millis(600);
/// The random extra wait after the channel has been busy, in milliseconds.
const BACKOFF_MS: (u64, u64) = (100, 600);
/// A channel that has been busy for this long without a break is not held any longer: a steady
/// carrier that never ends must not silence the station for good.
const MAX_BUSY_WAIT: Duration = Duration::from_secs(45);

pub struct ChannelWatch {
    pending: Vec<f32>,
    blocks: VecDeque<f32>,
    floor_db: f32,
    since_floor: usize,
    last_energy: Option<Instant>,
    /// Audio before this time is our own playback and is not judged.
    ignore_until: Option<Instant>,
    blocked_since: Option<Instant>,
    backoff_until: Option<Instant>,
    rng: u64,
}

impl ChannelWatch {
    pub fn new(seed: u64) -> Self {
        Self {
            pending: Vec::new(),
            blocks: VecDeque::new(),
            floor_db: -120.0,
            since_floor: 0,
            last_energy: None,
            ignore_until: None,
            blocked_since: None,
            backoff_until: None,
            rng: seed | 1,
        }
    }

    fn random(&mut self, lo: u64, hi: u64) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        lo + self.rng % (hi - lo + 1)
    }

    /// True while samples read now are our own playback.
    pub fn in_echo_guard(&self, now: Instant) -> bool {
        self.ignore_until.is_some_and(|t| now < t)
    }

    /// Our transmission ended: what arrives for a moment is our own signal.
    pub fn tx_finished(&mut self, now: Instant) {
        self.pending.clear();
        self.last_energy = None;
        self.ignore_until = Some(now + ECHO_GUARD);
    }

    /// Judges new receive audio read at `now`.
    pub fn push(&mut self, samples: &[f32], now: Instant) {
        if self.in_echo_guard(now) {
            self.pending.clear();
            return;
        }
        self.pending.extend_from_slice(samples);
        while self.pending.len() >= BLOCK {
            let block: Vec<f32> = self.pending.drain(..BLOCK).collect();
            let mean_sq = block.iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>() / BLOCK as f64;
            let db = if mean_sq <= 0.0 {
                -120.0
            } else {
                (10.0 * mean_sq.log10()) as f32
            }
            .max(-120.0);
            if db > (self.floor_db + BUSY_OVER_FLOOR_DB).max(BUSY_MIN_DB) {
                self.last_energy = Some(now);
            }
            self.blocks.push_back(db);
            if self.blocks.len() > HISTORY {
                self.blocks.pop_front();
            }
            self.since_floor += 1;
            if self.since_floor >= 25 && self.blocks.len() >= 50 {
                self.since_floor = 0;
                let mut sorted: Vec<f32> = self.blocks.iter().copied().collect();
                sorted.sort_by(f32::total_cmp);
                self.floor_db = sorted[sorted.len() / 5];
            }
        }
    }

    /// Whether a transmission may start now. When it was held back by someone else's signal,
    /// it also waits a random time once that signal has ended.
    pub fn may_transmit(&mut self, now: Instant) -> bool {
        let quiet = self
            .last_energy
            .is_none_or(|t| now.duration_since(t) >= LISTEN_QUIET);
        if !quiet {
            let since = *self.blocked_since.get_or_insert(now);
            self.backoff_until = None;
            if now.duration_since(since) > MAX_BUSY_WAIT {
                self.blocked_since = None;
                return true;
            }
            return false;
        }
        if self.blocked_since.is_some() && self.backoff_until.is_none() {
            let ms = self.random(BACKOFF_MS.0, BACKOFF_MS.1);
            self.backoff_until = Some(now + Duration::from_millis(ms));
        }
        if self.backoff_until.is_some_and(|b| now < b) {
            return false;
        }
        self.blocked_since = None;
        self.backoff_until = None;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize, amp: f32) -> Vec<f32> {
        (0..n).map(|i| amp * (i as f32 * 0.4).sin()).collect()
    }

    #[test]
    fn a_quiet_channel_is_free_at_once() {
        let mut w = ChannelWatch::new(1);
        let t = Instant::now();
        w.push(&vec![0.0; 4000], t);
        assert!(w.may_transmit(t));
    }

    #[test]
    fn someone_else_holds_the_channel_until_it_has_been_quiet_and_the_backoff_has_passed() {
        let mut w = ChannelWatch::new(7);
        let t0 = Instant::now();
        w.push(&tone(1600, 0.05), t0);
        assert!(!w.may_transmit(t0 + Duration::from_millis(100)));
        // Quiet again, but not for long enough.
        w.push(&vec![0.0; 800], t0 + Duration::from_millis(200));
        assert!(!w.may_transmit(t0 + Duration::from_millis(350)));
        // Quiet long enough: the random backoff starts and must run out.
        assert!(!w.may_transmit(t0 + Duration::from_millis(700)));
        assert!(w.may_transmit(t0 + Duration::from_millis(700 + 601)));
    }

    #[test]
    fn the_gap_inside_a_train_of_bursts_does_not_free_the_channel() {
        let mut w = ChannelWatch::new(3);
        let t0 = Instant::now();
        w.push(&tone(1600, 0.05), t0);
        w.push(&vec![0.0; 2400], t0 + Duration::from_millis(300));
        w.push(&tone(1600, 0.05), t0 + Duration::from_millis(600));
        assert!(!w.may_transmit(t0 + Duration::from_millis(700)));
    }

    #[test]
    fn our_own_playback_is_not_someone_else() {
        let mut w = ChannelWatch::new(9);
        let t0 = Instant::now();
        w.tx_finished(t0);
        assert!(w.in_echo_guard(t0 + Duration::from_millis(100)));
        // The playback of our own burst arrives just after the transmission.
        w.push(&tone(3200, 0.2), t0 + Duration::from_millis(100));
        assert!(w.may_transmit(t0 + Duration::from_millis(200)));
        assert!(!w.in_echo_guard(t0 + ECHO_GUARD + Duration::from_millis(1)));
    }

    #[test]
    fn a_steady_carrier_does_not_silence_the_station_for_ever() {
        let mut w = ChannelWatch::new(5);
        let t0 = Instant::now();
        let mut t = t0;
        let mut allowed = false;
        for _ in 0..3000 {
            w.push(&tone(160, 0.3), t);
            if w.may_transmit(t) {
                allowed = true;
                break;
            }
            t += Duration::from_millis(20);
        }
        assert!(allowed, "a constant signal held the station for over a minute");
        assert!(t.duration_since(t0) < Duration::from_secs(60));
    }

    #[test]
    fn digital_silence_noise_does_not_count() {
        let mut w = ChannelWatch::new(11);
        let t = Instant::now();
        w.push(&tone(1600, 1.0e-5), t);
        assert!(w.may_transmit(t));
    }
}
