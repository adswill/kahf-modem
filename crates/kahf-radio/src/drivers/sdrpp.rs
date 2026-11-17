//! Driver for an sdrpp server: a radio that a server on the network serves over TCP
//! (port 5259 by default). One client at a time.
//!
//! Wire format, all numbers little endian. Every packet starts with a 32-bit type and a
//! 32-bit size, and the size counts those 8 header bytes. The client sends command packets:
//! a 32-bit command number followed by its argument. The server sends its sample rate as a
//! command when a client connects and again whenever it changes. It answers some commands
//! with an ack packet that repeats the command number. A second client is turned away with a
//! disconnect command. Samples arrive in baseband packets: a 16-bit compression type (0 means
//! none), a 16-bit sample type, a 32-bit float scale, then the I/Q samples.
//!
//! The sample rate and the gain are set in the server's own menu. This driver cannot change
//! them over the network yet: it reports the rate the server runs at and leaves the gain as
//! the server has it.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use crate::{DeviceInfo, GainSetting, GainStage, RxRadio};

const EXAMPLE: &str = "192.168.1.10:5259";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Longest wait for the server's first packets during open.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(4);
/// Longest wait for the server to confirm a frequency change.
const ACK_TIMEOUT: Duration = Duration::from_secs(4);
/// Longest wait inside read_iq when no samples arrive.
const READ_WAIT: Duration = Duration::from_millis(200);
/// Silence on a running stream for this long means the radio is lost.
const STALL_LIMIT: Duration = Duration::from_secs(5);
/// Samples kept between read_iq calls. Beyond this the oldest are dropped and counted.
const QUEUE_LIMIT: usize = 1 << 20;
const MAX_PACKET: u32 = 32 << 20;
/// Largest packet type the server uses (the error packet).
const MAX_PACKET_TYPE: u32 = 6;
const READ_CHUNK: usize = 64 * 1024;

const PKT_COMMAND: u32 = 0;
const PKT_ACK: u32 = 1;
const PKT_BASEBAND: u32 = 2;
const PKT_BASEBAND_COMPRESSED: u32 = 3;
const PKT_ERROR: u32 = 6;

const CMD_START: u32 = 2;
const CMD_STOP: u32 = 3;
const CMD_SET_FREQUENCY: u32 = 4;
const CMD_SET_SAMPLE_TYPE: u32 = 6;
const CMD_SET_COMPRESSION: u32 = 7;
const CMD_SET_SAMPLE_RATE: u32 = 0x80;
const CMD_DISCONNECT: u32 = 0x81;

const SAMPLE_INT8: u16 = 0;
const SAMPLE_INT16: u16 = 1;
const SAMPLE_FLOAT: u16 = 2;
/// Sample type asked for: 16-bit keeps weak signals apart from strong ones.
const WANTED_SAMPLE_TYPE: u8 = SAMPLE_INT16 as u8;
const NO_COMPRESSION: u8 = 0;

/// Frequency range this driver accepts, in Hz. The server decides what its radio can tune.
const FREQ_RANGE: (u64, u64) = (1_000_000, 6_000_000_000);

/// Probing is not done: the operator names the server, so there is no list to read.
pub fn probe() -> Vec<DeviceInfo> {
    Vec::new()
}

pub fn open(id: Option<&str>) -> Result<Box<dyn RxRadio>, String> {
    let id = id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("this radio needs its server address as host:port, for example {EXAMPLE}"))?;
    let (host, port) = split_host_port(id)?;
    let stream = connect(id, &host, port)?;
    stream
        .set_nodelay(true)
        .map_err(|e| format!("could not set up {id}: {e}"))?;
    let mut radio = SdrppRadio::new(id.to_string(), stream);
    radio.handshake()?;
    Ok(Box::new(radio))
}

fn split_host_port(id: &str) -> Result<(String, u16), String> {
    let bad = || format!("bad address {id:?}: use host:port, for example {EXAMPLE}");
    let (host, port) = id.rsplit_once(':').ok_or_else(bad)?;
    let bracketed = host.starts_with('[') && host.ends_with(']');
    let host = if bracketed { &host[1..host.len() - 1] } else { host };
    if host.is_empty() || host.chars().any(char::is_whitespace) || (host.contains(':') && !bracketed) {
        return Err(bad());
    }
    let port: u16 = port.trim().parse().map_err(|_| bad())?;
    if port == 0 {
        return Err(bad());
    }
    Ok((host.to_string(), port))
}

fn connect(id: &str, host: &str, port: u16) -> Result<TcpStream, String> {
    let addrs = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("could not look up {host}: {e}"))?;
    let mut last = None;
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(s) => return Ok(s),
            Err(e) => last = Some(e),
        }
    }
    Err(connect_error(id, last))
}

fn connect_error(id: &str, e: Option<io::Error>) -> String {
    match e {
        Some(e) if e.kind() == io::ErrorKind::ConnectionRefused => format!(
            "nothing is listening at {id}: the connection was refused. Check the address and port, \
             and that the sdrpp server is running with its network port open"
        ),
        Some(e) if is_timeout(&e) => {
            format!("no answer from {id} within {} s", CONNECT_TIMEOUT.as_secs())
        }
        Some(e) => format!("could not connect to {id}: {e}"),
        None => format!("could not connect to {id}: no address found"),
    }
}

fn is_timeout(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock)
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn f32_at(b: &[u8], at: usize) -> Option<f32> {
    Some(f32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn f64_at(b: &[u8], at: usize) -> Option<f64> {
    Some(f64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

fn error_text(code: u8) -> String {
    match code {
        0 => "no error".into(),
        1 => "invalid packet".into(),
        2 => "invalid command".into(),
        3 => "invalid argument".into(),
        other => format!("error code {other}"),
    }
}

/// One whole packet from the server: its type and the body after the 8 header bytes.
struct Packet {
    ty: u32,
    body: Vec<u8>,
}

/// Cuts one whole packet off the front of `buf`. Ok(None) when the packet is not complete yet.
fn take_packet(buf: &mut Vec<u8>, id: &str) -> Result<Option<Packet>, String> {
    if buf.len() < 8 {
        return Ok(None);
    }
    let ty = u32_at(buf, 0).unwrap_or(u32::MAX);
    let size = u32_at(buf, 4).unwrap_or(0);
    if !(8..=MAX_PACKET).contains(&size) || ty > MAX_PACKET_TYPE {
        return Err(format!(
            "{id} sent data that is not the sdrpp protocol. Is this its port (5259 by default)?"
        ));
    }
    let size = size as usize;
    if buf.len() < size {
        return Ok(None);
    }
    let body = buf[8..size].to_vec();
    buf.drain(..size);
    Ok(Some(Packet { ty, body }))
}

struct SdrppRadio {
    id: String,
    stream: TcpStream,
    /// Bytes received but not yet cut into packets.
    buf: Vec<u8>,
    /// Scratch space for socket reads.
    chunk: Vec<u8>,
    /// Decoded samples waiting for read_iq.
    queue: VecDeque<(f32, f32)>,
    /// Sample rate the server reported. 0 until it does.
    rate: f64,
    /// Last error the server sent, kept for messages.
    server_error: Option<String>,
    started: bool,
    alive: bool,
    dropped: u64,
    last_rx: Instant,
}

impl SdrppRadio {
    fn new(id: String, stream: TcpStream) -> Self {
        SdrppRadio {
            id,
            stream,
            buf: Vec::new(),
            chunk: vec![0u8; READ_CHUNK],
            queue: VecDeque::new(),
            rate: 0.0,
            server_error: None,
            started: false,
            alive: true,
            dropped: 0,
            last_rx: Instant::now(),
        }
    }

    /// Waits for the server's first packet: its sample rate. A second client gets a
    /// disconnect command here, which fails the open with a clear message.
    fn handshake(&mut self) -> Result<(), String> {
        let end = Instant::now() + HANDSHAKE_TIMEOUT;
        while self.rate <= 0.0 {
            let now = Instant::now();
            if now >= end {
                return Err(format!(
                    "{} sent no sample rate within {} s. Is this the sdrpp server port (5259 by default)?",
                    self.id,
                    HANDSHAKE_TIMEOUT.as_secs()
                ));
            }
            match self.read_packet(end - now)? {
                Some(p) => self.handle(p)?,
                None => continue,
            }
        }
        Ok(())
    }

    /// Reads from the socket until one whole packet is buffered, waiting at most `wait`.
    /// Ok(None) when no whole packet arrived in time.
    fn read_packet(&mut self, wait: Duration) -> Result<Option<Packet>, String> {
        let end = Instant::now() + wait;
        loop {
            if let Some(p) = take_packet(&mut self.buf, &self.id)? {
                return Ok(Some(p));
            }
            let now = Instant::now();
            if now >= end {
                return Ok(None);
            }
            self.stream
                .set_read_timeout(Some(end - now))
                .map_err(|e| format!("the connection to {} failed: {e}", self.id))?;
            match self.stream.read(&mut self.chunk) {
                Ok(0) => {
                    self.alive = false;
                    return Err(format!("the server at {} closed the connection", self.id));
                }
                Ok(n) => {
                    self.buf.extend_from_slice(&self.chunk[..n]);
                    self.last_rx = Instant::now();
                }
                Err(e) if is_timeout(&e) || e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => {
                    self.alive = false;
                    return Err(format!("the connection to {} failed: {e}", self.id));
                }
            }
        }
    }

    /// Acts on one packet. Samples are queued, the rate is kept, a disconnect is an error.
    fn handle(&mut self, p: Packet) -> Result<(), String> {
        match p.ty {
            PKT_BASEBAND => {
                self.baseband(&p.body);
                Ok(())
            }
            PKT_COMMAND => match u32_at(&p.body, 0) {
                Some(CMD_SET_SAMPLE_RATE) => {
                    if let Some(r) = f64_at(&p.body, 4) {
                        if r.is_finite() && r > 0.0 {
                            self.rate = r;
                        }
                    }
                    Ok(())
                }
                Some(CMD_DISCONNECT) => {
                    self.alive = false;
                    Err(format!(
                        "another client holds {}: the server serves one client at a time",
                        self.id
                    ))
                }
                _ => Ok(()),
            },
            PKT_ERROR => {
                let code = p.body.first().copied().unwrap_or(0);
                self.server_error = Some(error_text(code));
                Ok(())
            }
            PKT_BASEBAND_COMPRESSED | PKT_ACK => Ok(()),
            _ => Ok(()),
        }
    }

    /// Decodes a baseband packet and queues its samples. Only uncompressed packets are read.
    fn baseband(&mut self, body: &[u8]) {
        if body.len() < 8 || u16_at(body, 0) != Some(NO_COMPRESSION as u16) {
            return;
        }
        let sample_type = u16_at(body, 2).unwrap_or(u16::MAX);
        let scale = f32_at(body, 4).unwrap_or(0.0);
        let data = &body[8..];
        match sample_type {
            SAMPLE_FLOAT => {
                for c in data.as_chunks::<8>().0 {
                    let i = f32_at(c, 0).unwrap_or(0.0);
                    let q = f32_at(c, 4).unwrap_or(0.0);
                    self.push_sample(i, q);
                }
            }
            SAMPLE_INT16 => {
                let m = scale / 32768.0;
                for c in data.as_chunks::<4>().0 {
                    let i = i16::from_le_bytes([c[0], c[1]]) as f32 * m;
                    let q = i16::from_le_bytes([c[2], c[3]]) as f32 * m;
                    self.push_sample(i, q);
                }
            }
            SAMPLE_INT8 => {
                let m = scale / 128.0;
                for c in data.as_chunks::<2>().0 {
                    self.push_sample(c[0] as i8 as f32 * m, c[1] as i8 as f32 * m);
                }
            }
            _ => {}
        }
    }

    fn push_sample(&mut self, i: f32, q: f32) {
        if self.queue.len() >= QUEUE_LIMIT {
            self.queue.pop_front();
            self.dropped += 1;
        }
        self.queue.push_back((i, q));
    }

    /// Sends one command packet: header, command number, argument.
    fn send_command(&mut self, cmd: u32, arg: &[u8]) -> Result<(), String> {
        let size = (12 + arg.len()) as u32;
        let mut out = Vec::with_capacity(size as usize);
        out.extend_from_slice(&PKT_COMMAND.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&cmd.to_le_bytes());
        out.extend_from_slice(arg);
        self.stream.write_all(&out).map_err(|e| {
            self.alive = false;
            format!("the connection to {} failed: {e}", self.id)
        })
    }

    /// Waits up to `wait` for the ack of `cmd`. Other packets that arrive are handled.
    /// Ok(false) when the ack did not come in time.
    fn wait_ack(&mut self, cmd: u32, wait: Duration) -> Result<bool, String> {
        let end = Instant::now() + wait;
        loop {
            let now = Instant::now();
            if now >= end {
                return Ok(false);
            }
            match self.read_packet(end - now)? {
                Some(p) => {
                    if p.ty == PKT_ACK && u32_at(&p.body, 0) == Some(cmd) {
                        return Ok(true);
                    }
                    self.handle(p)?;
                }
                None => return Ok(false),
            }
        }
    }

    fn rate_hz(&self) -> Option<u32> {
        (self.rate > 0.0).then(|| self.rate.round() as u32)
    }
}

impl RxRadio for SdrppRadio {
    fn label(&self) -> String {
        format!("sdrpp server {}", self.id)
    }

    fn freq_range_hz(&self) -> (u64, u64) {
        FREQ_RANGE
    }

    fn set_frequency_hz(&mut self, hz: u64) -> Result<(), String> {
        if hz < FREQ_RANGE.0 || hz > FREQ_RANGE.1 {
            return Err(format!(
                "{hz} Hz is outside the {} to {} Hz range this driver accepts",
                FREQ_RANGE.0, FREQ_RANGE.1
            ));
        }
        if !self.alive {
            return Err(format!("the connection to {} is closed", self.id));
        }
        // The argument is a 64-bit float, as the server expects.
        self.send_command(CMD_SET_FREQUENCY, &(hz as f64).to_le_bytes())?;
        let acked = match self.wait_ack(CMD_SET_FREQUENCY, ACK_TIMEOUT) {
            Ok(a) => a,
            Err(e) => {
                self.alive = false;
                return Err(e);
            }
        };
        if !acked {
            let detail = self
                .server_error
                .take()
                .map(|e| format!(" (server said: {e})"))
                .unwrap_or_default();
            return Err(format!(
                "{} did not confirm the frequency within {} s{detail}",
                self.id,
                ACK_TIMEOUT.as_secs()
            ));
        }
        // Samples queued before the change were taken at the old frequency.
        self.queue.clear();
        Ok(())
    }

    fn sample_rates(&self) -> Vec<u32> {
        self.rate_hz().into_iter().collect()
    }

    /// The server runs at one rate, and this driver cannot change it. The rate in use is
    /// returned, which is the only rate the server offers.
    fn set_sample_rate(&mut self, _hz: u32) -> Result<u32, String> {
        self.rate_hz()
            .ok_or_else(|| format!("{} has not reported its sample rate yet", self.id))
    }

    /// The gain is set in the server's own menu, so the driver lists no stages.
    fn gain_stages(&self) -> Vec<GainStage> {
        Vec::new()
    }

    fn set_gain(&mut self, stage: &str, _setting: GainSetting) -> Result<(), String> {
        Err(format!(
            "this driver cannot set the gain {stage:?}; set it in the server's own menu"
        ))
    }

    fn start(&mut self) -> Result<(), String> {
        if self.started {
            return Ok(());
        }
        if !self.alive {
            return Err(format!("the connection to {} is closed", self.id));
        }
        self.send_command(CMD_SET_COMPRESSION, &[NO_COMPRESSION])?;
        self.send_command(CMD_SET_SAMPLE_TYPE, &[WANTED_SAMPLE_TYPE])?;
        self.send_command(CMD_START, &[])?;
        self.started = true;
        self.queue.clear();
        self.last_rx = Instant::now();
        Ok(())
    }

    fn read_iq(&mut self, out: &mut [(f32, f32)]) -> Result<usize, String> {
        if !self.started {
            return Err("the radio is not started".into());
        }
        if !self.alive {
            return Err(format!("the connection to {} is closed", self.id));
        }
        if out.is_empty() {
            return Ok(0);
        }
        if self.queue.is_empty() {
            let end = Instant::now() + READ_WAIT;
            while self.queue.is_empty() {
                let now = Instant::now();
                if now >= end {
                    break;
                }
                match self.read_packet(end - now) {
                    Ok(Some(p)) => {
                        if let Err(e) = self.handle(p) {
                            self.alive = false;
                            return Err(e);
                        }
                    }
                    Ok(None) => break,
                    Err(e) => return Err(e),
                }
            }
        }
        if self.queue.is_empty() && self.last_rx.elapsed() > STALL_LIMIT {
            self.alive = false;
            return Err(format!(
                "no samples from {} for {} s",
                self.id,
                STALL_LIMIT.as_secs()
            ));
        }
        let n = out.len().min(self.queue.len());
        for (slot, s) in out.iter_mut().zip(self.queue.drain(..n)) {
            *slot = s;
        }
        Ok(n)
    }

    fn stop(&mut self) {
        if !self.started {
            return;
        }
        self.started = false;
        self.queue.clear();
        // A failed send here means the link is already gone; nothing more to do.
        let _ = self.send_command(CMD_STOP, &[]);
    }

    fn dropped_samples(&self) -> u64 {
        self.dropped
    }
}

impl Drop for SdrppRadio {
    fn drop(&mut self) {
        // Stop the server's radio so the next client finds it idle.
        self.stop();
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    fn packet(ty: u32, body: &[u8]) -> Vec<u8> {
        let mut v = Vec::with_capacity(8 + body.len());
        v.extend_from_slice(&ty.to_le_bytes());
        v.extend_from_slice(&((8 + body.len()) as u32).to_le_bytes());
        v.extend_from_slice(body);
        v
    }

    fn command(cmd: u32, arg: &[u8]) -> Vec<u8> {
        let mut b = cmd.to_le_bytes().to_vec();
        b.extend_from_slice(arg);
        packet(PKT_COMMAND, &b)
    }

    fn ack(cmd: u32) -> Vec<u8> {
        packet(PKT_ACK, &cmd.to_le_bytes())
    }

    fn rate(hz: f64) -> Vec<u8> {
        command(CMD_SET_SAMPLE_RATE, &hz.to_le_bytes())
    }

    fn baseband(sample_type: u16, scale: f32, data: &[u8]) -> Vec<u8> {
        let mut b = 0u16.to_le_bytes().to_vec();
        b.extend_from_slice(&sample_type.to_le_bytes());
        b.extend_from_slice(&scale.to_le_bytes());
        b.extend_from_slice(data);
        packet(PKT_BASEBAND, &b)
    }

    /// Reads one packet sent by the client: (type, body).
    fn read_packet(s: &mut TcpStream) -> io::Result<(u32, Vec<u8>)> {
        let mut h = [0u8; 8];
        s.read_exact(&mut h)?;
        let ty = u32::from_le_bytes([h[0], h[1], h[2], h[3]]);
        let size = u32::from_le_bytes([h[4], h[5], h[6], h[7]]) as usize;
        let mut body = vec![0u8; size.saturating_sub(8)];
        s.read_exact(&mut body)?;
        Ok((ty, body))
    }

    /// Runs `f` on the first client of a fake server on 127.0.0.1 and returns the address.
    fn spawn_server<F>(f: F) -> String
    where
        F: FnOnce(TcpStream) + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        thread::spawn(move || {
            if let Ok((s, _)) = listener.accept() {
                f(s);
            }
        });
        addr
    }

    /// A fake sdrpp server: sends `greeting` on connect, acks frequency changes, sends
    /// `on_start` when the client starts. Every client packet is passed to the returned channel.
    fn serve(greeting: Vec<u8>, on_start: Vec<u8>) -> (String, mpsc::Receiver<Vec<u8>>) {
        let (tx, rx) = mpsc::channel();
        let addr = spawn_server(move |mut s| {
            let _ = s.write_all(&greeting);
            while let Ok((ty, body)) = read_packet(&mut s) {
                let cmd = u32_at(&body, 0).unwrap_or(u32::MAX);
                let _ = tx.send(packet(ty, &body));
                match cmd {
                    CMD_SET_FREQUENCY => {
                        let _ = s.write_all(&ack(CMD_SET_FREQUENCY));
                    }
                    CMD_START => {
                        let _ = s.write_all(&on_start);
                    }
                    _ => {}
                }
            }
        });
        (addr, rx)
    }

    /// Collects every client packet until the client disconnects.
    fn collect(rx: &mpsc::Receiver<Vec<u8>>) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        while let Ok(p) = rx.recv_timeout(Duration::from_secs(3)) {
            out.push(p);
        }
        out
    }

    /// Calls read_iq until `want` samples arrived or the attempts run out.
    fn read_some(radio: &mut dyn RxRadio, want: usize) -> Vec<(f32, f32)> {
        let mut got = Vec::new();
        let mut buf = vec![(0.0f32, 0.0f32); 16];
        for _ in 0..50 {
            let n = radio.read_iq(&mut buf).unwrap();
            got.extend_from_slice(&buf[..n]);
            if got.len() >= want {
                break;
            }
        }
        got
    }

    fn open_ok(addr: &str) -> Box<dyn RxRadio> {
        open(Some(addr)).unwrap_or_else(|e| panic!("open failed: {e}"))
    }

    #[test]
    fn probe_lists_nothing() {
        assert!(probe().is_empty());
    }

    #[test]
    fn handshake_reports_rate_and_label() {
        let (addr, _rx) = serve(rate(2_048_000.0), Vec::new());
        let radio = open_ok(&addr);
        assert_eq!(radio.label(), format!("sdrpp server {addr}"));
        assert_eq!(radio.sample_rates(), vec![2_048_000]);
        assert_eq!(radio.gain_stages(), Vec::new());
        assert_eq!(radio.freq_range_hz(), FREQ_RANGE);
    }

    #[test]
    fn commands_are_sent_as_exact_bytes() {
        let (addr, rx) = serve(rate(2_048_000.0), Vec::new());
        let mut radio = open_ok(&addr);
        radio.set_frequency_hz(100_000_000).unwrap();
        radio.start().unwrap();
        radio.stop();
        drop(radio);
        let got = collect(&rx);

        let mut freq = vec![0, 0, 0, 0, 20, 0, 0, 0, 4, 0, 0, 0];
        freq.extend_from_slice(&100_000_000f64.to_le_bytes());
        let compression = vec![0, 0, 0, 0, 13, 0, 0, 0, 7, 0, 0, 0, 0];
        let sample_type = vec![0, 0, 0, 0, 13, 0, 0, 0, 6, 0, 0, 0, 1];
        let start = vec![0, 0, 0, 0, 12, 0, 0, 0, 2, 0, 0, 0];
        let stop = vec![0, 0, 0, 0, 12, 0, 0, 0, 3, 0, 0, 0];
        assert_eq!(got, vec![freq, compression, sample_type, start, stop]);
    }

    #[test]
    fn int16_samples_are_scaled() {
        let mut data = Vec::new();
        for v in [16384i16, -16384, 8192, 0] {
            data.extend_from_slice(&v.to_le_bytes());
        }
        // Samples (16384, -16384) and (8192, 0) at scale 2.0 give (1, -1) and (0.5, 0).
        let (addr, _rx) = serve(rate(2_048_000.0), baseband(SAMPLE_INT16, 2.0, &data));
        let mut radio = open_ok(&addr);
        radio.start().unwrap();
        assert_eq!(read_some(radio.as_mut(), 2), vec![(1.0, -1.0), (0.5, 0.0)]);
    }

    #[test]
    fn int8_and_float_samples_decode() {
        let (addr, _rx) = serve(rate(2_048_000.0), baseband(SAMPLE_INT8, 128.0, &[1, 255]));
        let mut radio = open_ok(&addr);
        radio.start().unwrap();
        assert_eq!(read_some(radio.as_mut(), 1), vec![(1.0, -1.0)]);

        let mut data = 0.25f32.to_le_bytes().to_vec();
        data.extend_from_slice(&(-0.5f32).to_le_bytes());
        let (addr, _rx) = serve(rate(2_048_000.0), baseband(SAMPLE_FLOAT, 1.0, &data));
        let mut radio = open_ok(&addr);
        radio.start().unwrap();
        assert_eq!(read_some(radio.as_mut(), 1), vec![(0.25, -0.5)]);
    }

    #[test]
    fn read_iq_returns_zero_without_samples() {
        let (addr, _rx) = serve(rate(2_048_000.0), Vec::new());
        let mut radio = open_ok(&addr);
        radio.start().unwrap();
        let mut buf = vec![(0.0f32, 0.0f32); 8];
        let t = Instant::now();
        assert_eq!(radio.read_iq(&mut buf), Ok(0));
        assert!(
            t.elapsed() < Duration::from_millis(600),
            "waited {:?}",
            t.elapsed()
        );
    }

    #[test]
    fn read_iq_before_start_is_an_error() {
        let (addr, _rx) = serve(rate(2_048_000.0), Vec::new());
        let mut radio = open_ok(&addr);
        let mut buf = vec![(0.0f32, 0.0f32); 4];
        assert!(radio.read_iq(&mut buf).is_err());
    }

    #[test]
    fn closed_connection_is_reported() {
        let addr = spawn_server(|mut s| {
            let _ = s.write_all(&rate(2_048_000.0));
            // Read the compression, sample type and start commands, then drop the connection.
            for _ in 0..3 {
                let _ = read_packet(&mut s);
            }
        });
        let mut radio = open_ok(&addr);
        radio.start().unwrap();
        let mut buf = vec![(0.0f32, 0.0f32); 4];
        let mut err = None;
        for _ in 0..20 {
            if let Err(e) = radio.read_iq(&mut buf) {
                err = Some(e);
                break;
            }
        }
        assert!(err.is_some(), "read_iq should report the lost connection");
    }

    #[test]
    fn silent_server_fails_the_open() {
        let addr = spawn_server(|s| {
            thread::sleep(Duration::from_secs(6));
            drop(s);
        });
        let err = open(Some(&addr)).err().expect("open should fail");
        assert!(err.contains("sent no sample rate"), "{err}");
    }

    #[test]
    fn second_client_is_turned_away() {
        let addr = spawn_server(|mut s| {
            let _ = s.write_all(&command(CMD_DISCONNECT, &[]));
            thread::sleep(Duration::from_millis(500));
        });
        let err = open(Some(&addr)).err().expect("open should fail");
        assert!(err.contains("another client"), "{err}");
    }

    #[test]
    fn garbage_is_not_the_protocol() {
        let addr = spawn_server(|mut s| {
            let _ = s.write_all(&[0xff; 16]);
            thread::sleep(Duration::from_millis(500));
        });
        let err = open(Some(&addr)).err().expect("open should fail");
        assert!(err.contains("not the sdrpp protocol"), "{err}");
    }

    #[test]
    fn refused_connection_is_clear() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        drop(listener);
        let err = open(Some(&addr)).err().expect("open should fail");
        assert!(err.contains("nothing is listening"), "{err}");
    }

    #[test]
    fn frequency_out_of_range_is_refused_without_sending() {
        let (addr, rx) = serve(rate(2_048_000.0), Vec::new());
        let mut radio = open_ok(&addr);
        assert!(radio.set_frequency_hz(0).is_err());
        drop(radio);
        assert!(collect(&rx).is_empty());
    }

    #[test]
    fn sample_rate_and_gain_are_read_only() {
        let (addr, _rx) = serve(rate(2_048_000.0), Vec::new());
        let mut radio = open_ok(&addr);
        assert_eq!(radio.set_sample_rate(1_000_000), Ok(2_048_000));
        assert!(radio.set_gain("LNA", GainSetting::Auto).is_err());
    }

    #[test]
    fn address_must_be_host_and_port() {
        assert_eq!(
            split_host_port("192.168.1.5:5259").unwrap(),
            ("192.168.1.5".to_string(), 5259)
        );
        assert_eq!(split_host_port("[::1]:5259").unwrap(), ("::1".to_string(), 5259));
        assert!(split_host_port("192.168.1.5").is_err());
        assert!(split_host_port("192.168.1.5:0").is_err());
        assert!(split_host_port("::1:5259").is_err());
        assert!(open(None).is_err());
    }
}
