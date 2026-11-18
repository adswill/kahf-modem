//! The relay as referee of a calling lane (MESH-SPEC.md 5.3).
//!
//! Per 12 s epoch of six 2 s slots on the calling lane:
//!
//! - slot 0: the relay's **beacon**: names the operator window (how many request slots) and the load;
//! - slots 1 to W: the **operator window**. An operator with something to send picks one slot at
//!   random and sends a **request** (who for, how big). Two requests in one slot are both lost;
//! - slot W+1: the relay's **grant** burst: for each request it decoded, the data slots (on the
//!   relay's lane) in which that operator, and only that operator, may transmit.
//!
//! Operators cannot hear each other (hidden stations) but all hear the relay, so a grant is also
//! the virtual carrier sense: nobody sends data except in its own granted slots. After a lost
//! request (no grant came) an operator backs off: a window of epochs that doubles up to 8, with
//! a random draw inside it.
//!
//! This module is the logic and a slot-level simulation of it, using the real frame encoding of
//! `bundle.rs`. The daemon does not run it yet, and no radio is involved.

use crate::bundle::{self, Frame, Header, Kind};

pub const SLOTS: u64 = 6;
pub const WINDOW_MIN: u8 = 2;
pub const WINDOW_MAX: u8 = 4;
pub const BACKOFF_MAX: u32 = 8;
/// Grants the relay can fit in one burst.
pub const GRANTS_MAX: usize = bundle::BUNDLE_MAX;

struct Rng(u64);
impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) % n.max(1)
    }
}

/// A request body: destination hash (3 bytes), size in data slots, urgency.
pub fn encode_request(me: u32, relay: u32, dest: u32, slots: u8, urgency: u8) -> Vec<u8> {
    let mut body = dest.to_be_bytes()[1..].to_vec();
    body.extend([slots, urgency]);
    Frame { header: Header { kind: Kind::Request, flags: 0, hops: 0, next_hop: relay, source: me, index: 0, count: 1 }, body }
        .encode()
        .expect("request fits")
}

/// A grant: data slots `first..first + count` counted from the start of epoch `epoch`.
pub fn encode_grant(relay: u32, to: u32, lane: u8, abs_slot: u64, count: u8, index: u8, of: u8) -> Vec<u8> {
    let mut body = vec![lane, count];
    body.extend((abs_slot as u32).to_be_bytes());
    Frame { header: Header { kind: Kind::Grant, flags: 0, hops: 0, next_hop: to, source: relay, index, count: of }, body }
        .encode()
        .expect("grant fits")
}

/// A beacon: the relay's callsign hash is the source; the body is the operator window (slots),
/// the load (0 to 255) and the epoch counter (the relay's UTC epoch), so an operator can lay the
/// slot grid on the burst it heard.
pub fn encode_beacon(relay_call: &str, window: u8, load: u8, epoch: u64) -> Vec<u8> {
    let mut body = vec![window, load];
    body.extend((epoch as u32).to_be_bytes());
    body.extend(relay_call.trim().to_uppercase().bytes().take(bundle::BODY_MAX - 6));
    Frame { header: Header { kind: Kind::Beacon, flags: 0, hops: 0, next_hop: 0, source: bundle::h24(relay_call), index: 0, count: 1 }, body }
        .encode()
        .expect("beacon fits")
}

/// `(window, load, epoch, relay callsign)` of a beacon frame.
pub fn decode_beacon(f: &Frame) -> Option<(u8, u8, u64, String)> {
    (f.header.kind == Kind::Beacon && f.body.len() > 6).then(|| {
        (
            f.body[0],
            f.body[1],
            u32::from_be_bytes([f.body[2], f.body[3], f.body[4], f.body[5]]) as u64,
            String::from_utf8_lossy(&f.body[6..]).into_owned(),
        )
    })
}

/// `(destination hash, data slots, urgency)` of a request frame.
pub fn decode_request(f: &Frame) -> Option<(u32, u8, u8)> {
    (f.header.kind == Kind::Request && f.body.len() >= 5)
        .then(|| (u32::from_be_bytes([0, f.body[0], f.body[1], f.body[2]]), f.body[3], f.body[4]))
}

/// `(lane, slots, first absolute slot)` of a grant frame.
pub fn decode_grant(f: &Frame) -> Option<(u8, u8, u64)> {
    (f.header.kind == Kind::Grant && f.body.len() >= 6)
        .then(|| (f.body[0], f.body[1], u32::from_be_bytes([f.body[2], f.body[3], f.body[4], f.body[5]]) as u64))
}

/// Epoch slots 0 to 3 belong to the relay and the request window; slots 4 and 5 carry data.
pub const FIRST_DATA_SLOT: u64 = 4;

/// The first absolute slot at or after `cursor` where `count` data slots fit inside one epoch.
pub fn next_data_slot(cursor: u64, count: u8) -> u64 {
    let count = (count as u64).clamp(1, SLOTS - FIRST_DATA_SLOT);
    let mut a = cursor;
    loop {
        let s = a % SLOTS;
        if s >= FIRST_DATA_SLOT && s + count <= SLOTS {
            return a;
        }
        a = if s < FIRST_DATA_SLOT { a - s + FIRST_DATA_SLOT } else { a - s + SLOTS + FIRST_DATA_SLOT };
    }
}

/// What an operator holds to send.
#[derive(Clone, Debug)]
pub struct Message {
    pub dest: u32,
    pub slots: u8,
}

#[derive(Debug)]
struct Operator {
    id: u32,
    queue: Vec<Message>,
    /// Absolute data slots this operator was granted (it sends only in these).
    granted: Vec<u64>,
    /// Epoch from which it may try again, and the current backoff window.
    wait_until: u64,
    cw: u32,
    asked: Option<(u64, u64)>,
    /// Probability (percent) that it misses a given relay burst.
    deaf_pct: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub requests: u64,
    pub collisions: u64,
    pub grants: u64,
    pub delivered: u64,
    /// Data transmissions: (absolute slot, operator).
    pub data: Vec<(u64, u32)>,
    /// Every transmission on the calling lane: (absolute slot, transmitter).
    pub calling: Vec<(u64, u32)>,
    pub last_delivery_epoch: u64,
}

pub struct Cell {
    relay: u32,
    ops: Vec<Operator>,
    rng: Rng,
    /// Next free absolute data slot on the relay's lane.
    cursor: u64,
    load: u8,
    pub stats: Stats,
    total: usize,
}

impl Cell {
    /// `load[i]` messages for operator `i`; operator ids are 100+i. `deaf_pct` of relay bursts
    /// are missed by each operator.
    pub fn new(seed: u64, relay: u32, messages: &[Vec<Message>], deaf_pct: u64) -> Self {
        Cell {
            relay,
            ops: messages
                .iter()
                .enumerate()
                .map(|(i, q)| Operator { id: 100 + i as u32, queue: q.clone(), granted: vec![], wait_until: 0, cw: 1, asked: None, deaf_pct })
                .collect(),
            rng: Rng(seed),
            cursor: 0,
            load: 0,
            stats: Stats::default(),
            total: messages.iter().map(|q| q.len()).sum(),
        }
    }

    pub fn done(&self) -> bool {
        self.stats.delivered as usize == self.total
    }

    /// Run one epoch.
    pub fn epoch(&mut self, e: u64) {
        self.send_due(e);
        let base = e * SLOTS;
        // Slot 0: the beacon names the window; every operator hears it (or misses it).
        let w = (WINDOW_MIN + self.load / 32).min(WINDOW_MAX);
        let beacon = Frame {
            header: Header { kind: Kind::Beacon, flags: 0, hops: 0, next_hop: 0, source: self.relay, index: 0, count: 1 },
            body: vec![w, self.load],
        }
        .encode()
        .expect("beacon fits");
        self.stats.calling.push((base, self.relay));
        let mut asking: Vec<(usize, u64)> = Vec::new();
        for i in 0..self.ops.len() {
            let heard = self.rng.below(100) >= self.ops[i].deaf_pct;
            let f = Frame::decode(&beacon).expect("beacon decodes");
            if !heard || !f.is_for(self.ops[i].id) || f.header.kind != Kind::Beacon {
                continue;
            }
            let window = f.body[0] as u64;
            // An unanswered request from an earlier epoch: the request was lost, back off.
            if let Some((asked_epoch, _)) = self.ops[i].asked {
                if asked_epoch < e {
                    let o = &mut self.ops[i];
                    o.cw = (o.cw * 2).min(BACKOFF_MAX);
                    o.wait_until = e + self.rng.below(o.cw as u64);
                    o.asked = None;
                }
            }
            let o = &self.ops[i];
            if o.asked.is_none() && !o.queue.is_empty() && o.granted.is_empty() && e >= o.wait_until {
                let slot = 1 + self.rng.below(window);
                asking.push((i, slot));
                self.ops[i].asked = Some((e, slot));
            }
        }
        // Slots 1..=W: requests. A slot with exactly one request is decoded; two or more collide.
        let mut decoded: Vec<usize> = Vec::new();
        for slot in 1..=w as u64 {
            let here: Vec<usize> = asking.iter().filter(|(_, s)| *s == slot).map(|(i, _)| *i).collect();
            for &i in &here {
                self.stats.calling.push((base + slot, self.ops[i].id));
                self.stats.requests += 1;
            }
            match here.len() {
                0 => {}
                1 => decoded.push(here[0]),
                _ => self.stats.collisions += here.len() as u64,
            }
        }
        self.load = (self.stats.collisions.min(200) as u8).saturating_mul(1);
        // Slot W+1: the grants, in the order the requests were decoded.
        let first_data = self.cursor.max(base + SLOTS);
        let mut cursor = first_data;
        let mut grant_frames: Vec<Vec<u8>> = Vec::new();
        let n = decoded.len().min(GRANTS_MAX);
        for (k, &i) in decoded.iter().take(n).enumerate() {
            let me = self.ops[i].id;
            let req = encode_request(me, self.relay, self.ops[i].queue[0].dest, self.ops[i].queue[0].slots, 0);
            let rf = Frame::decode(&req).expect("request decodes");
            debug_assert!(rf.is_for(self.relay));
            let count = self.ops[i].queue[0].slots;
            grant_frames.push(encode_grant(self.relay, me, 0, cursor, count, k as u8, n as u8));
            cursor += count as u64;
        }
        if !grant_frames.is_empty() {
            self.stats.calling.push((base + w as u64 + 1, self.relay));
        }
        for gf in &grant_frames {
            let f = Frame::decode(gf).expect("grant decodes");
            let Some(i) = self.ops.iter().position(|o| f.is_for(o.id)) else { continue };
            self.stats.grants += 1;
            if self.rng.below(100) < self.ops[i].deaf_pct {
                // Missed the grant: those data slots go unused; the operator will ask again.
                continue;
            }
            let start = u32::from_be_bytes([f.body[2], f.body[3], f.body[4], f.body[5]]) as u64;
            let o = &mut self.ops[i];
            o.granted = (start..start + f.body[1] as u64).collect();
            o.cw = 1;
            o.asked = None;
        }
        self.cursor = cursor;
    }

    /// Data: each operator sends in its own granted slots, in the epoch each slot falls in.
    fn send_due(&mut self, e: u64) {
        for o in self.ops.iter_mut() {
            while let Some(&s) = o.granted.first() {
                if s / SLOTS > e {
                    break;
                }
                o.granted.remove(0);
                self.stats.data.push((s, o.id));
                if o.granted.is_empty() {
                    o.queue.remove(0);
                    self.stats.delivered += 1;
                    self.stats.last_delivery_epoch = e;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::h24;
    use std::collections::HashSet;

    fn load(ops: usize, per: usize, seed: u64) -> Vec<Vec<Message>> {
        let mut r = Rng(seed);
        (0..ops).map(|_| (0..per).map(|_| Message { dest: h24("ZZ9ZZZ"), slots: 1 + r.below(3) as u8 }).collect()).collect()
    }

    fn run(cell: &mut Cell, max_epochs: u64) -> u64 {
        for e in 0..max_epochs {
            cell.epoch(e);
            if cell.done() {
                return e;
            }
        }
        max_epochs
    }

    /// The invariants of any run.
    fn check(cell: &Cell) {
        let s = &cell.stats;
        // Data: no two operators ever send in the same slot of the relay lane.
        let mut seen = HashSet::new();
        for (slot, op) in &s.data {
            assert!(seen.insert(*slot), "two operators in data slot {slot} (one is {op})");
        }
        // Calling lane: beacons and grants are the relay's alone; operators send only in window slots.
        for (slot, who) in &s.calling {
            let in_epoch = slot % SLOTS;
            if *who == cell.relay {
                continue;
            }
            assert!(in_epoch >= 1 && in_epoch <= WINDOW_MAX as u64, "operator {who} sent in slot {in_epoch}");
        }
    }

    #[test]
    fn ten_operators_one_relay_every_message_is_delivered_and_no_data_overlaps() {
        for seed in 1..=20 {
            let mut cell = Cell::new(seed, h24("RELAY1"), &load(10, 3, seed), 0);
            let epochs = run(&mut cell, 400);
            assert!(cell.done(), "seed {seed}: {} of 30 delivered in {epochs} epochs", cell.stats.delivered);
            check(&cell);
            // Each message got exactly its slots.
            assert_eq!(cell.stats.delivered, 30);
        }
    }

    #[test]
    fn requests_collide_sometimes_and_backoff_resolves_it() {
        let mut collisions = 0;
        let mut worst = 0;
        for seed in 1..=20 {
            let mut cell = Cell::new(seed, h24("RELAY1"), &load(10, 2, seed), 0);
            worst = worst.max(run(&mut cell, 400));
            collisions += cell.stats.collisions;
            assert!(cell.done());
        }
        assert!(collisions > 0, "ten operators on 2 to 4 slots must collide now and then");
        assert!(worst < 120, "backoff should clear 20 messages within {worst} epochs");
    }

    #[test]
    fn operators_that_miss_beacons_and_grants_still_get_through() {
        for seed in 1..=10 {
            let mut cell = Cell::new(seed, h24("RELAY1"), &load(10, 2, seed), 15);
            run(&mut cell, 800);
            assert!(cell.done(), "seed {seed}: {} of 20", cell.stats.delivered);
            check(&cell);
        }
    }

    #[test]
    fn one_operator_alone_is_served_at_once() {
        let mut cell = Cell::new(1, h24("RELAY1"), &load(1, 1, 1), 0);
        let e = run(&mut cell, 50);
        assert!(cell.done() && e <= 4, "served in epoch {e}");
        assert_eq!(cell.stats.collisions, 0);
    }

    #[test]
    fn beacon_request_and_grant_round_trip_and_data_slots_are_found() {
        let b = Frame::decode(&encode_beacon("RELAY1", 2, 9, 1234567)).unwrap();
        assert_eq!(decode_beacon(&b), Some((2, 9, 1234567, "RELAY1".to_string())));
        assert_eq!(b.header.source, h24("RELAY1"));
        let r = Frame::decode(&encode_request(h24("OP1"), h24("RELAY1"), h24("ZZ9ZZZ"), 2, 1)).unwrap();
        assert_eq!(decode_request(&r), Some((h24("ZZ9ZZZ"), 2, 1)));
        let g = Frame::decode(&encode_grant(h24("RELAY1"), h24("OP1"), 5, 77, 2, 0, 1)).unwrap();
        assert_eq!(decode_grant(&g), Some((5, 2, 77)));
        // Slots 4 and 5 of each epoch of six.
        assert_eq!(next_data_slot(0, 2), 4);
        assert_eq!(next_data_slot(5, 2), 10);
        assert_eq!(next_data_slot(5, 1), 5);
        assert_eq!(next_data_slot(6, 1), 10);
        assert_eq!(next_data_slot(11, 2), 16);
        for c in 0..60 {
            for n in 1..=2u8 {
                let a = next_data_slot(c, n);
                assert!(a >= c && a % SLOTS >= FIRST_DATA_SLOT && a % SLOTS + n as u64 <= SLOTS);
            }
        }
    }

    #[test]
    fn request_and_grant_frames_name_who_must_act() {
        let relay = h24("RELAY1");
        let req = Frame::decode(&encode_request(h24("OP1"), relay, h24("ZZ9ZZZ"), 2, 0)).unwrap();
        assert!(req.is_for(relay) && !req.is_for(h24("OP2")));
        let g = Frame::decode(&encode_grant(relay, h24("OP1"), 3, 1234, 2, 0, 1)).unwrap();
        assert!(g.is_for(h24("OP1")) && !g.is_for(h24("OP2")) && !g.is_for(relay));
        assert_eq!(g.body[0], 3);
    }
}
