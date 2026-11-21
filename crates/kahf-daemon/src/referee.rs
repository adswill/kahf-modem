//! The relay as referee of its lane, and the operator that obeys it (MESH-SPEC 5.3).
//!
//! The relay keeps a 12 s epoch of six 2 s slots on UTC. Slot 0: its **beacon** (window, load,
//! epoch, its callsign) together with the answers to the data it took last epoch. Slots 1 and 2:
//! the **request window**. Slot 3: the **grants**. Slots 4 and 5: **data**. An operator that
//! hears the beacon lays the slot grid on the burst it heard (no UTC needed), picks one of the
//! window slots at random and sends a one-codeword request. After a grant it sends its message
//! in exactly the granted slots. Nobody else sends then (it heard the grant, or it is the relay's
//! own downlink in a slot nobody was granted). A lost request or grant: the operator backs off
//! for a random number of epochs inside a window that doubles up to 8.
//!
//! Without a beacon (no relay heard) a station falls back to listen-before-talk (`datagram.rs`).
//!
//! The referee works on the lane the radio is on. Grants name a lane, but a grant to another lane
//! needs the radio to follow it: that waits for the planner (WP6).

use super::*;
use kahf_core::bundle::{self, Frame, Kind};
use kahf_core::referee as rf;

const SLOT_MS: u64 = 2_000;
const EPOCH_MS: u64 = SLOT_MS * rf::SLOTS;
const WINDOW: u8 = rf::WINDOW_MIN;
/// Slot of the grants (after the request window).
const GRANT_SLOT: u64 = WINDOW as u64 + 1;
/// A burst starts this long after its slot begins, and no later than `SEND_LATE_MS`.
const SEND_AT_MS: u64 = 100;
const SEND_LATE_MS: u64 = 700;
/// A beacon older than this no longer counts as a relay in range.
const BEACON_FRESH_MS: u64 = 40_000;
/// Requests, grants and beacons go out in this mode: one codeword fits a slot (1.8 s in a lane).
const REF_MODE: Modulation = Modulation::Kami1a;

#[derive(Clone)]
struct RelayView {
    call: String,
    h: u32,
    /// The relay's epoch counter at the beacon heard last.
    epoch: u64,
    /// When that epoch's slot 0 began, on this station's clock (ms).
    slot0_ms: u64,
    heard_ms: u64,
}

enum Op {
    Idle,
    /// A request is planned (or sent) for window slot time `at_ms` of epoch `epoch`.
    Asking { at_ms: u64, epoch: u64, slot0_ms: u64, sent: bool },
    Granted { start_ms: u64, end_ms: u64 },
    /// The data went out; the answer comes in a later beacon.
    Await { until_ms: u64 },
}

pub(crate) struct RefState {
    unix_base_ms: u64,
    // relay side
    epoch: u64,
    beacon_sent: bool,
    grant_sent: bool,
    reqs: Vec<(u32, u32, u8)>,
    cursor: u64,
    /// Granted absolute slot ranges `start..end`.
    granted: Vec<(u64, u64)>,
    /// Answers to data taken, sent with the next beacon.
    pub acks: Vec<Vec<u8>>,
    // operator side
    relay: Option<RelayView>,
    /// When this station last told the relay it is here (ms on this station's clock), and when the next try is planned.
    last_here_ms: Option<u64>,
    here_at_ms: Option<u64>,
    /// Stations a relay heard saying they are here: (24 bit hash, SNR dB), waiting to go to the mesh.
    pub here_seen: Vec<(u32, i32)>,
    op: Op,
    cw: u32,
    quiet_until_ms: u64,
}

impl RefState {
    pub fn new() -> Self {
        RefState {
            unix_base_ms: (now_unix_secs().max(0) as u64) * 1000,
            epoch: u64::MAX,
            beacon_sent: false,
            grant_sent: false,
            reqs: Vec::new(),
            cursor: 0,
            granted: Vec::new(),
            acks: Vec::new(),
            relay: None,
            last_here_ms: None,
            here_at_ms: None,
            here_seen: Vec::new(),
            op: Op::Idle,
            cw: 1,
            quiet_until_ms: 0,
        }
    }
}

/// What the referee says about sending a datagram burst now.
pub(crate) enum RefGate {
    /// No relay in charge: listen before talk.
    Csma,
    Wait,
    /// Send now, at most `max_air_s` of audio.
    Go { max_air_s: f64 },
}

impl Worker {
    /// This station is a relay that is running, on a lane dial: it referees its lane.
    pub(crate) fn is_ref_relay(&self) -> bool {
        self.lanes.is_some()
            && self.shared.flags.relay_running.load(Ordering::SeqCst)
            && lock(&self.shared.relay).as_ref().is_some_and(|r| r.role != Role::Station)
    }

    /// The relay in charge of the lane (an operator's view): its callsign while its beacon is fresh.
    pub(crate) fn ref_relay_call(&self) -> Option<String> {
        if self.is_ref_relay() {
            return None;
        }
        let now = self.now_ms();
        self.refs.relay.as_ref().filter(|r| now.saturating_sub(r.heard_ms) < BEACON_FRESH_MS).map(|r| r.call.clone())
    }

    fn ref_utc_ms(&self) -> u64 {
        self.refs.unix_base_ms + self.now_ms()
    }

    /// Longest burst (seconds of audio) the run `free` slots can hold.
    fn ref_air_for(&self, free_slots: u64) -> f64 {
        (free_slots * SLOT_MS) as f64 / 1000.0 - 0.3
    }

    /// Audio seconds of a burst of `n` codewords at the present speed, silences included.
    pub(crate) fn burst_secs(&self, n: usize) -> f64 {
        burst_len_for(MODULATIONS[self.tx_level as usize], n) as f64 / RATE as f64 * self.slow() as f64 + 0.3
    }

    /// Sends `frames` as a planned burst in the referee's mode: only the lane check applies (the
    /// slot is the relay's or was granted), not the random back-off.
    fn ref_send(&mut self, frames: &[Vec<u8>]) -> bool {
        if self.tx.is_some() || !self.bursts.is_empty() || tx_refusal(&self.shared.flags).is_some() {
            return false;
        }
        if let Some(Err(why)) = self.own_lane_clear(true) {
            self.log(why);
            return false;
        }
        self.push_burst_mode(frames, Some(REF_MODE));
        true
    }

    /// Relay: the epoch cycle. Operator: tell the relay it is here, once in a while.
    pub(crate) fn referee_step(&mut self) {
        if !self.is_ref_relay() {
            self.here_step();
            return;
        }
        let t = self.ref_utc_ms();
        let (e, off) = (t / EPOCH_MS, t % EPOCH_MS);
        let (slot, in_slot) = (off / SLOT_MS, off % SLOT_MS);
        if e != self.refs.epoch {
            self.refs.epoch = e;
            self.refs.beacon_sent = false;
            self.refs.grant_sent = false;
            self.refs.granted.retain(|(_, end)| *end > e * rf::SLOTS);
        }
        if slot == 0 && (SEND_AT_MS..SEND_LATE_MS).contains(&in_slot) && !self.refs.beacon_sent {
            self.ref_send_beacon(e);
        }
        if slot == GRANT_SLOT && (SEND_AT_MS..SEND_LATE_MS).contains(&in_slot) && !self.refs.grant_sent {
            self.ref_send_grants(e);
        }
    }

    fn ref_send_beacon(&mut self, e: u64) {
        let load = (self.outbox.pending().len() * 16 + self.refs.reqs.len() * 16).min(255) as u8;
        let mut frames = vec![rf::encode_beacon(&self.callsign, WINDOW, load, e)];
        let room = bundle::BUNDLE_MAX - 1;
        let take = self.refs.acks.len().min(room);
        frames.extend(self.refs.acks.drain(..take));
        let ok = self.ref_send(&frames);
        if ok {
            self.shared.flags.ref_counts[0].fetch_add(1, Ordering::Relaxed);
            self.refs.beacon_sent = true;
        } else if self.refs.acks.len() < take {
            // Not sent: the answers stay for the next try.
            self.refs.acks.extend(frames.into_iter().skip(1));
        }
    }

    fn ref_send_grants(&mut self, e: u64) {
        self.refs.grant_sent = true;
        let reqs = std::mem::take(&mut self.refs.reqs);
        if reqs.is_empty() {
            return;
        }
        let me = bundle::h24(&self.callsign);
        let n = reqs.len().min(rf::GRANTS_MAX);
        let mut cursor = self.refs.cursor.max(e * rf::SLOTS + rf::FIRST_DATA_SLOT);
        let mut frames = Vec::new();
        let mut ranges = Vec::new();
        for (k, (op, _dest, slots)) in reqs.iter().take(n).enumerate() {
            let start = rf::next_data_slot(cursor, *slots);
            frames.push(rf::encode_grant(me, *op, 0, start, *slots, k as u8, n as u8));
            ranges.push((start, start + *slots as u64));
            cursor = start + *slots as u64;
        }
        let ok = self.ref_send(&frames);
        if ok {
            self.shared.flags.ref_counts[2].fetch_add(frames.len() as u64, Ordering::Relaxed);
            self.refs.cursor = cursor;
            self.refs.granted.extend(ranges);
        }
    }

    /// An operator that hears a relay says so, in a window slot, at most every ten minutes and not
    /// while it has something to send (a request tells the relay as well). The relay passes only the
    /// 24 bit hash on to the mesh, which is what presence is made of.
    fn here_step(&mut self) {
        const EVERY_MS: u64 = 600_000;
        if tx_refusal(&self.shared.flags).is_some() || !self.dg.is_idle() {
            return;
        }
        let Some(rel) = self.refs.relay.clone().filter(|_| self.ref_relay_call().is_some()) else {
            self.refs.here_at_ms = None;
            return;
        };
        let now = self.now_ms();
        if self.refs.last_here_ms.is_some_and(|t| now.saturating_sub(t) < EVERY_MS) {
            return;
        }
        match self.refs.here_at_ms {
            None => {
                let r = 1 + self.dg_jitter(0, 1_000).as_millis() as u64 % WINDOW as u64;
                let mut k = 0;
                loop {
                    let at = rel.slot0_ms + k * EPOCH_MS + r * SLOT_MS + SEND_AT_MS;
                    if at > now + 300 {
                        self.refs.here_at_ms = Some(at);
                        break;
                    }
                    k += 1;
                }
            }
            Some(at) if now >= at => {
                self.refs.here_at_ms = None;
                if now > at + SEND_LATE_MS {
                    return;
                }
                let snr = self.link.own_snr_db().map_or(0u8, |s| ((s + 30.0) * 2.0).round().clamp(1.0, 255.0) as u8);
                let frame = Frame {
                    header: bundle::Header { kind: Kind::Here, flags: 0, hops: 0, next_hop: rel.h, source: self.dg.my, index: 0, count: 1 },
                    body: vec![snr],
                }
                .encode();
                if let Ok(f) = frame {
                    if self.ref_send(&[f]) {
                        self.refs.last_here_ms = Some(now);
                    }
                }
            }
            Some(_) => {}
        }
    }

    /// A beacon, request or grant frame addressed to (or heard by) this station.
    pub(crate) fn ref_on_frame(&mut self, f: &Frame) {
        match f.header.kind {
            Kind::Beacon if !self.is_ref_relay() => {
                let Some((_, _, epoch, call)) = rf::decode_beacon(f) else { return };
                // The beacon's first codeword decoded a moment ago: its burst began that long
                // before. Lead silence, the lane burst of one codeword and the decode delay.
                let one = burst_len_for(self.kami_mode.unwrap_or(REF_MODE), 1) as f64 / RATE as f64 * self.slow() as f64;
                let back_ms = ((LEAD_SILENCE as f64 / RATE as f64 + one + 0.15) * 1000.0) as u64 + SEND_AT_MS;
                let now = self.now_ms();
                self.refs.relay = Some(RelayView {
                    h: f.header.source,
                    call,
                    epoch,
                    slot0_ms: now.saturating_sub(back_ms),
                    heard_ms: now,
                });
            }
            Kind::Request if self.is_ref_relay() => {
                // Only requests that fall in this epoch's window count.
                let t = self.ref_utc_ms() % EPOCH_MS;
                let slot = t / SLOT_MS;
                if !(1..=WINDOW as u64).contains(&slot) && t / SLOT_MS != GRANT_SLOT {
                    return;
                }
                if let Some((dest, slots, _)) = rf::decode_request(f) {
                    if !self.refs.reqs.iter().any(|(op, _, _)| *op == f.header.source) {
                        self.shared.flags.ref_counts[1].fetch_add(1, Ordering::Relaxed);
                        self.refs.reqs.push((f.header.source, dest, slots.clamp(1, 2)));
                    }
                }
            }
            Kind::Here if self.is_ref_relay() => {
                let snr = f.body.first().filter(|b| **b > 0).map_or(0, |b| (*b as f32 / 2.0 - 30.0).round() as i32);
                self.shared.flags.ref_counts[3].fetch_add(1, Ordering::Relaxed);
                self.refs.here_seen.retain(|(h, _)| *h != f.header.source);
                self.refs.here_seen.push((f.header.source, snr));
            }
            Kind::Grant if !self.is_ref_relay() => {
                let Some((_, count, abs)) = rf::decode_grant(f) else { return };
                if let Op::Asking { epoch, slot0_ms, sent: true, .. } = self.refs.op {
                    let from_epoch = (abs as i64 - (epoch * rf::SLOTS) as i64).max(0) as u64;
                    let start_ms = slot0_ms + from_epoch * SLOT_MS + SEND_AT_MS;
                    self.refs.op = Op::Granted { start_ms, end_ms: start_ms - SEND_AT_MS + count as u64 * SLOT_MS };
                    self.refs.cw = 1;
                }
            }
            _ => {}
        }
    }

    /// The referee's word on sending a datagram burst now (see `RefGate`).
    pub(crate) fn ref_gate(&mut self) -> RefGate {
        if self.is_ref_relay() {
            return self.ref_relay_gate();
        }
        let Some(rel) = self.refs.relay.clone().filter(|_| self.ref_relay_call().is_some()) else {
            self.refs.op = Op::Idle;
            return RefGate::Csma;
        };
        let now = self.now_ms();
        match self.refs.op {
            Op::Idle => {
                if now < self.refs.quiet_until_ms {
                    return RefGate::Wait;
                }
                // The next window slot that is still ahead, in a random one of the window slots.
                let r = 1 + self.dg_jitter(0, 1_000).as_millis() as u64 % WINDOW as u64;
                let mut k = 0;
                loop {
                    let slot0 = rel.slot0_ms + k * EPOCH_MS;
                    let at = slot0 + r * SLOT_MS + SEND_AT_MS;
                    if at > now + 300 {
                        self.refs.op = Op::Asking { at_ms: at, epoch: rel.epoch + k, slot0_ms: slot0, sent: false };
                        break;
                    }
                    k += 1;
                }
                RefGate::Wait
            }
            Op::Asking { at_ms, epoch, slot0_ms, sent } => {
                if !sent {
                    if now < at_ms {
                        return RefGate::Wait;
                    }
                    if now > at_ms + SEND_LATE_MS {
                        self.refs.op = Op::Idle; // missed the slot: plan again
                        return RefGate::Wait;
                    }
                    let (dest, slots) = self.ref_request_content();
                    let req = rf::encode_request(self.dg.my, rel.h, dest, slots, 0);
                    let ok = self.ref_send(&[req]);
                    if ok {
                        self.refs.op = Op::Asking { at_ms, epoch, slot0_ms, sent: true };
                    }
                    return RefGate::Wait;
                }
                // Sent: the grant comes in slot 3. None by the end of it: the request was lost.
                if now > slot0_ms + (GRANT_SLOT + 1) * SLOT_MS + 400 {
                    self.refs.cw = (self.refs.cw * 2).min(rf::BACKOFF_MAX);
                    // Stay quiet for a random number of whole epochs, up to `cw` (1 s of jitter stands for one epoch).
                    let epochs = self.dg_jitter(0, 1_000 * self.refs.cw as u64).as_millis() as u64 / 1_000;
                    self.refs.quiet_until_ms = now + epochs * EPOCH_MS;
                    self.refs.op = Op::Idle;
                }
                RefGate::Wait
            }
            Op::Granted { start_ms, end_ms } => {
                if now < start_ms {
                    RefGate::Wait
                } else if now + 600 > end_ms || now > start_ms + SEND_LATE_MS {
                    self.refs.op = Op::Idle; // the slot is gone
                    RefGate::Wait
                } else {
                    RefGate::Go { max_air_s: (end_ms - now) as f64 / 1000.0 - 0.2 }
                }
            }
            Op::Await { until_ms } => {
                if now > until_ms {
                    self.refs.op = Op::Idle;
                }
                RefGate::Wait
            }
        }
    }

    /// The data went out in the granted slots: the answer will come with a later beacon.
    pub(crate) fn ref_data_sent(&mut self) {
        if !self.is_ref_relay() {
            self.refs.op = Op::Await { until_ms: self.now_ms() + 3 * EPOCH_MS };
        }
    }

    /// The datagram sender gave up or was answered: the operator may ask again.
    pub(crate) fn ref_op_reset(&mut self) {
        self.refs.op = Op::Idle;
    }

    /// What the next request says: the destination and the data slots the first burst needs.
    fn ref_request_content(&mut self) -> (u32, u8) {
        let Some(s) = self.dg.first_send() else { return (0, 1) };
        let (dest_h, left) = s;
        let n = left.min(bundle::BUNDLE_MAX);
        let slots = (self.burst_secs(n) / (SLOT_MS as f64 / 1000.0)).ceil() as u64;
        (dest_h, slots.clamp(1, 2) as u8)
    }

    /// The relay's own downlink: only in a data slot nobody was granted, and only what fits.
    fn ref_relay_gate(&mut self) -> RefGate {
        let t = self.ref_utc_ms();
        let (e, off) = (t / EPOCH_MS, t % EPOCH_MS);
        let (slot, in_slot) = (off / SLOT_MS, off % SLOT_MS);
        if slot < rf::FIRST_DATA_SLOT || in_slot >= SEND_LATE_MS {
            return RefGate::Wait;
        }
        let abs = e * rf::SLOTS + slot;
        if self.refs.granted.iter().any(|(a, b)| abs >= *a && abs < *b) {
            return RefGate::Wait;
        }
        // Only the first data slot, and only when the next one is free too: the station that
        // receives the downlink answers right after it, and that answer needs air of its own
        // (it would otherwise land in a granted slot, or on the next beacon).
        if slot != rf::FIRST_DATA_SLOT || self.refs.granted.iter().any(|(a, b)| abs + 1 >= *a && abs + 1 < *b) {
            return RefGate::Wait;
        }
        RefGate::Go { max_air_s: self.ref_air_for(1) - in_slot as f64 / 1000.0 }
    }
}
