//! The list of relays this station knows, nearest first.
//!
//! Three sources, none of them required: the public server (when there is internet), the
//! last list it gave (kept on disk, so it still works offline), and the callsigns the person
//! typed in by hand. KAHF never depends on the server to work; it only helps find relays.

use std::path::Path;
use std::process::Command;

use serde_json::{json, Value};

use crate::report::SERVER;

/// Kilometres between two points on the earth.
pub fn distance_km(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (la1, lo1, la2, lo2) = (a.0.to_radians(), a.1.to_radians(), b.0.to_radians(), b.1.to_radians());
    let h = ((la2 - la1) / 2.0).sin().powi(2) + la1.cos() * la2.cos() * ((lo2 - lo1) / 2.0).sin().powi(2);
    12742.0 * h.sqrt().asin()
}

fn get_text(url: &str) -> Result<String, String> {
    let out = Command::new("curl")
        .args(["-sS", "-m", "15", "-f", url])
        .output()
        .map_err(|e| format!("curl did not start ({e})"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn parse_time_ms(s: &str) -> Option<u64> {
    // "2026-11-18T09:30:00.000Z": enough of the date to order by; exact time is not needed.
    let (date, rest) = s.split_once('T')?;
    let mut d = date.split('-').filter_map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()?, d.next()?, d.next()?);
    let mut t = rest.trim_end_matches('Z').split(':');
    let h: i64 = t.next()?.parse().ok()?;
    let mi: i64 = t.next()?.parse().ok()?;
    // Days from the civil date (Howard Hinnant's algorithm).
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(((days * 24 + h) * 60 + mi) as u64 * 60_000)
}

/// The relay rows in a `/map-data` answer.
pub fn parse_map_data(text: &str, here: Option<(f64, f64)>) -> Vec<Value> {
    let Ok(v) = serde_json::from_str::<Value>(text) else { return Vec::new() };
    v["relays"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|r| {
                    let call = r["callsign"].as_str()?.to_uppercase();
                    let lat = r["location"]["lat"].as_f64();
                    let lon = r["location"]["lon"].as_f64();
                    let km = match (here, lat, lon) {
                        (Some(h), Some(la), Some(lo)) => Some(distance_km(h, (la, lo)).round()),
                        _ => None,
                    };
                    Some(json!({
                        "call": call, "lat": lat, "lon": lon, "km": km, "source": "server",
                        "lastSeen": r["last_seen"].as_str().and_then(parse_time_ms)
                    }))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Server list (or the saved one when offline) plus the typed-in callsigns, nearest first.
pub fn directory(data_dir: &Path, here: Option<(f64, f64)>, manual: &[String], refresh: bool) -> Result<Vec<Value>, String> {
    let cache = data_dir.join("relays.json");
    let mut error = None;
    let text = if refresh {
        match get_text(&format!("{SERVER}/map-data")) {
            Ok(t) => {
                let _ = std::fs::create_dir_all(data_dir);
                let _ = std::fs::write(&cache, &t);
                Some(t)
            }
            Err(e) => {
                error = Some(e);
                std::fs::read_to_string(&cache).ok()
            }
        }
    } else {
        std::fs::read_to_string(&cache).ok()
    };
    let mut rows = text.map(|t| parse_map_data(&t, here)).unwrap_or_default();
    for call in manual {
        let call = call.trim().to_uppercase();
        if !call.is_empty() && !rows.iter().any(|r| r["call"] == call.as_str()) {
            rows.push(json!({ "call": call, "lat": null, "lon": null, "km": null, "source": "manual", "lastSeen": null }));
        }
    }
    rows.sort_by(|a, b| {
        let ka = a["km"].as_f64().unwrap_or(f64::MAX);
        let kb = b["km"].as_f64().unwrap_or(f64::MAX);
        ka.total_cmp(&kb)
    });
    match (error, rows.is_empty()) {
        (Some(e), true) => Err(e),
        _ => Ok(rows),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn london_to_paris_is_about_340_km() {
        let km = distance_km((51.5, -0.12), (48.86, 2.35));
        assert!((330.0..350.0).contains(&km), "{km}");
    }

    #[test]
    fn map_data_rows_get_a_distance_and_a_time() {
        let t = r#"{"relays":[{"callsign":"dl0net","mode":"relay","last_seen":"2026-11-18T09:30:00.000Z",
            "location":{"mode":"area","lat":50.5,"lon":8.5,"radius_km":70}}],"stations":[]}"#;
        let rows = parse_map_data(t, Some((51.5, -0.12)));
        assert_eq!(rows[0]["call"], "DL0NET");
        assert!(rows[0]["km"].as_f64().unwrap() > 400.0);
        // 2026-11-18 09:30 UTC.
        assert_eq!(rows[0]["lastSeen"].as_u64().unwrap(), 1_794_994_200_000);
    }

    #[test]
    fn typed_callsigns_are_listed_after_the_known_ones_and_garbage_is_ignored() {
        let dir = std::env::temp_dir().join("kahf-dir-test");
        let _ = std::fs::remove_dir_all(&dir);
        let rows = directory(&dir, None, &["w6relay".into(), " ".into()], false).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["call"], "W6RELAY");
        assert_eq!(rows[0]["source"], "manual");
        assert!(parse_map_data("not json", None).is_empty());
    }
}
