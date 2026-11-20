//! A mailbox on disk, laid out like Pat's: `<root>/<CALL>/{in,out,sent,archive}/<MID>.b2f`,
//! each file one message in the Winlink wire format. Pat and Winlink Express exports use the
//! same files, so an existing Pat mailbox can be pointed at as it is: nothing is converted and
//! nothing is lost.
//!
//! [`Spool`] is the [`Mailbox`] a B2F session reads and writes. Mail for a callsign this spool
//! serves lands in that callsign's `in`; mail waiting in any served `out` is sent, and moves to
//! `sent` once the other side has it.

use crate::b2f::{Answer, Mailbox, Offer};
use crate::message::{Address, Message};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The four folders of one callsign's mailbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Folder {
    In,
    Out,
    Sent,
    Archive,
}

impl Folder {
    fn dir(self) -> &'static str {
        match self {
            Folder::In => "in",
            Folder::Out => "out",
            Folder::Sent => "sent",
            Folder::Archive => "archive",
        }
    }
}

/// Which messages go out in a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// Everything in the served `out` folders (a CMS takes mail for anyone).
    All,
    /// Only messages addressed to the callsigns the other station says it receives for
    /// (peer-to-peer).
    ForPeer,
}

pub struct Spool {
    root: PathBuf,
    served: Vec<String>,
    policy: Policy,
    /// Messages delivered during this session, as (callsign, message id).
    pub delivered: Vec<(String, String)>,
}

fn safe_name(mid: &str) -> Option<String> {
    (!mid.is_empty()
        && mid.len() <= 32
        && mid
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
    .then(|| mid.to_string())
}

impl Spool {
    /// Opens (and creates the folders of) the mailboxes of `served` callsigns under `root`.
    pub fn open(root: &Path, served: &[String], policy: Policy) -> io::Result<Spool> {
        let served: Vec<String> = served.iter().map(|c| c.trim().to_ascii_uppercase()).collect();
        for c in &served {
            for f in [Folder::In, Folder::Out, Folder::Sent, Folder::Archive] {
                fs::create_dir_all(root.join(c).join(f.dir()))?;
            }
        }
        Ok(Spool {
            root: root.to_path_buf(),
            served,
            policy,
            delivered: Vec::new(),
        })
    }

    pub fn served(&self) -> &[String] {
        &self.served
    }

    fn path(&self, call: &str, folder: Folder, mid: &str) -> PathBuf {
        self.root.join(call).join(folder.dir()).join(format!("{mid}.b2f"))
    }

    /// Writes a message into a folder (atomically: a temporary file, then a rename).
    pub fn put(&self, call: &str, folder: Folder, m: &Message) -> io::Result<()> {
        let name =
            safe_name(&m.mid).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad message id"))?;
        let dest = self.path(call, folder, &name);
        let tmp = dest.with_extension("tmp");
        fs::write(&tmp, m.to_bytes())?;
        fs::rename(tmp, dest)
    }

    /// Messages in a folder, oldest first. Files that do not parse are skipped.
    pub fn list(&self, call: &str, folder: Folder) -> Vec<Message> {
        let dir = self.root.join(call).join(folder.dir());
        let mut v: Vec<Message> = fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "b2f"))
            .filter_map(|e| fs::read(e.path()).ok())
            .filter_map(|b| Message::parse(&b).ok())
            .collect();
        v.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| a.mid.cmp(&b.mid)));
        v
    }

    /// Moves a message between folders of one callsign. False when it is not there.
    pub fn mv(&self, call: &str, mid: &str, from: Folder, to: Folder) -> bool {
        match safe_name(mid) {
            Some(n) => fs::rename(self.path(call, from, &n), self.path(call, to, &n)).is_ok(),
            None => false,
        }
    }

    /// True when any served mailbox already holds the message id, in any folder.
    pub fn has(&self, mid: &str) -> bool {
        let Some(n) = safe_name(mid) else { return false };
        self.served.iter().any(|c| {
            [Folder::In, Folder::Out, Folder::Sent, Folder::Archive]
                .iter()
                .any(|f| self.path(c, *f, &n).exists())
        })
    }

    /// Queues a message for sending from `call`'s mailbox.
    pub fn queue(&self, call: &str, m: &Message) -> io::Result<()> {
        self.put(&call.to_ascii_uppercase(), Folder::Out, m)
    }
}

impl Mailbox for Spool {
    fn outbound(&mut self, fw: &[Address]) -> Vec<Message> {
        let mut v = Vec::new();
        for c in &self.served {
            for m in self.list(c, Folder::Out) {
                let wanted = match self.policy {
                    Policy::All => true,
                    Policy::ForPeer => m.to.iter().chain(m.cc.iter()).any(|a| fw.contains(a)),
                };
                if wanted {
                    v.push(m);
                }
            }
        }
        v
    }

    fn answer(&mut self, o: &Offer) -> Answer {
        match safe_name(&o.mid) {
            None => Answer::Defer,
            Some(_) if self.has(&o.mid) => Answer::Reject,
            Some(_) => Answer::Accept,
        }
    }

    fn deliver(&mut self, m: Message) -> Result<(), String> {
        // Into every served mailbox it is addressed to; otherwise the first (a session with
        // one callsign, or mail the sender copied to us without naming us).
        let mut targets: Vec<String> = self.served.iter().filter(|c| m.is_for(c)).cloned().collect();
        if targets.is_empty() {
            targets.extend(self.served.first().cloned());
        }
        for c in targets {
            self.put(&c, Folder::In, &m).map_err(|e| e.to_string())?;
            self.delivered.push((c, m.mid.clone()));
        }
        Ok(())
    }

    fn sent(&mut self, mid: &str, _rejected: bool) {
        // A rejected message is one the other side already has: it counts as sent too.
        for c in self.served.clone() {
            if self.mv(&c, mid, Folder::Out, Folder::Sent) {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("kahf-winlink-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn folders_files_and_moves() {
        let root = tmp("spool");
        let s = Spool::open(&root, &["k1abc".into()], Policy::All).unwrap();
        let m = Message::new("K1ABC", &["N0CALL"], "hi", "body", 1_800_000_000);
        s.queue("K1ABC", &m).unwrap();
        assert!(root.join("K1ABC/out").join(format!("{}.b2f", m.mid)).exists());
        assert!(s.has(&m.mid));
        assert_eq!(s.list("K1ABC", Folder::Out).len(), 1);
        assert!(s.mv("K1ABC", &m.mid, Folder::Out, Folder::Sent));
        assert_eq!(s.list("K1ABC", Folder::Sent)[0].subject, "hi");
        assert!(!s.mv("K1ABC", "../etc/passwd", Folder::Out, Folder::Sent));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn delivers_to_each_served_recipient_and_dedupes() {
        let root = tmp("deliver");
        let mut s = Spool::open(&root, &["K1ABC".into(), "W1AW".into()], Policy::All).unwrap();
        let mut m = Message::new("N0CALL", &["W1AW"], "hello", "x", 0);
        m.cc.push(Address::call("K1ABC"));
        let offer = Offer {
            mid: m.mid.clone(),
            kind: "EM".into(),
            size: 1,
            compressed: 1,
        };
        assert_eq!(s.answer(&offer), Answer::Accept);
        s.deliver(m.clone()).unwrap();
        assert_eq!(s.list("K1ABC", Folder::In).len(), 1);
        assert_eq!(s.list("W1AW", Folder::In).len(), 1);
        assert_eq!(s.answer(&offer), Answer::Reject);
        let bad = Offer {
            mid: "../x".into(),
            ..offer
        };
        assert_eq!(s.answer(&bad), Answer::Defer);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn peer_policy_sends_only_what_the_peer_receives_for() {
        let root = tmp("peer");
        let mut s = Spool::open(&root, &["K1ABC".into()], Policy::ForPeer).unwrap();
        s.queue("K1ABC", &Message::new("K1ABC", &["N0CALL"], "to peer", "x", 0))
            .unwrap();
        s.queue(
            "K1ABC",
            &Message::new("K1ABC", &["W1AW"], "to someone else", "x", 1),
        )
        .unwrap();
        let out = s.outbound(&[Address::call("N0CALL")]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].subject, "to peer");
        let _ = fs::remove_dir_all(root);
    }
}
