//! What the window shows from the engine and the data folder: relay counters, the
//! schedule, hops, contacts and the logbook, as plain data the window can draw.

use std::path::{Path, PathBuf};

use kahf_core::relay::{Direction, Hop, Slot, Stats};
use kahf_log::contacts::{self, Contact};
use kahf_log::Qso;
use serde::Serialize;

/// How many recent forwards the hop log shows.
pub const HOP_ROWS: usize = 20;

pub const CONTACTS_FILE: &str = "contacts.txt";
pub const LOGBOOK_FILE: &str = "logbook.txt";
pub const ADIF_FILE: &str = "logbook.adi";

#[derive(Serialize, Default)]
pub struct StatsView {
    pub sent_today: u64,
    pub sent_total: u64,
    /// (hour start in unix seconds, messages sent in that hour), oldest first.
    pub per_hour: Vec<(u64, u64)>,
}

pub fn stats_view(s: &Stats) -> StatsView {
    StatsView {
        sent_today: s.sent_today,
        sent_total: s.sent_total,
        per_hour: s.sent_per_hour(),
    }
}

#[derive(Serialize)]
pub struct SlotView {
    pub channel_khz: f32,
    pub start_unix: u64,
    pub duration_s: u32,
    /// "receive" or "transmit".
    pub direction: &'static str,
}

impl From<&Slot> for SlotView {
    fn from(s: &Slot) -> Self {
        Self {
            channel_khz: s.channel_khz,
            start_unix: s.start_unix,
            duration_s: s.duration_s,
            direction: match s.direction {
                Direction::Receive => "receive",
                Direction::Transmit => "transmit",
            },
        }
    }
}

#[derive(Serialize)]
pub struct HopView {
    pub message_id: u64,
    pub relay: String,
    pub at_unix: u64,
    pub channel_khz: f32,
    pub heard_by_next: bool,
}

impl From<&Hop> for HopView {
    fn from(h: &Hop) -> Self {
        Self {
            message_id: h.message_id,
            relay: h.relay.clone(),
            at_unix: h.at_unix,
            channel_khz: h.channel_khz,
            heard_by_next: h.heard_by_next,
        }
    }
}

#[derive(Serialize)]
pub struct StationView {
    pub call: String,
    pub name: String,
    pub qth: String,
    pub notes: String,
    /// YYYY-MM-DD, or empty when never heard.
    pub last_heard: String,
}

#[derive(Serialize)]
pub struct QsoView {
    pub call: String,
    pub date: String,
    pub time: String,
    pub band: String,
    pub mode: String,
    pub freq_mhz: Option<f64>,
    pub rst_sent: String,
    pub rst_rcvd: String,
    pub name: String,
    pub comment: String,
}

pub fn read_contacts(dir: &Path) -> Result<Vec<Contact>, String> {
    contacts::load(&dir.join(CONTACTS_FILE))
}

pub fn read_qsos(dir: &Path) -> Result<(PathBuf, Vec<Qso>), String> {
    let path = dir.join(LOGBOOK_FILE);
    let mut list = kahf_log::load(&path)?;
    list.sort_by(|a, b| (&b.date, &b.time).cmp(&(&a.date, &a.time)));
    Ok((path, list))
}

/// Contacts (most recently heard first), the logbook (newest first) and why a file could not
/// be read, if one could not.
pub fn read_files(dir: &Path) -> (Vec<StationView>, Vec<QsoView>, Option<String>) {
    let mut error = None;
    let stations = match read_contacts(dir) {
        Ok(mut list) => {
            // YYYYMMDD sorts as text; never heard goes last.
            list.sort_by(|a, b| b.last_heard.cmp(&a.last_heard));
            list.into_iter()
                .map(|c| StationView {
                    last_heard: c.last_heard.as_deref().map(date).unwrap_or_default(),
                    call: c.call,
                    name: c.name,
                    qth: c.qth,
                    notes: c.notes,
                })
                .collect()
        }
        Err(e) => {
            error = Some(e);
            Vec::new()
        }
    };
    let qsos = match read_qsos(dir) {
        Ok((_, list)) => list
            .into_iter()
            .map(|q| QsoView {
                date: date(&q.date),
                time: time(&q.time),
                call: q.call,
                band: q.band,
                mode: q.mode,
                freq_mhz: q.freq_mhz,
                rst_sent: q.rst_sent,
                rst_rcvd: q.rst_rcvd,
                name: q.name,
                comment: q.comment,
            })
            .collect(),
        Err(e) => {
            error = Some(e);
            Vec::new()
        }
    };
    (stations, qsos, error)
}

/// Writes the logbook as ADIF next to it and returns the path written.
pub fn export_adif(qsos: &[Qso], data_dir: &Path) -> Result<PathBuf, String> {
    let path = data_dir.join(ADIF_FILE);
    std::fs::write(&path, kahf_log::export_adif(qsos))
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(path)
}

/// Centre of a Maidenhead locator square ("KP20" or "KP20le") as (latitude, longitude).
pub fn locator_position(loc: &str) -> Option<(f64, f64)> {
    let b = loc.trim().as_bytes();
    if b.len() != 4 && b.len() != 6 {
        return None;
    }
    let letter = |c: u8, max: u8| {
        let c = c.to_ascii_uppercase();
        (c.is_ascii_uppercase() && c - b'A' < max).then(|| f64::from(c - b'A'))
    };
    let digit = |c: u8| c.is_ascii_digit().then(|| f64::from(c - b'0'));
    let mut lon = letter(b[0], 18)? * 20.0 - 180.0 + digit(b[2])? * 2.0;
    let mut lat = letter(b[1], 18)? * 10.0 - 90.0 + digit(b[3])?;
    let (w_lon, w_lat) = if b.len() == 6 {
        lon += letter(b[4], 24)? * 2.0 / 24.0;
        lat += letter(b[5], 24)? / 24.0;
        (2.0 / 24.0, 1.0 / 24.0)
    } else {
        (2.0, 1.0)
    };
    Some((lat + w_lat / 2.0, lon + w_lon / 2.0))
}

/// The six-character Maidenhead locator of a position, such as "KP20le".
pub fn grid_of(lat: f64, lon: f64) -> String {
    let lon = (lon + 180.0).clamp(0.0, 359.999_999);
    let lat = (lat + 90.0).clamp(0.0, 179.999_999);
    let field = |v: f64, step: f64, base: u8| (base + (v / step) as u8) as char;
    let (lo, la) = (lon, lat);
    format!(
        "{}{}{}{}{}{}",
        field(lo, 20.0, b'A'),
        field(la, 10.0, b'A'),
        (((lo % 20.0) / 2.0) as u8),
        (((la % 10.0) / 1.0) as u8),
        field(lo % 2.0, 2.0 / 24.0, b'a'),
        field(la % 1.0, 1.0 / 24.0, b'a'),
    )
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// UTC hour (0 to 23) and month (1 to 12) of a unix time.
pub fn utc_hour_month(unix: u64) -> (u8, u8) {
    let hour = (unix % 86_400 / 3600) as u8;
    // Civil-from-days (H. Hinnant), month only.
    let z = (unix / 86_400) as i64 + 719_468;
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (hour, month as u8)
}

/// "20261116" as "2026-11-16". Anything else is shown as it is.
pub fn date(yyyymmdd: &str) -> String {
    if yyyymmdd.len() == 8 && yyyymmdd.bytes().all(|b| b.is_ascii_digit()) {
        format!("{}-{}-{}", &yyyymmdd[..4], &yyyymmdd[4..6], &yyyymmdd[6..])
    } else {
        yyyymmdd.to_string()
    }
}

/// "140530" (or "1405") as "14:05". Anything else is shown as it is.
pub fn time(hhmmss: &str) -> String {
    if hhmmss.len() >= 4 && hhmmss.bytes().all(|b| b.is_ascii_digit()) {
        format!("{}:{}", &hhmmss[..2], &hhmmss[2..4])
    } else {
        hhmmss.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_dates_and_times() {
        assert_eq!(date("20261116"), "2026-11-16");
        assert_eq!(date("soon"), "soon");
        assert_eq!(time("140530"), "14:05");
        assert_eq!(utc_hour_month(1_763_643_600), (13, 11));
    }

    #[test]
    fn positions_give_their_locator() {
        assert_eq!(grid_of(60.19, 24.95), "KP20le");
        assert_eq!(grid_of(51.5, 1.0), "JO01mm");
    }

    #[test]
    fn locators_give_the_square_centre() {
        assert_eq!(locator_position("KP20"), Some((60.5, 25.0)));
        for bad in ["", "KP2", "ZZ20", "KP2X", "KP20zz", "KP20l"] {
            assert_eq!(locator_position(bad), None, "{bad}");
        }
    }

    #[test]
    fn reads_files_newest_first_and_exports() {
        let dir = std::env::temp_dir().join(format!("kahf-app-live-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let contact = |call: &str, heard: Option<&str>| Contact {
            call: call.into(),
            name: String::new(),
            qth: String::new(),
            notes: String::new(),
            last_heard: heard.map(Into::into),
        };
        contacts::save(
            &dir.join(CONTACTS_FILE),
            &[
                contact("XX1AA", Some("20261101")),
                contact("XX2BB", None),
                contact("XX3CC", Some("20261119")),
            ],
        )
        .unwrap();
        for (call, d) in [("XX1AA", "20261101"), ("XX3CC", "20261119")] {
            let q = Qso {
                call: call.into(),
                date: d.into(),
                time: "120000".into(),
                band: "17 m".into(),
                mode: "KAHF".into(),
                freq_mhz: None,
                rst_sent: String::new(),
                rst_rcvd: String::new(),
                name: String::new(),
                comment: String::new(),
            };
            kahf_log::add(&dir.join(LOGBOOK_FILE), &q).unwrap();
        }
        let (stations, qsos, err) = read_files(&dir);
        let calls: Vec<_> = stations.iter().map(|c| c.call.as_str()).collect();
        assert_eq!(calls, ["XX3CC", "XX1AA", "XX2BB"]);
        assert_eq!(qsos[0].call, "XX3CC");
        assert_eq!(qsos[0].date, "2026-11-19");
        assert!(err.is_none());
        let (_, all) = read_qsos(&dir).unwrap();
        let adif = export_adif(&all, &dir).unwrap();
        assert!(std::fs::read_to_string(adif).unwrap().contains("XX3CC"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
