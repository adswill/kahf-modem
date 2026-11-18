//! Medium access: when may this station transmit? (NETWORK-DESIGN L0.)
//!
//! Pure logic, no clock and no radio: the caller says what it heard and when (milliseconds
//! on any steady clock) and asks `check`. The rules, in short:
//!
//! - Never start while a burst is being received.
//! - After a burst ends, the station it was *for* may answer after `SIFS_MS`. Everyone else
//!   waits `DIFS_MS` plus a random back-off, so an answer always goes first.
//! - Back-off is counted in slots, only while the channel is quiet, and the window doubles
//!   after every failure (`CW_MIN` to `CW_MAX`), so retries cannot run in lock-step.
//! - A train of transmissions is cut after `TRAIN_MAX_MS`, and while other stations are heard
//!   a station keeps to `SHARE_MAX_PCT` of the last `SHARE_WINDOW_MS`. Urgent traffic and
//!   answers are exempt.

use std::collections::VecDeque;

/// Gap after a burst before the addressed station answers.
pub const SIFS_MS: u64 = 300;
/// Gap after a burst before anyone else starts.
pub const DIFS_MS: u64 = 900;
/// Back-off slot.
pub const SLOT_MS: u64 = 250;
pub const CW_MIN: u32 = 4;
pub const CW_MAX: u32 = 64;
/// Back-off window for urgent traffic.
pub const CW_URGENT: u32 = 2;
/// Longest run of transmissions before a forced gap.
pub const TRAIN_MAX_MS: u64 = 20_000;
/// Two transmissions closer than this belong to one train.
const TRAIN_JOIN_MS: u64 = 3_000;
/// The forced gap after a full train.
const TRAIN_GAP_MS: u64 = 4_000;
pub const SHARE_WINDOW_MS: u64 = 120_000;
pub const SHARE_MAX_PCT: u64 = 25;
/// Other stations count as "active" for the share rule this long after being heard.
const ACTIVE_MS: u64 = 60_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// A reply the other station is waiting for (ACK, grant).
    Answer,
    /// Ordinary traffic.
    Data,
    /// Urgent or emergency traffic.
    Urgent,
}

#[derive(Clone, Debug)]
pub struct Mac {
    cw: u32,
    /// Quiet time still to wait, in ms, once the long gap has passed.
    backoff_ms: Option<u64>,
    rx_active: bool,
    last_rx_end: Option<u64>,
    /// Last time another station was heard at all.
    heard_at: Option<u64>,
    tx_until: Option<u64>,
    /// Finished transmissions, newest last: (start, end).
    tx_log: VecDeque<(u64, u64)>,
    train_start: Option<u64>,
    last_check: u64,
    rng: u64,
}

impl Mac {
    pub fn new(seed: u64) -> Self {
        Mac {
            cw: CW_MIN,
            backoff_ms: None,
            rx_active: false,
            last_rx_end: None,
            heard_at: None,
            tx_until: None,
            tx_log: VecDeque::new(),
            train_start: None,
            last_check: 0,
            rng: seed | 1,
        }
    }

    fn rand(&mut self, n: u32) -> u32 {
        // xorshift64*
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        ((x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 33) % n.max(1) as u64) as u32
    }

    fn draw(&mut self, cw: u32) {
        let slots = self.rand(cw) as u64;
        self.backoff_ms = Some(slots * SLOT_MS);
    }

    /// The channel was busy when this station wanted it: it will need a back-off.
    fn defer(&mut self, class: Class) {
        if class != Class::Answer && self.backoff_ms.is_none() {
            let cw = if class == Class::Urgent { CW_URGENT } else { self.cw };
            self.draw(cw);
        }
    }

    /// A burst from another station started arriving.
    pub fn on_rx_start(&mut self, now: u64) {
        self.rx_active = true;
        self.heard_at = Some(now);
    }

    /// The burst finished (or was given up).
    pub fn on_rx_end(&mut self, now: u64) {
        self.rx_active = false;
        self.last_rx_end = Some(now);
        self.heard_at = Some(now);
    }

    /// Our transmission starts now and ends at `end`.
    pub fn on_tx_start(&mut self, now: u64, end: u64) {
        self.tx_until = Some(end);
        let joins = self.train_start.is_some() && self.tx_log.back().is_some_and(|&(_, e)| now.saturating_sub(e) < TRAIN_JOIN_MS);
        if !joins {
            self.train_start = Some(now);
        }
        self.tx_log.push_back((now, end));
        while self.tx_log.front().is_some_and(|&(_, e)| e + SHARE_WINDOW_MS < now) {
            self.tx_log.pop_front();
        }
    }

    /// Our transmission ended. A new back-off starts, as after every send.
    pub fn on_tx_end(&mut self, now: u64) {
        self.tx_until = None;
        self.last_rx_end = Some(now);
        let cw = self.cw;
        self.draw(cw);
    }

    /// The exchange worked (an answer came back): the window shrinks to the minimum.
    pub fn success(&mut self) {
        self.cw = CW_MIN;
    }

    /// No answer came: the window doubles and a new back-off is drawn.
    pub fn failure(&mut self) {
        self.cw = (self.cw * 2).min(CW_MAX);
        let cw = self.cw;
        self.draw(cw);
    }

    pub fn window(&self) -> u32 {
        self.cw
    }

    fn tx_ms_in_window(&self, now: u64) -> u64 {
        let from = now.saturating_sub(SHARE_WINDOW_MS);
        self.tx_log
            .iter()
            .map(|&(s, e)| e.min(now).saturating_sub(s.max(from)))
            .sum()
    }

    /// May a transmission of class `class` start at `now`? `None`: yes. `Some(ms)`: not yet,
    /// ask again after about that long.
    pub fn check(&mut self, now: u64, class: Class) -> Option<u64> {
        let dt = now.saturating_sub(self.last_check);
        self.last_check = now;
        if let Some(end) = self.tx_until {
            if now < end {
                return Some(end - now);
            }
        }
        if self.rx_active {
            self.defer(class);
            return Some(SLOT_MS);
        }
        let quiet = now.saturating_sub(self.last_rx_end.unwrap_or(0));
        let gap = if class == Class::Answer { SIFS_MS } else { DIFS_MS };
        if self.last_rx_end.is_some() && quiet < gap {
            self.defer(class);
            return Some(gap - quiet);
        }
        if class == Class::Answer {
            return None;
        }
        // Back-off counts down only while the channel stays quiet past the long gap. A station
        // that finds the channel quiet and has no back-off pending goes at once.
        if let Some(left) = self.backoff_ms {
            let counted = dt.min(quiet.saturating_sub(DIFS_MS).max(0)).min(left);
            let left = left - counted;
            self.backoff_ms = Some(left);
            if left > 0 {
                return Some(left.min(SLOT_MS));
            }
        }
        if class == Class::Urgent {
            self.backoff_ms = None;
            return None;
        }
        // Fair share and train limit apply to ordinary traffic only.
        if let Some(start) = self.train_start {
            let last_end = self.tx_log.back().map(|&(_, e)| e).unwrap_or(start);
            if now.saturating_sub(last_end) < TRAIN_JOIN_MS && last_end.saturating_sub(start) >= TRAIN_MAX_MS {
                return Some(TRAIN_GAP_MS - now.saturating_sub(last_end).min(TRAIN_GAP_MS - 1));
            }
        }
        if self.heard_at.is_some_and(|h| now.saturating_sub(h) < ACTIVE_MS) {
            let used = self.tx_ms_in_window(now);
            if used * 100 >= SHARE_WINDOW_MS * SHARE_MAX_PCT {
                // Wait until enough of the oldest transmission has left the window.
                let oldest_end = self.tx_log.front().map(|&(_, e)| e).unwrap_or(now);
                let free_at = oldest_end + SHARE_WINDOW_MS;
                return Some(free_at.saturating_sub(now).clamp(SLOT_MS, 5_000));
            }
        }
        self.backoff_ms = None;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_starts_while_a_burst_is_arriving() {
        let mut m = Mac::new(1);
        m.on_rx_start(1000);
        assert!(m.check(2000, Class::Data).is_some());
        assert!(m.check(2000, Class::Answer).is_some());
    }

    #[test]
    fn the_addressed_station_answers_before_anyone_else_may_start() {
        let mut m = Mac::new(2);
        m.on_rx_start(0);
        m.on_rx_end(5000);
        // Answer: allowed after SIFS. Data: still waiting for DIFS.
        assert!(m.check(5100, Class::Answer).is_some());
        assert_eq!(m.check(5000 + SIFS_MS, Class::Answer), None);
        assert!(m.check(5000 + SIFS_MS, Class::Data).is_some());
    }

    #[test]
    fn data_waits_for_the_long_gap_and_the_backoff() {
        let mut m = Mac::new(3);
        m.on_rx_end(0);
        let mut t = 0;
        let mut waited = 0;
        while let Some(w) = m.check(t, Class::Data) {
            let step = w.max(50);
            t += step;
            waited += 1;
            assert!(waited < 100, "never allowed to go");
        }
        assert!(t >= DIFS_MS, "went before the long gap: {t}");
        assert!(t <= DIFS_MS + CW_MIN as u64 * SLOT_MS + 500);
    }

    #[test]
    fn the_window_doubles_on_failure_and_resets_on_success() {
        let mut m = Mac::new(4);
        for want in [8, 16, 32, 64, 64] {
            m.failure();
            assert_eq!(m.window(), want);
        }
        m.success();
        assert_eq!(m.window(), CW_MIN);
    }

    #[test]
    fn two_stations_with_different_seeds_do_not_pick_the_same_time_every_time() {
        let mut same = 0;
        for seed in 0..200u64 {
            let (mut a, mut b) = (Mac::new(seed * 2 + 1), Mac::new(seed * 7919 + 13));
            a.on_rx_end(0);
            b.on_rx_end(0);
            let go = |m: &mut Mac| {
                let mut t = 0;
                while let Some(w) = m.check(t, Class::Data) {
                    t += w.max(50);
                }
                t
            };
            if go(&mut a) / SLOT_MS == go(&mut b) / SLOT_MS {
                same += 1;
            }
        }
        // With 4 slots the chance of a tie is about a quarter, never always.
        assert!(same < 90, "ties {same} of 200");
    }

    #[test]
    fn a_long_train_is_cut_and_others_get_a_turn() {
        let mut m = Mac::new(5);
        m.on_rx_end(0);
        let mut t = 2_000;
        // Back-to-back 5 s bursts.
        for _ in 0..4 {
            m.on_tx_start(t, t + 5_000);
            t += 5_000;
            m.on_tx_end(t);
            m.last_rx_end = None;
            m.backoff_ms = Some(0);
        }
        let w = m.check(t, Class::Data);
        assert!(w.is_some(), "a 20 s train was not cut");
        assert_eq!(m.check(t, Class::Urgent), None, "urgent must ignore the train limit");
    }

    #[test]
    fn a_station_keeps_to_its_share_while_others_are_heard() {
        let mut m = Mac::new(6);
        m.on_rx_end(1_000);
        // 40 s of our own sending in the last two minutes, with others heard.
        m.on_tx_start(10_000, 50_000);
        m.on_tx_end(50_000);
        m.on_rx_start(60_000);
        m.on_rx_end(61_000);
        m.backoff_ms = Some(0);
        assert!(m.check(65_000, Class::Data).is_some(), "over its share and still allowed");
        // Alone on the channel there is no cap.
        let mut alone = Mac::new(7);
        alone.on_tx_start(10_000, 50_000);
        alone.on_tx_end(50_000);
        alone.backoff_ms = Some(0);
        alone.last_rx_end = Some(50_000);
        assert_eq!(alone.check(60_000, Class::Data), None);
    }
}
