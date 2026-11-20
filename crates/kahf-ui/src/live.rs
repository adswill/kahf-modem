//! Live data from the engine and the data folder, shown when sample data is off.
//!
//! The engine keeps relay counters, the schedule, the hop log and the outbox; the
//! contacts and the logbook are files the engine writes in the data folder. The app
//! refreshes this snapshot about once a second, so screens never block on a lock or a read.

use std::path::Path;

use kahf_core::relay::{Direction, Hop, Slot, Stats};
use kahf_daemon::Engine;
use kahf_log::contacts::{self, Contact};
use kahf_log::Qso;

use crate::strings as s;

/// How many recent forwards the hop log shows.
pub const HOP_ROWS: usize = 20;

pub const CONTACTS_FILE: &str = "contacts.txt";
pub const LOGBOOK_FILE: &str = "logbook.txt";
pub const ADIF_FILE: &str = "logbook.adi";

#[derive(Debug, Default)]
pub struct Live {
    /// Stations from the address book, most recently heard first.
    pub stations: Vec<Contact>,
    /// Logged contacts, newest first.
    pub qsos: Vec<Qso>,
    pub stats: Stats,
    /// Accepted relay slots in time order.
    pub slots: Vec<Slot>,
    /// Recent forwards, newest first.
    pub hops: Vec<Hop>,
    /// Outbox items as (id, description), in sending order.
    pub outbox: Vec<(u64, String)>,
    /// Why a data file could not be read, if one could not.
    pub file_error: Option<String>,
}

impl Live {
    /// Copies the engine's relay state and rereads the two data files.
    /// A file that cannot be read keeps its last good contents.
    pub fn refresh(&mut self, engine: Option<&Engine>, data_dir: &Path) {
        match engine {
            Some(e) => {
                self.stats = e.relay_stats();
                self.slots = e.relay_schedule();
                self.hops = e.recent_hops(HOP_ROWS);
                self.outbox = e.outbox_pending();
            }
            None => {
                self.stats = Stats::default();
                self.slots.clear();
                self.hops.clear();
                self.outbox.clear();
            }
        }
        self.file_error = None;
        match contacts::load(&data_dir.join(CONTACTS_FILE)) {
            Ok(mut list) => {
                // YYYYMMDD sorts as text; never heard goes last.
                list.sort_by(|a, b| b.last_heard.cmp(&a.last_heard));
                self.stations = list;
            }
            Err(e) => self.file_error = Some(e),
        }
        match kahf_log::load(&data_dir.join(LOGBOOK_FILE)) {
            Ok(mut list) => {
                list.sort_by(|a, b| (&b.date, &b.time).cmp(&(&a.date, &a.time)));
                self.qsos = list;
            }
            Err(e) => self.file_error = Some(e),
        }
    }

    /// Stations whose call or name contains `query` (already lower case), with their index.
    pub fn stations_matching<'a>(&'a self, query: &'a str) -> impl Iterator<Item = (usize, &'a Contact)> {
        self.stations.iter().enumerate().filter(move |(_, c)| {
            query.is_empty() || c.call.to_lowercase().contains(query) || c.name.to_lowercase().contains(query)
        })
    }
}

/// Writes the logbook as ADIF next to it and returns the path written.
pub fn export_adif(qsos: &[Qso], data_dir: &Path) -> Result<std::path::PathBuf, String> {
    let path = data_dir.join(ADIF_FILE);
    std::fs::write(&path, kahf_log::export_adif(qsos))
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(path)
}

// ------------------------------------------------------------------ formatting

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

/// Unix seconds as "HH:MM" UTC.
pub fn utc_hm(unix: u64) -> String {
    let day_s = unix % 86_400;
    format!("{:02}:{:02}", day_s / 3600, day_s % 3600 / 60)
}

/// Seconds as "45 s", "15 min" or "1 h 30 min".
pub fn duration(secs: u32) -> String {
    match secs {
        0..=59 => format!("{secs} s"),
        60..=3599 => format!("{} min", secs / 60),
        _ if secs % 3600 == 0 => format!("{} h", secs / 3600),
        _ => format!("{} h {} min", secs / 3600, secs % 3600 / 60),
    }
}

/// A channel as "18 104.0 kHz", with a thin space between thousands.
pub fn channel(khz: f32) -> String {
    let whole = khz.trunc() as u64;
    let tenth = ((khz.fract() * 10.0).round() as u64).min(9);
    if whole >= 1000 {
        format!("{} {:03}.{tenth} kHz", whole / 1000, whole % 1000)
    } else {
        format!("{whole}.{tenth} kHz")
    }
}

pub fn direction(d: Direction) -> &'static str {
    match d {
        Direction::Receive => s::DIR_RECEIVE,
        Direction::Transmit => s::DIR_TRANSMIT,
    }
}

pub fn hop_state(h: &Hop) -> &'static str {
    if h.heard_by_next {
        s::HOP_HEARD
    } else {
        s::HOP_NOT_HEARD
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
        assert_eq!(time("0912"), "09:12");
        assert_eq!(utc_hm(86_400 * 3 + 3600 * 7 + 60 * 5 + 9), "07:05");
    }

    #[test]
    fn formats_durations_and_channels() {
        assert_eq!(duration(45), "45 s");
        assert_eq!(duration(900), "15 min");
        assert_eq!(duration(7200), "2 h");
        assert_eq!(duration(5400), "1 h 30 min");
        assert_eq!(channel(18104.0), "18 104.0 kHz");
        assert_eq!(channel(18106.5), "18 106.5 kHz");
        assert_eq!(channel(475.0), "475.0 kHz");
    }

    #[test]
    fn refresh_reads_files_newest_first() {
        let dir = std::env::temp_dir().join(format!("kahf-ui-live-{}", std::process::id()));
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

        let mut live = Live::default();
        live.refresh(None, &dir);
        let calls: Vec<_> = live.stations.iter().map(|c| c.call.as_str()).collect();
        assert_eq!(calls, ["XX3CC", "XX1AA", "XX2BB"]);
        assert_eq!(live.qsos[0].call, "XX3CC");
        assert!(live.file_error.is_none());
        assert_eq!(live.stations_matching("xx1").count(), 1);

        let adif = export_adif(&live.qsos, &dir).unwrap();
        assert!(std::fs::read_to_string(adif).unwrap().contains("XX3CC"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A filled snapshot, as the engine would hand it over during a relay run.
    pub fn filled() -> Live {
        let mut stats = Stats::default();
        for _ in 0..3 {
            stats.count_sent(1_763_640_000, 1_763_596_800);
        }
        let contact = |call: &str, name: &str, heard: &str| Contact {
            call: call.into(),
            name: name.into(),
            qth: "KP20".into(),
            notes: String::new(),
            last_heard: Some(heard.into()),
        };
        Live {
            stations: vec![
                contact("XX3RL", "Relay North", "20261120"),
                contact("XX1AA", "Aino", "20261118"),
            ],
            qsos: vec![Qso {
                call: "XX1AA".into(),
                date: "20261118".into(),
                time: "093000".into(),
                band: "17 m".into(),
                mode: "KAHF".into(),
                freq_mhz: Some(18.104),
                rst_sent: "57".into(),
                rst_rcvd: "55".into(),
                name: String::new(),
                comment: "Long path".into(),
            }],
            stats,
            slots: vec![Slot {
                channel_khz: 18104.0,
                start_unix: 1_763_643_600,
                duration_s: 900,
                direction: Direction::Transmit,
            }],
            hops: vec![Hop {
                message_id: 7,
                relay: "XX3RL".into(),
                at_unix: 1_763_640_000,
                channel_khz: 18104.0,
                heard_by_next: true,
            }],
            outbox: vec![(9, "relay of a message from XX1AA to XX9ZZ".into())],
            file_error: None,
        }
    }

    #[test]
    fn screens_show_live_data() {
        use crate::app::{App, Role, Section};
        let settings = iced::Settings {
            default_font: crate::theme::UI_FONT,
            ..Default::default()
        };
        let cases: [(Section, &[&str]); 3] = [
            (
                Section::Relay,
                &[
                    "18 104.0 kHz",
                    "13:00",
                    "15 min",
                    "Send",
                    "XX3RL",
                    "12:00",
                    "Heard",
                    "relay of a message from XX1AA to XX9ZZ",
                    "3",
                ],
            ),
            (Section::Stations, &["XX3RL", "Relay North", "2026-11-20", "KP20"]),
            (
                Section::Logbook,
                &["XX1AA", "2026-11-18", "09:30", "17 m", "Long path"],
            ),
        ];
        for (section, expected) in cases {
            let mut app = App::init();
            app.role = Role::Relay;
            app.section = section;
            app.live = filled();
            let mut sim =
                iced_test::Simulator::with_size(settings.clone(), iced::Size::new(1512.0, 940.0), app.view());
            for text in expected {
                assert!(sim.find(*text).is_ok(), "{section:?} does not show {text:?}");
            }
        }
    }

    /// Renders the live screens offscreen into ~/kahf-work/shots/, like tech::shots.
    /// Run with: ICED_TEST_BACKEND=tiny-skia cargo test -p kahf-ui render_live -- --ignored
    #[test]
    #[ignore]
    fn render_live_screens() {
        use crate::app::{App, Role, Section};
        let dir = std::path::PathBuf::from(std::env::var("HOME").unwrap()).join("kahf-work/shots");
        std::fs::create_dir_all(&dir).unwrap();
        let settings = iced::Settings {
            default_font: crate::theme::UI_FONT,
            ..Default::default()
        };
        for (section, name) in [
            (Section::Relay, "live-relay"),
            (Section::Stations, "live-stations"),
            (Section::Logbook, "live-logbook"),
            (Section::Operate, "live-operate"),
        ] {
            let mut app = App::init();
            app.role = Role::Relay;
            app.section = section;
            app.live = filled();
            let mut sim =
                iced_test::Simulator::with_size(settings.clone(), iced::Size::new(1512.0, 940.0), app.view());
            let snap = sim.snapshot(&app.theme()).unwrap();
            let path = dir.join(format!("{name}.png"));
            let _ = std::fs::remove_file(dir.join(format!("{name}-tiny-skia.png")));
            let _ = std::fs::remove_file(&path);
            snap.matches_image(&path).unwrap();
        }
    }
}
