//! Time broadcast payload, so stations can align their clocks.
//!
//! Wire format (all multi-byte fields big-endian):
//!   byte 0       stratum, 0..=15 (hops from a reference clock)
//!   bytes 1..=6  UTC time as unix seconds, unsigned 48-bit (6 bytes, good to year 8921)
//!   byte 7       callsign length n, 1..=16
//!   bytes 8..    callsign, n bytes of ASCII letters, digits or '/'
//!
//! Encoded size is 8 + callsign length bytes.

/// 2020-01-01T00:00:00Z as unix seconds. Earlier times are rejected.
pub const MIN_UNIX_S: u64 = 1_577_836_800;

/// Largest value that fits in the 6-byte time field.
const MAX_UNIX_S: u64 = (1u64 << 48) - 1;
const MAX_STRATUM: u8 = 15;
const MAX_CALL_LEN: usize = 16;
const HEADER_LEN: usize = 8;

/// A time broadcast: who sent it, UTC unix seconds, and how far from a reference clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeSignal {
    pub call: String,
    pub unix_s: u64,
    pub stratum: u8,
}

fn call_char_ok(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'/'
}

/// Encode a time signal. Fails if the stratum, time, or callsign is out of range.
pub fn encode(t: &TimeSignal) -> Result<Vec<u8>, String> {
    if t.stratum > MAX_STRATUM {
        return Err(format!("stratum {} out of range 0..=15", t.stratum));
    }
    if t.unix_s < MIN_UNIX_S {
        return Err(format!("time {} is before 2020-01-01", t.unix_s));
    }
    if t.unix_s > MAX_UNIX_S {
        return Err(format!("time {} does not fit in 6 bytes", t.unix_s));
    }
    let call = t.call.as_bytes();
    if call.is_empty() || call.len() > MAX_CALL_LEN {
        return Err(format!("callsign length {} not in 1..=16", call.len()));
    }
    if !call.iter().all(|&b| call_char_ok(b)) {
        return Err("callsign has characters other than letters, digits or '/'".to_string());
    }

    let mut out = Vec::with_capacity(HEADER_LEN + call.len());
    out.push(t.stratum);
    out.extend_from_slice(&t.unix_s.to_be_bytes()[2..8]);
    out.push(call.len() as u8);
    out.extend_from_slice(call);
    Ok(out)
}

/// Decode a time signal. Returns None for any malformed or out-of-range input,
/// including trailing bytes. Never panics.
pub fn decode(bytes: &[u8]) -> Option<TimeSignal> {
    if bytes.len() < HEADER_LEN {
        return None;
    }
    let stratum = bytes[0];
    if stratum > MAX_STRATUM {
        return None;
    }
    let mut buf = [0u8; 8];
    buf[2..8].copy_from_slice(&bytes[1..7]);
    let unix_s = u64::from_be_bytes(buf);
    if unix_s < MIN_UNIX_S {
        return None;
    }
    let n = bytes[7] as usize;
    if n == 0 || n > MAX_CALL_LEN || bytes.len() != HEADER_LEN + n {
        return None;
    }
    let call_bytes = &bytes[HEADER_LEN..];
    if !call_bytes.iter().all(|&b| call_char_ok(b)) {
        return None;
    }
    let call = String::from_utf8(call_bytes.to_vec()).ok()?;
    Some(TimeSignal {
        call,
        unix_s,
        stratum,
    })
}

/// Estimate the local clock offset from a received time signal, in seconds.
///
/// The sender's time at arrival is expected to be `received.unix_s + delay_s`.
/// The result is `local - expected`: positive means the local clock is ahead.
pub fn offset_estimate(local_unix_s: u64, received: &TimeSignal, delay_s: f64) -> f64 {
    let expected_now = received.unix_s as f64 + delay_s;
    local_unix_s as f64 - expected_now
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: u64 = 1_700_000_000;

    fn sig(call: &str, unix_s: u64, stratum: u8) -> TimeSignal {
        TimeSignal {
            call: call.to_string(),
            unix_s,
            stratum,
        }
    }

    #[test]
    fn round_trip() {
        let t = sig("LY2ABC/P", T0, 3);
        let bytes = encode(&t).unwrap();
        assert_eq!(bytes.len(), HEADER_LEN + t.call.len());
        assert_eq!(decode(&bytes), Some(t));

        let edge = sig("K1", MAX_UNIX_S, 15);
        assert_eq!(decode(&encode(&edge).unwrap()), Some(edge));

        let min = sig("K1", MIN_UNIX_S, 0);
        assert_eq!(decode(&encode(&min).unwrap()), Some(min));
    }

    #[test]
    fn encoded_size_for_six_char_call() {
        let bytes = encode(&sig("LY2ABC", T0, 1)).unwrap();
        assert_eq!(bytes.len(), 14);
    }

    #[test]
    fn stratum_16_rejected() {
        assert!(encode(&sig("LY2ABC", T0, 16)).is_err());
        assert!(encode(&sig("LY2ABC", T0, 15)).is_ok());
    }

    #[test]
    fn time_before_2020_rejected() {
        assert!(encode(&sig("LY2ABC", MIN_UNIX_S - 1, 1)).is_err());
        assert!(encode(&sig("LY2ABC", MIN_UNIX_S, 1)).is_ok());
    }

    #[test]
    fn bad_callsign_rejected() {
        assert!(encode(&sig("", T0, 1)).is_err());
        assert!(encode(&sig("ABCDEFGHIJKLMNOPQ", T0, 1)).is_err());
        assert!(encode(&sig("LY 2A", T0, 1)).is_err());
    }

    #[test]
    fn decode_rejects_bad_fields() {
        let mut bytes = encode(&sig("LY2ABC", T0, 1)).unwrap();
        bytes[0] = 16;
        assert!(decode(&bytes).is_none());

        let mut trailing = encode(&sig("LY2ABC", T0, 1)).unwrap();
        trailing.push(0);
        assert!(decode(&trailing).is_none());

        let truncated = encode(&sig("LY2ABC", T0, 1)).unwrap();
        assert!(decode(&truncated[..truncated.len() - 1]).is_none());
        assert!(decode(&[]).is_none());
    }

    #[test]
    fn offset_estimate_for_clock_ahead() {
        // Sender stamped T0 with zero delay; local clock reads T0 + 2 s.
        // The 1 s integer resolution of unix seconds means the estimate is 2.0,
        // within 0.5 s of the 2.5 s true offset.
        let received = sig("LY2ABC", T0, 1);
        let est = offset_estimate(T0 + 2, &received, 0.0);
        assert!((est - 2.5).abs() <= 0.5, "estimate {est}");
    }

    #[test]
    fn offset_estimate_corrects_for_delay() {
        let received = sig("LY2ABC", T0, 1);
        // Signal took 0.5 s, so the sender's clock now reads T0 + 0.5.
        // Local reads T0 + 3, so it is 2.5 s ahead.
        let est = offset_estimate(T0 + 3, &received, 0.5);
        assert!((est - 2.5).abs() < 1e-9);
    }

    #[test]
    fn decode_random_bytes_never_panics() {
        // Small LCG so the test needs no extra dependencies.
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as u8
        };
        for _ in 0..100_000 {
            let len = (next() % 40) as usize;
            let mut buf: Vec<u8> = (0..len).map(|_| next()).collect();
            // Bias some inputs toward valid headers so the accept path is exercised too.
            if len >= HEADER_LEN && next() % 2 == 0 {
                buf[0] = next() % 16;
                buf[7] = (len - HEADER_LEN) as u8;
                for b in buf[HEADER_LEN..].iter_mut() {
                    *b = b'A' + (*b % 26);
                }
                buf[1] = 0x00;
                buf[2] = 0x5E;
            }
            if let Some(t) = decode(&buf) {
                // Anything accepted must re-encode to the same bytes.
                assert_eq!(encode(&t).unwrap(), buf);
            }
        }
    }
}
