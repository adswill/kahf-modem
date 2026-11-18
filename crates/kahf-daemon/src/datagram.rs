//! Chat on the air as a bundle frame stream (MESH-SPEC 8.1 to 8.3), without a link.
//!
//! A chat message is cut into session chunks of 26 bytes. Each chunk goes into one codeword as a
//! `bundle::Kind::Message` frame (9 byte header, one sequence byte, the chunk), addressed to the
//! next hop by its 24 bit hash. A station that is not the next hop drops the frame after the
//! header. The receiver answers once per burst with an `Ack` frame (a bitmap of the codewords it
//! got, the message sequence and its own SNR), in the place the burst lists it: the first
//! recipient answers 300 ms after the burst, the next 300 ms later, and so on. The sender repeats
//! only the chunks that were not acknowledged. A message the receiver already delivered (an
//! answer was lost) is acknowledged again and not shown again.
//!
//! Mail and files still use the connection (`link.rs`), until the delivery work (WP8) moves them.

use super::*;
use kahf_core::bundle::{self, Header, Kind};

/// Bytes of a session chunk in one codeword: the 36 byte payload less the 9 byte header and the
/// sequence byte.
const DG_CHUNK: usize = bundle::BODY_MAX - 1;
/// Longest chat text sent this way (bytes); longer ones use the link.
const DG_TEXT_MAX: usize = 400;
/// Tries before the message is given back to the outbox with its own back-off.
const DG_TRIES: u32 = 6;
/// How long to wait for the acknowledgement after the burst ends.
const DG_ACK_WAIT: Duration = Duration::from_millis(3_500);
/// How long a delivered message is remembered, so a repeat is not shown twice.
const DG_REMEMBER: Duration = Duration::from_secs(600);
/// Flag bit in the frame header: the message is private (also inside the session chunks).
const FLAG_PRIVATE: u8 = 0x01;

/// Messages sent together in one burst (each to its own recipient, or several to one).
const DG_PARALLEL: usize = 4;

/// The state of the datagram path.
pub(crate) struct DgState {
    /// This station's 24 bit address.
    pub my: u32,
    seq: u8,
    /// Messages being sent, in outbox order.
    sends: Vec<DgSend>,
    /// The burst on the air: (message oid, chunk number) by codeword position.
    burst: Vec<(u64, usize)>,
    /// Recipients of the burst on the air that have answered.
    answered: Vec<u32>,
    /// When to give up waiting for the answers to the burst on the air.
    deadline: Option<Instant>,
    /// No burst before this (random back-off, so two stations that start together part).
    hold_until: Option<Instant>,
    /// Chunks received per (source, sequence).
    rx: HashMap<(u32, u8), Vec<Vec<u8>>>,
    /// Delivered messages: (source, sequence) and when.
    done: VecDeque<((u32, u8), Instant)>,
    /// The burst being received.
    rx_burst: Option<RxBurst>,
    /// Acknowledgements waiting for their place in the answer order.
    acks: Vec<(Instant, Vec<u8>)>,
}

struct RxBurst {
    headers: Vec<Header>,
    /// Codewords of this burst for this station: (source, sequence, position).
    mine: Vec<(u32, u8, u8)>,
    snr: Option<f32>,
}

struct DgSend {
    oid: u64,
    /// The message's own destination (the next hop may be a relay).
    dest_h: u32,
    peer: String,
    peer_h: u32,
    seq: u8,
    private: bool,
    chunks: Vec<Vec<u8>>,
    acked: Vec<bool>,
    /// Bursts in a row with no answer from the recipient.
    tries: u32,
}

impl DgState {
    /// The first message being sent: its destination and how many chunks are not acknowledged.
    pub(crate) fn first_send(&self) -> Option<(u32, usize)> {
        self.sends.first().map(|s| (s.dest_h, s.acked.iter().filter(|a| !**a).count()))
    }

    #[cfg(test)]
    pub(crate) fn waiting_acks(&self) -> usize {
        self.acks.len()
    }

    pub fn new(callsign: &str) -> Self {
        DgState {
            my: bundle::h24(callsign),
            seq: 0,
            sends: Vec::new(),
            burst: Vec::new(),
            answered: Vec::new(),
            deadline: None,
            hold_until: None,
            rx: HashMap::new(),
            done: VecDeque::new(),
            rx_burst: None,
            acks: Vec::new(),
        }
    }
}

/// An outbox item that goes out as a datagram: a chat of at most `DG_TEXT_MAX` bytes.
pub(crate) fn is_dg_item(item: &OutItem) -> bool {
    item.payload.first().is_some_and(|t| t & 0x7F == 0) && item.payload.len() <= DG_TEXT_MAX + 80
}

fn is_ack_payload(p: &[u8]) -> bool {
    p.first() == Some(&(Kind::Ack as u8))
}

impl Worker {
    /// One pass of the datagram path: acknowledgements that are due, then the sender.
    pub(crate) fn datagram_step(&mut self) {
        self.dg_flush_acks();
        if tx_refusal(&self.shared.flags).is_some() {
            return;
        }
        let now = self.clock.now();
        match self.dg.deadline {
            // A burst is out: wait for the answers.
            Some(d) if now < d => return,
            Some(_) => return self.dg_retry(),
            None => {}
        }
        self.dg_drop_rerouted();
        self.dg_start();
        if !self.dg.sends.is_empty() {
            self.dg_send_burst();
        }
    }

    /// A message whose route changed (the station was not reached, a relay was chosen) starts again.
    fn dg_drop_rerouted(&mut self) {
        let routes: Vec<Option<String>> = self.dg.sends.iter().map(|s| self.route_peer_of(s.oid)).collect();
        let mut keep = routes.iter();
        self.dg.sends.retain(|s| keep.next().and_then(|r| r.as_ref()).is_some_and(|p| p.eq_ignore_ascii_case(&s.peer)));
    }

    /// Takes chats from the outbox, up to `DG_PARALLEL`, when the station is free to send them.
    fn dg_start(&mut self) {
        if !self.transmit_allowed() || !self.relay_slot_open() {
            return;
        }
        if !matches!(self.link_state, LinkState::Idle) || self.tx.is_some() || !self.bursts.is_empty() || !self.control.is_empty() {
            return;
        }
        if self.access_retry_at.is_some_and(|t| self.clock.now() < t) {
            return;
        }
        let was_empty = self.dg.sends.is_empty();
        while self.dg.sends.len() < DG_PARALLEL {
            let now = self.now_ms();
            let taken: Vec<u64> = self.dg.sends.iter().map(|s| s.oid).collect();
            let cooling: Vec<String> = self
                .auto_fails
                .iter()
                .filter(|(_, (_, until))| self.clock.now() < *until)
                .map(|(k, _)| k.clone())
                .collect();
            let (via, queued) = (&self.via, &self.queued);
            let wanted = |oid: u64, item: &OutItem| {
                let peer = via.get(&oid).cloned().or_else(|| queued.get(&oid).map(|(d, _)| d.clone()));
                is_dg_item(item) && !taken.contains(&oid) && peer.is_some_and(|p| !cooling.contains(&p.to_uppercase()))
            };
            let Some((oid, item)) = self.outbox.next_matching(now, wanted) else { break };
            let Some(peer) = self.route_peer_of(oid) else { break };
            let Some((msg, private)) = decode_item(&item.payload) else {
                self.outbox.cancel(oid);
                self.log("dropped an unreadable outbox item");
                self.persist_outbox();
                continue;
            };
            let chunks = match session::try_encode_sized(&msg, None, private, DG_CHUNK) {
                Ok(c) if c.len() <= 4096 => c,
                Ok(_) => {
                    self.fail_item(oid, "message too long".to_string());
                    continue;
                }
                Err(e) => {
                    self.fail_item(oid, e.to_string());
                    continue;
                }
            };
            self.dg.seq = self.dg.seq.wrapping_add(1);
            let n = chunks.len();
            self.dg.sends.push(DgSend {
                oid,
                dest_h: bundle::h24(dest_of(&msg)),
                peer_h: bundle::h24(&peer),
                peer,
                seq: self.dg.seq,
                private,
                chunks,
                acked: vec![false; n],
                tries: 0,
            });
        }
        if was_empty && !self.dg.sends.is_empty() {
            let wait = self.dg_jitter(0, 800);
            self.dg.hold_until = Some(self.clock.now() + wait);
        }
    }

    /// A random wait of up to `base_ms` doubled `tries` times (at most 16 s), at least a quarter of it.
    pub(crate) fn dg_jitter(&self, tries: u32, base_ms: u64) -> Duration {
        use kahf_core::select::{RandomSource, SplitMix};
        let seed = if self.clock.is_virtual() {
            fnv(&self.callsign)
        } else {
            SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1)
        } ^ ((self.dg.seq as u64) << 32 | tries as u64)
            ^ self.now_ms();
        let top = (base_ms << tries.min(5)).min(16_000) as f32;
        let u = SplitMix(seed).next_unit();
        Duration::from_millis((top * (0.25 + 0.75 * u)) as u64)
    }

    /// Sends one burst with every chunk that is not acknowledged yet (up to a full burst), if the
    /// channel allows. Chunks of messages for different stations share the burst.
    fn dg_send_burst(&mut self) {
        if self.tx.is_some() || !self.bursts.is_empty() || !self.control.is_empty() {
            return;
        }
        // A relay that referees the lane (or is one) decides when; otherwise listen before talk.
        let mut max_air: Option<f64> = None;
        match self.ref_gate() {
            RefGate::Csma => {
                if self.dg.hold_until.is_some_and(|t| self.clock.now() < t) {
                    return;
                }
            }
            RefGate::Wait => return,
            RefGate::Go { max_air_s } => max_air = Some(max_air_s),
        }
        let Some(first) = self.dg.sends.first().map(|s| s.oid) else { return };
        // A send opens the radio when nothing else has (listening starts it too).
        if let Err(e) = self.ensure_radio() {
            self.log(e);
            return;
        }
        // In a slot the referee gave there is no random back-off: it is an answer-class send.
        match self.pick_channel(Some(first), false, max_air.is_some()) {
            Pick::Go => {}
            Pick::Wait => return,
            Pick::GaveUp => {
                self.dg.sends.clear();
                return;
            }
        }
        let my = self.dg.my;
        let mut burst: Vec<(u64, usize)> = Vec::new();
        'fill: for s in &self.dg.sends {
            for ci in (0..s.chunks.len()).filter(|i| !s.acked[*i]) {
                if burst.len() == bundle::BUNDLE_MAX {
                    break 'fill;
                }
                burst.push((s.oid, ci));
            }
        }
        if let Some(limit) = max_air {
            self.pick_level();
            while burst.len() > 1 && self.burst_secs(burst.len()) > limit {
                burst.pop();
            }
            if self.burst_secs(burst.len()) > limit {
                return; // not even one codeword fits what is left of the slot
            }
        }
        let frames: Result<Vec<Vec<u8>>, String> = burst
            .iter()
            .enumerate()
            .filter_map(|(pos, (oid, ci))| self.dg.sends.iter().find(|s| s.oid == *oid).map(|s| (pos, s, *ci)))
            .map(|(pos, s, ci)| {
                let mut body = vec![s.seq];
                body.extend_from_slice(&s.chunks[ci]);
                bundle::Frame {
                    header: Header {
                        kind: Kind::Message,
                        flags: if s.private { FLAG_PRIVATE } else { 0 },
                        hops: 0,
                        next_hop: s.peer_h,
                        source: my,
                        index: pos as u8,
                        count: burst.len() as u8,
                    },
                    body,
                }
                .encode()
            })
            .collect();
        match frames {
            Ok(frames) => {
                let n = frames.len();
                self.dg.burst = burst;
                self.dg.answered.clear();
                self.push_burst(&frames);
                let air = burst_len_for(MODULATIONS[self.tx_level as usize], n) as f64 / RATE as f64 * self.slow() as f64;
                // Own transmit time (it may wait behind another burst), then every answer in turn.
                let peers = self.dg_burst_peers().len() as u32;
                let referee_wait = if max_air.is_some() { Duration::from_secs(36) } else { Duration::ZERO };
                let wait = referee_wait + Duration::from_secs_f64(air + 1.0) + DG_ACK_WAIT * self.slow() + (self.dg_ack_air() + Duration::from_millis(bundle::ANSWER_GAP_MS)) * peers;
                self.dg.deadline = Some(self.clock.now() + wait);
                if max_air.is_some() {
                    self.ref_data_sent();
                }
            }
            Err(e) => {
                self.dg.burst.clear();
                for s in std::mem::take(&mut self.dg.sends) {
                    self.fail_item(s.oid, e.clone());
                }
            }
        }
    }

    /// The recipients of the burst on the air, in the order they were listed.
    fn dg_burst_peers(&self) -> Vec<u32> {
        let mut peers: Vec<u32> = Vec::new();
        for (oid, _) in &self.dg.burst {
            if let Some(s) = self.dg.sends.iter().find(|s| s.oid == *oid) {
                if !peers.contains(&s.peer_h) {
                    peers.push(s.peer_h);
                }
            }
        }
        peers
    }

    /// The answers did not all come in time: whoever stayed silent has one more try used.
    fn dg_retry(&mut self) {
        self.dg.deadline = None;
        self.ref_op_reset();
        let silent: Vec<u32> = self.dg_burst_peers().into_iter().filter(|p| !self.dg.answered.contains(p)).collect();
        let mut gave_up: Vec<DgSend> = Vec::new();
        for s in self.dg.sends.iter_mut().filter(|s| silent.contains(&s.peer_h)) {
            s.tries += 1;
        }
        let mut keep = Vec::new();
        for s in std::mem::take(&mut self.dg.sends) {
            if s.tries >= DG_TRIES {
                gave_up.push(s);
            } else {
                keep.push(s);
            }
        }
        self.dg.sends = keep;
        let tries = self.dg.sends.iter().map(|s| s.tries).max().unwrap_or(0);
        for s in gave_up {
            let key = s.peer.to_uppercase();
            let fails = self.auto_fails.get(&key).map_or(0, |f| f.0) + 1;
            let pause = AUTO_RETRY_BASE.saturating_mul(1 << (fails - 1).min(5)).min(Duration::from_secs(900));
            self.auto_fails.insert(key, (fails, self.clock.now() + pause));
            self.fail_item(s.oid, format!("{} did not answer", s.peer));
        }
        if !silent.is_empty() && !self.dg.sends.is_empty() {
            let wait = self.dg_jitter(tries, 1_500);
            self.dg.hold_until = Some(self.clock.now() + wait);
            self.log("No answer yet. Sending the missing parts again.");
        }
    }

    /// An acknowledgement arrived for a message of the burst on the air.
    fn dg_on_ack(&mut self, f: &bundle::Frame) {
        if self.dg.deadline.is_none() || f.body.len() < 2 {
            return;
        }
        let seq = f.body[1];
        let Some(si) = self.dg.sends.iter().position(|s| s.peer_h == f.header.source && s.seq == seq) else { return };
        let oid = self.dg.sends[si].oid;
        for pos in bundle::acked(f) {
            if let Some((o, ci)) = self.dg.burst.get(pos as usize).copied() {
                if o == oid {
                    self.dg.sends[si].acked[ci] = true;
                }
            }
        }
        self.dg.sends[si].tries = 0;
        if !self.dg.answered.contains(&f.header.source) {
            self.dg.answered.push(f.header.source);
        }
        if let Some(b) = f.body.get(2).filter(|b| **b > 0) {
            self.link.set_peer_snr(Some(*b as f32 / 2.0 - 30.0));
        }
        if self.dg.sends[si].acked.iter().all(|a| *a) {
            let s = self.dg.sends.remove(si);
            self.auto_fails.remove(&s.peer.to_uppercase());
            let public = self.tags.get(&s.oid).map(|t| t.0).unwrap_or(s.oid);
            self.outbox.done(s.oid);
            self.bb_item_done(s.oid, true);
            self.record_forward(public);
            self.persist_outbox();
            self.ref_op_reset();
        }
        // Everyone in the burst answered: the next burst may go at once.
        if self.dg_burst_peers().iter().all(|p| self.dg.answered.contains(p)) || self.dg.burst.iter().all(|(o, _)| !self.dg.sends.iter().any(|s| s.oid == *o)) {
            self.dg.deadline = None;
        }
    }

    /// A frame with a bundle tag (0x30 to 0x37) was decoded.
    pub(crate) fn dg_on_frame(&mut self, payload: &[u8], snr: Option<f32>) {
        let Some(frame) = bundle::Frame::decode(payload) else { return };
        let h = frame.header;
        // Everyone listens to the header of every codeword: the answer order needs the list.
        let burst = self.dg.rx_burst.get_or_insert(RxBurst { headers: Vec::new(), mine: Vec::new(), snr });
        burst.headers.push(h);
        burst.snr = snr.or(burst.snr);
        if !frame.is_for(self.dg.my) {
            return;
        }
        match h.kind {
            Kind::Message => self.dg_on_message(&frame),
            Kind::Ack => self.dg_on_ack(&frame),
            Kind::Beacon | Kind::Request | Kind::Grant => self.ref_on_frame(&frame),
            _ => {}
        }
    }

    fn dg_on_message(&mut self, f: &bundle::Frame) {
        let Some((seq, chunk)) = f.body.split_first() else { return };
        let key = (f.header.source, *seq);
        if let Some(b) = self.dg.rx_burst.as_mut() {
            b.mine.push((key.0, key.1, f.header.index));
        }
        let now = self.clock.now();
        self.dg.done.retain(|(_, at)| now.duration_since(*at) < DG_REMEMBER);
        if self.dg.done.iter().any(|(k, _)| *k == key) {
            return; // already delivered: acknowledged again, not shown again
        }
        let chunks = self.dg.rx.entry(key).or_default();
        if !chunks.iter().any(|c| c == chunk) {
            chunks.push(chunk.to_vec());
        }
        if chunks.len() > 4096 {
            self.dg.rx.remove(&key);
            return;
        }
        match guard("session decode", || session::decode_signed(&self.dg.rx[&key])) {
            Ok(Some(signed)) => {
                self.dg.rx.remove(&key);
                self.dg.done.push_back((key, now));
                let from = sender_of(&signed.message).to_string();
                self.note_heard_recent(&from, self.link.own_snr_db());
                self.emit(Event::Heard { call: from, snr_db: self.link.own_snr_db() });
                self.on_signed(signed);
            }
            Ok(None) => {}
            Err(e) => {
                self.dg.rx.remove(&key);
                self.log(e);
            }
        }
        // Old half-received messages are not kept for ever.
        if self.dg.rx.len() > 64 {
            self.dg.rx.clear();
        }
    }

    /// The audio of a burst ended: answer it, in the place the burst lists this station.
    pub(crate) fn dg_burst_end(&mut self) {
        let Some(b) = self.dg.rx_burst.take() else { return };
        if b.mine.is_empty() {
            return;
        }
        let slot = bundle::answer_slot(&b.headers, self.dg.my).unwrap_or(0);
        // The MAC already waits one short gap after the burst for an answer; each station ahead
        // in the list adds the length of its answer and one answer gap. (MESH-SPEC 8.3 gives 300 ms
        // only: that holds when an answer is much shorter than a burst with its preamble is today.)
        let due = self.clock.now() + (self.dg_ack_air() + Duration::from_millis(bundle::ANSWER_GAP_MS)) * slot as u32;
        let snr_byte = b.snr.map_or(0u8, |s| ((s + 30.0) * 2.0).round().clamp(1.0, 255.0) as u8);
        let mut seen: Vec<(u32, u8)> = Vec::new();
        for (src, seq, _) in &b.mine {
            if seen.contains(&(*src, *seq)) {
                continue;
            }
            seen.push((*src, *seq));
            let got: Vec<u8> = b.mine.iter().filter(|(s, q, _)| s == src && q == seq).map(|(_, _, i)| *i).collect();
            let mut map = 0u8;
            for i in &got {
                map |= 1 << i;
            }
            let ack = bundle::Frame {
                header: Header { kind: Kind::Ack, flags: 0, hops: 0, next_hop: *src, source: self.dg.my, index: 0, count: 1 },
                body: vec![map, *seq, snr_byte],
            };
            if let Ok(p) = ack.encode() {
                if self.is_ref_relay() {
                    // The referee answers everyone together, with its next beacon.
                    self.refs.acks.push(p);
                } else {
                    self.dg.acks.push((due, p));
                }
            }
        }
    }

    /// How long one acknowledgement is on the air at the present speed (one codeword and its preamble).
    fn dg_ack_air(&self) -> Duration {
        Duration::from_secs_f64(burst_len_for(MODULATIONS[self.tx_level as usize], 1) as f64 / RATE as f64 * self.slow() as f64)
    }

    /// Puts acknowledgements whose time has come on the control queue (sent as answers).
    fn dg_flush_acks(&mut self) {
        let now = self.clock.now();
        let mut i = 0;
        while i < self.dg.acks.len() {
            if self.dg.acks[i].0 <= now {
                let (_, p) = self.dg.acks.remove(i);
                self.control.push_back(vec![p]);
            } else {
                i += 1;
            }
        }
    }
}

/// Control bursts that answer something (sent after the short answer gap).
pub(crate) fn is_answer_payload(p: &[u8]) -> bool {
    matches!(p.first(), Some(0x11) | Some(0x02) | Some(0x06)) || is_ack_payload(p)
}
