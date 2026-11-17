//! Pure slot planner. It proposes transmit slots for queued contact requests.
//!
//! This module never transmits or keys a radio. Every proposal is only a
//! suggestion and needs an operator's confirmation elsewhere.
//!
//! Times are unix seconds and are shown in UTC. A channel is `(kHz, busy score)`;
//! a busy score of 1.0 or more (or NaN) means the channel is never proposed.

use std::cmp::Ordering;

/// Busy score at or above which a channel is never proposed.
const UNUSABLE_BUSY: f32 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    Urgent,
    Normal,
    Low,
}

impl Priority {
    fn rank(self) -> u8 {
        match self {
            Priority::Urgent => 0,
            Priority::Normal => 1,
            Priority::Low => 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub id: u64,
    pub from: String,
    pub to: String,
    pub priority: Priority,
    /// Expected transmit duration in seconds.
    pub duration_s: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub request_id: u64,
    pub channel_khz: u32,
    pub start_s: u64,
    pub end_s: u64,
    /// Idle time since the previous slot on this channel, in seconds (0 if none).
    pub gap_s: u64,
}

/// Propose non-overlapping slots for `requests`.
///
/// Requests are placed in priority order (urgent, normal, low; input order
/// within a priority). Each request takes the channel where it can start
/// earliest, with a quieter channel winning ties. Every slot starts at or
/// after `now_s` and is separated from the neighbouring slots on its channel
/// by at least `guard_s` seconds. Requests that cannot be placed (for
/// example, no usable channel) are left out; see [`unplanned`].
pub fn plan(requests: &[Request], channels: &[(u32, f32)], now_s: u64, guard_s: u64) -> Vec<Slot> {
    let mut order: Vec<&Request> = requests.iter().collect();
    order.sort_by_key(|r| r.priority.rank());

    // Per channel: occupied (start, end) intervals, kept sorted by start.
    let mut occupied: Vec<Vec<(u64, u64)>> = vec![Vec::new(); channels.len()];
    let mut out = Vec::new();

    for req in order {
        let mut best: Option<(u64, f32, usize)> = None; // (start, busy, index)
        for (idx, &(_, busy)) in channels.iter().enumerate() {
            if busy.partial_cmp(&UNUSABLE_BUSY) != Some(Ordering::Less) {
                continue;
            }
            let start = earliest_start(&occupied[idx], now_s, req.duration_s, guard_s);
            let better = match best {
                None => true,
                Some((bs, bb, _)) => start < bs || (start == bs && busy.total_cmp(&bb) == Ordering::Less),
            };
            if better {
                best = Some((start, busy, idx));
            }
        }

        if let Some((start, _, idx)) = best {
            let end = start.saturating_add(req.duration_s);
            let intervals = &mut occupied[idx];
            let pos = intervals.partition_point(|&(s, _)| s < start);
            let gap_s = if pos > 0 { start - intervals[pos - 1].1 } else { 0 };
            intervals.insert(pos, (start, end));
            out.push(Slot {
                request_id: req.id,
                channel_khz: channels[idx].0,
                start_s: start,
                end_s: end,
                gap_s,
            });
        }
    }
    out
}

/// Earliest start at or after `now_s` that fits `duration_s` between the
/// sorted, non-overlapping `intervals` with `guard_s` on both sides.
fn earliest_start(intervals: &[(u64, u64)], now_s: u64, duration_s: u64, guard_s: u64) -> u64 {
    let mut t = now_s;
    for &(s, e) in intervals {
        if t.saturating_add(duration_s).saturating_add(guard_s) <= s {
            return t;
        }
        t = t.max(e.saturating_add(guard_s));
    }
    t
}

/// One plain sentence for the operator, for example
/// "Send at 14:05 on 14.105 MHz, after a 10 second gap."
pub fn explain(slot: &Slot) -> String {
    let of_day = slot.start_s % 86_400;
    let (hh, mm) = (of_day / 3600, (of_day % 3600) / 60);
    let khz = slot.channel_khz;
    let gap = if slot.gap_s > 0 {
        format!("after a {} second gap", slot.gap_s)
    } else {
        "with no gap before it".to_string()
    };
    format!(
        "Send at {hh:02}:{mm:02} on {}.{:03} MHz, {gap}.",
        khz / 1000,
        khz % 1000
    )
}

/// Ids of requests that have no slot, in input order.
pub fn unplanned(requests: &[Request], slots: &[Slot]) -> Vec<u64> {
    requests
        .iter()
        .filter(|r| !slots.iter().any(|s| s.request_id == r.id))
        .map(|r| r.id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(id: u64, priority: Priority, duration_s: u64) -> Request {
        Request {
            id,
            from: "AB1CD".into(),
            to: "EF2GH".into(),
            priority,
            duration_s,
        }
    }

    #[test]
    fn planner_no_overlap_on_one_channel() {
        let reqs: Vec<Request> = (1..=5).map(|i| req(i, Priority::Normal, 60)).collect();
        let channels = [(14_105, 0.2)];
        let slots = plan(&reqs, &channels, 1_000, 10);
        assert_eq!(slots.len(), 5);

        let mut on_channel: Vec<&Slot> = slots.iter().collect();
        on_channel.sort_by_key(|s| s.start_s);
        for pair in on_channel.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            assert!(b.start_s >= a.end_s + 10, "{a:?} then {b:?}");
            assert_eq!(b.gap_s, b.start_s - a.end_s);
        }
    }

    #[test]
    fn planner_no_overlap_across_two_channels() {
        let reqs: Vec<Request> = (1..=6).map(|i| req(i, Priority::Low, 30)).collect();
        let channels = [(14_105, 0.1), (7_040, 0.3)];
        let slots = plan(&reqs, &channels, 0, 5);
        assert_eq!(slots.len(), 6);
        for a in &slots {
            for b in &slots {
                if a.request_id != b.request_id && a.channel_khz == b.channel_khz {
                    let apart = b.start_s >= a.end_s + 5 || a.start_s >= b.end_s + 5;
                    assert!(apart, "{a:?} overlaps {b:?}");
                }
            }
        }
    }

    #[test]
    fn planner_urgent_before_normal() {
        let reqs = [req(1, Priority::Normal, 60), req(2, Priority::Urgent, 60)];
        let channels = [(14_105, 0.0)];
        let slots = plan(&reqs, &channels, 500, 10);
        let urgent = slots.iter().find(|s| s.request_id == 2).unwrap();
        let normal = slots.iter().find(|s| s.request_id == 1).unwrap();
        assert_eq!(urgent.start_s, 500);
        assert!(urgent.start_s < normal.start_s);
        assert_eq!(slots[0].request_id, 2, "urgent is planned first");
    }

    #[test]
    fn planner_never_starts_before_now() {
        let reqs: Vec<Request> = (1..=4).map(|i| req(i, Priority::Normal, 20)).collect();
        let channels = [(14_105, 0.0), (14_100, 0.5)];
        let now = 9_999;
        for slot in plan(&reqs, &channels, now, 3) {
            assert!(slot.start_s >= now, "{slot:?}");
            assert_eq!(slot.end_s, slot.start_s + 20);
        }
    }

    #[test]
    fn planner_busy_channel_used_last() {
        let channels = [(14_105, 0.9), (14_100, 0.1)];
        let one = plan(&[req(1, Priority::Normal, 60)], &channels, 0, 10);
        assert_eq!(one[0].channel_khz, 14_100);

        // Equal start times: the quieter channel wins regardless of input order.
        let reversed = [(14_100, 0.1), (14_105, 0.9)];
        let again = plan(&[req(1, Priority::Normal, 60)], &reversed, 0, 10);
        assert_eq!(again[0].channel_khz, 14_100);
    }

    #[test]
    fn planner_unplanned_when_no_channel_fits() {
        let reqs = [req(1, Priority::Urgent, 60), req(2, Priority::Low, 60)];
        let channels = [(14_105, 1.0), (7_040, f32::NAN)];
        let slots = plan(&reqs, &channels, 0, 10);
        assert!(slots.is_empty());
        assert_eq!(unplanned(&reqs, &slots), vec![1, 2]);
    }

    #[test]
    fn planner_unplanned_lists_only_missing() {
        let reqs = [req(7, Priority::Normal, 60)];
        let slots = plan(&reqs, &[(14_105, 0.0)], 0, 0);
        assert!(unplanned(&reqs, &slots).is_empty());
        assert_eq!(unplanned(&reqs, &[]), vec![7]);
    }

    #[test]
    fn planner_empty_requests_give_no_slots() {
        assert!(plan(&[], &[(14_105, 0.0)], 0, 10).is_empty());
        assert!(unplanned(&[], &[]).is_empty());
    }

    #[test]
    fn planner_explain_sentence() {
        let slot = Slot {
            request_id: 1,
            channel_khz: 14_105,
            start_s: 1_728_000_000 + 14 * 3600 + 5 * 60,
            end_s: 0,
            gap_s: 10,
        };
        assert_eq!(
            explain(&slot),
            "Send at 14:05 on 14.105 MHz, after a 10 second gap."
        );
    }
}
