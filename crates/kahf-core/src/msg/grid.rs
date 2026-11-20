//! Maidenhead grid locators and the grid beacon payload.
//!
//! A station with a GPS position beacons `payload_position` (about 11 m); receivers derive the
//! grid from it with [`to_grid`]. A station without GPS that only knows its grid beacons a
//! grid payload instead, so the heard list, the contacts book and the 17 m route planner still
//! learn where it is without anyone typing it in.
//!
//! Grid payload: byte 0 is `TYPE_GRID4` (0xA5) or `TYPE_GRID6` (0xA7), then the 4 or 6 character
//! locator, then the callsign (A-Z, 0-9, '/', 1 to 15 characters). Both type bytes have a
//! non-zero reserved nibble pattern, so `payload_position::decode` rejects them.

pub const TYPE_GRID4: u8 = 0xA5;
pub const TYPE_GRID6: u8 = 0xA7;
const MAX_CALL: usize = 15;

/// Locator of `(lat, lon)` with 4 or 6 characters (anything else becomes 6). Upper case field
/// and subsquare letters like "IO91wm" are written "IO91WM".
pub fn to_grid(lat: f64, lon: f64, chars: usize) -> Option<String> {
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return None;
    }
    let lon = (lon + 180.0).min(359.999_999);
    let lat = (lat + 90.0).min(179.999_999);
    let (f_lon, f_lat) = ((lon / 20.0) as u8, (lat / 10.0) as u8);
    let (rl, rt) = (lon - f_lon as f64 * 20.0, lat - f_lat as f64 * 10.0);
    let (s_lon, s_lat) = ((rl / 2.0) as u8, rt as u8);
    let (rl, rt) = (rl - s_lon as f64 * 2.0, rt - s_lat as f64);
    let (u_lon, u_lat) = ((rl * 12.0) as u8, (rt * 24.0) as u8);
    let mut s = String::new();
    s.push((b'A' + f_lon) as char);
    s.push((b'A' + f_lat) as char);
    s.push((b'0' + s_lon) as char);
    s.push((b'0' + s_lat) as char);
    if chars != 4 {
        s.push((b'A' + u_lon.min(23)) as char);
        s.push((b'A' + u_lat.min(23)) as char);
    }
    Some(s)
}

/// True for a well-formed 4 or 6 character locator (case insensitive).
pub fn valid(g: &str) -> bool {
    let b = g.as_bytes();
    (b.len() == 4 || b.len() == 6)
        && matches!(b[0].to_ascii_uppercase(), b'A'..=b'R')
        && matches!(b[1].to_ascii_uppercase(), b'A'..=b'R')
        && b[2].is_ascii_digit()
        && b[3].is_ascii_digit()
        && (b.len() == 4
            || (matches!(b[4].to_ascii_uppercase(), b'A'..=b'X')
                && matches!(b[5].to_ascii_uppercase(), b'A'..=b'X')))
}

/// Centre `(lat, lon)` of a locator.
pub fn from_grid(g: &str) -> Option<(f64, f64)> {
    if !valid(g) {
        return None;
    }
    let b: Vec<u8> = g.bytes().map(|c| c.to_ascii_uppercase()).collect();
    let mut lon = (b[0] - b'A') as f64 * 20.0 + (b[2] - b'0') as f64 * 2.0 - 180.0;
    let mut lat = (b[1] - b'A') as f64 * 10.0 + (b[3] - b'0') as f64 - 90.0;
    if b.len() == 6 {
        lon += (b[4] - b'A') as f64 / 12.0 + 1.0 / 24.0;
        lat += (b[5] - b'A') as f64 / 24.0 + 1.0 / 48.0;
    } else {
        lon += 1.0;
        lat += 0.5;
    }
    Some((lat, lon))
}

fn call_ok(c: &str) -> bool {
    !c.is_empty()
        && c.len() <= MAX_CALL
        && c.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'/')
}

/// Builds a grid beacon payload.
pub fn encode_grid(call: &str, grid: &str) -> Result<Vec<u8>, String> {
    if !valid(grid) {
        return Err(format!("invalid grid locator: {grid:?}"));
    }
    let call = call.to_ascii_uppercase();
    if !call_ok(&call) {
        return Err(format!("invalid callsign: {call:?}"));
    }
    let grid = grid.to_ascii_uppercase();
    let mut out = vec![if grid.len() == 4 { TYPE_GRID4 } else { TYPE_GRID6 }];
    out.extend_from_slice(grid.as_bytes());
    out.extend_from_slice(call.as_bytes());
    Ok(out)
}

/// Decodes a grid beacon into (callsign, grid). None for anything else.
pub fn decode_grid(bytes: &[u8]) -> Option<(String, String)> {
    let n = match *bytes.first()? {
        TYPE_GRID4 => 4,
        TYPE_GRID6 => 6,
        _ => return None,
    };
    if bytes.len() < 1 + n + 1 {
        return None;
    }
    let grid = std::str::from_utf8(&bytes[1..1 + n]).ok()?;
    let call = std::str::from_utf8(&bytes[1 + n..]).ok()?;
    (valid(grid) && call_ok(call)).then(|| (call.to_string(), grid.to_string()))
}

/// What a station beacons for its location: GPS position when known, else its configured grid.
pub fn beacon_payload(call: &str, pos: Option<(f64, f64)>, grid: Option<&str>) -> Option<Vec<u8>> {
    if let Some((lat, lon)) = pos {
        return crate::payload_position::encode(&crate::payload_position::Position {
            call: call.to_ascii_uppercase(),
            lat,
            lon,
            alt_m: None,
        })
        .ok();
    }
    encode_grid(call, grid?).ok()
}

/// (callsign, grid, exact position when the beacon carried one).
pub type Learned = (String, String, Option<(f64, f64)>);

/// Learns (callsign, grid) from any location beacon payload: a grid beacon, or a position
/// beacon whose position is converted to a 6 character locator, along with the position.
pub fn learn(bytes: &[u8]) -> Option<Learned> {
    if let Some((call, g)) = decode_grid(bytes) {
        let g = g.to_ascii_uppercase();
        return Some((call, g, None));
    }
    let p = crate::payload_position::decode(bytes)?;
    let g = to_grid(p.lat, p.lon, 6)?;
    Some((p.call, g, Some((p.lat, p.lon))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_locators() {
        assert_eq!(to_grid(51.5, -0.12, 6).unwrap(), "IO91WM");
        assert_eq!(to_grid(51.5, -0.12, 4).unwrap(), "IO91");
        assert_eq!(to_grid(-34.9, 138.6, 6).unwrap(), "PF95HC");
        assert_eq!(to_grid(90.0, 180.0, 6).unwrap().len(), 6);
        assert!(to_grid(91.0, 0.0, 6).is_none());
    }

    #[test]
    fn round_trip_within_cell() {
        for (la, lo) in [(51.5, -0.12), (-34.9, 138.6), (40.7, -74.0), (0.1, 0.1)] {
            let g = to_grid(la, lo, 6).unwrap();
            let (a, b) = from_grid(&g).unwrap();
            assert!(
                (a - la).abs() < 1.0 / 24.0 && (b - lo).abs() < 1.0 / 12.0,
                "{g} {a} {b}"
            );
            assert_eq!(to_grid(a, b, 6).unwrap(), g);
        }
        assert!(from_grid("ZZ99").is_none() && from_grid("IO9").is_none() && !valid("IO91ZZ"));
        assert_eq!(from_grid("io91").unwrap(), (51.5, -1.0));
    }

    #[test]
    fn payloads() {
        let p = encode_grid("g4abc", "io91wm").unwrap();
        assert_eq!(decode_grid(&p), Some(("G4ABC".into(), "IO91WM".into())));
        let p4 = encode_grid("G4ABC", "IO91").unwrap();
        assert_eq!(p4[0], TYPE_GRID4);
        assert_eq!(decode_grid(&p4), Some(("G4ABC".into(), "IO91".into())));
        assert!(
            crate::payload_position::decode(&p).is_none(),
            "never mistaken for a position"
        );
        assert!(encode_grid("G4ABC", "bad").is_err() && encode_grid("g 4", "IO91").is_err());
        assert_eq!(decode_grid(&[TYPE_GRID6, b'I']), None);
    }

    #[test]
    fn beacon_prefers_position_and_learn_handles_both() {
        let gps = beacon_payload("G4ABC", Some((51.5, -0.12)), Some("JJ00")).unwrap();
        let (c, g, pos) = learn(&gps).unwrap();
        assert_eq!((c.as_str(), g.as_str()), ("G4ABC", "IO91WM"));
        assert!(pos.is_some());
        let manual = beacon_payload("G4ABC", None, Some("io91")).unwrap();
        assert_eq!(learn(&manual), Some(("G4ABC".into(), "IO91".into(), None)));
        assert!(beacon_payload("G4ABC", None, None).is_none());
        assert!(learn(b"junk").is_none());
    }
}
