//! Driver for the Hermes Lite 2, speaking the openHPSDR protocol 1 over UDP.
//!
//! Receive side only, receiver 1 only. The radio is named by its IP address
//! (`hermeslite2:192.168.1.50`, or `hermeslite2:192.168.1.50:1024` for another port).
//! `probe()` finds nothing by itself. `discover()` sends one broadcast, and only when
//! the caller asks for it.
//!
//! Wire format (multi-byte fields are big endian):
//! - Discovery: 63 bytes, EF FE 02 then zeros. The reply starts EF FE 02 (idle) or
//!   EF FE 03 (streaming). Byte 9 is the gateware major version, byte 10 the board id
//!   (6 for this radio), byte 0x13 the receiver count of the gateware, byte 0x15 the
//!   minor version.
//! - Start and stop: 64 bytes, EF FE 04 01 (start) or EF FE 04 00 (stop). They are sent
//!   from the socket that receives the samples.
//! - Samples: 1032-byte packets, EF FE 01 06, a 32-bit sequence number, then two
//!   512-byte frames. A frame is 7F 7F 7F, five control bytes and 504 bytes of slots.
//!   A slot for one receiver is 8 bytes: a 24-bit Q word, a 24-bit I word and a 16-bit
//!   microphone word (zero here). That gives 63 slots per frame.
//! - Control: each sample packet is answered with a 1032-byte packet, EF FE 01 02, a
//!   sequence number and two frames. Each frame carries one register: its address shifted
//!   left one bit in byte 0, its value in bytes 1 to 4. The radio stops streaming when
//!   these stop coming.

use std::io;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use crate::{DeviceInfo, GainSetting, GainStage, RxRadio};

const PORT: u16 = 1024;
const BOARD_HL2: u8 = 6;
const DISCOVERY_LEN: usize = 63;
const COMMAND_LEN: usize = 64;
const PACKET_LEN: usize = 1032;
const HEADER: usize = 8;
const FRAME_LEN: usize = 512;
const FRAME_DATA: usize = 504;
const SLOT: usize = 8;
const SLOTS: usize = FRAME_DATA / SLOT;
const SAMPLE_RATES: [u32; 4] = [48_000, 96_000, 192_000, 384_000];
const MAX_FREQ_HZ: u32 = 38_400_000;
const LNA_MIN_DB: f32 = -12.0;
const LNA_MAX_DB: f32 = 48.0;
const SCALE: f32 = 1.0 / 8_388_608.0;
const DEFAULT_FREQ_HZ: u32 = 7_074_000;
const DEFAULT_RATE_IDX: usize = 2;
const DEFAULT_LNA_RAW: u8 = 32;

const REG_RATE: u8 = 0x00;
const REG_TX_NCO: u8 = 0x01;
const REG_RX1_NCO: u8 = 0x02;
const REG_LNA: u8 = 0x0a;
const REG_CYCLE: [u8; 4] = [REG_RATE, REG_TX_NCO, REG_RX1_NCO, REG_LNA];

const READ_WAIT: Duration = Duration::from_millis(200);
const START_WAIT: Duration = Duration::from_millis(1500);
const START_RESEND: Duration = Duration::from_millis(400);
const NUDGE_AFTER: Duration = Duration::from_millis(500);
const SETTLE_WAIT: Duration = Duration::from_millis(300);
const STALL_LIMIT: Duration = Duration::from_secs(3);
const GAP_LIMIT: u32 = 100_000;

/// Finds nothing by itself. The operator names the radio, or asks for `discover()`.
pub fn probe() -> Vec<DeviceInfo> {
    Vec::new()
}

/// Sends one discovery broadcast and lists the Hermes Lite 2 radios that answer
/// within `timeout`. Only called when the operator asks for it.
pub fn discover(timeout: Duration) -> Result<Vec<DeviceInfo>, String> {
    let broadcast = SocketAddr::from(([255, 255, 255, 255], PORT));
    discover_at(&[broadcast], timeout)
}

/// Opens the radio named by `id` (an IP address, optionally with `:port`).
pub fn open(id: Option<&str>) -> Result<Box<dyn RxRadio>, String> {
    let text = id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("name the Hermes Lite 2 by its IP address, for example hermeslite2:192.168.1.50")?;
    let dev = parse_address(text)?;
    Ok(Box::new(Hl2::connect(dev)?))
}

fn parse_address(text: &str) -> Result<SocketAddr, String> {
    let addr = match text.parse::<SocketAddr>() {
        Ok(a) => a,
        Err(_) => match text.parse::<IpAddr>() {
            Ok(ip) => SocketAddr::new(ip, PORT),
            Err(_) => {
                return Err(format!(
                    "{text:?} is not an IP address; give the radio's address, for example 192.168.1.50"
                ))
            }
        },
    };
    if !addr.is_ipv4() {
        return Err(format!("{text:?}: only IPv4 addresses are supported for the Hermes Lite 2"));
    }
    Ok(addr)
}

/// Fields of a discovery reply that matter here.
#[derive(Debug, Clone, PartialEq)]
struct DiscoveryReply {
    busy: bool,
    gateware_major: u8,
    gateware_minor: Option<u8>,
    board_id: u8,
    /// Receivers the gateware has; 0 when the reply does not say.
    receivers: u8,
}

fn parse_discovery_reply(b: &[u8]) -> Result<DiscoveryReply, String> {
    if b.len() < 11 {
        return Err(format!("discovery reply too short ({} bytes)", b.len()));
    }
    if b[0] != 0xEF || b[1] != 0xFE || (b[2] != 0x02 && b[2] != 0x03) {
        return Err("the packet is not a discovery reply".into());
    }
    let receivers = match b.get(0x13) {
        Some(&n) if (1..=12).contains(&n) => n,
        _ => 0,
    };
    Ok(DiscoveryReply {
        busy: b[2] == 0x03,
        gateware_major: b[9],
        gateware_minor: b.get(0x15).copied(),
        board_id: b[10],
        receivers,
    })
}

fn discovery_packet() -> [u8; DISCOVERY_LEN] {
    let mut p = [0u8; DISCOVERY_LEN];
    p[..3].copy_from_slice(&[0xEF, 0xFE, 0x02]);
    p
}

fn command_packet(start: bool) -> [u8; COMMAND_LEN] {
    let mut p = [0u8; COMMAND_LEN];
    p[..4].copy_from_slice(&[0xEF, 0xFE, 0x04, if start { 0x01 } else { 0x00 }]);
    p
}

fn is_sample_packet(b: &[u8]) -> bool {
    b.len() >= PACKET_LEN && b[..4] == [0xEF, 0xFE, 0x01, 0x06]
}

fn is_timeout(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut)
}

/// Low-pass filter relay bits for the frequency (the filter board's outputs, already in register bit positions).
fn filter_bits(hz: f64) -> u32 {
    if hz < 2.0e6 {
        0x02
    } else if hz < 4.0e6 {
        0x04
    } else if hz < 8.0e6 {
        0x08
    } else if hz < 16.0e6 {
        0x10
    } else if hz < 24.0e6 {
        0x20
    } else if hz < 30.0e6 {
        0x40
    } else {
        0x00
    }
}

/// The two registers sent in one control packet, given the running counter.
fn control_regs(cc: usize) -> [u8; 2] {
    [REG_CYCLE[cc % 4], REG_CYCLE[(cc + 1) % 4]]
}

/// Value of one register as the radio expects it.
fn register_value(reg: u8, freq_hz: u32, rate_idx: usize, lna_raw: u8) -> u32 {
    match reg {
        // rate in bits 25..24, filter relays in bits 23..16, one receiver (bits 6..3), bit 2 set
        REG_RATE => {
            ((rate_idx as u32 & 3) << 24) | (filter_bits(freq_hz as f64) << 16) | 0x04
        }
        REG_TX_NCO | REG_RX1_NCO => freq_hz,
        REG_LNA => 0x40 | (lna_raw as u32 & 0x3f),
        _ => 0,
    }
}

fn signed24(b: &[u8]) -> i32 {
    let v = (b[0] as i32) << 16 | (b[1] as i32) << 8 | b[2] as i32;
    if v & 0x80_0000 != 0 {
        v - 0x100_0000
    } else {
        v
    }
}

fn frame_sync(f: &[u8]) -> bool {
    f[0] == 0x7F && f[1] == 0x7F && f[2] == 0x7F
}

/// True when every microphone word in the frame is zero: the layout of one receiver.
fn layout_fits(f: &[u8]) -> bool {
    (0..SLOTS).all(|i| {
        let m = HEADER + i * SLOT + 6;
        f[m] == 0 && f[m + 1] == 0
    })
}

/// Decodes the samples of both frames of one packet, first frame first.
/// The first 24-bit word of a slot is Q, the second is I.
fn decode_slots(f0: &[u8], f1: &[u8], out: &mut Vec<(f32, f32)>) {
    out.clear();
    for f in [f0, f1] {
        for i in 0..SLOTS {
            let p = HEADER + i * SLOT;
            let q = signed24(&f[p..p + 3]);
            let iv = signed24(&f[p + 3..p + 6]);
            out.push((iv as f32 * SCALE, q as f32 * SCALE));
        }
    }
}

fn discover_at(targets: &[SocketAddr], timeout: Duration) -> Result<Vec<DeviceInfo>, String> {
    let sock = UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("cannot open a network socket: {e}"))?;
    sock.set_broadcast(true).map_err(|e| format!("cannot allow broadcast: {e}"))?;
    let q = discovery_packet();
    for t in targets {
        // a target that cannot be reached is skipped; the others still answer
        let _ = sock.send_to(&q, t);
    }
    let end = Instant::now() + timeout;
    let mut buf = [0u8; 256];
    let mut seen: Vec<IpAddr> = Vec::new();
    let mut found = Vec::new();
    loop {
        let now = Instant::now();
        if now >= end {
            break;
        }
        sock.set_read_timeout(Some((end - now).max(Duration::from_millis(1))))
            .map_err(|e| format!("cannot set the discovery wait: {e}"))?;
        match sock.recv_from(&mut buf) {
            Ok((n, from)) => {
                let Ok(r) = parse_discovery_reply(&buf[..n]) else { continue };
                if r.board_id != BOARD_HL2 || seen.contains(&from.ip()) {
                    continue;
                }
                seen.push(from.ip());
                let mut label = format!("Hermes Lite 2 at {}", from.ip());
                match r.gateware_minor {
                    Some(minor) => label.push_str(&format!(", gateware {}.{}", r.gateware_major, minor)),
                    None => label.push_str(&format!(", gateware {}", r.gateware_major)),
                }
                if r.receivers > 0 {
                    label.push_str(&format!(", {} receivers", r.receivers));
                }
                if r.busy {
                    label.push_str(" (busy, streaming to another host)");
                }
                found.push(DeviceInfo {
                    driver: "hermeslite2",
                    label,
                    id: from.ip().to_string(),
                });
            }
            // a read that times out or an error from an unreachable host: keep listening until the end time
            Err(_) => {}
        }
    }
    Ok(found)
}

/// An open Hermes Lite 2: one connected UDP socket and the stream state.
pub struct Hl2 {
    label: String,
    sock: UdpSocket,
    freq_hz: u32,
    rate_idx: usize,
    lna_raw: u8,
    running: bool,
    tx_seq: u32,
    cc: usize,
    rx_seq: u32,
    rx_seq_known: bool,
    live: bool,
    settling: bool,
    settle_at: Instant,
    pending: Vec<(f32, f32)>,
    pending_pos: usize,
    lost: u64,
    last_data: Instant,
    last_start: Instant,
    stall_limit: Duration,
}

impl Hl2 {
    fn connect(dev: SocketAddr) -> Result<Self, String> {
        let sock = UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("cannot open a network socket: {e}"))?;
        sock.connect(dev)
            .map_err(|e| format!("cannot use {dev} as the radio's address: {e}"))?;
        let now = Instant::now();
        Ok(Hl2 {
            label: format!("Hermes Lite 2 at {dev}"),
            sock,
            freq_hz: DEFAULT_FREQ_HZ,
            rate_idx: DEFAULT_RATE_IDX,
            lna_raw: DEFAULT_LNA_RAW,
            running: false,
            tx_seq: 0,
            cc: 0,
            rx_seq: 0,
            rx_seq_known: false,
            live: false,
            settling: true,
            settle_at: now,
            pending: Vec::new(),
            pending_pos: 0,
            lost: 0,
            last_data: now,
            last_start: now,
            stall_limit: STALL_LIMIT,
        })
    }

    /// Maps a network error to a plain message that says what to check.
    fn net_error(&self, e: io::Error) -> String {
        if e.kind() == io::ErrorKind::ConnectionRefused {
            format!(
                "{}: nothing answered, the port refused the packets. Check that the radio is on and the address is right",
                self.label
            )
        } else {
            format!("{}: network error: {e}", self.label)
        }
    }

    fn send_command(&mut self, start: bool) -> Result<(), String> {
        let p = command_packet(start);
        self.sock.send(&p).map(|_| ()).map_err(|e| self.net_error(e))
    }

    /// Sends the next two registers. Errors are left for the next read to report.
    fn send_control(&mut self) {
        let regs = control_regs(self.cc);
        self.cc = (self.cc + 2) % REG_CYCLE.len();
        let mut p = [0u8; PACKET_LEN];
        p[..4].copy_from_slice(&[0xEF, 0xFE, 0x01, 0x02]);
        p[4..8].copy_from_slice(&self.tx_seq.to_be_bytes());
        self.tx_seq = self.tx_seq.wrapping_add(1);
        for (h, &reg) in regs.iter().enumerate() {
            let base = HEADER + h * FRAME_LEN;
            p[base..base + 3].fill(0x7F);
            p[base + 3] = reg << 1;
            let v = register_value(reg, self.freq_hz, self.rate_idx, self.lna_raw);
            p[base + 4..base + 8].copy_from_slice(&v.to_be_bytes());
        }
        let _ = self.sock.send(&p);
    }

    fn start_stream(&mut self) -> Result<(), String> {
        if self.running {
            return Ok(());
        }
        self.tx_seq = 0;
        self.cc = 0;
        self.rx_seq_known = false;
        self.live = false;
        self.settling = true;
        self.settle_at = Instant::now();
        self.pending.clear();
        self.pending_pos = 0;
        // registers first (the rate it starts at), then start, then registers again
        self.send_control();
        self.send_control();
        self.send_command(true)?;
        self.send_control();
        self.send_control();
        let t0 = Instant::now();
        self.last_start = t0;
        let mut buf = [0u8; 1500];
        loop {
            let now = Instant::now();
            if now.duration_since(t0) >= START_WAIT {
                return Err(format!(
                    "{} sent no samples within {} s: check that the radio is on, the address is right, and that no other host is streaming from it",
                    self.label,
                    START_WAIT.as_secs_f32()
                ));
            }
            if now.duration_since(self.last_start) >= START_RESEND {
                self.send_command(true)?;
                self.last_start = now;
            }
            self.sock
                .set_read_timeout(Some(Duration::from_millis(50)))
                .map_err(|e| format!("{}: cannot set the read wait: {e}", self.label))?;
            match self.sock.recv(&mut buf) {
                Ok(n) if is_sample_packet(&buf[..n]) => {
                    self.last_data = Instant::now();
                    self.handle_packet(&buf[..n]);
                    self.running = true;
                    return Ok(());
                }
                Ok(_) => {}
                Err(e) if is_timeout(&e) => {}
                Err(e) => return Err(self.net_error(e)),
            }
        }
    }

    fn stop_stream(&mut self) {
        if self.running {
            let _ = self.send_command(false);
            let _ = self.send_command(false);
        }
        self.running = false;
        self.pending.clear();
        self.pending_pos = 0;
    }

    /// Takes the decoded samples that are still waiting, as many as fit.
    fn take_pending(&mut self, out: &mut [(f32, f32)]) -> usize {
        let avail = self.pending.len() - self.pending_pos;
        let k = avail.min(out.len());
        out[..k].copy_from_slice(&self.pending[self.pending_pos..self.pending_pos + k]);
        self.pending_pos += k;
        if self.pending_pos == self.pending.len() {
            self.pending.clear();
            self.pending_pos = 0;
        }
        k
    }

    /// Decodes one sample packet, counts what was lost, and answers it with control registers.
    fn handle_packet(&mut self, b: &[u8]) {
        let seq = u32::from_be_bytes([b[4], b[5], b[6], b[7]]);
        let mut gap = 0u32;
        if self.rx_seq_known && seq != self.rx_seq.wrapping_add(1) {
            gap = seq.wrapping_sub(self.rx_seq).wrapping_sub(1);
        }
        if gap >= GAP_LIMIT {
            // a jump this big is a restart of the counter, not a loss
            gap = 0;
        }
        self.rx_seq = seq;
        self.rx_seq_known = true;

        let f0 = &b[HEADER..HEADER + FRAME_LEN];
        let f1 = &b[HEADER + FRAME_LEN..PACKET_LEN];
        let per = SLOTS as u64;
        if gap > 0 && self.live {
            self.lost += gap as u64 * 2 * per;
        }
        if self.settling {
            // after a start the radio may still send an older layout: those packets are dropped for a short while
            let fits = layout_fits(f0) && layout_fits(f1);
            if !fits && self.settle_at.elapsed() < SETTLE_WAIT {
                if self.live {
                    self.lost += 2 * per;
                }
                self.send_control();
                return;
            }
            self.settling = false;
        }
        if frame_sync(f0) && frame_sync(f1) {
            self.pending.clear();
            self.pending_pos = 0;
            decode_slots(f0, f1, &mut self.pending);
            self.live = true;
        }
        self.send_control();
    }

    fn read_samples(&mut self, out: &mut [(f32, f32)]) -> Result<usize, String> {
        if !self.running {
            return Err(format!("{} is not streaming; start it first", self.label));
        }
        if out.is_empty() {
            return Ok(0);
        }
        let k = self.take_pending(out);
        if k > 0 {
            return Ok(k);
        }
        let deadline = Instant::now() + READ_WAIT;
        let mut buf = [0u8; 1500];
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Ok(0);
            }
            self.sock
                .set_read_timeout(Some((deadline - now).max(Duration::from_millis(1))))
                .map_err(|e| format!("{}: cannot set the read wait: {e}", self.label))?;
            match self.sock.recv(&mut buf) {
                Ok(n) => {
                    if is_sample_packet(&buf[..n]) {
                        self.last_data = Instant::now();
                        self.handle_packet(&buf[..n]);
                        let k = self.take_pending(out);
                        if k > 0 {
                            return Ok(k);
                        }
                    }
                }
                Err(e) if is_timeout(&e) => {
                    let now = Instant::now();
                    if now.duration_since(self.last_data) > self.stall_limit {
                        self.running = false;
                        return Err(format!(
                            "{} sent no samples for {} s: the radio has stopped or lost power",
                            self.label,
                            self.stall_limit.as_secs_f32()
                        ));
                    }
                    if now.duration_since(self.last_data) > NUDGE_AFTER
                        && now.duration_since(self.last_start) > NUDGE_AFTER
                    {
                        self.send_command(true)?;
                        self.last_start = now;
                    }
                }
                Err(e) => return Err(self.net_error(e)),
            }
        }
    }
}

impl Drop for Hl2 {
    fn drop(&mut self) {
        self.stop_stream();
    }
}

impl RxRadio for Hl2 {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn freq_range_hz(&self) -> (u64, u64) {
        (0, MAX_FREQ_HZ as u64)
    }

    fn set_frequency_hz(&mut self, hz: u64) -> Result<(), String> {
        if hz > MAX_FREQ_HZ as u64 {
            return Err(format!(
                "{hz} Hz is above the Hermes Lite 2 range of 0 to 38.4 MHz"
            ));
        }
        self.freq_hz = hz as u32;
        Ok(())
    }

    fn sample_rates(&self) -> Vec<u32> {
        SAMPLE_RATES.to_vec()
    }

    fn set_sample_rate(&mut self, hz: u32) -> Result<u32, String> {
        let (idx, _) = SAMPLE_RATES
            .iter()
            .enumerate()
            .min_by_key(|(_, r)| r.abs_diff(hz))
            .expect("the rate table is not empty");
        self.rate_idx = idx;
        Ok(SAMPLE_RATES[idx])
    }

    fn gain_stages(&self) -> Vec<GainStage> {
        vec![GainStage {
            name: "LNA".into(),
            min_db: LNA_MIN_DB,
            max_db: LNA_MAX_DB,
            step_db: 1.0,
            auto: false,
        }]
    }

    fn set_gain(&mut self, stage: &str, setting: GainSetting) -> Result<(), String> {
        if stage != "LNA" {
            return Err(format!("the Hermes Lite 2 has no gain stage named {stage:?}; its stage is LNA"));
        }
        match setting {
            GainSetting::Auto => Err("the Hermes Lite 2 LNA has no automatic setting; give a value in dB".into()),
            GainSetting::Manual(db) => {
                if !db.is_finite() {
                    return Err("the LNA gain must be a number of dB".into());
                }
                let db = db.clamp(LNA_MIN_DB, LNA_MAX_DB);
                self.lna_raw = (db.round() as i32 + 12) as u8;
                Ok(())
            }
        }
    }

    fn start(&mut self) -> Result<(), String> {
        self.start_stream()
    }

    fn read_iq(&mut self, out: &mut [(f32, f32)]) -> Result<usize, String> {
        self.read_samples(out)
    }

    fn stop(&mut self) {
        self.stop_stream();
    }

    fn dropped_samples(&self) -> u64 {
        self.lost
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc};
    use std::thread::{self, JoinHandle};

    fn put24(dst: &mut [u8], v: i32) {
        let u = v as u32;
        dst[0] = (u >> 16) as u8;
        dst[1] = (u >> 8) as u8;
        dst[2] = u as u8;
    }

    /// A sample packet. Frame h, slot i: I = 1000 + 100h + i, Q = -I, microphone word zero.
    fn sample_packet(seq: u32) -> Vec<u8> {
        let mut p = vec![0u8; PACKET_LEN];
        p[..4].copy_from_slice(&[0xEF, 0xFE, 0x01, 0x06]);
        p[4..8].copy_from_slice(&seq.to_be_bytes());
        for h in 0..2 {
            let base = HEADER + h * FRAME_LEN;
            p[base..base + 3].fill(0x7F);
            for i in 0..SLOTS {
                let s = base + HEADER + i * SLOT;
                let iv = 1000 + (100 * h + i) as i32;
                put24(&mut p[s..s + 3], -iv);
                put24(&mut p[s + 3..s + 6], iv);
            }
        }
        p
    }

    fn discovery_reply(board: u8, busy: bool) -> Vec<u8> {
        let mut r = vec![0u8; 0x16];
        r[..3].copy_from_slice(&[0xEF, 0xFE, if busy { 0x03 } else { 0x02 }]);
        r[3..9].copy_from_slice(&[0x00, 0x1C, 0xC0, 0xA2, 0x11, 0x22]);
        r[9] = 7;
        r[10] = board;
        r[0x13] = 4;
        r[0x15] = 3;
        r
    }

    /// A fake radio on 127.0.0.1. It answers discovery, and after a start command it
    /// sends `script` (sequence numbers) once. Every packet it gets is passed to `rx`.
    struct Peer {
        addr: SocketAddr,
        rx: mpsc::Receiver<Vec<u8>>,
        done: Arc<AtomicBool>,
        handle: Option<JoinHandle<()>>,
    }

    impl Drop for Peer {
        fn drop(&mut self) {
            self.done.store(true, Ordering::SeqCst);
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
        }
    }

    fn spawn_peer(script: Vec<u32>) -> Peer {
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = sock.local_addr().unwrap();
        sock.set_read_timeout(Some(Duration::from_millis(10))).unwrap();
        let (tx, rx) = mpsc::channel();
        let done = Arc::new(AtomicBool::new(false));
        let done_t = done.clone();
        let handle = thread::spawn(move || {
            let mut buf = [0u8; 1500];
            let mut sent = false;
            while !done_t.load(Ordering::SeqCst) {
                let Ok((n, from)) = sock.recv_from(&mut buf) else { continue };
                let pkt = buf[..n].to_vec();
                if n == DISCOVERY_LEN && pkt[2] == 0x02 {
                    let _ = sock.send_to(&discovery_reply(BOARD_HL2, false), from);
                }
                if n == COMMAND_LEN && pkt[2] == 0x04 && pkt[3] == 0x01 && !sent {
                    sent = true;
                    for &seq in &script {
                        let _ = sock.send_to(&sample_packet(seq), from);
                    }
                }
                let _ = tx.send(pkt);
            }
        });
        Peer { addr, rx, done, handle: Some(handle) }
    }

    fn driver_for(peer: &Peer) -> Hl2 {
        Hl2::connect(peer.addr).unwrap()
    }

    fn drain_samples(r: &mut Hl2, want: usize) -> usize {
        let mut buf = [(0f32, 0f32); 200];
        let mut total = 0;
        for _ in 0..200 {
            if total >= want {
                break;
            }
            total += r.read_iq(&mut buf).unwrap();
        }
        total
    }

    #[test]
    fn discovery_reply_fields_parse() {
        let r = parse_discovery_reply(&discovery_reply(6, true)).unwrap();
        assert_eq!(
            r,
            DiscoveryReply { busy: true, gateware_major: 7, gateware_minor: Some(3), board_id: 6, receivers: 4 }
        );
        let short_reply = parse_discovery_reply(&[0xEF, 0xFE, 0x02, 0, 0]);
        assert!(short_reply.is_err());
        let mut wrong = discovery_reply(6, false);
        wrong[2] = 0x01;
        assert!(parse_discovery_reply(&wrong).is_err());
        // no receiver count in the reply (a radio that does not say) gives 0
        let mut no_count = discovery_reply(1, false);
        no_count[0x13] = 0;
        assert_eq!(parse_discovery_reply(&no_count).unwrap().receivers, 0);
    }

    #[test]
    fn command_packets_have_the_wire_layout() {
        let d = discovery_packet();
        assert_eq!(d.len(), 63);
        assert_eq!(&d[..3], &[0xEF, 0xFE, 0x02]);
        assert!(d[3..].iter().all(|&b| b == 0));
        let start = command_packet(true);
        assert_eq!(start.len(), 64);
        assert_eq!(&start[..4], &[0xEF, 0xFE, 0x04, 0x01]);
        assert_eq!(command_packet(false)[3], 0x00);
    }

    #[test]
    fn control_packet_carries_rate_frequency_and_gain() {
        let mut r = Hl2::connect("127.0.0.1:9".parse().unwrap()).unwrap();
        r.set_frequency_hz(7_100_000).unwrap();
        r.set_sample_rate(192_000).unwrap();
        r.set_gain("LNA", GainSetting::Manual(20.0)).unwrap();
        // first packet: rate register (frame 0) and TX NCO (frame 1)
        let regs = control_regs(r.cc);
        assert_eq!(regs, [REG_RATE, REG_TX_NCO]);
        let v_rate = register_value(REG_RATE, r.freq_hz, r.rate_idx, r.lna_raw);
        assert_eq!(v_rate >> 24, 2, "192 ksps is speed 2");
        assert_eq!((v_rate >> 3) & 0x0f, 0, "one receiver");
        assert_eq!(v_rate & 0x04, 0x04);
        // the frame layout and sequence number in a real packet
        let mut sent = [0u8; PACKET_LEN];
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let dst = sock.local_addr().unwrap();
        r.sock.connect(dst).unwrap();
        r.send_control();
        let n = sock.recv(&mut sent).unwrap();
        assert_eq!(n, PACKET_LEN);
        assert_eq!(&sent[..4], &[0xEF, 0xFE, 0x01, 0x02]);
        assert_eq!(&sent[4..8], &0u32.to_be_bytes());
        assert_eq!(sent[8 + 3], REG_RATE << 1);
        assert_eq!(&sent[8 + 4..8 + 8], &v_rate.to_be_bytes());
        let base1 = HEADER + FRAME_LEN;
        assert_eq!(sent[base1 + 3], REG_TX_NCO << 1);
        assert_eq!(&sent[base1 + 4..base1 + 8], &7_100_000u32.to_be_bytes());
        // second packet: receiver 1 NCO and LNA, sequence 1
        r.send_control();
        let n = sock.recv(&mut sent).unwrap();
        assert_eq!(n, PACKET_LEN);
        assert_eq!(&sent[4..8], &1u32.to_be_bytes());
        assert_eq!(sent[8 + 3], REG_RX1_NCO << 1);
        assert_eq!(sent[base1 + 3], REG_LNA << 1);
        assert_eq!(&sent[base1 + 4..base1 + 8], &(0x40u32 | 32).to_be_bytes());
    }

    #[test]
    fn frequency_rate_and_gain_limits() {
        let mut r = Hl2::connect("127.0.0.1:9".parse().unwrap()).unwrap();
        assert!(r.set_frequency_hz(38_400_000).is_ok());
        assert!(r.set_frequency_hz(38_400_001).is_err());
        assert_eq!(r.set_sample_rate(100_000).unwrap(), 96_000);
        assert_eq!(r.set_sample_rate(1_000_000).unwrap(), 384_000);
        assert_eq!(r.sample_rates(), vec![48_000, 96_000, 192_000, 384_000]);
        r.set_gain("LNA", GainSetting::Manual(60.0)).unwrap();
        assert_eq!(r.lna_raw, 60);
        r.set_gain("LNA", GainSetting::Manual(-30.0)).unwrap();
        assert_eq!(r.lna_raw, 0);
        assert!(r.set_gain("LNA", GainSetting::Auto).is_err());
        assert!(r.set_gain("LNA", GainSetting::Manual(f32::NAN)).is_err());
        assert!(r.set_gain("IF", GainSetting::Manual(0.0)).is_err());
        assert_eq!(filter_bits(7.1e6), 0x08);
        assert_eq!(filter_bits(14.0e6), 0x10);
    }

    #[test]
    fn sample_packet_decodes_in_order_q_then_i() {
        let p = sample_packet(0);
        let mut out = Vec::new();
        decode_slots(&p[HEADER..HEADER + FRAME_LEN], &p[HEADER + FRAME_LEN..PACKET_LEN], &mut out);
        assert_eq!(out.len(), 2 * SLOTS);
        let (i0, q0) = out[0];
        assert!((i0 - 1000.0 * SCALE).abs() < 1e-9);
        assert!((q0 + 1000.0 * SCALE).abs() < 1e-9);
        let (i1, _) = out[SLOTS];
        assert!((i1 - 1100.0 * SCALE).abs() < 1e-9);
        assert!(layout_fits(&p[HEADER..HEADER + FRAME_LEN]));
    }

    #[test]
    fn bad_frames_and_settling_are_dropped_and_counted() {
        let mut r = Hl2::connect("127.0.0.1:9".parse().unwrap()).unwrap();
        // an old layout (microphone word not zero) is dropped while settling
        let mut old = sample_packet(0);
        // microphone word of slot 0 in the first frame (packet bytes 22 and 23)
        old[HEADER + HEADER + 6] = 0x12;
        r.handle_packet(&old);
        assert!(r.pending.is_empty());
        // after the settle time it is taken as it is
        r.settle_at = Instant::now().checked_sub(Duration::from_secs(1)).unwrap();
        r.handle_packet(&old);
        assert_eq!(r.pending.len(), 2 * SLOTS);
        r.pending.clear();
        // a frame without 7F 7F 7F is dropped, nothing is delivered, and nothing is counted as lost
        let mut bad = sample_packet(1);
        bad[HEADER] = 0x00;
        r.handle_packet(&bad);
        assert!(r.pending.is_empty());
        assert_eq!(r.lost, 0);
    }

    #[test]
    fn sequence_gap_counts_lost_samples() {
        let mut r = Hl2::connect("127.0.0.1:9".parse().unwrap()).unwrap();
        for seq in [0u32, 1, 2, 5] {
            r.handle_packet(&sample_packet(seq));
            r.pending.clear();
        }
        // seq 3 and 4 missing: two packets of 2 * 63 samples each
        assert_eq!(r.lost, 2 * 2 * SLOTS as u64);
        // a restart of the counter is not counted as a loss
        r.handle_packet(&sample_packet(10_000_000));
        assert_eq!(r.lost, 2 * 2 * SLOTS as u64);
    }

    #[test]
    fn discovery_finds_the_radio_on_loopback() {
        let peer = spawn_peer(vec![]);
        let found = discover_at(&[peer.addr], Duration::from_millis(300)).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].driver, "hermeslite2");
        assert_eq!(found[0].id, "127.0.0.1");
        assert!(found[0].label.starts_with("Hermes Lite 2 at 127.0.0.1"));
    }

    #[test]
    fn probe_finds_nothing_without_being_asked() {
        assert!(probe().is_empty());
    }

    #[test]
    fn open_parses_the_address() {
        assert!(open(None).is_err());
        assert!(open(Some("not an address")).is_err());
        assert!(open(Some("::1")).is_err());
        let r = open(Some("127.0.0.1")).unwrap();
        assert_eq!(r.label(), "Hermes Lite 2 at 127.0.0.1:1024");
        let r = open(Some("127.0.0.1:5555")).unwrap();
        assert_eq!(r.label(), "Hermes Lite 2 at 127.0.0.1:5555");
        assert_eq!(parse_address("10.0.0.2").unwrap().port(), PORT);
        assert_eq!(parse_address("10.0.0.2").unwrap().ip(), IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)));
    }

    #[test]
    fn start_stream_read_gap_and_stop() {
        // packets 0, 1, 2, 5, 6: packets 3 and 4 never arrive
        let peer = spawn_peer(vec![0, 1, 2, 5, 6]);
        let mut r = driver_for(&peer);
        r.start().unwrap();
        let total = drain_samples(&mut r, 5 * 2 * SLOTS);
        assert_eq!(total, 5 * 2 * SLOTS);
        assert_eq!(r.dropped_samples(), 2 * 2 * SLOTS as u64);
        r.stop();
        let mut saw_stop = false;
        while let Ok(pkt) = peer.rx.recv_timeout(Duration::from_secs(1)) {
            if pkt.len() == COMMAND_LEN && pkt[3] == 0x00 {
                saw_stop = true;
                break;
            }
        }
        assert!(saw_stop, "the stop command reached the radio");
        assert!(r.read_iq(&mut [(0.0, 0.0); 4]).is_err(), "no reads after stop");
    }

    #[test]
    fn read_waits_at_most_about_200_ms() {
        let peer = spawn_peer(vec![0]);
        let mut r = driver_for(&peer);
        r.start().unwrap();
        // the one packet is 126 samples; drain them
        assert_eq!(drain_samples(&mut r, 2 * SLOTS), 2 * SLOTS);
        let t = Instant::now();
        let n = r.read_iq(&mut [(0.0, 0.0); 64]).unwrap();
        let waited = t.elapsed();
        assert_eq!(n, 0);
        assert!(waited >= Duration::from_millis(150), "waited {waited:?}");
        assert!(waited < Duration::from_millis(500), "waited {waited:?}");
    }

    #[test]
    fn silence_after_streaming_is_reported_as_lost() {
        let peer = spawn_peer(vec![0]);
        let mut r = driver_for(&peer);
        r.stall_limit = Duration::from_millis(300);
        r.start().unwrap();
        assert_eq!(drain_samples(&mut r, 2 * SLOTS), 2 * SLOTS);
        let t = Instant::now();
        let err = loop {
            match r.read_iq(&mut [(0.0, 0.0); 64]) {
                Ok(0) => {
                    assert!(t.elapsed() < Duration::from_secs(3), "the stall was not reported in time");
                }
                Ok(_) => {}
                Err(e) => break e,
            }
        };
        assert!(err.contains("no samples"), "{err}");
    }

    #[test]
    fn no_answer_is_a_clear_error() {
        let peer = spawn_peer(vec![]);
        let mut r = driver_for(&peer);
        let err = r.start().unwrap_err();
        assert!(err.contains("sent no samples"), "{err}");
        assert!(err.contains("Hermes Lite 2"), "{err}");
    }

    #[test]
    fn closed_port_is_a_clear_error() {
        // take a free port, then close it: nothing listens there
        let port = UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let mut r = open(Some(&format!("127.0.0.1:{port}"))).unwrap();
        let err = r.start().unwrap_err();
        assert!(err.contains("Hermes Lite 2"), "{err}");
        assert!(err.contains("127.0.0.1"), "{err}");
    }
}
