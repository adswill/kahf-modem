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
const TAG_ADVERT: u8 = 0x04;
const TAG_BUSY: u8 = 0x06;
/// A connected link with nothing to send and nothing heard from the peer for this long is
/// closed (the peer left without a goodbye, or the goodbye was lost).
pub const LINK_IDLE_MS: u64 = 60_000;
/// A new caller may take over a link that has been quiet for this long.
const TAKEOVER_IDLE_MS: u64 = 15_000;
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
/// Interval of `Action::Metrics` while connected, in link time.
pub const METRICS_INTERVAL_MS: u64 = 5_000;
/// Number of most recent received frames the frame error rate is measured over.
pub const FER_WINDOW_FRAMES: usize = 50;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkState {
    Idle,
    Connecting { peer: String },
    Connected { peer: String },
    Disconnecting,
}

/// What the link layer asks the caller to do after an input.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Send this payload (one frame's worth of bytes, <= 36) on air.
    Transmit(Vec<u8>),
    /// Deliver this payload to the session layer.
    Deliver(Vec<u8>),
    StateChanged(LinkState),
    /// A station announced itself (no link needed). Relays pass it on while `ttl` lasts.
    Advert { call: String, ttl: u8, seq: u8, flags: u8, loc: Option<(f32, f32)> },
    /// Link quality snapshot, emitted from `tick` every `METRICS_INTERVAL_MS` while connected.
    Metrics(LinkMetrics),
}

/// Snapshot of link quality for a user interface.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkMetrics {
    pub state: LinkState,
    /// Peer callsign while connecting or connected.
    pub peer: Option<String>,
    /// DATA frames sent for the first time in the current session.
    pub frames_sent: u64,
    /// New DATA frames received (duplicates not counted) in the current session.
    pub frames_received: u64,
    /// DATA frames retransmitted in the current session (handshake retries not counted).
    pub retransmits: u64,
    /// Share of the last `FER_WINDOW_FRAMES` received frames that were lost, in percent.
    pub frame_error_pct: f32,
    /// SNR (dB) of what we receive, smoothed over recent frames, when a KAMI frame has been heard.
    pub own_snr_db: Option<f32>,
    /// SNR (dB) the peer last reported for what it receives from us.
    pub peer_snr_db: Option<f32>,
    /// Suggested modulation level: 0 = robust, 1 = normal, 2 = fast.
    pub suggested_modulation: u8,
    /// Text name of `suggested_modulation`: "robust", "normal" or "fast".
    pub suggested_modulation_name: &'static str,
}

#[derive(Clone, Debug)]
struct InFlight {
    seq: u8,
    payload: Vec<u8>,
    deadline: u64,
    retries: u32,
}

pub struct Link {
    /// Extra time added to every retransmit timer: the air time of a frame in both directions
    /// plus the peer's processing delay. Zero until `set_air_ms` is called.
    air_extra: u64,
    /// Stations that said they are busy, and until when (ms on our clock).
    busy_until: Vec<(String, u64)>,
    /// Clock at the last frame of the current exchange (sent or received).
    last_activity: u64,
    /// Data or an acknowledgement has been exchanged on this link (not just a connect).
    peer_active: bool,
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
    peer_snr: Option<f32>,
    /// Callsigns of stations heard in handshake frames, not yet handed to the caller.
    heard: Vec<String>,
    // Receiver side.
    rx_snr: Option<f32>,
    rcv_base: u8,
    rx_buf: Vec<Option<Vec<u8>>>,
    /// Highest sequence number accepted so far (used for gap-based FER).
    hi_seen: u8,
    rx_new: u32,
    rx_missing: u32,
    // Metrics. Counters restart with each session (see `begin_session`).
    frames_sent: u64,
    retransmits: u64,
    frames_received: u64,
    /// Outcomes of the last `FER_WINDOW_FRAMES` received frames (true = lost).
    fer_window: VecDeque<bool>,
    /// Link time at which the next `Action::Metrics` is due.
    metrics_due: u64,
}

impl Link {
    pub fn new(callsign: &str) -> Self {
        let mut l = Link {
            callsign: callsign.to_string(),
            state: LinkState::Idle,
            clock: 0,
            air_extra: 0,
            busy_until: Vec::new(),
            last_activity: 0,
            peer_active: false,
            peer_snr: None,
            heard: Vec::new(),
            rx_snr: None,
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
            frames_sent: 0,
            retransmits: 0,
            frames_received: 0,
            fer_window: VecDeque::new(),
            metrics_due: 0,
        };
        l.reset_arq();
        l
    }

    /// Start the handshake with `peer`. Returns nothing unless the link is Idle and both
    /// callsigns are 1..=15 bytes.
    /// Tell the link how long one frame takes on the air (ms) in the current mode. The retransmit
    /// timers grow by two frames (ours and the answer) plus a second for the peer's search.
    pub fn set_air_ms(&mut self, frame_ms: u64) {
        self.air_extra = 2 * frame_ms + 1000;
    }

    pub fn connect(&mut self, peer: &str) -> Vec<Action> {
        if self.state != LinkState::Idle || !valid_callsign(peer) || !valid_callsign(&self.callsign) {
            return Vec::new();
        }
        self.begin_session();
        self.hs_retries = 0;
        self.hs_deadline = self.clock + CONNECT_TIMEOUT_MS + self.air_extra;
        let mut out = Vec::new();
        // The station said it is busy: do not call into its busy exchange, wait it out.
        if let Some(until) = self.busy_for(peer) {
            self.hs_deadline = until;
        } else {
            out.push(Action::Transmit(encode_hs(TAG_CONNECT, &self.callsign, peer)));
        }
        self.set_state(
            LinkState::Connecting {
                peer: peer.to_string(),
            },
            &mut out,
        );
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
        if matches!(payload.first(), Some(&TAG_CONNECT) | Some(&TAG_CONNECT_ACK) | Some(&TAG_DISCONNECT)) {
            if let Some((from, _)) = decode_hs(payload) {
                self.note_heard(&from);
            }
        }
        match payload.first() {
            Some(&TAG_CONNECT) => self.on_connect(payload),
            Some(&TAG_CONNECT_ACK) => self.on_connect_ack(payload),
            Some(&TAG_DISCONNECT) => self.on_disconnect(payload),
            Some(&TAG_DATA) => {
                self.last_activity = self.clock;
                self.peer_active = true;
                self.on_data(payload)
            }
            Some(&TAG_ACK) => {
                self.last_activity = self.clock;
                self.peer_active = true;
                self.on_ack(payload)
            }
            Some(&TAG_BUSY) => self.on_busy(payload),
            Some(&TAG_ADVERT) => match decode_advert(payload) {
                Some((call, ttl, seq, flags, loc)) if call != self.callsign && valid_callsign(&call) => {
                    self.note_heard(&call);
                    vec![Action::Advert { call, ttl, seq, flags, loc }]
                }
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    /// Feed one outgoing data chunk from the session layer. Data longer than 34 bytes is
    /// split. Nothing is sent unless the link is Connected.
    pub fn send(&mut self, data: &[u8]) -> Vec<Action> {
        if !matches!(self.state, LinkState::Connected { .. }) {
            return Vec::new();
        }
        self.last_activity = self.clock;
        self.peer_active = true;
        for chunk in data.chunks(MAX_CHUNK) {
            self.tx_queue.push_back(chunk.to_vec());
        }
        let mut out = Vec::new();
        self.pump(&mut out);
        out
    }

    /// A burst has finished going out at `now_ms`. The wait for its answer starts now, not when
    /// it was queued: a slow channel or a long burst must not look like a lost frame, and
    /// resending while the other station is still answering collides with its reply.
    pub fn tx_finished(&mut self, now_ms: u64) {
        self.clock = self.clock.max(now_ms);
        let data_deadline = now_ms + DATA_TIMEOUT_MS + self.air_extra;
        for f in self.slots.iter_mut().flatten() {
            f.deadline = f.deadline.max(data_deadline);
        }
        if matches!(self.state, LinkState::Connecting { .. }) {
            self.hs_deadline = self.hs_deadline.max(now_ms + CONNECT_TIMEOUT_MS + self.air_extra);
        }
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
                        self.hs_deadline = now_ms + CONNECT_TIMEOUT_MS + self.air_extra;
                        out.push(Action::Transmit(encode_hs(TAG_CONNECT, &self.callsign, &peer)));
                    } else {
                        self.reset_arq();
                        self.set_state(LinkState::Idle, &mut out);
                    }
                }
            }
            LinkState::Connected { .. } => {
                if self.nothing_pending() && now_ms.saturating_sub(self.last_activity) >= LINK_IDLE_MS {
                    self.reset_arq();
                    self.set_state(LinkState::Idle, &mut out);
                    return out;
                }
                if now_ms >= self.metrics_due {
                    self.metrics_due = now_ms + METRICS_INTERVAL_MS;
                    out.push(Action::Metrics(self.metrics()));
                }
                let mut dead = false;
                for slot in self.slots.iter_mut() {
                    if let Some(f) = slot.as_mut() {
                        if now_ms >= f.deadline {
                            if f.retries >= MAX_DATA_RETRIES {
                                dead = true;
                                break;
                            }
                            f.retries += 1;
                            f.deadline = now_ms + DATA_TIMEOUT_MS + self.air_extra;
                            self.retransmits += 1;
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

    /// Record the SNR (dB) the receiver measured on a frame it has just decoded. It is smoothed
    /// and sent back to the peer in every ACK, so the peer can pick its mode.
    pub fn note_rx_snr(&mut self, snr_db: f32) {
        let x = snr_db.clamp(-30.0, 90.0);
        self.rx_snr = Some(self.rx_snr.map_or(x, |p| 0.6 * p + 0.4 * x));
    }

    /// Remember a station we heard, once per burst of frames.
    fn note_heard(&mut self, call: &str) {
        if call != self.callsign && valid_callsign(call) && self.heard.last().map(|c| c.as_str()) != Some(call) {
            if self.heard.len() >= 32 {
                self.heard.remove(0);
            }
            self.heard.push(call.to_string());
        }
    }

    /// Stations heard since the last call, oldest first. The caller reports them to the operator.
    pub fn take_heard(&mut self) -> Vec<String> {
        std::mem::take(&mut self.heard)
    }

    /// SNR (dB) the peer last reported for our transmissions, if it has reported one.
    pub fn peer_snr_db(&self) -> Option<f32> {
        self.peer_snr
    }

    /// SNR (dB) of what we receive from the peer, smoothed, if known.
    pub fn own_snr_db(&self) -> Option<f32> {
        self.rx_snr
    }

    /// Frame error rate (percent) the peer last reported for our transmissions.
    pub fn peer_frame_error_pct(&self) -> Option<u8> {
        self.peer_fer
    }

    /// Snapshot of the link's quality counters and suggested modulation.
    pub fn metrics(&self) -> LinkMetrics {
        let peer = match &self.state {
            LinkState::Connecting { peer } | LinkState::Connected { peer } => Some(peer.clone()),
            _ => None,
        };
        let lost = self.fer_window.iter().filter(|&&l| l).count();
        let frame_error_pct = if self.fer_window.is_empty() {
            0.0
        } else {
            lost as f32 * 100.0 / self.fer_window.len() as f32
        };
        let (level, name) = metric_modulation(self.suggested_modulation());
        LinkMetrics {
            state: self.state.clone(),
            peer,
            frames_sent: self.frames_sent,
            frames_received: self.frames_received,
            retransmits: self.retransmits,
            frame_error_pct,
            own_snr_db: self.rx_snr,
            peer_snr_db: self.peer_snr,
            suggested_modulation: level,
            suggested_modulation_name: name,
        }
    }

    /// Short user-facing description of the current state, e.g. "Connecting to BB1BBB,
    /// attempt 2 of 6" or "Connected to BB1BBB, resending data (retry 3 of 8)".
    pub fn describe(&self) -> String {
        match &self.state {
            LinkState::Idle => "Idle".to_string(),
            LinkState::Connecting { peer } => format!(
                "Connecting to {peer}, attempt {} of {}",
                self.hs_retries + 1,
                MAX_CONNECT_RETRIES + 1
            ),
            LinkState::Connected { peer } => {
                let worst = self.slots.iter().flatten().map(|f| f.retries).max().unwrap_or(0);
                if worst > 0 {
                    format!("Connected to {peer}, resending data (retry {worst} of {MAX_DATA_RETRIES})")
                } else {
                    format!("Connected to {peer}")
                }
            }
            LinkState::Disconnecting => "Disconnecting".to_string(),
        }
    }

    // ---- handshake -------------------------------------------------------

    fn on_connect(&mut self, p: &[u8]) -> Vec<Action> {
        let Some((from, to)) = decode_hs(p) else {
            return Vec::new();
        };
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
                self.last_activity = self.clock;
                out.push(Action::Transmit(encode_hs(
                    TAG_CONNECT_ACK,
                    &self.callsign,
                    &from,
                )));
            }
            // Already in an exchange with someone else: say so once, with how long to wait,
            // instead of letting the caller retry into it.
            LinkState::Connected { .. } if self.quiet_for_takeover() => {
                // The old peer has gone quiet with nothing pending: the new caller gets the link.
                self.reset_arq();
                self.accept(&from, &mut out);
            }
            _ => {
                let wait = self.busy_wait_s();
                out.push(Action::Transmit(encode_busy(&self.callsign, &from, wait)));
            }
        }
        out
    }

    fn accept(&mut self, from: &str, out: &mut Vec<Action>) {
        self.last_activity = self.clock;
        self.peer_active = false;
        self.begin_session();
        self.set_state(
            LinkState::Connected {
                peer: from.to_string(),
            },
            out,
        );
        out.push(Action::Transmit(encode_hs(TAG_CONNECT_ACK, &self.callsign, from)));
    }

    fn nothing_pending(&self) -> bool {
        self.tx_queue.is_empty() && self.slots.iter().all(|s| s.is_none())
    }

    fn quiet_for_takeover(&self) -> bool {
        self.peer_active && self.nothing_pending() && self.clock.saturating_sub(self.last_activity) >= TAKEOVER_IDLE_MS
    }

    /// Seconds a caller should stay away: a fixed part plus what we still have to send.
    fn busy_wait_s(&self) -> u8 {
        let queued = self.tx_queue.len() + self.slots.iter().flatten().count();
        (8 + 2 * queued as u64).min(90) as u8
    }

    fn busy_for(&self, peer: &str) -> Option<u64> {
        self.busy_until.iter().find(|(c, u)| c == peer && *u > self.clock).map(|(_, u)| *u)
    }

    /// A busy notice. Everyone who hears it leaves that station alone for a while, whoever it
    /// was addressed to (a hidden caller learns from the notice the others hear).
    fn on_busy(&mut self, p: &[u8]) -> Vec<Action> {
        let Some((from, to, wait)) = decode_busy(p) else { return Vec::new() };
        if from == self.callsign {
            return Vec::new();
        }
        self.note_heard(&from);
        // A small spread by callsign, so the callers do not all return at the same moment.
        let spread: u64 = to.bytes().map(u64::from).sum::<u64>() % 7 * 400;
        let until = self.clock + wait as u64 * 1000 + spread;
        self.busy_until.retain(|(c, u)| c != &from && *u > self.clock);
        self.busy_until.push((from.clone(), until));
        if let LinkState::Connecting { peer } = &self.state {
            if *peer == from {
                self.hs_deadline = self.hs_deadline.max(until);
            }
        }
        Vec::new()
    }

    fn on_connect_ack(&mut self, p: &[u8]) -> Vec<Action> {
        let Some((from, to)) = decode_hs(p) else {
            return Vec::new();
        };
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
        let Some((from, to)) = decode_hs(p) else {
            return Vec::new();
        };
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
                for _ in 1..ahead {
                    self.push_fer_sample(true);
                }
                self.push_fer_sample(false);
                self.hi_seen = seq;
            }
            self.rx_new += 1;
            self.frames_received += 1;
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
        out.push(Action::Transmit(vec![
            TAG_ACK,
            self.rcv_base,
            bitmap,
            self.measured_fer_pct(),
            // SNR report: half dB steps from -30 dB, 0 means unknown.
            self.rx_snr.map_or(0, |s| ((s + 30.0) * 2.0).round().clamp(1.0, 255.0) as u8),
        ]));
        out
    }

    fn on_ack(&mut self, p: &[u8]) -> Vec<Action> {
        if !matches!(self.state, LinkState::Connected { .. }) || p.len() < 4 {
            return Vec::new();
        }
        let (base, bitmap, fer) = (p[1], p[2], p[3]);
        self.peer_fer = Some(fer.min(100));
        if let Some(&b) = p.get(4) {
            self.peer_snr = (b > 0).then(|| b as f32 / 2.0 - 30.0);
        }
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
            let Some(chunk) = self.tx_queue.pop_front() else {
                break;
            };
            let seq = self.next_seq;
            self.slots[seq as usize % WINDOW] = Some(InFlight {
                seq,
                payload: chunk.clone(),
                deadline: self.clock + DATA_TIMEOUT_MS + self.air_extra,
                retries: 0,
            });
            out.push(Action::Transmit(encode_data(seq, &chunk)));
            self.frames_sent += 1;
            self.next_seq = seq.wrapping_add(1);
        }
    }

    fn push_fer_sample(&mut self, lost: bool) {
        self.fer_window.push_back(lost);
        if self.fer_window.len() > FER_WINDOW_FRAMES {
            self.fer_window.pop_front();
        }
    }

    /// Start a new connection: ARQ state and session counters all restart.
    fn begin_session(&mut self) {
        self.reset_arq();
        self.frames_sent = 0;
        self.retransmits = 0;
        self.frames_received = 0;
        self.fer_window.clear();
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
        self.peer_snr = None;
        self.rx_snr = None;
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
        if let LinkState::Connected { .. } = s {
            self.metrics_due = self.clock + METRICS_INTERVAL_MS;
        }
        self.state = s.clone();
        out.push(Action::StateChanged(s));
    }
}

/// Map the internal four-step level (0..=MAX_MOD_LEVEL) onto the reported three-step scale.
fn metric_modulation(raw: u8) -> (u8, &'static str) {
    match raw {
        0 => (0, "robust"),
        1 | 2 => (1, "normal"),
        _ => (2, "fast"),
    }
}

fn valid_callsign(c: &str) -> bool {
    !c.is_empty() && c.len() <= MAX_CALLSIGN
}

/// Advert flag: the announcing station runs a relay.
pub const ADVERT_RELAY: u8 = 1;
/// Advert flag: four bytes of position follow the callsign (hundredths of a degree).
pub const ADVERT_LOCATION: u8 = 2;
/// How many more relays an advert may be passed through.
pub const ADVERT_TTL: u8 = 4;

/// An announcement of a station, for everyone who can hear it: tag, hops left, sequence number,
/// flags, callsign.
pub fn encode_advert(call: &str, ttl: u8, seq: u8, flags: u8, loc: Option<(f32, f32)>) -> Vec<u8> {
    let flags = if loc.is_some() { flags | ADVERT_LOCATION } else { flags & !ADVERT_LOCATION };
    let mut v = vec![TAG_ADVERT, ttl, seq, flags, call.len() as u8];
    v.extend_from_slice(call.as_bytes());
    if let Some((lat, lon)) = loc {
        v.extend_from_slice(&((lat * 100.0).round().clamp(-9000.0, 9000.0) as i16).to_be_bytes());
        v.extend_from_slice(&((lon * 100.0).round().clamp(-18000.0, 18000.0) as i16).to_be_bytes());
    }
    v
}

type DecodedAdvert = (String, u8, u8, u8, Option<(f32, f32)>);

fn decode_advert(p: &[u8]) -> Option<DecodedAdvert> {
    let n = *p.get(4)? as usize;
    let flags = *p.get(3)?;
    let extra = if flags & ADVERT_LOCATION != 0 { 4 } else { 0 };
    if n == 0 || n > MAX_CALLSIGN || p.len() != 5 + n + extra {
        return None;
    }
    let call = String::from_utf8(p[5..5 + n].to_vec()).ok()?;
    let loc = (extra == 4).then(|| {
        let lat = i16::from_be_bytes([p[5 + n], p[6 + n]]) as f32 / 100.0;
        let lon = i16::from_be_bytes([p[7 + n], p[8 + n]]) as f32 / 100.0;
        (lat, lon)
    });
    Some((call, p[1], p[2], flags, loc))
}

fn encode_busy(from: &str, to: &str, wait_s: u8) -> Vec<u8> {
    let mut v = encode_hs(TAG_BUSY, from, to);
    v.push(wait_s);
    v
}

fn decode_busy(p: &[u8]) -> Option<(String, String, u8)> {
    let (&wait, body) = p.split_last()?;
    let (from, to) = decode_hs(body)?;
    Some((from, to, wait))
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
                        if from_a {
                            self.a_rx.push(p)
                        } else {
                            self.b_rx.push(p)
                        }
                    }
                    Action::StateChanged(_) | Action::Metrics(_) | Action::Advert { .. } => {}
                }
            }
        }

        fn run(&mut self) {
            while let Some((from_a, p)) = self.wire.pop_front() {
                let acts = if from_a {
                    self.b.on_receive(&p)
                } else {
                    self.a.on_receive(&p)
                };
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

    /// The 18-byte text from the offline case, through session encode, the link, then decode.
    #[test]
    fn session_18_byte_text_round_trips_through_link() {
        use crate::session::{self, Message};
        let msg = Message::Chat {
            from: "AA1AAA".into(),
            to: "BB1BBB".into(),
            text: "sent while offline".into(),
        };
        let mut n = connected_pair();
        for chunk in session::encode(&msg) {
            n.send_a(&chunk);
        }
        assert_eq!(session::decode(&n.b_rx), Some(msg));
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

    #[test]
    fn metrics_count_after_loopback_exchange() {
        let mut n = connected_pair();
        let idle = n.a.metrics();
        assert_eq!(idle.state, LinkState::Connected { peer: "BBB2".into() });
        assert_eq!(idle.peer.as_deref(), Some("BBB2"));
        assert_eq!(
            (idle.frames_sent, idle.frames_received, idle.retransmits),
            (0, 0, 0)
        );
        assert_eq!(idle.frame_error_pct, 0.0);
        assert_eq!(idle.suggested_modulation, 1);
        assert_eq!(idle.suggested_modulation_name, "normal");

        n.send_a(b"hello");
        n.send_a(b"world");
        let a = n.a.metrics();
        let b = n.b.metrics();
        assert_eq!(a.frames_sent, 2);
        assert_eq!(a.frames_received, 0);
        assert_eq!(a.retransmits, 0);
        assert_eq!(b.frames_sent, 0);
        assert_eq!(b.frames_received, 2);
        assert_eq!(b.peer.as_deref(), Some("AAA1"));
        assert_eq!(b.frame_error_pct, 0.0);
    }

    #[test]
    fn metrics_count_retransmits_and_loss_window() {
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
        n.send_a(b"lost?");
        n.tick(DATA_TIMEOUT_MS);
        let a = n.a.metrics();
        assert_eq!((a.frames_sent, a.retransmits), (1, 1));
        assert_eq!(n.b.metrics().frames_received, 1);

        // b sees seq 0 then seq 3: two of four frames lost, so 50%.
        let mut l = connected_pair();
        l.b.on_receive(&encode_data(0, b"a"));
        l.b.on_receive(&encode_data(3, b"d"));
        assert_eq!(l.b.metrics().frame_error_pct, 50.0);
    }

    #[test]
    fn metrics_emitted_every_five_seconds_while_connected() {
        let mut n = connected_pair();
        let emitted = |acts: &[Action]| acts.iter().any(|a| matches!(a, Action::Metrics(_)));
        assert!(!emitted(&n.a.tick(METRICS_INTERVAL_MS - 1)));
        assert!(emitted(&n.a.tick(METRICS_INTERVAL_MS)));
        assert!(!emitted(&n.a.tick(METRICS_INTERVAL_MS + 1)));
        assert!(emitted(&n.a.tick(2 * METRICS_INTERVAL_MS)));

        let mut idle = Link::new("AAA1");
        assert!(!emitted(&idle.tick(10 * METRICS_INTERVAL_MS)));
    }

    #[test]
    fn describe_idle_connecting_and_connected() {
        let mut a = Link::new("AAA1");
        assert_eq!(a.describe(), "Idle");

        a.connect("BBB2");
        assert_eq!(a.describe(), "Connecting to BBB2, attempt 1 of 6");
        a.tick(CONNECT_TIMEOUT_MS);
        assert_eq!(a.describe(), "Connecting to BBB2, attempt 2 of 6");

        let n = connected_pair();
        assert_eq!(n.a.describe(), "Connected to BBB2");
    }

    #[test]
    fn describe_reports_data_retries_and_disconnecting() {
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
        n.send_a(b"x");
        // Check while the frame is still unacknowledged, before the retransmit is delivered.
        let acts = n.a.tick(DATA_TIMEOUT_MS);
        assert_eq!(n.a.describe(), "Connected to BBB2, resending data (retry 1 of 8)");
        n.apply(true, acts);
        n.run();
        assert_eq!(n.b_rx, vec![b"x".to_vec()]);
        assert_eq!(n.a.describe(), "Connected to BBB2");

        n.a.state = LinkState::Disconnecting;
        assert_eq!(n.a.describe(), "Disconnecting");
    }

    #[test]
    fn snr_travels_in_the_ack_and_is_smoothed() {
        let mut n = connected_pair();
        assert_eq!(n.a.peer_snr_db(), None);
        n.b.note_rx_snr(10.0);
        n.b.note_rx_snr(20.0);
        let own = n.b.own_snr_db().unwrap();
        assert!((own - 14.0).abs() < 0.01, "smoothed {own}");
        // b answers a data frame with an ACK that carries its SNR.
        n.send_a(b"x");
        let heard = n.a.peer_snr_db().expect("a hears b's snr");
        assert!((heard - own).abs() <= 0.5, "heard {heard} vs {own}");
        let m = n.a.metrics();
        assert_eq!(m.peer_snr_db, Some(heard));
        // An ACK from an old peer without the byte leaves the value alone.
        let _ = n.a.on_receive(&[TAG_ACK, 1, 0, 0]);
        assert_eq!(n.a.peer_snr_db(), Some(heard));
    }

    #[test]
    fn stations_heard_in_handshakes_are_reported_once() {
        let mut n = connected_pair();
        assert_eq!(n.a.take_heard(), vec!["BBB2".to_string()]);
        assert_eq!(n.b.take_heard(), vec!["AAA1".to_string()]);
        assert!(n.a.take_heard().is_empty(), "taken once");
        // Our own callsign and junk are never reported.
        let own = encode_hs(TAG_CONNECT, "AAA1", "BBB2");
        let _ = n.a.on_receive(&own);
        assert!(n.a.take_heard().is_empty());
    }
}

#[cfg(test)]
mod advert_tests {
    use super::*;

    #[test]
    fn an_advert_is_heard_without_a_link() {
        let mut l = Link::new("KB2BRV");
        let acts = l.on_receive(&encode_advert("KA1ALF", 3, 7, ADVERT_RELAY, Some((25.2, 55.27))));
        assert_eq!(
            acts,
            vec![Action::Advert {
                call: "KA1ALF".into(), ttl: 3, seq: 7, flags: ADVERT_RELAY | ADVERT_LOCATION, loc: Some((25.2, 55.27))
            }]
        );
        let plain = l.on_receive(&encode_advert("KC3CCC", 1, 2, 0, None));
        assert!(matches!(&plain[0], Action::Advert { loc: None, .. }));
        assert_eq!(l.take_heard(), vec!["KA1ALF".to_string(), "KC3CCC".to_string()]);
        // Our own advert coming back, and nonsense, do nothing.
        assert!(l.on_receive(&encode_advert("KB2BRV", 3, 7, 0, None)).is_empty());
        assert!(l.on_receive(&[TAG_ADVERT, 1, 1, 0, 9, b'x']).is_empty());
    }
}
