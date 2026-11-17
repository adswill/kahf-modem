//! Presence beacon payload: "I am here, on channel N, in mode M".
//!
//! Layout (4-byte header followed by the callsign, no length byte):
//!   byte 0      payload type, always `TYPE_PRESENCE`
//!   byte 1      mode
//!   bytes 2-3   big-endian u16: status in the top 2 bits, channel in the low 14 bits
//!   bytes 4..   callsign, ASCII (A-Z, 0-9, '/'), 1 to 15 characters
//!
//! A 6 character callsign encodes to 10 bytes.

/// Type byte that marks a payload as presence.
pub const TYPE_PRESENCE: u8 = 0x50;

const HEADER_LEN: usize = 4;
const MAX_CALL_LEN: usize = 15;
const MAX_CHANNEL: u16 = 0x3FFF;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Listening,
    Available,
    Busy,
    Away,
}

impl Status {
    fn code(self) -> u16 {
        match self {
            Status::Listening => 0,
            Status::Available => 1,
            Status::Busy => 2,
            Status::Away => 3,
        }
    }

    fn from_code(code: u16) -> Option<Status> {
        match code {
            0 => Some(Status::Listening),
            1 => Some(Status::Available),
            2 => Some(Status::Busy),
            3 => Some(Status::Away),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Presence {
    pub call: String,
    /// Channel number, 0 to 16383.
    pub channel: u16,
    pub mode: u8,
    pub status: Status,
}

fn is_call_byte(b: u8) -> bool {
    b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'/'
}

fn valid_call(call: &str) -> bool {
    !call.is_empty() && call.len() <= MAX_CALL_LEN && call.bytes().all(is_call_byte)
}

/// Encode a presence beacon. Fails on an invalid callsign or a channel above 16383.
pub fn encode(p: &Presence) -> Result<Vec<u8>, String> {
    if !valid_call(&p.call) {
        return Err(format!("invalid callsign: {:?}", p.call));
    }
    if p.channel > MAX_CHANNEL {
        return Err(format!("channel {} exceeds {}", p.channel, MAX_CHANNEL));
    }
    let packed = (p.status.code() << 14) | p.channel;
    let mut out = Vec::with_capacity(HEADER_LEN + p.call.len());
    out.push(TYPE_PRESENCE);
    out.push(p.mode);
    out.extend_from_slice(&packed.to_be_bytes());
    out.extend_from_slice(p.call.as_bytes());
    Ok(out)
}

/// Decode a presence beacon. Returns None for any other type byte or any malformed input.
pub fn decode(bytes: &[u8]) -> Option<Presence> {
    if bytes.len() < HEADER_LEN + 1 || bytes.len() > HEADER_LEN + MAX_CALL_LEN {
        return None;
    }
    if bytes[0] != TYPE_PRESENCE {
        return None;
    }
    let mode = bytes[1];
    let packed = u16::from_be_bytes([bytes[2], bytes[3]]);
    let status = Status::from_code(packed >> 14)?;
    let channel = packed & MAX_CHANNEL;
    let call_bytes = &bytes[HEADER_LEN..];
    if !call_bytes.iter().all(|&b| is_call_byte(b)) {
        return None;
    }
    let call = String::from_utf8(call_bytes.to_vec()).ok()?;
    Some(Presence {
        call,
        channel,
        mode,
        status,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Presence {
        Presence {
            call: "KAHF01".to_string(),
            channel: 1234,
            mode: 52,
            status: Status::Busy,
        }
    }

    #[test]
    fn round_trip_all_statuses() {
        for status in [Status::Listening, Status::Available, Status::Busy, Status::Away] {
            for channel in [0u16, 1, 1234, MAX_CHANNEL] {
                let p = Presence {
                    call: "W1AW/P".to_string(),
                    channel,
                    mode: 255,
                    status,
                };
                let bytes = encode(&p).expect("encode");
                assert_eq!(decode(&bytes), Some(p));
            }
        }
    }

    #[test]
    fn six_char_callsign_fits_in_10_bytes() {
        let bytes = encode(&sample()).expect("encode");
        assert_eq!(bytes.len(), 10);
        assert!(bytes.len() <= 10);
        assert_eq!(bytes[0], TYPE_PRESENCE);
    }

    #[test]
    fn invalid_callsign_rejected() {
        for bad in ["", "kahf01", "KA HF", "KAHF-1", "ABCDEFGHIJKLMNOP", "KÄHF"] {
            let p = Presence {
                call: bad.to_string(),
                channel: 1,
                mode: 1,
                status: Status::Listening,
            };
            assert!(encode(&p).is_err(), "accepted {:?}", bad);
        }
    }

    #[test]
    fn channel_out_of_range_rejected() {
        let mut p = sample();
        p.channel = MAX_CHANNEL + 1;
        assert!(encode(&p).is_err());
    }

    #[test]
    fn different_type_byte_returns_none() {
        let mut bytes = encode(&sample()).expect("encode");
        for t in 0u8..=255 {
            if t == TYPE_PRESENCE {
                continue;
            }
            bytes[0] = t;
            assert_eq!(decode(&bytes), None, "type byte {:#04x} accepted", t);
        }
    }

    #[test]
    fn malformed_inputs_return_none() {
        assert_eq!(decode(&[]), None);
        assert_eq!(decode(&[TYPE_PRESENCE]), None);
        assert_eq!(decode(&[TYPE_PRESENCE, 1, 0, 0]), None);
        // lowercase callsign
        assert_eq!(decode(&[TYPE_PRESENCE, 1, 0, 0, b'a']), None);
    }

    #[test]
    fn random_bytes_never_panic() {
        // xorshift64, deterministic and dependency-free
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..20_000 {
            let len = (next() % 64) as usize;
            let mut buf = Vec::with_capacity(len);
            for _ in 0..len {
                buf.push(next() as u8);
            }
            if let Some(p) = decode(&buf) {
                // anything accepted must re-encode to the same bytes
                assert_eq!(encode(&p).expect("re-encode"), buf);
            }
        }
    }
}
