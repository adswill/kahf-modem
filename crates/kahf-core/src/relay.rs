//! Relay station data model: roles, radio capability, hop records, daily statistics
//! and half-duplex slot planning. Pure data, no I/O.

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

    /// All hops for one message, in recorded order.
    pub fn chain_for(&self, message_id: u64) -> Vec<Hop> {
        self.hops
            .iter()
            .filter(|h| h.message_id == message_id)
            .cloned()
            .collect()
    }
}

/// True only for radios that can receive while they transmit.
pub fn can_receive_while_transmitting(radio: Radio) -> bool {
    matches!(radio, Radio::FullDuplex | Radio::SdrReceiveAssist)
}

/// Send counters for a relay.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stats {
    pub sent_today: u64,
    pub sent_total: u64,
    /// Start of the day `sent_today` currently counts. Not part of the public data model.
    day_start: u64,
}

impl Stats {
    /// Count one sent message. `day_start_unix` is the start of the current day;
    /// when it moves past the stored day start, the daily count restarts at zero.
    pub fn count_sent(&mut self, now_unix: u64, day_start_unix: u64) {
        debug_assert!(day_start_unix <= now_unix, "day start must not be after now");
        if day_start_unix != self.day_start {
            self.day_start = day_start_unix;
            self.sent_today = 0;
        }
        self.sent_today += 1;
        self.sent_total += 1;
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

    fn hop(id: u64, relay: &str, at: u64) -> Hop {
        Hop {
            message_id: id,
            relay: relay.to_string(),
            at_unix: at,
            channel_khz: 145_500.0,
            heard_by_next: true,
        }
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
    fn alternating_slots_alternate_from_receive() {
        assert_eq!(
            alternating_slots(10, 4),
            vec![(0, false), (10, true), (20, false), (30, true)]
        );
        assert!(alternating_slots(10, 0).is_empty());
    }
}
