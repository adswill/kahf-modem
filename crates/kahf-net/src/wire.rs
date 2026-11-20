//! Wire formats shared with the server: canonical JSON, area keys and the
//! channel grid.

use serde_json::Value;
use std::fmt;

/// Canonical JSON, as `canonicalJson` in `server/src/core.js` builds it:
/// object keys sorted at every level, no whitespace. These are the bytes
/// that are signed.
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                write_canonical(&map[key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// A one-degree square named by its south-west corner, such as `N51W1`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Area(String);

impl Area {
    /// The square that holds a position. Latitude 90 falls in the top row
    /// and longitude 180 is the same meridian as -180.
    pub fn from_position(latitude: f64, longitude: f64) -> Result<Area, String> {
        if !(-90.0..=90.0).contains(&latitude) || !(-180.0..=180.0).contains(&longitude) {
            return Err(format!("position {latitude}, {longitude} is out of range"));
        }
        let lat = (latitude.floor() as i32).min(89);
        let mut lon = longitude.floor() as i32;
        if lon == 180 {
            lon = -180;
        }
        let ns = if lat >= 0 { 'N' } else { 'S' };
        let ew = if lon >= 0 { 'E' } else { 'W' };
        Ok(Area(format!("{ns}{}{ew}{}", lat.abs(), lon.abs())))
    }

    /// Parse an area key, with the same rules as the server.
    pub fn parse(key: &str) -> Result<Area, String> {
        let bad = || format!("{key:?} is not a one-degree square such as \"N51W1\"");
        let split = key.find(['E', 'W']).ok_or_else(bad)?;
        let (lat, lon) = key.split_at(split);
        let lat_ok = match lat.split_at_checked(1) {
            Some(("N", n)) => number(n).is_some_and(|v| v <= 89),
            Some(("S", n)) => number(n).is_some_and(|v| (1..=90).contains(&v)),
            _ => false,
        };
        let lon_ok = match lon.split_at(1) {
            ("E", n) => number(n).is_some_and(|v| v <= 179),
            (_, n) => number(n).is_some_and(|v| (1..=180).contains(&v)),
        };
        if lat_ok && lon_ok {
            Ok(Area(key.to_string()))
        } else {
            Err(bad())
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Area {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Whole number with no sign and no leading zero.
fn number(s: &str) -> Option<u32> {
    let ok = !s.is_empty()
        && s.len() <= 3
        && s.bytes().all(|b| b.is_ascii_digit())
        && (s == "0" || !s.starts_with('0'));
    ok.then(|| s.parse().ok()).flatten()
}

/// How the server numbers channels: channel `n` covers
/// `[low + n * width, low + (n + 1) * width)` kHz. The engine works in centre
/// frequencies, so slots are converted at this boundary only.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChannelGrid {
    /// Lower edge of channel 0, in kHz.
    pub low_khz: f64,
    /// Width of one channel, in kHz.
    pub width_khz: f64,
    /// Number of channels.
    pub channels: u32,
}

impl ChannelGrid {
    /// The 17 m relay segment, 18.111 to 18.120 MHz: three 2.7 kHz channels
    /// centred at 18112.35, 18115.05 and 18117.75 kHz. Must match
    /// `BAND_SEGMENT_KHZ` on the server.
    pub const RELAY_17M: ChannelGrid = ChannelGrid {
        low_khz: 18_111.0,
        width_khz: 2.7,
        channels: 3,
    };

    /// Centre frequency of a channel in kHz, or None when it is not on the grid.
    pub fn centre_khz(&self, channel: u32) -> Option<f32> {
        (channel < self.channels).then(|| (self.low_khz + self.width_khz * (f64::from(channel) + 0.5)) as f32)
    }

    /// Channel that holds a frequency in kHz. Works in tenths of a kHz, as the
    /// server does, so edges are exact.
    pub fn channel_of(&self, khz: f32) -> Option<u32> {
        let tenths = ((f64::from(khz) - self.low_khz) * 10.0).round();
        let width = (self.width_khz * 10.0).round();
        if !tenths.is_finite() || tenths < 0.0 {
            return None;
        }
        let channel = (tenths / width).floor() as u32;
        (channel < self.channels).then_some(channel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonical_json_sorts_keys_at_every_level() {
        let v = json!({"b": 1, "a": [{"z": true, "y": null}], "c": {"e": "x", "d": -2}});
        assert_eq!(
            canonical_json(&v),
            r#"{"a":[{"y":null,"z":true}],"b":1,"c":{"d":-2,"e":"x"}}"#
        );
    }

    #[test]
    fn canonical_json_escapes_strings_like_json_stringify() {
        assert_eq!(canonical_json(&json!({"k": "a\"b\\c\n"})), r#"{"k":"a\"b\\c\n"}"#);
    }

    #[test]
    fn area_from_position_matches_the_server() {
        let cases = [
            ((51.5, -0.1), "N51W1"),
            ((54.68, 25.28), "N54E25"),
            ((-33.9, 18.4), "S34E18"),
            ((0.0, 0.0), "N0E0"),
            ((-0.5, -0.5), "S1W1"),
            ((90.0, 180.0), "N89W180"),
            ((-90.0, -180.0), "S90W180"),
        ];
        for ((lat, lon), want) in cases {
            let area = Area::from_position(lat, lon).unwrap();
            assert_eq!(area.as_str(), want, "{lat}, {lon}");
            assert_eq!(Area::parse(want).unwrap(), area, "parse {want}");
        }
        assert!(Area::from_position(91.0, 0.0).is_err());
        assert!(Area::from_position(0.0, f64::NAN).is_err());
    }

    #[test]
    fn area_parse_refuses_what_the_server_refuses() {
        for bad in [
            "", "N51", "W1", "N90E0", "S0E0", "N05E1", "N1E180", "N1W0", "n51w1", "N51W1 ", "N-1W1", "X1W1",
        ] {
            assert!(Area::parse(bad).is_err(), "{bad:?} should be refused");
        }
        for good in ["N0E0", "S90W180", "N89E179", "S1W1"] {
            assert!(Area::parse(good).is_ok(), "{good:?} should be accepted");
        }
    }

    #[test]
    fn grid_maps_channels_to_centres_and_back() {
        let g = ChannelGrid::RELAY_17M;
        let centres: Vec<f32> = (0..3).map(|c| g.centre_khz(c).unwrap()).collect();
        for (got, want) in centres.iter().zip([18_112.35_f32, 18_115.05, 18_117.75]) {
            assert!((got - want).abs() < 0.001, "{got} vs {want}");
        }
        for (c, khz) in centres.iter().enumerate() {
            assert_eq!(g.channel_of(*khz), Some(c as u32));
        }
        assert_eq!(g.centre_khz(3), None);
        assert_eq!(g.channel_of(18_111.0), Some(0), "lower edge is included");
        assert_eq!(g.channel_of(18_120.0), None, "upper edge is not");
        assert_eq!(g.channel_of(18_110.9), None);
        assert_eq!(g.channel_of(f32::NAN), None);
    }
}
