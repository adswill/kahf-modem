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
    p.grants.retain(|g| (epoch + g.relay as u64) % LOCAL_DUTY_DIV == 0);
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
