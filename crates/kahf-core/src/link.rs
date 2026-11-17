//! Link layer (owner: link agent). Connection handshake, ARQ, rate adaptation.
//!
//! Wire format. Every payload is <= 36 bytes (the frame size of `kahf_core::frame`):
//! - `CONNECT`, `CONNECT_ACK`, `DISCONNECT`: `[tag, from_len, from..., to_len, to...]`
//!   (callsigns are 1..=15 bytes).
//! - `DATA`: `[0x10, seq, body...]`, body <= 34 bytes.
//! - `ACK`: `[0x11, base, bitmap, fer_pct]`. `base` is the receiver's next in-order
//!   sequence number, bit i of `bitmap` says `base + i` is buffered, and `fer_pct` is the
//!   receiver's measured frame error rate in percent.
//!
//! Time is supplied by the caller through `tick`. Timers run relative to the time given
//! to the most recent `tick` call (0 before the first one).

use std::collections::VecDeque;

const TAG_CONNECT: u8 = 0x01;
const TAG_CONNECT_ACK: u8 = 0x02;
const TAG_DISCONNECT: u8 = 0x03;
const TAG_DATA: u8 = 0x10;
const TAG_ACK: u8 = 0x11;

/// Maximum payload size in bytes.
pub const MAX_PAYLOAD: usize = 36;
/// Maximum callsign length in bytes.
pub const MAX_CALLSIGN: usize = 15;
/// Selective-repeat window (sequence numbers are modulo 256).
pub const WINDOW: usize = 8;
/// Largest data body carried by one DATA frame.
pub const MAX_CHUNK: usize = MAX_PAYLOAD - 2;
/// Handshake retransmit interval.
pub const CONNECT_TIMEOUT_MS: u64 = 1000;
/// Handshake retransmissions before giving up.
pub const MAX_CONNECT_RETRIES: u32 = 5;
/// DATA retransmit timeout.
pub const DATA_TIMEOUT_MS: u64 = 500;
/// DATA retransmissions of one frame before the link drops to Idle.
pub const MAX_DATA_RETRIES: u32 = 8;
/// Highest suggested modulation level.
pub const MAX_MOD_LEVEL: u8 = 3;
/// Suggested level before the peer has reported any frame error rate.
pub const DEFAULT_MOD_LEVEL: u8 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkState { Idle, Connecting { peer: String }, Connected { peer: String }, Disconnecting }

/// What the link layer asks the caller to do after an input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Send this payload (one frame's worth of bytes, <= 36) on air.
    Transmit(Vec<u8>),
    /// Deliver this payload to the session layer.
    Deliver(Vec<u8>),
    StateChanged(LinkState),
}

#[derive(Clone, Debug)]
struct InFlight {
    seq: u8,
    payload: Vec<u8>,
    deadline: u64,
    retries: u32,
}

pub struct Link {
    pub callsign: String,
    pub state: LinkState,
    clock: u64,
    hs_deadline: u64,
    hs_retries: u32,
    // Sender side.
    tx_queue: VecDeque<Vec<u8>>,
    /// Indexed by `seq % WINDOW`; a window of 8 never aliases because 256 % 8 == 0.
    slots: Vec<Option<InFlight>>,
    send_base: u8,
    next_seq: u8,
    peer_fer: Option<u8>,
    // Receiver side.
    rcv_base: u8,
    rx_buf: Vec<Option<Vec<u8>>>,
    /// Highest sequence number accepted so far (used for gap-based FER).
    hi_seen: u8,
    rx_new: u32,
    rx_missing: u32,
}

impl Link {
    pub fn new(callsign: &str) -> Self {
        let mut l = Link {
            callsign: callsign.to_string(),
            state: LinkState::Idle,
            clock: 0,
            hs_deadline: 0,
            hs_retries: 0,
            tx_queue: VecDeque::new(),
            slots: Vec::new(),
            send_base: 0,
            next_seq: 0,
            peer_fer: None,
            rcv_base: 0,
            rx_buf: Vec::new(),
            hi_seen: 0,
            rx_new: 0,
            rx_missing: 0,
        };
        l.reset_arq();
        l
    }

    /// Start the handshake with `peer`. Returns nothing unless the link is Idle and both
    /// callsigns are 1..=15 bytes.
    pub fn connect(&mut self, peer: &str) -> Vec<Action> {
        if self.state != LinkState::Idle || !valid_callsign(peer) || !valid_callsign(&self.callsign) {
            return Vec::new();
        }
        self.reset_arq();
        self.hs_retries = 0;
        self.hs_deadline = self.clock + CONNECT_TIMEOUT_MS;
        let mut out = vec![Action::Transmit(encode_hs(TAG_CONNECT, &self.callsign, peer))];
        self.set_state(LinkState::Connecting { peer: peer.to_string() }, &mut out);
        out
    }

    /// Send DISCONNECT to the peer and return to Idle. No-op when not connecting or connected.
    /// The ARQ state is dropped; there is no DISCONNECT acknowledgement.
    pub fn disconnect(&mut self) -> Vec<Action> {
        let peer = match &self.state {
            LinkState::Connected { peer } | LinkState::Connecting { peer } => peer.clone(),
            _ => return Vec::new(),
        };
        let mut out = vec![Action::Transmit(encode_hs(TAG_DISCONNECT, &self.callsign, &peer))];
        self.set_state(LinkState::Disconnecting, &mut out);
        self.reset_arq();
        self.set_state(LinkState::Idle, &mut out);
        out
    }

    /// Feed one decoded frame payload from the air.
    pub fn on_receive(&mut self, payload: &[u8]) -> Vec<Action> {
        match payload.first() {
            Some(&TAG_CONNECT) => self.on_connect(payload),
            Some(&TAG_CONNECT_ACK) => self.on_connect_ack(payload),
            Some(&TAG_DISCONNECT) => self.on_disconnect(payload),
            Some(&TAG_DATA) => self.on_data(payload),
            Some(&TAG_ACK) => self.on_ack(payload),
            _ => Vec::new(),
        }
    }

    /// Feed one outgoing data chunk from the session layer. Data longer than 34 bytes is
    /// split. Nothing is sent unless the link is Connected.
    pub fn send(&mut self, data: &[u8]) -> Vec<Action> {
        if !matches!(self.state, LinkState::Connected { .. }) {
            return Vec::new();
        }
        for chunk in data.chunks(MAX_CHUNK) {
            self.tx_queue.push_back(chunk.to_vec());
        }
        let mut out = Vec::new();
        self.pump(&mut out);
        out
    }

    /// Advance the clock to `now_ms`: retransmit handshake and DATA frames whose timers
    /// expired. Gives up (drops to Idle) after the retry limits are reached.
    pub fn tick(&mut self, now_ms: u64) -> Vec<Action> {
        self.clock = now_ms;
        let mut out = Vec::new();
        match self.state.clone() {
            LinkState::Connecting { peer } => {
                if now_ms >= self.hs_deadline {
                    if self.hs_retries < MAX_CONNECT_RETRIES {
                        self.hs_retries += 1;
                        self.hs_deadline = now_ms + CONNECT_TIMEOUT_MS;
                        out.push(Action::Transmit(encode_hs(TAG_CONNECT, &self.callsign, &peer)));
                    } else {
                        self.reset_arq();
                        self.set_state(LinkState::Idle, &mut out);
                    }
                }
            }
            LinkState::Connected { .. } => {
                let mut dead = false;
                for slot in self.slots.iter_mut() {
                    if let Some(f) = slot.as_mut() {
                        if now_ms >= f.deadline {
                            if f.retries >= MAX_DATA_RETRIES {
                                dead = true;
                                break;
                            }
                            f.retries += 1;
                            f.deadline = now_ms + DATA_TIMEOUT_MS;
                            out.push(Action::Transmit(encode_data(f.seq, &f.payload)));
                        }
                    }
                }
                if dead {
                    self.reset_arq();
                    self.set_state(LinkState::Idle, &mut out);
                }
            }
            _ => {}
        }
        out
    }

    /// Suggested modulation level (0 = most robust, MAX_MOD_LEVEL = fastest) from the
    /// frame error rate the peer last reported. DEFAULT_MOD_LEVEL until a report arrives.
    /// Thresholds: FER < 2% -> 3, < 8% -> 2, < 20% -> 1, otherwise 0.
    pub fn suggested_modulation(&self) -> u8 {
        match self.peer_fer {
            None => DEFAULT_MOD_LEVEL,
            Some(f) if f < 2 => MAX_MOD_LEVEL,
            Some(f) if f < 8 => 2,
            Some(f) if f < 20 => 1,
            Some(_) => 0,
        }
    }

    /// Frame error rate (percent) the peer last reported for our transmissions.
    pub fn peer_frame_error_pct(&self) -> Option<u8> {
        self.peer_fer
    }

    // ---- handshake -------------------------------------------------------

    fn on_connect(&mut self, p: &[u8]) -> Vec<Action> {
        let Some((from, to)) = decode_hs(p) else { return Vec::new() };
        if to != self.callsign {
            return Vec::new();
        }
        let mut out = Vec::new();
        match self.state.clone() {
            LinkState::Idle => self.accept(&from, &mut out),
            // Simultaneous open: both sides sent CONNECT.
            LinkState::Connecting { peer } if peer == from => self.accept(&from, &mut out),
            // Our CONNECT_ACK was lost and the peer retried: acknowledge again.
            LinkState::Connected { peer } if peer == from => {
                out.push(Action::Transmit(encode_hs(TAG_CONNECT_ACK, &self.callsign, &from)));
            }
            _ => {}
        }
        out
    }

    fn accept(&mut self, from: &str, out: &mut Vec<Action>) {
        self.reset_arq();
        self.set_state(LinkState::Connected { peer: from.to_string() }, out);
        out.push(Action::Transmit(encode_hs(TAG_CONNECT_ACK, &self.callsign, from)));
    }

    fn on_connect_ack(&mut self, p: &[u8]) -> Vec<Action> {
        let Some((from, to)) = decode_hs(p) else { return Vec::new() };
        if to != self.callsign {
            return Vec::new();
        }
        let mut out = Vec::new();
        if let LinkState::Connecting { peer } = self.state.clone() {
            if peer == from {
                self.reset_arq();
                self.set_state(LinkState::Connected { peer: from }, &mut out);
            }
        }
        out
    }

    fn on_disconnect(&mut self, p: &[u8]) -> Vec<Action> {
        let Some((from, to)) = decode_hs(p) else { return Vec::new() };
        if to != self.callsign {
            return Vec::new();
        }
        let peer = match &self.state {
            LinkState::Connecting { peer } | LinkState::Connected { peer } => Some(peer.clone()),
            _ => None,
        };
        let mut out = Vec::new();
        if peer.as_deref() == Some(from.as_str()) {
            self.reset_arq();
            self.set_state(LinkState::Idle, &mut out);
        }
        out
    }

    // ---- ARQ -------------------------------------------------------------

    fn on_data(&mut self, p: &[u8]) -> Vec<Action> {
        if !matches!(self.state, LinkState::Connected { .. }) || p.len() < 2 {
            return Vec::new();
        }
        let seq = p[1];
        let body = &p[2..];
        let mut out = Vec::new();
        let off = seq.wrapping_sub(self.rcv_base) as usize;
        if off < WINDOW && self.rx_buf[off].is_none() {
            let ahead = seq.wrapping_sub(self.hi_seen) as usize;
            if (1..=WINDOW).contains(&ahead) {
                self.rx_missing += (ahead - 1) as u32;
                self.hi_seen = seq;
            }
            self.rx_new += 1;
            self.rx_buf[off] = Some(body.to_vec());
        }
        // Deliver the in-order prefix to the session layer.
        while let Some(data) = self.rx_buf[0].take() {
            out.push(Action::Deliver(data));
            self.rx_buf.rotate_left(1);
            self.rcv_base = self.rcv_base.wrapping_add(1);
        }
        // ACK every DATA frame, including duplicates, so the sender can stop resending.
        let mut bitmap = 0u8;
        for (i, slot) in self.rx_buf.iter().enumerate() {
            if slot.is_some() {
                bitmap |= 1u8 << i;
            }
        }
        out.push(Action::Transmit(vec![TAG_ACK, self.rcv_base, bitmap, self.measured_fer_pct()]));
        out
    }

    fn on_ack(&mut self, p: &[u8]) -> Vec<Action> {
        if !matches!(self.state, LinkState::Connected { .. }) || p.len() < 4 {
            return Vec::new();
        }
        let (base, bitmap, fer) = (p[1], p[2], p[3]);
        self.peer_fer = Some(fer.min(100));
        let inflight = self.next_seq.wrapping_sub(self.send_base);
        let adv = base.wrapping_sub(self.send_base);
        // Cumulative part; a stale or out-of-range base is ignored.
        if adv <= inflight {
            for k in 0..adv {
                let seq = self.send_base.wrapping_add(k);
                self.slots[seq as usize % WINDOW] = None;
            }
            self.send_base = base;
        }
        // Selective part.
        for i in 0..WINDOW {
            if bitmap & (1u8 << i) != 0 {
                let seq = base.wrapping_add(i as u8);
                let idx = seq as usize % WINDOW;
                if matches!(&self.slots[idx], Some(f) if f.seq == seq) {
                    self.slots[idx] = None;
                }
            }
        }
        let mut out = Vec::new();
        self.pump(&mut out);
        out
    }

    /// Move queued chunks into the window and transmit them.
    fn pump(&mut self, out: &mut Vec<Action>) {
        while (self.next_seq.wrapping_sub(self.send_base) as usize) < WINDOW {
            let Some(chunk) = self.tx_queue.pop_front() else { break };
            let seq = self.next_seq;
            self.slots[seq as usize % WINDOW] = Some(InFlight {
                seq,
                payload: chunk.clone(),
                deadline: self.clock + DATA_TIMEOUT_MS,
                retries: 0,
            });
            out.push(Action::Transmit(encode_data(seq, &chunk)));
            self.next_seq = seq.wrapping_add(1);
        }
    }

    /// Gap-based estimate of first-attempt frame loss: skipped sequence numbers over all
    /// sequence numbers seen. Corrupted frames never reach the link, so a gap is the only
    /// loss signal available here.
    fn measured_fer_pct(&self) -> u8 {
        let total = self.rx_new + self.rx_missing;
        if total == 0 {
            return 0;
        }
        (self.rx_missing * 100 / total).min(100) as u8
    }

    fn reset_arq(&mut self) {
        self.tx_queue.clear();
        self.slots = vec![None; WINDOW];
        self.send_base = 0;
        self.next_seq = 0;
        self.peer_fer = None;
        self.rcv_base = 0;
        self.rx_buf = vec![None; WINDOW];
        self.hi_seen = 255; // so that sequence 0 is the first expected frame without a gap
        self.rx_new = 0;
        self.rx_missing = 0;
    }

    fn set_state(&mut self, s: LinkState, out: &mut Vec<Action>) {
        if self.state == s {
            return;
        }
        self.state = s.clone();
        out.push(Action::StateChanged(s));
    }
}

fn valid_callsign(c: &str) -> bool {
    !c.is_empty() && c.len() <= MAX_CALLSIGN
}

fn encode_hs(tag: u8, from: &str, to: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(4 + from.len() + to.len());
    v.push(tag);
    v.push(from.len() as u8);
    v.extend_from_slice(from.as_bytes());
    v.push(to.len() as u8);
    v.extend_from_slice(to.as_bytes());
    v
}

fn decode_hs(p: &[u8]) -> Option<(String, String)> {
    let fl = *p.get(1)? as usize;
    let tl = *p.get(2 + fl)? as usize;
    if fl == 0 || tl == 0 || fl > MAX_CALLSIGN || tl > MAX_CALLSIGN || p.len() != 3 + fl + tl {
        return None;
    }
    let from = std::str::from_utf8(&p[2..2 + fl]).ok()?;
    let to = std::str::from_utf8(&p[3 + fl..3 + fl + tl]).ok()?;
    Some((from.to_string(), to.to_string()))
}

fn encode_data(seq: u8, body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(2 + body.len());
    v.push(TAG_DATA);
    v.push(seq);
    v.extend_from_slice(body);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    type DropFn = Box<dyn FnMut(&[u8]) -> bool>;

    /// Two links joined by a loopback wire. Frames sent by `a` pass through `drop_a`.
    struct Net {
        a: Link,
        b: Link,
        a_rx: Vec<Vec<u8>>,
        b_rx: Vec<Vec<u8>>,
        wire: VecDeque<(bool, Vec<u8>)>, // (sent by a, payload)
        drop_a: DropFn,
    }

    impl Net {
        fn new(a: &str, b: &str) -> Net {
            Net {
                a: Link::new(a),
                b: Link::new(b),
                a_rx: Vec::new(),
                b_rx: Vec::new(),
                wire: VecDeque::new(),
                drop_a: Box::new(|_| false),
            }
        }

        /// `from_a` says whether the actions belong to link a.
        fn apply(&mut self, from_a: bool, acts: Vec<Action>) {
            for act in acts {
                match act {
                    Action::Transmit(p) => {
                        assert!(p.len() <= MAX_PAYLOAD, "payload over 36 bytes");
                        if from_a && (self.drop_a)(&p) {
                            continue;
                        }
                        self.wire.push_back((from_a, p));
                    }
                    Action::Deliver(p) => {
                        if from_a { self.a_rx.push(p) } else { self.b_rx.push(p) }
                    }
                    Action::StateChanged(_) => {}
                }
            }
        }

        fn run(&mut self) {
            while let Some((from_a, p)) = self.wire.pop_front() {
                let acts = if from_a { self.b.on_receive(&p) } else { self.a.on_receive(&p) };
                self.apply(!from_a, acts);
            }
        }

        fn open(&mut self) {
            let peer = self.b.callsign.clone();
            let acts = self.a.connect(&peer);
            self.apply(true, acts);
            self.run();
        }

        fn send_a(&mut self, data: &[u8]) {
            let acts = self.a.send(data);
            self.apply(true, acts);
            self.run();
        }

        fn tick(&mut self, now: u64) {
            let aa = self.a.tick(now);
            self.apply(true, aa);
            let ba = self.b.tick(now);
            self.apply(false, ba);
            self.run();
        }
    }

    fn connected_pair() -> Net {
        let mut n = Net::new("AAA1", "BBB2");
        n.open();
        n
    }

    fn count_transmits(acts: &[Action]) -> usize {
        acts.iter().filter(|a| matches!(a, Action::Transmit(_))).count()
    }

    #[test]
    fn handshake_between_two_links() {
        let n = connected_pair();
        assert_eq!(n.a.state, LinkState::Connected { peer: "BBB2".into() });
        assert_eq!(n.b.state, LinkState::Connected { peer: "AAA1".into() });
    }

    #[test]
    fn data_is_delivered_in_order_and_split_to_frame_size() {
        let mut n = connected_pair();
        n.send_a(b"hello");
        n.send_a(b"world");
        assert_eq!(n.b_rx, vec![b"hello".to_vec(), b"world".to_vec()]);

        let blob: Vec<u8> = (0..100u8).collect();
        n.send_a(&blob);
        assert_eq!(n.b_rx[2..].concat(), blob);
        assert!(n.b_rx[2..].iter().all(|c| c.len() <= MAX_CHUNK));
    }

    #[test]
    fn lost_data_frame_is_retransmitted_after_tick() {
        let mut n = connected_pair();
        let mut dropped = false;
        n.drop_a = Box::new(move |p: &[u8]| {
            if !dropped && p[0] == TAG_DATA {
                dropped = true;
                true
            } else {
                false
            }
        });
        n.tick(1_000);
        n.send_a(b"lost?");
        assert!(n.b_rx.is_empty());
        n.tick(1_000 + DATA_TIMEOUT_MS - 1);
        assert!(n.b_rx.is_empty(), "retransmitted too early");
        n.tick(1_000 + DATA_TIMEOUT_MS);
        assert_eq!(n.b_rx, vec![b"lost?".to_vec()]);
    }

    #[test]
    fn disconnect_returns_to_idle() {
        let mut n = connected_pair();
        let acts = n.a.disconnect();
        n.apply(true, acts);
        n.run();
        assert_eq!(n.a.state, LinkState::Idle);
        assert_eq!(n.b.state, LinkState::Idle);
    }

    #[test]
    fn connect_retries_then_gives_up() {
        let mut a = Link::new("AAA1");
        let first = a.connect("BBB2");
        assert_eq!(count_transmits(&first), 1);
        let mut retries = 0;
        for k in 1..=6u64 {
            retries += count_transmits(&a.tick(k * CONNECT_TIMEOUT_MS));
        }
        assert_eq!(retries, MAX_CONNECT_RETRIES as usize);
        assert_eq!(a.state, LinkState::Idle);
    }

    #[test]
    fn window_limits_frames_in_flight() {
        let mut n = connected_pair();
        let acts = n.a.send(&vec![7u8; MAX_CHUNK * 20]);
        assert_eq!(count_transmits(&acts), WINDOW);
        n.apply(true, acts);
        n.run();
        assert_eq!(n.b_rx.len(), 20);
    }

    #[test]
    fn selective_repeat_buffers_out_of_order_and_delivers_in_order() {
        let mut n = connected_pair();
        let mut dropped = false;
        n.drop_a = Box::new(move |p: &[u8]| {
            if !dropped && p[0] == TAG_DATA && p[1] == 0 {
                dropped = true;
                true
            } else {
                false
            }
        });
        n.send_a(b"c0");
        n.send_a(b"c1");
        n.send_a(b"c2");
        assert!(n.b_rx.is_empty(), "frames 1 and 2 must wait for frame 0");
        n.tick(DATA_TIMEOUT_MS);
        assert_eq!(n.b_rx, vec![b"c0".to_vec(), b"c1".to_vec(), b"c2".to_vec()]);
    }

    #[test]
    fn sequence_numbers_wrap_modulo_256() {
        let mut n = connected_pair();
        for i in 0..300u32 {
            n.send_a(&[(i % 251) as u8]);
        }
        assert_eq!(n.b_rx.len(), 300);
        assert_eq!(n.b_rx[299], vec![(299 % 251) as u8]);
    }

    #[test]
    fn receiver_reports_fer_and_sender_suggests_modulation() {
        let mut n = connected_pair();
        assert_eq!(n.a.suggested_modulation(), DEFAULT_MOD_LEVEL);
        // b sees seq 0 then seq 3: seqs 1 and 2 are missing, so FER = 2 / 4 = 50%.
        n.b.on_receive(&encode_data(0, b"a"));
        let acts = n.b.on_receive(&encode_data(3, b"d"));
        let ack = acts
            .iter()
            .find_map(|a| match a {
                Action::Transmit(p) if p[0] == TAG_ACK => Some(p.clone()),
                _ => None,
            })
            .expect("ACK sent");
        assert_eq!(ack[3], 50);
        n.a.on_receive(&ack);
        assert_eq!(n.a.peer_frame_error_pct(), Some(50));
        assert_eq!(n.a.suggested_modulation(), 0);
    }

    #[test]
    fn modulation_thresholds() {
        let mut l = Link::new("AAA1");
        for (fer, level) in [(0u8, 3u8), (5, 2), (10, 1), (30, 0)] {
            l.peer_fer = Some(fer);
            assert_eq!(l.suggested_modulation(), level, "fer {fer}");
        }
    }
}
