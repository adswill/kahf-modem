//! The epoch planner (MESH-SPEC.md 5.2 and 5.4), Rust twin of `server/src/planner.js`.
//!
//! One clash domain: the relays given are all treated as able to interfere with each other, so
//! no two of them ever hold the same (lane, slot), and no relay holds two lanes in one slot
//! (one radio). An epoch is 12 s of six 2 s slots; there are 8 relay lanes, so 48 cells.
//!
//! - Idle relays (empty queue) hold nothing.
//! - Active relays share the 48 cells by demand (queue size and urgency), at least one cell
//!   each while there are 48 or fewer, at most one per slot.
//! - A relay keeps the same lane from slot to slot where it can.
//! - The plan is a pure function of (epoch, seed, relays): any relay can recompute it.
//!
//! The server signs plans and publishes four epochs ahead. `Held` is what a relay keeps: it
//! follows signed plans, and when the server has been silent past its lookahead it computes the
//! local plan from the last published inputs at a quarter of the normal duty. With nothing to
//! go on it stays silent. Nothing here is on the air yet.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use std::collections::BTreeMap;

pub const LANES: usize = 8;
pub const SLOTS: usize = 6;
pub const CELLS: usize = LANES * SLOTS;
pub const EPOCH_S: u64 = 12;
pub const SLOT_S: u64 = 2;
pub const LOOKAHEAD: u64 = 4;
/// The local plan uses one epoch in this many for each relay.
pub const LOCAL_DUTY_DIV: u64 = 4;

pub fn epoch_of(utc_s: u64) -> u64 {
    utc_s / EPOCH_S
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayIn {
    pub id: u32,
    pub queue_bytes: u32,
    pub urgency: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grant {
    pub relay: u32,
    pub lane: u8,
    pub slot: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub epoch: u64,
    pub grants: Vec<Grant>,
}

/// FNV-1a over (seed, epoch, id): the tie-break, the same in the JS twin.
pub fn mix(seed: u32, epoch: u64, id: u32) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in seed.to_le_bytes().into_iter().chain((epoch as u32).to_le_bytes()).chain(id.to_le_bytes()) {
        h = (h ^ b as u32).wrapping_mul(0x0100_0193);
    }
    h
}

fn weight(r: &RelayIn) -> u64 {
    1 + (r.queue_bytes.min(2000) / 200) as u64 + 2 * r.urgency.min(5) as u64
}

pub fn plan(epoch: u64, seed: u32, relays: &[RelayIn]) -> Plan {
    let mut active: Vec<&RelayIn> = relays.iter().filter(|r| r.queue_bytes > 0).collect();
    active.sort_by_key(|r| (mix(seed, epoch, r.id), r.id));
    if active.len() > CELLS {
        // More than cells: a rotating window of relays gets one cell each.
        let off = (epoch as usize * CELLS) % active.len();
        active.rotate_left(off);
        active.truncate(CELLS);
    }
    let n = active.len();
    if n == 0 {
        return Plan { epoch, grants: vec![] };
    }
    // Cells by demand: one each, then the next cell to the relay with the largest weight per cell held.
    let w: Vec<u64> = active.iter().map(|r| weight(r)).collect();
    let mut t = vec![1usize; n];
    let mut left = CELLS - n;
    while left > 0 {
        let mut best: Option<usize> = None;
        for i in 0..n {
            if t[i] >= SLOTS {
                continue;
            }
            // w[i]/t[i] > w[b]/t[b]  ->  w[i]*t[b] > w[b]*t[i]; ties keep the earlier (hash order).
            if best.is_none_or(|b| w[i] * t[b] as u64 > w[b] * t[i] as u64) {
                best = Some(i);
            }
        }
        let Some(b) = best else { break };
        t[b] += 1;
        left -= 1;
    }
    // Place: most cells first, each into the least loaded slots.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&i| (std::cmp::Reverse(t[i]), i));
    let off = (epoch % SLOTS as u64) as usize;
    let mut load = [0usize; SLOTS];
    let mut in_slot: Vec<Vec<usize>> = vec![vec![]; SLOTS];
    for &i in &order {
        let mut slots: Vec<usize> = (0..SLOTS).collect();
        slots.sort_by_key(|&s| (load[s], (s + SLOTS - off) % SLOTS));
        for &s in slots.iter().take(t[i]) {
            load[s] += 1;
            in_slot[s].push(i);
        }
    }
    // Lanes: keep the lane of the previous slot where it is free.
    let mut grants = Vec::new();
    let mut prev: BTreeMap<usize, usize> = BTreeMap::new();
    for (s, members) in in_slot.iter_mut().enumerate() {
        members.sort_by_key(|&i| active[i].id);
        let mut used = [false; LANES];
        let mut lane_of: BTreeMap<usize, usize> = BTreeMap::new();
        for &i in members.iter() {
            if let Some(&l) = prev.get(&i) {
                if !used[l] {
                    used[l] = true;
                    lane_of.insert(i, l);
                }
            }
        }
        for &i in members.iter() {
            if let std::collections::btree_map::Entry::Vacant(e) = lane_of.entry(i) {
                let l = (0..LANES).find(|l| !used[*l]).unwrap_or(0);
                used[l] = true;
                e.insert(l);
            }
        }
        for &i in members.iter() {
            grants.push(Grant { relay: active[i].id, lane: lane_of[&i] as u8, slot: s as u8 });
        }
        prev = lane_of;
    }
    Plan { epoch, grants }
}

/// The plan every relay computes for itself when the server is silent: all known relays as
/// active, cut to one epoch in `LOCAL_DUTY_DIV` for each relay.
pub fn local_plan(epoch: u64, seed: u32, relay_ids: &[u32]) -> Plan {
    let all: Vec<RelayIn> = relay_ids.iter().map(|&id| RelayIn { id, queue_bytes: 1, urgency: 0 }).collect();
    let mut p = plan(epoch, seed, &all);
    p.grants.retain(|g| (epoch + g.relay as u64).is_multiple_of(LOCAL_DUTY_DIV));
    p
}

impl Plan {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut o = b"KPL1".to_vec();
        o.extend(self.epoch.to_le_bytes());
        o.extend((self.grants.len() as u16).to_le_bytes());
        for g in &self.grants {
            o.extend(g.relay.to_le_bytes());
            o.push(g.lane);
            o.push(g.slot);
        }
        o
    }

    pub fn sign(&self, project_key: &[u8; 32]) -> [u8; 64] {
        SigningKey::from_bytes(project_key).sign(&self.to_bytes()).to_bytes()
    }

    pub fn grant_for(&self, relay: u32, slot: u8) -> Option<&Grant> {
        self.grants.iter().find(|g| g.relay == relay && g.slot == slot)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Server,
    /// The degraded local plan: labelled "local plan" in the beacon.
    Local,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Go { lane: u8, source: Source },
    /// Nothing in the plan for this slot, or nothing to go on: silence.
    Hold,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    BadSignature,
}

/// What a relay holds: signed plans for the next epochs and the last published inputs.
#[derive(Default)]
pub struct Held {
    plans: BTreeMap<u64, Plan>,
    inputs: Option<(u32, Vec<u32>)>,
}

impl Held {
    /// Keep a plan only if the project key signed it.
    pub fn accept(&mut self, plan: Plan, sig: &[u8; 64], project_public: &[u8; 32]) -> Result<(), Refused> {
        let vk = VerifyingKey::from_bytes(project_public).map_err(|_| Refused::BadSignature)?;
        vk.verify(&plan.to_bytes(), &Signature::from_bytes(sig)).map_err(|_| Refused::BadSignature)?;
        let keep_from = plan.epoch.saturating_sub(LOOKAHEAD + 1);
        self.plans.retain(|e, _| *e >= keep_from);
        self.plans.insert(plan.epoch, plan);
        Ok(())
    }

    /// The seed and relay list the server publishes every ten minutes.
    pub fn set_inputs(&mut self, seed: u32, relay_ids: Vec<u32>) {
        self.inputs = Some((seed, relay_ids));
    }

    /// May relay `me` transmit in `slot` of `epoch`? Follows the signed plan for that epoch; if
    /// there is none, the local plan from the last inputs; if there are none, silence.
    pub fn decide(&self, me: u32, epoch: u64, slot: u8) -> Decision {
        if let Some(p) = self.plans.get(&epoch) {
            return match p.grant_for(me, slot) {
                Some(g) => Decision::Go { lane: g.lane, source: Source::Server },
                None => Decision::Hold,
            };
        }
        match &self.inputs {
            Some((seed, ids)) => match local_plan(epoch, *seed, ids).grant_for(me, slot) {
                Some(g) => Decision::Go { lane: g.lane, source: Source::Local },
                None => Decision::Hold,
            },
            None => Decision::Hold,
        }
    }
}

/// Physical lane (the 12 of `lanes.rs`) of each relay lane R1 to R8. Lanes 4 and 7 are spares,
/// 5 and 6 are the calling lanes.
pub const PHYS: [u8; LANES] = [0, 1, 2, 3, 8, 9, 10, 11];
const PHYS_LANES: u8 = 12;
const CALLING: [u8; 2] = [5, 6];

/// A relay's run of adjacent physical lanes in one slot, sent as one wider signal (2.5): the
/// run is `first..first + count`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bonded {
    pub relay: u32,
    pub slot: u8,
    pub first: u8,
    pub count: u8,
}

/// How many lanes a transfer should ask for: one unless the link is strong (a fast mode works)
/// and the transfer is more than a few frames. The thresholds are guesses (est.), to be tuned.
pub fn bond_count(snr_db: f32, bytes: u32) -> u8 {
    if bytes < 4 * 27 || snr_db < 9.0 {
        1
    } else if snr_db < 15.0 {
        2
    } else if snr_db < 21.0 {
        3
    } else {
        4
    }
}

/// Widen grants into runs of adjacent lanes. `wants` is (relay, lanes wanted, at most 4; five
/// lanes would be 3280 Hz, over the 2700 Hz cap). A run takes only lanes that no other grant
/// holds in that slot, never the calling lanes, and never overlaps another bonded run. A run
/// that cannot grow stays as wide as it can get, down to the relay's own lane.
pub fn bond(plan: &Plan, wants: &[(u32, u8)]) -> Vec<Bonded> {
    let mut taken: Vec<[bool; PHYS_LANES as usize]> = vec![[false; PHYS_LANES as usize]; SLOTS];
    for g in &plan.grants {
        taken[g.slot as usize][PHYS[g.lane as usize] as usize] = true;
    }
    let mut out = Vec::new();
    for &(relay, want) in wants {
        let want = want.clamp(1, 4);
        for g in plan.grants.iter().filter(|g| g.relay == relay) {
            let own = PHYS[g.lane as usize];
            let free = |slot: usize, l: u8| !CALLING.contains(&l) && (l == own || !taken[slot][l as usize]);
            let mut best = (own, 1u8);
            'n: for n in (1..=want).rev() {
                for first in own.saturating_sub(n - 1)..=own {
                    if first + n <= PHYS_LANES && (first..first + n).all(|l| free(g.slot as usize, l)) {
                        best = (first, n);
                        break 'n;
                    }
                }
            }
            for l in best.0..best.0 + best.1 {
                taken[g.slot as usize][l as usize] = true;
            }
            out.push(Bonded { relay, slot: g.slot, first: best.0, count: best.1 });
        }
    }
    out
}

/// One of a relay's granted cells, used to send to another relay over the air (purpose HOP).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HopGrant {
    pub relay: u32,
    pub target: u32,
    pub lane: u8,
    pub slot: u8,
}

/// Give each (from, to) request one cell of `from`'s grants: the earliest slot in which `to`
/// holds no grant of its own (a relay cannot listen while it sends) and is not already the target
/// of another hop. Requests that find no such cell get none this epoch.
pub fn hop_grants(plan: &Plan, requests: &[(u32, u32)]) -> Vec<HopGrant> {
    let mut receiving: Vec<(u8, u32)> = Vec::new();
    let mut used: Vec<(u32, u8)> = Vec::new();
    let mut out = Vec::new();
    for &(from, to) in requests {
        let mut mine: Vec<&Grant> = plan.grants.iter().filter(|g| g.relay == from).collect();
        mine.sort_by_key(|g| g.slot);
        let pick = mine.into_iter().find(|g| {
            plan.grant_for(to, g.slot).is_none() && !receiving.contains(&(g.slot, to)) && !used.contains(&(from, g.slot))
        });
        if let Some(g) = pick {
            receiving.push((g.slot, to));
            used.push((from, g.slot));
            out.push(HopGrant { relay: from, target: to, lane: g.lane, slot: g.slot });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    struct Rng(u64);
    impl Rng {
        fn next(&mut self, n: u64) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 33) % n
        }
    }

    fn relays(r: &mut Rng, n: u32, busy_pct: u64) -> Vec<RelayIn> {
        (0..n)
            .map(|i| RelayIn {
                id: 1000 + i,
                queue_bytes: if r.next(100) < busy_pct { 1 + r.next(3000) as u32 } else { 0 },
                urgency: r.next(3) as u8,
            })
            .collect()
    }

    /// The invariants, for any plan.
    fn check(p: &Plan, rs: &[RelayIn]) {
        let mut cells = HashSet::new();
        let mut per_slot = HashSet::new();
        for g in &p.grants {
            assert!((g.lane as usize) < LANES && (g.slot as usize) < SLOTS);
            assert!(cells.insert((g.lane, g.slot)), "two relays on lane {} slot {}", g.lane, g.slot);
            assert!(per_slot.insert((g.relay, g.slot)), "relay {} twice in slot {}", g.relay, g.slot);
            let r = rs.iter().find(|r| r.id == g.relay).unwrap();
            assert!(r.queue_bytes > 0, "idle relay {} holds a cell", g.relay);
        }
    }

    #[test]
    fn thirty_relays_random_queues_never_clash_and_idle_ones_hold_nothing() {
        let mut r = Rng(1);
        for epoch in 0..300 {
            let pct = 20 + r.next(80);
            let rs = relays(&mut r, 30, pct);
            let p = plan(epoch, 7, &rs);
            check(&p, &rs);
            let active = rs.iter().filter(|x| x.queue_bytes > 0).count();
            // Every cell is used while there is demand for it.
            let max_cells: usize = active * SLOTS;
            assert_eq!(p.grants.len(), CELLS.min(max_cells), "epoch {epoch}");
        }
    }

    #[test]
    fn a_busy_relay_among_idle_ones_gets_over_eighty_percent_of_a_lane() {
        let mut rs: Vec<RelayIn> = (0..30).map(|i| RelayIn { id: i, queue_bytes: 0, urgency: 0 }).collect();
        rs[7].queue_bytes = 900;
        rs[19].queue_bytes = 50;
        for epoch in 0..60 {
            let p = plan(epoch, 3, &rs);
            check(&p, &rs);
            for id in [7, 19] {
                let mine: Vec<&Grant> = p.grants.iter().filter(|g| g.relay == id).collect();
                let mut lanes: HashMap<u8, usize> = HashMap::new();
                for g in &mine {
                    *lanes.entry(g.lane).or_default() += 1;
                }
                let best = lanes.values().max().copied().unwrap_or(0);
                assert!(best * 100 >= SLOTS * 80, "relay {id} holds {best} of {SLOTS} slots on one lane");
            }
        }
    }

    #[test]
    fn thirty_busy_relays_share_fairly_and_the_big_queue_gets_more() {
        let mut rs: Vec<RelayIn> = (0..30).map(|i| RelayIn { id: i, queue_bytes: 100, urgency: 0 }).collect();
        rs[4].queue_bytes = 2000;
        let mut served: HashMap<u32, usize> = HashMap::new();
        for epoch in 0..20 {
            let p = plan(epoch, 9, &rs);
            check(&p, &rs);
            assert_eq!(p.grants.len(), CELLS);
            for g in &p.grants {
                *served.entry(g.relay).or_default() += 1;
            }
        }
        assert!(rs.iter().all(|r| served.get(&r.id).copied().unwrap_or(0) >= 20), "someone starved");
        let others = served.iter().filter(|(k, _)| **k != 4).map(|(_, v)| *v).max().unwrap();
        assert!(served[&4] > others, "the large queue should get more than any other");
    }

    #[test]
    fn more_relays_than_cells_rotate() {
        let rs: Vec<RelayIn> = (0..70).map(|i| RelayIn { id: i, queue_bytes: 10, urgency: 0 }).collect();
        let mut seen = HashSet::new();
        for epoch in 0..6 {
            let p = plan(epoch, 1, &rs);
            check(&p, &rs);
            assert_eq!(p.grants.len(), CELLS);
            seen.extend(p.grants.iter().map(|g| g.relay));
        }
        assert_eq!(seen.len(), 70, "every relay is served within a few epochs");
    }

    #[test]
    fn the_plan_depends_only_on_its_inputs() {
        let mut r = Rng(5);
        let rs = relays(&mut r, 30, 60);
        assert_eq!(plan(42, 11, &rs), plan(42, 11, &rs));
        let mut rev = rs.clone();
        rev.reverse();
        assert_eq!(plan(42, 11, &rs), plan(42, 11, &rev), "input order does not matter");
    }

    #[test]
    fn unsigned_forged_or_foreign_plans_are_refused() {
        let secret = [3u8; 32];
        let public = SigningKey::from_bytes(&secret).verifying_key().to_bytes();
        let p = plan(10, 1, &[RelayIn { id: 1, queue_bytes: 5, urgency: 0 }]);
        let sig = p.sign(&secret);
        let mut h = Held::default();
        assert_eq!(h.accept(p.clone(), &sig, &public), Ok(()));
        let mut forged = p.clone();
        forged.grants[0].lane = 7;
        assert_eq!(h.accept(forged, &sig, &public), Err(Refused::BadSignature));
        let other = SigningKey::from_bytes(&[4u8; 32]).verifying_key().to_bytes();
        assert_eq!(h.accept(p, &sig, &other), Err(Refused::BadSignature));
    }

    /// Thirty relays follow the server, the server dies for 60 s, then comes back.
    #[test]
    fn a_server_outage_falls_to_the_local_plan_then_recovers() {
        let secret = [8u8; 32];
        let public = SigningKey::from_bytes(&secret).verifying_key().to_bytes();
        let mut r = Rng(2);
        let rs = relays(&mut r, 30, 70);
        let ids: Vec<u32> = rs.iter().map(|x| x.id).collect();
        let mut held: Vec<Held> = ids.iter().map(|_| Held::default()).collect();
        for h in held.iter_mut() {
            h.set_inputs(5, ids.clone());
        }
        // The server publishes epochs 100..=103 (lookahead 4) and then stops at epoch 100's time.
        for e in 100..100 + LOOKAHEAD {
            let p = plan(e, 5, &rs);
            let sig = p.sign(&secret);
            for h in held.iter_mut() {
                h.accept(p.clone(), &sig, &public).unwrap();
            }
        }
        // Epochs 100..=104: 60 s of silence. The first four follow the server's plans, the fifth the local plan.
        let mut local_grants = 0;
        for e in 100..105 {
            let mut cells = HashSet::new();
            let mut source = None;
            for s in 0..SLOTS as u8 {
                for (i, id) in ids.iter().enumerate() {
                    if let Decision::Go { lane, source: src } = held[i].decide(*id, e, s) {
                        assert!(cells.insert((lane, s)), "clash in outage, epoch {e}");
                        source = Some(src);
                        if e == 104 {
                            local_grants += 1;
                            assert_eq!(src, Source::Local);
                        }
                    }
                }
            }
            if e < 104 {
                assert_eq!(source, Some(Source::Server), "epoch {e}");
            }
        }
        // Reduced duty: a quarter of what a full epoch would give, at most.
        assert!(local_grants > 0 && local_grants <= CELLS / LOCAL_DUTY_DIV as usize + LANES, "{local_grants}");
        // The server returns: its plan is followed again.
        let p = plan(106, 5, &rs);
        let sig = p.sign(&secret);
        for h in held.iter_mut() {
            h.accept(p.clone(), &sig, &public).unwrap();
        }
        let go = ids.iter().enumerate().any(|(i, id)| (0..SLOTS as u8).any(|s| matches!(held[i].decide(*id, 106, s), Decision::Go { source: Source::Server, .. })));
        assert!(go);
    }

    #[test]
    fn a_relay_with_nothing_to_go_on_stays_silent() {
        let h = Held::default();
        assert_eq!(h.decide(1, 5, 0), Decision::Hold);
    }

    /// 20 relays from a formula (the JS test builds the same), epochs 77 and 78.
    pub fn twin_input() -> Vec<RelayIn> {
        (0..20u32).map(|i| RelayIn { id: 10 + i, queue_bytes: (i * 37 % 11) * 250, urgency: (i % 3) as u8 }).collect()
    }

    pub fn twin_line(p: &Plan) -> String {
        p.grants.iter().map(|g| format!("{}:{}:{}", g.relay, g.lane, g.slot)).collect::<Vec<_>>().join(",")
    }

    #[test]
    fn known_plans_for_the_js_twin() {
        let rs = twin_input();
        eprintln!("TWIN77 {}", twin_line(&plan(77, 42, &rs)));
        eprintln!("TWIN78 {}", twin_line(&plan(78, 42, &rs)));
        eprintln!("TWINL {}", twin_line(&local_plan(80, 42, &rs.iter().map(|r| r.id).collect::<Vec<_>>())));
    }
}

#[cfg(test)]
mod bonding_tests {
    use super::*;
    use crate::lanes::{BandTable, LANE_PITCH_HZ, LANE_WIDTH_HZ};

    fn busy(ids: &[u32]) -> Vec<RelayIn> {
        ids.iter().map(|&id| RelayIn { id, queue_bytes: 800, urgency: 0 }).collect()
    }

    /// What the JS twin (server/src/planner.js) must give for the same input: run with `--nocapture`.
    #[test]
    fn known_bonds_and_hops_for_the_js_twin() {
        let rs: Vec<RelayIn> = (0..20u32).map(|i| RelayIn { id: 10 + i, queue_bytes: (i * 37 % 11) * 250, urgency: (i % 3) as u8 }).collect();
        let p = plan(77, 42, &rs);
        let wants: Vec<(u32, u8)> = rs.iter().map(|r| (r.id, 1 + (r.id % 4) as u8)).collect();
        let b: Vec<String> = bond(&p, &wants).iter().map(|b| format!("{}:{}:{}:{}", b.relay, b.slot, b.first, b.count)).collect();
        eprintln!("TWINB {}", b.join(","));
        let h: Vec<String> = hop_grants(&p, &[(10, 11), (12, 13), (14, 15), (10, 15), (16, 17), (18, 19)])
            .iter()
            .map(|h| format!("{}>{}:{}:{}", h.relay, h.target, h.lane, h.slot))
            .collect();
        eprintln!("TWINH {}", h.join(","));
        eprintln!("TWINC {}", [(25.0, 50), (5.0, 5000), (10.0, 500), (16.0, 500), (30.0, 500), (9.0, 108), (15.0, 108), (21.0, 108)].iter().map(|(s, b)| bond_count(*s, *b).to_string()).collect::<Vec<_>>().join(","));
    }

    #[test]
    fn eligibility_needs_a_strong_link_and_a_large_transfer() {
        assert_eq!(bond_count(25.0, 50), 1, "too small");
        assert_eq!(bond_count(5.0, 5000), 1, "too weak");
        assert_eq!([bond_count(10.0, 500), bond_count(16.0, 500), bond_count(30.0, 500)], [2, 3, 4]);
    }

    #[test]
    fn a_lone_relay_widens_but_never_into_the_calling_lanes_and_never_past_four() {
        let rs = busy(&[1]);
        for epoch in 0..30 {
            let p = plan(epoch, 5, &rs);
            for want in 1..=6u8 {
                for b in bond(&p, &[(1, want)]) {
                    assert!(b.count >= 1 && b.count <= want.min(4));
                    for l in b.first..b.first + b.count {
                        assert!(![5, 6].contains(&l), "run {b:?} takes a calling lane");
                    }
                    let own = PHYS[p.grants.iter().find(|g| g.slot == b.slot).unwrap().lane as usize];
                    assert!(b.first <= own && own < b.first + b.count, "run {b:?} lost its own lane");
                }
            }
        }
        // A relay on its own can reach the full four lanes.
        assert!(bond(&plan(1, 5, &rs), &[(1, 4)]).iter().any(|b| b.count == 4));
    }

    #[test]
    fn bonded_runs_never_overlap_each_other_or_anyone_elses_lane() {
        let mut seen_wide = false;
        for epoch in 0..200 {
            let rs: Vec<RelayIn> = (0..6u32).map(|i| RelayIn { id: i, queue_bytes: if (epoch + i as u64).is_multiple_of(4) { 0 } else { 500 }, urgency: 0 }).collect();
            let p = plan(epoch, 2, &rs);
            let wants: Vec<(u32, u8)> = (0..6).map(|i| (i, 1 + (i as u8 + epoch as u8) % 4)).collect();
            let runs = bond(&p, &wants);
            for slot in 0..SLOTS as u8 {
                let mut used = [None::<u32>; 12];
                for b in runs.iter().filter(|b| b.slot == slot) {
                    seen_wide |= b.count > 1;
                    for l in b.first..b.first + b.count {
                        assert!(used[l as usize].replace(b.relay).is_none(), "lane {l} twice in slot {slot}");
                    }
                }
                // Another relay's own grant is never inside someone else's run.
                for g in p.grants.iter().filter(|g| g.slot == slot) {
                    let l = PHYS[g.lane as usize] as usize;
                    assert_eq!(used[l], Some(g.relay), "lane {l} in slot {slot}");
                }
            }
        }
        assert!(seen_wide, "some run should have grown");
    }

    #[test]
    fn a_bonded_run_passes_the_band_table_and_fits_2700_hz() {
        let mut t = BandTable::builtin_r1();
        t.plan_verified = true; // for this check only
        let b = t.band("17m").unwrap().clone();
        for first in 0..9u8 {
            for count in 1..=4u8 {
                if first + count > 12 || (first..first + count).any(|l| [5, 6].contains(&l)) {
                    continue;
                }
                let (a, z) = (b.lane(first).unwrap(), b.lane(first + count - 1).unwrap());
                let width = z.upper_hz() - a.lower_hz;
                assert_eq!(width, count as u64 * LANE_WIDTH_HZ + (count as u64 - 1) * (LANE_PITCH_HZ - LANE_WIDTH_HZ));
                assert!(width <= 2_700);
                let (lo, hi) = (1500 - LANE_WIDTH_HZ / 2, 1500 - LANE_WIDTH_HZ / 2 + width);
                assert_eq!(t.permit_tx("17m", first, first + count - 1, a.dial_hz, lo, hi), Ok(()), "{first}+{count}");
            }
        }
    }
}

#[cfg(test)]
mod hop_tests {
    use super::*;

    #[test]
    fn a_hop_uses_a_cell_of_the_sender_and_a_slot_when_the_target_is_listening() {
        let rs: Vec<RelayIn> = (0..12u32).map(|i| RelayIn { id: i, queue_bytes: if i < 8 { 500 } else { 0 }, urgency: 0 }).collect();
        for epoch in 0..100 {
            let p = plan(epoch, 4, &rs);
            // Relay 0 wants to hop to 9 (idle: always listening) and relay 1 to 2 (busy).
            let hops = hop_grants(&p, &[(0, 9), (1, 2), (3, 9)]);
            let mut by_target: Vec<(u8, u32)> = vec![];
            for h in &hops {
                assert!(p.grants.iter().any(|g| g.relay == h.relay && g.slot == h.slot && g.lane == h.lane), "not the sender's own cell");
                assert!(p.grant_for(h.target, h.slot).is_none(), "target {} sends in slot {}", h.target, h.slot);
                assert!(!by_target.contains(&(h.slot, h.target)), "two hops into one relay in one slot");
                by_target.push((h.slot, h.target));
            }
            assert!(hops.iter().any(|h| h.relay == 0 && h.target == 9), "an idle target is always reachable");
        }
    }

    #[test]
    fn a_target_that_transmits_all_epoch_gets_no_hop() {
        let mut rs: Vec<RelayIn> = vec![RelayIn { id: 1, queue_bytes: 900, urgency: 0 }, RelayIn { id: 2, queue_bytes: 900, urgency: 0 }];
        rs[0].queue_bytes = 900;
        // Two busy relays each hold all six slots on their own lane: neither can listen to the other.
        let p = plan(3, 1, &rs);
        assert_eq!(p.grants.len(), 12);
        assert!(hop_grants(&p, &[(1, 2)]).is_empty());
    }
}
