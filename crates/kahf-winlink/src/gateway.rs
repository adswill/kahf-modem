//! The kahf side of the bridge: what a kahf station does with mail that crosses to or from
//! Winlink. It only touches the on-disk [`Spool`]; the CMS sync is a separate process
//! (`kahf-winlink gateway`) working on the same folder, so no network code runs in a radio
//! daemon.
//!
//! Out: a kahf mail (with its attachments, which arrive as separate files) from an approved
//! callsign, addressed to an email or `@winlink.org` address, is joined and queued in the
//! sender's `out` folder. Plain callsigns are never taken: those are kahf stations, reached
//! by radio.
//!
//! In: Winlink mail waiting in a served callsign's `in` folder is handed out as [`KahfMail`]
//! and moved to `archive` once the station has queued it.

use crate::bridge::{self, KahfMail};
use crate::spool::{Folder, Policy, Spool};
use kahf_core::session::Message as Kahf;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

/// How long a mail waits for its attachments before it is dropped, in seconds.
pub const PENDING_SECS: i64 = 3600;

/// What the gateway did with a received kahf message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Taken {
    /// Not for the gateway: the station handles it as usual.
    No,
    /// Taken, waiting for attachments.
    Held,
    /// Taken and queued for Winlink under this message id.
    Queued(String),
    /// Taken but could not be queued (the reason).
    Refused(String),
}

struct Pending {
    mail: KahfMail,
    want: Vec<String>,
    since: i64,
}

pub struct WinlinkGateway {
    spool: Mutex<Spool>,
    approved: Vec<String>,
    pending: Mutex<HashMap<(String, String), Pending>>,
}

impl WinlinkGateway {
    /// `approved` are the kahf callsigns whose mail this station forwards to Winlink, and
    /// whose Winlink mail it delivers. Nobody else's mail is touched.
    pub fn new(root: &Path, approved: &[String]) -> std::io::Result<WinlinkGateway> {
        let approved: Vec<String> = approved.iter().map(|c| c.trim().to_ascii_uppercase()).collect();
        Ok(WinlinkGateway {
            spool: Mutex::new(Spool::open(root, &approved, Policy::All)?),
            approved,
            pending: Mutex::new(HashMap::new()),
        })
    }

    pub fn approved(&self) -> &[String] {
        &self.approved
    }

    fn is_approved(&self, call: &str) -> bool {
        self.approved.iter().any(|a| a.eq_ignore_ascii_case(call))
    }

    /// Offers a received kahf message. `now` is unix seconds.
    pub fn take(&self, msg: &Kahf, now: i64) -> Taken {
        let mut pending = self.pending.lock().unwrap_or_else(|p| p.into_inner());
        pending.retain(|_, p| now - p.since < PENDING_SECS);
        match msg {
            Kahf::Mail {
                from,
                to,
                subject,
                body,
                attachments,
            } if self.is_approved(from) && bridge::is_external(to) => {
                let mail = KahfMail {
                    from: from.to_ascii_uppercase(),
                    to: to.clone(),
                    subject: subject.clone(),
                    body: body.clone(),
                    files: Vec::new(),
                };
                if attachments.is_empty() {
                    return self.finish(mail, now);
                }
                let key = (from.to_ascii_uppercase(), to.clone());
                pending.insert(
                    key,
                    Pending {
                        mail,
                        want: attachments.clone(),
                        since: now,
                    },
                );
                Taken::Held
            }
            Kahf::File { from, to, name, data } => {
                let key = (from.to_ascii_uppercase(), to.clone());
                let Some(p) = pending.get_mut(&key) else {
                    return Taken::No;
                };
                let Some(i) = p.want.iter().position(|w| w == name) else {
                    return Taken::No;
                };
                p.want.remove(i);
                p.mail.files.push((name.clone(), data.clone()));
                if p.want.is_empty() {
                    let done = pending.remove(&key).map(|p| p.mail);
                    return match done {
                        Some(mail) => self.finish(mail, now),
                        None => Taken::No,
                    };
                }
                Taken::Held
            }
            _ => Taken::No,
        }
    }

    fn finish(&self, mail: KahfMail, now: i64) -> Taken {
        match bridge::to_winlink(&mail, now) {
            Err(e) => Taken::Refused(e.to_string()),
            Ok(m) => {
                let spool = self.spool.lock().unwrap_or_else(|p| p.into_inner());
                match spool.queue(&mail.from, &m) {
                    Ok(()) => Taken::Queued(m.mid),
                    Err(e) => Taken::Refused(format!("cannot queue the mail: {e}")),
                }
            }
        }
    }

    /// Hands every waiting Winlink mail for an approved callsign to `deliver`; those it
    /// accepts (returns `Ok`) move to `archive`. Returns how many were delivered.
    pub fn pump_inbound<F>(&self, mut deliver: F) -> usize
    where
        F: FnMut(&KahfMail) -> Result<(), String>,
    {
        let spool = self.spool.lock().unwrap_or_else(|p| p.into_inner());
        let mut n = 0;
        for call in &self.approved {
            for m in spool.list(call, Folder::In) {
                for k in bridge::from_winlink(&m, std::slice::from_ref(call)) {
                    if deliver(&k).is_ok() {
                        spool.mv(call, &m.mid, Folder::In, Folder::Archive);
                        n += 1;
                    }
                }
            }
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Message;

    fn root(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("kahf-winlink-gw-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn mail(from: &str, to: &str, attachments: &[&str]) -> Kahf {
        Kahf::Mail {
            from: from.into(),
            to: to.into(),
            subject: "hi".into(),
            body: "body".into(),
            attachments: attachments.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn takes_only_approved_senders_and_external_recipients() {
        let r = root("take");
        let g = WinlinkGateway::new(&r, &["K1ABC".into()]).unwrap();
        assert!(matches!(
            g.take(&mail("K1ABC", "bob@example.com", &[]), 0),
            Taken::Queued(_)
        ));
        assert_eq!(g.take(&mail("K1ABC", "N0CALL", &[]), 0), Taken::No);
        assert_eq!(g.take(&mail("W1AW", "bob@example.com", &[]), 0), Taken::No);
        assert!(matches!(
            g.take(&mail("k1abc", "SMTP:bob@example.com", &[]), 0),
            Taken::Queued(_)
        ));
        let _ = std::fs::remove_dir_all(r);
    }

    #[test]
    fn joins_attachments_and_drops_stale_mail() {
        let r = root("join");
        let g = WinlinkGateway::new(&r, &["K1ABC".into()]).unwrap();
        let to = "bob@example.com";
        assert_eq!(g.take(&mail("K1ABC", to, &["a.txt", "b.txt"]), 0), Taken::Held);
        let file = |n: &str| Kahf::File {
            from: "K1ABC".into(),
            to: to.into(),
            name: n.into(),
            data: n.as_bytes().to_vec(),
        };
        assert_eq!(g.take(&file("a.txt"), 1), Taken::Held);
        assert_eq!(g.take(&file("other.txt"), 1), Taken::No);
        let Taken::Queued(mid) = g.take(&file("b.txt"), 2) else {
            panic!("not queued")
        };
        let spool = g.spool.lock().unwrap();
        let out = spool.list("K1ABC", Folder::Out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].mid, mid);
        assert_eq!(out[0].files.len(), 2);
        drop(spool);
        // A mail whose files never come is forgotten.
        assert_eq!(g.take(&mail("K1ABC", to, &["x"]), 10), Taken::Held);
        assert_eq!(
            g.take(
                &Kahf::File {
                    from: "K1ABC".into(),
                    to: to.into(),
                    name: "x".into(),
                    data: vec![]
                },
                10 + PENDING_SECS + 1
            ),
            Taken::No
        );
        let _ = std::fs::remove_dir_all(r);
    }

    #[test]
    fn inbound_winlink_mail_is_delivered_once() {
        let r = root("in");
        let g = WinlinkGateway::new(&r, &["K1ABC".into()]).unwrap();
        {
            let mut s = Spool::open(&r, &["K1ABC".into()], Policy::All).unwrap();
            use crate::b2f::Mailbox;
            s.deliver(Message::new(
                "bob@example.com",
                &["K1ABC"],
                "from the internet",
                "hello",
                0,
            ))
            .unwrap();
        }
        let mut seen = Vec::new();
        assert_eq!(
            g.pump_inbound(|m| {
                seen.push(m.clone());
                Ok(())
            }),
            1
        );
        assert_eq!(seen[0].to, "K1ABC");
        assert_eq!(g.pump_inbound(|_| Ok(())), 0);
        // A refusal leaves the mail waiting.
        {
            let mut s = Spool::open(&r, &["K1ABC".into()], Policy::All).unwrap();
            use crate::b2f::Mailbox;
            s.deliver(Message::new("bob@example.com", &["K1ABC"], "again", "hello", 1))
                .unwrap();
        }
        assert_eq!(g.pump_inbound(|_| Err("busy".into())), 0);
        assert_eq!(g.pump_inbound(|_| Ok(())), 1);
        let _ = std::fs::remove_dir_all(r);
    }
}
