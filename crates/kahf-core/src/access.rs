//! Carrier sense on an energy trace (plan section 2.1).
//!
//! The caller feeds one power value per 50 ms frame for one channel. The level is the
//! mean power over the last 500 ms. The noise floor is the 10th percentile of the level
//! over the last 10 s. A channel is busy when its level is more than 10 dB above the
//! floor, or when a start marker was seen on it.
//!
//! Known limit: the floor is a percentile of the last 10 s, so a channel that is busy for
//! more than 90 percent of that time reads as quiet. Sensing is never a guarantee; the
//! station still checks the channel again before it keys up.
//!
//! This module is pure logic. The FFT based sensing in `sense.rs` works on audio; this
//! type works on a power trace that any caller can build.

use std::collections::VecDeque;

/// Length of one power frame in seconds.
pub const FRAME_S: f32 = 0.050;
/// Frames averaged for the level (500 ms).
pub const LEVEL_FRAMES: usize = 10;
/// Levels kept for the noise floor (10 s).
pub const FLOOR_LEVELS: usize = 200;
/// Levels needed before a floor exists (1 s).
pub const MIN_FLOOR_LEVELS: usize = 20;
/// Busy when the level is more than this many dB above the floor.
pub const BUSY_ABOVE_FLOOR_DB: f32 = 10.0;
/// Percentile used for the floor.
pub const FLOOR_PERCENTILE: f32 = 0.10;
/// Powers below this are treated as this, which keeps the dB values finite.
const MIN_POWER: f32 = 1e-20;

/// Carrier sense state for one channel.
#[derive(Debug, Clone, Default)]
pub struct CarrierSense {
    /// Newest frame powers, at most `LEVEL_FRAMES`.
    frames: VecDeque<f32>,
    /// Level after each full window, newest last, at most `FLOOR_LEVELS`.
    levels: VecDeque<f32>,
}

impl CarrierSense {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one frame of mean power for this channel. Invalid values count as zero.
    pub fn push(&mut self, power: f32) {
        let p = if power.is_finite() && power > 0.0 { power } else { 0.0 };
        self.frames.push_back(p);
        if self.frames.len() > LEVEL_FRAMES {
            self.frames.pop_front();
        }
        if self.frames.len() == LEVEL_FRAMES {
            let level = self.frames.iter().sum::<f32>() / LEVEL_FRAMES as f32;
            self.levels.push_back(level);
            if self.levels.len() > FLOOR_LEVELS {
                self.levels.pop_front();
            }
        }
    }

    /// Mean power over the last 500 ms, or `None` before the first full window.
    pub fn level(&self) -> Option<f32> {
        self.levels.back().copied()
    }

    /// 10th percentile of the recent levels, or `None` until `MIN_FLOOR_LEVELS` exist.
    pub fn noise_floor(&self) -> Option<f32> {
        if self.levels.len() < MIN_FLOOR_LEVELS {
            return None;
        }
        let mut sorted: Vec<f32> = self.levels.iter().copied().collect();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let idx = ((sorted.len() as f32 * FLOOR_PERCENTILE) as usize).min(sorted.len() - 1);
        Some(sorted[idx])
    }

    /// True when the channel must be treated as busy. `marker_seen` is the result of the
    /// start marker detector for this channel. With no level or no floor yet, only the
    /// marker can make the channel busy.
    pub fn is_busy(&self, marker_seen: bool) -> bool {
        if marker_seen {
            return true;
        }
        match (self.level(), self.noise_floor()) {
            (Some(level), Some(floor)) => {
                let level_db = 10.0 * level.max(MIN_POWER).log10();
                let floor_db = 10.0 * floor.max(MIN_POWER).log10();
                level_db - floor_db > BUSY_ABOVE_FLOOR_DB
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(s: &mut CarrierSense, power: f32, frames: usize) {
        for _ in 0..frames {
            s.push(power);
        }
    }

    #[test]
    fn steady_channel_is_not_busy() {
        let mut s = CarrierSense::new();
        feed(&mut s, 1.0, 300);
        assert!(!s.is_busy(false));
    }

    #[test]
    fn level_is_the_mean_of_the_last_500_ms() {
        let mut s = CarrierSense::new();
        for i in 0..LEVEL_FRAMES {
            s.push(if i == 0 { 10.0 } else { 0.0 });
        }
        assert!((s.level().unwrap() - 1.0).abs() < 1e-6);
        assert!(s.level().is_some());
        let mut short = CarrierSense::new();
        feed(&mut short, 1.0, LEVEL_FRAMES - 1);
        assert_eq!(short.level(), None);
    }

    #[test]
    fn burst_20_db_above_floor_is_busy() {
        let mut s = CarrierSense::new();
        feed(&mut s, 1.0, 100);
        feed(&mut s, 100.0, 10);
        assert!(s.is_busy(false));
    }

    #[test]
    fn rise_of_7_db_is_not_busy() {
        let mut s = CarrierSense::new();
        feed(&mut s, 1.0, 100);
        feed(&mut s, 5.0, 10); // about 7 dB
        assert!(!s.is_busy(false));
    }

    #[test]
    fn no_floor_means_only_the_marker_counts() {
        let mut s = CarrierSense::new();
        feed(&mut s, 100.0, 25); // 16 levels, below MIN_FLOOR_LEVELS
        assert_eq!(s.noise_floor(), None);
        assert!(!s.is_busy(false));
        assert!(s.is_busy(true));
    }

    #[test]
    fn start_marker_makes_a_silent_channel_busy() {
        let mut s = CarrierSense::new();
        feed(&mut s, 1.0, 300);
        assert!(!s.is_busy(false));
        assert!(s.is_busy(true));
    }

    #[test]
    fn invalid_power_counts_as_zero() {
        let mut s = CarrierSense::new();
        feed(&mut s, 1.0, 100);
        s.push(f32::NAN);
        s.push(-5.0);
        s.push(f32::INFINITY);
        assert!(s.level().unwrap().is_finite());
        assert!(!s.is_busy(false));
    }

    #[test]
    fn long_signal_becomes_its_own_floor() {
        // Documents the known limit: a channel busy for most of the 10 s window reads as quiet.
        let mut s = CarrierSense::new();
        feed(&mut s, 1.0, 100);
        feed(&mut s, 100.0, FLOOR_LEVELS + LEVEL_FRAMES);
        assert!(!s.is_busy(false));
    }
}
