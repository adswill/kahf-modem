//! Driver for a radio behind a network server that speaks the RTL-SDR network protocol.
//!
//! The server sends a 12 byte greeting on connect: four magic characters ("RTL0" for an
//! RTL-SDR), then the tuner type and the gain count as big endian 32-bit numbers. After that
//! it streams unsigned 8-bit I/Q pairs without end. The client sends 5 byte commands: one
//! command byte and a big endian 32-bit value. Frequency and rate are in Hz, gain in tenths of a dB.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::{DeviceInfo, GainSetting, GainStage, RxRadio};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const GREETING_TIMEOUT: Duration = Duration::from_secs(5);
/// Longest wait inside read_iq when no samples arrive.
const READ_WAIT: Duration = Duration::from_millis(200);
const GREETING_LEN: usize = 12;

const CMD_FREQ: u8 = 0x01;
const CMD_RATE: u8 = 0x02;
const CMD_GAIN_MODE: u8 = 0x03;
const CMD_GAIN: u8 = 0x04;
const CMD_AGC: u8 = 0x08;

/// Tuner types 1 to 6 are the ones the RTL-SDR tuners use.
const TUNER_FIRST: u32 = 1;
const TUNER_LAST: u32 = 6;

const RTL_RATES: &[u32] = &[
    250_000, 1_024_000, 1_536_000, 1_792_000, 1_920_000, 2_048_000, 2_160_000, 2_400_000, 2_560_000,
];
/// Any other server takes these rates; the protocol itself allows any value.
const ANY_RATES: &[u32] = &[
    250_000, 500_000, 1_000_000, 1_024_000, 2_000_000, 2_048_000, 2_400_000, 2_560_000, 4_000_000, 5_000_000,
    8_000_000, 10_000_000, 16_000_000, 20_000_000,
];

const RTL_FREQ_RANGE: (u64, u64) = (24_000_000, 1_766_000_000);
const ANY_FREQ_RANGE: (u64, u64) = (1_000_000, u32::MAX as u64);
const ANY_GAIN_RANGE: (f32, f32) = (0.0, 49.6);

// Gain lists in tenths of a dB, as the tuners list them.
const GAINS_E4000: &[i16] = &[-10, 15, 40, 65, 90, 115, 140, 165, 190, 215, 240, 290, 340, 420];
const GAINS_FC0012: &[i16] = &[-99, -40, 71, 179, 192];
const GAINS_FC0013: &[i16] = &[
    -99, -73, -65, -63, -60, -58, -54, 58, 61, 63, 65, 67, 68, 70, 71, 179, 181, 182, 184, 186, 188, 191, 197,
];
const GAINS_FC2580: &[i16] = &[0];
const GAINS_R820T: &[i16] = &[
    0, 9, 14, 27, 37, 77, 87, 125, 144, 157, 166, 197, 207, 229, 254, 280, 297, 328, 338, 364, 372, 386, 402, 421,
    434, 439, 445, 480, 496,
];

/// What the greeting says about the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Greeting {
    tuner: u32,
    /// True when the magic is "RTL0" and the tuner type is one of the RTL-SDR tuners.
    rtl: bool,
}

/// Reads the greeting. None when the bytes do not look like one.
fn parse_greeting(h: &[u8; GREETING_LEN]) -> Option<Greeting> {
    if h[..4].iter().any(|&c| !(0x20..=0x7e).contains(&c)) {
        return None;
    }
    let tuner = u32::from_be_bytes([h[4], h[5], h[6], h[7]]);
    let gains = u32::from_be_bytes([h[8], h[9], h[10], h[11]]);
    if tuner > 0xffff || gains > 0xffff {
        return None;
    }
    let rtl = &h[..4] == b"RTL0" && (TUNER_FIRST..=TUNER_LAST).contains(&tuner);
    Some(Greeting { tuner, rtl })
}

fn tuner_name(tuner: u32) -> &'static str {
    match tuner {
        1 => "E4000",
        2 => "FC0012",
        3 => "FC0013",
        4 => "FC2580",
        5 => "R820T",
        6 => "R828D",
        _ => "unknown tuner",
    }
}

fn tuner_gains(tuner: u32) -> &'static [i16] {
    match tuner {
        1 => GAINS_E4000,
        2 => GAINS_FC0012,
        3 => GAINS_FC0013,
        4 => GAINS_FC2580,
        _ => GAINS_R820T,
    }
}

/// Five byte command: command byte, then the value big endian.
fn command_bytes(cmd: u8, value: u32) -> [u8; 5] {
    let v = value.to_be_bytes();
    [cmd, v[0], v[1], v[2], v[3]]
}

/// Unsigned 8-bit sample to a float in about -1..1 (127.4 is the centre of the offset binary range).
fn u8_to_f32(b: u8) -> f32 {
    (b as f32 - 127.4) / 128.0
}

/// Splits "host:port" or "[v6-address]:port". The port must be 1 to 65535.
fn split_host_port(id: &str) -> Result<(String, u16), String> {
    let bad = || format!("{id:?} is not a host and port; write it as host:port, for example 192.168.1.5:1234");
    let (host, port) = if let Some(rest) = id.strip_prefix('[') {
        let end = rest.find(']').ok_or_else(bad)?;
        let port = rest[end + 1..].strip_prefix(':').ok_or_else(bad)?;
        (&rest[..end], port)
    } else {
        id.rsplit_once(':').ok_or_else(bad)?
    };
    if host.is_empty() {
        return Err(bad());
    }
    let port: u16 = port.parse().map_err(|_| bad())?;
    if port == 0 {
        return Err("the port must be 1 to 65535".into());
    }
    Ok((host.to_string(), port))
}

fn connect_error(id: &str, e: Option<io::Error>) -> String {
    match e {
        None => format!("could not find an address for {id}; check the host name"),
        Some(e) => match e.kind() {
            io::ErrorKind::ConnectionRefused => {
                format!("{id} refused the connection; check that the server is running and the port is right")
            }
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => {
                format!("no answer from {id} within {} s; check the address and the network", CONNECT_TIMEOUT.as_secs())
            }
            _ => format!("could not connect to {id}: {e}"),
        },
    }
}

fn is_timeout(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut)
}

/// Connects to the server named by `id` (host:port) and reads its greeting.
pub fn open(id: Option<&str>) -> Result<Box<dyn RxRadio>, String> {
    let id = id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "this radio needs its server address as host:port, for example 192.168.1.5:1234".to_string())?;
    let (host, port) = split_host_port(id)?;

    let mut last_err = None;
    let mut found = None;
    let addrs = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| format!("could not look up {host}: {e}"))?;
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(s) => {
                found = Some(s);
                break;
            }
            Err(e) => last_err = Some(e),
        }
    }
    let mut stream = found.ok_or_else(|| connect_error(id, last_err))?;

    stream
        .set_read_timeout(Some(GREETING_TIMEOUT))
        .map_err(|e| format!("could not set up {id}: {e}"))?;
    let mut hdr = [0u8; GREETING_LEN];
    if let Err(e) = stream.read_exact(&mut hdr) {
        return Err(if is_timeout(&e) {
            format!("{id} sent no greeting within {} s; another client may be connected to it", GREETING_TIMEOUT.as_secs())
        } else if e.kind() == io::ErrorKind::UnexpectedEof {
            format!("{id} closed the connection before its greeting; another client may be connected to it")
        } else {
            format!("could not read the greeting from {id}: {e}")
        });
    }
    let greeting = parse_greeting(&hdr).ok_or_else(|| format!("{id} is not a network radio server (the greeting did not match)"))?;

    stream.set_read_timeout(Some(READ_WAIT)).map_err(|e| format!("could not set up {id}: {e}"))?;
    stream
        .set_write_timeout(Some(GREETING_TIMEOUT))
        .map_err(|e| format!("could not set up {id}: {e}"))?;
    let _ = stream.set_nodelay(true);

    let rtl = greeting.rtl;
    Ok(Box::new(RtlTcpRadio {
        id: id.to_string(),
        stream,
        tuner: greeting.tuner,
        rtl,
        freq_hz: 100_000_000,
        rate_hz: 2_048_000,
        gain_auto: true,
        gain_tenths: 0,
        started: false,
        alive: true,
        buf: Vec::new(),
        carry: None,
    }))
}

/// Probing finds nothing: a network radio is named by the operator.
pub fn probe() -> Vec<DeviceInfo> {
    Vec::new()
}

struct RtlTcpRadio {
    id: String,
    stream: TcpStream,
    tuner: u32,
    /// True for an RTL-SDR tuner: fixed rate list and the tuner's gain list.
    rtl: bool,
    freq_hz: u64,
    rate_hz: u32,
    gain_auto: bool,
    /// Manual gain in tenths of a dB, already snapped to a value the tuner has.
    gain_tenths: i32,
    started: bool,
    alive: bool,
    /// Scratch space for reads.
    buf: Vec<u8>,
    /// One byte of an I/Q pair that arrived alone; it goes first in the next read.
    carry: Option<u8>,
}

impl RtlTcpRadio {
    fn freq_range(&self) -> (u64, u64) {
        if self.rtl {
            RTL_FREQ_RANGE
        } else {
            ANY_FREQ_RANGE
        }
    }

    fn gain_limits(&self) -> (f32, f32) {
        if self.rtl {
            let g = tuner_gains(self.tuner);
            let lo = g.iter().copied().min().unwrap_or(0);
            let hi = g.iter().copied().max().unwrap_or(0);
            (lo as f32 / 10.0, hi as f32 / 10.0)
        } else {
            ANY_GAIN_RANGE
        }
    }

    fn send(&mut self, cmd: u8, value: u32) -> Result<(), String> {
        self.stream
            .write_all(&command_bytes(cmd, value))
            .map_err(|e| format!("could not send to {}: {e}; the server may have stopped", self.id))
    }

    /// Sends the frequency, rate and gain to the server.
    fn send_all(&mut self) -> Result<(), String> {
        self.send(CMD_FREQ, self.freq_hz as u32)?;
        self.send(CMD_RATE, self.rate_hz)?;
        self.send_gain()
    }

    fn send_gain(&mut self) -> Result<(), String> {
        if self.gain_auto {
            self.send(CMD_GAIN_MODE, 0)?;
            self.send(CMD_AGC, 1)
        } else {
            self.send(CMD_GAIN_MODE, 1)?;
            self.send(CMD_GAIN, self.gain_tenths as u32)?;
            self.send(CMD_AGC, 0)
        }
    }

    /// Clamps a gain to the stage range and, for an RTL-SDR tuner, picks the nearest value the tuner has.
    fn snap_gain(&self, db: f32) -> Result<i32, String> {
        if !db.is_finite() {
            return Err(format!("gain {db} is not a number"));
        }
        let (lo, hi) = self.gain_limits();
        let want = (db.clamp(lo, hi) * 10.0).round() as i32;
        if self.rtl {
            let nearest = tuner_gains(self.tuner)
                .iter()
                .copied()
                .min_by_key(|&g| (g as i32 - want).abs())
                .map(i32::from);
            Ok(nearest.unwrap_or(want))
        } else {
            Ok(want)
        }
    }
}

impl RxRadio for RtlTcpRadio {
    fn label(&self) -> String {
        if self.rtl {
            format!("{} at {}", tuner_name(self.tuner), self.id)
        } else {
            format!("network radio at {}", self.id)
        }
    }

    fn freq_range_hz(&self) -> (u64, u64) {
        self.freq_range()
    }

    fn set_frequency_hz(&mut self, hz: u64) -> Result<(), String> {
        let (lo, hi) = self.freq_range();
        if hz < lo || hz > hi {
            return Err(format!("{hz} Hz is outside {lo} to {hi} Hz for this radio"));
        }
        self.freq_hz = hz;
        if self.started {
            self.send(CMD_FREQ, hz as u32)?;
        }
        Ok(())
    }

    fn sample_rates(&self) -> Vec<u32> {
        if self.rtl {
            RTL_RATES.to_vec()
        } else {
            ANY_RATES.to_vec()
        }
    }

    fn set_sample_rate(&mut self, hz: u32) -> Result<u32, String> {
        let rate = self
            .sample_rates()
            .into_iter()
            .min_by_key(|&r| (r as i64 - hz as i64).abs())
            .ok_or_else(|| "this radio offers no sample rates".to_string())?;
        self.rate_hz = rate;
        if self.started {
            self.send(CMD_RATE, rate)?;
        }
        Ok(rate)
    }

    fn gain_stages(&self) -> Vec<GainStage> {
        let (min_db, max_db) = self.gain_limits();
        vec![GainStage { name: "gain".into(), min_db, max_db, step_db: 0.0, auto: true }]
    }

    fn set_gain(&mut self, stage: &str, setting: GainSetting) -> Result<(), String> {
        if stage != "gain" {
            return Err(format!("no gain stage named {stage:?}; this radio has one stage, \"gain\""));
        }
        match setting {
            GainSetting::Auto => self.gain_auto = true,
            GainSetting::Manual(db) => {
                self.gain_tenths = self.snap_gain(db)?;
                self.gain_auto = false;
            }
        }
        if self.started {
            self.send_gain()?;
        }
        Ok(())
    }

    fn start(&mut self) -> Result<(), String> {
        if self.started {
            return Ok(());
        }
        if !self.alive {
            return Err(format!("the connection to {} is closed; open the radio again", self.id));
        }
        self.send_all()?;
        self.started = true;
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
        let want = out.len() * 2;
        if self.buf.len() != want {
            self.buf.resize(want, 0);
        }
        let mut start = 0;
        if let Some(c) = self.carry.take() {
            self.buf[0] = c;
            start = 1;
        }
        let got = match self.stream.read(&mut self.buf[start..]) {
            Ok(0) => {
                self.alive = false;
                return Err(format!("the server at {} closed the connection", self.id));
            }
            Ok(n) => n,
            Err(e) if is_timeout(&e) || e.kind() == io::ErrorKind::Interrupted => 0,
            Err(e) => {
                self.alive = false;
                return Err(format!("lost the connection to {}: {e}", self.id));
            }
        };
        let mut total = start + got;
        if total % 2 == 1 {
            total -= 1;
            self.carry = Some(self.buf[total]);
        }
        let pairs = total / 2;
        for (i, slot) in out.iter_mut().take(pairs).enumerate() {
            *slot = (u8_to_f32(self.buf[2 * i]), u8_to_f32(self.buf[2 * i + 1]));
        }
        Ok(pairs)
    }

    fn stop(&mut self) {
        if self.alive {
            let _ = self.stream.shutdown(Shutdown::Both);
        }
        self.alive = false;
        self.started = false;
        self.carry = None;
    }
}

impl Drop for RtlTcpRadio {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Instant;

    /// Greeting bytes: magic, tuner type, gain count.
    fn greeting(magic: &[u8; 4], tuner: u32, gains: u32) -> [u8; 12] {
        let mut h = [0u8; 12];
        h[..4].copy_from_slice(magic);
        h[4..8].copy_from_slice(&tuner.to_be_bytes());
        h[8..].copy_from_slice(&gains.to_be_bytes());
        h
    }

    /// Starts a server on 127.0.0.1 that sends `greet`, then runs `script` on the connection.
    fn fake_server<F>(greet: [u8; 12], script: F) -> (String, thread::JoinHandle<()>)
    where
        F: FnOnce(TcpStream) + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let port = listener.local_addr().expect("test server address").port();
        let handle = thread::spawn(move || {
            let (mut s, _) = listener.accept().expect("accept");
            s.write_all(&greet).expect("send greeting");
            script(s);
        });
        (format!("127.0.0.1:{port}"), handle)
    }

    fn open_ok(id: &str) -> Box<dyn RxRadio> {
        match open(Some(id)) {
            Ok(r) => r,
            Err(e) => panic!("open failed: {e}"),
        }
    }

    #[test]
    fn greeting_is_read() {
        let g = parse_greeting(&greeting(b"RTL0", 5, 29)).expect("greeting");
        assert_eq!(g.tuner, 5);
        assert!(g.rtl);
        let other = parse_greeting(&greeting(b"ABCD", 5, 29)).expect("greeting");
        assert!(!other.rtl);
        let odd_tuner = parse_greeting(&greeting(b"RTL0", 9, 1)).expect("greeting");
        assert!(!odd_tuner.rtl);
        assert!(parse_greeting(&greeting(b"R\x01L0", 5, 1)).is_none());
        assert!(parse_greeting(&greeting(b"RTL0", 0x1_0000, 1)).is_none());
    }

    #[test]
    fn command_bytes_are_big_endian() {
        assert_eq!(command_bytes(CMD_FREQ, 100_000_000), [0x01, 0x05, 0xF5, 0xE1, 0x00]);
        assert_eq!(command_bytes(CMD_RATE, 2_048_000), [0x02, 0x00, 0x1F, 0x40, 0x00]);
        assert_eq!(command_bytes(CMD_AGC, 1), [0x08, 0, 0, 0, 1]);
        assert_eq!(command_bytes(CMD_GAIN, -99i32 as u32), [0x04, 0xFF, 0xFF, 0xFF, 0x9D]);
    }

    #[test]
    fn samples_convert_to_about_minus_one_to_one() {
        assert!(u8_to_f32(0) < -0.99 && u8_to_f32(0) >= -1.0);
        assert!(u8_to_f32(255) > 0.99 && u8_to_f32(255) <= 1.0);
        assert!(u8_to_f32(127).abs() < 0.01);
    }

    #[test]
    fn host_and_port_are_split() {
        assert_eq!(split_host_port("192.168.1.5:1234").unwrap(), ("192.168.1.5".into(), 1234));
        assert_eq!(split_host_port("[::1]:1234").unwrap(), ("::1".into(), 1234));
        assert!(split_host_port("192.168.1.5").is_err());
        assert!(split_host_port("192.168.1.5:0").is_err());
        assert!(split_host_port(":1234").is_err());
        assert!(open(None).is_err());
        assert!(open(Some("   ")).is_err());
    }

    #[test]
    fn refused_connection_gives_clear_error() {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        match open(Some(&format!("127.0.0.1:{port}"))) {
            Ok(_) => panic!("open should fail"),
            Err(e) => assert!(e.contains("refused"), "message was: {e}"),
        }
    }

    #[test]
    fn silent_server_gives_a_wait_of_at_most_about_200_ms() {
        let (id, handle) = fake_server(greeting(b"RTL0", 5, 29), |s| {
            thread::sleep(Duration::from_millis(800));
            drop(s);
        });
        let mut r = open_ok(&id);
        r.start().unwrap();
        let mut out = vec![(0.0f32, 0.0f32); 64];
        let t = Instant::now();
        let n = r.read_iq(&mut out).unwrap();
        let waited = t.elapsed();
        assert_eq!(n, 0);
        assert!(waited < Duration::from_millis(600), "waited {waited:?}");
        handle.join().unwrap();
    }

    #[test]
    fn stream_sends_commands_and_converts_samples() {
        let (cmd_tx, cmd_rx) = mpsc::channel::<Vec<u8>>();
        let (id, handle) = fake_server(greeting(b"RTL0", 5, 29), move |mut s| {
            let mut cmds = [0u8; 20];
            s.read_exact(&mut cmds).expect("read commands");
            cmd_tx.send(cmds.to_vec()).unwrap();
            // 8 pairs: byte 0 and 255 alternate
            s.write_all(&[0, 255, 255, 0, 0, 0, 255, 255, 127, 127, 0, 255, 255, 0, 127, 0]).unwrap();
            thread::sleep(Duration::from_millis(300));
        });
        let mut r = open_ok(&id);
        assert_eq!(r.set_sample_rate(2_000_000).unwrap(), 2_048_000);
        r.set_frequency_hz(100_000_000).unwrap();
        r.start().unwrap();

        let sent = cmd_rx.recv().unwrap();
        let mut expected = Vec::new();
        expected.extend_from_slice(&command_bytes(CMD_FREQ, 100_000_000));
        expected.extend_from_slice(&command_bytes(CMD_RATE, 2_048_000));
        expected.extend_from_slice(&command_bytes(CMD_GAIN_MODE, 0));
        expected.extend_from_slice(&command_bytes(CMD_AGC, 1));
        assert_eq!(sent, expected);

        let mut out = vec![(0.0f32, 0.0f32); 8];
        let mut total = 0;
        while total < 8 {
            total += r.read_iq(&mut out[total..]).unwrap();
        }
        assert!(out[0].0 < -0.99 && out[0].1 > 0.99);
        assert!(out[1].0 > 0.99 && out[1].1 < -0.99);
        assert!(out[2].0 < -0.99 && out[2].1 < -0.99);
        assert!(out[3].0 > 0.99 && out[3].1 > 0.99);
        r.stop();
        handle.join().unwrap();
    }

    #[test]
    fn odd_byte_is_kept_for_the_next_read() {
        let (cmd_tx, cmd_rx) = mpsc::channel::<()>();
        let (id, handle) = fake_server(greeting(b"RTL0", 5, 29), move |mut s| {
            let mut cmds = [0u8; 20];
            s.read_exact(&mut cmds).unwrap();
            s.write_all(&[10, 20, 30]).unwrap();
            cmd_rx.recv().unwrap();
            s.write_all(&[40, 50, 60]).unwrap();
            thread::sleep(Duration::from_millis(300));
        });
        let mut r = open_ok(&id);
        r.start().unwrap();
        let mut out = vec![(0.0f32, 0.0f32); 4];
        let mut first = 0;
        while first == 0 {
            first = r.read_iq(&mut out).unwrap();
        }
        assert_eq!(first, 1);
        cmd_tx.send(()).unwrap();
        let mut second = 0;
        while second == 0 {
            second = r.read_iq(&mut out).unwrap();
        }
        // carry byte 30 plus 40 makes the second pair, then 50 60 the third; the read gives what fits
        assert!(second >= 1);
        assert!((out[0].0 - u8_to_f32(30)).abs() < 1e-6);
        assert!((out[0].1 - u8_to_f32(40)).abs() < 1e-6);
        r.stop();
        handle.join().unwrap();
    }

    #[test]
    fn closed_connection_is_an_error() {
        let (id, handle) = fake_server(greeting(b"RTL0", 5, 29), |mut s| {
            let mut cmds = [0u8; 20];
            s.read_exact(&mut cmds).unwrap();
            drop(s);
        });
        let mut r = open_ok(&id);
        r.start().unwrap();
        let mut out = vec![(0.0f32, 0.0f32); 4];
        let mut err = None;
        for _ in 0..20 {
            if let Err(e) = r.read_iq(&mut out) {
                err = Some(e);
                break;
            }
        }
        assert!(err.is_some(), "a closed connection must be reported");
        handle.join().unwrap();
    }

    #[test]
    fn frequency_range_and_gain_stage_names_are_checked() {
        let (id, handle) = fake_server(greeting(b"RTL0", 5, 29), |s| {
            thread::sleep(Duration::from_millis(200));
            drop(s);
        });
        let mut r = open_ok(&id);
        assert!(r.set_frequency_hz(10_000).is_err());
        assert!(r.set_frequency_hz(144_000_000).is_ok());
        assert!(r.set_gain("lna", GainSetting::Auto).is_err());
        let stages = r.gain_stages();
        assert_eq!(stages.len(), 1);
        assert_eq!(stages[0].name, "gain");
        assert!(stages[0].auto);
        assert!((stages[0].min_db - 0.0).abs() < 1e-6 && (stages[0].max_db - 49.6).abs() < 1e-6);
        handle.join().unwrap();
    }

    #[test]
    fn manual_gain_snaps_to_the_tuner_list() {
        let (cmd_tx, cmd_rx) = mpsc::channel::<Vec<u8>>();
        let (id, handle) = fake_server(greeting(b"RTL0", 5, 29), move |mut s| {
            let mut cmds = [0u8; 25];
            s.read_exact(&mut cmds).unwrap();
            cmd_tx.send(cmds.to_vec()).unwrap();
            thread::sleep(Duration::from_millis(200));
        });
        let mut r = open_ok(&id);
        r.set_gain("gain", GainSetting::Manual(12.0)).unwrap();
        let stages = r.gain_stages();
        assert!((stages[0].max_db - 49.6).abs() < 1e-4);
        r.start().unwrap();
        let sent = cmd_rx.recv().unwrap();
        // gain 12.0 dB is 120 tenths; the nearest R820T value is 125
        assert_eq!(&sent[10..15], &command_bytes(CMD_GAIN_MODE, 1));
        assert_eq!(&sent[15..20], &command_bytes(CMD_GAIN, 125));
        assert_eq!(&sent[20..25], &command_bytes(CMD_AGC, 0));
        r.stop();
        handle.join().unwrap();
    }

    #[test]
    fn non_rtl_server_offers_any_rate_and_plain_gain() {
        let (id, handle) = fake_server(greeting(b"ABCD", 9, 0), |s| {
            thread::sleep(Duration::from_millis(100));
            drop(s);
        });
        let mut r = open_ok(&id);
        assert_eq!(r.set_sample_rate(8_000_000).unwrap(), 8_000_000);
        assert_eq!(r.freq_range_hz().0, 1_000_000);
        assert!(r.label().contains("network radio"));
        handle.join().unwrap();
    }
}
