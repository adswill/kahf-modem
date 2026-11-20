//! Beacon scheduling: when to send a position beacon, a status/presence beacon, and when a
//! callsign identification is due. Beacons cost real airtime on a shared channel, so they back
//! off while the station is stationary and never fire faster than a minimum gap. The scheduler
//! only decides; the daemon builds the payload (`payload_position`, `payload_presence`).
use super::heard::haversine_km;
use crate::payload_presence::Status;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Interval while moving or freshly started.
    pub base_s: u64,
    /// Longest interval after repeated stationary beacons.
    pub max_s: u64,
    /// Never beacon position more often than this.
    pub min_gap_s: u64,
    /// Moving farther than this since the last beacon counts as movement.
    pub move_km: f64,
    /// Identification interval while transmitting (amateur rules: every 10 minutes).
    pub id_every_s: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            base_s: 900,
            max_s: 3600,
            min_gap_s: 300,
            move_km: 2.0,
            id_every_s: 600,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Due {
    Position,
    Status(Status),
}

#[derive(Clone, Debug)]
pub struct Scheduler {
    s: Settings,
    enabled: bool,
    last_pos_tx: Option<u64>,
    last_pos: Option<(f64, f64)>,
    stationary_beacons: u32,
    status: Status,
    status_sent: Option<Status>,
    last_status_tx: Option<u64>,
    last_tx: Option<u64>,
    last_id: Option<u64>,
}

impl Scheduler {
    pub fn new(s: Settings) -> Self {
        Self {
            s,
            enabled: false,
            last_pos_tx: None,
            last_pos: None,
            stationary_beacons: 0,
            status: Status::Listening,
            status_sent: None,
            last_status_tx: None,
            last_tx: None,
            last_id: None,
        }
    }

    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
    }

    pub fn set_status(&mut self, st: Status) {
        self.status = st;
    }

    /// Current interval for the next position beacon, with stationary back-off (doubling).
    pub fn interval_s(&self) -> u64 {
        let k = self.stationary_beacons.min(8);
        self.s
            .base_s
            .saturating_mul(1u64 << k)
            .min(self.s.max_s)
            .max(self.s.min_gap_s)
    }

    /// What, if anything, to beacon now. At most one is returned; call again after
    /// `beaconed`. `pos` is the station's current position when known.
    pub fn due(&self, now: u64, pos: Option<(f64, f64)>) -> Option<Due> {
        if !self.enabled {
            return None;
        }
        if self.status_sent != Some(self.status)
            && self.last_status_tx.is_none_or(|t| now.saturating_sub(t) >= 60)
        {
            return Some(Due::Status(self.status));
        }
        let pos = pos?;
        match self.last_pos_tx {
            None => Some(Due::Position),
            Some(t) => {
                let age = now.saturating_sub(t);
                let moved = self
                    .last_pos
                    .is_some_and(|p| haversine_km(p.0, p.1, pos.0, pos.1) >= self.s.move_km);
                let want = if moved {
                    self.s.min_gap_s
                } else {
                    self.interval_s()
                };
                (age >= want).then_some(Due::Position)
            }
        }
    }

    /// Records that a beacon went out.
    pub fn beaconed(&mut self, due: Due, now: u64, pos: Option<(f64, f64)>) {
        match due {
            Due::Status(st) => {
                self.status_sent = Some(st);
                self.last_status_tx = Some(now);
            }
            Due::Position => {
                let moved = match (self.last_pos, pos) {
                    (Some(a), Some(b)) => haversine_km(a.0, a.1, b.0, b.1) >= self.s.move_km,
                    _ => true,
                };
                self.stationary_beacons = if moved {
                    0
                } else {
                    self.stationary_beacons.saturating_add(1)
                };
                self.last_pos_tx = Some(now);
                self.last_pos = pos;
            }
        }
        self.note_tx(now);
    }

    /// Records any transmission (for the identification timer).
    pub fn note_tx(&mut self, now: u64) {
        self.last_tx = Some(now);
    }

    /// Records that the callsign was identified (beacons carry it, so they count).
    pub fn note_id(&mut self, now: u64) {
        self.last_id = Some(now);
    }

    /// True when the station has transmitted since its last ID and the ID interval has passed.
    pub fn id_due(&self, now: u64) -> bool {
        match (self.last_tx, self.last_id) {
            (Some(tx), Some(id)) => tx > id && now.saturating_sub(id) >= self.s.id_every_s,
            (Some(_), None) => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: Option<(f64, f64)> = Some((51.5, -0.1));

    #[test]
    fn disabled_is_silent() {
        let s = Scheduler::new(Settings::default());
        assert_eq!(s.due(0, HOME), None);
    }

    #[test]
    fn status_then_position_with_backoff() {
        let mut s = Scheduler::new(Settings::default());
        s.set_enabled(true);
        s.set_status(Status::Available);
        assert_eq!(s.due(0, HOME), Some(Due::Status(Status::Available)));
        s.beaconed(Due::Status(Status::Available), 0, HOME);
        assert_eq!(s.due(1, HOME), Some(Due::Position));
        s.beaconed(Due::Position, 1, HOME);
        assert_eq!(s.due(500, HOME), None);
        assert_eq!(s.due(1 + 900, HOME), Some(Due::Position));
        s.beaconed(Due::Position, 901, HOME);
        assert_eq!(s.interval_s(), 1800, "stationary doubles");
        s.beaconed(Due::Position, 3000, HOME);
        s.beaconed(Due::Position, 7000, HOME);
        assert_eq!(s.interval_s(), 3600, "capped");
        // Moving 10 km resets the back-off and fires after the minimum gap.
        let moved = Some((51.6, -0.1));
        assert_eq!(s.due(7000 + 100, moved), None);
        assert_eq!(s.due(7000 + 300, moved), Some(Due::Position));
        s.beaconed(Due::Position, 7300, moved);
        assert_eq!(s.interval_s(), 900);
    }

    #[test]
    fn status_change_is_rate_limited() {
        let mut s = Scheduler::new(Settings::default());
        s.set_enabled(true);
        s.beaconed(Due::Status(Status::Listening), 100, None);
        s.set_status(Status::Busy);
        assert_eq!(s.due(130, None), None);
        assert_eq!(s.due(160, None), Some(Due::Status(Status::Busy)));
    }

    #[test]
    fn id_timer() {
        let mut s = Scheduler::new(Settings::default());
        assert!(!s.id_due(0), "never transmitted");
        s.note_tx(10);
        assert!(s.id_due(10));
        s.note_id(10);
        assert!(!s.id_due(100));
        s.note_tx(500);
        assert!(!s.id_due(500));
        assert!(s.id_due(700));
        s.note_id(700);
        assert!(!s.id_due(2000), "no tx since last id");
    }
}
