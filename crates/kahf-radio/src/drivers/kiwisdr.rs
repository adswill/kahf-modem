//! Driver for KiwiSDR, a network receiver for 0 to 30 MHz. The id is host:port
//! (the port defaults to 8073; an IPv6 address goes in brackets, for example [::1]:8073).
//!
//! The server streams 12 kHz of IQ around the tuned frequency over a WebSocket.
//! This module has its own small WebSocket client over std::net. It sends the
//! HTTP upgrade request, masks every frame it sends, and reads server frames
//! (binary, text, fragments, ping, close). The SHA-1 check of the upgrade reply
//! is not done; only the 101 status line is checked.
//!
//! No KiwiSDR was available while this was written. The tests cover the text
//! commands, the frame and message decoding, and the error texts. They do not
//! test a live connection.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::{DeviceInfo, GainSetting, GainStage, RxRadio};

const DEFAULT_PORT: u16 = 8073;
/// The server's one IQ rate. Every request for another rate gets this one.
const IQ_RATE: u32 = 12_000;
const MIN_FREQ_HZ: u64 = 10_000;
const MAX_FREQ_HZ: u64 = 30_000_000;
/// Half-width of the passband in Hz. The stream carries 12 kHz, so +-5 kHz stays inside it.
const PASSBAND_HZ: u32 = 5_000;
const GAIN_STAGE: &str = "RF gain";
const GAIN_MAX_DB: f32 = 120.0;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
const ANSWER_TIMEOUT: Duration = Duration::from_secs(6);
/// read_iq waits at most this long for data.
const READ_WAIT: Duration = Duration::from_millis(200);
const KEEPALIVE_EVERY: Duration = Duration::from_secs(1);
const MAX_MESSAGE: usize = 64 * 1024;
const MAX_HEADER: usize = 4096;
/// Samples held back when the reader is late. Older ones are dropped above this.
const MAX_PENDING: usize = 48_000;

/// Network radios are named by the operator, so there is nothing to scan for.
pub fn probe() -> Vec<DeviceInfo> {
    Vec::new()
}

pub fn open(id: Option<&str>) -> Result<Box<dyn RxRadio>, String> {
    let addr = parse_address(id.unwrap_or(""))?;
    Ok(Box::new(KiwiSdr::new(addr)))
}

/// A server address. `host` has no brackets, even for IPv6.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    pub host: String,
    pub port: u16,
}

impl Address {
    /// host:port, with brackets around an IPv6 host.
    pub fn display(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

pub fn parse_address(text: &str) -> Result<Address, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("no KiwiSDR named: give host:port, for example 192.168.1.20:8073".into());
    }
    let (host, port_text) = if let Some(rest) = text.strip_prefix('[') {
        let end = rest
            .find(']')
            .ok_or_else(|| "an IPv6 address needs a closing bracket, for example [::1]:8073".to_string())?;
        let host = &rest[..end];
        let after = &rest[end + 1..];
        if after.is_empty() {
            (host, None)
        } else if let Some(port) = after.strip_prefix(':') {
            (host, Some(port))
        } else {
            return Err(format!("unexpected text {after:?} after the IPv6 address"));
        }
    } else if text.matches(':').count() > 1 {
        return Err("an IPv6 address needs brackets, for example [2001:db8::1]:8073".into());
    } else {
        match text.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (text, None),
        }
    };
    if host.trim().is_empty() {
        return Err("the KiwiSDR host name is empty; give host:port".into());
    }
    let port = match port_text {
        None => DEFAULT_PORT,
        Some(p) => match p.trim().parse::<u16>() {
            Ok(n) if n != 0 => n,
            _ => return Err(format!("the port {p:?} is not a number from 1 to 65535")),
        },
    };
    Ok(Address {
        host: host.trim().to_string(),
        port,
    })
}

// ---- text commands -------------------------------------------------------------------------------

/// No password. The operator cannot name one yet.
pub fn cmd_auth() -> String {
    "SET auth t=kiwi p=#".into()
}

pub fn cmd_ident() -> String {
    "SET ident_user=KAHF".into()
}

/// Sent once after the hand-shake, before tuning. Turns off the server's audio compression and
/// its extra signal sources, so that only plain IQ frames come back.
pub const CONFIG_COMMANDS: &[&str] = &[
    "SET compression=0",
    "SET squelch=0 max=0",
    "SET genattn=0",
    "SET gen=0 mix=-1",
];

pub fn cmd_keepalive() -> String {
    "SET keepalive".into()
}

pub fn cmd_ar_ok(input_rate: u32) -> String {
    format!("SET AR OK in={input_rate} out=44100")
}

/// IQ mode with a symmetric passband. The frequency is in kHz with three decimals.
pub fn cmd_tune(freq_hz: u64) -> String {
    format!(
        "SET mod=iq low_cut=-{PASSBAND_HZ} high_cut={PASSBAND_HZ} freq={:.3}",
        freq_hz as f64 / 1000.0
    )
}

pub fn cmd_agc(setting: GainSetting) -> String {
    match setting {
        GainSetting::Auto => "SET agc=1 hang=0 thresh=-100 slope=6 decay=1000 manGain=0".into(),
        GainSetting::Manual(db) => {
            let g = db.clamp(0.0, GAIN_MAX_DB).round() as i32;
            format!("SET agc=0 hang=0 thresh=-100 slope=6 decay=1000 manGain={g}")
        }
    }
}

/// The text of the GET request that starts the WebSocket upgrade.
pub fn handshake_request(addr: &Address, path: &str, key: &str) -> String {
    format!(
        "GET {path} HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n",
        addr.display()
    )
}

// ---- server messages (MSG) -----------------------------------------------------------------------

fn url_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(v) = hex {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A MSG body is a 3-byte tag, one more byte, then space separated name=value tokens.
/// Values are URL-encoded. A token with no '=' is a flag and gets an empty value.
pub fn parse_msg(body: &[u8]) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for token in body
        .get(4..)
        .unwrap_or(&[])
        .split(|b| *b == b' ' || *b == b'\n' || *b == 0)
    {
        if token.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(token);
        match text.split_once('=') {
            Some((name, value)) => pairs.push((name.to_string(), url_decode(value))),
            None => pairs.push((text.to_string(), String::new())),
        }
    }
    pairs
}

/// The error text for a MSG name=value that ends the session, or None for an ordinary one.
pub fn msg_error(name: &str, value: &str) -> Option<String> {
    match name {
        "too_busy" => Some(format!(
            "all user slots of the KiwiSDR are in use ({value} users); try again later"
        )),
        "badp" => match value {
            "0" => None, // the password was accepted
            "1" => Some("the KiwiSDR refused the connection: wrong password, or no free channel".into()),
            "2" => Some("the KiwiSDR is still starting up; try again in a moment".into()),
            "5" => Some(
                "this KiwiSDR allows only one connection from this address; close the other one first".into(),
            ),
            "6" => Some("the KiwiSDR is updating its database; try again in about a minute".into()),
            other => Some(format!("the KiwiSDR refused the connection (code {other})")),
        },
        "down" => Some("the KiwiSDR is down (maintenance or restart)".into()),
        "redirect" => {
            Some("the KiwiSDR asks to use another address; this driver does not follow redirects".into())
        }
        "inactivity_timeout" | "ip_limit_exceeded" | "tlimit" | "time_limit" => {
            Some("the KiwiSDR's time limit for one listener ran out".into())
        }
        "kick_by_admin" | "kicked" => Some("the owner of the KiwiSDR disconnected this session".into()),
        _ => None,
    }
}

// ---- audio/IQ frames (SND) -----------------------------------------------------------------------

const SND_STEREO: u8 = 0x08; // IQ, not audio
const SND_COMPRESSED: u8 = 0x10; // ADPCM audio
const SND_LITTLE_ENDIAN: u8 = 0x80;

#[derive(Debug, Clone, PartialEq)]
pub struct Snd {
    pub seq: u32,
    pub samples: Vec<(f32, f32)>,
}

/// Decodes one SND frame. Ok(None) for a frame that is not IQ (audio); the caller ignores it.
/// Layout: tag (3 bytes), flags (1), sequence (u32 LE at 4), S-meter (u16 BE at 8),
/// a 10-byte GPS block (at 10), then interleaved s16 samples from byte 20: I first, then Q.
pub fn parse_snd(msg: &[u8]) -> Result<Option<Snd>, String> {
    if msg.len() < 10 {
        return Err(format!("a KiwiSDR frame is too short ({} bytes)", msg.len()));
    }
    let flags = msg[3];
    if flags & SND_COMPRESSED != 0 || flags & SND_STEREO == 0 {
        return Ok(None);
    }
    if msg.len() < 20 {
        return Err(format!("a KiwiSDR IQ frame is too short ({} bytes)", msg.len()));
    }
    let seq = u32::from_le_bytes([msg[4], msg[5], msg[6], msg[7]]);
    let little = flags & SND_LITTLE_ENDIAN != 0;
    let samples = msg[20..]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| {
            let read = |b: &[u8]| {
                let v = if little {
                    i16::from_le_bytes([b[0], b[1]])
                } else {
                    i16::from_be_bytes([b[0], b[1]])
                };
                v as f32 / 32768.0
            };
            (read(&c[0..2]), read(&c[2..4]))
        })
        .collect();
    Ok(Some(Snd { seq, samples }))
}

// ---- WebSocket framing ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub fin: bool,
    pub opcode: u8,
    pub payload: Vec<u8>,
}

/// One client frame. Client frames are always masked.
pub fn encode_client_frame(opcode: u8, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
    let mut out = vec![0x80 | opcode];
    let len = payload.len();
    if len < 126 {
        out.push(0x80 | len as u8);
    } else if len <= 0xFFFF {
        out.push(0x80 | 126);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(0x80 | 127);
        out.extend_from_slice(&(len as u64).to_be_bytes());
    }
    out.extend_from_slice(&mask);
    out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    out
}

/// Reads one server frame from the start of `buf`. Ok(None) when the frame is not complete yet.
/// Returns the frame and the number of bytes it used.
pub fn parse_server_frame(buf: &[u8]) -> Result<Option<(Frame, usize)>, String> {
    if buf.len() < 2 {
        return Ok(None);
    }
    if buf[0] & 0x70 != 0 {
        return Err("the KiwiSDR sent a frame with reserved bits set".into());
    }
    if buf[1] & 0x80 != 0 {
        return Err("the KiwiSDR sent a masked frame, which a server must not do".into());
    }
    let fin = buf[0] & 0x80 != 0;
    let opcode = buf[0] & 0x0F;
    let (len, head) = match buf[1] & 0x7F {
        126 => {
            if buf.len() < 4 {
                return Ok(None);
            }
            (u16::from_be_bytes([buf[2], buf[3]]) as u64, 4)
        }
        127 => {
            if buf.len() < 10 {
                return Ok(None);
            }
            let mut b = [0u8; 8];
            b.copy_from_slice(&buf[2..10]);
            (u64::from_be_bytes(b), 10)
        }
        n => (n as u64, 2),
    };
    if len > MAX_MESSAGE as u64 {
        return Err(format!(
            "a frame of {len} bytes is over the {MAX_MESSAGE} byte limit"
        ));
    }
    let len = len as usize;
    if buf.len() < head + len {
        return Ok(None);
    }
    let payload = buf[head..head + len].to_vec();
    Ok(Some((Frame { fin, opcode, payload }, head + len)))
}

/// Standard base64 with padding. Used for the upgrade key only.
pub fn base64(bytes: &[u8]) -> String {
    const ALPHA: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(ALPHA[(n >> 18) as usize & 63] as char);
        out.push(ALPHA[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHA[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHA[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

// ---- the driver ----------------------------------------------------------------------------------

pub struct KiwiSdr {
    addr: Address,
    label: String,
    stream: Option<TcpStream>,
    inbuf: Vec<u8>,
    /// Parts of a fragmented message, while more are expected.
    fragments: Option<Vec<u8>>,
    pending: VecDeque<(f32, f32)>,
    freq_hz: u64,
    gain: GainSetting,
    last_seq: Option<u32>,
    dropped: u64,
    last_keepalive: Instant,
    /// Set by the server's sample_rate line during the hand-shake.
    server_rate: Option<f64>,
    version: String,
    rng: u64,
}

impl KiwiSdr {
    fn new(addr: Address) -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);
        KiwiSdr {
            label: format!("KiwiSDR at {}", addr.display()),
            addr,
            stream: None,
            inbuf: Vec::new(),
            fragments: None,
            pending: VecDeque::new(),
            freq_hz: 7_074_000,
            gain: GainSetting::Auto,
            last_seq: None,
            dropped: 0,
            last_keepalive: Instant::now(),
            server_rate: None,
            version: String::new(),
            rng: seed | 1,
        }
    }

    /// Four bytes from a small xorshift generator. Masks do not need to be secret.
    fn next_mask(&mut self) -> [u8; 4] {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        (x as u32).to_le_bytes()
    }

    fn send_frame(&mut self, opcode: u8, payload: &[u8]) -> Result<(), String> {
        let mask = self.next_mask();
        let bytes = encode_client_frame(opcode, payload, mask);
        let addr = self.addr.display();
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| format!("the KiwiSDR at {addr} is not connected"))?;
        stream
            .write_all(&bytes)
            .map_err(|e| format!("could not send to the KiwiSDR at {addr}: {e}"))
    }

    fn send_command(&mut self, cmd: &str) -> Result<(), String> {
        self.send_frame(0x1, cmd.as_bytes())
    }

    /// Reads whatever arrives before the deadline. Ok(false) when nothing came in time.
    fn read_some(&mut self, deadline: Instant) -> Result<bool, String> {
        let now = Instant::now();
        if now >= deadline {
            return Ok(false);
        }
        let addr = self.addr.display();
        let mut chunk = [0u8; 8192];
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| format!("the KiwiSDR at {addr} is not connected"))?;
        stream
            .set_read_timeout(Some(deadline - now))
            .map_err(|e| format!("could not set the read wait for the KiwiSDR at {addr}: {e}"))?;
        match stream.read(&mut chunk) {
            Ok(0) => Err(format!("the KiwiSDR at {addr} closed the connection")),
            Ok(n) => {
                self.inbuf.extend_from_slice(&chunk[..n]);
                Ok(true)
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) =>
            {
                Ok(false)
            }
            Err(e) => Err(format!("lost the connection to the KiwiSDR at {addr}: {e}")),
        }
    }

    /// The next complete data message, or None when the deadline passes first.
    /// Pings are answered here. A close frame is an error.
    fn take_message(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>, String> {
        loop {
            if let Some((frame, used)) = parse_server_frame(&self.inbuf)? {
                self.inbuf.drain(..used);
                match frame.opcode {
                    0x8 => {
                        return Err(format!(
                            "the KiwiSDR at {} closed the connection",
                            self.addr.display()
                        ))
                    }
                    0x9 => self.send_frame(0xA, &frame.payload)?,
                    0xA => {}
                    0x0..=0x2 => {
                        if frame.opcode != 0 {
                            if self.fragments.is_some() {
                                return Err("the KiwiSDR started a message before the last one ended".into());
                            }
                            self.fragments = Some(Vec::new());
                        }
                        let buf = self.fragments.as_mut().ok_or_else(|| {
                            "the KiwiSDR sent a continuation frame with no message to continue".to_string()
                        })?;
                        buf.extend_from_slice(&frame.payload);
                        if buf.len() > MAX_MESSAGE {
                            return Err(format!(
                                "a message from the KiwiSDR is over the {MAX_MESSAGE} byte limit"
                            ));
                        }
                        if frame.fin {
                            return Ok(self.fragments.take());
                        }
                    }
                    other => return Err(format!("the KiwiSDR sent an unknown frame type {other}")),
                }
                continue;
            }
            if !self.read_some(deadline)? {
                return Ok(None);
            }
        }
    }

    /// Reads the HTTP reply to the upgrade request. Only the status line is checked.
    fn read_upgrade_reply(&mut self) -> Result<(), String> {
        let deadline = Instant::now() + ANSWER_TIMEOUT;
        loop {
            if let Some(pos) = find_subslice(&self.inbuf, b"\r\n\r\n") {
                let head: Vec<u8> = self.inbuf.drain(..pos + 4).collect();
                let text = String::from_utf8_lossy(&head);
                let status = text.lines().next().unwrap_or("").to_string();
                if status.starts_with("HTTP/1.1 101") {
                    return Ok(());
                }
                return Err(format!(
                    "the KiwiSDR at {} did not accept the WebSocket request (reply: {status:?}); is it a KiwiSDR? the default port is 8073",
                    self.addr.display()
                ));
            }
            if self.inbuf.len() > MAX_HEADER {
                return Err(format!(
                    "the KiwiSDR at {} sent a reply header that is too long",
                    self.addr.display()
                ));
            }
            if !self.read_some(deadline)? {
                return Err(format!(
                    "the KiwiSDR at {} did not answer the WebSocket request in time",
                    self.addr.display()
                ));
            }
        }
    }

    /// Handles one MSG. Errors that end the session are returned as text.
    fn handle_message(&mut self, msg: &[u8]) -> Result<(), String> {
        for (name, value) in parse_msg(msg) {
            if let Some(text) = msg_error(&name, &value) {
                return Err(text);
            }
            match name.as_str() {
                "sample_rate" => self.server_rate = value.parse::<f64>().ok(),
                "audio_rate" => {
                    let rate = value.parse::<u32>().unwrap_or(0);
                    self.send_command(&cmd_ar_ok(rate))?;
                }
                "version_maj" => self.version = format!("{value}{}", self.version),
                "version_min" => self.version.push_str(&format!(".{value}")),
                _ => {}
            }
        }
        Ok(())
    }

    fn connect(&mut self) -> Result<(), String> {
        let addr_text = self.addr.display();
        let target = addr_text
            .as_str()
            .to_socket_addrs()
            .map_err(|e| format!("cannot find the KiwiSDR at {addr_text}: {e}"))?
            .next()
            .ok_or_else(|| format!("no address found for the KiwiSDR at {addr_text}"))?;
        let stream = TcpStream::connect_timeout(&target, CONNECT_TIMEOUT).map_err(|e| {
            format!("cannot connect to the KiwiSDR at {addr_text}: {e}; check the host, the port and that the server is online")
        })?;
        let _ = stream.set_nodelay(true);
        let _ = stream.set_write_timeout(Some(Duration::from_secs(4)));
        self.stream = Some(stream);
        self.inbuf.clear();
        self.fragments = None;
        self.pending.clear();
        self.last_seq = None;
        self.server_rate = None;
        self.version.clear();

        let mut raw = [0u8; 16];
        for chunk in raw.chunks_mut(4) {
            chunk.copy_from_slice(&self.next_mask());
        }
        let key = base64(&raw);
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let path = format!("/kiwi/{secs}/SND");
        let request = handshake_request(&self.addr, &path, &key);
        self.send_raw_text(request.as_bytes())?;
        self.read_upgrade_reply()
    }

    fn send_raw_text(&mut self, bytes: &[u8]) -> Result<(), String> {
        let addr = self.addr.display();
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| format!("the KiwiSDR at {addr} is not connected"))?;
        stream
            .write_all(bytes)
            .map_err(|e| format!("could not send to the KiwiSDR at {addr}: {e}"))
    }

    /// Reads server messages until the sample rate arrives, then checks it.
    fn read_server_hello(&mut self) -> Result<(), String> {
        self.send_command(&cmd_auth())?;
        let deadline = Instant::now() + ANSWER_TIMEOUT;
        while self.server_rate.is_none() {
            match self.take_message(deadline)? {
                Some(msg) => {
                    if msg.starts_with(b"MSG") {
                        self.handle_message(&msg)?;
                    }
                }
                None => {
                    return Err(format!(
                        "the KiwiSDR at {} did not finish the start-up messages in time (no sample rate)",
                        self.addr.display()
                    ))
                }
            }
        }
        let rate = self.server_rate.unwrap_or(0.0);
        if !(8000.0..=16000.0).contains(&rate) {
            return Err(format!(
                "the KiwiSDR at {} reports an IQ rate of {} Hz, not the 12 kHz of a KiwiSDR",
                self.addr.display(),
                rate as u32
            ));
        }
        Ok(())
    }

    fn configure_stream(&mut self) -> Result<(), String> {
        self.send_command(&cmd_ident())?;
        for cmd in CONFIG_COMMANDS {
            self.send_command(cmd)?;
        }
        self.send_command(&cmd_keepalive())?;
        self.last_keepalive = Instant::now();
        self.send_command(&cmd_tune(self.freq_hz))?;
        self.send_command(&cmd_agc(self.gain))?;
        Ok(())
    }

    fn start_inner(&mut self) -> Result<(), String> {
        self.connect()?;
        self.read_server_hello()?;
        if !self.version.is_empty() {
            self.label = format!("KiwiSDR at {}, version {}", self.addr.display(), self.version);
        }
        self.configure_stream()
    }

    fn close(&mut self) {
        if let Some(mut stream) = self.stream.take() {
            let mask = self.next_mask();
            let bytes = encode_client_frame(0x8, &[], mask);
            let _ = stream.write_all(&bytes);
            let _ = stream.shutdown(Shutdown::Both);
        }
        self.inbuf.clear();
        self.fragments = None;
        self.pending.clear();
    }

    fn push_snd(&mut self, msg: &[u8]) -> Result<(), String> {
        let snd = match parse_snd(msg)? {
            Some(s) => s,
            None => return Ok(()),
        };
        if let Some(last) = self.last_seq {
            if snd.seq == last {
                return Ok(()); // the same frame again
            }
            let gap = snd.seq.wrapping_sub(last.wrapping_add(1));
            // A step backwards (the server counts from a new start) is followed, not counted.
            if gap < 0x8000_0000 {
                self.dropped += gap as u64 * snd.samples.len() as u64;
            }
        }
        self.last_seq = Some(snd.seq);
        self.pending.extend(snd.samples);
        if self.pending.len() > MAX_PENDING {
            let extra = self.pending.len() - MAX_PENDING;
            self.pending.drain(..extra);
            self.dropped += extra as u64;
        }
        Ok(())
    }
}

impl Drop for KiwiSdr {
    fn drop(&mut self) {
        self.close();
    }
}

impl RxRadio for KiwiSdr {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn freq_range_hz(&self) -> (u64, u64) {
        (MIN_FREQ_HZ, MAX_FREQ_HZ)
    }

    fn set_frequency_hz(&mut self, hz: u64) -> Result<(), String> {
        if !(MIN_FREQ_HZ..=MAX_FREQ_HZ).contains(&hz) {
            return Err(format!(
                "{hz} Hz is outside the KiwiSDR range of 10 kHz to 30 MHz"
            ));
        }
        self.freq_hz = hz;
        if self.stream.is_some() {
            self.send_command(&cmd_tune(hz))?;
        }
        Ok(())
    }

    fn sample_rates(&self) -> Vec<u32> {
        vec![IQ_RATE]
    }

    /// The server has one rate, so any request returns 12 kHz.
    fn set_sample_rate(&mut self, _hz: u32) -> Result<u32, String> {
        Ok(IQ_RATE)
    }

    fn gain_stages(&self) -> Vec<GainStage> {
        vec![GainStage {
            name: GAIN_STAGE.into(),
            min_db: 0.0,
            max_db: GAIN_MAX_DB,
            step_db: 1.0,
            auto: true,
        }]
    }

    fn set_gain(&mut self, stage: &str, setting: GainSetting) -> Result<(), String> {
        if stage != GAIN_STAGE {
            return Err(format!(
                "this radio has no gain stage named {stage:?}; its stage is {GAIN_STAGE:?}"
            ));
        }
        self.gain = match setting {
            GainSetting::Manual(db) => GainSetting::Manual(db.clamp(0.0, GAIN_MAX_DB)),
            GainSetting::Auto => GainSetting::Auto,
        };
        if self.stream.is_some() {
            self.send_command(&cmd_agc(self.gain))?;
        }
        Ok(())
    }

    fn start(&mut self) -> Result<(), String> {
        if self.stream.is_some() {
            return Ok(());
        }
        let result = self.start_inner();
        if result.is_err() {
            self.close();
        }
        result
    }

    fn read_iq(&mut self, out: &mut [(f32, f32)]) -> Result<usize, String> {
        if self.stream.is_none() {
            return Err(format!("the KiwiSDR at {} is not started", self.addr.display()));
        }
        if out.is_empty() {
            return Ok(0);
        }
        let deadline = Instant::now() + READ_WAIT;
        loop {
            if !self.pending.is_empty() {
                let n = out.len().min(self.pending.len());
                for (slot, sample) in out.iter_mut().zip(self.pending.drain(..n)) {
                    *slot = sample;
                }
                return Ok(n);
            }
            if self.last_keepalive.elapsed() >= KEEPALIVE_EVERY {
                self.last_keepalive = Instant::now();
                self.send_command(&cmd_keepalive())?;
            }
            let msg = match self.take_message(deadline)? {
                Some(m) => m,
                None => return Ok(0),
            };
            if msg.starts_with(b"MSG") {
                self.handle_message(&msg)?;
            } else if msg.starts_with(b"SND") {
                self.push_snd(&msg)?;
            }
        }
    }

    fn stop(&mut self) {
        self.close();
    }

    fn dropped_samples(&self) -> u64 {
        self.dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kiwi(host: &str, port: u16) -> KiwiSdr {
        KiwiSdr::new(Address {
            host: host.into(),
            port,
        })
    }

    #[test]
    fn tune_command_text() {
        assert_eq!(
            cmd_tune(7_074_000),
            "SET mod=iq low_cut=-5000 high_cut=5000 freq=7074.000"
        );
        assert_eq!(
            cmd_tune(10_000),
            "SET mod=iq low_cut=-5000 high_cut=5000 freq=10.000"
        );
        assert_eq!(
            cmd_tune(14_200_500),
            "SET mod=iq low_cut=-5000 high_cut=5000 freq=14200.500"
        );
    }

    #[test]
    fn gain_command_text() {
        assert_eq!(
            cmd_agc(GainSetting::Auto),
            "SET agc=1 hang=0 thresh=-100 slope=6 decay=1000 manGain=0"
        );
        assert_eq!(
            cmd_agc(GainSetting::Manual(45.4)),
            "SET agc=0 hang=0 thresh=-100 slope=6 decay=1000 manGain=45"
        );
        assert_eq!(
            cmd_agc(GainSetting::Manual(200.0)),
            "SET agc=0 hang=0 thresh=-100 slope=6 decay=1000 manGain=120"
        );
        assert_eq!(
            cmd_agc(GainSetting::Manual(-3.0)),
            "SET agc=0 hang=0 thresh=-100 slope=6 decay=1000 manGain=0"
        );
    }

    #[test]
    fn setup_command_text() {
        assert_eq!(cmd_auth(), "SET auth t=kiwi p=#");
        assert_eq!(cmd_ident(), "SET ident_user=KAHF");
        assert_eq!(cmd_keepalive(), "SET keepalive");
        assert_eq!(cmd_ar_ok(12000), "SET AR OK in=12000 out=44100");
        assert!(CONFIG_COMMANDS.contains(&"SET compression=0"));
    }

    #[test]
    fn upgrade_request_text() {
        let addr = Address {
            host: "192.168.1.20".into(),
            port: 8073,
        };
        let text = handshake_request(&addr, "/kiwi/1700000000/SND", "dGhlIHNhbXBsZSBub25jZQ==");
        assert!(text.starts_with("GET /kiwi/1700000000/SND HTTP/1.1\r\n"));
        assert!(text.contains("\r\nHost: 192.168.1.20:8073\r\n"));
        assert!(text.contains("\r\nUpgrade: websocket\r\n"));
        assert!(text.contains("\r\nSec-WebSocket-Version: 13\r\n"));
        assert!(text.ends_with("\r\n\r\n"));
    }

    #[test]
    fn address_parsing() {
        assert_eq!(
            parse_address("192.168.1.20:8073").unwrap(),
            Address {
                host: "192.168.1.20".into(),
                port: 8073
            }
        );
        assert_eq!(
            parse_address("kiwi.example").unwrap(),
            Address {
                host: "kiwi.example".into(),
                port: 8073
            }
        );
        let v6 = parse_address("[::1]:8074").unwrap();
        assert_eq!(
            v6,
            Address {
                host: "::1".into(),
                port: 8074
            }
        );
        assert_eq!(v6.display(), "[::1]:8074");
        assert!(parse_address("").unwrap_err().contains("host:port"));
        assert!(parse_address("::1").unwrap_err().contains("brackets"));
        assert!(parse_address("host:0").unwrap_err().contains("1 to 65535"));
        assert!(parse_address("host:99999").unwrap_err().contains("1 to 65535"));
        assert!(parse_address("host:").is_err());
        assert!(parse_address(":8073").is_err());
    }

    #[test]
    fn open_needs_a_server_name() {
        let err = open(None).err().expect("open without an id must fail");
        assert!(err.contains("host:port"), "{err}");
        assert!(open(Some("kiwi.example:8073")).is_ok());
    }

    #[test]
    fn probe_finds_nothing_to_scan() {
        assert!(probe().is_empty());
    }

    #[test]
    fn base64_known_values() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
        assert_eq!(base64(&[0u8; 16]).len(), 24);
    }

    #[test]
    fn client_frame_is_masked() {
        let bytes = encode_client_frame(0x1, b"ab", [1, 2, 3, 4]);
        assert_eq!(bytes, vec![0x81, 0x82, 1, 2, 3, 4, b'a' ^ 1, b'b' ^ 2]);
    }

    #[test]
    fn client_frame_long_length() {
        let payload = vec![7u8; 200];
        let bytes = encode_client_frame(0x2, &payload, [0, 0, 0, 0]);
        assert_eq!(&bytes[..4], &[0x82, 0xFE, 0, 200]);
        assert_eq!(bytes.len(), 4 + 4 + 200);
    }

    #[test]
    fn server_frame_decoding() {
        let frame = [0x82u8, 3, b'x', b'y', b'z', 0x81, 0x01, b'q'];
        let (f, used) = parse_server_frame(&frame).unwrap().unwrap();
        assert_eq!(used, 5);
        assert_eq!(
            f,
            Frame {
                fin: true,
                opcode: 2,
                payload: b"xyz".to_vec()
            }
        );
        let (g, used2) = parse_server_frame(&frame[5..]).unwrap().unwrap();
        assert_eq!(used2, 3);
        assert_eq!(
            g,
            Frame {
                fin: true,
                opcode: 1,
                payload: b"q".to_vec()
            }
        );
    }

    #[test]
    fn server_frame_with_16_bit_length_and_partial_input() {
        let mut frame = vec![0x82u8, 126, 0, 200];
        frame.extend(std::iter::repeat(9u8).take(200));
        assert!(parse_server_frame(&frame[..100]).unwrap().is_none());
        let (f, used) = parse_server_frame(&frame).unwrap().unwrap();
        assert_eq!(used, 204);
        assert_eq!(f.payload.len(), 200);
        assert!(parse_server_frame(&[0x82]).unwrap().is_none());
    }

    #[test]
    fn server_frame_errors() {
        assert!(parse_server_frame(&[0x82, 0x81, 0, 0, 0, 0, 1])
            .unwrap_err()
            .contains("masked"));
        assert!(parse_server_frame(&[0xC2, 0x00])
            .unwrap_err()
            .contains("reserved"));
        let big = [0x82u8, 127, 0, 0, 0, 1, 0, 0, 0, 0]; // 4 GiB
        assert!(parse_server_frame(&big).unwrap_err().contains("byte limit"));
    }

    #[test]
    fn msg_tokens_are_decoded() {
        let body = b"MSG sample_rate=12000.000 ident=a%20b%2Fc flag too_busy=3";
        let pairs = parse_msg(body);
        assert_eq!(
            pairs,
            vec![
                ("sample_rate".to_string(), "12000.000".to_string()),
                ("ident".to_string(), "a b/c".to_string()),
                ("flag".to_string(), String::new()),
                ("too_busy".to_string(), "3".to_string()),
            ]
        );
        assert!(parse_msg(b"MS").is_empty());
    }

    #[test]
    fn session_error_texts() {
        assert!(msg_error("too_busy", "4").unwrap().contains("4 users"));
        assert!(msg_error("badp", "1").unwrap().contains("wrong password"));
        assert!(msg_error("badp", "5").unwrap().contains("one connection"));
        assert!(msg_error("badp", "0").is_none());
        assert!(msg_error("badp", "9").unwrap().contains("code 9"));
        assert!(msg_error("down", "1").unwrap().contains("down"));
        assert!(msg_error("inactivity_timeout", "1")
            .unwrap()
            .contains("time limit"));
        assert!(msg_error("kick_by_admin", "1").unwrap().contains("disconnected"));
        assert!(msg_error("sample_rate", "12000").is_none());
    }

    #[test]
    fn iq_frame_big_endian_and_little_endian() {
        // Tag, flags (stereo), sequence 5, S-meter 0, 10 GPS bytes, then I = 0x4000, Q = 0xC000.
        let mut msg = b"SND".to_vec();
        msg.push(SND_STEREO);
        msg.extend_from_slice(&5u32.to_le_bytes());
        msg.extend_from_slice(&[0, 0]);
        msg.extend_from_slice(&[0u8; 10]);
        msg.extend_from_slice(&[0x40, 0x00, 0xC0, 0x00]);
        let snd = parse_snd(&msg).unwrap().unwrap();
        assert_eq!(snd.seq, 5);
        assert_eq!(snd.samples, vec![(0.5, -0.5)]);

        // Same sample, little-endian: I = 0x4000 as [0x00, 0x40], Q = 0xC000 as [0x00, 0xC0].
        msg[3] = SND_STEREO | SND_LITTLE_ENDIAN;
        msg[20..24].copy_from_slice(&[0x00, 0x40, 0x00, 0xC0]);
        let snd = parse_snd(&msg).unwrap().unwrap();
        assert_eq!(snd.samples, vec![(0.5, -0.5)]);
    }

    #[test]
    fn audio_and_short_frames() {
        let mut audio = b"SND".to_vec();
        audio.push(SND_COMPRESSED);
        audio.extend_from_slice(&[0u8; 20]);
        assert_eq!(parse_snd(&audio).unwrap(), None);
        assert!(parse_snd(b"SND\x08\x00").unwrap_err().contains("too short"));
        let mut short_iq = b"SND".to_vec();
        short_iq.push(SND_STEREO);
        short_iq.extend_from_slice(&[0u8; 10]);
        assert!(parse_snd(&short_iq).unwrap_err().contains("too short"));
    }

    #[test]
    fn base_driver_api_without_connection() {
        let mut radio = kiwi("kiwi.example", 8073);
        assert_eq!(radio.label(), "KiwiSDR at kiwi.example:8073");
        assert_eq!(radio.freq_range_hz(), (10_000, 30_000_000));
        assert_eq!(radio.sample_rates(), vec![12_000]);
        assert_eq!(radio.set_sample_rate(48_000), Ok(12_000));
        assert!(radio
            .set_frequency_hz(5_000)
            .unwrap_err()
            .contains("outside the KiwiSDR range"));
        assert!(radio.set_frequency_hz(14_074_000).is_ok());
        assert_eq!(radio.gain_stages().len(), 1);
        assert!(radio
            .set_gain("LNA", GainSetting::Auto)
            .unwrap_err()
            .contains("no gain stage named"));
        assert!(radio.set_gain(GAIN_STAGE, GainSetting::Manual(500.0)).is_ok());
        assert_eq!(radio.gain, GainSetting::Manual(120.0));
        let mut out = [(0.0f32, 0.0f32); 4];
        assert!(radio.read_iq(&mut out).unwrap_err().contains("not started"));
        assert_eq!(radio.dropped_samples(), 0);
    }

    #[test]
    fn sequence_gap_is_counted_and_repeat_dropped() {
        let mut radio = kiwi("kiwi.example", 8073);
        let make = |seq: u32| {
            let mut m = b"SND".to_vec();
            m.push(SND_STEREO);
            m.extend_from_slice(&seq.to_le_bytes());
            m.extend_from_slice(&[0, 0]); // S-meter
            m.extend_from_slice(&[0u8; 10]); // GPS block
            m.extend_from_slice(&[0u8; 8]); // two samples
            m
        };
        radio.push_snd(&make(1)).unwrap();
        radio.push_snd(&make(1)).unwrap(); // repeat
        radio.push_snd(&make(4)).unwrap(); // two frames missing = 4 samples
        assert_eq!(radio.dropped_samples(), 4);
        assert_eq!(radio.pending.len(), 4);
    }
}
