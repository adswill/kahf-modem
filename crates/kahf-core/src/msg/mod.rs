//! Messaging layer: everything above the session layer that makes KAHF feel like a modern
//! messenger on 17 m. All modules are pure state machines with an injected clock (`now`
//! in unix seconds) and a hand-rolled, versioned save format, so the daemon can drive them
//! from its worker thread and the UI can show their state without any I/O in here.
//!
//! - [`heard`]: stations heard on air (SNR, channel, relay path, last position and status).
//! - [`mailbox`]: store-and-forward for stations that are off air, released when they come back.
//! - [`receipts`]: delivery and read tracking for direct and group messages.
//! - [`grid`]: Maidenhead locators and the grid beacon, so stations learn where others are.
//! - [`groups`]: `#NAME` group chat with membership control lines and duplicate suppression.
//! - [`traffic`]: emergency / priority / routine classes and preemption rules.
//! - [`beacon`]: position and status beacon scheduling with stationary back-off.
//! - [`airtime`]: how long a file or image takes on air, and how big it may be for a time budget.
//! - [`messenger`]: the facade the daemon calls; see `docs/MESSAGING.md`.
//!
//! Radio rules: nothing here encrypts or hides content. Signatures (session layer) only prove
//! who sent a message. Callsign identification stays the daemon's job; [`beacon`] only reports
//! when an ID is due.
pub mod airtime;
pub mod beacon;
pub mod grid;
pub mod groups;
pub mod heard;
pub mod mailbox;
pub mod messenger;
pub mod receipts;
pub mod traffic;

use sha2::{Digest, Sha256};

/// Stable 64-bit identity of a message: SHA-256 over sender, addressee, send time and content.
pub fn message_id(from: &str, to: &str, sent_unix: u64, content: &[u8]) -> u64 {
    let mut h = Sha256::new();
    h.update(from.as_bytes());
    h.update([0]);
    h.update(to.as_bytes());
    h.update([0]);
    h.update(sent_unix.to_be_bytes());
    h.update(content);
    let d = h.finalize();
    u64::from_be_bytes(d[..8].try_into().unwrap())
}

/// 16-bit id used on air in receipts (`extras::Receipt`): the low 16 bits of the message id.
pub fn short_id(id: u64) -> u32 {
    (id & 0xFFFF) as u32
}

/// Escape tab, newline and backslash so a value fits one tab-separated line.
pub(crate) fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '\t' => o.push_str("\\t"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            c => o.push(c),
        }
    }
    o
}

pub(crate) fn unesc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('t') => o.push('\t'),
                Some('n') => o.push('\n'),
                Some('r') => o.push('\r'),
                Some(c) => o.push(c),
                None => {}
            }
        } else {
            o.push(c);
        }
    }
    o
}

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub(crate) fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_is_stable_and_sensitive() {
        let a = message_id("A1AA", "B2BB", 10, b"hi");
        assert_eq!(a, message_id("A1AA", "B2BB", 10, b"hi"));
        assert_ne!(a, message_id("A1AA", "B2BB", 11, b"hi"));
        assert_ne!(a, message_id("A1AA", "B2BC", 10, b"hi"));
        assert!(short_id(a) <= 0xFFFF);
    }

    #[test]
    fn escaping_round_trips() {
        let s = "a\tb\nc\\d\re";
        assert_eq!(unesc(&esc(s)), s);
        assert!(!esc(s).contains('\t') && !esc(s).contains('\n'));
        assert_eq!(unhex(&hex(&[0, 255, 16])), Some(vec![0, 255, 16]));
        assert_eq!(unhex("abc"), None);
    }
}
