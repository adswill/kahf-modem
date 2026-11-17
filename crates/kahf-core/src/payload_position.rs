//! Position beacon payload: callsign, latitude, longitude and optional altitude
//! in one fixed bit layout of at most 16 bytes.
//!
//! Layout, MSB first, bit-packed, zero padding to a whole byte at the end:
//!   flags      8 bits : bit 0 = altitude present; bits 1..=3 reserved (must be 0);
//!                       bits 4..=7 = callsign length (1..=10)
//!   callsign  60 bits : 10 slots of 6 bits, unused trailing slots are 0
//!                       (0..=25 = A..Z, 26..=35 = 0..9, 36 = '/')
//!   latitude  21 bits : round(lat * 1e4) + 900000     (about 11 m per step)
//!   longitude 22 bits : round(lon * 1e4) + 1800000    (about 11 m per step at the equator)
//!   altitude  16 bits : alt_m + 32768, present only when the flag bit is set
//!
//! Size: 14 bytes without altitude, 16 bytes with altitude.

/// A position beacon: callsign, latitude and longitude in degrees, optional altitude in metres.
#[derive(Debug, Clone, PartialEq)]
pub struct Position {
    pub call: String,
    pub lat: f64,
    pub lon: f64,
    pub alt_m: Option<i32>,
}

const CALL_MAX: usize = 10;
const CALL_BITS: usize = 6 * CALL_MAX;
const LAT_BITS: usize = 21;
const LON_BITS: usize = 22;
const ALT_BITS: usize = 16;
const LAT_OFFSET: i64 = 900_000;
const LON_OFFSET: i64 = 1_800_000;
const ALT_OFFSET: i64 = 32_768;

fn call_code(c: u8) -> Option<u64> {
    match c {
        b'A'..=b'Z' => Some((c - b'A') as u64),
        b'0'..=b'9' => Some((c - b'0') as u64 + 26),
        b'/' => Some(36),
        _ => None,
    }
}

fn call_char(code: u64) -> Option<char> {
    match code {
        0..=25 => Some((b'A' + code as u8) as char),
        26..=35 => Some((b'0' + (code - 26) as u8) as char),
        36 => Some('/'),
        _ => None,
    }
}

/// Encode a position into a compact payload of 14 or 16 bytes.
pub fn encode(p: &Position) -> Result<Vec<u8>, String> {
    if !p.call.is_ascii() {
        return Err("callsign must be ASCII".to_string());
    }
    let call = p.call.as_bytes();
    if call.is_empty() || call.len() > CALL_MAX {
        return Err(format!("callsign must be 1..=10 characters, got {}", call.len()));
    }
    let mut packed: u64 = 0;
    for i in 0..CALL_MAX {
        let code = match call.get(i) {
            Some(&c) => call_code(c).ok_or_else(|| format!("invalid callsign character '{}'", c as char))?,
            None => 0,
        };
        packed = (packed << 6) | code;
    }
    if !(-90.0..=90.0).contains(&p.lat) {
        return Err(format!("latitude {} outside -90..=90", p.lat));
    }
    if !(-180.0..=180.0).contains(&p.lon) {
        return Err(format!("longitude {} outside -180..=180", p.lon));
    }
    let lat_raw = ((p.lat * 1e4).round() as i64 + LAT_OFFSET) as u64;
    let lon_raw = ((p.lon * 1e4).round() as i64 + LON_OFFSET) as u64;
    let alt_raw = match p.alt_m {
        Some(a) => {
            if !(-32_768..=32_767).contains(&a) {
                return Err(format!("altitude {} m outside -32768..=32767", a));
            }
            Some((a as i64 + ALT_OFFSET) as u64)
        }
        None => None,
    };

    let flags = ((call.len() as u64) << 4) | u64::from(alt_raw.is_some());
    let mut acc: u128 = 0;
    let mut nbits: usize = 0;
    let mut push = |value: u64, width: usize| {
        acc = (acc << width) | (u128::from(value) & ((1u128 << width) - 1));
        nbits += width;
    };
    push(flags, 8);
    push(packed, CALL_BITS);
    push(lat_raw, LAT_BITS);
    push(lon_raw, LON_BITS);
    if let Some(a) = alt_raw {
        push(a, ALT_BITS);
    }

    let nbytes = nbits.div_ceil(8);
    acc <<= nbytes * 8 - nbits;
    Ok((0..nbytes)
        .map(|i| (acc >> (8 * (nbytes - 1 - i))) as u8)
        .collect())
}

/// Decode a payload produced by `encode`. Returns None for any malformed input; never panics.
pub fn decode(bytes: &[u8]) -> Option<Position> {
    let first = *bytes.first()?;
    let call_len = (first >> 4) as usize;
    let has_alt = first & 1 == 1;
    if first & 0x0E != 0 || call_len == 0 || call_len > CALL_MAX {
        return None;
    }
    let nbits = 8 + CALL_BITS + LAT_BITS + LON_BITS + if has_alt { ALT_BITS } else { 0 };
    let nbytes = nbits.div_ceil(8);
    if bytes.len() != nbytes {
        return None;
    }
    let mut acc: u128 = 0;
    for &b in bytes {
        acc = (acc << 8) | u128::from(b);
    }
    acc >>= nbytes * 8 - nbits;

    let mut rem = nbits;
    let mut take = |width: usize| -> u64 {
        rem -= width;
        ((acc >> rem) & ((1u128 << width) - 1)) as u64
    };
    let _flags = take(8);
    let packed = take(CALL_BITS);
    let mut call = String::with_capacity(call_len);
    for i in 0..CALL_MAX {
        let code = (packed >> (6 * (CALL_MAX - 1 - i))) & 0x3F;
        if i < call_len {
            call.push(call_char(code)?);
        } else if code != 0 {
            return None;
        }
    }
    let lat_raw = take(LAT_BITS) as i64;
    if lat_raw > 2 * LAT_OFFSET {
        return None;
    }
    let lon_raw = take(LON_BITS) as i64;
    if lon_raw > 2 * LON_OFFSET {
        return None;
    }
    let alt_m = if has_alt {
        Some((take(ALT_BITS) as i64 - ALT_OFFSET) as i32)
    } else {
        None
    };
    Some(Position {
        call,
        lat: (lat_raw - LAT_OFFSET) as f64 / 1e4,
        lon: (lon_raw - LON_OFFSET) as f64 / 1e4,
        alt_m,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOL: f64 = 0.00005 + 1e-9;

    fn pos(call: &str, lat: f64, lon: f64, alt_m: Option<i32>) -> Position {
        Position {
            call: call.to_string(),
            lat,
            lon,
            alt_m,
        }
    }

    #[test]
    fn round_trip_without_altitude() {
        let p = pos("G4ABC/P", 51.5074, -0.1278, None);
        let bytes = encode(&p).unwrap();
        assert_eq!(bytes.len(), 14);
        let d = decode(&bytes).unwrap();
        assert_eq!(d.call, "G4ABC/P");
        assert!((d.lat - 51.5074).abs() <= TOL);
        assert!((d.lon - (-0.1278)).abs() <= TOL);
        assert_eq!(d.alt_m, None);
    }

    #[test]
    fn round_trip_with_altitude_and_edges() {
        let cases = [
            pos("AB1CDE/M", -33.8688, 151.2093, Some(-120)),
            pos("ABCDEFGHIJ", 90.0, -180.0, Some(32_767)),
            pos("0123456789", -90.0, 180.0, Some(-32_768)),
            pos("X", 0.0, 0.0, Some(0)),
        ];
        for p in cases {
            let bytes = encode(&p).unwrap();
            let d = decode(&bytes).unwrap();
            assert_eq!(d.call, p.call);
            assert!((d.lat - p.lat).abs() <= TOL, "lat {} -> {}", p.lat, d.lat);
            assert!((d.lon - p.lon).abs() <= TOL, "lon {} -> {}", p.lon, d.lon);
            assert_eq!(d.alt_m, p.alt_m);
        }
    }

    #[test]
    fn size_is_within_16_bytes() {
        let full = pos("ABCDEFGHIJ", 12.3456, 98.7654, Some(1234));
        let n = encode(&full).unwrap().len();
        assert_eq!(n, 16);
        assert!(n <= 16);
        let short = pos("A", 1.0, 1.0, None);
        assert!(encode(&short).unwrap().len() <= 16);
    }

    #[test]
    fn invalid_latitude_rejected() {
        assert!(encode(&pos("A1", 90.0001, 0.0, None)).is_err());
        assert!(encode(&pos("A1", -90.5, 0.0, None)).is_err());
        assert!(encode(&pos("A1", f64::NAN, 0.0, None)).is_err());
    }

    #[test]
    fn invalid_longitude_and_altitude_rejected() {
        assert!(encode(&pos("A1", 0.0, 180.1, None)).is_err());
        assert!(encode(&pos("A1", 0.0, 0.0, Some(32_768))).is_err());
        assert!(encode(&pos("A1", 0.0, 0.0, Some(-32_769))).is_err());
    }

    #[test]
    fn invalid_callsign_rejected() {
        assert!(encode(&pos("", 0.0, 0.0, None)).is_err());
        assert!(encode(&pos("ABCDEFGHIJK", 0.0, 0.0, None)).is_err());
        assert!(encode(&pos("ab1", 0.0, 0.0, None)).is_err());
        assert!(encode(&pos("A B", 0.0, 0.0, None)).is_err());
        assert!(encode(&pos("A-1", 0.0, 0.0, None)).is_err());
    }

    #[test]
    fn decode_rejects_malformed_layout() {
        let mut bytes = encode(&pos("AB1", 10.0, 20.0, None)).unwrap();
        bytes[0] |= 0x02; // reserved flag bit
        assert!(decode(&bytes).is_none());
        let bytes = encode(&pos("AB1", 10.0, 20.0, None)).unwrap();
        assert!(decode(&bytes[..13]).is_none());
        assert!(decode(&[]).is_none());
    }

    #[test]
    fn random_short_input_returns_none_without_panic() {
        assert!(decode(&[0x00, 0x00, 0x00]).is_none());
        assert!(decode(&[0xFF, 0xFF, 0xFF]).is_none());
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        for _ in 0..200_000 {
            let mut b = [0u8; 3];
            for x in b.iter_mut() {
                seed = seed
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                *x = (seed >> 56) as u8;
            }
            assert!(decode(&b).is_none());
        }
    }
}
