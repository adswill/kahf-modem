//! Routing table the server sends to registered relays.
//!
//! This is data only. A relay shows each entry to its operator, who reads it
//! and confirms it. Nothing here transmits or keys a radio.
//!
//! Wire format, one message (integers are big-endian):
//!
//! ```text
//! count       u8            1..=6 entries
//! per entry:
//!   len       u8            callsign length in bytes
//!   relay     len bytes     ASCII callsign (see `valid_call`)
//!   channel   u24           kHz
//!   start     u16           minutes since midnight UTC, 0..=1439
//!   duration  u16           seconds
//!   priority  u8            plain priority code
//! ```
//!
//! A message holds at most [`MAX_ENTRIES`] entries. Longer tables are split
//! with [`encode_messages`]; [`encode`] refuses them and returns an empty Vec.

use crate::valid_call;

/// Most entries that fit in one message.
pub const MAX_ENTRIES: usize = 6;

const MINUTES_PER_DAY: u16 = 1440;
const MAX_CHANNEL_KHZ: u32 = 0xFF_FFFF;
/// Bytes per entry after the callsign: channel (3) + start (2) + duration (2) + priority (1).
const FIXED_FIELDS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayEntry {
    pub relay: String,
    pub channel_khz: u32,
    /// Minutes since midnight UTC.
    pub start_min: u16,
    pub duration_s: u16,
    pub priority: u8,
}

/// Convert planner output to table entries.
///
/// Each tuple is `(relay, channel_khz, start_unix, duration_s, priority)`. Build
/// it from a planner `Slot` as `(relay, slot.channel_khz, slot.start_s,
/// slot.end_s - slot.start_s, priority)`. The start is reduced to minutes since
/// midnight UTC. Slots whose duration does not fit in `u16` seconds are dropped.
pub fn table_from_slots(slots: &[(String, u32, u64, u64, u8)]) -> Vec<RelayEntry> {
    slots
        .iter()
        .filter_map(|(relay, channel, start_unix, duration, priority)| {
            let duration_s = u16::try_from(*duration).ok()?;
            Some(RelayEntry {
                relay: relay.clone(),
                channel_khz: *channel,
                start_min: ((start_unix % 86_400) / 60) as u16,
                duration_s,
                priority: *priority,
            })
        })
        .collect()
}

fn well_formed(e: &RelayEntry) -> bool {
    valid_call(&e.relay) && e.channel_khz <= MAX_CHANNEL_KHZ && e.start_min < MINUTES_PER_DAY
}

/// Encode one message. Returns an empty Vec if there are no entries, more than
/// [`MAX_ENTRIES`], or any entry is malformed.
pub fn encode(entries: &[RelayEntry]) -> Vec<u8> {
    if entries.is_empty() || entries.len() > MAX_ENTRIES || !entries.iter().all(well_formed) {
        return Vec::new();
    }
    let mut out = vec![entries.len() as u8];
    for e in entries {
        out.push(e.relay.len() as u8);
        out.extend_from_slice(e.relay.as_bytes());
        out.extend_from_slice(&e.channel_khz.to_be_bytes()[1..]);
        out.extend_from_slice(&e.start_min.to_be_bytes());
        out.extend_from_slice(&e.duration_s.to_be_bytes());
        out.push(e.priority);
    }
    out
}

/// Split any table into messages of at most [`MAX_ENTRIES`] entries each.
/// A table of seven entries becomes two messages, of six and one.
pub fn encode_messages(entries: &[RelayEntry]) -> Vec<Vec<u8>> {
    entries.chunks(MAX_ENTRIES).map(encode).collect()
}

/// Decode one message. Returns `None` for a bad count, truncated or trailing
/// bytes, a malformed callsign, or a start time outside the day.
pub fn decode(bytes: &[u8]) -> Option<Vec<RelayEntry>> {
    let (&count, mut rest) = bytes.split_first()?;
    let count = count as usize;
    if count == 0 || count > MAX_ENTRIES {
        return None;
    }
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let (&len, tail) = rest.split_first()?;
        let len = len as usize;
        if tail.len() < len + FIXED_FIELDS {
            return None;
        }
        let relay = std::str::from_utf8(&tail[..len]).ok()?.to_string();
        let f = &tail[len..];
        let entry = RelayEntry {
            relay,
            channel_khz: u32::from_be_bytes([0, f[0], f[1], f[2]]),
            start_min: u16::from_be_bytes([f[3], f[4]]),
            duration_s: u16::from_be_bytes([f[5], f[6]]),
            priority: f[7],
        };
        if !well_formed(&entry) {
            return None;
        }
        out.push(entry);
        rest = &f[FIXED_FIELDS..];
    }
    if !rest.is_empty() {
        return None;
    }
    Some(out)
}

/// The plain line the operator reads. It always asks the operator to confirm.
pub fn describe(e: &RelayEntry) -> String {
    format!(
        "Relay {}: listen and wait for a slot at {}.{:03} MHz at {:02}:{:02} UTC for {} seconds. \
         This is an instruction for you to read and confirm; nothing is keyed until you confirm it.",
        e.relay,
        e.channel_khz / 1000,
        e.channel_khz % 1000,
        e.start_min / 60,
        e.start_min % 60,
        e.duration_s
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::{plan, Priority, Request};

    fn entry(relay: &str, channel_khz: u32, start_min: u16, duration_s: u16, priority: u8) -> RelayEntry {
        RelayEntry {
            relay: relay.into(),
            channel_khz,
            start_min,
            duration_s,
            priority,
        }
    }

    fn six(relay: &str) -> Vec<RelayEntry> {
        (0..6)
            .map(|i| entry(relay, 14_105 + i, 845 + i as u16, 30, i as u8))
            .collect()
    }

    #[test]
    fn round_trip() {
        let table = vec![
            entry("BB1BBB", 14_105, 845, 30, 0),
            entry("AB/9", 7_040, 0, 65_535, 2),
            entry("A2/BB1BBB1", 144_300, 1439, 1, 255),
        ];
        let bytes = encode(&table);
        assert!(!bytes.is_empty());
        assert_eq!(decode(&bytes), Some(table));
    }

    #[test]
    fn six_entries_size() {
        // Six entries with 6-character callsigns: 1 count byte + 6 * (1 + 6 + 8).
        let bytes = encode(&six("BB1BBB"));
        assert_eq!(bytes.len(), 91);
        assert_eq!(decode(&bytes).map(|t| t.len()), Some(6));
    }

    #[test]
    fn seven_entries_are_refused_and_split() {
        let seven: Vec<RelayEntry> = six("BB1BBB")
            .into_iter()
            .chain(std::iter::once(entry("BB1BBB", 14_200, 900, 20, 1)))
            .collect();
        assert!(encode(&seven).is_empty());
        assert!(decode(&[]).is_none());

        let messages = encode_messages(&seven);
        assert_eq!(messages.len(), 2);
        assert_eq!(decode(&messages[0]).map(|t| t.len()), Some(6));
        assert_eq!(decode(&messages[1]).map(|t| t.len()), Some(1));
        let mut joined = decode(&messages[0]).unwrap();
        joined.extend(decode(&messages[1]).unwrap());
        assert_eq!(joined, seven);
    }

    #[test]
    fn decode_rejects_bad_messages() {
        let good = encode(&[entry("BB1BBB", 14_105, 845, 30, 0)]);
        assert!(decode(&good[..good.len() - 1]).is_none(), "truncated");
        let mut trailing = good.clone();
        trailing.push(0);
        assert!(decode(&trailing).is_none(), "trailing byte");
        assert!(decode(&[0]).is_none(), "zero count");
        assert!(decode(&[7]).is_none(), "count above six");

        let mut bad_start = good.clone();
        let start_at = 1 + 1 + 6 + 3;
        bad_start[start_at..start_at + 2].copy_from_slice(&1440u16.to_be_bytes());
        assert!(decode(&bad_start).is_none(), "start past midnight");
    }

    #[test]
    fn encode_refuses_bad_entries() {
        assert!(encode(&[entry("BAD CALL", 14_105, 0, 30, 0)]).is_empty());
        assert!(encode(&[entry("BB1BBB", 0x100_0000, 0, 30, 0)]).is_empty());
        assert!(encode(&[entry("BB1BBB", 14_105, 1440, 30, 0)]).is_empty());
    }

    #[test]
    fn table_from_planner_output() {
        let reqs = [Request {
            id: 1,
            from: "AB1CD".into(),
            to: "EF2GH".into(),
            priority: Priority::Urgent,
            duration_s: 30,
        }];
        let start = 1_728_000_000 + 14 * 3600 + 5 * 60; // 14:05 UTC
        let slots = plan(&reqs, &[(14_105, 0.0)], start, 0);
        let input: Vec<(String, u32, u64, u64, u8)> = slots
            .iter()
            .map(|s| {
                (
                    "BB1BBB".to_string(),
                    s.channel_khz,
                    s.start_s,
                    s.end_s - s.start_s,
                    0,
                )
            })
            .collect();
        let table = table_from_slots(&input);
        assert_eq!(table, vec![entry("BB1BBB", 14_105, 845, 30, 0)]);
        assert_eq!(
            describe(&table[0]),
            "Relay BB1BBB: listen and wait for a slot at 14.105 MHz at 14:05 UTC for 30 seconds. \
             This is an instruction for you to read and confirm; nothing is keyed until you confirm it."
        );
    }

    #[test]
    fn table_from_slots_drops_oversized_duration() {
        let input = [
            ("BB1BBB".to_string(), 14_105, 0, 70_000, 0),
            ("BB1BBB".to_string(), 14_105, 60, 30, 1),
        ];
        assert_eq!(table_from_slots(&input), vec![entry("BB1BBB", 14_105, 1, 30, 1)]);
    }

    #[test]
    fn describe_asks_for_confirmation() {
        let line = describe(&entry("BB1BBB", 14_105, 845, 30, 0));
        assert!(line.contains("confirm"), "{line}");
        assert!(line.starts_with("Relay BB1BBB: listen and wait for a slot at 14.105 MHz at 14:05 UTC"));
    }
}
