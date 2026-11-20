//! The B2F forwarding protocol (Winlink's compressed FBB variant), over any byte stream.
//!
//! One side is the *master* (the one that was called, e.g. a CMS or a listening station) and
//! sends its greeting first; the other is the client and takes the first turn. In a turn a
//! side either proposes a block of up to five messages (`FC` lines, then `F>` with a
//! checksum) and sends the ones the other side accepts, or says it has nothing (`FF`), or
//! quits (`FQ`). Message data is LZHUF compressed and sent in checksummed blocks.
//!
//! This module is plain `std::io` and has no radio or internet knowledge. A TCP stream gives
//! a telnet CMS or Pat/Winlink Express peer-to-peer link; a kahf link can feed the same
//! code. Secure login uses the account password (`secure` module); it is a challenge
//! response, nothing is encrypted.

use crate::lzhuf;
use crate::message::{Address, Message, ParseError};
use crate::secure;
use std::collections::HashSet;
use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};

/// Most proposals in one block.
pub const MAX_BLOCK: usize = 5;
/// Bytes of compressed data per `STX` block. 125 keeps blocks inside an AX.25 paclen of 128.
pub const CHUNK: usize = 125;

const SOH: u8 = 1;
const STX: u8 = 2;
const EOT: u8 = 4;

/// Errors that end a session.
#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    /// The other side broke the protocol.
    Protocol(String),
    /// The other side reported an error (a `***` line).
    Remote(String),
    /// The other side does not speak B2F.
    NoB2,
    /// A received message was not valid.
    Message(ParseError),
    Compression(lzhuf::Error),
    /// The mailbox refused a delivered message.
    Mailbox(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "connection error: {e}"),
            Error::Protocol(s) => write!(f, "protocol error: {s}"),
            Error::Remote(s) => write!(f, "remote error: {s}"),
            Error::NoB2 => f.write_str("remote does not support the B2 forwarding protocol"),
            Error::Message(e) => e.fmt(f),
            Error::Compression(e) => e.fmt(f),
            Error::Mailbox(s) => write!(f, "mailbox error: {s}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

fn proto<T>(s: impl Into<String>) -> Result<T, Error> {
    Err(Error::Protocol(s.into()))
}

/// What a session needs to know about itself.
#[derive(Clone, Debug)]
pub struct Config {
    /// Our callsign.
    pub mycall: String,
    /// The callsign we are talking to (`wl2k` for the CMS).
    pub target: String,
    /// Grid locator sent in the greeting comment, may be empty.
    pub locator: String,
    /// Account password for secure login, when the remote challenges.
    pub password: Option<String>,
    /// Extra callsigns to receive mail for in this session, each with an optional password
    /// (the CMS asks for one on addresses that have a password set).
    pub aux: Vec<(String, Option<String>)>,
    /// Program name and version in the SID, e.g. `("kahf", "1.0.0")`.
    pub program: (String, String),
    /// True when we were called and speak first.
    pub master: bool,
    /// Lines shown before the greeting (master only).
    pub motd: Vec<String>,
    /// Challenge the client must answer (master only). The password it must be built with is
    /// looked up through [`Mailbox::password_for`].
    pub require_login: bool,
}

impl Config {
    /// A client configuration for talking to the CMS or another station.
    pub fn client(mycall: &str, target: &str) -> Config {
        Config {
            mycall: mycall.trim().to_ascii_uppercase(),
            target: target.to_string(),
            locator: String::new(),
            password: None,
            aux: Vec::new(),
            program: ("kahf".into(), env!("CARGO_PKG_VERSION").into()),
            master: false,
            motd: Vec::new(),
            require_login: false,
        }
    }
}

/// A message the other side proposes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub mid: String,
    /// `EM` (message) or `CM` (command).
    pub kind: String,
    pub size: usize,
    pub compressed: usize,
}

/// Answer to an offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Accept,
    /// Already have it.
    Reject,
    /// Try again next session.
    Defer,
}

/// The mailbox a session reads from and writes to.
pub trait Mailbox {
    /// Messages to send. `fw` lists the callsigns the other side receives for.
    fn outbound(&mut self, fw: &[Address]) -> Vec<Message>;
    /// Decide on an offer, from its message id (reject what is already stored).
    fn answer(&mut self, offer: &Offer) -> Answer;
    /// A message arrived intact. Return an error to abort the session.
    fn deliver(&mut self, msg: Message) -> Result<(), String>;
    /// The other side took (or already had) the message.
    fn sent(&mut self, mid: &str, rejected: bool);
    /// The other side deferred the message; it stays queued.
    fn deferred(&mut self, _mid: &str) {}
    /// Password used to check a client's login (master with `require_login`).
    fn password_for(&mut self, _call: &str) -> Option<String> {
        None
    }
}

/// What a finished session moved.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub sent: Vec<String>,
    pub received: Vec<String>,
}

#[derive(Debug, Default)]
struct Hello {
    sid: Option<String>,
    fw: Vec<Address>,
    challenge: Option<String>,
    response: Option<String>,
    caller: Option<String>,
}

struct Session<'a, S: Read + Write, M: Mailbox> {
    rd: BufReader<S>,
    cfg: &'a Config,
    mbox: &'a mut M,
    remote_fw: Vec<Address>,
    remote_no_msgs: bool,
    stats: Stats,
}

/// Runs a whole session to the end. Errors are echoed to the other side as a `***` line.
pub fn run<S: Read + Write, M: Mailbox>(stream: S, cfg: &Config, mbox: &mut M) -> Result<Stats, Error> {
    let mut s = Session {
        rd: BufReader::new(stream),
        cfg,
        mbox,
        remote_fw: Vec::new(),
        remote_no_msgs: false,
        stats: Stats::default(),
    };
    match s.exchange() {
        Ok(()) => Ok(s.stats),
        Err(e) => {
            if !matches!(e, Error::Io(_) | Error::Remote(_)) {
                let _ = s.send(format!("*** {e}\r\n").as_bytes());
            }
            Err(e)
        }
    }
}

impl<S: Read + Write, M: Mailbox> Session<'_, S, M> {
    fn send(&mut self, data: &[u8]) -> Result<(), Error> {
        let w = self.rd.get_mut();
        w.write_all(data)?;
        w.flush()?;
        Ok(())
    }

    /// Reads one line, ended by CR (or LF). Skips blanks. A `***` line is the remote's error.
    fn line(&mut self, remote_errors: bool) -> Result<String, Error> {
        loop {
            let mut buf = Vec::new();
            loop {
                let mut b = [0u8; 1];
                if self.rd.read(&mut b)? == 0 {
                    return Err(Error::Io(io::ErrorKind::UnexpectedEof.into()));
                }
                match b[0] {
                    b'\r' | b'\n' => break,
                    c => buf.push(c),
                }
                if buf.len() > 4096 {
                    return proto("line too long");
                }
            }
            let line: String = String::from_utf8_lossy(&buf)
                .trim_matches(|c: char| c.is_whitespace() || c == '\0')
                .to_string();
            if line.is_empty() {
                continue;
            }
            if remote_errors && line.starts_with('*') {
                let msg = line.trim_start_matches('*').trim();
                if !msg.is_empty() {
                    return Err(Error::Remote(msg.to_string()));
                }
            }
            return Ok(line);
        }
    }

    fn peek(&mut self) -> Result<u8, Error> {
        let b = self.rd.fill_buf()?;
        match b.first() {
            Some(&c) => Ok(c),
            None => Err(Error::Io(io::ErrorKind::UnexpectedEof.into())),
        }
    }

    fn exchange(&mut self) -> Result<(), Error> {
        let mut challenge = None;
        if self.cfg.master {
            for l in &self.cfg.motd {
                self.send(format!("{l}\r").as_bytes())?;
            }
            if self.cfg.require_login {
                challenge = Some(secure::new_challenge());
            }
            self.send_hello(challenge.as_deref(), None)?;
        }
        let hello = self.read_hello()?;
        let sid = hello
            .sid
            .clone()
            .ok_or_else(|| Error::Protocol("no SID in handshake".into()))?;
        if !sid.to_ascii_uppercase().contains("B2") {
            return Err(Error::NoB2);
        }
        self.remote_fw = hello.fw.clone();
        if self.cfg.master {
            if let Some(ch) = &challenge {
                let caller = hello.caller.clone().unwrap_or_default();
                let want = self.mbox.password_for(&caller).map(|p| secure::response(ch, &p));
                if want.is_none() || want != hello.response {
                    return Err(Error::Remote(
                        "Secure login failed - account password does not match.".into(),
                    ));
                }
            }
        } else {
            self.send_hello(None, hello.challenge.as_deref())?;
        }

        let (mut my_turn, mut quit_sent, mut quit_recv) = (!self.cfg.master, false, false);
        while !(quit_sent || quit_recv) {
            if my_turn {
                quit_sent = self.outbound_turn()?;
            } else {
                quit_recv = self.inbound_turn()?;
            }
            my_turn = !my_turn;
        }
        Ok(())
    }

    fn send_hello(&mut self, challenge: Option<&str>, answer_to: Option<&str>) -> Result<(), Error> {
        let mut out = String::new();
        out.push_str(";FW:");
        for (i, (addr, pw)) in std::iter::once((self.cfg.mycall.clone(), None::<String>))
            .chain(self.cfg.aux.iter().cloned())
            .enumerate()
        {
            match (answer_to, pw, i) {
                (Some(ch), Some(pw), 1..) => out.push_str(&format!(" {addr}|{}", secure::response(ch, &pw))),
                _ => out.push_str(&format!(" {addr}")),
            }
        }
        out.push('\r');
        out.push_str(&format!(
            "[{}-{}-B2FHM$]\r",
            self.cfg.program.0, self.cfg.program.1
        ));
        if let Some(ch) = challenge {
            out.push_str(&format!(";PQ: {ch}\r"));
        }
        if let Some(ch) = answer_to {
            let pw = self.cfg.password.clone().ok_or_else(|| {
                Error::Protocol("the remote asks for a login but no password is set".into())
            })?;
            out.push_str(&format!(";PR: {}\r", secure::response(ch, &pw)));
        }
        out.push_str(&format!(
            "; {} DE {} ({})",
            self.cfg.target, self.cfg.mycall, self.cfg.locator
        ));
        out.push_str(if self.cfg.master { ">\r" } else { "\r" });
        self.send(out.as_bytes())
    }

    fn read_hello(&mut self) -> Result<Hello, Error> {
        let mut h = Hello::default();
        loop {
            if self.cfg.master && self.peek()? == b'F' {
                return Ok(h);
            }
            let line = self.line(false)?;
            if line.starts_with('[') && line.ends_with(']') {
                h.sid = Some(
                    line[1..line.len() - 1]
                        .rsplit('-')
                        .next()
                        .unwrap_or("")
                        .to_string(),
                );
            } else if let Some(fw) = line.strip_prefix(";FW:") {
                h.fw = fw
                    .split_whitespace()
                    .map(|a| Address::parse(a.split('|').next().unwrap_or(a)))
                    .collect();
                if let Some(first) = h.fw.first() {
                    h.caller = Some(first.addr.clone());
                }
            } else if let Some(c) = line.strip_prefix(";PQ:") {
                h.challenge = Some(c.trim().to_string());
            } else if let Some(r) = line.strip_prefix(";PR:") {
                h.response = Some(r.trim().to_string());
            } else if line.starts_with('*') {
                let msg = line.trim_start_matches('*').trim().to_string();
                if msg.to_ascii_lowercase().contains("secure login failed") || !self.cfg.master {
                    return Err(Error::Remote(msg));
                }
            } else if line.ends_with('>') && !self.cfg.master {
                return Ok(h);
            }
        }
    }

    // ---- sending turn ----

    fn outbound_turn(&mut self) -> Result<bool, Error> {
        let fw = self.remote_fw.clone();
        let mut msgs = self.mbox.outbound(&fw);
        msgs.retain(|m| !m.mid.is_empty());
        let mut seen = HashSet::new();
        msgs.retain(|m| seen.insert(m.mid.clone()));
        msgs.truncate(MAX_BLOCK);
        if msgs.is_empty() {
            let (cmd, quit) = if self.remote_no_msgs {
                ("FQ", true)
            } else {
                ("FF", false)
            };
            self.send(format!("{cmd}\r").as_bytes())?;
            return Ok(quit);
        }
        struct Prop {
            mid: String,
            title: String,
            size: usize,
            data: Vec<u8>,
        }
        let props: Vec<Prop> = msgs
            .iter()
            .map(|m| {
                let raw = m.to_bytes();
                Prop {
                    mid: m.mid.clone(),
                    title: title_for(&m.subject),
                    size: raw.len(),
                    data: lzhuf::compress(&raw),
                }
            })
            .collect();
        let mut block = String::new();
        let mut sum: i64 = 0;
        for p in &props {
            let line = format!("FC EM {} {} {} 0", p.mid, p.size, p.data.len());
            sum += line.bytes().map(i64::from).sum::<i64>() + 13;
            block.push_str(&line);
            block.push('\r');
        }
        block.push_str(&format!("F> {:02X}\r", (-sum) & 0xff));
        self.send(block.as_bytes())?;

        let answer = loop {
            let l = self.line(true)?;
            if l.starts_with("FS ") {
                break l;
            }
            if l.starts_with(';') {
                continue;
            }
            return proto(format!("expected a proposal answer, got {l:?}"));
        };
        let answers = parse_answers(&answer[3..], props.len())?;
        let mut done: Vec<(String, bool)> = Vec::new();
        for (p, a) in props.iter().zip(&answers) {
            match a {
                Answer::Defer => self.mbox.deferred(&p.mid),
                Answer::Reject => done.push((p.mid.clone(), true)),
                Answer::Accept => {
                    self.write_compressed(&p.title, &p.data)?;
                    done.push((p.mid.clone(), false));
                }
            }
        }
        for (mid, rej) in done.iter().filter(|(_, r)| *r) {
            self.mbox.sent(mid, *rej);
        }
        // After a turn the other side may only continue with F or ;. Anything else is an error.
        let next = self.peek()?;
        if next != b'F' && next != b';' {
            let l = self.line(false)?;
            return proto(format!("unexpected response {l:?}"));
        }
        for (mid, rej) in done.into_iter().filter(|(_, r)| !*r) {
            self.mbox.sent(&mid, rej);
            self.stats.sent.push(mid);
        }
        Ok(false)
    }

    fn write_compressed(&mut self, title: &str, data: &[u8]) -> Result<(), Error> {
        let mut out = Vec::with_capacity(data.len() + data.len() / CHUNK * 2 + 16);
        let offset = "0";
        out.extend_from_slice(&[SOH, (title.len() + offset.len() + 2) as u8]);
        out.extend_from_slice(title.as_bytes());
        out.push(0);
        out.extend_from_slice(offset.as_bytes());
        out.push(0);
        let mut sum: u32 = 0;
        for chunk in data.chunks(CHUNK) {
            out.extend_from_slice(&[STX, chunk.len() as u8]);
            out.extend_from_slice(chunk);
            sum += chunk.iter().map(|&b| u32::from(b)).sum::<u32>();
        }
        out.extend_from_slice(&[EOT, (sum.wrapping_neg() & 0xff) as u8]);
        self.send(&out)
    }

    // ---- receiving turn ----

    fn inbound_turn(&mut self) -> Result<bool, Error> {
        let mut offers: Vec<Offer> = Vec::new();
        let mut supported: Vec<bool> = Vec::new();
        let mut sum: i64 = 0;
        loop {
            let line = self.line(true)?;
            if line.starts_with(';') {
                continue;
            }
            if !line.starts_with('F') || line.len() < 2 {
                return proto(format!("unexpected protocol line {line:?}"));
            }
            match &line[..2] {
                "FA" | "FB" | "FC" | "FD" => {
                    sum += line.bytes().map(i64::from).sum::<i64>() + 13;
                    if offers.len() >= MAX_BLOCK {
                        return proto("more proposals than a block allows");
                    }
                    let code = line.as_bytes()[1];
                    let parts: Vec<&str> = line.get(3..).unwrap_or("").split(' ').collect();
                    if (code == b'C' || code == b'D') && parts.len() >= 4 {
                        offers.push(Offer {
                            kind: parts[0].to_string(),
                            mid: parts[1].to_string(),
                            size: parts[2].parse().unwrap_or(0),
                            compressed: parts[3].parse().unwrap_or(0),
                        });
                        // Gzip proposals (D) are an experiment of one client; defer them.
                        supported.push(code == b'C' && parts[0] == "EM");
                    } else {
                        offers.push(Offer {
                            kind: "?".into(),
                            mid: String::new(),
                            size: 0,
                            compressed: 0,
                        });
                        supported.push(false);
                    }
                }
                "FF" => {
                    self.remote_no_msgs = true;
                    return Ok(false);
                }
                "FQ" => return Ok(true),
                "F>" => {
                    let want = (-sum) & 0xff;
                    let got = i64::from_str_radix(line.get(3..).unwrap_or("").trim(), 16).unwrap_or(-1);
                    if want != got {
                        return proto(format!("proposal checksum {got:02X} does not match {want:02X}"));
                    }
                    if offers.is_empty() {
                        self.remote_no_msgs = true;
                        return Ok(false);
                    }
                    self.remote_no_msgs = false;
                    break;
                }
                _ => return proto(format!("unknown protocol command {line:?}")),
            }
        }

        let mut answers = Vec::with_capacity(offers.len());
        let mut seen = HashSet::new();
        for (o, ok) in offers.iter().zip(&supported) {
            let a = if !ok || o.mid.is_empty() || !seen.insert(o.mid.clone()) {
                Answer::Defer
            } else {
                self.mbox.answer(o)
            };
            answers.push(a);
        }
        let reply: String = answers
            .iter()
            .map(|a| match a {
                Answer::Accept => '+',
                Answer::Reject => '-',
                Answer::Defer => '=',
            })
            .collect();
        self.send(format!("FS {reply}\r").as_bytes())?;

        for (o, a) in offers.iter().zip(&answers) {
            if *a != Answer::Accept {
                continue;
            }
            let data = self.read_compressed(o)?;
            let raw = lzhuf::decompress(&data).map_err(Error::Compression)?;
            let msg = Message::parse(&raw).map_err(Error::Message)?;
            self.mbox.deliver(msg).map_err(Error::Mailbox)?;
            self.stats.received.push(o.mid.clone());
        }
        Ok(false)
    }

    fn byte(&mut self) -> Result<u8, Error> {
        let mut b = [0u8; 1];
        if self.rd.read(&mut b)? == 0 {
            return Err(Error::Io(io::ErrorKind::UnexpectedEof.into()));
        }
        Ok(b[0])
    }

    fn until_nul(&mut self) -> Result<Vec<u8>, Error> {
        let mut v = Vec::new();
        loop {
            match self.byte()? {
                0 => return Ok(v),
                c => v.push(c),
            }
            if v.len() > 256 {
                return proto("header field too long");
            }
        }
    }

    fn read_compressed(&mut self, o: &Offer) -> Result<Vec<u8>, Error> {
        match self.byte()? {
            SOH => {}
            b'*' => {
                let l = self.line(false)?;
                return Err(Error::Remote(l.trim_start_matches('*').trim().to_string()));
            }
            c => return proto(format!("expected message start, got byte {c}")),
        }
        let head_len = self.byte()? as usize;
        let title = self.until_nul()?;
        let offset = self.until_nul()?;
        if head_len != title.len() + offset.len() + 2 {
            return proto("message header length mismatch");
        }
        if offset != b"0" {
            return proto("message offsets are not supported");
        }
        if o.compressed > lzhuf::MAX_DECODED {
            return proto("proposed message is too large");
        }
        let mut data = Vec::with_capacity(o.compressed);
        let mut sum: u32 = 0;
        loop {
            match self.byte()? {
                STX => {
                    let n = match self.byte()? {
                        0 => 256,
                        n => n as usize,
                    };
                    let start = data.len();
                    data.resize(start + n, 0);
                    self.rd.read_exact(&mut data[start..])?;
                    sum += data[start..].iter().map(|&b| u32::from(b)).sum::<u32>();
                    if data.len() > o.compressed {
                        return proto("message is longer than proposed");
                    }
                }
                EOT => {
                    let c = self.byte()?;
                    if (sum + u32::from(c)) & 0xff != 0 {
                        return proto("bad message checksum");
                    }
                    if data.len() != o.compressed {
                        return proto("message length differs from the proposal");
                    }
                    return Ok(data);
                }
                c => return proto(format!("unexpected byte {c} in message data")),
            }
        }
    }
}

fn parse_answers(s: &str, n: usize) -> Result<Vec<Answer>, Error> {
    let mut v = Vec::new();
    for c in s.trim().chars() {
        v.push(match c {
            'Y' | 'y' | '+' => Answer::Accept,
            'N' | 'n' | 'R' | 'r' | '-' => Answer::Reject,
            'L' | 'l' | '=' | 'H' | 'h' => Answer::Defer,
            // An offset request ('!' or 'A' plus digits). We always resend from the start.
            'A' | 'a' | '!' => return proto("offset requests are not supported"),
            c => return proto(format!("bad character {c:?} in proposal answer")),
        });
    }
    if v.len() != n {
        return proto(format!("answer has {} entries for {n} proposals", v.len()));
    }
    Ok(v)
}

/// The title field must be ASCII and at most 80 bytes.
fn title_for(subject: &str) -> String {
    let t: String = subject
        .chars()
        .map(|c| if (' '..='~').contains(&c) { c } else { '?' })
        .take(80)
        .collect();
    if t.is_empty() {
        "No title".into()
    } else {
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::mpsc::{channel, Receiver, Sender};

    /// One end of an in-memory duplex pipe.
    struct Pipe {
        tx: Sender<Vec<u8>>,
        rx: Receiver<Vec<u8>>,
        buf: VecDeque<u8>,
    }

    fn pair() -> (Pipe, Pipe) {
        let (a_tx, b_rx) = channel();
        let (b_tx, a_rx) = channel();
        (
            Pipe {
                tx: a_tx,
                rx: a_rx,
                buf: VecDeque::new(),
            },
            Pipe {
                tx: b_tx,
                rx: b_rx,
                buf: VecDeque::new(),
            },
        )
    }

    impl Read for Pipe {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            while self.buf.is_empty() {
                match self.rx.recv() {
                    Ok(v) => self.buf.extend(v),
                    Err(_) => return Ok(0),
                }
            }
            let n = out.len().min(self.buf.len());
            for o in out.iter_mut().take(n) {
                *o = self.buf.pop_front().unwrap();
            }
            Ok(n)
        }
    }

    impl Write for Pipe {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            self.tx
                .send(data.to_vec())
                .map_err(|_| io::ErrorKind::BrokenPipe)?;
            Ok(data.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct Box1 {
        out: Vec<Message>,
        have: HashSet<String>,
        got: Vec<Message>,
        sent: Vec<(String, bool)>,
        password: Option<String>,
    }

    impl Mailbox for Box1 {
        fn outbound(&mut self, _fw: &[Address]) -> Vec<Message> {
            let sent: HashSet<&str> = self.sent.iter().map(|(m, _)| m.as_str()).collect();
            self.out
                .iter()
                .filter(|m| !sent.contains(m.mid.as_str()))
                .cloned()
                .collect()
        }
        fn answer(&mut self, o: &Offer) -> Answer {
            if self.have.contains(&o.mid) {
                Answer::Reject
            } else {
                Answer::Accept
            }
        }
        fn deliver(&mut self, m: Message) -> Result<(), String> {
            self.have.insert(m.mid.clone());
            self.got.push(m);
            Ok(())
        }
        fn sent(&mut self, mid: &str, rejected: bool) {
            self.sent.push((mid.into(), rejected));
        }
        fn password_for(&mut self, _c: &str) -> Option<String> {
            self.password.clone()
        }
    }

    fn msgs(from: &str, to: &str, n: usize) -> Vec<Message> {
        (0..n)
            .map(|i| {
                let mut m = Message::new(
                    from,
                    &[to],
                    &format!("subject {i}"),
                    &"body text ".repeat(40 * (i + 1)),
                    1_800_000_000 + i as i64,
                );
                if i == 1 {
                    m.files.push(crate::message::File {
                        name: "x.bin".into(),
                        data: (0..=255u8).cycle().take(900).collect(),
                    });
                }
                m
            })
            .collect()
    }

    fn run_pair(
        mut client_box: Box1,
        mut server_box: Box1,
        server_cfg: Config,
        client_cfg: Config,
    ) -> (Result<Stats, Error>, Result<Stats, Error>, Box1, Box1) {
        let (a, b) = pair();
        let h = std::thread::spawn(move || {
            let r = run(b, &server_cfg, &mut server_box);
            (r, server_box)
        });
        let r = run(a, &client_cfg, &mut client_box);
        let (sr, sbox) = h.join().unwrap();
        (r, sr, client_box, sbox)
    }

    #[test]
    fn two_way_exchange_with_more_than_one_block() {
        let client = Box1 {
            out: msgs("K1ABC", "N0CALL", 7),
            ..Default::default()
        };
        let server = Box1 {
            out: msgs("N0CALL", "K1ABC", 2),
            ..Default::default()
        };
        let mut sc = Config::client("N0CALL", "K1ABC");
        sc.master = true;
        let (cr, sr, cb, sb) = run_pair(client, server, sc, Config::client("K1ABC", "N0CALL"));
        let (cs, ss) = (cr.unwrap(), sr.unwrap());
        assert_eq!(cs.sent.len(), 7);
        assert_eq!(ss.received.len(), 7);
        assert_eq!(cs.received.len(), 2);
        assert_eq!(sb.got.len(), 7);
        assert_eq!(cb.got.len(), 2);
        assert_eq!(sb.got[1].files.len(), 1);
        assert_eq!(sb.got[1].files[0].data.len(), 900);
        assert!(cb.sent.is_empty() || cb.sent.iter().all(|(_, r)| !*r));
    }

    #[test]
    fn rejects_messages_already_held() {
        let client = Box1 {
            out: msgs("K1ABC", "N0CALL", 2),
            ..Default::default()
        };
        let mut server = Box1::default();
        server.have.insert(client.out[0].mid.clone());
        let mut sc = Config::client("N0CALL", "K1ABC");
        sc.master = true;
        let (cr, sr, cb, sb) = run_pair(client, server, sc, Config::client("K1ABC", "N0CALL"));
        assert_eq!(cr.unwrap().sent.len(), 1);
        sr.unwrap();
        assert_eq!(sb.got.len(), 1);
        assert!(cb.sent.contains(&(cb.out[0].mid.clone(), true)));
    }

    #[test]
    fn secure_login_accepts_right_password_and_rejects_wrong() {
        for (client_pw, ok) in [("hunter2", true), ("wrong", false)] {
            let server = Box1 {
                password: Some("hunter2".into()),
                ..Default::default()
            };
            let mut sc = Config::client("N0CALL", "K1ABC");
            sc.master = true;
            sc.require_login = true;
            let mut cc = Config::client("K1ABC", "N0CALL");
            cc.password = Some(client_pw.into());
            let (cr, sr, _, _) = run_pair(Box1::default(), server, sc, cc);
            assert_eq!(cr.is_ok(), ok, "{cr:?}");
            assert_eq!(sr.is_ok(), ok);
        }
    }

    #[test]
    fn client_without_password_stops_at_a_challenge() {
        let server = Box1 {
            password: Some("x".into()),
            ..Default::default()
        };
        let mut sc = Config::client("N0CALL", "K1ABC");
        sc.master = true;
        sc.require_login = true;
        let (cr, _, _, _) = run_pair(Box1::default(), server, sc, Config::client("K1ABC", "N0CALL"));
        assert!(matches!(cr, Err(Error::Protocol(_))));
    }

    #[test]
    fn answer_parsing() {
        assert_eq!(
            parse_answers("+-=", 3).unwrap(),
            vec![Answer::Accept, Answer::Reject, Answer::Defer]
        );
        assert!(parse_answers("+", 2).is_err());
        assert!(parse_answers("!5", 1).is_err());
        assert_eq!(title_for("café"), "caf?");
        assert_eq!(title_for(""), "No title");
    }
}
