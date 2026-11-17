//! Driver for a TCI server. TCI (Transceiver Control Interface) is a WebSocket protocol
//! that streams the IQ samples of a receiver over the network. This driver is receive
//! only: it never sends transmit or audio commands. The operator names the server as
//! host:port. There is no discovery, so probe() returns an empty list.
//!
//! Protocol as used here:
//! - WebSocket connection to ws://host:port/ (no TLS, no login).
//! - Text messages hold commands "name:arg,arg;". One message can hold several.
//! - Binary messages hold a 64-byte header (16 little-endian u32 fields), then the samples.
//!   Header fields used: receiver (0), sample rate (1), format (2), kind (6, 0 = IQ),
//!   channels (7). Formats: 0 = int16, 1 = int24, 2 = int32, 3 = float32.
//! - Commands sent: iq_samplerate, dds, vfo, iq_start, iq_stop. Also start, once per
//!   connection, when the server reports its radio as stopped before any IQ arrived.

use std::collections::VecDeque;
use std::hash::{BuildHasher, Hasher};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::{DeviceInfo, GainSetting, GainStage, RxRadio};

const DEFAULT_PORT: u16 = 50001;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Longest time one read_iq call waits for samples.
const WAIT_MAX: Duration = Duration::from_millis(200);
/// Largest frame or message accepted from the server.
const MAX_FRAME: u64 = 16 << 20;
/// Samples kept waiting for the reader. Older ones are dropped beyond this.
const MAX_PENDING: usize = 1 << 22;
const RATES: [u32; 4] = [48_000, 96_000, 192_000, 384_000];
/// Tuning range used until the server names its own (vfo_limits).
const DEFAULT_RANGE: (u64, u64) = (10_000, 6_000_000_000);
/// The receiver whose IQ is read. The address names only the server.
const RECEIVER: u32 = 0;
const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const NEED_ADDRESS: &str = "TCI needs the server address as host:port, for example 192.168.1.5:50001";

/// Finds the radios this computer can use. TCI servers are named by the operator, so none are found.
pub fn probe() -> Vec<DeviceInfo> {
    Vec::new()
}

pub fn open(id: Option<&str>) -> Result<Box<dyn RxRadio>, String> {
    let text = id.ok_or_else(|| NEED_ADDRESS.to_string())?;
    let (host, port) = parse_address(text)?;
    Ok(Box::new(TciRadio::connect(&host, port)?))
}

enum Message {
    Text(String),
    Binary(Vec<u8>),
}

struct Frame {
    fin: bool,
    opcode: u8,
    payload: Vec<u8>,
}

pub struct TciRadio {
    addr: String,
    stream: TcpStream,
    rng: u64,
    /// Bytes received but not yet parsed into frames.
    rbuf: Vec<u8>,
    /// A fragmented message still being assembled: opcode and bytes so far.
    partial: Option<(u8, Vec<u8>)>,
    device: String,
    range: (u64, u64),
    ready: bool,
    saw_protocol: bool,
    server_stopped: bool,
    rate: u32,
    freq: Option<u64>,
    started: bool,
    iq_seen: bool,
    start_sent: bool,
    pending: VecDeque<(f32, f32)>,
    dropped: u64,
}

impl TciRadio {
    fn connect(host: &str, port: u16) -> Result<Self, String> {
        let addr = format_addr(host, port);
        let targets: Vec<SocketAddr> = (host, port)
            .to_socket_addrs()
            .map_err(|e| format!("cannot find the TCI server host {host:?}: {e}"))?
            .collect();
        let mut last_error = None;
        let mut stream = None;
        for target in &targets {
            match TcpStream::connect_timeout(target, CONNECT_TIMEOUT) {
                Ok(s) => {
                    stream = Some(s);
                    break;
                }
                Err(e) => last_error = Some(e),
            }
        }
        let mut stream = stream.ok_or_else(|| match last_error {
            Some(e) => format!(
                "cannot connect to the TCI server at {addr}: {e}. Check that the server is running, \
                 that TCI is switched on in it, and that the address and port are right"
            ),
            None => format!("no network address found for {addr}"),
        })?;
        let _ = stream.set_nodelay(true);
        stream
            .set_read_timeout(Some(CONNECT_TIMEOUT))
            .and_then(|_| stream.set_write_timeout(Some(CONNECT_TIMEOUT)))
            .map_err(|e| format!("cannot set timeouts on the connection to {addr}: {e}"))?;

        let mut rng = random_seed();
        let key = {
            let a = next_u64(&mut rng).to_le_bytes();
            let b = next_u64(&mut rng).to_le_bytes();
            base64(&[a, b].concat())
        };
        let request = format!(
            "GET / HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        stream
            .write_all(request.as_bytes())
            .map_err(|e| format!("could not send the connection request to {addr}: {e}"))?;
        let head = read_head(&mut stream, &addr)?;
        check_handshake(&head, &key, &addr)?;

        let mut radio = TciRadio {
            addr,
            stream,
            rng,
            rbuf: Vec::new(),
            partial: None,
            device: String::new(),
            range: DEFAULT_RANGE,
            ready: false,
            saw_protocol: false,
            server_stopped: false,
            rate: RATES[0],
            freq: None,
            started: false,
            iq_seen: false,
            start_sent: false,
            pending: VecDeque::new(),
            dropped: 0,
        };
        radio.read_init()?;
        Ok(radio)
    }

    /// Reads the server's opening commands up to "ready".
    fn read_init(&mut self) -> Result<(), String> {
        let deadline = Instant::now() + CONNECT_TIMEOUT;
        while !self.ready {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            match self.next_message(deadline - now)? {
                Some(Message::Text(text)) => self.absorb(&text),
                Some(Message::Binary(_)) => {}
                None => break,
            }
        }
        if !self.ready && !self.saw_protocol {
            return Err(format!(
                "{} answered, but not as a TCI server (no protocol or ready command). \
                 Check that the address is the TCI port, default 50001",
                self.addr
            ));
        }
        Ok(())
    }

    fn lost(&self, why: &str) -> String {
        format!("lost the connection to the TCI server at {}: {why}", self.addr)
    }

    fn absorb(&mut self, text: &str) {
        for (name, args) in parse_commands(text) {
            match name.as_str() {
                "ready" => self.ready = true,
                "protocol" => self.saw_protocol = true,
                "device" => {
                    if let Some(v) = args.first() {
                        self.device = v.clone();
                    }
                }
                "vfo_limits" => {
                    if args.len() >= 2 {
                        if let (Ok(lo), Ok(hi)) = (args[0].parse::<f64>(), args[1].parse::<f64>()) {
                            if lo >= 0.0 && hi > lo && hi < 1e12 {
                                self.range = (lo.round() as u64, hi.round() as u64);
                            }
                        }
                    }
                }
                "stop" => self.server_stopped = true,
                "start" => self.server_stopped = false,
                _ => {}
            }
        }
    }

    /// Returns the next whole message, or None when `wait` passes with nothing complete.
    fn next_message(&mut self, wait: Duration) -> Result<Option<Message>, String> {
        let deadline = Instant::now() + wait;
        loop {
            if let Some((used, frame)) = parse_frame(&self.rbuf).map_err(|e| self.lost(&e))? {
                self.rbuf.drain(..used);
                if let Some(message) = self.on_frame(frame)? {
                    return Ok(Some(message));
                }
                continue;
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            let left = (deadline - now).max(Duration::from_millis(1));
            self.stream
                .set_read_timeout(Some(left))
                .map_err(|e| self.lost(&e.to_string()))?;
            let mut tmp = vec![0u8; 65536];
            match self.stream.read(&mut tmp) {
                Ok(0) => return Err(self.lost("the server closed the connection")),
                Ok(n) => self.rbuf.extend_from_slice(&tmp[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(e) => return Err(self.lost(&e.to_string())),
            }
        }
    }

    fn on_frame(&mut self, frame: Frame) -> Result<Option<Message>, String> {
        match frame.opcode {
            0x8 => {
                let _ = self.send_frame(0x8, &[]);
                Err(self.lost("the server sent a close request"))
            }
            0x9 => {
                self.send_frame(0xA, &frame.payload)?;
                Ok(None)
            }
            0xA => Ok(None),
            0x0..=0x2 => {
                if frame.opcode != 0 {
                    if self.partial.is_some() {
                        return Err(self.lost("a new message started before the last one ended"));
                    }
                    self.partial = Some((frame.opcode, Vec::new()));
                }
                let Some((_, buf)) = self.partial.as_mut() else {
                    return Err(self.lost("a continuation frame arrived with no message to continue"));
                };
                buf.extend_from_slice(&frame.payload);
                if buf.len() as u64 > MAX_FRAME {
                    return Err(self.lost("a message was larger than the limit"));
                }
                if !frame.fin {
                    return Ok(None);
                }
                let Some((opcode, data)) = self.partial.take() else {
                    return Ok(None);
                };
                Ok(Some(if opcode == 0x1 {
                    Message::Text(String::from_utf8_lossy(&data).into_owned())
                } else {
                    Message::Binary(data)
                }))
            }
            other => Err(self.lost(&format!("the server sent an unknown frame type {other}"))),
        }
    }

    fn next_mask(&mut self) -> [u8; 4] {
        (next_u64(&mut self.rng) as u32).to_le_bytes()
    }

    fn send_frame(&mut self, opcode: u8, payload: &[u8]) -> Result<(), String> {
        let mask = self.next_mask();
        let bytes = encode_client_frame(opcode, payload, mask);
        self.stream
            .write_all(&bytes)
            .map_err(|e| format!("could not send to the TCI server at {}: {e}", self.addr))
    }

    fn send_text(&mut self, text: &str) -> Result<(), String> {
        self.send_frame(0x1, text.as_bytes())
    }

    fn maybe_send_start(&mut self) -> Result<(), String> {
        if self.started && self.server_stopped && !self.iq_seen && !self.start_sent {
            self.start_sent = true;
            self.send_text("start;")?;
        }
        Ok(())
    }

    fn push_pending(&mut self, samples: Vec<(f32, f32)>) {
        self.pending.extend(samples);
        if self.pending.len() > MAX_PENDING {
            let excess = self.pending.len() - MAX_PENDING;
            self.pending.drain(..excess);
            self.dropped += excess as u64;
        }
    }

    fn drain_pending(&mut self, out: &mut [(f32, f32)]) -> usize {
        let n = out.len().min(self.pending.len());
        for slot in out.iter_mut().take(n) {
            if let Some(s) = self.pending.pop_front() {
                *slot = s;
            }
        }
        n
    }
}

impl RxRadio for TciRadio {
    fn label(&self) -> String {
        if self.device.is_empty() {
            format!("TCI {}", self.addr)
        } else {
            format!("{} (TCI {})", self.device, self.addr)
        }
    }

    fn freq_range_hz(&self) -> (u64, u64) {
        self.range
    }

    fn set_frequency_hz(&mut self, hz: u64) -> Result<(), String> {
        check_frequency(hz, self.range)?;
        self.freq = Some(hz);
        if self.started {
            self.send_text(&cmd_dds(RECEIVER, hz))?;
            self.send_text(&cmd_vfo(RECEIVER, hz))?;
        }
        Ok(())
    }

    fn sample_rates(&self) -> Vec<u32> {
        RATES.to_vec()
    }

    fn set_sample_rate(&mut self, hz: u32) -> Result<u32, String> {
        let rate = closest_rate(hz);
        self.rate = rate;
        if self.started {
            self.send_text(&cmd_samplerate(rate))?;
        }
        Ok(rate)
    }

    fn gain_stages(&self) -> Vec<GainStage> {
        Vec::new()
    }

    fn set_gain(&mut self, stage: &str, _setting: GainSetting) -> Result<(), String> {
        Err(format!(
            "TCI has no gain stage named {stage:?}. The server sets its own gain"
        ))
    }

    fn start(&mut self) -> Result<(), String> {
        if self.started {
            return Ok(());
        }
        let freq = self
            .freq
            .ok_or_else(|| "no frequency set. Set a frequency before starting the TCI radio".to_string())?;
        self.send_text(&cmd_samplerate(self.rate))?;
        self.send_text(&cmd_dds(RECEIVER, freq))?;
        self.send_text(&cmd_vfo(RECEIVER, freq))?;
        self.send_text(&cmd_iq_start(RECEIVER))?;
        self.started = true;
        self.iq_seen = false;
        self.start_sent = false;
        Ok(())
    }

    fn read_iq(&mut self, out: &mut [(f32, f32)]) -> Result<usize, String> {
        if !self.started {
            return Err("the TCI radio is stopped. Call start before reading samples".into());
        }
        let n = self.drain_pending(out);
        if n > 0 {
            return Ok(n);
        }
        let deadline = Instant::now() + WAIT_MAX;
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Ok(0);
            }
            let message = match self.next_message(deadline - now)? {
                Some(m) => m,
                None => return Ok(0),
            };
            match message {
                Message::Text(text) => {
                    self.absorb(&text);
                    self.maybe_send_start()?;
                }
                Message::Binary(block) => {
                    // A block that is not IQ for this receiver, or is malformed, is skipped.
                    if let Ok(Some(samples)) = decode_iq_block(&block, RECEIVER) {
                        self.iq_seen = true;
                        self.push_pending(samples);
                        let n = self.drain_pending(out);
                        if n > 0 {
                            return Ok(n);
                        }
                    }
                }
            }
        }
    }

    fn stop(&mut self) {
        if self.started {
            let _ = self.send_text(&cmd_iq_stop(RECEIVER));
        }
        self.started = false;
        self.pending.clear();
    }

    fn dropped_samples(&self) -> u64 {
        self.dropped
    }
}

// ---------- address and tuning helpers ----------

/// Splits "host:port", "[v6]:port" or "host" (default port 50001).
fn parse_address(text: &str) -> Result<(String, u16), String> {
    let t = text.trim();
    if t.is_empty() {
        return Err(NEED_ADDRESS.into());
    }
    let (host, port): (String, Option<String>) = if let Some(rest) = t.strip_prefix('[') {
        let end = rest.find(']').ok_or_else(|| {
            format!("bad address {t:?}: the square bracket is not closed. Write it as [address]:port")
        })?;
        let after = &rest[end + 1..];
        let port = if after.is_empty() {
            None
        } else {
            let p = after
                .strip_prefix(':')
                .ok_or_else(|| format!("bad address {t:?}: expected a colon and a port after the bracket"))?;
            Some(p.to_string())
        };
        (rest[..end].to_string(), port)
    } else {
        match t.rsplit_once(':') {
            Some((h, p)) if !h.contains(':') => (h.to_string(), Some(p.to_string())),
            _ => (t.to_string(), None),
        }
    };
    if host.trim().is_empty() {
        return Err(format!(
            "bad address {t:?}: the host part is empty. {NEED_ADDRESS}"
        ));
    }
    let port = match port {
        None => DEFAULT_PORT,
        Some(p) => p
            .trim()
            .parse::<u16>()
            .ok()
            .filter(|p| *p > 0)
            .ok_or_else(|| format!("bad port in {t:?}: use a number from 1 to 65535"))?,
    };
    Ok((host.trim().to_string(), port))
}

fn format_addr(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn check_frequency(hz: u64, range: (u64, u64)) -> Result<(), String> {
    if hz < range.0 || hz > range.1 {
        return Err(format!(
            "{hz} Hz is outside the range this TCI server allows ({} to {} Hz)",
            range.0, range.1
        ));
    }
    Ok(())
}

/// The offered rate closest to `hz`. A tie goes to the lower rate.
fn closest_rate(hz: u32) -> u32 {
    let mut best = RATES[0];
    for r in RATES {
        if (r as i64 - hz as i64).abs() < (best as i64 - hz as i64).abs() {
            best = r;
        }
    }
    best
}

// ---------- text commands ----------

fn cmd_samplerate(rate: u32) -> String {
    format!("iq_samplerate:{rate};")
}

fn cmd_dds(rx: u32, hz: u64) -> String {
    format!("dds:{rx},{hz};")
}

fn cmd_vfo(rx: u32, hz: u64) -> String {
    format!("vfo:{rx},0,{hz};")
}

fn cmd_iq_start(rx: u32) -> String {
    format!("iq_start:{rx};")
}

fn cmd_iq_stop(rx: u32) -> String {
    format!("iq_stop:{rx};")
}

/// Splits a text message into (name, arguments). Names are lower case.
fn parse_commands(text: &str) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    for part in text.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (name, rest) = match part.split_once(':') {
            Some((n, r)) => (n, Some(r)),
            None => (part, None),
        };
        let name = name.trim().to_ascii_lowercase();
        if name.is_empty() || name.len() > 64 {
            continue;
        }
        let args = match rest {
            Some(r) => r.split(',').map(|a| a.trim().to_string()).collect(),
            None => Vec::new(),
        };
        out.push((name, args));
    }
    out
}

// ---------- binary blocks ----------

/// Decodes one binary block. Ok(None) for a block that is not IQ of `rx`.
/// Err for a malformed block.
fn decode_iq_block(msg: &[u8], rx: u32) -> Result<Option<Vec<(f32, f32)>>, String> {
    if msg.len() < 64 {
        return Err("block is shorter than its 64-byte header".into());
    }
    let field = |i: usize| u32::from_le_bytes([msg[4 * i], msg[4 * i + 1], msg[4 * i + 2], msg[4 * i + 3]]);
    let recv = field(0);
    let format = field(2);
    let kind = field(6);
    let channels = field(7);
    if kind != 0 || recv != rx {
        return Ok(None);
    }
    if channels != 2 && channels != 0 {
        return Err(format!("block has {channels} channels, IQ needs 2"));
    }
    let width = match format {
        0 => 2,
        1 => 3,
        2 | 3 => 4,
        other => return Err(format!("unknown sample format {other}")),
    };
    let payload = &msg[64..];
    let pair = width * 2;
    if payload.is_empty() || !payload.len().is_multiple_of(pair) {
        return Err("sample data does not match the sample format".into());
    }
    let samples = payload
        .chunks_exact(pair)
        .map(|p| (sample(&p[..width], format), sample(&p[width..], format)))
        .collect();
    Ok(Some(samples))
}

/// One sample as f32 in -1..1.
fn sample(b: &[u8], format: u32) -> f32 {
    match format {
        0 => i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0,
        1 => {
            // 24-bit signed: shift up into an i32, then back down to keep the sign.
            let v = (b[0] as i32) | ((b[1] as i32) << 8) | ((b[2] as i32) << 16);
            ((v << 8) >> 8) as f32 / 8_388_608.0
        }
        2 => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
        _ => {
            let f = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            if f.is_finite() {
                f
            } else {
                0.0
            }
        }
    }
}

// ---------- WebSocket framing ----------

/// Parses one frame from the start of `buf`. Ok(None) when the frame is not complete yet.
/// Server frames are normally unmasked; a mask is removed if present.
fn parse_frame(buf: &[u8]) -> Result<Option<(usize, Frame)>, String> {
    if buf.len() < 2 {
        return Ok(None);
    }
    if buf[0] & 0x70 != 0 {
        return Err("the server set reserved bits in a frame".into());
    }
    let fin = buf[0] & 0x80 != 0;
    let opcode = buf[0] & 0x0F;
    let masked = buf[1] & 0x80 != 0;
    let mut len = (buf[1] & 0x7F) as u64;
    let mut pos = 2usize;
    if len == 126 {
        if buf.len() < 4 {
            return Ok(None);
        }
        len = u16::from_be_bytes([buf[2], buf[3]]) as u64;
        pos = 4;
    } else if len == 127 {
        if buf.len() < 10 {
            return Ok(None);
        }
        let mut b = [0u8; 8];
        b.copy_from_slice(&buf[2..10]);
        len = u64::from_be_bytes(b);
        pos = 10;
    }
    if len > MAX_FRAME {
        return Err(format!("the server sent a frame of {len} bytes, over the limit"));
    }
    let mask = if masked {
        if buf.len() < pos + 4 {
            return Ok(None);
        }
        let m = [buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]];
        pos += 4;
        Some(m)
    } else {
        None
    };
    let len = len as usize;
    if buf.len() < pos + len {
        return Ok(None);
    }
    let mut payload = buf[pos..pos + len].to_vec();
    if let Some(m) = mask {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= m[i % 4];
        }
    }
    Ok(Some((pos + len, Frame { fin, opcode, payload })))
}

/// Builds a frame as a client must send it: masked, one final fragment.
fn encode_client_frame(opcode: u8, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
    let mut f = Vec::with_capacity(payload.len() + 14);
    f.push(0x80 | opcode);
    let len = payload.len();
    if len < 126 {
        f.push(0x80 | len as u8);
    } else if len <= 0xFFFF {
        f.push(0x80 | 126);
        f.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        f.push(0x80 | 127);
        f.extend_from_slice(&(len as u64).to_be_bytes());
    }
    f.extend_from_slice(&mask);
    f.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    f
}

/// Reads the HTTP reply to the connection request, up to the blank line.
fn read_head(stream: &mut TcpStream, addr: &str) -> Result<Vec<u8>, String> {
    let mut head = Vec::new();
    let mut one = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > 8192 {
            return Err(format!(
                "{addr} sent a reply to the connection request that is too long"
            ));
        }
        match stream.read(&mut one) {
            Ok(0) => {
                return Err(format!(
                    "{addr} closed the connection during the WebSocket handshake"
                ))
            }
            Ok(_) => head.push(one[0]),
            Err(e) => {
                return Err(format!(
                    "no valid reply from {addr} during the WebSocket handshake: {e}"
                ))
            }
        }
    }
    Ok(head)
}

fn check_handshake(head: &[u8], key: &str, addr: &str) -> Result<(), String> {
    let text = String::from_utf8_lossy(head).into_owned();
    let mut lines = text.split("\r\n");
    let status = lines.next().unwrap_or("");
    if status.split_whitespace().nth(1) != Some("101") {
        return Err(format!(
            "{addr} did not accept the WebSocket connection (reply starts {status:?}). \
             Check that the address is the TCI port, default 50001"
        ));
    }
    let want = accept_key(key);
    let got = lines.find_map(|l| {
        let (name, value) = l.split_once(':')?;
        if name.trim().eq_ignore_ascii_case("sec-websocket-accept") {
            Some(value.trim().to_string())
        } else {
            None
        }
    });
    if got.as_deref() != Some(want.as_str()) {
        return Err(format!(
            "{addr} sent a wrong WebSocket accept key, so it is not a valid WebSocket server"
        ));
    }
    Ok(())
}

fn accept_key(key: &str) -> String {
    let mut s = key.as_bytes().to_vec();
    s.extend_from_slice(WS_GUID.as_bytes());
    base64(&sha1(&s))
}

fn random_seed() -> u64 {
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    h.write_u128(nanos);
    h.finish() | 1
}

/// xorshift64. Only used for masks and the handshake key, which need no secrecy.
fn next_u64(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

fn base64(data: &[u8]) -> String {
    const ALPHA: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        s.push(ALPHA[((n >> 18) & 63) as usize] as char);
        s.push(ALPHA[((n >> 12) & 63) as usize] as char);
        s.push(if chunk.len() > 1 {
            ALPHA[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        s.push(if chunk.len() > 2 {
            ALPHA[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    s
}

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
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[4 * i], chunk[4 * i + 1], chunk[4 * i + 2], chunk[4 * i + 3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn server_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
        assert!(payload.len() < 126, "test helper handles short frames only");
        let mut f = vec![0x80 | opcode, payload.len() as u8];
        f.extend_from_slice(payload);
        f
    }

    fn block_bytes(recv: u32, format: u32, kind: u32, channels: u32, payload: &[u8]) -> Vec<u8> {
        let mut header = [0u32; 16];
        header[0] = recv;
        header[1] = 48_000;
        header[2] = format;
        header[5] = 1;
        header[6] = kind;
        header[7] = channels;
        let mut out = Vec::new();
        for w in header {
            out.extend_from_slice(&w.to_le_bytes());
        }
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn sha1_matches_known_value() {
        let digest = sha1(b"abc");
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn accept_key_matches_rfc_example() {
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn base64_pads_correctly() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
    }

    #[test]
    fn handshake_check_accepts_good_reply_and_rejects_bad_ones() {
        let key = "dGhlIHNhbXBsZSBub25jZQ==";
        let good = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {}\r\n\r\n",
            accept_key(key)
        );
        assert!(check_handshake(good.as_bytes(), key, "h:1").is_ok());
        let wrong_key = good.replace("s3pP", "xxxx");
        let err = check_handshake(wrong_key.as_bytes(), key, "h:1").unwrap_err();
        assert!(err.contains("wrong WebSocket accept key"), "{err}");
        let http = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
        let err = check_handshake(http, key, "h:1").unwrap_err();
        assert!(err.contains("did not accept") && err.contains("50001"), "{err}");
    }

    #[test]
    fn parses_unmasked_short_text_frame() {
        let (used, f) = parse_frame(&[0x81, 0x02, b'H', b'i']).unwrap().unwrap();
        assert_eq!(used, 4);
        assert!(f.fin);
        assert_eq!(f.opcode, 1);
        assert_eq!(f.payload, b"Hi");
    }

    #[test]
    fn parses_16_bit_length_frame() {
        let payload = vec![7u8; 200];
        let mut buf = vec![0x82, 126, 0x00, 0xC8];
        buf.extend_from_slice(&payload);
        let (used, f) = parse_frame(&buf).unwrap().unwrap();
        assert_eq!(used, 204);
        assert_eq!(f.opcode, 2);
        assert_eq!(f.payload, payload);
    }

    #[test]
    fn incomplete_frame_waits_for_more_bytes() {
        assert!(parse_frame(&[0x81, 0x05, b'a']).unwrap().is_none());
        assert!(parse_frame(&[0x81]).unwrap().is_none());
        assert!(parse_frame(&[0x82, 126, 0x01]).unwrap().is_none());
    }

    #[test]
    fn reserved_bits_and_huge_lengths_are_errors() {
        assert!(parse_frame(&[0xC1, 0x00]).is_err());
        let huge = [0x81, 127, 0, 0, 0, 0, 0x02, 0, 0, 0];
        let err = parse_frame(&huge).err().unwrap();
        assert!(err.contains("over the limit"), "{err}");
    }

    #[test]
    fn client_frame_is_masked_and_round_trips() {
        let frame = encode_client_frame(0x1, b"abc", [1, 2, 3, 4]);
        assert_eq!(frame[0], 0x81);
        assert_eq!(frame[1], 0x80 | 3);
        assert_ne!(&frame[6..9], b"abc");
        let (used, f) = parse_frame(&frame).unwrap().unwrap();
        assert_eq!(used, frame.len());
        assert_eq!(f.payload, b"abc");
    }

    #[test]
    fn command_strings_have_the_wire_format() {
        assert_eq!(cmd_samplerate(192_000), "iq_samplerate:192000;");
        assert_eq!(cmd_dds(0, 14_074_000), "dds:0,14074000;");
        assert_eq!(cmd_vfo(0, 14_074_000), "vfo:0,0,14074000;");
        assert_eq!(cmd_iq_start(0), "iq_start:0;");
        assert_eq!(cmd_iq_stop(1), "iq_stop:1;");
    }

    #[test]
    fn text_message_splits_into_commands() {
        let cmds = parse_commands("VFO_LIMITS:0.0,30000000.0; ready;device:Test Box;;");
        assert_eq!(cmds.len(), 3);
        assert_eq!(cmds[0].0, "vfo_limits");
        assert_eq!(cmds[0].1, vec!["0.0", "30000000.0"]);
        assert_eq!(cmds[1], ("ready".to_string(), Vec::new()));
        assert_eq!(cmds[2].1, vec!["Test Box"]);
        assert!(parse_commands("  ;  ").is_empty());
    }

    #[test]
    fn decodes_int16_block() {
        let payload = [0x00, 0x40, 0x00, 0xC0];
        let samples = decode_iq_block(&block_bytes(0, 0, 0, 2, &payload), 0)
            .unwrap()
            .unwrap();
        assert_eq!(samples.len(), 1);
        assert!((samples[0].0 - 0.5).abs() < 1e-6);
        assert!((samples[0].1 + 0.5).abs() < 1e-6);
    }

    #[test]
    fn decodes_int24_with_sign() {
        // -4194304 is -0.5 of full scale in 24 bits: 0xC00000 little endian.
        let payload = [0x00, 0x00, 0x40, 0x00, 0x00, 0xC0];
        let samples = decode_iq_block(&block_bytes(0, 1, 0, 2, &payload), 0)
            .unwrap()
            .unwrap();
        assert!((samples[0].0 - 0.5).abs() < 1e-6);
        assert!((samples[0].1 + 0.5).abs() < 1e-6);
    }

    #[test]
    fn decodes_int32_and_float32() {
        let mut p = Vec::new();
        p.extend_from_slice(&(1_073_741_824i32).to_le_bytes());
        p.extend_from_slice(&(-1_073_741_824i32).to_le_bytes());
        let s = decode_iq_block(&block_bytes(0, 2, 0, 2, &p), 0).unwrap().unwrap();
        assert!((s[0].0 - 0.5).abs() < 1e-6 && (s[0].1 + 0.5).abs() < 1e-6);

        let mut f = Vec::new();
        f.extend_from_slice(&0.25f32.to_le_bytes());
        f.extend_from_slice(&f32::NAN.to_le_bytes());
        let s = decode_iq_block(&block_bytes(0, 3, 0, 2, &f), 0).unwrap().unwrap();
        assert_eq!(s[0].0, 0.25);
        assert_eq!(s[0].1, 0.0);
    }

    #[test]
    fn skips_other_receivers_and_other_types() {
        let p = [0u8; 4];
        assert!(decode_iq_block(&block_bytes(1, 0, 0, 2, &p), 0)
            .unwrap()
            .is_none());
        assert!(decode_iq_block(&block_bytes(0, 0, 3, 2, &p), 0)
            .unwrap()
            .is_none());
    }

    #[test]
    fn malformed_blocks_are_errors() {
        assert!(decode_iq_block(&[0u8; 10], 0).is_err());
        let err = decode_iq_block(&block_bytes(0, 0, 0, 3, &[0; 4]), 0).unwrap_err();
        assert!(err.contains("channels"), "{err}");
        assert!(decode_iq_block(&block_bytes(0, 0, 0, 2, &[0; 3]), 0).is_err());
        assert!(decode_iq_block(&block_bytes(0, 0, 0, 2, &[]), 0).is_err());
        assert!(decode_iq_block(&block_bytes(0, 9, 0, 2, &[0; 4]), 0).is_err());
    }

    #[test]
    fn address_parsing_and_errors() {
        assert_eq!(
            parse_address("192.168.1.5:1234").unwrap(),
            ("192.168.1.5".to_string(), 1234)
        );
        assert_eq!(parse_address("[::1]:50001").unwrap(), ("::1".to_string(), 50001));
        assert_eq!(
            parse_address("localhost").unwrap(),
            ("localhost".to_string(), DEFAULT_PORT)
        );
        assert!(parse_address("  ").unwrap_err().contains("host:port"));
        assert!(parse_address("host:0").unwrap_err().contains("port"));
        assert!(parse_address("host:99999").unwrap_err().contains("port"));
        assert!(parse_address("[::1:50001").unwrap_err().contains("bracket"));
        assert!(open(None).err().unwrap().contains("host:port"));
    }

    #[test]
    fn rate_choice_and_frequency_range() {
        assert_eq!(closest_rate(0), 48_000);
        assert_eq!(closest_rate(90_000), 96_000);
        assert_eq!(closest_rate(300_000), 384_000);
        assert_eq!(closest_rate(u32::MAX), 384_000);
        assert!(check_frequency(14_074_000, DEFAULT_RANGE).is_ok());
        let err = check_frequency(1, DEFAULT_RANGE).unwrap_err();
        assert!(err.contains("outside the range"), "{err}");
    }

    #[test]
    fn refused_connection_says_what_to_do() {
        let err = open(Some("127.0.0.1:1")).err().unwrap();
        assert!(
            err.contains("cannot connect to the TCI server at 127.0.0.1:1"),
            "{err}"
        );
        assert!(err.contains("TCI is switched on"), "{err}");
    }

    /// Fake server: answers the handshake, sends `ready` and the given blocks, then
    /// collects the text commands the driver sends until the driver closes.
    fn fake_server(listener: TcpListener, blocks: Vec<Vec<u8>>) -> std::thread::JoinHandle<Vec<String>> {
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut head = Vec::new();
            let mut one = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                s.read_exact(&mut one).unwrap();
                head.push(one[0]);
            }
            let text = String::from_utf8(head).unwrap();
            let key = text
                .lines()
                .find_map(|l| l.strip_prefix("Sec-WebSocket-Key: "))
                .unwrap()
                .trim()
                .to_string();
            let reply = format!(
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
                 Sec-WebSocket-Accept: {}\r\n\r\n",
                accept_key(&key)
            );
            s.write_all(reply.as_bytes()).unwrap();
            s.write_all(&server_frame(
                0x1,
                b"device:Test Box;protocol:TCI,2.0;vfo_limits:0,30000000;ready;",
            ))
            .unwrap();
            for b in blocks {
                s.write_all(&server_frame(0x2, &b)).unwrap();
            }
            let mut got = Vec::new();
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            loop {
                match s.read(&mut tmp) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&tmp[..n]);
                        while let Ok(Some((used, f))) = parse_frame(&buf) {
                            buf.drain(..used);
                            if f.opcode == 1 {
                                got.push(String::from_utf8_lossy(&f.payload).into_owned());
                            }
                        }
                    }
                }
            }
            got
        })
    }

    #[test]
    fn loopback_open_tune_stream_and_stop() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut payload = vec![0x00, 0x40, 0x00, 0xC0];
        payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        let block = block_bytes(0, 0, 0, 2, &payload);
        let server = fake_server(listener, vec![block]);
        {
            let mut r = open(Some(&format!("127.0.0.1:{port}"))).unwrap();
            assert!(r.label().contains("Test Box"), "{}", r.label());
            assert_eq!(r.freq_range_hz(), (0, 30_000_000));
            assert_eq!(r.set_sample_rate(90_000).unwrap(), 96_000);
            r.set_frequency_hz(14_074_000).unwrap();
            assert!(r.set_frequency_hz(40_000_000).is_err());
            r.start().unwrap();
            let mut buf = [(0.0f32, 0.0f32); 8];
            let mut got = 0;
            for _ in 0..20 {
                got = r.read_iq(&mut buf).unwrap();
                if got > 0 {
                    break;
                }
            }
            assert_eq!(got, 2);
            assert!((buf[0].0 - 0.5).abs() < 1e-6);
            assert!((buf[0].1 + 0.5).abs() < 1e-6);
            r.stop();
        }
        let cmds = server.join().unwrap();
        assert_eq!(
            cmds,
            vec![
                "iq_samplerate:96000;",
                "dds:0,14074000;",
                "vfo:0,0,14074000;",
                "iq_start:0;",
                "iq_stop:0;"
            ]
        );
    }

    #[test]
    fn read_iq_returns_zero_within_wait_when_idle() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = fake_server(listener, Vec::new());
        {
            let mut r = open(Some(&format!("127.0.0.1:{port}"))).unwrap();
            r.set_frequency_hz(7_100_000).unwrap();
            r.start().unwrap();
            let mut buf = [(0.0f32, 0.0f32); 8];
            let t = Instant::now();
            let n = r.read_iq(&mut buf).unwrap();
            let took = t.elapsed();
            assert_eq!(n, 0);
            assert!(took < Duration::from_millis(1000), "read_iq waited {took:?}");
            r.stop();
        }
        server.join().unwrap();
    }

    #[test]
    fn start_without_frequency_is_an_error() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = fake_server(listener, Vec::new());
        {
            let mut r = open(Some(&format!("127.0.0.1:{port}"))).unwrap();
            let err = r.start().unwrap_err();
            assert!(err.contains("no frequency set"), "{err}");
        }
        server.join().unwrap();
    }
}
