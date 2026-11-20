//! Heard-station list: who has been on air recently, how well, through whom, and what they
//! last said about themselves (position, status). Capped, oldest-first eviction.
use super::{esc, unesc};
use crate::payload_presence::Status;

const MAGIC: &str = "KHL1";
/// Default number of stations kept.
pub const DEFAULT_CAP: usize = 500;

/// How a station was heard.
#[derive(Clone, Debug, PartialEq)]
pub struct Observation {
    pub snr_db: f32,
    pub channel_khz: f32,
    /// Relay callsign the frame came through, or `None` when heard direct.
    pub via: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Heard {
    pub call: String,
    pub first_heard: u64,
    pub last_heard: u64,
    pub count: u32,
    pub last_snr_db: f32,
    pub best_snr_db: f32,
    pub channel_khz: f32,
    pub via: Option<String>,
    /// Last position as (lat, lon) degrees.
    pub position: Option<(f64, f64)>,
    pub status: Option<Status>,
    /// Maidenhead locator (4 or 6 characters) from a position or grid beacon.
    pub grid: Option<String>,
}

impl Heard {
    /// True when the station was heard directly (no relay) at its last contact.
    pub fn direct(&self) -> bool {
        self.via.is_none()
    }
}

#[derive(Clone, Debug)]
pub struct HeardList {
    cap: usize,
    entries: Vec<Heard>,
}

impl Default for HeardList {
    fn default() -> Self {
        Self::new(DEFAULT_CAP)
    }
}

fn status_name(s: Status) -> &'static str {
    match s {
        Status::Listening => "listening",
        Status::Available => "available",
        Status::Busy => "busy",
        Status::Away => "away",
    }
}

fn status_from(s: &str) -> Option<Status> {
    Some(match s {
        "listening" => Status::Listening,
        "available" => Status::Available,
        "busy" => Status::Busy,
        "away" => Status::Away,
        _ => return None,
    })
}

impl HeardList {
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            entries: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Records one reception. Returns true when the station is new or was silent for more
    /// than `returning_after_s` seconds (a "came on air" moment, used to release its mailbox).
    pub fn hear(&mut self, call: &str, now: u64, obs: Observation, returning_after_s: u64) -> bool {
        let call = call.to_ascii_uppercase();
        if let Some(e) = self.entries.iter_mut().find(|e| e.call == call) {
            let returning = now.saturating_sub(e.last_heard) > returning_after_s;
            e.last_heard = now;
            e.count = e.count.saturating_add(1);
            e.last_snr_db = obs.snr_db;
            if obs.snr_db > e.best_snr_db {
                e.best_snr_db = obs.snr_db;
            }
            e.channel_khz = obs.channel_khz;
            e.via = obs.via;
            return returning;
        }
        if self.entries.len() >= self.cap {
            if let Some(i) = self
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.last_heard)
                .map(|(i, _)| i)
            {
                self.entries.remove(i);
            }
        }
        self.entries.push(Heard {
            call,
            first_heard: now,
            last_heard: now,
            count: 1,
            last_snr_db: obs.snr_db,
            best_snr_db: obs.snr_db,
            channel_khz: obs.channel_khz,
            via: obs.via,
            position: None,
            status: None,
            grid: None,
        });
        true
    }

    pub fn set_position(&mut self, call: &str, lat: f64, lon: f64) {
        let g = super::grid::to_grid(lat, lon, 6);
        if let Some(e) = self.find_mut(call) {
            e.position = Some((lat, lon));
            e.grid = g;
        }
    }

    /// Records a grid locator; also sets the position to its centre unless a more exact
    /// position is already known for the same cell.
    pub fn set_grid(&mut self, call: &str, grid: &str) {
        let Some(centre) = super::grid::from_grid(grid) else {
            return;
        };
        if let Some(e) = self.find_mut(call) {
            let keep = e.position.is_some_and(|(la, lo)| {
                super::grid::to_grid(la, lo, grid.len()).is_some_and(|g| g.eq_ignore_ascii_case(grid))
            });
            if !keep {
                e.position = Some(centre);
            }
            e.grid = Some(grid.to_ascii_uppercase());
        }
    }

    pub fn set_status(&mut self, call: &str, status: Status) {
        if let Some(e) = self.find_mut(call) {
            e.status = Some(status);
        }
    }

    fn find_mut(&mut self, call: &str) -> Option<&mut Heard> {
        let call = call.to_ascii_uppercase();
        self.entries.iter_mut().find(|e| e.call == call)
    }

    pub fn get(&self, call: &str) -> Option<&Heard> {
        let call = call.to_ascii_uppercase();
        self.entries.iter().find(|e| e.call == call)
    }

    /// True when `call` was heard within `window_s` seconds of `now`.
    pub fn on_air(&self, call: &str, now: u64, window_s: u64) -> bool {
        self.get(call)
            .is_some_and(|e| now.saturating_sub(e.last_heard) <= window_s)
    }

    /// Most recently heard first, at most `limit`.
    pub fn recent(&self, limit: usize) -> Vec<&Heard> {
        let mut v: Vec<&Heard> = self.entries.iter().collect();
        v.sort_by(|a, b| b.last_heard.cmp(&a.last_heard).then_with(|| a.call.cmp(&b.call)));
        v.truncate(limit);
        v
    }

    /// Stations heard within `window_s`, strongest last SNR first.
    pub fn active(&self, now: u64, window_s: u64) -> Vec<&Heard> {
        let mut v: Vec<&Heard> = self
            .entries
            .iter()
            .filter(|e| now.saturating_sub(e.last_heard) <= window_s)
            .collect();
        v.sort_by(|a, b| {
            b.last_snr_db
                .partial_cmp(&a.last_snr_db)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.call.cmp(&b.call))
        });
        v
    }

    /// Stations heard directly that could relay to `target`: heard within the window, never
    /// `target` itself. Best SNR first. Used to pick a next hop for a held message.
    pub fn relay_candidates(&self, target: &str, now: u64, window_s: u64) -> Vec<&Heard> {
        let target = target.to_ascii_uppercase();
        self.active(now, window_s)
            .into_iter()
            .filter(|e| e.direct() && e.call != target)
            .collect()
    }

    /// Great-circle distance in km from `(lat, lon)` to a station's last position.
    pub fn distance_km(&self, call: &str, lat: f64, lon: f64) -> Option<f64> {
        let (la, lo) = self.get(call)?.position?;
        Some(haversine_km(lat, lon, la, lo))
    }

    pub fn to_text(&self) -> String {
        let mut s = format!("{MAGIC}\n");
        for e in &self.entries {
            let (lat, lon) = e.position.map_or((String::new(), String::new()), |(a, b)| {
                (a.to_string(), b.to_string())
            });
            s.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                esc(&e.call),
                e.first_heard,
                e.last_heard,
                e.count,
                e.last_snr_db,
                e.best_snr_db,
                e.channel_khz,
                esc(e.via.as_deref().unwrap_or("")),
                lat,
                lon,
                e.status.map_or("", status_name),
                e.grid.as_deref().unwrap_or(""),
            ));
        }
        s
    }

    pub fn from_text(text: &str, cap: usize) -> Option<Self> {
        let mut lines = text.lines();
        if lines.next()? != MAGIC {
            return None;
        }
        let mut list = Self::new(cap);
        for line in lines.filter(|l| !l.is_empty()) {
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() != 12 {
                return None;
            }
            let position = if f[8].is_empty() {
                None
            } else {
                Some((f[8].parse().ok()?, f[9].parse().ok()?))
            };
            list.entries.push(Heard {
                call: unesc(f[0]),
                first_heard: f[1].parse().ok()?,
                last_heard: f[2].parse().ok()?,
                count: f[3].parse().ok()?,
                last_snr_db: f[4].parse().ok()?,
                best_snr_db: f[5].parse().ok()?,
                channel_khz: f[6].parse().ok()?,
                via: if f[7].is_empty() { None } else { Some(unesc(f[7])) },
                position,
                status: status_from(f[10]),
                grid: if f[11].is_empty() {
                    None
                } else {
                    Some(f[11].to_string())
                },
            });
        }
        Some(list)
    }
}

pub fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dp = p2 - p1;
    let dl = (lon2 - lon1).to_radians();
    let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * 6371.0 * a.sqrt().asin()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(snr: f32, via: Option<&str>) -> Observation {
        Observation {
            snr_db: snr,
            channel_khz: 18111.0,
            via: via.map(String::from),
        }
    }

    #[test]
    fn new_then_returning() {
        let mut h = HeardList::default();
        assert!(h.hear("g4abc", 100, obs(5.0, None), 3600));
        assert!(!h.hear("G4ABC", 200, obs(8.0, None), 3600));
        assert!(h.hear("G4ABC", 200 + 3601, obs(1.0, Some("RLY1")), 3600));
        let e = h.get("g4abc").unwrap();
        assert_eq!((e.count, e.best_snr_db, e.last_snr_db), (3, 8.0, 1.0));
        assert!(!e.direct());
    }

    #[test]
    fn evicts_oldest() {
        let mut h = HeardList::new(2);
        h.hear("A1", 10, obs(0.0, None), 60);
        h.hear("B1", 20, obs(0.0, None), 60);
        h.hear("C1", 30, obs(0.0, None), 60);
        assert!(h.get("A1").is_none() && h.get("B1").is_some() && h.get("C1").is_some());
    }

    #[test]
    fn active_and_candidates() {
        let mut h = HeardList::default();
        h.hear("A1", 100, obs(2.0, None), 60);
        h.hear("B1", 100, obs(9.0, None), 60);
        h.hear("C1", 100, obs(20.0, Some("B1")), 60);
        h.hear("D1", 1, obs(30.0, None), 60);
        let act: Vec<_> = h.active(110, 60).iter().map(|e| e.call.clone()).collect();
        assert_eq!(act, ["C1", "B1", "A1"]);
        let rc: Vec<_> = h
            .relay_candidates("A1", 110, 60)
            .iter()
            .map(|e| e.call.clone())
            .collect();
        assert_eq!(rc, ["B1"]);
        assert!(h.on_air("a1", 150, 60) && !h.on_air("D1", 150, 60));
    }

    #[test]
    fn save_round_trip_and_distance() {
        let mut h = HeardList::default();
        h.hear("A1", 5, obs(3.5, Some("R\t1")), 60);
        h.set_position("A1", 51.5, -0.12);
        h.set_status("A1", Status::Busy);
        h.hear("B1", 6, obs(-4.0, None), 60);
        let back = HeardList::from_text(&h.to_text(), 10).unwrap();
        assert_eq!(back.get("A1"), h.get("A1"));
        assert_eq!(back.get("B1"), h.get("B1"));
        assert_eq!(h.get("A1").unwrap().grid.as_deref(), Some("IO91WM"));
        let d = h.distance_km("A1", 48.85, 2.35).unwrap();
        assert!((300.0..390.0).contains(&d), "{d}");
        assert!(HeardList::from_text("junk", 5).is_none());
    }
}
