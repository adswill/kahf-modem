//! Driver for SpyServer network radios: a server that runs a radio and streams
//! its samples over TCP. The id is host:port, for example 192.168.1.20:5555.
//!
//! Wire format (little endian). A command the client sends is a 32-bit type,
//! a 32-bit body size and the body. A message the server sends has a 20-byte
//! header (protocol id, a word holding the message type in its low 16 bits and
//! a digital gain in dB in its high 16 bits, stream type, sequence number, body
//! size), then the body.
//!
//! The sample rates are the radio's full rate divided by 2 to the power of a
//! decimation step. A server that another client controls is read only here:
//! its gain cannot be set by this driver.

use std::collections::VecDeque;
use std::io::{self, ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use crate::{DeviceInfo, GainSetting, GainStage, RxRadio};

/// Protocol version sent in HELLO: 2.0.1700.
const PROTOCOL_VERSION: u32 = (2 << 24) | 1700;
/// Client name sent in HELLO.
const CLIENT_NAME: &[u8] = b"KAHF";

const CMD_HELLO: u32 = 0;
const CMD_SET_SETTING: u32 = 2;

const SET_STREAMING_MODE: u32 = 0;
const SET_STREAMING_ENABLED: u32 = 1;
const SET_GAIN: u32 = 2;
const SET_IQ_FORMAT: u32 = 100;
const SET_IQ_FREQUENCY: u32 = 101;
const SET_IQ_DECIMATION: u32 = 102;

/// Streaming mode that sends only IQ samples.
const MODE_IQ_ONLY: u32 = 1;

const FORMAT_UINT8: u32 = 1;
const FORMAT_INT16: u32 = 2;
const FORMAT_INT24: u32 = 3;
const FORMAT_FLOAT: u32 = 4;

const MSG_DEVICE_INFO: u32 = 0;
const MSG_CLIENT_SYNC: u32 = 1;
const MSG_IQ_UINT8: u32 = 100;
const MSG_IQ_INT16: u32 = 101;
const MSG_IQ_FLOAT: u32 = 103;

const DEV_INVALID: u32 = 0;
const HEADER_LEN: usize = 20;
/// Largest message body accepted; a bigger one means the stream is out of step.
const MAX_BODY: u32 = 1 << 20;
/// Messages read during the handshake before giving up.
const MAX_HANDSHAKE_MESSAGES: usize = 64;
/// Longest a connect or a handshake step may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// No samples for this long means the connection is lost.
const STALL_TIMEOUT: Duration = Duration::from_secs(3);
/// Longest one read_iq call waits for the network.
const READ_WAIT: Duration = Duration::from_millis(200);
const READ_CHUNK: usize = 64 * 1024;
/// Name of the one gain stage this server offers.
const GAIN_STAGE: &str = "gain";
const EXAMPLE: &str = "192.168.1.20:5555";

/// The server cannot be searched for; the operator names it.
pub fn probe() -> Vec<DeviceInfo> {
    Vec::new()
}

/// Connects to the server named by `id` (host:port) and reads its radio description.
pub fn open(id: Option<&str>) -> Result<Box<dyn RxRadio>, String> {
    let id = id.map(str::trim).unwrap_or("");
    let needs_name = || format!("name the SpyServer as host:port, for example {EXAMPLE}");
    if id.is_empty() {
        return Err(needs_name());
    }
    match id.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() && port.parse::<u16>().is_ok() => {}
        _ => return Err(needs_name()),
    }
    let radio = SpyServer::connect_and_sync(id)?;
    Ok(Box::new(radio))
}

/// What the server said about its radio: its DEVICE_INFO and CLIENT_SYNC words.
#[derive(Debug, Clone, Default)]
struct ServerInfo {
    dev_type: u32,
    max_rate: u32,
    dec_stages: u32,
    min_dec: u32,
    gain_max: u32,
    min_freq: u32,
    max_freq: u32,
    forced_format: u32,
    can_control: bool,
    gain: u32,
    device_centre: u32,
    min_iq_centre: u32,
    max_iq_centre: u32,
}

impl ServerInfo {
    /// Takes the words of a DEVICE_INFO body. Words past the end read as 0.
    fn apply_device_info(&mut self, b: &[u8]) {
        self.dev_type = word(b, 0);
        self.max_rate = word(b, 2);
        self.dec_stages = word(b, 4);
        self.gain_max = word(b, 6);
        self.min_freq = word(b, 7);
        self.max_freq = word(b, 8);
        self.min_dec = word(b, 10);
        self.forced_format = word(b, 11);
    }

    /// Takes the words of a CLIENT_SYNC body. Words past the end read as 0.
    fn apply_sync(&mut self, b: &[u8]) {
        self.can_control = word(b, 0) != 0;
        self.gain = word(b, 1);
        self.device_centre = word(b, 2);
        self.min_iq_centre = word(b, 5);
        self.max_iq_centre = word(b, 6);
    }

    /// Tuning range in Hz. The radio's range, widened to the centre range the server accepts.
    fn freq_range(&self) -> (u32, u32) {
        let mut lo = self.min_freq;
        let mut hi = self.max_freq;
        if self.max_iq_centre > hi {
            hi = self.max_iq_centre;
        }
        if self.min_iq_centre > 0 && (lo == 0 || self.min_iq_centre < lo) {
            lo = self.min_iq_centre;
        }
        (lo, hi)
    }

    /// Offered rates, fastest first. Index k is decimation step `min_dec + k`.
    fn rates(&self) -> Vec<u32> {
        offered_rates(self.max_rate, self.dec_stages, self.min_dec)
    }
}

/// An open SpyServer connection.
struct SpyServer {
    addr: String,
    stream: TcpStream,
    info: ServerInfo,
    frequency_hz: u32,
    decimation: u32,
    gain: u32,
    started: bool,
    last_data: Instant,
    /// Bytes read from the socket that do not yet make a whole message.
    pending: Vec<u8>,
    /// Decoded samples not yet handed to the caller.
    samples: VecDeque<(f32, f32)>,
    scratch: Vec<u8>,
}

impl SpyServer {
    /// Connects, says hello, and reads the device info and client sync the server sends back.
    fn connect_and_sync(id: &str) -> Result<SpyServer, String> {
        let mut stream = connect(id)?;
        let _ = stream.set_nodelay(true);
        stream
            .set_read_timeout(Some(CONNECT_TIMEOUT))
            .and_then(|_| stream.set_write_timeout(Some(STALL_TIMEOUT)))
            .map_err(|e| format!("could not set the timeouts for {id}: {e}"))?;

        let mut body = PROTOCOL_VERSION.to_le_bytes().to_vec();
        body.extend_from_slice(CLIENT_NAME);
        stream
            .write_all(&command_bytes(CMD_HELLO, &body))
            .map_err(|_| format!("the connection to {id} dropped while saying hello"))?;

        let mut info = ServerInfo::default();
        let (mut have_info, mut have_sync) = (false, false);
        for _ in 0..MAX_HANDSHAKE_MESSAGES {
            if have_info && have_sync {
                break;
            }
            let (ty, _flags, body) = match read_message(&mut stream) {
                Ok(m) => m,
                Err(_) if have_info => return Err(format!("{id} sent no client sync")),
                Err(_) => {
                    return Err(format!(
                        "{id} is not a SpyServer, or it refused this client (it may be full)"
                    ))
                }
            };
            match ty {
                MSG_DEVICE_INFO => {
                    info.apply_device_info(&body);
                    have_info = true;
                }
                MSG_CLIENT_SYNC => {
                    info.apply_sync(&body);
                    have_sync = true;
                }
                _ => {}
            }
        }
        if !have_info {
            return Err(format!("{id} sent no device info"));
        }
        if !have_sync {
            return Err(format!("{id} sent no client sync"));
        }
        if info.dev_type == DEV_INVALID || info.max_rate == 0 {
            return Err(format!("{id} has no radio connected"));
        }

        let frequency_hz = if info.device_centre > 0 { info.device_centre } else { info.min_freq };
        let mut radio = SpyServer {
            addr: id.to_string(),
            stream,
            frequency_hz,
            decimation: info.min_dec,
            gain: info.gain.min(gain_top(&info)),
            info,
            started: false,
            last_data: Instant::now(),
            pending: Vec::new(),
            samples: VecDeque::new(),
            scratch: vec![0u8; READ_CHUNK],
        };
        radio.send_setting(SET_STREAMING_MODE, MODE_IQ_ONLY)?;
        Ok(radio)
    }

    fn send_command(&mut self, ty: u32, body: &[u8]) -> Result<(), String> {
        self.stream
            .write_all(&command_bytes(ty, body))
            .map_err(|e| format!("lost the connection to {}: {e}", self.addr))
    }

    fn send_setting(&mut self, id: u32, value: u32) -> Result<(), String> {
        let mut body = id.to_le_bytes().to_vec();
        body.extend_from_slice(&value.to_le_bytes());
        self.send_command(CMD_SET_SETTING, &body)
    }

    /// Reads one network chunk (waiting at most READ_WAIT), then decodes whole messages from it.
    fn fill(&mut self) -> Result<(), String> {
        self.stream
            .set_read_timeout(Some(READ_WAIT))
            .map_err(|e| format!("lost the connection to {}: {e}", self.addr))?;
        match self.stream.read(&mut self.scratch) {
            Ok(0) => return Err(format!("{} closed the connection", self.addr)),
            Ok(n) => {
                self.last_data = Instant::now();
                self.pending.extend_from_slice(&self.scratch[..n]);
            }
            Err(e) if is_wait(&e) => {}
            Err(e) => return Err(format!("lost the connection to {}: {e}", self.addr)),
        }
        self.decode_pending()?;
        if self.last_data.elapsed() > STALL_TIMEOUT {
            return Err(format!(
                "{} sent no samples for {} seconds; the connection is lost",
                self.addr,
                STALL_TIMEOUT.as_secs()
            ));
        }
        Ok(())
    }

    /// Decodes every whole message in the pending bytes and keeps the rest.
    fn decode_pending(&mut self) -> Result<(), String> {
        let data = std::mem::take(&mut self.pending);
        let mut pos = 0;
        let mut failure = None;
        while data.len() - pos >= HEADER_LEN {
            let Some((ty, flags, size)) = parse_header(&data[pos..pos + HEADER_LEN]) else {
                failure = Some(format!(
                    "{} sent a message header that makes no sense; the stream is out of step",
                    self.addr
                ));
                break;
            };
            let total = HEADER_LEN + size as usize;
            if data.len() - pos < total {
                break;
            }
            self.handle_message(ty, flags, &data[pos + HEADER_LEN..pos + total]);
            pos += total;
        }
        self.pending = data[pos..].to_vec();
        match failure {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    fn handle_message(&mut self, ty: u32, flags: u32, body: &[u8]) {
        // The server's digital gain (flags, in dB) is taken back out of the samples.
        let scale = if flags == 0 { 1.0 } else { 10f32.powf(-(flags as f32) / 20.0) };
        match ty {
            MSG_IQ_UINT8 => self.samples.extend(body.chunks_exact(2).map(|p| {
                (
                    (p[0] as f32 - 128.0) / 128.0 * scale,
                    (p[1] as f32 - 128.0) / 128.0 * scale,
                )
            })),
            MSG_IQ_INT16 => self.samples.extend(body.chunks_exact(4).map(|c| {
                let re = i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0 * scale;
                let im = i16::from_le_bytes([c[2], c[3]]) as f32 / 32768.0 * scale;
                (re, im)
            })),
            MSG_IQ_FLOAT => self.samples.extend(body.chunks_exact(8).map(|c| {
                let re = f32::from_le_bytes([c[0], c[1], c[2], c[3]]) * scale;
                let im = f32::from_le_bytes([c[4], c[5], c[6], c[7]]) * scale;
                (re, im)
            })),
            MSG_CLIENT_SYNC => self.info.apply_sync(body),
            _ => {}
        }
    }
}

impl Drop for SpyServer {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

impl RxRadio for SpyServer {
    fn label(&self) -> String {
        format!("SpyServer at {} ({})", self.addr, dev_name(self.info.dev_type))
    }

    fn freq_range_hz(&self) -> (u64, u64) {
        let (lo, hi) = self.info.freq_range();
        (lo as u64, hi as u64)
    }

    fn set_frequency_hz(&mut self, hz: u64) -> Result<(), String> {
        let (lo, hi) = self.freq_range_hz();
        if hi > lo && (hz < lo || hz > hi) {
            return Err(format!(
                "{:.3} MHz is outside the {:.3} to {:.3} MHz that {} gives",
                hz as f64 / 1e6,
                lo as f64 / 1e6,
                hi as f64 / 1e6,
                self.addr
            ));
        }
        let v = u32::try_from(hz).map_err(|_| format!("{hz} Hz is too high for a SpyServer"))?;
        self.send_setting(SET_IQ_FREQUENCY, v)?;
        self.frequency_hz = v;
        Ok(())
    }

    fn sample_rates(&self) -> Vec<u32> {
        let mut r = self.info.rates();
        r.reverse();
        r
    }

    fn set_sample_rate(&mut self, hz: u32) -> Result<u32, String> {
        let rates = self.info.rates();
        if rates.is_empty() {
            return Err(format!("{} offers no sample rate", self.addr));
        }
        let (k, rate) = rates
            .iter()
            .enumerate()
            .min_by_key(|&(_, &r)| (r as i64 - hz as i64).abs())
            .map(|(k, &r)| (k, r))
            .ok_or_else(|| format!("{} offers no sample rate", self.addr))?;
        let dec = self.info.min_dec.saturating_add(k as u32);
        self.send_setting(SET_IQ_DECIMATION, dec)?;
        self.decimation = dec;
        Ok(rate)
    }

    fn gain_stages(&self) -> Vec<GainStage> {
        vec![GainStage {
            name: GAIN_STAGE.to_string(),
            min_db: 0.0,
            max_db: gain_top(&self.info) as f32,
            step_db: 1.0,
            auto: false,
        }]
    }

    fn set_gain(&mut self, stage: &str, setting: GainSetting) -> Result<(), String> {
        if stage != GAIN_STAGE {
            return Err(format!(
                "this SpyServer has no gain stage named {stage:?}; its only stage is {GAIN_STAGE:?}"
            ));
        }
        let value = match setting {
            GainSetting::Auto => {
                return Err("this SpyServer has no automatic gain; give a fixed gain index".into())
            }
            GainSetting::Manual(v) => v,
        };
        if !self.info.can_control {
            return Err(format!(
                "{} is read only (another client controls it), so its gain cannot be set",
                self.addr
            ));
        }
        let v = (value.round().max(0.0) as u32).min(gain_top(&self.info));
        self.send_setting(SET_GAIN, v)?;
        self.gain = v;
        Ok(())
    }

    fn start(&mut self) -> Result<(), String> {
        if self.started {
            return Ok(());
        }
        if self.info.forced_format == FORMAT_INT24 {
            return Err(format!(
                "{} sends 24-bit samples, which this driver does not read",
                self.addr
            ));
        }
        let format = match self.info.forced_format {
            FORMAT_UINT8 | FORMAT_INT16 | FORMAT_FLOAT => self.info.forced_format,
            _ => FORMAT_UINT8,
        };
        self.send_setting(SET_IQ_FORMAT, format)?;
        self.send_setting(SET_IQ_DECIMATION, self.decimation)?;
        self.send_setting(SET_IQ_FREQUENCY, self.frequency_hz)?;
        if self.info.can_control {
            self.send_setting(SET_GAIN, self.gain)?;
        }
        self.send_setting(SET_STREAMING_ENABLED, 1)?;
        self.started = true;
        self.last_data = Instant::now();
        self.pending.clear();
        self.samples.clear();
        Ok(())
    }

    fn read_iq(&mut self, out: &mut [(f32, f32)]) -> Result<usize, String> {
        if !self.started {
            return Err("the radio is not streaming; call start first".into());
        }
        if self.samples.is_empty() {
            self.fill()?;
        }
        let n = out.len().min(self.samples.len());
        for (slot, s) in out.iter_mut().zip(self.samples.drain(..n)) {
            *slot = s;
        }
        Ok(n)
    }

    fn stop(&mut self) {
        if self.started {
            let _ = self.send_setting(SET_STREAMING_ENABLED, 0);
        }
        self.started = false;
        self.samples.clear();
        self.pending.clear();
    }
}

/// Opens a TCP connection to `id`, trying each address it resolves to.
fn connect(id: &str) -> Result<TcpStream, String> {
    let no_address = || format!("cannot find the address {id}; check the host name and port");
    let targets: Vec<SocketAddr> = id.to_socket_addrs().map_err(|_| no_address())?.collect();
    let mut last: Option<io::Error> = None;
    for t in targets {
        match TcpStream::connect_timeout(&t, CONNECT_TIMEOUT) {
            Ok(s) => return Ok(s),
            Err(e) => last = Some(e),
        }
    }
    match last {
        None => Err(no_address()),
        Some(e) => Err(match e.kind() {
            ErrorKind::ConnectionRefused => format!(
                "the connection to {id} was refused: nothing is listening on that port, or the address is wrong"
            ),
            ErrorKind::TimedOut | ErrorKind::WouldBlock => {
                format!("no answer from {id} within {} seconds", CONNECT_TIMEOUT.as_secs())
            }
            _ => format!("could not connect to {id}: {e}"),
        }),
    }
}

/// Reads one whole message (header and body) during the handshake.
fn read_message(s: &mut TcpStream) -> io::Result<(u32, u32, Vec<u8>)> {
    let mut h = [0u8; HEADER_LEN];
    s.read_exact(&mut h)?;
    let (ty, flags, size) = parse_header(&h)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "bad message header"))?;
    let mut body = vec![0u8; size as usize];
    s.read_exact(&mut body)?;
    Ok((ty, flags, body))
}

/// Splits a message header into (type, gain flags, body size). None when the body is too large.
fn parse_header(h: &[u8]) -> Option<(u32, u32, u32)> {
    if h.len() < HEADER_LEN {
        return None;
    }
    let mt = word(h, 1);
    let size = word(h, 4);
    if size > MAX_BODY {
        return None;
    }
    Some((mt & 0xffff, mt >> 16, size))
}

/// A command on the wire: type, body size, body.
fn command_bytes(ty: u32, body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(8 + body.len());
    v.extend_from_slice(&ty.to_le_bytes());
    v.extend_from_slice(&(body.len() as u32).to_le_bytes());
    v.extend_from_slice(body);
    v
}

/// The little endian 32-bit word `k` of `b`; 0 when it is past the end.
fn word(b: &[u8], k: usize) -> u32 {
    match b.get(4 * k..4 * k + 4) {
        Some(s) => u32::from_le_bytes([s[0], s[1], s[2], s[3]]),
        None => 0,
    }
}

/// Rates the server offers for its radio's full rate, fastest first. Index k is decimation step `min_dec + k`.
fn offered_rates(max_rate: u32, dec_stages: u32, min_dec: u32) -> Vec<u32> {
    let stages = dec_stages.max(min_dec.saturating_add(1));
    let mut out = Vec::new();
    for d in min_dec..stages {
        if out.len() >= 20 || d >= 32 {
            break;
        }
        let divisor = (1u64 << d) as f64;
        out.push((max_rate as f64 / divisor).round() as u32);
    }
    out
}

/// Largest gain index; at least 1 so the stage always has a range.
fn gain_top(info: &ServerInfo) -> u32 {
    info.gain_max.max(1)
}

fn dev_name(t: u32) -> &'static str {
    match t {
        1 => "Airspy One",
        2 => "Airspy HF+",
        3 => "RTL-SDR",
        _ => "radio",
    }
}

fn is_wait(e: &io::Error) -> bool {
    matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread::{self, JoinHandle};

    /// A message as the server sends it: 20-byte header, then the body.
    fn frame(ty: u32, flags: u32, body: &[u8]) -> Vec<u8> {
        let mut v = words(&[0, ty | (flags << 16), 1, 0, body.len() as u32]);
        v.extend_from_slice(body);
        v
    }

    fn words(ws: &[u32]) -> Vec<u8> {
        ws.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    /// DEVICE_INFO: type 1, 10 Msps, 4 decimation steps, 24 MHz to 1.8 GHz.
    fn device_info(max_gain: u32) -> Vec<u8> {
        words(&[1, 0x1234, 10_000_000, 10_000_000, 4, 1, max_gain, 24_000_000, 1_800_000_000, 0, 0, 0])
    }

    /// CLIENT_SYNC: control flag, gain 7, centre 100 MHz, IQ centre range 24 MHz to 1.8 GHz.
    fn sync(can_control: u32) -> Vec<u8> {
        words(&[can_control, 7, 100_000_000, 100_000_000, 0, 24_000_000, 1_800_000_000, 0, 0])
    }

    /// Reads one command from the client; None when the client has closed.
    fn read_command(s: &mut TcpStream) -> Option<(u32, Vec<u8>)> {
        let mut h = [0u8; 8];
        s.read_exact(&mut h).ok()?;
        let ty = u32::from_le_bytes([h[0], h[1], h[2], h[3]]);
        let n = u32::from_le_bytes([h[4], h[5], h[6], h[7]]) as usize;
        let mut body = vec![0u8; n];
        s.read_exact(&mut body).ok()?;
        Some((ty, body))
    }

    /// The command bytes for SET_SETTING with id and value.
    fn setting(id: u32, value: u32) -> (u32, Vec<u8>) {
        let mut b = id.to_le_bytes().to_vec();
        b.extend_from_slice(&value.to_le_bytes());
        (CMD_SET_SETTING, b)
    }

    /// Checks the client's HELLO, then sends DEVICE_INFO and CLIENT_SYNC.
    fn answer_hello(s: &mut TcpStream, can_control: u32) {
        let (ty, body) = read_command(s).expect("hello");
        assert_eq!(ty, CMD_HELLO);
        assert_eq!(body, [PROTOCOL_VERSION.to_le_bytes().to_vec(), b"KAHF".to_vec()].concat());
        s.write_all(&frame(MSG_DEVICE_INFO, 0, &device_info(21))).unwrap();
        s.write_all(&frame(MSG_CLIENT_SYNC, 0, &sync(can_control))).unwrap();
    }

    /// Reads commands until the client switches streaming on.
    fn wait_for_stream_start(s: &mut TcpStream) {
        let on = setting(SET_STREAMING_ENABLED, 1);
        loop {
            let c = read_command(s).expect("command before start");
            if c == on {
                break;
            }
        }
    }

    /// A fake server on 127.0.0.1 with a free port. `script` runs on the one accepted connection.
    fn fake_server<T, F>(script: F) -> (String, JoinHandle<T>)
    where
        T: Send + 'static,
        F: FnOnce(TcpStream) -> T + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("address").to_string();
        let h = thread::spawn(move || {
            let (s, _) = listener.accept().expect("accept");
            script(s)
        });
        (addr, h)
    }

    #[test]
    fn hello_device_info_and_commands() {
        let (addr, h) = fake_server(|mut s| {
            answer_hello(&mut s, 1);
            let mut seen = Vec::new();
            while let Some(c) = read_command(&mut s) {
                seen.push(c);
            }
            seen
        });
        {
            let mut r = open(Some(&addr)).expect("open");
            assert_eq!(r.freq_range_hz(), (24_000_000, 1_800_000_000));
            assert_eq!(r.sample_rates(), vec![1_250_000, 2_500_000, 5_000_000, 10_000_000]);
            assert!(r.label().contains("Airspy One"));
            let stages = r.gain_stages();
            assert_eq!(stages.len(), 1);
            assert!(!stages[0].auto);
            assert_eq!(r.set_sample_rate(2_600_000).unwrap(), 2_500_000);
            r.set_frequency_hz(100_000_000).unwrap();
            r.set_gain("gain", GainSetting::Manual(7.4)).unwrap();
            assert!(r.set_gain("gain", GainSetting::Auto).is_err());
            assert!(r.set_frequency_hz(10).is_err());
        }
        let seen = h.join().expect("server thread");
        assert_eq!(
            seen,
            vec![
                setting(SET_STREAMING_MODE, MODE_IQ_ONLY),
                setting(SET_IQ_DECIMATION, 2),
                setting(SET_IQ_FREQUENCY, 100_000_000),
                setting(SET_GAIN, 7),
            ]
        );
    }

    #[test]
    fn decodes_packets_split_across_reads_and_applies_gain() {
        let (addr, h) = fake_server(|mut s| {
            answer_hello(&mut s, 1);
            wait_for_stream_start(&mut s);
            // one 16-bit sample (0.5, -0.5)
            s.write_all(&frame(MSG_IQ_INT16, 0, &[0x00, 0x40, 0x00, 0xC0])).unwrap();
            // two 8-bit samples, sent in two writes
            let u8_packet = frame(MSG_IQ_UINT8, 0, &[128, 255, 0, 128]);
            s.write_all(&u8_packet[..7]).unwrap();
            s.flush().unwrap();
            thread::sleep(Duration::from_millis(50));
            s.write_all(&u8_packet[7..]).unwrap();
            // one float sample (1.0, -2.0) with 20 dB of digital gain taken back out
            let mut f = 1.0f32.to_le_bytes().to_vec();
            f.extend_from_slice(&(-2.0f32).to_le_bytes());
            s.write_all(&frame(MSG_IQ_FLOAT, 20, &f)).unwrap();
            // keep the connection open until the client closes it
            let mut b = [0u8; 1];
            let _ = s.read(&mut b);
        });
        let mut r = open(Some(&addr)).expect("open");
        r.start().expect("start");
        let mut got: Vec<(f32, f32)> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut buf = [(0.0f32, 0.0f32); 8];
        while got.len() < 4 && Instant::now() < deadline {
            let n = r.read_iq(&mut buf).expect("read");
            got.extend_from_slice(&buf[..n]);
        }
        drop(r);
        h.join().expect("server thread");

        let expect: [(f32, f32); 4] = [(0.5, -0.5), (0.0, 127.0 / 128.0), (-1.0, 0.0), (0.1, -0.2)];
        assert_eq!(got.len(), 4, "{got:?}");
        for (g, e) in got.iter().zip(expect.iter()) {
            assert!((g.0 - e.0).abs() < 1e-5 && (g.1 - e.1).abs() < 1e-5, "{g:?} is not {e:?}");
        }
    }

    #[test]
    fn quiet_link_returns_zero_within_about_200_ms() {
        let (addr, h) = fake_server(|mut s| {
            answer_hello(&mut s, 1);
            wait_for_stream_start(&mut s);
            let mut b = [0u8; 1];
            let _ = s.read(&mut b);
        });
        let mut r = open(Some(&addr)).expect("open");
        r.start().expect("start");
        let mut buf = [(0.0f32, 0.0f32); 4];
        let t = Instant::now();
        let n = r.read_iq(&mut buf).expect("a quiet link is not an error");
        let took = t.elapsed();
        assert_eq!(n, 0);
        assert!(took < Duration::from_secs(1), "took {took:?}");
        drop(r);
        h.join().expect("server thread");
    }

    #[test]
    fn oversize_message_header_is_an_error() {
        let (addr, h) = fake_server(|mut s| {
            answer_hello(&mut s, 1);
            wait_for_stream_start(&mut s);
            s.write_all(&words(&[0, 1, 1, 0, MAX_BODY + 1])).unwrap();
            let mut b = [0u8; 1];
            let _ = s.read(&mut b);
        });
        let mut r = open(Some(&addr)).expect("open");
        r.start().expect("start");
        let mut buf = [(0.0f32, 0.0f32); 4];
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut err = None;
        while err.is_none() && Instant::now() < deadline {
            if let Err(e) = r.read_iq(&mut buf) {
                err = Some(e);
            }
        }
        drop(r);
        h.join().expect("server thread");
        let err = err.expect("an error");
        assert!(err.contains("makes no sense"), "{err}");
    }

    #[test]
    fn refused_connection_gives_a_clear_error() {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let err = open(Some(&format!("127.0.0.1:{port}"))).err().expect("an error");
        assert!(err.contains("refused"), "{err}");
    }

    #[test]
    fn bad_ids_are_explained() {
        assert!(open(None).err().unwrap().contains("host:port"));
        assert!(open(Some("  ")).err().unwrap().contains("host:port"));
        assert!(open(Some("localhost")).err().unwrap().contains("host:port"));
        assert!(open(Some("127.0.0.1:notaport")).err().unwrap().contains("host:port"));
    }

    #[test]
    fn a_server_that_closes_at_once_is_not_a_server() {
        let (addr, h) = fake_server(|s| drop(s));
        let err = open(Some(&addr)).err().expect("an error");
        assert!(err.contains("not a SpyServer"), "{err}");
        h.join().expect("server thread");
    }

    #[test]
    fn read_only_server_refuses_gain_and_unknown_stage() {
        let (addr, h) = fake_server(|mut s| {
            answer_hello(&mut s, 0);
            while read_command(&mut s).is_some() {}
        });
        {
            let mut r = open(Some(&addr)).expect("open");
            let e = r.set_gain("gain", GainSetting::Manual(3.0)).err().expect("an error");
            assert!(e.contains("read only"), "{e}");
            assert!(r.set_gain("nope", GainSetting::Manual(1.0)).is_err());
        }
        h.join().expect("server thread");
    }

    #[test]
    fn rates_and_headers_decode() {
        assert_eq!(offered_rates(10_000_000, 4, 0), vec![10_000_000, 5_000_000, 2_500_000, 1_250_000]);
        assert_eq!(offered_rates(10_000_000, 3, 2), vec![2_500_000]);
        assert_eq!(parse_header(&frame(1, 20, &[])), Some((1, 20, 0)));
        assert_eq!(parse_header(&words(&[0, 0, 0, 0, MAX_BODY + 1])), None);
    }
}
