//! Small plain-text files and helpers: recent stations, the settings file, callsign checks and UTC time text.
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// One line per station: callsign, last seen (unix seconds), last message (unix seconds).
const STATIONS_FILE: &str = "recent_stations.txt";
/// Interface and station settings. Always kept in the default data folder.
const CONF_FILE: &str = "gui.conf";
const MAX_RECENT: usize = 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Station {
    pub call: String,
    /// Unix seconds of the last sign of this station; 0 when never.
    pub last_seen: u64,
    /// Unix seconds of the last message or mail; 0 when never.
    pub last_msg: u64,
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn normalise(text: &str) -> String {
    text.trim().to_uppercase()
}

/// Letters and digits only, 3 to 10 characters.
pub fn valid_callsign(call: &str) -> bool {
    let n = call.chars().count();
    (3..=10).contains(&n) && call.chars().all(|c| c.is_ascii_alphanumeric())
}

/// host:port with a numeric port.
pub fn valid_rigctld(addr: &str) -> bool {
    match addr.rsplit_once(':') {
        Some((host, port)) => !host.is_empty() && port.parse::<u16>().is_ok(),
        None => false,
    }
}

pub fn load_stations(dir: &Path) -> Vec<Station> {
    let text = fs::read_to_string(dir.join(STATIONS_FILE)).unwrap_or_default();
    let mut list: Vec<Station> = text.lines().filter_map(parse_station).collect();
    sort(&mut list);
    list.truncate(MAX_RECENT);
    list
}

pub fn save_stations(dir: &Path, list: &[Station]) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let text: String = list
        .iter()
        .map(|s| format!("{} {} {}\n", s.call, s.last_seen, s.last_msg))
        .collect();
    fs::write(dir.join(STATIONS_FILE), text)
}

fn parse_station(line: &str) -> Option<Station> {
    let mut parts = line.split_whitespace();
    let call = normalise(parts.next()?);
    if !valid_callsign(&call) {
        return None;
    }
    let last_seen: u64 = parts.next()?.parse().ok()?;
    let last_msg: u64 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    Some(Station {
        call,
        last_seen,
        last_msg,
    })
}

/// Records a sign of `call` now. `seen` updates the last-seen time, `msg` the last-message time.
pub fn touch(list: &mut Vec<Station>, call: &str, seen: bool, msg: bool) {
    if !valid_callsign(call) {
        return;
    }
    let t = now();
    let index = match list.iter().position(|s| s.call == call) {
        Some(i) => i,
        None => {
            list.push(Station {
                call: call.to_string(),
                last_seen: 0,
                last_msg: 0,
            });
            list.len() - 1
        }
    };
    if seen {
        list[index].last_seen = t;
    }
    if msg {
        list[index].last_msg = t;
    }
    sort(list);
    list.truncate(MAX_RECENT);
}

fn sort(list: &mut [Station]) {
    list.sort_by(|a, b| {
        let ka = a.last_seen.max(a.last_msg);
        let kb = b.last_seen.max(b.last_msg);
        kb.cmp(&ka).then_with(|| a.call.cmp(&b.call))
    });
}

pub fn read_conf(dir: &Path) -> BTreeMap<String, String> {
    fs::read_to_string(dir.join(CONF_FILE))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

pub fn write_conf(dir: &Path, conf: &BTreeMap<String, String>) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let text: String = conf.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    fs::write(dir.join(CONF_FILE), text)
}

/// "2026-11-17 14:05 UTC", or "never" for 0.
pub fn stamp(secs: u64) -> String {
    if secs == 0 {
        return "never".to_string();
    }
    let (y, m, d) = civil((secs / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02} {}", clock(secs))
}

/// "14:05 UTC".
pub fn clock(secs: u64) -> String {
    let rem = secs % 86_400;
    format!("{:02}:{:02} UTC", rem / 3600, rem % 3600 / 60)
}

/// Year, month and day for a count of days since 1970-01-01 (proleptic Gregorian).
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}
