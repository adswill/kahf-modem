//! Delivery tracking. Every outgoing message gets a [`Track`] with one status per recipient
//! (one recipient for a direct message, the group fan-out for a group message). Statuses only
//! move forward: Queued, Held (in a mailbox or relay), Sent, Delivered, Read. Failed and
//! Expired are terminal. Receipts arrive as the 3-byte `extras::Receipt` carrying a 16-bit id.
use super::short_id;
use crate::extras::Receipt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Queued,
    /// Waiting in a mailbox or relay for the station to come on air.
    Held,
    /// Handed to the link or a relay; no receipt yet.
    Sent,
    Delivered,
    Read,
    Failed,
    Expired,
}

impl Status {
    pub fn is_final(self) -> bool {
        matches!(self, Status::Read | Status::Failed | Status::Expired)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub id: u64,
    pub created: u64,
    /// Recipient callsign and its status.
    pub recipients: Vec<(String, Status)>,
    pub updated: u64,
}

impl Track {
    /// Overall status: the least advanced recipient that is not failed/expired; if every
    /// recipient failed or expired, Failed.
    pub fn overall(&self) -> Status {
        let live = self
            .recipients
            .iter()
            .map(|r| r.1)
            .filter(|s| !matches!(s, Status::Failed | Status::Expired));
        live.min().unwrap_or(Status::Failed)
    }

    pub fn delivered_count(&self) -> usize {
        self.recipients
            .iter()
            .filter(|r| matches!(r.1, Status::Delivered | Status::Read))
            .count()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Tracker {
    tracks: Vec<Track>,
}

impl Tracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn track(&mut self, id: u64, recipients: &[String], now: u64) {
        if let Some(old) = self.tracks.iter().position(|t| t.id == id) {
            // The same content sent again after the first copy finished starts a new track.
            if !self.tracks[old].recipients.iter().all(|r| r.1.is_final()) {
                return;
            }
            self.tracks.remove(old);
        }
        self.tracks.push(Track {
            id,
            created: now,
            recipients: recipients
                .iter()
                .map(|r| (r.to_ascii_uppercase(), Status::Queued))
                .collect(),
            updated: now,
        });
    }

    pub fn get(&self, id: u64) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    pub fn all(&self) -> &[Track] {
        &self.tracks
    }

    /// Moves one recipient (or every recipient when `to` is None) forward. A status never goes
    /// backward, and a final status is never replaced. Returns true if anything changed.
    pub fn advance(&mut self, id: u64, to: Option<&str>, status: Status, now: u64) -> bool {
        let Some(t) = self.tracks.iter_mut().find(|t| t.id == id) else {
            return false;
        };
        let want = to.map(str::to_ascii_uppercase);
        let mut changed = false;
        for r in t
            .recipients
            .iter_mut()
            .filter(|r| want.as_ref().is_none_or(|w| *w == r.0))
        {
            let allowed = !r.1.is_final() && (status > r.1 || status.is_final());
            if allowed && r.1 != status {
                r.1 = status;
                changed = true;
            }
        }
        if changed {
            t.updated = now;
        }
        changed
    }

    /// Applies a receipt heard from `from`. The wire id is 16 bits, so the newest track to
    /// that station with a matching short id is chosen. Returns the full id if one matched.
    pub fn on_receipt(&mut self, receipt: &Receipt, from: &str, now: u64) -> Option<u64> {
        let (wire, status) = match *receipt {
            Receipt::Delivered { message_id } => (message_id, Status::Delivered),
            Receipt::Read { message_id } => (message_id, Status::Read),
        };
        let from = from.to_ascii_uppercase();
        let id = self
            .tracks
            .iter()
            .filter(|t| short_id(t.id) == wire && t.recipients.iter().any(|r| r.0 == from))
            .max_by_key(|t| t.created)
            .map(|t| t.id)?;
        self.advance(id, Some(&from), status, now);
        Some(id)
    }

    /// Ids still Sent (no receipt) for longer than `timeout_s`: candidates for a resend.
    pub fn overdue(&self, now: u64, timeout_s: u64) -> Vec<u64> {
        self.tracks
            .iter()
            .filter(|t| {
                t.recipients.iter().any(|r| r.1 == Status::Sent) && now.saturating_sub(t.updated) > timeout_s
            })
            .map(|t| t.id)
            .collect()
    }

    /// Forgets finished tracks older than `keep_s`.
    pub fn prune(&mut self, now: u64, keep_s: u64) {
        self.tracks.retain(|t| {
            !(t.recipients
                .iter()
                .all(|r| r.1.is_final() || r.1 >= Status::Delivered)
                && now.saturating_sub(t.updated) > keep_s)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(s: &str) -> Vec<String> {
        vec![s.to_string()]
    }

    #[test]
    fn forward_only() {
        let mut t = Tracker::new();
        t.track(1, &call("B1"), 0);
        assert!(t.advance(1, None, Status::Sent, 1));
        assert!(!t.advance(1, None, Status::Queued, 2), "no regression");
        assert!(t.advance(1, Some("b1"), Status::Delivered, 3));
        assert!(t.advance(1, Some("B1"), Status::Read, 4));
        assert!(!t.advance(1, Some("B1"), Status::Failed, 5), "final stays final");
        assert_eq!(t.get(1).unwrap().overall(), Status::Read);
    }

    #[test]
    fn group_overall_is_slowest_live_member() {
        let mut t = Tracker::new();
        t.track(9, &["B1".into(), "C1".into(), "D1".into()], 0);
        t.advance(9, Some("B1"), Status::Read, 1);
        t.advance(9, Some("C1"), Status::Delivered, 1);
        t.advance(9, Some("D1"), Status::Expired, 1);
        let tr = t.get(9).unwrap();
        assert_eq!(tr.overall(), Status::Delivered);
        assert_eq!(tr.delivered_count(), 2);
        t.advance(9, Some("C1"), Status::Failed, 2);
        assert_eq!(t.get(9).unwrap().overall(), Status::Read, "only B1 is still live");
    }

    #[test]
    fn receipt_matching_and_overdue() {
        let mut t = Tracker::new();
        let id = 0xABCD_0000_0000_1234u64;
        t.track(id, &call("B1"), 10);
        t.advance(id, None, Status::Sent, 10);
        assert_eq!(t.overdue(100, 60), vec![id]);
        assert_eq!(
            t.on_receipt(&Receipt::Delivered { message_id: 0x1234 }, "C1", 20),
            None,
            "wrong station"
        );
        assert_eq!(
            t.on_receipt(&Receipt::Delivered { message_id: 0x1234 }, "b1", 20),
            Some(id)
        );
        assert!(t.overdue(100, 60).is_empty());
        t.prune(10_000, 100);
        assert!(t.get(id).is_none());
    }
}
