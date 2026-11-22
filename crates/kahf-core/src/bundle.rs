//! The air frame header and bundles (MESH-SPEC.md 8.1 to 8.3).
//!
//! Every new frame starts with a 9 byte header, so a station that is not the next hop drops the
//! frame after reading it. A bundle is everything a relay has ready, sent at once in one burst
//! of up to 8 codewords: one item per codeword, each addressed by a 24 bit hash, each complete
//! on its own, so a lost codeword loses only its own item. The stations named in a bundle answer
//! in the order they are listed, 300 ms apart, so answers never collide.
//!
//! Tags `0x01` to `0x06`, `0x10` and `0x11` belong to the link layer (`link.rs`); new tags start
//! at `0x30`. Nothing here is on the air yet: the engine does not send these frames.

use crate::mac::SIFS_MS;
use sha2::{Digest, Sha256};

/// Bytes of one codeword payload (the daemon's `MAX_CHUNK`).
pub const PAYLOAD: usize = 36;
pub const HEADER: usize = 9;
pub const BODY_MAX: usize = PAYLOAD - HEADER;
/// Codewords in one burst.
pub const BUNDLE_MAX: usize = 8;
/// Gap between two stations' answers (ms).
pub const ANSWER_GAP_MS: u64 = 300;
/// Hops a frame may make.
pub const HOP_MAX: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Message = 0x30,
    Receipt = 0x31,
    Ack = 0x32,
    Page = 0x33,
    Request = 0x34,
    Grant = 0x35,
    Beacon = 0x36,
    Here = 0x37,
}

impl Kind {
    pub fn from_u8(v: u8) -> Option<Kind> {
        Some(match v {
            0x30 => Kind::Message,
            0x31 => Kind::Receipt,
            0x32 => Kind::Ack,
            0x33 => Kind::Page,
            0x34 => Kind::Request,
            0x35 => Kind::Grant,
            0x36 => Kind::Beacon,
            0x37 => Kind::Here,
            _ => return None,
        })
    }
}

/// The first three bytes of SHA-256 of the upper-case callsign (24 bits; never 0, which means "anyone").
pub fn h24(callsign: &str) -> u32 {
    let d = Sha256::digest(callsign.trim().to_uppercase().as_bytes());
    let v = u32::from_be_bytes([0, d[0], d[1], d[2]]);
    if v == 0 {
        1
    } else {
        v
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub kind: Kind,
    pub flags: u8,
    pub hops: u8,
    /// 0 = anyone.
    pub next_hop: u32,
    pub source: u32,
    /// Position of this codeword in its burst, and how many there are.
    pub index: u8,
    pub count: u8,
}

/// Read only the header of a payload. None for a short payload or an unknown tag (not ours:
/// ignore it).
pub fn peek(payload: &[u8]) -> Option<Header> {
    if payload.len() < HEADER {
        return None;
    }
    let kind = Kind::from_u8(payload[0])?;
    let (flags, hops) = (payload[1] >> 4, payload[1] & 0x0F);
    let n = |i: usize| u32::from_be_bytes([0, payload[i], payload[i + 1], payload[i + 2]]);
    let (index, count) = (payload[8] >> 4, (payload[8] & 0x0F) + 1);
    if index >= count {
        return None;
    }
    Some(Header {
        kind,
        flags,
        hops,
        next_hop: n(2),
        source: n(5),
        index,
        count,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub header: Header,
    pub body: Vec<u8>,
}

impl Frame {
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        let h = &self.header;
        if self.body.len() > BODY_MAX {
            return Err(format!("body of {} bytes is over {BODY_MAX}", self.body.len()));
        }
        if h.hops > HOP_MAX || h.flags > 0x0F || h.next_hop > 0xFF_FFFF || h.source > 0xFF_FFFF {
            return Err("header field out of range".into());
        }
        if h.index >= h.count || h.count as usize > BUNDLE_MAX {
            return Err("bad frame index or count".into());
        }
        let mut o = vec![h.kind as u8, (h.flags << 4) | h.hops];
        o.extend(&h.next_hop.to_be_bytes()[1..]);
        o.extend(&h.source.to_be_bytes()[1..]);
        o.push((h.index << 4) | (h.count - 1));
        o.extend(&self.body);
        Ok(o)
    }

    pub fn decode(payload: &[u8]) -> Option<Frame> {
        Some(Frame {
            header: peek(payload)?,
            body: payload[HEADER..].to_vec(),
        })
    }

    /// Should this station act on the frame? Only the named next hop (or anyone, for 0) does.
    /// A frame from this station itself (a heard echo) or one that has used up its hops is not.
    pub fn is_for(&self, me: u32) -> bool {
        let h = &self.header;
        h.source != me && h.hops <= HOP_MAX && (h.next_hop == 0 || h.next_hop == me)
    }
}

/// One thing to send to one station.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub to: u32,
    pub kind: Kind,
    pub body: Vec<u8>,
}

/// Everything ready, in bursts of at most `BUNDLE_MAX` codewords, one item per codeword. Nothing
/// waits to fill a bundle: the caller sends what it has when it may transmit.
pub fn build(source: u32, items: &[Item]) -> Result<Vec<Vec<Vec<u8>>>, String> {
    items
        .chunks(BUNDLE_MAX)
        .map(|chunk| {
            chunk
                .iter()
                .enumerate()
                .map(|(i, it)| {
                    Frame {
                        header: Header {
                            kind: it.kind,
                            flags: 0,
                            hops: 0,
                            next_hop: it.to,
                            source,
                            index: i as u8,
                            count: chunk.len() as u8,
                        },
                        body: it.body.clone(),
                    }
                    .encode()
                })
                .collect()
        })
        .collect()
}

/// Recipients in the order they answer: first appearance in the burst, each once.
pub fn answer_order(frames: &[Header]) -> Vec<u32> {
    let mut out = Vec::new();
    for h in frames {
        if h.next_hop != 0 && !out.contains(&h.next_hop) {
            out.push(h.next_hop);
        }
    }
    out
}

/// This station's place in the answer order, if it is named at all.
pub fn answer_slot(frames: &[Header], me: u32) -> Option<usize> {
    answer_order(frames).iter().position(|r| *r == me)
}

/// How long after the burst ends this station starts its answer: one short gap, then one gap per
/// station ahead of it in the list.
pub fn answer_delay_ms(slot: usize) -> u64 {
    SIFS_MS + ANSWER_GAP_MS * slot as u64
}

/// One acknowledgement for a burst: a bitmap of the codeword indexes this station received.
pub fn ack(me: u32, to: u32, got: &[u8]) -> Result<Vec<u8>, String> {
    let mut map = 0u8;
    for i in got {
        if *i as usize >= BUNDLE_MAX {
            return Err("index out of range".into());
        }
        map |= 1 << i;
    }
    Frame {
        header: Header {
            kind: Kind::Ack,
            flags: 0,
            hops: 0,
            next_hop: to,
            source: me,
            index: 0,
            count: 1,
        },
        body: vec![map],
    }
    .encode()
}

/// The codeword indexes an acknowledgement says arrived.
pub fn acked(frame: &Frame) -> Vec<u8> {
    let map = frame.body.first().copied().unwrap_or(0);
    (0..BUNDLE_MAX as u8).filter(|i| map & (1 << i) != 0).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CALLS: [&str; 5] = ["AA1AAA", "BB1BBB", "CC1CCC", "DD1DDD", "EE1EEE"];

    fn item(to: &str, text: &str) -> Item {
        Item {
            to: h24(to),
            kind: Kind::Message,
            body: text.as_bytes().to_vec(),
        }
    }

    #[test]
    fn the_header_round_trips_and_is_nine_bytes() {
        let f = Frame {
            header: Header {
                kind: Kind::Grant,
                flags: 5,
                hops: 3,
                next_hop: 0xABCDEF,
                source: 0x123456,
                index: 6,
                count: 8,
            },
            body: b"xyz".to_vec(),
        };
        let b = f.encode().unwrap();
        assert_eq!(b.len(), HEADER + 3);
        assert_eq!(Frame::decode(&b), Some(f));
        assert_eq!(peek(&b).unwrap().next_hop, 0xABCDEF);
    }

    #[test]
    fn bad_input_is_refused() {
        assert_eq!(peek(&[0x30; 8]), None, "short");
        assert_eq!(peek(&[0x11; 36]), None, "a link layer tag is not ours");
        let mut b = Frame {
            header: Header {
                kind: Kind::Ack,
                flags: 0,
                hops: 0,
                next_hop: 1,
                source: 2,
                index: 0,
                count: 1,
            },
            body: vec![],
        }
        .encode()
        .unwrap();
        b[8] = 0x30; // index 3 of a count of 1
        assert_eq!(peek(&b), None);
        let big = Frame {
            header: peek(
                &b[..0]
                    .iter()
                    .chain(&[0x32, 0, 0, 0, 1, 0, 0, 2, 0])
                    .copied()
                    .collect::<Vec<u8>>(),
            )
            .unwrap(),
            body: vec![0; BODY_MAX + 1],
        };
        assert!(big.encode().is_err());
    }

    #[test]
    fn a_bundle_for_five_stations_is_read_by_each_and_ignored_by_the_rest() {
        let src = h24("RELAY1");
        let items: Vec<Item> = CALLS.iter().map(|c| item(c, &format!("hi {c}"))).collect();
        let bursts = build(src, &items).unwrap();
        assert_eq!(bursts.len(), 1);
        let burst = &bursts[0];
        assert_eq!(burst.len(), 5);
        for (i, me) in CALLS.iter().enumerate() {
            let mine: Vec<Frame> = burst
                .iter()
                .filter_map(|p| Frame::decode(p))
                .filter(|f| f.is_for(h24(me)))
                .collect();
            assert_eq!(mine.len(), 1, "{me}");
            assert_eq!(mine[0].body, format!("hi {me}").as_bytes());
            assert_eq!(mine[0].header.index as usize, i);
            // Ignoring needs only the header.
            for p in burst {
                let h = peek(&p[..HEADER]);
                assert!(h.is_some());
            }
        }
        // The sender ignores its own frames coming back (echo), whoever they name.
        assert!(burst
            .iter()
            .filter_map(|p| Frame::decode(p))
            .all(|f| !f.is_for(src)));
        // A station that is not in the bundle acts on nothing.
        let other = h24("ZZ9ZZZ");
        assert!(burst
            .iter()
            .filter_map(|p| Frame::decode(p))
            .all(|f| !f.is_for(other)));
    }

    #[test]
    fn answers_come_in_the_order_listed_and_never_share_a_slot() {
        let src = h24("RELAY1");
        // CC1CCC has two items; it answers once, at its first place.
        let items = vec![
            item("BB1BBB", "1"),
            item("CC1CCC", "2"),
            item("CC1CCC", "3"),
            item("AA1AAA", "4"),
            item("EE1EEE", "5"),
        ];
        let burst = &build(src, &items).unwrap()[0];
        let heads: Vec<Header> = burst.iter().map(|p| peek(p).unwrap()).collect();
        assert_eq!(
            answer_order(&heads),
            vec![h24("BB1BBB"), h24("CC1CCC"), h24("AA1AAA"), h24("EE1EEE")]
        );
        let delays: Vec<u64> = ["BB1BBB", "CC1CCC", "AA1AAA", "EE1EEE"]
            .iter()
            .map(|c| answer_delay_ms(answer_slot(&heads, h24(c)).unwrap()))
            .collect();
        assert_eq!(delays, vec![300, 600, 900, 1200]);
        assert_eq!(answer_slot(&heads, h24("DD1DDD")), None);
    }

    #[test]
    fn a_lost_codeword_loses_only_its_own_item() {
        let src = h24("RELAY1");
        let items: Vec<Item> = CALLS.iter().map(|c| item(c, c)).collect();
        let mut burst = build(src, &items).unwrap().remove(0);
        burst.remove(2); // CC1CCC's codeword is lost
        for me in CALLS {
            let got = burst
                .iter()
                .filter_map(|p| Frame::decode(p))
                .any(|f| f.is_for(h24(me)));
            assert_eq!(got, me != "CC1CCC", "{me}");
        }
    }

    #[test]
    fn one_acknowledgement_carries_a_bitmap_and_the_sender_sees_what_is_missing() {
        let a = ack(h24("BB1BBB"), h24("RELAY1"), &[0, 1, 3]).unwrap();
        assert!(a.len() <= PAYLOAD);
        let f = Frame::decode(&a).unwrap();
        assert!(f.is_for(h24("RELAY1")) && !f.is_for(h24("CC1CCC")));
        assert_eq!(acked(&f), vec![0, 1, 3]);
        let sent: Vec<u8> = vec![0, 1, 2, 3];
        let missing: Vec<u8> = sent.into_iter().filter(|i| !acked(&f).contains(i)).collect();
        assert_eq!(missing, vec![2]);
        assert!(ack(1, 2, &[8]).is_err());
    }

    #[test]
    fn more_than_eight_items_go_in_more_bursts_and_nothing_waits() {
        let items: Vec<Item> = (0..11).map(|i| item("BB1BBB", &i.to_string())).collect();
        let bursts = build(h24("RELAY1"), &items).unwrap();
        assert_eq!(bursts.iter().map(|b| b.len()).collect::<Vec<_>>(), vec![8, 3]);
        assert_eq!(peek(&bursts[1][2]).unwrap().count, 3);
        assert_eq!(build(h24("RELAY1"), &[]).unwrap().len(), 0);
        assert!(build(
            1,
            &[Item {
                to: 2,
                kind: Kind::Message,
                body: vec![0; BODY_MAX + 1]
            }]
        )
        .is_err());
    }

    #[test]
    fn the_hash_ignores_case_and_is_never_zero() {
        assert_eq!(h24("aa1aaa"), h24(" AA1AAA "));
        assert_ne!(h24("AA1AAA"), h24("BB1BBB"));
        assert!(CALLS.iter().all(|c| h24(c) != 0 && h24(c) <= 0xFF_FFFF));
    }
}
