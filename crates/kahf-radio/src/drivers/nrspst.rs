//! Driver for the SDRplay nRSP-ST receiver, reached over the network.
//!
//! The receiver is reached through a network server at host:port. The server
//! speaks WebSocket (RFC 6455, no TLS). Control messages are JSON text objects
//! with the fields event_type, property and value. Samples arrive as binary
//! messages: a 16-bit little-endian type (2 = interleaved signed 16-bit I/Q of
//! the primary device), then the samples.
//!
//! Values taken from the reference design. Not confirmed here, because no
//! receiver is connected and the reference marks them as assumed:
//! - the request path "/";
//! - the rate range 62.5 kHz to 10 MHz;
//! - the LNA state direction (state 0 = most gain), and the 9 dB fallback range
//!   when the server does not report its LNA states;
//! - the list of sample-rate steps (the reference has a continuous range).
//!
//! Nothing in this file has been run against a real receiver. Tests use a fake
//! peer on 127.0.0.1.

use std::collections::hash_map::RandomState;
use std::collections::{HashMap, VecDeque};
use std::hash::{BuildHasher, Hasher};
use std::io::{self, ErrorKind, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use crate::{DeviceInfo, GainSetting, GainStage, RxRadio};

/// Message type of the interleaved signed 16-bit I/Q samples of the primary device.
pub const IQ_MESSAGE_TYPE: u16 = 2;
/// Wait for the TCP connection to open.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(3000);
/// Wait for each answer during the handshake and the first questions.
const ANSWER_TIMEOUT: Duration = Duration::from_millis(3000);
/// No samples for this long while streaming counts as a lost receiver.
const STALL_TIMEOUT: Duration = Duration::from_millis(5000);
/// The longest read_iq waits for samples.
const READ_WAIT: Duration = Duration::from_millis(200);
/// Largest message accepted from the server, in bytes.
const MAX_MESSAGE: usize = 1 << 20;
/// Samples kept for the next read_iq call before the oldest are dropped.
const MAX_PENDING: usize = 1 << 20;
/// Request path of the WebSocket upgrade (unconfirmed, see the module notes).
const PATH: &str = "/";
/// Fixed GUID from RFC 6455, used to check the server's accept key.
const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Lowest and highest sample rate in Hz (reference design, assumed there).
pub const MIN_RATE_HZ: u32 = 62_500;
pub const MAX_RATE_HZ: u32 = 10_000_000;
/// Rate asked for when the caller gives none (reference design).
pub const DEFAULT_RATE_HZ: u32 = 2_000_000;
/// Tuning range in Hz (datasheet, from the reference design).
pub const MIN_FREQ_HZ: u64 = 1_000;
pub const MAX_FREQ_HZ: u64 = 2_000_000_000;
/// LNA gain range used when the server does not report its LNA states (reference design).
pub const FALLBACK_LNA_RANGE_DB: i32 = 9;
/// Name of the one gain stage.
pub const LNA_STAGE: &str = "LNA";
/// The sample-rate steps offered. The rate may be set anywhere in the range.
pub const SAMPLE_RATES: &[u32] = &[
    62_500, 125_000, 250_000, 500_000, 1_000_000, 2_000_000, 4_000_000, 8_000_000, 10_000_000,
];

const NO_ADDRESS: &str = "nRSP-ST: no address given. Give the receiver's network address as host:port.";

// ---------------------------------------------------------------- pure helpers

/// The sample rates the receiver offers, in ascending order.
pub fn sample_rates() -> Vec<u32> {
    SAMPLE_RATES.to_vec()
}

/// Clamps a requested sample rate to the range of the receiver.
pub fn clamp_rate(hz: u32) -> u32 {
    hz.clamp(MIN_RATE_HZ, MAX_RATE_HZ)
}

/// Width of the LNA range in dB from the server's state minimum and maximum.
/// Falls back to 9 dB when the maximum is not above the minimum.
pub fn lna_range_db(state_min: i32, state_max: i32) -> i32 {
    if state_max > state_min {
        state_max - state_min
    } else {
        FALLBACK_LNA_RANGE_DB
    }
}

/// Clamps a requested gain to 0 dB .. `range_db` and rounds it to a whole dB.
/// A NaN request is treated as 0 dB.
pub fn clamp_gain_db(requested: f32, range_db: i32) -> i32 {
    let value = if requested.is_nan() { 0.0 } else { requested };
    value.clamp(0.0, range_db.max(0) as f32).round() as i32
}

/// The LNA state that gives `gain_db` of gain, given the server's maximum state.
/// State 0 is taken as the most gain (reference design, unconfirmed), so the
/// state counts down from the maximum.
pub fn lna_state_for(gain_db: i32, state_max: i32) -> i32 {
    state_max - gain_db
}

/// Splits a binary message into its 16-bit little-endian type and its payload.
pub fn split_message(msg: &[u8]) -> Option<(u16, &[u8])> {
    if msg.len() < 2 {
        return None;
    }
    Some((u16::from_le_bytes([msg[0], msg[1]]), &msg[2..]))
}

/// Converts interleaved signed 16-bit little-endian I/Q to complex floats in
/// -1.0 .. 1.0 (divided by 32768). A trailing partial sample is ignored.
pub fn iq_from_i16_le(payload: &[u8]) -> Vec<(f32, f32)> {
    payload
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| {
            let re = i16::from_le_bytes([b[0], b[1]]);
            let im = i16::from_le_bytes([b[2], b[3]]);
            (re as f32 / 32768.0, im as f32 / 32768.0)
        })
        .collect()
}

/// Escapes a string as a JSON string literal.
pub fn json_escape(s: &str) -> String {
    let mut r = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => r.push_str("\\\""),
            '\\' => r.push_str("\\\\"),
            c if (c as u32) < 0x20 => r.push_str(&format!("\\u{:04x}", c as u32)),
            c => r.push(c),
        }
    }
    r.push('"');
    r
}

/// One control message: {"event_type":..,"property":..,"value":..}.
pub fn control_message(event: &str, property: &str, value: &str) -> String {
    format!(
        "{{\"event_type\":{},\"property\":{},\"value\":{}}}",
        json_escape(event),
        json_escape(property),
        json_escape(value)
    )
}

/// Parses one flat JSON object into text values. Nested objects and arrays are
/// skipped. Returns `None` when the text is not such an object.
pub fn parse_flat_json(text: &str) -> Option<HashMap<String, String>> {
    let b = text.as_bytes();
    let mut i = 0usize;
    let mut out = HashMap::new();
    skip_ws(b, &mut i);
    if b.get(i) != Some(&b'{') {
        return None;
    }
    i += 1;
    skip_ws(b, &mut i);
    if b.get(i) == Some(&b'}') {
        return Some(out);
    }
    loop {
        skip_ws(b, &mut i);
        let key = parse_string(b, &mut i)?;
        skip_ws(b, &mut i);
        if b.get(i) != Some(&b':') {
            return None;
        }
        i += 1;
        skip_ws(b, &mut i);
        match *b.get(i)? {
            b'"' => {
                let v = parse_string(b, &mut i)?;
                out.insert(key, v);
            }
            b'{' | b'[' => {
                // Nested values are skipped and not recorded.
                skip_nested(b, &mut i)?;
            }
            _ => {
                let start = i;
                while i < b.len() && !matches!(b[i], b',' | b'}' | b' ' | b'\t' | b'\r' | b'\n') {
                    i += 1;
                }
                let v = text[start..i].to_string();
                if v.is_empty() || v.len() > 64 {
                    return None;
                }
                out.insert(key, v);
            }
        }
        if out.len() > 64 {
            return None;
        }
        skip_ws(b, &mut i);
        match *b.get(i)? {
            b',' => i += 1,
            b'}' => return Some(out),
            _ => return None,
        }
    }
}

fn skip_ws(b: &[u8], i: &mut usize) {
    while *i < b.len() && matches!(b[*i], b' ' | b'\t' | b'\r' | b'\n') {
        *i += 1;
    }
}

fn parse_string(b: &[u8], i: &mut usize) -> Option<String> {
    if b.get(*i) != Some(&b'"') {
        return None;
    }
    *i += 1;
    let mut out: Vec<u8> = Vec::new();
    while *i < b.len() && b[*i] != b'"' {
        let c = b[*i];
        *i += 1;
        if c == b'\\' {
            let e = *b.get(*i)?;
            *i += 1;
            match e {
                b'n' => out.push(b'\n'),
                b't' => out.push(b'\t'),
                b'r' => out.push(b'\r'),
                b'u' => {
                    let hex = b.get(*i..*i + 4)?;
                    let v = u32::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?;
                    *i += 4;
                    let ch = char::from_u32(v).unwrap_or('\u{FFFD}');
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
                other => out.push(other),
            }
        } else {
            out.push(c);
        }
        if out.len() > 4096 {
            return None;
        }
    }
    if *i >= b.len() {
        return None;
    }
    *i += 1;
    String::from_utf8(out).ok()
}

fn skip_nested(b: &[u8], i: &mut usize) -> Option<()> {
    let mut depth = 0i32;
    let mut in_str = false;
    while *i < b.len() {
        let c = b[*i];
        *i += 1;
        if in_str {
            if c == b'\\' {
                *i += 1;
            } else if c == b'"' {
                in_str = false;
            }
        } else if c == b'"' {
            in_str = true;
        } else if c == b'{' || c == b'[' {
            depth += 1;
            if depth > 8 {
                return None;
            }
        } else if c == b'}' || c == b']' {
            depth -= 1;
            if depth == 0 {
                return Some(());
            }
        }
    }
    None
}

/// Splits "host:port" (IPv6 hosts in brackets). Returns `None` for a bad port.
pub fn split_host_port(addr: &str) -> Option<(&str, u16)> {
    let c = addr.rfind(':')?;
    if c == 0 {
        return None;
    }
    let mut host = &addr[..c];
    if host.len() > 2 && host.starts_with('[') && host.ends_with(']') {
        host = &host[1..host.len() - 1];
    }
    let port: u16 = addr[c + 1..].parse().ok()?;
    if port == 0 {
        return None;
    }
    Some((host, port))
}

/// The error text when the connection is refused or cannot be made.
pub fn connect_error_text(addr: &str, detail: &str) -> String {
    format!(
        "nRSP-ST: cannot connect to {addr}: {detail}. Check that the receiver's network server is running and that the address is right."
    )
}

// ------------------------------------------------------------- SHA-1 and base64

fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.as_chunks::<64>().0 {
        let mut w = [0u32; 80];
        for t in 0..16 {
            w[t] = u32::from_be_bytes([chunk[4 * t], chunk[4 * t + 1], chunk[4 * t + 2], chunk[4 * t + 3]]);
        }
        for t in 16..80 {
            w[t] = (w[t - 3] ^ w[t - 8] ^ w[t - 14] ^ w[t - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (t, &wt) in w.iter().enumerate() {
            let (f, k) = match t {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wt);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (i, v) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

fn base64(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in data.chunks(3) {
        let b0 = c[0] as u32;
        let b1 = *c.get(1).unwrap_or(&0) as u32;
        let b2 = *c.get(2).unwrap_or(&0) as u32;
        let v = b0 << 16 | b1 << 8 | b2;
        out.push(T[(v >> 18 & 63) as usize] as char);
        out.push(T[(v >> 12 & 63) as usize] as char);
        out.push(if c.len() > 1 {
            T[(v >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[(v & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Random bytes for keys and masks. Not for security, only for variety.
fn random_bytes16() -> [u8; 16] {
    let mut out = [0u8; 16];
    for chunk in out.chunks_mut(8) {
        let v = RandomState::new().build_hasher().finish().to_le_bytes();
        chunk.copy_from_slice(&v[..chunk.len()]);
    }
    out
}

// ------------------------------------------------------------- WebSocket link

enum Incoming {
    Text(String),
    Binary(Vec<u8>),
    Timeout,
    Closed,
    Bad(String),
}

enum Fill {
    Done,
    Idle,
    Gone,
}

fn is_timeout(e: &io::Error) -> bool {
    matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
}

/// A WebSocket connection over one TCP stream. Client frames are masked.
struct Link {
    stream: TcpStream,
}

impl Link {
    fn new(stream: TcpStream) -> Link {
        let _ = stream.set_nodelay(true);
        Link { stream }
    }

    fn set_timeout(&mut self, d: Duration) {
        let _ = self
            .stream
            .set_read_timeout(Some(d.max(Duration::from_millis(1))));
    }

    /// Upgrades the TCP connection to WebSocket. Checks the server's accept key.
    fn handshake(&mut self, host: &str, port: u16, addr: &str) -> Result<(), String> {
        self.set_timeout(ANSWER_TIMEOUT);
        let key = base64(&random_bytes16());
        let hp = if host.contains(':') {
            format!("[{host}]:{port}")
        } else {
            format!("{host}:{port}")
        };
        let req = format!(
            "GET {PATH} HTTP/1.1\r\nHost: {hp}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        if self.stream.write_all(req.as_bytes()).is_err() {
            return Err(format!(
                "nRSP-ST: the connection to {addr} dropped during the handshake"
            ));
        }
        let mut head: Vec<u8> = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if head.len() >= 8192 {
                return Err(format!("nRSP-ST: {addr} did not answer with a WebSocket header"));
            }
            match self.stream.read(&mut byte) {
                Ok(1) => head.push(byte[0]),
                Ok(_) => {
                    return Err(format!(
                        "nRSP-ST: the connection to {addr} closed during the handshake"
                    ))
                }
                Err(e) if is_timeout(&e) => {
                    return Err(format!(
                        "nRSP-ST: no answer from {addr} within {} s during the handshake",
                        ANSWER_TIMEOUT.as_secs()
                    ))
                }
                Err(_) => {
                    return Err(format!(
                        "nRSP-ST: the connection to {addr} dropped during the handshake"
                    ))
                }
            }
        }
        let text = String::from_utf8_lossy(&head).to_string();
        let is_101 = text.starts_with("HTTP/") && text.split_whitespace().nth(1) == Some("101");
        if !is_101 {
            let first = text.lines().next().unwrap_or("").trim();
            return Err(format!(
                "nRSP-ST: {addr} is not a WebSocket server, or refused the connection: {first}"
            ));
        }
        let accept = text.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            if k.trim().eq_ignore_ascii_case("sec-websocket-accept") {
                Some(v.trim().to_string())
            } else {
                None
            }
        });
        let expect = base64(&sha1(format!("{key}{WS_GUID}").as_bytes()));
        if accept.as_deref() != Some(expect.as_str()) {
            return Err(format!(
                "nRSP-ST: {addr} gave a WebSocket accept key that does not match"
            ));
        }
        Ok(())
    }

    /// Reads exactly `buf.len()` bytes. At the start of a frame, a quiet
    /// socket gives `Idle`. Inside a frame, a few timeouts are waited out.
    fn fill(&mut self, buf: &mut [u8], at_start: bool) -> Fill {
        let mut got = 0;
        let mut stalls = 0;
        while got < buf.len() {
            match self.stream.read(&mut buf[got..]) {
                Ok(0) => return Fill::Gone,
                Ok(n) => got += n,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) if is_timeout(&e) => {
                    if at_start && got == 0 {
                        return Fill::Idle;
                    }
                    stalls += 1;
                    if stalls > 10 {
                        return Fill::Gone;
                    }
                }
                Err(_) => return Fill::Gone,
            }
        }
        Fill::Done
    }

    fn send_frame(&mut self, op: u8, payload: &[u8]) -> io::Result<()> {
        let mut f: Vec<u8> = Vec::with_capacity(payload.len() + 14);
        f.push(0x80 | op);
        let n = payload.len();
        if n < 126 {
            f.push(0x80 | n as u8);
        } else if n < 65_536 {
            f.push(0x80 | 126);
            f.extend_from_slice(&(n as u16).to_be_bytes());
        } else {
            f.push(0x80 | 127);
            f.extend_from_slice(&(n as u64).to_be_bytes());
        }
        let mask = random_bytes16();
        let m = [mask[0], mask[1], mask[2], mask[3]];
        f.extend_from_slice(&m);
        f.extend(payload.iter().enumerate().map(|(i, &b)| b ^ m[i & 3]));
        self.stream.write_all(&f)
    }

    fn send_text(&mut self, s: &str) -> io::Result<()> {
        self.send_frame(0x1, s.as_bytes())
    }

    /// Sends a close frame and shuts the socket.
    fn close(&mut self) {
        let _ = self.send_frame(0x8, &1000u16.to_be_bytes());
        let _ = self.stream.shutdown(Shutdown::Both);
    }

    /// One whole message. Pings are answered and pongs dropped.
    fn read_message(&mut self) -> Incoming {
        let mut out: Vec<u8> = Vec::new();
        let mut kind: Option<u8> = None;
        loop {
            let mut h = [0u8; 2];
            match self.fill(&mut h, true) {
                Fill::Done => {}
                Fill::Idle => return Incoming::Timeout,
                Fill::Gone => return Incoming::Closed,
            }
            let fin = h[0] & 0x80 != 0;
            let op = h[0] & 0x0F;
            if h[0] & 0x70 != 0 {
                return Incoming::Bad("an extension was used that was not agreed".into());
            }
            if h[1] & 0x80 != 0 {
                return Incoming::Bad("the server sent a masked frame".into());
            }
            let mut len = (h[1] & 0x7F) as u64;
            if len == 126 {
                let mut x = [0u8; 2];
                if !matches!(self.fill(&mut x, false), Fill::Done) {
                    return Incoming::Closed;
                }
                len = u16::from_be_bytes(x) as u64;
            } else if len == 127 {
                let mut x = [0u8; 8];
                if !matches!(self.fill(&mut x, false), Fill::Done) {
                    return Incoming::Closed;
                }
                len = u64::from_be_bytes(x);
            }
            if op >= 0x8 {
                if len > 125 || !fin {
                    return Incoming::Bad("a control frame was too long or split".into());
                }
                let mut p = vec![0u8; len as usize];
                if len > 0 && !matches!(self.fill(&mut p, false), Fill::Done) {
                    return Incoming::Closed;
                }
                match op {
                    0x8 => {
                        let keep = p.len().min(2);
                        let _ = self.send_frame(0x8, &p[..keep]);
                        return Incoming::Closed;
                    }
                    0x9 => {
                        let _ = self.send_frame(0xA, &p);
                    }
                    0xA => {}
                    _ => return Incoming::Bad("an unknown control frame".into()),
                }
                continue;
            }
            if op == 0x0 {
                if kind.is_none() {
                    return Incoming::Bad("a continuation frame with nothing to continue".into());
                }
            } else {
                if kind.is_some() || (op != 0x1 && op != 0x2) {
                    return Incoming::Bad("an unexpected data frame".into());
                }
                kind = Some(op);
            }
            if len > MAX_MESSAGE as u64 || out.len() as u64 + len > MAX_MESSAGE as u64 {
                return Incoming::Bad("a message was too large".into());
            }
            let at = out.len();
            out.resize(at + len as usize, 0);
            if len > 0 && !matches!(self.fill(&mut out[at..], false), Fill::Done) {
                return Incoming::Closed;
            }
            if fin {
                return match kind {
                    Some(0x1) => match String::from_utf8(out) {
                        Ok(s) => Incoming::Text(s),
                        Err(_) => Incoming::Bad("a text message was not valid UTF-8".into()),
                    },
                    _ => Incoming::Binary(out),
                };
            }
        }
    }
}

// ------------------------------------------------------------ peer information

/// What the server reported about itself.
#[derive(Debug, Default, Clone, PartialEq)]
struct Peer {
    api_version: Option<String>,
    lna_min: Option<i32>,
    lna_max: Option<i32>,
}

fn absorb(peer: &mut Peer, kv: &HashMap<String, String>) {
    let (Some(event), Some(prop), Some(value)) = (kv.get("event_type"), kv.get("property"), kv.get("value"))
    else {
        return;
    };
    if event != "get_property_response" && event != "property_changed" {
        return;
    }
    match prop.as_str() {
        "api_version" => peer.api_version = Some(value.clone()),
        "lna_state_min" => peer.lna_min = value.parse().ok(),
        "lna_state_max" => peer.lna_max = value.parse().ok(),
        _ => {}
    }
}

const PEER_QUESTIONS: &[&str] = &[
    "api_version",
    "valid_devices",
    "active_device",
    "valid_antennas",
    "active_antenna",
    "lna_state_min",
    "lna_state_max",
    "can_control",
    "device_sample_rate",
];

// --------------------------------------------------------------- the radio

/// An nRSP-ST reached over the network. Built by `open`.
pub struct NrspSt {
    addr: String,
    link: Option<Link>,
    lna_min: i32,
    lna_max: i32,
    pending: VecDeque<(f32, f32)>,
    dropped: u64,
    last_data: Instant,
    started: bool,
}

impl NrspSt {
    fn lna_range(&self) -> i32 {
        lna_range_db(self.lna_min, self.lna_max)
    }

    fn link_mut(&mut self) -> Result<&mut Link, String> {
        let addr = self.addr.clone();
        self.link
            .as_mut()
            .ok_or_else(|| format!("nRSP-ST: not connected to {addr}; open it again"))
    }

    fn send_prop(&mut self, prop: &str, value: &str) -> Result<(), String> {
        let text = control_message("set_property", prop, value);
        let addr = self.addr.clone();
        let link = self.link_mut()?;
        link.send_text(&text)
            .map_err(|_| format!("nRSP-ST: the connection to {addr} dropped"))
    }

    fn send_raw(&mut self, text: &str) -> Result<(), String> {
        let addr = self.addr.clone();
        let link = self.link_mut()?;
        link.send_text(text)
            .map_err(|_| format!("nRSP-ST: the connection to {addr} dropped"))
    }

    fn queue(&mut self, s: (f32, f32)) {
        if self.pending.len() >= MAX_PENDING {
            self.pending.pop_front();
            self.dropped += 1;
        }
        self.pending.push_back(s);
    }
}

impl Drop for NrspSt {
    fn drop(&mut self) {
        self.stop();
    }
}

impl RxRadio for NrspSt {
    fn label(&self) -> String {
        format!("nRSP-ST at {}", self.addr)
    }

    fn freq_range_hz(&self) -> (u64, u64) {
        (MIN_FREQ_HZ, MAX_FREQ_HZ)
    }

    fn set_frequency_hz(&mut self, hz: u64) -> Result<(), String> {
        if !(MIN_FREQ_HZ..=MAX_FREQ_HZ).contains(&hz) {
            return Err(format!(
                "nRSP-ST: {hz} Hz is outside the tuning range of 1 kHz to 2 GHz"
            ));
        }
        self.send_prop("device_center_frequency", &hz.to_string())?;
        self.send_prop("device_vfo_frequency", &hz.to_string())
    }

    fn sample_rates(&self) -> Vec<u32> {
        sample_rates()
    }

    fn set_sample_rate(&mut self, hz: u32) -> Result<u32, String> {
        let rate = clamp_rate(hz);
        self.send_prop("device_sample_rate", &rate.to_string())?;
        Ok(rate)
    }

    fn gain_stages(&self) -> Vec<GainStage> {
        vec![GainStage {
            name: LNA_STAGE.to_string(),
            min_db: 0.0,
            max_db: self.lna_range() as f32,
            step_db: 1.0,
            auto: false,
        }]
    }

    fn set_gain(&mut self, stage: &str, setting: GainSetting) -> Result<(), String> {
        if stage != LNA_STAGE {
            return Err(format!(
                "nRSP-ST: no gain stage named {stage:?}; the stage is {LNA_STAGE:?}"
            ));
        }
        let db = match setting {
            GainSetting::Auto => {
                return Err("nRSP-ST: the LNA stage has no automatic setting; give a fixed dB value".into())
            }
            GainSetting::Manual(v) => v,
        };
        let gain = clamp_gain_db(db, self.lna_range());
        let state = lna_state_for(gain, self.lna_max);
        self.send_prop("lna_state", &state.to_string())
    }

    fn start(&mut self) -> Result<(), String> {
        if self.started {
            return Ok(());
        }
        self.send_raw(&control_message("device_stream_enable", "", "true"))?;
        self.send_raw(&control_message("iq_stream_enable", "", "true"))?;
        self.started = true;
        self.last_data = Instant::now();
        Ok(())
    }

    fn read_iq(&mut self, out: &mut [(f32, f32)]) -> Result<usize, String> {
        let mut n = 0;
        while n < out.len() {
            match self.pending.pop_front() {
                Some(s) => {
                    out[n] = s;
                    n += 1;
                }
                None => break,
            }
        }
        let deadline = Instant::now() + READ_WAIT;
        while n < out.len() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() && n > 0 {
                break;
            }
            let incoming = {
                let link = self.link_mut()?;
                link.set_timeout(left);
                link.read_message()
            };
            match incoming {
                Incoming::Text(t) => {
                    if let Some(kv) = parse_flat_json(&t) {
                        if kv.get("property").map(String::as_str) == Some("started")
                            && kv.get("value").map(String::as_str) == Some("false")
                        {
                            self.send_raw(&control_message("device_stream_enable", "", "true"))?;
                        }
                    }
                }
                Incoming::Binary(b) => {
                    if let Some((t, payload)) = split_message(&b) {
                        if t == IQ_MESSAGE_TYPE {
                            self.last_data = Instant::now();
                            for s in iq_from_i16_le(payload) {
                                if n < out.len() {
                                    out[n] = s;
                                    n += 1;
                                } else {
                                    self.queue(s);
                                }
                            }
                        }
                    }
                }
                Incoming::Timeout => break,
                Incoming::Closed => {
                    return Err(format!("nRSP-ST: the connection to {} closed", self.addr));
                }
                Incoming::Bad(why) => {
                    return Err(format!("nRSP-ST: bad data from {}: {why}", self.addr));
                }
            }
        }
        if n == 0 && self.started && self.last_data.elapsed() > STALL_TIMEOUT {
            return Err(format!(
                "nRSP-ST: no samples from {} for {} s; the receiver is treated as lost",
                self.addr,
                STALL_TIMEOUT.as_secs()
            ));
        }
        Ok(n)
    }

    fn stop(&mut self) {
        if let Some(mut link) = self.link.take() {
            link.close();
        }
        self.started = false;
        self.pending.clear();
    }

    fn dropped_samples(&self) -> u64 {
        self.dropped
    }
}

// ------------------------------------------------------------- entry points

/// Lists nRSP-ST receivers. Returns an empty list: the network address must be
/// given by the operator, and there is no discovery.
pub fn probe() -> Vec<DeviceInfo> {
    Vec::new()
}

/// Opens the receiver at `id` (host:port). Connects, upgrades to WebSocket,
/// and asks for the server's basic properties.
pub fn open(id: Option<&str>) -> Result<Box<dyn RxRadio>, String> {
    let addr = match id.map(str::trim) {
        Some(a) if !a.is_empty() => a,
        _ => return Err(NO_ADDRESS.into()),
    };
    let (host, port) = split_host_port(addr).ok_or_else(|| {
        format!("nRSP-ST: bad address {addr:?}; use host:port, for example 192.168.1.5:5454")
    })?;
    let stream = connect(host, port, addr)?;
    let mut link = Link::new(stream);
    link.handshake(host, port, addr)?;
    let peer = ask_peer(&mut link, addr)?;
    Ok(Box::new(NrspSt {
        addr: addr.to_string(),
        link: Some(link),
        lna_min: peer.lna_min.unwrap_or(0),
        lna_max: peer.lna_max.unwrap_or(FALLBACK_LNA_RANGE_DB),
        pending: VecDeque::new(),
        dropped: 0,
        last_data: Instant::now(),
        started: false,
    }))
}

fn connect(host: &str, port: u16, addr: &str) -> Result<TcpStream, String> {
    let targets = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("nRSP-ST: cannot find the address {host}: {e}"))?;
    let mut last = String::from("no address found");
    for t in targets {
        match TcpStream::connect_timeout(&t, CONNECT_TIMEOUT) {
            Ok(s) => return Ok(s),
            Err(e) => {
                last = match e.kind() {
                    ErrorKind::ConnectionRefused => "the connection was refused".to_string(),
                    ErrorKind::TimedOut => format!("no answer within {} s", CONNECT_TIMEOUT.as_secs()),
                    _ => e.to_string(),
                };
            }
        }
    }
    Err(connect_error_text(addr, &last))
}

/// Asks the server its basic properties. Reads answers until the key ones
/// arrive or the server goes quiet.
fn ask_peer(link: &mut Link, addr: &str) -> Result<Peer, String> {
    let dropped = || format!("nRSP-ST: the connection to {addr} dropped");
    for p in PEER_QUESTIONS {
        link.send_text(&control_message("get_property", p, ""))
            .map_err(|_| dropped())?;
    }
    link.set_timeout(ANSWER_TIMEOUT);
    let mut peer = Peer::default();
    for _ in 0..64 {
        match link.read_message() {
            Incoming::Text(t) => {
                if let Some(kv) = parse_flat_json(&t) {
                    absorb(&mut peer, &kv);
                }
            }
            Incoming::Binary(_) => {}
            Incoming::Timeout => break,
            Incoming::Closed => return Err(dropped()),
            Incoming::Bad(why) => return Err(format!("nRSP-ST: bad data from {addr}: {why}")),
        }
        if peer.api_version.is_some() && peer.lna_min.is_some() && peer.lna_max.is_some() {
            break;
        }
    }
    if peer.api_version.is_none() {
        return Err(format!(
            "nRSP-ST: {addr} answered, but not with the expected API (no api_version). Check the address and port."
        ));
    }
    Ok(peer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    // ---- pure helpers

    #[test]
    fn rates_are_ascending_and_inside_the_range() {
        let r = sample_rates();
        assert_eq!(r.first(), Some(&MIN_RATE_HZ));
        assert_eq!(r.last(), Some(&MAX_RATE_HZ));
        assert!(r.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn rate_is_clamped_to_the_range() {
        assert_eq!(clamp_rate(100), 62_500);
        assert_eq!(clamp_rate(2_500_000), 2_500_000);
        assert_eq!(clamp_rate(1_000_000_000), 10_000_000);
    }

    #[test]
    fn gain_is_clamped_and_rounded() {
        assert_eq!(clamp_gain_db(4.6, 9), 5);
        assert_eq!(clamp_gain_db(4.4, 9), 4);
        assert_eq!(clamp_gain_db(-3.0, 9), 0);
        assert_eq!(clamp_gain_db(100.0, 9), 9);
        assert_eq!(clamp_gain_db(f32::NAN, 9), 0);
    }

    #[test]
    fn lna_range_uses_the_server_states_or_the_fallback() {
        assert_eq!(lna_range_db(0, 8), 8);
        assert_eq!(lna_range_db(2, 2), FALLBACK_LNA_RANGE_DB);
        assert_eq!(lna_range_db(3, 1), FALLBACK_LNA_RANGE_DB);
    }

    #[test]
    fn lna_state_counts_down_from_the_maximum() {
        assert_eq!(lna_state_for(0, 9), 9);
        assert_eq!(lna_state_for(4, 9), 5);
    }

    #[test]
    fn message_split_reads_the_type_and_payload() {
        let (t, p) = split_message(&[2, 0, 0xAA, 0xBB]).unwrap();
        assert_eq!(t, IQ_MESSAGE_TYPE);
        assert_eq!(p, &[0xAA, 0xBB]);
        assert_eq!(split_message(&[0x34, 0x12]).unwrap().0, 0x1234);
        assert!(split_message(&[2]).is_none());
    }

    #[test]
    fn sample_conversion_scales_signed_16_bit_values() {
        assert_eq!(iq_from_i16_le(&[0x00, 0x40, 0x00, 0xC0]), vec![(0.5, -0.5)]);
        assert_eq!(
            iq_from_i16_le(&[0x00, 0x80, 0xFF, 0x7F]),
            vec![(-1.0, 32767.0 / 32768.0)]
        );
        assert_eq!(iq_from_i16_le(&[0x00, 0x40, 0x00, 0xC0, 0x01]).len(), 1);
    }

    #[test]
    fn control_message_has_the_reference_layout_and_escapes() {
        assert_eq!(
            control_message("set_property", "lna_state", "4"),
            r#"{"event_type":"set_property","property":"lna_state","value":"4"}"#
        );
        assert_eq!(json_escape("a\"b\\c\n"), "\"a\\\"b\\\\c\\u000a\"");
    }

    #[test]
    fn flat_json_parses_strings_bare_values_and_skips_nested() {
        let kv = parse_flat_json(r#"{"event_type": "property_changed", "property":"device_sample_rate", "value": 2000000, "extra":{"a":[1,2]}}"#)
            .unwrap();
        assert_eq!(kv["event_type"], "property_changed");
        assert_eq!(kv["property"], "device_sample_rate");
        assert_eq!(kv["value"], "2000000");
        assert!(!kv.contains_key("extra"));
        assert!(parse_flat_json("[1,2]").is_none());
        assert!(parse_flat_json(r#"{"a" 1}"#).is_none());
        assert!(parse_flat_json("{}").unwrap().is_empty());
    }

    #[test]
    fn flat_json_round_trips_escaped_text() {
        let text = control_message("get_property", "x", "a\"b\\c");
        let kv = parse_flat_json(&text).unwrap();
        assert_eq!(kv["value"], "a\"b\\c");
    }

    #[test]
    fn host_port_split_handles_ipv6_and_bad_ports() {
        assert_eq!(split_host_port("127.0.0.1:5454"), Some(("127.0.0.1", 5454)));
        assert_eq!(split_host_port("[::1]:5454"), Some(("::1", 5454)));
        assert_eq!(split_host_port("127.0.0.1"), None);
        assert_eq!(split_host_port("127.0.0.1:0"), None);
        assert_eq!(split_host_port("127.0.0.1:99999"), None);
    }

    #[test]
    fn sha1_and_base64_match_the_known_answers() {
        let d = sha1(b"abc");
        let hex: String = d.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        // The accept key example from RFC 6455.
        let accept = base64(&sha1(format!("dGhlIHNhbXBsZSBub25jZQ=={WS_GUID}").as_bytes()));
        assert_eq!(accept, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn probe_lists_nothing() {
        assert!(probe().is_empty());
    }

    #[test]
    fn empty_address_is_refused_with_a_clear_message() {
        let err = open(None).err().unwrap();
        assert!(err.starts_with("nRSP-ST:"));
        assert!(err.contains("host:port"));
        assert!(open(Some("  ")).is_err());
    }

    #[test]
    fn bad_address_is_refused() {
        let err = open(Some("no-port-here")).err().unwrap();
        assert!(err.contains("bad address"));
    }

    // ---- fake peer on 127.0.0.1

    const WS_ANSWERS: &[(&str, &str)] = &[
        ("api_version", "1.0.3"),
        ("valid_devices", "nRSP-ST A"),
        ("lna_state_min", "0"),
        ("lna_state_max", "9"),
        ("can_control", "true"),
        ("device_sample_rate", "2000000"),
    ];

    fn read_client_frame(s: &mut TcpStream) -> Option<(u8, Vec<u8>)> {
        let mut h = [0u8; 2];
        s.read_exact(&mut h).ok()?;
        if h[1] & 0x80 == 0 {
            return None;
        }
        let op = h[0] & 0x0F;
        let mut len = (h[1] & 0x7F) as usize;
        if len == 126 {
            let mut x = [0u8; 2];
            s.read_exact(&mut x).ok()?;
            len = u16::from_be_bytes(x) as usize;
        }
        let mut m = [0u8; 4];
        s.read_exact(&mut m).ok()?;
        let mut p = vec![0u8; len];
        s.read_exact(&mut p).ok()?;
        for (i, b) in p.iter_mut().enumerate() {
            *b ^= m[i & 3];
        }
        Some((op, p))
    }

    fn send_server_frame(s: &mut TcpStream, op: u8, payload: &[u8]) {
        let mut f = vec![0x80 | op, payload.len() as u8];
        f.extend_from_slice(payload);
        s.write_all(&f).unwrap();
    }

    /// Does the WebSocket handshake, sends the basic answers, then records every
    /// text message from the client. Answers the stream start with one burst of
    /// two I/Q samples. Ends when the client closes.
    fn fake_peer(listener: TcpListener, seen: mpsc::Sender<String>) {
        let (mut s, _) = listener.accept().unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut head: Vec<u8> = Vec::new();
        let mut b = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            s.read_exact(&mut b).unwrap();
            head.push(b[0]);
        }
        let text = String::from_utf8(head).unwrap();
        let key = text
            .lines()
            .find_map(|l| l.strip_prefix("Sec-WebSocket-Key: "))
            .unwrap()
            .trim()
            .to_string();
        let accept = base64(&sha1(format!("{key}{WS_GUID}").as_bytes()));
        write!(
            s,
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
        )
        .unwrap();
        for (p, v) in WS_ANSWERS {
            send_server_frame(
                &mut s,
                0x1,
                control_message("get_property_response", p, v).as_bytes(),
            );
        }
        while let Some((op, payload)) = read_client_frame(&mut s) {
            match op {
                0x1 => {
                    let t = String::from_utf8(payload).unwrap();
                    let kv = parse_flat_json(&t).unwrap();
                    let event = kv["event_type"].clone();
                    let _ = seen.send(t);
                    if event == "iq_stream_enable" {
                        // Samples: (0.5, -0.5) and (0, 0), type 2 in front.
                        let burst = [2u8, 0, 0x00, 0x40, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00];
                        send_server_frame(&mut s, 0x2, &burst);
                    }
                }
                0x8 => break,
                _ => {}
            }
        }
    }

    /// Waits until the fake peer has received a message for `prop` with `value`.
    fn expect_set(rx: &mpsc::Receiver<String>, prop: &str, value: &str) {
        loop {
            let t = rx
                .recv_timeout(Duration::from_secs(3))
                .unwrap_or_else(|_| panic!("no {prop} message"));
            let kv = parse_flat_json(&t).unwrap();
            let named = kv.get("property").map(String::as_str) == Some(prop)
                || kv.get("event_type").map(String::as_str) == Some(prop);
            if named && kv.get("value").map(String::as_str) == Some(value) {
                return;
            }
        }
    }

    fn free_port() -> u16 {
        TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    #[test]
    fn refused_connection_is_reported() {
        let port = free_port();
        let err = open(Some(&format!("127.0.0.1:{port}"))).err().unwrap();
        assert!(err.starts_with("nRSP-ST:"), "{err}");
        assert!(err.contains("refused"), "{err}");
    }

    #[test]
    fn silent_peer_times_out_in_the_handshake() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let t = thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 512];
            let _ = s.read(&mut buf);
            thread::sleep(Duration::from_secs(4));
        });
        let started = Instant::now();
        let err = open(Some(&format!("127.0.0.1:{port}"))).err().unwrap();
        assert!(err.contains("no answer"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(4));
        t.join().unwrap();
    }

    #[test]
    fn full_session_against_the_fake_peer() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel();
        let peer = thread::spawn(move || fake_peer(listener, tx));

        let mut r = open(Some(&format!("127.0.0.1:{port}"))).unwrap();
        assert!(r.label().contains("127.0.0.1"));
        assert_eq!(r.freq_range_hz(), (MIN_FREQ_HZ, MAX_FREQ_HZ));

        r.set_frequency_hz(144_300_000).unwrap();
        expect_set(&rx, "device_center_frequency", "144300000");
        expect_set(&rx, "device_vfo_frequency", "144300000");
        assert!(r.set_frequency_hz(5).is_err());

        assert_eq!(r.set_sample_rate(2_500_000).unwrap(), 2_500_000);
        expect_set(&rx, "device_sample_rate", "2500000");
        assert_eq!(r.set_sample_rate(100).unwrap(), 62_500);

        let stages = r.gain_stages();
        assert_eq!(stages[0].name, "LNA");
        assert_eq!(stages[0].max_db, 9.0);
        r.set_gain("LNA", GainSetting::Manual(4.6)).unwrap();
        expect_set(&rx, "lna_state", "4");
        assert!(r.set_gain("LNA", GainSetting::Auto).is_err());
        assert!(r.set_gain("IF", GainSetting::Manual(1.0)).is_err());

        r.start().unwrap();
        expect_set(&rx, "iq_stream_enable", "true");

        let mut one = [(0.0f32, 0.0f32); 1];
        assert_eq!(r.read_iq(&mut one).unwrap(), 1);
        assert_eq!(one[0], (0.5, -0.5));
        let mut four = [(9.0f32, 9.0f32); 4];
        assert_eq!(r.read_iq(&mut four).unwrap(), 1);
        assert_eq!(four[0], (0.0, 0.0));

        // Nothing more is sent, so read_iq returns 0 after about 200 ms.
        let started = Instant::now();
        let mut buf = [(0.0f32, 0.0f32); 16];
        assert_eq!(r.read_iq(&mut buf).unwrap(), 0);
        assert!(started.elapsed() < Duration::from_millis(800));
        assert_eq!(r.dropped_samples(), 0);

        r.stop();
        peer.join().unwrap();
    }
}
