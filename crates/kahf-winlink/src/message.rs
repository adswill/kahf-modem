//! The Winlink message format (the payload of a B2F proposal): a header block, a body and any
//! attachments, all with CRLF line ends. Everything is plain text and bytes; nothing is
//! encrypted, as amateur radio rules require.

use md5::{Digest, Md5};
use std::fmt;

/// Longest message id the protocol allows.
pub const MID_LEN: usize = 12;
/// Winlink limits a message (body plus attachments) to about 120 kB over the air.
pub const MAX_MESSAGE_BYTES: usize = 120_000;

/// A Winlink address: a callsign (the short form of `CALL@winlink.org`) or an `SMTP:` address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    /// `None` for a Winlink callsign, `Some("SMTP")` for an internet address.
    pub proto: Option<String>,
    pub addr: String,
}

impl Address {
    /// Parses `CALL`, `CALL@winlink.org`, `user@example.com` or `PROTO:addr`.
    pub fn parse(s: &str) -> Address {
        let s = s.trim();
        if let Some((proto, addr)) = s.split_once(':') {
            if !proto.is_empty() && !addr.contains(':') {
                return Address {
                    proto: Some(proto.to_ascii_uppercase()),
                    addr: addr.to_string(),
                };
            }
        }
        match s.split_once('@') {
            None => Address {
                proto: None,
                addr: s.to_ascii_uppercase(),
            },
            Some((call, host)) if host.eq_ignore_ascii_case("winlink.org") => Address {
                proto: None,
                addr: call.to_ascii_uppercase(),
            },
            Some(_) => Address {
                proto: Some("SMTP".into()),
                addr: s.to_string(),
            },
        }
    }

    /// A Winlink callsign address.
    pub fn call(call: &str) -> Address {
        Address {
            proto: None,
            addr: call.trim().to_ascii_uppercase(),
        }
    }

    /// True for an address on the Winlink network (no internet protocol prefix).
    pub fn is_winlink(&self) -> bool {
        self.proto.is_none()
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.proto {
            None => f.write_str(&self.addr),
            Some(p) => write!(f, "{p}:{}", self.addr),
        }
    }
}

/// One attachment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    pub name: String,
    pub data: Vec<u8>,
}

/// A Winlink message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub mid: String,
    /// Seconds since the Unix epoch (UTC). The wire format has minute resolution.
    pub date: i64,
    /// Usually `Private`.
    pub kind: String,
    pub from: Address,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub subject: String,
    /// The mailbox operator that first took the message in.
    pub mbo: String,
    pub body: String,
    pub files: Vec<File>,
}

/// Why a message could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bad Winlink message: {}", self.0)
    }
}

impl std::error::Error for ParseError {}

fn bad<T>(why: &str) -> Result<T, ParseError> {
    Err(ParseError(why.to_string()))
}

impl Message {
    /// A new private message from `from` to `to`, with a fresh message id.
    pub fn new(from: &str, to: &[&str], subject: &str, body: &str, now: i64) -> Message {
        Message {
            mid: generate_mid(from, now),
            date: now,
            kind: "Private".into(),
            from: Address::parse(from),
            to: to.iter().map(|t| Address::parse(t)).collect(),
            cc: Vec::new(),
            subject: subject.to_string(),
            mbo: from.trim().to_ascii_uppercase(),
            body: body.to_string(),
            files: Vec::new(),
        }
    }

    /// Serialises to the wire format.
    pub fn to_bytes(&self) -> Vec<u8> {
        let body = self.body.replace("\r\n", "\n").replace('\n', "\r\n");
        let mut h = String::new();
        let mut line = |k: &str, v: &str| {
            h.push_str(k);
            h.push_str(": ");
            h.push_str(v);
            h.push_str("\r\n");
        };
        line("Mid", &self.mid);
        line("Body", &body.len().to_string());
        line("Content-Transfer-Encoding", "8bit");
        line("Content-Type", "text/plain; charset=UTF-8");
        line("Date", &format_date(self.date));
        for c in &self.cc {
            line("Cc", &c.to_string());
        }
        for f in &self.files {
            line("File", &format!("{} {}", f.data.len(), encode_word(&f.name)));
        }
        line("From", &self.from.to_string());
        line("Mbo", &self.mbo);
        line("Subject", &encode_word(&self.subject));
        for t in &self.to {
            line("To", &t.to_string());
        }
        line("Type", &self.kind);
        let mut out = h.into_bytes();
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(body.as_bytes());
        if !self.files.is_empty() {
            out.extend_from_slice(b"\r\n");
        }
        for f in &self.files {
            out.extend_from_slice(&f.data);
            out.extend_from_slice(b"\r\n");
        }
        out
    }

    /// Parses the wire format.
    pub fn parse(data: &[u8]) -> Result<Message, ParseError> {
        let mut pos = 0;
        while pos < data.len() && data[pos].is_ascii_whitespace() {
            pos += 1;
        }
        let mut headers: Vec<(String, String)> = Vec::new();
        loop {
            let end = match data[pos..].iter().position(|&b| b == b'\n') {
                Some(i) => pos + i,
                None => return bad("header block does not end"),
            };
            let line = String::from_utf8_lossy(&data[pos..end])
                .trim_end_matches('\r')
                .to_string();
            pos = end + 1;
            if line.is_empty() {
                break;
            }
            if line.starts_with([' ', '\t']) {
                if let Some(last) = headers.last_mut() {
                    last.1.push(' ');
                    last.1.push_str(line.trim());
                }
                continue;
            }
            match line.split_once(':') {
                Some((k, v)) => headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string())),
                None => return bad("header line without a colon"),
            }
        }
        let get = |k: &str| headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        let all = |k: &str| -> Vec<&str> {
            headers
                .iter()
                .filter(|(n, _)| n == k)
                .map(|(_, v)| v.as_str())
                .collect()
        };
        let mid = get("mid")
            .ok_or_else(|| ParseError("missing Mid".into()))?
            .to_string();
        let body_len: usize = get("body").and_then(|b| b.parse().ok()).unwrap_or(0);
        let section = |pos: &mut usize, n: usize| -> Result<Vec<u8>, ParseError> {
            if pos.saturating_add(n) > data.len() {
                return bad("section is shorter than its header says");
            }
            let s = data[*pos..*pos + n].to_vec();
            *pos += n;
            if data[*pos..].starts_with(b"\r\n") {
                *pos += 2;
            } else if *pos < data.len() {
                return bad("section is not followed by CRLF");
            }
            Ok(s)
        };
        let body_bytes = section(&mut pos, body_len)?;
        let charset = get("content-type")
            .and_then(|ct| {
                ct.split(';')
                    .skip(1)
                    .find_map(|p| p.trim().strip_prefix("charset="))
            })
            .map(|c| c.trim_matches('"').to_ascii_lowercase())
            .unwrap_or_else(|| "iso-8859-1".into());
        let body = decode_text(&body_bytes, &charset);
        let mut files = Vec::new();
        for f in all("file") {
            let (size, name) = f
                .split_once(' ')
                .ok_or_else(|| ParseError("bad File header".into()))?;
            let size: usize = size.parse().map_err(|_| ParseError("bad File size".into()))?;
            files.push(File {
                name: decode_header(name),
                data: section(&mut pos, size)?,
            });
        }
        let date = get("date").and_then(parse_date).unwrap_or(0);
        Ok(Message {
            mid,
            date,
            kind: get("type").unwrap_or("Private").to_string(),
            from: Address::parse(get("from").unwrap_or("")),
            to: all("to").into_iter().map(Address::parse).collect(),
            cc: all("cc").into_iter().map(Address::parse).collect(),
            subject: decode_header(get("subject").unwrap_or("")),
            mbo: get("mbo").unwrap_or("").to_string(),
            body: body.replace("\r\n", "\n"),
            files,
        })
    }

    /// True if any recipient (To or Cc) is the given Winlink callsign.
    pub fn is_for(&self, call: &str) -> bool {
        let want = Address::call(call);
        self.to.iter().chain(self.cc.iter()).any(|a| *a == want)
    }
}

/// A fresh message id: 12 characters of base32 from a hash of the sender, time and a counter.
pub fn generate_mid(call: &str, now: i64) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let mut h = Md5::new();
    h.update(format!("{now}-{nanos}-{n}-{call}").as_bytes());
    let sum = h.finalize();
    const ALPHA: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut bits = 0u32;
    let mut nbits = 0;
    let mut out = String::new();
    for b in sum.iter() {
        bits = (bits << 8) | u32::from(*b);
        nbits += 8;
        while nbits >= 5 && out.len() < MID_LEN {
            nbits -= 5;
            out.push(ALPHA[((bits >> nbits) & 31) as usize] as char);
        }
    }
    out
}

// ---- dates --------------------------------------------------------------------------------

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `YYYY/MM/DD HH:MM` in UTC, the Winlink date format.
pub fn format_date(t: i64) -> String {
    let (y, m, d) = civil_from_days(t.div_euclid(86_400));
    let s = t.rem_euclid(86_400);
    format!("{y:04}/{m:02}/{d:02} {:02}:{:02}", s / 3600, s % 3600 / 60)
}

/// Reads the Winlink date and the variants seen in the field.
pub fn parse_date(s: &str) -> Option<i64> {
    let digits: Vec<i64> = {
        let s = s.trim();
        if s.len() == 14 && s.bytes().all(|b| b.is_ascii_digit()) {
            // 20060102150405
            [&s[0..4], &s[4..6], &s[6..8], &s[8..10], &s[10..12]]
                .iter()
                .map(|p| p.parse().ok())
                .collect::<Option<_>>()?
        } else {
            s.split(|c: char| !c.is_ascii_digit())
                .filter(|p| !p.is_empty())
                .take(5)
                .map(|p| p.parse().ok())
                .collect::<Option<_>>()?
        }
    };
    if digits.len() < 5 || !(1..=12).contains(&digits[1]) || !(1..=31).contains(&digits[2]) {
        return None;
    }
    Some(days_from_civil(digits[0], digits[1], digits[2]) * 86_400 + digits[3] * 3600 + digits[4] * 60)
}

// ---- text encodings -----------------------------------------------------------------------

fn decode_text(bytes: &[u8], charset: &str) -> String {
    match charset {
        "utf-8" | "utf8" | "us-ascii" | "ascii" => match std::str::from_utf8(bytes) {
            Ok(s) => s.to_string(),
            Err(_) => bytes.iter().map(|&b| b as char).collect(),
        },
        // ISO-8859-1 is the Winlink default; it maps byte to code point directly.
        _ => bytes.iter().map(|&b| b as char).collect(),
    }
}

/// Encodes a header value as RFC 2047 Q words when it has anything outside printable ASCII.
pub fn encode_word(s: &str) -> String {
    if s.bytes().all(|b| (0x20..0x7f).contains(&b)) {
        return s.to_string();
    }
    let mut out = String::from("=?utf-8?q?");
    for b in s.bytes() {
        match b {
            b' ' => out.push('_'),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => out.push(b as char),
            _ => out.push_str(&format!("={b:02X}")),
        }
    }
    out.push_str("?=");
    out
}

/// Decodes RFC 2047 words; plain values are taken as UTF-8, or ISO-8859-1 when they are not
/// valid UTF-8 (RMS Express sends raw ISO-8859-1 in the Subject).
pub fn decode_header(s: &str) -> String {
    if !s.contains("=?") {
        return s.to_string();
    }
    let mut out = String::new();
    let mut rest = s;
    let mut last_was_word = false;
    while let Some(start) = rest.find("=?") {
        let (before, tail) = rest.split_at(start);
        match decode_one_word(tail) {
            Some((text, used)) => {
                if !(last_was_word && before.trim().is_empty()) {
                    out.push_str(before);
                }
                out.push_str(&text);
                rest = &tail[used..];
                last_was_word = true;
            }
            None => {
                out.push_str(before);
                out.push_str("=?");
                rest = &tail[2..];
                last_was_word = false;
            }
        }
    }
    out.push_str(rest);
    out
}

fn decode_one_word(s: &str) -> Option<(String, usize)> {
    let inner = s.strip_prefix("=?")?;
    let end = inner.find("?=")?;
    let word = &inner[..end];
    let mut parts = word.splitn(3, '?');
    let charset = parts.next()?.to_ascii_lowercase();
    let enc = parts.next()?;
    let text = parts.next()?;
    let bytes = match enc {
        "q" | "Q" => {
            let b = text.as_bytes();
            let mut v = Vec::new();
            let mut i = 0;
            while i < b.len() {
                match b[i] {
                    b'_' => v.push(b' '),
                    b'=' if i + 2 < b.len() => {
                        v.push(u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).ok()?, 16).ok()?);
                        i += 2;
                    }
                    c => v.push(c),
                }
                i += 1;
            }
            v
        }
        "b" | "B" => base64_decode(text)?,
        _ => return None,
    };
    Some((decode_text(&bytes, &charset), 2 + end + 2))
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acc, mut n) = (0u32, 0);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        n += 6;
        if n >= 8 {
            n -= 8;
            out.push((acc >> n) as u8);
            acc &= (1 << n) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        assert_eq!(Address::parse("k1abc"), Address::call("K1ABC"));
        assert_eq!(Address::parse("k1abc@Winlink.org"), Address::call("K1ABC"));
        let a = Address::parse("bob@example.com");
        assert_eq!(a.to_string(), "SMTP:bob@example.com");
        assert!(!a.is_winlink());
        assert_eq!(Address::parse("SMTP:bob@example.com"), a);
    }

    #[test]
    fn dates() {
        assert_eq!(format_date(0), "1970/01/01 00:00");
        let t = parse_date("2026/11/20 15:17").unwrap();
        assert_eq!(format_date(t), "2026/11/20 15:17");
        assert_eq!(parse_date("20261120151700"), Some(t));
        assert_eq!(parse_date("2026-11-20 15:17"), Some(t));
        assert_eq!(parse_date("nonsense"), None);
    }

    #[test]
    fn round_trip_with_attachment_and_unicode() {
        let mut m = Message::new(
            "k1abc",
            &["N0CALL", "bob@example.com"],
            "Café report",
            "line1\nline2 ü",
            1_800_000_000,
        );
        m.cc.push(Address::call("W1AW"));
        m.files.push(File {
            name: "log ä.txt".into(),
            data: vec![0, 1, 2, 13, 10, 255],
        });
        let wire = m.to_bytes();
        let back = Message::parse(&wire).unwrap();
        assert_eq!(back.subject, "Café report");
        assert_eq!(back.body, "line1\nline2 ü");
        assert_eq!(back.files, m.files);
        assert_eq!(back.to, m.to);
        assert_eq!(back.cc, m.cc);
        assert_eq!(back.date, m.date - m.date % 60);
        assert!(back.is_for("w1aw"));
        assert_eq!(back.mid.len(), MID_LEN);
    }

    #[test]
    fn reads_winlink_express_style_message() {
        let wire = b"Mid: ABCDEFGH1234\r\nDate: 2026/01/02 03:04\r\nType: Private\r\nFrom: N0CALL\r\nTo: K1ABC\r\n\
Subject: =?iso-8859-1?Q?caf=E9?=\r\nMbo: N0CALL\r\nBody: 5\r\nContent-Type: text/plain; charset=ISO-8859-1\r\n\r\ncaf\xe9!\r\n";
        let m = Message::parse(wire).unwrap();
        assert_eq!(m.subject, "café");
        assert_eq!(m.body, "café!");
        assert!(m.is_for("K1ABC"));
    }

    #[test]
    fn rejects_truncated() {
        let m = Message::new("K1ABC", &["N0CALL"], "s", "hello", 0);
        let wire = m.to_bytes();
        assert!(Message::parse(&wire[..wire.len() - 4]).is_err());
        assert!(Message::parse(b"nothing").is_err());
    }
}
