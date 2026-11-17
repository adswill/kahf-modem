//! Short weather report payload.
//!
//! Layout:
//! - byte 0: presence flags. Bit 0 temperature, 1 humidity, 2 pressure,
//!   3 wind speed, 4 wind direction, 5 rain. Bits 6 and 7 must be zero.
//! - then a packed bit stream, most significant bit first:
//!   - callsign length: 4 bits (3 to 9 characters)
//!   - callsign characters: 6 bits each, indexed into `CALL_CHARS`
//!   - temperature: 11 bits, tenths of a degree plus 600 (only if present)
//!   - humidity: 7 bits, percent
//!   - pressure: 9 bits, hPa minus 800
//!   - wind speed: 7 bits, m/s
//!   - wind direction: 9 bits, degrees
//!   - rain: 9 bits, mm
//!
//! The bit stream is zero padded to a whole byte. A 6 character callsign with
//! every field filled takes 13 bytes; the largest callsign takes 15.

/// Callsign alphabet. Index in this table is the 6-bit code.
const CALL_CHARS: &[u8; 38] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ/-";

const FLAG_TEMP: u8 = 1 << 0;
const FLAG_HUMIDITY: u8 = 1 << 1;
const FLAG_PRESSURE: u8 = 1 << 2;
const FLAG_WIND_SPEED: u8 = 1 << 3;
const FLAG_WIND_DIR: u8 = 1 << 4;
const FLAG_RAIN: u8 = 1 << 5;
const FLAG_RESERVED: u8 = 0xC0;

/// One weather report. Only `call` is required.
#[derive(Debug, Clone, PartialEq)]
pub struct Weather {
    pub call: String,
    /// Air temperature in degrees Celsius, one decimal place, -60.0 to 60.0.
    pub temp_c: Option<f32>,
    /// Relative humidity in percent, 0 to 100.
    pub humidity: Option<u8>,
    /// Sea level pressure in hPa, 800 to 1100.
    pub pressure_hpa: Option<u16>,
    /// Wind speed in m/s, 0 to 100.
    pub wind_ms: Option<u8>,
    /// Wind direction in degrees, 0 to 359.
    pub wind_dir: Option<u16>,
    /// Rain in the last hour in mm, 0 to 300.
    pub rain_mm: Option<u16>,
}

/// Writes bits, most significant first.
struct BitWriter {
    bytes: Vec<u8>,
    used: usize,
}

impl BitWriter {
    fn new() -> Self {
        BitWriter {
            bytes: Vec::new(),
            used: 0,
        }
    }

    fn put(&mut self, value: u32, width: u32) {
        for i in (0..width).rev() {
            if self.used.is_multiple_of(8) {
                self.bytes.push(0);
            }
            if (value >> i) & 1 == 1 {
                let idx = self.used / 8;
                self.bytes[idx] |= 0x80 >> (self.used % 8);
            }
            self.used += 1;
        }
    }
}

/// Reads bits, most significant first. Every read is bounds checked.
struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0 }
    }

    fn get(&mut self, width: u32) -> Option<u32> {
        let mut value = 0u32;
        for _ in 0..width {
            let byte = *self.data.get(self.pos / 8)?;
            let bit = (byte >> (7 - self.pos % 8)) & 1;
            value = (value << 1) | u32::from(bit);
            self.pos += 1;
        }
        Some(value)
    }
}

/// Encodes a weather report. Returns a plain-language error if a value is out of range.
pub fn encode(w: &Weather) -> Result<Vec<u8>, String> {
    if !w.call.is_ascii() {
        return Err("callsign must use only letters, digits, / or -".to_string());
    }
    let call = w.call.to_ascii_uppercase();
    if !(3..=9).contains(&call.len()) {
        return Err("callsign must be 3 to 9 characters long".to_string());
    }

    let mut bits = BitWriter::new();
    bits.put(call.len() as u32, 4);
    for ch in call.bytes() {
        match CALL_CHARS.iter().position(|&a| a == ch) {
            Some(code) => bits.put(code as u32, 6),
            None => {
                return Err(format!(
                    "callsign character '{}' is not allowed (use letters, digits, / or -)",
                    ch as char
                ))
            }
        }
    }

    let mut flags = 0u8;

    if let Some(t) = w.temp_c {
        if !(-60.0..=60.0).contains(&t) {
            return Err(format!(
                "temperature {t} C is out of range; it must be between -60.0 and 60.0 C"
            ));
        }
        let tenths = (t * 10.0).round() as i32 + 600;
        bits.put(tenths as u32, 11);
        flags |= FLAG_TEMP;
    }
    if let Some(h) = w.humidity {
        if h > 100 {
            return Err(format!(
                "humidity {h}% is out of range; it must be between 0 and 100%"
            ));
        }
        bits.put(u32::from(h), 7);
        flags |= FLAG_HUMIDITY;
    }
    if let Some(p) = w.pressure_hpa {
        if !(800..=1100).contains(&p) {
            return Err(format!(
                "pressure {p} hPa is out of range; it must be between 800 and 1100 hPa"
            ));
        }
        bits.put(u32::from(p - 800), 9);
        flags |= FLAG_PRESSURE;
    }
    if let Some(v) = w.wind_ms {
        if v > 100 {
            return Err(format!(
                "wind speed {v} m/s is out of range; it must be between 0 and 100 m/s"
            ));
        }
        bits.put(u32::from(v), 7);
        flags |= FLAG_WIND_SPEED;
    }
    if let Some(d) = w.wind_dir {
        if d > 359 {
            return Err(format!(
                "wind direction {d} degrees is out of range; it must be between 0 and 359"
            ));
        }
        bits.put(u32::from(d), 9);
        flags |= FLAG_WIND_DIR;
    }
    if let Some(r) = w.rain_mm {
        if r > 300 {
            return Err(format!(
                "rain {r} mm is out of range; it must be between 0 and 300 mm"
            ));
        }
        bits.put(u32::from(r), 9);
        flags |= FLAG_RAIN;
    }

    let mut out = Vec::with_capacity(1 + bits.bytes.len());
    out.push(flags);
    out.extend_from_slice(&bits.bytes);
    Ok(out)
}

/// Decodes a weather report. Returns None for any malformed input; never panics.
pub fn decode(bytes: &[u8]) -> Option<Weather> {
    let (&flags, body) = bytes.split_first()?;
    if flags & FLAG_RESERVED != 0 {
        return None;
    }

    let mut r = BitReader::new(body);
    let len = r.get(4)? as usize;
    if !(3..=9).contains(&len) {
        return None;
    }
    let mut call = String::with_capacity(len);
    for _ in 0..len {
        let code = r.get(6)? as usize;
        call.push(char::from(*CALL_CHARS.get(code)?));
    }

    let mut w = Weather {
        call,
        temp_c: None,
        humidity: None,
        pressure_hpa: None,
        wind_ms: None,
        wind_dir: None,
        rain_mm: None,
    };

    if flags & FLAG_TEMP != 0 {
        let t = r.get(11)? as i32;
        if t > 1200 {
            return None;
        }
        w.temp_c = Some((t - 600) as f32 / 10.0);
    }
    if flags & FLAG_HUMIDITY != 0 {
        let h = r.get(7)?;
        if h > 100 {
            return None;
        }
        w.humidity = Some(h as u8);
    }
    if flags & FLAG_PRESSURE != 0 {
        let p = r.get(9)?;
        if p > 300 {
            return None;
        }
        w.pressure_hpa = Some(800 + p as u16);
    }
    if flags & FLAG_WIND_SPEED != 0 {
        let v = r.get(7)?;
        if v > 100 {
            return None;
        }
        w.wind_ms = Some(v as u8);
    }
    if flags & FLAG_WIND_DIR != 0 {
        let d = r.get(9)?;
        if d > 359 {
            return None;
        }
        w.wind_dir = Some(d as u16);
    }
    if flags & FLAG_RAIN != 0 {
        let rain = r.get(9)?;
        if rain > 300 {
            return None;
        }
        w.rain_mm = Some(rain as u16);
    }

    // Exactly the bytes the bit stream needs, no trailing bytes.
    if body.len() != r.pos.div_ceil(8) {
        return None;
    }
    Some(w)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> Weather {
        Weather {
            call: "KA1XYZ".to_string(),
            temp_c: Some(-12.3),
            humidity: Some(87),
            pressure_hpa: Some(1013),
            wind_ms: Some(12),
            wind_dir: Some(359),
            rain_mm: Some(300),
        }
    }

    #[test]
    fn round_trip_all_fields() {
        let w = full();
        let bytes = encode(&w).unwrap();
        assert_eq!(bytes.len(), 13);
        assert!(bytes.len() < 16);
        assert_eq!(decode(&bytes), Some(w));
    }

    #[test]
    fn round_trip_longest_callsign_all_fields() {
        let w = Weather {
            call: "VK2ABC/QR".to_string(),
            ..full()
        };
        let bytes = encode(&w).unwrap();
        assert_eq!(bytes.len(), 15);
        assert_eq!(decode(&bytes), Some(w));
    }

    #[test]
    fn round_trip_only_temperature() {
        let w = Weather {
            call: "W1AW".to_string(),
            temp_c: Some(21.5),
            humidity: None,
            pressure_hpa: None,
            wind_ms: None,
            wind_dir: None,
            rain_mm: None,
        };
        let bytes = encode(&w).unwrap();
        assert_eq!(decode(&bytes), Some(w));
    }

    #[test]
    fn round_trip_temperature_limits() {
        for t in [-60.0f32, 60.0, 0.0, -0.1] {
            let w = Weather {
                temp_c: Some(t),
                ..full()
            };
            assert_eq!(decode(&encode(&w).unwrap()), Some(w));
        }
    }

    #[test]
    fn out_of_range_rejected() {
        let base = || Weather {
            call: "K1ABC".to_string(),
            temp_c: None,
            humidity: None,
            pressure_hpa: None,
            wind_ms: None,
            wind_dir: None,
            rain_mm: None,
        };
        let cases = [
            Weather {
                temp_c: Some(60.1),
                ..base()
            },
            Weather {
                temp_c: Some(-60.1),
                ..base()
            },
            Weather {
                temp_c: Some(f32::NAN),
                ..base()
            },
            Weather {
                humidity: Some(101),
                ..base()
            },
            Weather {
                pressure_hpa: Some(799),
                ..base()
            },
            Weather {
                pressure_hpa: Some(1101),
                ..base()
            },
            Weather {
                wind_ms: Some(101),
                ..base()
            },
            Weather {
                wind_dir: Some(360),
                ..base()
            },
            Weather {
                rain_mm: Some(301),
                ..base()
            },
            Weather {
                call: "AB".to_string(),
                ..base()
            },
            Weather {
                call: "ABCDEFGHIJ".to_string(),
                ..base()
            },
            Weather {
                call: "K1 ABC".to_string(),
                ..base()
            },
            Weather {
                call: "K1ÅBC".to_string(),
                ..base()
            },
        ];
        for w in cases {
            let err = encode(&w).expect_err("should be rejected");
            assert!(!err.is_empty());
        }
    }

    #[test]
    fn decode_rejects_bad_input() {
        assert_eq!(decode(&[]), None);
        assert_eq!(decode(&[0xC0, 0x30]), None);
        let mut bytes = encode(&full()).unwrap();
        bytes.push(0);
        assert_eq!(decode(&bytes), None);
    }

    #[test]
    fn decode_never_panics_on_random_bytes() {
        // Small deterministic LCG so the test needs no dependencies.
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as u8
        };
        for _ in 0..20_000 {
            let len = (next() % 24) as usize;
            let buf: Vec<u8> = (0..len).map(|_| next()).collect();
            let _ = decode(&buf);
        }
        for flags in 0..=255u8 {
            let mut buf = vec![flags];
            buf.extend([0xFF; 16]);
            let _ = decode(&buf);
        }
    }
}
