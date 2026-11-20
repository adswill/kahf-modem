//! Traffic classes and preemption. Emergency traffic goes first, may interrupt a lower-class
//! transmission between frames, is flooded to every relay path and is kept longest in mailboxes.
//! Nothing here encrypts or disguises content: emergency text is plain and callsign-identified.
use crate::outbox::Priority;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    /// Life or property at risk. Requires the two-step confirmation in `payload_urgent`.
    Emergency,
    /// Time-sensitive but not an emergency (welfare, logistics).
    Priority,
    Routine,
    /// Bulk: files, images, bulletins.
    Bulk,
}

impl Class {
    pub fn outbox_priority(self) -> Priority {
        match self {
            Class::Emergency => Priority::Urgent,
            Class::Priority | Class::Routine => Priority::Normal,
            Class::Bulk => Priority::Low,
        }
    }

    /// Default class for a session message kind: files are bulk, chat and mail routine.
    pub fn default_for(kind: Kind) -> Class {
        match kind {
            Kind::File => Class::Bulk,
            Kind::Chat | Kind::Mail => Class::Routine,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Class::Emergency => "EMERGENCY",
            Class::Priority => "PRIORITY",
            Class::Routine => "ROUTINE",
            Class::Bulk => "BULK",
        }
    }

    /// Relays should flood (try every path) rather than pick one.
    pub fn flood(self) -> bool {
        self == Class::Emergency
    }

    /// Receipts are always requested for emergency and priority traffic.
    pub fn receipt_required(self) -> bool {
        self <= Class::Priority
    }

    /// Retries before giving up on a direct send.
    pub fn max_attempts(self) -> u32 {
        match self {
            Class::Emergency => 20,
            Class::Priority => 12,
            Class::Routine => 8,
            Class::Bulk => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Chat,
    Mail,
    File,
}

/// May `incoming` interrupt a transmission of class `current` at a frame boundary?
/// Only emergency traffic preempts, and never another emergency.
pub fn preempts(incoming: Class, current: Class) -> bool {
    incoming == Class::Emergency && current != Class::Emergency
}

/// Rate limiter for unattended relays: bulk traffic is refused past `bulk_per_hour`, everything
/// else (and all emergency traffic) is always accepted so a flood of files cannot block alerts.
pub fn relay_accepts(class: Class, bulk_this_hour: u32, bulk_per_hour: u32) -> bool {
    class != Class::Bulk || bulk_this_hour < bulk_per_hour
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordering_and_mapping() {
        assert!(
            Class::Emergency < Class::Priority
                && Class::Priority < Class::Routine
                && Class::Routine < Class::Bulk
        );
        assert_eq!(Class::Emergency.outbox_priority(), Priority::Urgent);
        assert_eq!(Class::Bulk.outbox_priority(), Priority::Low);
        assert_eq!(Class::default_for(Kind::File), Class::Bulk);
        assert!(Class::Priority.receipt_required() && !Class::Routine.receipt_required());
        assert!(Class::Emergency.flood() && !Class::Priority.flood());
        assert!(Class::Emergency.max_attempts() > Class::Bulk.max_attempts());
    }

    #[test]
    fn preemption_and_relay_limits() {
        assert!(preempts(Class::Emergency, Class::Bulk) && !preempts(Class::Emergency, Class::Emergency));
        assert!(!preempts(Class::Priority, Class::Bulk));
        assert!(relay_accepts(Class::Emergency, 999, 5) && relay_accepts(Class::Routine, 999, 5));
        assert!(relay_accepts(Class::Bulk, 4, 5) && !relay_accepts(Class::Bulk, 5, 5));
    }
}
