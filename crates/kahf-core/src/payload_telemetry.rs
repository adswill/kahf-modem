//! Telemetry payload for remote stations: a callsign, a sequence number and up to
//! [`MAX_READINGS`] sensor readings. Pure encode and decode, no I/O.
//!
//! Wire layout (multi-byte fields big endian):
//! - `u8` callsign length, then the callsign bytes (UTF-8)
//! - `u16` sequence number
//! - `u8` reading count (0..=8)
//! - per reading, 4 bytes:
//!   - `u8` sensor id
//!   - `u8` high nibble = unit code, low nibble = scale exponent (-8..=7, two's complement)
//!   - `i16` value, which must fit in 16 bits

/// Maximum number of readings in one payload.
pub const MAX_READINGS: usize = 8;

const READING_LEN: usize = 4;
const SCALE_MIN: i8 = -8;
const SCALE_MAX: i8 = 7;

/// Physical unit of a reading. Wire codes are 0..=5 in declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Volts,
    Celsius,
    Hpa,
    Percent,
    Metres,
    Count,
}

impl Unit {
    fn code(self) -> u8 {
        match self {
            Unit::Volts => 0,
            Unit::Celsius => 1,
            Unit::Hpa => 2,
            Unit::Percent => 3,
            Unit::Metres => 4,
            Unit::Count => 5,
        }
    }

    fn from_code(code: u8) -> Option<Unit> {
        match code {
            0 => Some(Unit::Volts),
            1 => Some(Unit::Celsius),
            2 => Some(Unit::Hpa),
            3 => Some(Unit::Percent),
            4 => Some(Unit::Metres),
            5 => Some(Unit::Count),
            _ => None,
        }
    }
}

/// One sensor reading. The real value is `value * 10^scale_exp`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    pub sensor_id: u8,
    pub value: i32,
    pub scale_exp: i8,
    pub unit: Unit,
}

/// A telemetry payload from one station.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Telemetry {
    pub call: String,
    pub seq: u16,
    pub readings: Vec<Reading>,
}

/// Encode a telemetry payload.
///
/// Errors when there are more than [`MAX_READINGS`] readings, when the callsign is longer
/// than 255 bytes, when a scale exponent is outside -8..=7, or when a value does not fit
/// in 16 bits.
pub fn encode(t: &Telemetry) -> Result<Vec<u8>, String> {
    let call = t.call.as_bytes();
    if call.len() > usize::from(u8::MAX) {
        return Err(format!("callsign too long: {} bytes", call.len()));
    }
    if t.readings.len() > MAX_READINGS {
        return Err(format!(
            "too many readings: {} (max {MAX_READINGS})",
            t.readings.len()
        ));
    }
    let mut out = Vec::with_capacity(1 + call.len() + 3 + READING_LEN * t.readings.len());
    out.push(call.len() as u8);
    out.extend_from_slice(call);
    out.extend_from_slice(&t.seq.to_be_bytes());
    out.push(t.readings.len() as u8);
    for (i, r) in t.readings.iter().enumerate() {
        if !(SCALE_MIN..=SCALE_MAX).contains(&r.scale_exp) {
            return Err(format!(
                "reading {i}: scale exponent {} outside {SCALE_MIN}..={SCALE_MAX}",
                r.scale_exp
            ));
        }
        let value = i16::try_from(r.value)
            .map_err(|_| format!("reading {i}: value {} does not fit in 16 bits", r.value))?;
        out.push(r.sensor_id);
        out.push((r.unit.code() << 4) | (r.scale_exp as u8 & 0x0F));
        out.extend_from_slice(&value.to_be_bytes());
    }
    Ok(out)
}

/// Decode a telemetry payload. Returns `None` for any malformed input: truncated or
/// trailing bytes, invalid UTF-8 callsign, more than [`MAX_READINGS`] readings, or an
/// unknown unit code. Never panics.
pub fn decode(bytes: &[u8]) -> Option<Telemetry> {
    let (&call_len, rest) = bytes.split_first()?;
    let call_len = usize::from(call_len);
    if rest.len() < call_len {
        return None;
    }
    let call = std::str::from_utf8(&rest[..call_len]).ok()?.to_string();
    let rest = &rest[call_len..];
    if rest.len() < 3 {
        return None;
    }
    let seq = u16::from_be_bytes([rest[0], rest[1]]);
    let count = usize::from(rest[2]);
    if count > MAX_READINGS {
        return None;
    }
    let body = &rest[3..];
    if body.len() != count * READING_LEN {
        return None;
    }
    let readings = body
        .as_chunks::<READING_LEN>()
        .0
        .iter()
        .map(|c| {
            let unit = Unit::from_code(c[1] >> 4)?;
            // Sign-extend the 4-bit scale exponent.
            let scale_exp = (((c[1] & 0x0F) << 4) as i8) >> 4;
            Some(Reading {
                sensor_id: c[0],
                value: i32::from(i16::from_be_bytes([c[2], c[3]])),
                scale_exp,
                unit,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Telemetry { call, seq, readings })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Telemetry {
        Telemetry {
            call: "N0CALL".to_string(),
            seq: 0xBEEF,
            readings: vec![
                Reading {
                    sensor_id: 1,
                    value: 1234,
                    scale_exp: -2,
                    unit: Unit::Volts,
                },
                Reading {
                    sensor_id: 2,
                    value: -15,
                    scale_exp: 0,
                    unit: Unit::Celsius,
                },
                Reading {
                    sensor_id: 3,
                    value: 1013,
                    scale_exp: -1,
                    unit: Unit::Hpa,
                },
            ],
        }
    }

    #[test]
    fn round_trip_three_readings() {
        let t = sample();
        let bytes = encode(&t).expect("encode");
        // 1 + 6 callsign + 2 seq + 1 count + 3 * 4 readings
        assert_eq!(bytes.len(), 22);
        assert_eq!(decode(&bytes), Some(t));
    }

    #[test]
    fn scale_exponent_extremes_round_trip() {
        let mut t = sample();
        t.readings[0].scale_exp = -8;
        t.readings[1].scale_exp = 7;
        t.readings[2].value = i32::from(i16::MIN);
        let bytes = encode(&t).expect("encode");
        assert_eq!(decode(&bytes), Some(t));
    }

    #[test]
    fn too_many_readings_rejected() {
        let mut t = sample();
        t.readings = (0..9)
            .map(|i| Reading {
                sensor_id: i,
                value: 0,
                scale_exp: 0,
                unit: Unit::Count,
            })
            .collect();
        assert!(encode(&t).is_err());

        t.readings.truncate(MAX_READINGS);
        let bytes = encode(&t).expect("8 readings allowed");
        assert_eq!(decode(&bytes), Some(t));
    }

    #[test]
    fn value_out_of_16_bits_rejected() {
        let mut t = sample();
        t.readings[1].value = 40_000;
        assert!(encode(&t).is_err());
        t.readings[1].value = -40_000;
        assert!(encode(&t).is_err());
    }

    #[test]
    fn scale_exponent_out_of_range_rejected() {
        let mut t = sample();
        t.readings[0].scale_exp = 8;
        assert!(encode(&t).is_err());
        t.readings[0].scale_exp = -9;
        assert!(encode(&t).is_err());
    }

    #[test]
    fn bad_input_decodes_to_none() {
        assert_eq!(decode(&[]), None);

        let good = encode(&sample()).expect("encode");
        // Truncated anywhere.
        for n in 0..good.len() {
            assert_eq!(decode(&good[..n]), None, "prefix of length {n}");
        }
        // Trailing byte.
        let mut long = good.clone();
        long.push(0);
        assert_eq!(decode(&long), None);

        // Count above the maximum.
        let mut many = good.clone();
        many[1 + 6 + 2] = 9;
        assert_eq!(decode(&many), None);

        // Unknown unit code (6) in the first reading.
        let mut bad_unit = good.clone();
        bad_unit[1 + 6 + 3 + 1] = 6 << 4;
        assert_eq!(decode(&bad_unit), None);

        // Invalid UTF-8 callsign.
        let bad_utf8 = [2u8, 0xFF, 0xFE, 0, 0, 0];
        assert_eq!(decode(&bad_utf8), None);
    }

    #[test]
    fn random_bytes_never_panic() {
        // xorshift64 with a fixed seed.
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..2000 {
            let len = (next() % 80) as usize;
            let buf: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            let _ = decode(&buf);
        }
    }
}
