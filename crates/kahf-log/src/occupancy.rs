//! Channel occupancy model.
//!
//! Built only from what the station hears itself or receives from reports.
//! It ranks channels by how busy they look; it never transmits and never
//! decides to transmit.
//!
//! Each observation counts inside a sliding window. Newer observations weigh
//! more: an observation's weight falls linearly from 1 (now) to 0 (at the
//! window edge). Observations at or beyond the window are ignored.

/// Level at or below this counts as fully quiet.
const LEVEL_QUIET_DB: f32 = -90.0;
/// Level at or above this counts as fully busy.
const LEVEL_BUSY_DB: f32 = -30.0;
/// Share of the busy score that comes from marker sightings.
const MARKER_WEIGHT: f32 = 0.6;
/// Share of the busy score that comes from the mean level.
const LEVEL_WEIGHT: f32 = 0.4;

/// One reading of a channel by the station, or a report from another station.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Observation {
    pub channel_khz: u32,
    pub at_unix: u64,
    pub level_db: f32,
    pub marker_seen: bool,
}

/// Sliding-window occupancy model over observations.
#[derive(Debug, Clone)]
pub struct Occupancy {
    window_secs: u64,
    observations: Vec<Observation>,
    /// Newest observation time seen so far. Used as "now" by `quiet_slots`.
    latest_unix: u64,
}

impl Occupancy {
    pub fn new(window_secs: u64) -> Self {
        Self {
            window_secs,
            observations: Vec::new(),
            latest_unix: 0,
        }
    }

    pub fn add(&mut self, obs: Observation) {
        self.latest_unix = self.latest_unix.max(obs.at_unix);
        self.observations.push(obs);
    }

    /// Busy score in 0..=1 for one channel at `now_unix`: 0 is quiet, 1 is busy.
    /// A channel with no observations in the window scores 0.
    pub fn busy_score(&self, channel_khz: u32, now_unix: u64) -> f32 {
        let mut weight_sum = 0.0_f32;
        let mut marker_sum = 0.0_f32;
        let mut level_sum = 0.0_f32;
        for obs in self.observations.iter().filter(|o| o.channel_khz == channel_khz) {
            let Some(w) = self.weight(obs, now_unix) else {
                continue;
            };
            weight_sum += w;
            if obs.marker_seen {
                marker_sum += w;
            }
            level_sum += w * obs.level_db;
        }
        if weight_sum <= 0.0 {
            return 0.0;
        }
        let marker_frac = marker_sum / weight_sum;
        let mean_db = level_sum / weight_sum;
        let level_norm = ((mean_db - LEVEL_QUIET_DB) / (LEVEL_BUSY_DB - LEVEL_QUIET_DB)).clamp(0.0, 1.0);
        (MARKER_WEIGHT * marker_frac + LEVEL_WEIGHT * level_norm).clamp(0.0, 1.0)
    }

    /// Candidate channels with their busy scores, quietest first.
    /// Ties are broken by lower channel frequency.
    pub fn ranked_channels(&self, candidates: &[u32], now_unix: u64) -> Vec<(u32, f32)> {
        let mut ranked: Vec<(u32, f32)> = candidates
            .iter()
            .map(|&ch| (ch, self.busy_score(ch, now_unix)))
            .collect();
        ranked.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        ranked
    }

    /// Slot starts in `from_khz..=to_khz` (each slot is `width_khz` wide and must
    /// fit before `to_khz`) that have no marker seen in the window. The window is
    /// measured back from the newest observation the model holds.
    pub fn quiet_slots(&self, from_khz: u32, to_khz: u32, width_khz: u32) -> Vec<u32> {
        if width_khz == 0 {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut start = from_khz as u64;
        let end_limit = to_khz as u64;
        while start + width_khz as u64 <= end_limit {
            let slot_end = start + width_khz as u64;
            let busy = self.observations.iter().any(|o| {
                o.marker_seen
                    && (o.channel_khz as u64) >= start
                    && (o.channel_khz as u64) < slot_end
                    && self.weight(o, self.latest_unix).is_some()
            });
            if !busy {
                out.push(start as u32);
            }
            start += width_khz as u64;
        }
        out
    }

    /// Weight of an observation at `now_unix`, or `None` if it is outside the window.
    fn weight(&self, obs: &Observation, now_unix: u64) -> Option<f32> {
        if self.window_secs == 0 {
            return None;
        }
        // A future-dated observation is treated as just seen.
        let age = now_unix.saturating_sub(obs.at_unix);
        if age >= self.window_secs {
            return None;
        }
        Some(1.0 - age as f32 / self.window_secs as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(channel_khz: u32, at_unix: u64, level_db: f32, marker_seen: bool) -> Observation {
        Observation {
            channel_khz,
            at_unix,
            level_db,
            marker_seen,
        }
    }

    #[test]
    fn empty_model_scores_zero_everywhere() {
        let m = Occupancy::new(600);
        for ch in [14_000, 14_100, 14_200] {
            assert_eq!(m.busy_score(ch, 1_000), 0.0);
        }
        assert_eq!(m.quiet_slots(14_000, 14_100, 50), vec![14_000, 14_050]);
    }

    #[test]
    fn marker_channel_is_busier_than_empty_channel() {
        let mut m = Occupancy::new(600);
        m.add(obs(14_100, 990, -50.0, true));
        m.add(obs(14_100, 995, -50.0, true));
        let busy = m.busy_score(14_100, 1_000);
        let empty = m.busy_score(14_200, 1_000);
        assert!(busy > empty, "busy {busy} empty {empty}");
        assert_eq!(empty, 0.0);
    }

    #[test]
    fn old_observation_outside_window_does_not_count() {
        let mut m = Occupancy::new(100);
        m.add(obs(14_100, 0, -40.0, true));
        assert_eq!(m.busy_score(14_100, 200), 0.0);
        // Still inside the window at t=50.
        assert!(m.busy_score(14_100, 50) > 0.0);
    }

    #[test]
    fn newer_observations_weigh_more() {
        let mut old_marker = Occupancy::new(100);
        old_marker.add(obs(14_100, 0, -60.0, true));
        old_marker.add(obs(14_100, 99, -60.0, false));
        let mut new_marker = Occupancy::new(100);
        new_marker.add(obs(14_100, 0, -60.0, false));
        new_marker.add(obs(14_100, 99, -60.0, true));
        assert!(new_marker.busy_score(14_100, 100) > old_marker.busy_score(14_100, 100));
    }

    #[test]
    fn ranking_is_quietest_first() {
        let mut m = Occupancy::new(600);
        m.add(obs(14_200, 999, -35.0, true)); // busy
        m.add(obs(14_100, 999, -85.0, false)); // quiet, but heard
        let ranked = m.ranked_channels(&[14_200, 14_100, 14_300], 1_000);
        let order: Vec<u32> = ranked.iter().map(|r| r.0).collect();
        assert_eq!(order, vec![14_300, 14_100, 14_200]);
        assert!(ranked[0].1 <= ranked[1].1 && ranked[1].1 <= ranked[2].1);
    }

    #[test]
    fn quiet_slots_skip_busy_slot() {
        let mut m = Occupancy::new(600);
        m.add(obs(14_105, 1_000, -50.0, true));
        let slots = m.quiet_slots(14_000, 14_300, 50);
        assert_eq!(slots, vec![14_000, 14_050, 14_150, 14_200, 14_250]);
    }

    #[test]
    fn quiet_slots_ignore_expired_markers() {
        let mut m = Occupancy::new(100);
        m.add(obs(14_105, 0, -50.0, true));
        m.add(obs(14_000, 1_000, -50.0, false)); // sets "now" to 1000
        let slots = m.quiet_slots(14_000, 14_100, 50);
        assert_eq!(slots, vec![14_000, 14_050]);
    }
}
