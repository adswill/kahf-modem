//! Reception report payload: what a station says it heard.
//!
//! Wire format (multi-byte integers little-endian):
//!   [0]          length byte: ((reporter_len - 3) << 3) | (heard_len - 3)
//!   [1..]        reporter callsign (ASCII)
//!   [..]         heard callsign (ASCII)
//!   [..+4]       freq_khz u32
//!   [..+2]       minute_utc u16
//!   [..+1]       level_db i8
//!   [..+1]       snr_db i8
//!   [..+1]       mode u8
//!
//! Size is 10 + reporter_len + heard_len bytes: 22 bytes for two 6-character
//! calls, 30 bytes at the maximum of two 10-character calls.

const MIN_CALL: usize = 3;
const MAX_CALL: usize = 10;
const MAX_MINUTE: u16 = 1439;
const MAX_DB: i8 = 40;
/// Length byte + freq (4) + minute (2) + level + snr + mode.
const FIXED_LEN: usize = 10;

/// One reception report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reception {
    pub reporter: String,
    pub heard: String,
    pub freq_khz: u32,
    pub minute_utc: u16,
    pub level_db: i8,
    pub snr_db: i8,
    pub mode: u8,
}

fn valid_call(s: &str) -> bool {
    (MIN_CALL..=MAX_CALL).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'/' || b == b'-')
}

fn check(r: &Reception) -> Result<(), String> {
    if !valid_call(&r.reporter) {
        return Err(format!("reporter callsign invalid: {:?}", r.reporter));
    }
    if !valid_call(&r.heard) {
        return Err(format!("heard callsign invalid: {:?}", r.heard));
    }
    if r.minute_utc > MAX_MINUTE {
        return Err(format!("minute_utc out of range: {}", r.minute_utc));
    }
    if !(-MAX_DB..=MAX_DB).contains(&r.level_db) {
        return Err(format!("level_db out of range: {}", r.level_db));
    }
    if !(-MAX_DB..=MAX_DB).contains(&r.snr_db) {
        return Err(format!("snr_db out of range: {}", r.snr_db));
    }
    Ok(())
}

/// Encode a reception report. Fails if any field is out of range.
pub fn encode(r: &Reception) -> Result<Vec<u8>, String> {
    check(r)?;
    let rl = r.reporter.len() - MIN_CALL;
    let hl = r.heard.len() - MIN_CALL;
    let mut out = Vec::with_capacity(FIXED_LEN + r.reporter.len() + r.heard.len());
    out.push(((rl as u8) << 3) | hl as u8);
    out.extend_from_slice(r.reporter.as_bytes());
    out.extend_from_slice(r.heard.as_bytes());
    out.extend_from_slice(&r.freq_khz.to_le_bytes());
    out.extend_from_slice(&r.minute_utc.to_le_bytes());
    out.push(r.level_db as u8);
    out.push(r.snr_db as u8);
    out.push(r.mode);
    Ok(out)
}

/// Decode a reception report. Returns None for any malformed or out-of-range input.
pub fn decode(bytes: &[u8]) -> Option<Reception> {
    let (&lens, rest) = bytes.split_first()?;
    if lens >> 6 != 0 {
        return None;
    }
    let rl = ((lens >> 3) & 0x07) as usize + MIN_CALL;
    let hl = (lens & 0x07) as usize + MIN_CALL;
    if bytes.len() != FIXED_LEN + rl + hl {
        return None;
    }
    let reporter = std::str::from_utf8(&rest[..rl]).ok()?;
    let heard = std::str::from_utf8(&rest[rl..rl + hl]).ok()?;
    let tail = &rest[rl + hl..];
    let r = Reception {
        reporter: reporter.to_string(),
        heard: heard.to_string(),
        freq_khz: u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]),
        minute_utc: u16::from_le_bytes([tail[4], tail[5]]),
        level_db: tail[6] as i8,
        snr_db: tail[7] as i8,
        mode: tail[8],
    };
    check(&r).ok()?;
    Some(r)
}

/// Stable duplicate-removal key: heard, reporter, frequency and minute.
pub fn key(r: &Reception) -> String {
    format!("{}|{}|{}|{}", r.heard, r.reporter, r.freq_khz, r.minute_utc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Reception {
        Reception {
            reporter: "N0CALL/P".to_string(),
            heard: "OH2ABC".to_string(),
            freq_khz: 14_074,
            minute_utc: 732,
            level_db: -12,
            snr_db: 7,
            mode: 3,
        }
    }

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u8 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 33) as u8
        }
    }

    fn random_call(g: &mut Lcg) -> String {
        const SET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789/-";
        let len = MIN_CALL + (g.next() as usize % (MAX_CALL - MIN_CALL + 1));
        (0..len)
            .map(|_| SET[g.next() as usize % SET.len()] as char)
            .collect()
    }

    #[test]
    fn round_trip() {
        let r = sample();
        let bytes = encode(&r).unwrap();
        assert_eq!(bytes.len(), 10 + 8 + 6);
        assert!(bytes.len() < 30);
        assert_eq!(decode(&bytes), Some(r));
    }

    #[test]
    fn max_size_is_30() {
        let r = Reception {
            reporter: "ABCDEFGHIJ".to_string(),
            heard: "KLMNOPQRST".to_string(),
            ..sample()
        };
        let bytes = encode(&r).unwrap();
        assert_eq!(bytes.len(), 30);
        assert_eq!(decode(&bytes), Some(r));
    }

    #[test]
    fn boundaries_accepted() {
        let r = Reception {
            reporter: "ABC".to_string(),
            heard: "Z9/-Z9/-Z".to_string(),
            minute_utc: 1439,
            level_db: -40,
            snr_db: 40,
            ..sample()
        };
        let bytes = encode(&r).unwrap();
        assert_eq!(decode(&bytes), Some(r));
    }

    #[test]
    fn duplicate_key() {
        let a = sample();
        let b = sample();
        assert_eq!(key(&a), key(&b));
        let c = Reception {
            minute_utc: 733,
            ..sample()
        };
        assert_ne!(key(&a), key(&c));
        let d = Reception {
            level_db: 5,
            snr_db: -3,
            mode: 9,
            ..sample()
        };
        assert_eq!(key(&a), key(&d), "level/snr/mode are not part of the key");
    }

    #[test]
    fn out_of_range_rejected() {
        let bad = [
            Reception {
                reporter: "AB".to_string(),
                ..sample()
            },
            Reception {
                heard: "ABCDEFGHIJK".to_string(),
                ..sample()
            },
            Reception {
                reporter: "AB C".to_string(),
                ..sample()
            },
            Reception {
                minute_utc: 1440,
                ..sample()
            },
            Reception {
                level_db: 41,
                ..sample()
            },
            Reception {
                level_db: -41,
                ..sample()
            },
            Reception {
                snr_db: 41,
                ..sample()
            },
            Reception {
                snr_db: i8::MIN,
                ..sample()
            },
        ];
        for r in &bad {
            assert!(encode(r).is_err(), "should reject {:?}", r);
        }
    }

    #[test]
    fn malformed_decode_rejected() {
        let good = encode(&sample()).unwrap();
        assert!(decode(&good[..good.len() - 1]).is_none(), "truncated");
        let mut long = good.clone();
        long.push(0);
        assert!(decode(&long).is_none(), "trailing byte");
        assert!(decode(&[]).is_none());
        let mut bad_len = good.clone();
        bad_len[0] |= 0x80;
        assert!(decode(&bad_len).is_none(), "non-canonical length byte");
    }

    #[test]
    fn decode_random_bytes_never_panics() {
        let mut g = Lcg(0x5eed);
        for _ in 0..50_000 {
            let len = g.next() as usize % 48;
            let buf: Vec<u8> = (0..len).map(|_| g.next()).collect();
            if let Some(r) = decode(&buf) {
                assert_eq!(
                    encode(&r).unwrap(),
                    buf,
                    "decoded report must re-encode identically"
                );
            }
        }
    }

    #[test]
    fn random_valid_reports_round_trip() {
        let mut g = Lcg(42);
        for _ in 0..5_000 {
            let r = Reception {
                reporter: random_call(&mut g),
                heard: random_call(&mut g),
                freq_khz: u32::from_le_bytes([g.next(), g.next(), g.next(), g.next()]),
                minute_utc: (u16::from(g.next()) << 8 | u16::from(g.next())) % 1440,
                level_db: (g.next() % 81) as i8 - 40,
                snr_db: (g.next() % 81) as i8 - 40,
                mode: g.next(),
            };
            let bytes = encode(&r).unwrap();
            assert_eq!(decode(&bytes), Some(r));
        }
    }
}
