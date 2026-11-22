//! Relay to relay over the air (MESH-SPEC.md section 9): the forward rule.
//!
//! A hop frame is a `bundle` message frame whose next hop is a relay. Its body starts with the
//! origin relay id (3 bytes), a message id (2) and the destination (3); the rest is the payload
//! (19 bytes). Only the named next hop acts. It forwards if the hop count is under 4, the
//! message is not one it has already seen or sent itself (a loop), and a route exists. The relay
//! that sent a frame treats hearing the next hop forward it, with the hop count one higher, as
//! the acknowledgement: no separate ack frame. If that does not come in time the hop failed and
//! the message goes the internet way instead.
//!
//! Logic and a simulation only: the daemon does not send these frames, and the planner's HOP
//! grants (`planner::hop_grants`) are not published.

use crate::bundle::{Frame, Header, Kind, BODY_MAX, HOP_MAX};
use std::collections::HashMap;

pub const PAYLOAD_MAX: usize = BODY_MAX - 8;
/// How long a seen (origin, id) is remembered, and how long to wait for the passive ack (ms).
pub const SEEN_MS: u64 = 10 * 60_000;
pub const ACK_WAIT_MS: u64 = 30_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HopItem {
    pub origin: u32,
    pub id: u16,
    pub dest: u32,
    pub payload: Vec<u8>,
}

impl HopItem {
    fn body(&self) -> Vec<u8> {
        let mut b = self.origin.to_be_bytes()[1..].to_vec();
        b.extend(self.id.to_be_bytes());
        b.extend(&self.dest.to_be_bytes()[1..]);
        b.extend(&self.payload);
        b
    }

    fn parse(body: &[u8]) -> Option<HopItem> {
        if body.len() < 8 {
            return None;
        }
        let n = |i: usize| u32::from_be_bytes([0, body[i], body[i + 1], body[i + 2]]);
        Some(HopItem {
            origin: n(0),
            id: u16::from_be_bytes([body[3], body[4]]),
            dest: n(5),
            payload: body[8..].to_vec(),
        })
    }
}

/// Build the frame that sends `item` to relay `next`, from relay `me`, at hop count `hops`.
pub fn frame(me: u32, next: u32, hops: u8, item: &HopItem) -> Result<Vec<u8>, String> {
    if item.payload.len() > PAYLOAD_MAX {
        return Err(format!(
            "hop payload of {} bytes is over {PAYLOAD_MAX}",
            item.payload.len()
        ));
    }
    Frame {
        header: Header {
            kind: Kind::Message,
            flags: 0,
            hops,
            next_hop: next,
            source: me,
            index: 0,
            count: 1,
        },
        body: item.body(),
    }
    .encode()
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    /// Not for this relay, or not a hop frame: do nothing.
    Ignore,
    /// This relay is the destination: hand the payload to its own delivery.
    Deliver(HopItem),
    /// Send this frame to the next relay (hops already raised).
    Forward(Vec<u8>),
    /// The sender's passive acknowledgement: the next hop passed it on.
    Acked { id: u16 },
    /// Dropped, with the reason.
    Drop(Why),
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Why {
    Loop,
    HopLimit,
    NoRoute,
}

struct Pending {
    next: u32,
    hops: u8,
    at_ms: u64,
}

pub struct Forwarder {
    me: u32,
    seen: HashMap<(u32, u16), u64>,
    pending: HashMap<(u32, u16), Pending>,
}

impl Forwarder {
    pub fn new(me: u32) -> Self {
        Forwarder {
            me,
            seen: HashMap::new(),
            pending: HashMap::new(),
        }
    }

    /// A message of this relay's own, to start the hop. Returns the frame; the hop is
    /// pending until the passive ack arrives or `expired` reports it.
    pub fn originate(&mut self, next: u32, item: &HopItem, now_ms: u64) -> Result<Vec<u8>, String> {
        let f = frame(self.me, next, 0, item)?;
        self.seen.insert((item.origin, item.id), now_ms);
        self.pending.insert(
            (item.origin, item.id),
            Pending {
                next,
                hops: 0,
                at_ms: now_ms,
            },
        );
        Ok(f)
    }

    /// A frame was heard. `route(dest)` is the next relay towards `dest`, when there is one.
    pub fn on_frame(&mut self, payload: &[u8], now_ms: u64, route: impl Fn(u32) -> Option<u32>) -> Action {
        self.seen.retain(|_, t| now_ms.saturating_sub(*t) <= SEEN_MS);
        let Some(f) = Frame::decode(payload) else {
            return Action::Ignore;
        };
        if f.header.kind != Kind::Message {
            return Action::Ignore;
        }
        let Some(item) = HopItem::parse(&f.body) else {
            return Action::Ignore;
        };
        let key = (item.origin, item.id);
        // Passive ack: the relay we sent it to has passed it on, one hop further.
        if let Some(p) = self.pending.get(&key) {
            if f.header.source == p.next && f.header.hops == p.hops + 1 && f.header.next_hop != self.me {
                self.pending.remove(&key);
                return Action::Acked { id: item.id };
            }
        }
        if !f.is_for(self.me) {
            return Action::Ignore;
        }
        if item.origin == self.me || self.seen.contains_key(&key) {
            return Action::Drop(Why::Loop);
        }
        self.seen.insert(key, now_ms);
        if item.dest == self.me {
            return Action::Deliver(item);
        }
        // The limit is four transmissions: the origin's has hop count 0, so no frame may be made with 4.
        if f.header.hops + 1 >= HOP_MAX {
            return Action::Drop(Why::HopLimit);
        }
        let Some(next) = route(item.dest) else {
            return Action::Drop(Why::NoRoute);
        };
        let hops = f.header.hops + 1;
        match frame(self.me, next, hops, &item) {
            Ok(out) => {
                self.pending.insert(
                    key,
                    Pending {
                        next,
                        hops,
                        at_ms: now_ms,
                    },
                );
                Action::Forward(out)
            }
            Err(_) => Action::Ignore,
        }
    }

    /// Hops whose passive ack did not come in `ACK_WAIT_MS`: the message must go another way.
    pub fn expired(&mut self, now_ms: u64) -> Vec<(u32, u16)> {
        let dead: Vec<(u32, u16)> = self
            .pending
            .iter()
            .filter(|(_, p)| now_ms.saturating_sub(p.at_ms) > ACK_WAIT_MS)
            .map(|(k, _)| *k)
            .collect();
        for k in &dead {
            self.pending.remove(k);
        }
        dead
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Relays on a line or ring; `hears[a]` lists who hears a's transmissions. Routes come from a table.
    struct Net {
        fw: BTreeMap<u32, Forwarder>,
        hears: BTreeMap<u32, Vec<u32>>,
        /// next hop towards each destination, per relay.
        route: BTreeMap<(u32, u32), u32>,
        delivered: Vec<(u32, HopItem)>,
        acked: Vec<(u32, u16)>,
        drops: Vec<(u32, Why)>,
        sent: usize,
    }

    impl Net {
        fn new(ids: &[u32]) -> Self {
            Net {
                fw: ids.iter().map(|&i| (i, Forwarder::new(i))).collect(),
                hears: BTreeMap::new(),
                route: BTreeMap::new(),
                delivered: vec![],
                acked: vec![],
                drops: vec![],
                sent: 0,
            }
        }

        /// Everyone who hears `from` gets the frame, in rounds, until nothing is left to send.
        fn run(&mut self, from: u32, first: Vec<u8>, mut now: u64) {
            let mut queue = vec![(from, first)];
            let mut guard = 0;
            while let Some((tx, f)) = queue.pop() {
                guard += 1;
                assert!(guard < 200, "the frame never stops");
                self.sent += 1;
                now += 1000;
                for rx in self.hears.get(&tx).cloned().unwrap_or_default() {
                    let table = self.route.clone();
                    let act = self
                        .fw
                        .get_mut(&rx)
                        .unwrap()
                        .on_frame(&f, now, |d| table.get(&(rx, d)).copied());
                    match act {
                        Action::Forward(out) => queue.insert(0, (rx, out)),
                        Action::Deliver(i) => self.delivered.push((rx, i)),
                        Action::Acked { id } => self.acked.push((rx, id)),
                        Action::Drop(w) => self.drops.push((rx, w)),
                        Action::Ignore => {}
                    }
                }
            }
        }
    }

    fn item(origin: u32, dest: u32) -> HopItem {
        HopItem {
            origin,
            id: 7,
            dest,
            payload: b"hello".to_vec(),
        }
    }

    /// A -- B -- C -- D, each hears its neighbours only.
    fn line(n: u32) -> Net {
        let ids: Vec<u32> = (1..=n).collect();
        let mut net = Net::new(&ids);
        for i in 1..=n {
            let mut h = vec![];
            if i > 1 {
                h.push(i - 1);
            }
            if i < n {
                h.push(i + 1);
            }
            net.hears.insert(i, h);
            for d in 1..=n {
                if d > i {
                    net.route.insert((i, d), i + 1);
                } else if d < i {
                    net.route.insert((i, d), i - 1);
                }
            }
        }
        net
    }

    #[test]
    fn a_message_hops_three_relays_to_its_destination_and_each_sender_hears_the_next_forward() {
        let mut net = line(4);
        let it = item(1, 4);
        let f = net.fw.get_mut(&1).unwrap().originate(2, &it, 0).unwrap();
        net.run(1, f, 0);
        assert_eq!(net.delivered, vec![(4, it)]);
        // A hears B forward, B hears C forward: those are the acknowledgements.
        assert!(
            net.acked.contains(&(1, 7)) && net.acked.contains(&(2, 7)),
            "{:?}",
            net.acked
        );
        assert!(net.fw.get_mut(&1).unwrap().expired(60_000).is_empty());
        assert!(net.drops.iter().all(|(_, w)| *w != Why::HopLimit));
    }

    #[test]
    fn a_frame_not_for_a_relay_is_ignored_even_if_heard() {
        let mut net = line(3);
        net.hears.insert(1, vec![2, 3]); // C hears A directly too
        let f = net.fw.get_mut(&1).unwrap().originate(2, &item(1, 2), 0).unwrap();
        let act = net.fw.get_mut(&3).unwrap().on_frame(&f, 5, |_| Some(2));
        assert_eq!(act, Action::Ignore);
    }

    #[test]
    fn a_ring_cannot_make_a_message_circle() {
        // 1 -> 2 -> 3 -> 1 ...: each relay's route to dest 9 (nobody) points round the ring.
        let mut net = Net::new(&[1, 2, 3]);
        net.hears.insert(1, vec![2]);
        net.hears.insert(2, vec![3]);
        net.hears.insert(3, vec![1]);
        for (a, b) in [(1, 2), (2, 3), (3, 1)] {
            net.route.insert((a, 9), b);
        }
        let f = net.fw.get_mut(&1).unwrap().originate(2, &item(1, 9), 0).unwrap();
        net.run(1, f, 0);
        assert!(net.delivered.is_empty());
        assert!(
            net.drops.iter().any(|(r, w)| *r == 1 && *w == Why::Loop),
            "{:?}",
            net.drops
        );
        assert!(net.sent <= 4, "{} transmissions", net.sent);
    }

    #[test]
    fn four_hops_is_the_limit() {
        // Seven relays in a line: 1 to 7 needs six hops.
        let mut net = line(7);
        let f = net.fw.get_mut(&1).unwrap().originate(2, &item(1, 7), 0).unwrap();
        net.run(1, f, 0);
        assert!(net.delivered.is_empty(), "six transmissions must not get through");
        assert!(
            net.drops.contains(&(6, Why::HopLimit)) || net.drops.iter().any(|(_, w)| *w == Why::HopLimit),
            "{:?}",
            net.drops
        );
        // Six relays need five transmissions: over the limit. Five relays need four: allowed.
        let mut net = line(6);
        let f = net.fw.get_mut(&1).unwrap().originate(2, &item(1, 6), 0).unwrap();
        net.run(1, f, 0);
        assert!(
            net.delivered.is_empty(),
            "five transmissions must not get through"
        );
        let mut net = line(5);
        let f = net.fw.get_mut(&1).unwrap().originate(2, &item(1, 5), 0).unwrap();
        net.run(1, f, 0);
        assert_eq!(net.delivered.len(), 1);
    }

    #[test]
    fn no_route_is_a_drop_and_a_lost_hop_shows_as_expired() {
        let mut net = line(3);
        net.route.remove(&(2, 3));
        let f = net.fw.get_mut(&1).unwrap().originate(2, &item(1, 3), 0).unwrap();
        net.run(1, f, 0);
        assert!(net.drops.contains(&(2, Why::NoRoute)));
        // Nobody forwarded: A never heard the passive ack, so after the wait the hop is reported failed.
        assert!(net.fw.get_mut(&1).unwrap().expired(ACK_WAIT_MS / 2).is_empty());
        assert_eq!(
            net.fw.get_mut(&1).unwrap().expired(ACK_WAIT_MS + 5000),
            vec![(1, 7)]
        );
    }

    #[test]
    fn payload_size_and_bad_frames_are_checked() {
        assert!(frame(
            1,
            2,
            0,
            &HopItem {
                origin: 1,
                id: 1,
                dest: 2,
                payload: vec![0; PAYLOAD_MAX + 1]
            }
        )
        .is_err());
        assert!(frame(
            1,
            2,
            0,
            &HopItem {
                origin: 1,
                id: 1,
                dest: 2,
                payload: vec![0; PAYLOAD_MAX]
            }
        )
        .is_ok());
        let mut f = Forwarder::new(1);
        assert_eq!(f.on_frame(&[1, 2, 3], 0, |_| None), Action::Ignore);
    }
}
