//! Driver for Icom receivers (IC-R8600 and similar) over the network.
//!
//! One TCP connection to host:port carries everything. Commands go out as CI-V frames
//! (fe fe 96 e0 ... fd). The radio answers each one with an ack frame (fe fe e0 96 fb fd)
//! or a refusal frame (fe fe e0 96 fa fd). I/Q comes back as 16-bit little-endian words.
//! The word 0x8000 is a marker: it is dropped, and the words after it form I/Q pairs.
//!
//! Not verified on hardware. The known I/Q path is a USB bulk pipe, with commands on
//! other pipes. This driver assumes the network port carries the same bytes in one
//! stream. No receiver was used to check that. USB radios (a serial number) are not
//! written here.
//!
//! The rfgain stage is in radio units (0 to 255), not dB. Its name says so.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::{DeviceInfo, GainSetting, GainStage, RxRadio};

const LABEL: &str = "Icom receiver";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Longest wait inside read_iq when no samples arrive.
const READ_WAIT: Duration = Duration::from_millis(200);
/// How long one command waits for its answer.
const ANSWER_WAIT: Duration = Duration::from_secs(1);
/// Silent waits allowed for one answer before giving up.
const ANSWER_TRIES: usize = 4;
/// Reads allowed for one answer, so a busy stream cannot hold a command forever.
const MAX_ANSWER_READS: usize = 256;
const READ_CHUNK: usize = 16 * 1024;
/// Most samples kept while the reader is late. Older ones are dropped and counted.
const MAX_PENDING: usize = 1 << 20;
/// Scale from 16-bit samples to -1..1.
const SAMPLE_SCALE: f32 = 32768.0;

const FREQ_RANGE: (u64, u64) = (10_000, 3_000_000_000);
/// Rates the radio offers, in ascending order.
const RATES: [u32; 6] = [240_000, 480_000, 960_000, 1_920_000, 3_840_000, 5_120_000];
const DEFAULT_RATE_INDEX: usize = 2;

const CIV_HEAD: [u8; 4] = [0xfe, 0xfe, 0x96, 0xe0];
const CIV_TAIL: u8 = 0xfd;

const ENTER_STREAM: [&[u8]; 2] = [&[0x1a, 0x13, 0x01, 0x00], &[0x1a, 0x13, 0x00, 0x01]];
const LEAVE_STREAM: [&[u8]; 2] = [&[0x1a, 0x13, 0x01, 0x00], &[0x1a, 0x13, 0x00, 0x00]];

/// Reports nothing. Network radios are opened by name, and USB radios are not written yet.
pub fn probe() -> Vec<DeviceInfo> {
    Vec::new()
}

/// Opens the receiver at `host:port`. A bare serial number is refused with a clear message.
pub fn open(id: Option<&str>) -> Result<Box<dyn RxRadio>, String> {
    let id = id.map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| {
        "give the receiver's network address as host:port, for example 192.168.1.5:1234".to_string()
    })?;
    if !id.contains(':') {
        return Err(format!(
            "{id:?} looks like a serial number. USB receivers are not supported yet; give the network address as host:port"
        ));
    }
    let (host, port) = split_host_port(id)?;
    let stream = connect(id, &host, port)?;
    Ok(Box::new(IcomNet {
        addr: id.to_string(),
        stream,
        decoder: IqDecoder::default(),
        pending: VecDeque::new(),
        dropped: 0,
        rate_index: DEFAULT_RATE_INDEX,
        freq_hz: None,
        started: false,
    }))
}

struct IcomNet {
    addr: String,
    stream: TcpStream,
    decoder: IqDecoder,
    pending: VecDeque<(f32, f32)>,
    dropped: u64,
    rate_index: usize,
    freq_hz: Option<u64>,
    started: bool,
}

impl RxRadio for IcomNet {
    fn label(&self) -> String {
        format!("{LABEL} at {}", self.addr)
    }

    fn freq_range_hz(&self) -> (u64, u64) {
        FREQ_RANGE
    }

    fn set_frequency_hz(&mut self, hz: u64) -> Result<(), String> {
        if hz < FREQ_RANGE.0 || hz > FREQ_RANGE.1 {
            return Err(format!(
                "{hz} Hz is outside the receiver range of {} Hz to {} Hz",
                FREQ_RANGE.0, FREQ_RANGE.1
            ));
        }
        self.send_civ(&civ_tune(hz))?;
        self.freq_hz = Some(hz);
        Ok(())
    }

    fn sample_rates(&self) -> Vec<u32> {
        RATES.to_vec()
    }

    fn set_sample_rate(&mut self, hz: u32) -> Result<u32, String> {
        let index = rate_index_for(hz);
        if self.started {
            self.send_civ(&set_rate_payload(index))?;
        }
        self.rate_index = index;
        Ok(RATES[index])
    }

    fn gain_stages(&self) -> Vec<GainStage> {
        vec![
            GainStage {
                name: "rfgain".into(),
                min_db: 0.0,
                max_db: 255.0,
                step_db: 1.0,
                auto: false,
            },
            GainStage {
                name: "att".into(),
                min_db: 0.0,
                max_db: 30.0,
                step_db: 10.0,
                auto: false,
            },
            GainStage {
                name: "preamp".into(),
                min_db: 0.0,
                max_db: 1.0,
                step_db: 1.0,
                auto: false,
            },
        ]
    }

    fn set_gain(&mut self, stage: &str, setting: GainSetting) -> Result<(), String> {
        let value = match setting {
            GainSetting::Manual(v) => v,
            GainSetting::Auto => return Err(format!("{stage} has no automatic setting")),
        };
        let payload = match stage {
            "rfgain" => rfgain_payload(value.round().clamp(0.0, 255.0) as u32),
            "att" => {
                let steps = (value / 10.0).round().clamp(0.0, 3.0) as u32;
                let db = steps * 10;
                vec![0x11, ((db / 10) << 4) as u8]
            }
            "preamp" => vec![0x16, 0x02, if value >= 0.5 { 1 } else { 0 }],
            _ => {
                return Err(format!(
                    "{LABEL} has no gain stage named {stage:?}; use rfgain, att or preamp"
                ))
            }
        };
        self.send_civ(&payload)
    }

    fn start(&mut self) -> Result<(), String> {
        if self.started {
            return Ok(());
        }
        self.decoder = IqDecoder::default();
        self.pending.clear();
        for p in ENTER_STREAM {
            self.send_civ(p)?;
        }
        self.send_civ(&set_rate_payload(self.rate_index))?;
        if let Some(hz) = self.freq_hz {
            self.send_civ(&civ_tune(hz))?;
        }
        self.started = true;
        Ok(())
    }

    fn read_iq(&mut self, out: &mut [(f32, f32)]) -> Result<usize, String> {
        if !self.started {
            return Err(format!("{LABEL}: not streaming; call start first"));
        }
        if out.is_empty() {
            return Ok(0);
        }
        if self.pending.is_empty() {
            let mut buf = [0u8; READ_CHUNK];
            match self.stream.read(&mut buf) {
                Ok(0) => {
                    self.started = false;
                    return Err(format!("{LABEL}: the connection to {} closed", self.addr));
                }
                Ok(n) => self.push_samples(&buf[..n]),
                Err(e) if is_timeout(&e) => {}
                Err(e) => {
                    self.started = false;
                    return Err(format!("{LABEL}: lost the connection to {} ({e})", self.addr));
                }
            }
        }
        let n = out.len().min(self.pending.len());
        for (slot, s) in out[..n].iter_mut().zip(self.pending.drain(..n)) {
            *slot = s;
        }
        Ok(n)
    }

    fn stop(&mut self) {
        if !self.started {
            return;
        }
        for p in LEAVE_STREAM {
            let _ = self.stream.write_all(&civ_frame(p));
        }
        self.started = false;
        self.pending.clear();
    }

    fn dropped_samples(&self) -> u64 {
        self.dropped
    }
}

impl Drop for IcomNet {
    fn drop(&mut self) {
        self.stop();
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

impl IcomNet {
    /// Sends one command and waits for the radio's answer.
    fn send_civ(&mut self, payload: &[u8]) -> Result<(), String> {
        let frame = civ_frame(payload);
        self.stream
            .write_all(&frame)
            .map_err(|e| format!("{LABEL}: a CI-V command was not sent ({e})"))?;
        self.stream
            .set_read_timeout(Some(ANSWER_WAIT))
            .map_err(|e| format!("{LABEL}: cannot set the wait time ({e})"))?;
        let result = self.wait_answer();
        let restore = self
            .stream
            .set_read_timeout(Some(READ_WAIT))
            .map_err(|e| format!("{LABEL}: cannot set the wait time ({e})"));
        result.and(restore)
    }

    /// Reads until an ack or refusal frame arrives. I/Q bytes after the answer are kept.
    fn wait_answer(&mut self) -> Result<(), String> {
        let mut seen: Vec<u8> = Vec::new();
        let mut buf = [0u8; 512];
        let mut silent = 0;
        let mut reads = 0;
        while silent < ANSWER_TRIES && reads < MAX_ANSWER_READS {
            reads += 1;
            let n = match self.stream.read(&mut buf) {
                Ok(0) => {
                    return Err(format!(
                        "{LABEL}: the connection closed while waiting for a CI-V answer"
                    ))
                }
                Ok(n) => n,
                Err(e) if is_timeout(&e) => {
                    silent += 1;
                    continue;
                }
                Err(e) => return Err(format!("{LABEL}: no CI-V answer ({e})")),
            };
            seen.extend_from_slice(&buf[..n]);
            if let Some((ack, end)) = find_answer(&seen) {
                self.push_samples(&seen[end..]);
                return if ack {
                    Ok(())
                } else {
                    Err(format!("{LABEL}: the radio refused a CI-V command"))
                };
            }
        }
        Err(format!("{LABEL}: the radio did not acknowledge a CI-V command"))
    }

    /// Decodes received bytes into I/Q samples. Drops the oldest when the reader is late.
    fn push_samples(&mut self, bytes: &[u8]) {
        let mut pairs = Vec::new();
        self.decoder.feed(bytes, &mut pairs);
        for (i, q) in pairs {
            if self.pending.len() >= MAX_PENDING {
                self.pending.pop_front();
                self.dropped += 1;
            }
            self.pending
                .push_back((i as f32 / SAMPLE_SCALE, q as f32 / SAMPLE_SCALE));
        }
    }
}

/// Turns 16-bit words into I/Q pairs. Words before the first marker are dropped, and a
/// pair cut by a marker is dropped too.
#[derive(Default)]
struct IqDecoder {
    byte_carry: Option<u8>,
    marker: bool,
    locked: bool,
    pending_i: Option<i16>,
}

impl IqDecoder {
    fn feed(&mut self, bytes: &[u8], out: &mut Vec<(i16, i16)>) {
        let mut rest = bytes;
        if let Some(lo) = self.byte_carry.take() {
            match rest.split_first() {
                Some((&hi, tail)) => {
                    self.word(i16::from_le_bytes([lo, hi]), out);
                    rest = tail;
                }
                None => {
                    self.byte_carry = Some(lo);
                    return;
                }
            }
        }
        let (pairs, tail) = rest.as_chunks::<2>();
        for w in pairs {
            self.word(i16::from_le_bytes(*w), out);
        }
        if let [b] = tail {
            self.byte_carry = Some(*b);
        }
    }

    fn word(&mut self, v: i16, out: &mut Vec<(i16, i16)>) {
        if v == i16::MIN {
            self.marker = true;
            return;
        }
        if self.marker {
            self.marker = false;
            self.locked = true;
            self.pending_i = None;
        }
        if !self.locked {
            return;
        }
        match self.pending_i.take() {
            None => self.pending_i = Some(v),
            Some(i) => out.push((i, v)),
        }
    }
}

/// Finds an ack or refusal frame. Returns (true for ack, index just after the frame).
fn find_answer(seen: &[u8]) -> Option<(bool, usize)> {
    for k in 0..seen.len().saturating_sub(5) {
        if seen[k] == 0xfe
            && seen[k + 1] == 0xfe
            && seen[k + 2] == 0xe0
            && seen[k + 3] == 0x96
            && seen[k + 5] == CIV_TAIL
        {
            match seen[k + 4] {
                0xfb => return Some((true, k + 6)),
                0xfa => return Some((false, k + 6)),
                _ => {}
            }
        }
    }
    None
}

/// Packs a frequency as five BCD bytes, least significant pair first, after the 0x05 sub-command.
fn civ_tune(hz: u64) -> Vec<u8> {
    let mut f = hz;
    let mut out = vec![0x05];
    for _ in 0..5 {
        let lo = (f % 10) as u8;
        f /= 10;
        let hi = (f % 10) as u8;
        f /= 10;
        out.push(lo | (hi << 4));
    }
    out
}

fn civ_frame(payload: &[u8]) -> Vec<u8> {
    let mut f = CIV_HEAD.to_vec();
    f.extend_from_slice(payload);
    f.push(CIV_TAIL);
    f
}

/// Index into RATES for the rate closest to `hz` that the radio offers, rounding up.
fn rate_index_for(hz: u32) -> usize {
    let wanted = hz as f64 * 0.999;
    RATES
        .iter()
        .position(|&r| r as f64 >= wanted)
        .unwrap_or(RATES.len() - 1)
}

fn set_rate_payload(index: usize) -> Vec<u8> {
    vec![0x1a, 0x13, 0x01, 0x01, 0x00, (6 - index) as u8]
}

/// RF gain 0 to 255 in radio units, as two BCD bytes after the 0x14 0x02 sub-command.
fn rfgain_payload(n: u32) -> Vec<u8> {
    vec![
        0x14,
        0x02,
        (n / 100) as u8,
        ((((n % 100) / 10) << 4) | (n % 10)) as u8,
    ]
}

/// Splits "host:port" or "[v6-address]:port". The port must be 1 to 65535.
fn split_host_port(id: &str) -> Result<(String, u16), String> {
    let bad =
        || format!("{id:?} is not a host and port; write it as host:port, for example 192.168.1.5:1234");
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

fn connect(id: &str, host: &str, port: u16) -> Result<TcpStream, String> {
    let addrs = (host, port)
        .to_socket_addrs()
        .map_err(|e| connect_error(id, Some(e)))?;
    let mut last = None;
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(s) => {
                let _ = s.set_nodelay(true);
                s.set_read_timeout(Some(READ_WAIT))
                    .and_then(|_| s.set_write_timeout(Some(ANSWER_WAIT)))
                    .map_err(|e| format!("could not set up {id}: {e}"))?;
                return Ok(s);
            }
            Err(e) => last = Some(e),
        }
    }
    Err(connect_error(id, last))
}

fn connect_error(id: &str, e: Option<io::Error>) -> String {
    match e {
        None => format!("could not find an address for {id}; check the host name"),
        Some(e) => match e.kind() {
            io::ErrorKind::ConnectionRefused => {
                format!("{id} refused the connection; check that the receiver is on its network and the port is right")
            }
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => {
                format!(
                    "no answer from {id} within {} s; check the address and the network",
                    CONNECT_TIMEOUT.as_secs()
                )
            }
            _ => format!("could not connect to {id}: {e}"),
        },
    }
}

fn is_timeout(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;
    use std::time::Instant;

    const ACK: [u8; 6] = [0xfe, 0xfe, 0xe0, 0x96, 0xfb, 0xfd];
    const NAK: [u8; 6] = [0xfe, 0xfe, 0xe0, 0x96, 0xfa, 0xfd];

    /// Starts a fake radio on 127.0.0.1. `script` gets the accepted connection.
    fn fake_peer<F>(script: F) -> (String, thread::JoinHandle<()>)
    where
        F: FnOnce(TcpStream) + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let h = thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            script(s);
        });
        (addr, h)
    }

    /// Reads one CI-V frame (up to and including fd).
    fn read_frame(s: &mut TcpStream) -> Vec<u8> {
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut f = Vec::new();
        let mut b = [0u8; 1];
        loop {
            s.read_exact(&mut b).unwrap();
            f.push(b[0]);
            if b[0] == CIV_TAIL && f.len() > 4 {
                return f;
            }
        }
    }

    fn err_of(r: Result<Box<dyn RxRadio>, String>) -> String {
        match r {
            Ok(_) => panic!("expected an error"),
            Err(e) => e,
        }
    }

    fn le(v: i16) -> [u8; 2] {
        v.to_le_bytes()
    }

    #[test]
    fn tune_bcd_and_frame_bytes() {
        assert_eq!(civ_tune(14_074_000), vec![0x05, 0x00, 0x40, 0x07, 0x14, 0x00]);
        assert_eq!(
            civ_frame(&civ_tune(14_074_000)),
            vec![0xfe, 0xfe, 0x96, 0xe0, 0x05, 0x00, 0x40, 0x07, 0x14, 0x00, 0xfd]
        );
    }

    #[test]
    fn rate_codes_and_rounding() {
        assert_eq!(set_rate_payload(0), vec![0x1a, 0x13, 0x01, 0x01, 0x00, 6]);
        assert_eq!(set_rate_payload(5), vec![0x1a, 0x13, 0x01, 0x01, 0x00, 1]);
        assert_eq!(rate_index_for(1), 0);
        assert_eq!(rate_index_for(1_000_000), 3);
        assert_eq!(rate_index_for(u32::MAX), 5);
    }

    #[test]
    fn rf_gain_and_attenuator_bytes() {
        assert_eq!(rfgain_payload(255), vec![0x14, 0x02, 0x02, 0x55]);
        assert_eq!(rfgain_payload(7), vec![0x14, 0x02, 0x00, 0x07]);
    }

    #[test]
    fn answer_detection() {
        assert_eq!(find_answer(&ACK), Some((true, 6)));
        assert_eq!(find_answer(&NAK), Some((false, 6)));
        assert_eq!(find_answer(&[0xfe, 0xfe]), None);
    }

    #[test]
    fn iq_decoder_drops_marker_and_cut_pairs() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&le(5));
        bytes.extend_from_slice(&le(6));
        bytes.extend_from_slice(&le(i16::MIN));
        bytes.extend_from_slice(&le(100));
        bytes.extend_from_slice(&le(-100));
        bytes.extend_from_slice(&le(2));
        bytes.extend_from_slice(&le(3));
        let mut whole = IqDecoder::default();
        let mut out = Vec::new();
        whole.feed(&bytes, &mut out);
        assert_eq!(out, vec![(100, -100), (2, 3)]);

        // The same bytes one at a time must give the same pairs.
        let mut split = IqDecoder::default();
        let mut out2 = Vec::new();
        for b in &bytes {
            split.feed(std::slice::from_ref(b), &mut out2);
        }
        assert_eq!(out2, out);
    }

    #[test]
    fn host_port_parsing() {
        assert_eq!(
            split_host_port("192.168.1.5:1234").unwrap(),
            ("192.168.1.5".to_string(), 1234)
        );
        assert_eq!(split_host_port("[::1]:1234").unwrap(), ("::1".to_string(), 1234));
        assert!(split_host_port("192.168.1.5").is_err());
        assert!(split_host_port("host:0").is_err());
        assert!(split_host_port(":1234").is_err());
    }

    #[test]
    fn refused_connection_gives_clear_error() {
        let port = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let e = err_of(open(Some(&format!("127.0.0.1:{port}"))));
        assert!(e.contains("refused"), "message was: {e}");
    }

    #[test]
    fn serial_number_is_refused_with_usb_message() {
        let e = err_of(open(Some("ABC123")));
        assert!(e.contains("USB"), "message was: {e}");
        let e = err_of(open(None));
        assert!(e.contains("host:port"), "message was: {e}");
    }

    #[test]
    fn frequency_outside_range_is_refused_without_sending() {
        let (addr, h) = fake_peer(|_s| {});
        let mut r = open(Some(&addr)).unwrap();
        let e = r.set_frequency_hz(1).unwrap_err();
        assert!(e.contains("outside"), "message was: {e}");
        h.join().unwrap();
    }

    #[test]
    fn commands_and_streaming_end_to_end() {
        let (addr, h) = fake_peer(|mut s| {
            let expected: Vec<Vec<u8>> = vec![
                civ_frame(&civ_tune(14_074_000)),
                civ_frame(&[0x11, 0x20]),
                civ_frame(&[0x1a, 0x13, 0x01, 0x00]),
                civ_frame(&[0x1a, 0x13, 0x00, 0x01]),
                civ_frame(&[0x1a, 0x13, 0x01, 0x01, 0x00, 0x04]),
                civ_frame(&civ_tune(14_074_000)),
            ];
            for want in &expected {
                let got = read_frame(&mut s);
                assert_eq!(&got, want);
                s.write_all(&ACK).unwrap();
            }
            // I/Q right after the last ack: a marker, then one pair (100, -100) and one pair (2, 3).
            let mut iq = Vec::new();
            for v in [i16::MIN, 100, -100, 2, 3] {
                iq.extend_from_slice(&le(v));
            }
            s.write_all(&iq).unwrap();
            thread::sleep(Duration::from_secs(1));
        });
        let mut r = open(Some(&addr)).unwrap();
        r.set_frequency_hz(14_074_000).unwrap();
        r.set_gain("att", GainSetting::Manual(20.0)).unwrap();
        r.start().unwrap();
        let mut out = [(0.0f32, 0.0f32); 4];
        let mut n = 0;
        for _ in 0..10 {
            n = r.read_iq(&mut out).unwrap();
            if n > 0 {
                break;
            }
        }
        assert_eq!(n, 2);
        assert!((out[0].0 - 100.0 / 32768.0).abs() < 1e-6);
        assert!((out[0].1 + 100.0 / 32768.0).abs() < 1e-6);
        assert!((out[1].0 - 2.0 / 32768.0).abs() < 1e-6);
        assert!((out[1].1 - 3.0 / 32768.0).abs() < 1e-6);
        r.stop();
        h.join().unwrap();
    }

    #[test]
    fn read_iq_returns_zero_within_wait_when_quiet() {
        let (addr, h) = fake_peer(|mut s| {
            for _ in 0..3 {
                let _ = read_frame(&mut s);
                s.write_all(&ACK).unwrap();
            }
            thread::sleep(Duration::from_secs(2));
        });
        let mut r = open(Some(&addr)).unwrap();
        r.start().unwrap();
        let mut out = [(0.0f32, 0.0f32); 8];
        let t = Instant::now();
        let n = r.read_iq(&mut out).unwrap();
        assert_eq!(n, 0);
        assert!(
            t.elapsed() < Duration::from_millis(900),
            "waited {:?}",
            t.elapsed()
        );
        h.join().unwrap();
    }

    #[test]
    fn refused_command_is_reported() {
        let (addr, h) = fake_peer(|mut s| {
            let _ = read_frame(&mut s);
            s.write_all(&NAK).unwrap();
            thread::sleep(Duration::from_millis(300));
        });
        let mut r = open(Some(&addr)).unwrap();
        let e = r.start().unwrap_err();
        assert!(e.contains("refused"), "message was: {e}");
        h.join().unwrap();
    }

    #[test]
    fn unknown_gain_stage_and_auto_are_refused() {
        let (addr, h) = fake_peer(|_s| {});
        let mut r = open(Some(&addr)).unwrap();
        assert!(r.set_gain("nope", GainSetting::Manual(1.0)).is_err());
        assert!(r.set_gain("att", GainSetting::Auto).is_err());
        h.join().unwrap();
    }
}
