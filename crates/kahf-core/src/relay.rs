//! Relay station model: roles, radio capability, assigned channels, the slot
//! schedule, hop records and send statistics. Pure data and logic, no I/O.

use std::collections::BTreeMap;

/// Seconds in an hour, used for the per-hour statistics.
const HOUR_S: u64 = 3_600;
/// How long per-hour counts are kept, in seconds.
const HOURLY_KEEP_S: u64 = 48 * HOUR_S;
/// Two channel frequencies closer than this (kHz) are the same channel.
const CHANNEL_MATCH_KHZ: f32 = 0.01;

/// What a station does on the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Station,
    Relay,
    RelayAndStation,
}

/// Radio hardware class, which decides whether it can receive while transmitting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Radio {
    HalfDuplex,
    FullDuplex,
    SdrReceiveAssist,
}

/// True only for radios that can receive while they transmit.
pub fn can_receive_while_transmitting(radio: Radio) -> bool {
    matches!(radio, Radio::FullDuplex | Radio::SdrReceiveAssist)
}

/// Whether a slot is for listening or for sending.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Receive,
    Transmit,
}

/// One scheduled slot on one channel. Times are unix seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slot {
    pub channel_khz: f32,
    pub start_unix: u64,
    pub duration_s: u32,
    pub direction: Direction,
}

impl Slot {
    /// First second after the slot.
    pub fn end_unix(&self) -> u64 {
        self.start_unix + u64::from(self.duration_s)
    }

    /// True when `at_unix` falls inside the slot (start included, end excluded).
    pub fn contains(&self, at_unix: u64) -> bool {
        at_unix >= self.start_unix && at_unix < self.end_unix()
    }
}

/// One forwarding step of a message.
#[derive(Debug, Clone, PartialEq)]
pub struct Hop {
    /// Message this hop belongs to.
    pub message_id: u64,
    pub relay: String,
    pub at_unix: u64,
    pub channel_khz: f32,
    pub heard_by_next: bool,
}

/// Ordered log of hops, in the order they were recorded.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HopLog {
    hops: Vec<Hop>,
}

impl HopLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, hop: Hop) {
        self.hops.push(hop);
    }

    /// The newest `n` hops, newest first, and the total number recorded.
    pub fn recent(&self, n: usize) -> (Vec<Hop>, usize) {
        (self.hops.iter().rev().take(n).cloned().collect(), self.hops.len())
    }

    /// All hops for one message, in recorded order.
    pub fn chain_for(&self, message_id: u64) -> Vec<Hop> {
        self.hops
            .iter()
            .filter(|h| h.message_id == message_id)
            .cloned()
            .collect()
    }
}

/// Send counters for a relay.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stats {
    pub sent_today: u64,
    pub sent_total: u64,
    /// Start of the day `sent_today` currently counts. Not part of the public data model.
    day_start: u64,
    /// Sent count per hour, keyed by the start of that hour (unix seconds).
    hourly: BTreeMap<u64, u64>,
}

impl Stats {
    /// Count one sent message. `day_start_unix` is the start of the current day;
    /// when it moves past the stored day start, the daily count restarts at zero.
    /// The message is also counted in the hour that contains `now_unix`.
    pub fn count_sent(&mut self, now_unix: u64, day_start_unix: u64) {
        debug_assert!(day_start_unix <= now_unix, "day start must not be after now");
        if day_start_unix != self.day_start {
            self.day_start = day_start_unix;
            self.sent_today = 0;
        }
        self.sent_today += 1;
        self.sent_total += 1;

        *self.hourly.entry(hour_start(now_unix)).or_insert(0) += 1;
        let oldest_kept = now_unix.saturating_sub(HOURLY_KEEP_S);
        self.hourly.retain(|hour, _| *hour >= oldest_kept);
    }

    /// Messages sent in the hour that contains `at_unix`.
    pub fn sent_in_hour_of(&self, at_unix: u64) -> u64 {
        self.hourly.get(&hour_start(at_unix)).copied().unwrap_or(0)
    }

    /// Kept per-hour counts as `(hour start, count)`, oldest first.
    pub fn sent_per_hour(&self) -> Vec<(u64, u64)> {
        self.hourly.iter().map(|(h, n)| (*h, *n)).collect()
    }
}

fn hour_start(at_unix: u64) -> u64 {
    at_unix - at_unix % HOUR_S
}

fn on_channel(channels_khz: &[f32], khz: f32) -> bool {
    channels_khz.iter().any(|c| (c - khz).abs() < CHANNEL_MATCH_KHZ)
}

/// True when two slots of the same direction overlap, checked on a sorted list.
fn same_direction_overlap(sorted: &[Slot], direction: Direction) -> bool {
    let picked: Vec<&Slot> = sorted.iter().filter(|s| s.direction == direction).collect();
    picked.windows(2).any(|p| p[1].start_unix < p[0].end_unix())
}

/// A station acting as a relay: assigned channels, the accepted slot schedule
/// and the half-duplex rules that decide when it may send and listen.
#[derive(Debug, Clone, PartialEq)]
pub struct Relay {
    pub callsign: String,
    pub role: Role,
    pub radio: Radio,
    channels_khz: Vec<f32>,
    slots: Vec<Slot>,
}

impl Relay {
    /// Create a relay with its assigned channels and an empty schedule.
    pub fn new(callsign: &str, role: Role, radio: Radio, channels_khz: &[f32]) -> Result<Self, String> {
        let mut relay = Self {
            callsign: callsign.to_string(),
            role,
            radio,
            channels_khz: Vec::new(),
            slots: Vec::new(),
        };
        relay.assign_channels(channels_khz)?;
        Ok(relay)
    }

    /// Channels this relay listens on and may send on.
    pub fn channels_khz(&self) -> &[f32] {
        &self.channels_khz
    }

    /// The accepted schedule, sorted by start time.
    pub fn slots(&self) -> &[Slot] {
        &self.slots
    }

    /// Replace the assigned channels. Fails when the current schedule uses a
    /// channel that would be removed, or when the list is empty or invalid.
    pub fn assign_channels(&mut self, channels_khz: &[f32]) -> Result<(), String> {
        if channels_khz.is_empty() {
            return Err("A relay needs at least one channel.".to_string());
        }
        if channels_khz.iter().any(|f| !f.is_finite() || *f <= 0.0) {
            return Err("Channel frequencies must be positive numbers.".to_string());
        }
        if let Some(slot) = self
            .slots
            .iter()
            .find(|s| !on_channel(channels_khz, s.channel_khz))
        {
            return Err(format!(
                "The schedule uses {:.3} kHz, which is not in the new channel list.",
                slot.channel_khz
            ));
        }
        self.channels_khz = channels_khz.to_vec();
        Ok(())
    }

    /// Accept a schedule from the server. The slots are checked against the
    /// assigned channels and the radio's duplex rules, then sorted and kept.
    /// A rejected schedule leaves the old one in place.
    pub fn accept_schedule(&mut self, mut slots: Vec<Slot>) -> Result<(), String> {
        for slot in &slots {
            if slot.duration_s == 0 {
                return Err("Every slot must last at least one second.".to_string());
            }
            if !on_channel(&self.channels_khz, slot.channel_khz) {
                return Err(format!(
                    "A slot is on {:.3} kHz, which this relay is not assigned.",
                    slot.channel_khz
                ));
            }
        }
        slots.sort_by_key(|s| s.start_unix);

        if can_receive_while_transmitting(self.radio) {
            if same_direction_overlap(&slots, Direction::Receive)
                || same_direction_overlap(&slots, Direction::Transmit)
            {
                return Err("Two slots in the same direction cannot overlap.".to_string());
            }
        } else {
            for pair in slots.windows(2) {
                if pair[1].start_unix < pair[0].end_unix() {
                    return Err("A half-duplex relay cannot have overlapping slots.".to_string());
                }
                if pair[0].direction == pair[1].direction {
                    return Err("A half-duplex relay must alternate receive and transmit slots.".to_string());
                }
            }
        }

        self.slots = slots;
        Ok(())
    }

    /// The first slot covering `at_unix`, if any.
    pub fn slot_at(&self, at_unix: u64) -> Option<&Slot> {
        self.slots.iter().find(|s| s.contains(at_unix))
    }

    /// The first slot that starts at or after `at_unix`, if any.
    pub fn next_slot_after(&self, at_unix: u64) -> Option<&Slot> {
        self.slots.iter().find(|s| s.start_unix >= at_unix)
    }

    /// True when this station may send as a relay at `at_unix`: it must be a
    /// relay, and a transmit slot must cover that time.
    pub fn may_relay_transmit_at(&self, at_unix: u64) -> bool {
        self.role != Role::Station
            && self
                .slots
                .iter()
                .any(|s| s.direction == Direction::Transmit && s.contains(at_unix))
    }

    /// True when the radio can receive at `at_unix` under the schedule. A
    /// receive slot always counts. A transmit slot counts only when the radio
    /// can receive while transmitting.
    pub fn may_receive_at(&self, at_unix: u64) -> bool {
        let duplex = can_receive_while_transmitting(self.radio);
        self.slots
            .iter()
            .any(|s| s.contains(at_unix) && (s.direction == Direction::Receive || duplex))
    }
}

/// Half-duplex schedule: `count` slots of `slot_s` seconds, alternating receive and
/// transmit, starting with receive. Returns `(start_s, is_transmit)` pairs.
pub fn alternating_slots(slot_s: u32, count: u32) -> Vec<(u32, bool)> {
    (0..count).map(|i| (i * slot_s, i % 2 == 1)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CH_A: f32 = 145_500.0;
    const CH_B: f32 = 145_525.0;

    fn hop(id: u64, relay: &str, at: u64) -> Hop {
        Hop {
            message_id: id,
            relay: relay.to_string(),
            at_unix: at,
            channel_khz: CH_A,
            heard_by_next: true,
        }
    }

    fn slot(start: u64, secs: u32, dir: Direction) -> Slot {
        Slot {
            channel_khz: CH_A,
            start_unix: start,
            duration_s: secs,
            direction: dir,
        }
    }

    fn half_relay() -> Relay {
        Relay::new("KA1ABC", Role::Relay, Radio::HalfDuplex, &[CH_A, CH_B]).unwrap()
    }

    #[test]
    fn hop_log_chain_filters_by_message_in_order() {
        let mut log = HopLog::new();
        log.record(hop(1, "A", 10));
        log.record(hop(2, "B", 11));
        log.record(hop(1, "C", 12));
        let chain = log.chain_for(1);
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].relay, "A");
        assert_eq!(chain[1].relay, "C");
        assert!(log.chain_for(99).is_empty());
    }

    #[test]
    fn duplex_capability() {
        assert!(!can_receive_while_transmitting(Radio::HalfDuplex));
        assert!(can_receive_while_transmitting(Radio::FullDuplex));
        assert!(can_receive_while_transmitting(Radio::SdrReceiveAssist));
    }

    #[test]
    fn roles_are_distinct() {
        assert_ne!(Role::Station, Role::Relay);
        assert_ne!(Role::Relay, Role::RelayAndStation);
    }

    #[test]
    fn stats_reset_daily_count_on_new_day() {
        let mut s = Stats::default();
        s.count_sent(1_000, 0);
        s.count_sent(2_000, 0);
        assert_eq!((s.sent_today, s.sent_total), (2, 2));
        s.count_sent(90_000, 86_400);
        assert_eq!((s.sent_today, s.sent_total), (1, 3));
    }

    #[test]
    fn stats_count_per_hour_and_prune_old_hours() {
        let mut s = Stats::default();
        s.count_sent(3_600 * 10 + 5, 0);
        s.count_sent(3_600 * 10 + 900, 0);
        s.count_sent(3_600 * 11 + 1, 0);
        assert_eq!(s.sent_in_hour_of(3_600 * 10 + 59), 2);
        assert_eq!(s.sent_in_hour_of(3_600 * 11), 1);
        assert_eq!(s.sent_per_hour(), vec![(36_000, 2), (39_600, 1)]);
        // 60 hours later the old hours fall out of the 48 hour window.
        s.count_sent(3_600 * 71, 0);
        assert_eq!(s.sent_per_hour(), vec![(3_600 * 71, 1)]);
        assert_eq!(s.sent_total, 4);
    }

    #[test]
    fn alternating_slots_alternate_from_receive() {
        assert_eq!(
            alternating_slots(10, 4),
            vec![(0, false), (10, true), (20, false), (30, true)]
        );
        assert!(alternating_slots(10, 0).is_empty());
    }

    #[test]
    fn new_relay_needs_channels() {
        assert!(Relay::new("KA1ABC", Role::Relay, Radio::HalfDuplex, &[]).is_err());
        assert!(Relay::new("KA1ABC", Role::Relay, Radio::HalfDuplex, &[-1.0]).is_err());
        assert_eq!(half_relay().channels_khz(), &[CH_A, CH_B]);
    }

    #[test]
    fn assign_channels_keeps_schedule_valid() {
        let mut r = half_relay();
        r.accept_schedule(vec![slot(100, 10, Direction::Receive)])
            .unwrap();
        assert!(r.assign_channels(&[CH_B]).is_err());
        assert!(r.assign_channels(&[CH_A]).is_ok());
    }

    #[test]
    fn schedule_on_unassigned_channel_is_rejected() {
        let mut r = half_relay();
        let mut s = slot(100, 10, Direction::Receive);
        s.channel_khz = 14_000.0;
        assert!(r.accept_schedule(vec![s]).is_err());
        assert!(r.slots().is_empty());
    }

    #[test]
    fn zero_length_slot_is_rejected() {
        let mut r = half_relay();
        assert!(r
            .accept_schedule(vec![slot(100, 0, Direction::Transmit)])
            .is_err());
    }

    #[test]
    fn half_duplex_schedule_sorts_and_alternates() {
        let mut r = half_relay();
        r.accept_schedule(vec![
            slot(120, 10, Direction::Receive),
            slot(100, 10, Direction::Transmit),
        ])
        .unwrap();
        assert_eq!(r.slots()[0].start_unix, 100);
        assert!(r.may_relay_transmit_at(105));
        assert!(!r.may_relay_transmit_at(125));
        assert!(!r.may_receive_at(105));
        assert!(r.may_receive_at(125));
        assert_eq!(r.next_slot_after(101).unwrap().start_unix, 120);
        assert!(r.slot_at(200).is_none());
    }

    #[test]
    fn half_duplex_rejects_overlap_and_same_direction_runs() {
        let mut r = half_relay();
        let overlap = vec![
            slot(100, 20, Direction::Receive),
            slot(110, 10, Direction::Transmit),
        ];
        assert!(r.accept_schedule(overlap).is_err());
        let run = vec![
            slot(100, 10, Direction::Receive),
            slot(110, 10, Direction::Receive),
        ];
        assert!(r.accept_schedule(run).is_err());
        assert!(r.slots().is_empty(), "rejected schedules must not be kept");
    }

    #[test]
    fn full_duplex_may_receive_while_transmitting() {
        let mut r = Relay::new("KA1ABC", Role::Relay, Radio::FullDuplex, &[CH_A]).unwrap();
        r.accept_schedule(vec![
            slot(100, 60, Direction::Transmit),
            slot(100, 60, Direction::Receive),
        ])
        .unwrap();
        assert!(r.may_relay_transmit_at(110));
        assert!(r.may_receive_at(110));
        let same = vec![
            slot(100, 60, Direction::Transmit),
            slot(120, 60, Direction::Transmit),
        ];
        assert!(r.accept_schedule(same).is_err());
    }

    #[test]
    fn plain_station_never_relays() {
        let mut r = Relay::new("KA1ABC", Role::Station, Radio::HalfDuplex, &[CH_A]).unwrap();
        r.accept_schedule(vec![slot(100, 10, Direction::Transmit)])
            .unwrap();
        assert!(!r.may_relay_transmit_at(105));
    }
}
