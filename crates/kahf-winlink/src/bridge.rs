//! Mapping between kahf mail and Winlink messages.
//!
//! A kahf callsign and a Winlink callsign are the same thing, so `K1ABC` on kahf is
//! `K1ABC@winlink.org` on Winlink and nobody has to change address. Internet recipients are
//! written `user@example.com` on the kahf side and travel as `SMTP:` addresses.
//!
//! kahf sends a mail and each of its attachments as separate messages; Winlink carries them in
//! one. [`KahfMail`] is the joined form.

use crate::message::{Address, File, Message, MAX_MESSAGE_BYTES};
use std::fmt;

/// The mark that starts the signature block kahf appends to a signed mail body. Winlink
/// recipients cannot check it, so it is removed on the way out.
pub const KAHF_SIG_MARK: &str = "\n-----KAHF SIGNATURE-----\n";

/// A kahf mail with its attachments joined to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KahfMail {
    pub from: String,
    pub to: String,
    pub subject: String,
    pub body: String,
    pub files: Vec<(String, Vec<u8>)>,
}

/// Why a mail cannot be bridged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BridgeError {
    NoRecipient,
    BadRecipient(String),
    /// Body plus attachments exceed what Winlink accepts.
    TooLarge(usize),
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BridgeError::NoRecipient => f.write_str("the mail has no recipient"),
            BridgeError::BadRecipient(r) => write!(f, "{r:?} is not a callsign or an email address"),
            BridgeError::TooLarge(n) => {
                write!(
                    f,
                    "mail is {n} bytes; Winlink accepts at most {MAX_MESSAGE_BYTES}"
                )
            }
        }
    }
}

impl std::error::Error for BridgeError {}

/// Removes kahf's signature block from a body.
pub fn strip_signature(body: &str) -> &str {
    match body.rfind(KAHF_SIG_MARK) {
        Some(at) => &body[..at],
        None => body,
    }
}

/// True when `to` is meant for the Winlink network rather than a kahf station: an email
/// address, an explicit `SMTP:` address, or a `@winlink.org` address.
pub fn is_external(to: &str) -> bool {
    let t = to.trim();
    t.contains('@') || t.contains(':')
}

fn recipients(to: &str) -> Result<Vec<Address>, BridgeError> {
    let list: Vec<&str> = to
        .split([',', ';', ' '])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if list.is_empty() {
        return Err(BridgeError::NoRecipient);
    }
    list.into_iter()
        .map(|r| {
            let a = Address::parse(r);
            let ok = match &a.proto {
                None => {
                    a.addr.len() >= 3
                        && a.addr
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'/' || b == b'-')
                }
                Some(_) => a.addr.contains('@') && !a.addr.contains(char::is_whitespace),
            };
            if ok {
                Ok(a)
            } else {
                Err(BridgeError::BadRecipient(r.to_string()))
            }
        })
        .collect()
}

/// Builds the Winlink message for a kahf mail.
pub fn to_winlink(mail: &KahfMail, now: i64) -> Result<Message, BridgeError> {
    let to = recipients(&mail.to)?;
    let body = strip_signature(&mail.body).to_string();
    let size = body.len() + mail.files.iter().map(|(_, d)| d.len()).sum::<usize>();
    if size > MAX_MESSAGE_BYTES {
        return Err(BridgeError::TooLarge(size));
    }
    let mut m = Message::new(&mail.from, &[], &mail.subject, &body, now);
    m.to = to;
    m.files = mail
        .files
        .iter()
        .map(|(name, data)| File {
            name: name.clone(),
            data: data.clone(),
        })
        .collect();
    Ok(m)
}

/// The kahf mail for each of `served` (kahf callsigns this bridge delivers for) that the
/// Winlink message is addressed to, To or Cc. Mail for nobody served gives an empty list.
pub fn from_winlink(m: &Message, served: &[String]) -> Vec<KahfMail> {
    let mut out = Vec::new();
    for call in served {
        if m.is_for(call) {
            out.push(KahfMail {
                from: m.from.to_string(),
                to: call.to_ascii_uppercase(),
                subject: m.subject.clone(),
                body: m.body.clone(),
                files: m.files.iter().map(|f| (f.name.clone(), f.data.clone())).collect(),
            });
        }
    }
    out
}

/// Joins kahf's separate mail and file messages (the mail names its attachments, the files
/// follow) into [`KahfMail`]s. Files not named by a mail are kept as a mail of their own.
pub fn join(msgs: &[kahf_core::session::Message]) -> Vec<KahfMail> {
    use kahf_core::session::Message as K;
    let mut mails: Vec<(KahfMail, Vec<String>)> = Vec::new();
    for m in msgs {
        if let K::Mail {
            from,
            to,
            subject,
            body,
            attachments,
        } = m
        {
            mails.push((
                KahfMail {
                    from: from.clone(),
                    to: to.clone(),
                    subject: subject.clone(),
                    body: body.clone(),
                    files: Vec::new(),
                },
                attachments.clone(),
            ));
        }
    }
    for m in msgs {
        if let K::File { from, to, name, data } = m {
            match mails
                .iter_mut()
                .find(|(k, want)| &k.from == from && &k.to == to && want.contains(name))
            {
                Some((k, _)) => k.files.push((name.clone(), data.clone())),
                None => mails.push((
                    KahfMail {
                        from: from.clone(),
                        to: to.clone(),
                        subject: name.clone(),
                        body: String::new(),
                        files: vec![(name.clone(), data.clone())],
                    },
                    Vec::new(),
                )),
            }
        }
    }
    mails.into_iter().map(|(k, _)| k).collect()
}

/// Splits a [`KahfMail`] back into kahf's mail message and one file message per attachment.
pub fn split(mail: &KahfMail) -> Vec<kahf_core::session::Message> {
    use kahf_core::session::Message as K;
    let mut v = vec![K::Mail {
        from: mail.from.clone(),
        to: mail.to.clone(),
        subject: mail.subject.clone(),
        body: mail.body.clone(),
        attachments: mail.files.iter().map(|(n, _)| n.clone()).collect(),
    }];
    v.extend(mail.files.iter().map(|(n, d)| K::File {
        from: mail.from.clone(),
        to: mail.to.clone(),
        name: n.clone(),
        data: d.clone(),
    }));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mail(to: &str) -> KahfMail {
        KahfMail {
            from: "K1ABC".into(),
            to: to.into(),
            subject: "Field day".into(),
            body: format!("see you there{KAHF_SIG_MARK}deadbeef\n"),
            files: vec![("plan.txt".into(), b"bring water".to_vec())],
        }
    }

    #[test]
    fn same_callsign_both_sides() {
        let m = to_winlink(&mail("n0call"), 1_800_000_000).unwrap();
        assert_eq!(m.to, vec![Address::call("N0CALL")]);
        assert_eq!(m.body, "see you there");
        assert_eq!(m.files.len(), 1);
        let back = from_winlink(
            &Message::parse(&m.to_bytes()).unwrap(),
            &["N0CALL".into(), "W1AW".into()],
        );
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].to, "N0CALL");
        assert_eq!(back[0].from, "K1ABC");
        assert_eq!(back[0].files[0].1, b"bring water");
    }

    #[test]
    fn internet_and_multiple_recipients() {
        let m = to_winlink(&mail("bob@example.com, W1AW"), 0).unwrap();
        assert_eq!(m.to[0].to_string(), "SMTP:bob@example.com");
        assert_eq!(m.to[1], Address::call("W1AW"));
        assert!(is_external("bob@example.com"));
        assert!(is_external("K1ABC@winlink.org"));
        assert!(!is_external("K1ABC"));
    }

    #[test]
    fn refuses_bad_and_oversized() {
        assert_eq!(to_winlink(&mail(" "), 0), Err(BridgeError::NoRecipient));
        assert!(matches!(
            to_winlink(&mail("not a call!"), 0),
            Err(BridgeError::BadRecipient(_))
        ));
        let mut big = mail("N0CALL");
        big.files[0].1 = vec![0; MAX_MESSAGE_BYTES];
        assert!(matches!(to_winlink(&big, 0), Err(BridgeError::TooLarge(_))));
    }

    #[test]
    fn sender_from_the_internet_keeps_its_address() {
        let mut w = Message::new("bob@example.com", &["K1ABC"], "hi", "yo", 0);
        w.from = Address::parse("bob@example.com");
        let k = from_winlink(&w, &["K1ABC".into()]);
        assert_eq!(k[0].from, "SMTP:bob@example.com");
        assert!(from_winlink(&w, &["W1AW".into()]).is_empty());
    }

    #[test]
    fn join_and_split_round_trip() {
        let k = mail("N0CALL");
        let parts = split(&k);
        assert_eq!(parts.len(), 2);
        assert_eq!(join(&parts), vec![k]);
    }
}
