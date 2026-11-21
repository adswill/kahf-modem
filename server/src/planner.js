// The epoch planner (MESH-SPEC.md 5.2), JS twin of crates/kahf-core/src/planner.rs.
// Same algorithm, same tie-breaks: test/planner.test.mjs checks both give the same plans.
// Pure functions; signing and publishing come with the WebSocket push (spec step 10).

export const LANES = 8;
export const SLOTS = 6;
export const CELLS = LANES * SLOTS;
export const EPOCH_S = 12;
export const LOCAL_DUTY_DIV = 4;

export const epochOf = (utcS) => Math.floor(utcS / EPOCH_S);

function le32(v, out) {
  for (let i = 0; i < 4; i++) out.push((v >>> (8 * i)) & 0xff);
}

// FNV-1a over (seed, epoch, id), little endian 32 bit each.
export function mix(seed, epoch, id) {
  const bytes = [];
  le32(seed >>> 0, bytes);
  le32(epoch >>> 0, bytes);
  le32(id >>> 0, bytes);
  let h = 0x811c9dc5;
  for (const b of bytes) h = Math.imul(h ^ b, 0x01000193) >>> 0;
  return h;
}

const weight = (r) => 1 + Math.floor(Math.min(r.queueBytes, 2000) / 200) + 2 * Math.min(r.urgency, 5);

// relays: [{id, queueBytes, urgency}] -> {epoch, grants: [{relay, lane, slot}]}
export function plan(epoch, seed, relays) {
  let active = relays.filter((r) => r.queueBytes > 0);
  const key = new Map(active.map((r) => [r.id, mix(seed, epoch, r.id)]));
  active.sort((a, b) => key.get(a.id) - key.get(b.id) || a.id - b.id);
  if (active.length > CELLS) {
    const off = (epoch * CELLS) % active.length;
    active = active.slice(off).concat(active.slice(0, off)).slice(0, CELLS);
  }
  const n = active.length;
  if (n === 0) return { epoch, grants: [] };
  const w = active.map(weight);
  const t = new Array(n).fill(1);
  let left = CELLS - n;
  while (left > 0) {
    let best = -1;
    for (let i = 0; i < n; i++) {
      if (t[i] >= SLOTS) continue;
      if (best < 0 || w[i] * t[best] > w[best] * t[i]) best = i;
    }
    if (best < 0) break;
    t[best]++;
    left--;
  }
  const order = [...Array(n).keys()].sort((a, b) => t[b] - t[a] || a - b);
  const off = epoch % SLOTS;
  const load = new Array(SLOTS).fill(0);
  const inSlot = Array.from({ length: SLOTS }, () => []);
  for (const i of order) {
    const slots = [...Array(SLOTS).keys()].sort(
      (a, b) => load[a] - load[b] || ((a + SLOTS - off) % SLOTS) - ((b + SLOTS - off) % SLOTS),
    );
    for (const s of slots.slice(0, t[i])) {
      load[s]++;
      inSlot[s].push(i);
    }
  }
  const grants = [];
  let prev = new Map();
  for (let s = 0; s < SLOTS; s++) {
    const members = inSlot[s].sort((a, b) => active[a].id - active[b].id);
    const used = new Array(LANES).fill(false);
    const laneOf = new Map();
    for (const i of members) {
      const l = prev.get(i);
      if (l !== undefined && !used[l]) {
        used[l] = true;
        laneOf.set(i, l);
      }
    }
    for (const i of members) {
      if (!laneOf.has(i)) {
        let l = used.indexOf(false);
        if (l < 0) l = 0;
        used[l] = true;
        laneOf.set(i, l);
      }
    }
    for (const i of members) grants.push({ relay: active[i].id, lane: laneOf.get(i), slot: s });
    prev = laneOf;
  }
  return { epoch, grants };
}

// What every relay computes for itself when the server is silent: all known relays as active,
// one epoch in LOCAL_DUTY_DIV each.
export function localPlan(epoch, seed, relayIds) {
  const all = relayIds.map((id) => ({ id, queueBytes: 1, urgency: 0 }));
  const p = plan(epoch, seed, all);
  p.grants = p.grants.filter((g) => (epoch + g.relay) % LOCAL_DUTY_DIV === 0);
  return p;
}

// ---- bonding and hopping: JS twins of bond_count, bond and hop_grants (planner.rs) -------------

/** Physical lane (the 12 of lanes.rs) of each relay lane R1 to R8. */
export const PHYS = [0, 1, 2, 3, 8, 9, 10, 11];
const PHYS_LANES = 12;
const CALLING = [5, 6];

/** How many lanes a transfer should ask for: one unless the link is strong and the transfer is more than a few frames. */
export function bondCount(snrDb, bytes) {
  if (bytes < 4 * 27 || snrDb < 9) return 1;
  if (snrDb < 15) return 2;
  if (snrDb < 21) return 3;
  return 4;
}

/**
 * Widen grants into runs of adjacent physical lanes. `wants` is [[relay, lanes wanted]], at most 4
 * (five lanes would be 3280 Hz, over the segment bandwidth cap). Never the calling lanes, never over another run.
 * Returns [{relay, slot, first, count}].
 */
export function bond(p, wants) {
  const taken = Array.from({ length: SLOTS }, () => new Array(PHYS_LANES).fill(false));
  for (const g of p.grants) taken[g.slot][PHYS[g.lane]] = true;
  const out = [];
  for (const [relay, w] of wants) {
    const want = Math.min(4, Math.max(1, w));
    for (const g of p.grants.filter((x) => x.relay === relay)) {
      const own = PHYS[g.lane];
      const free = (slot, l) => !CALLING.includes(l) && (l === own || !taken[slot][l]);
      let best = [own, 1];
      search: for (let n = want; n >= 1; n--) {
        for (let first = Math.max(0, own - (n - 1)); first <= own; first++) {
          let ok = first + n <= PHYS_LANES;
          for (let l = first; ok && l < first + n; l++) ok = free(g.slot, l);
          if (ok) {
            best = [first, n];
            break search;
          }
        }
      }
      for (let l = best[0]; l < best[0] + best[1]; l++) taken[g.slot][l] = true;
      out.push({ relay, slot: g.slot, first: best[0], count: best[1] });
    }
  }
  return out;
}

/**
 * Give each [from, to] request one cell of `from`'s grants: the earliest slot in which `to` holds no
 * grant of its own and is not already the target of another hop. Returns [{relay, target, lane, slot}].
 */
export function hopGrants(p, requests) {
  const receiving = new Set();
  const used = new Set();
  const out = [];
  for (const [from, to] of requests) {
    const mine = p.grants.filter((g) => g.relay === from).sort((a, b) => a.slot - b.slot);
    const pick = mine.find((g) => !p.grants.some((x) => x.relay === to && x.slot === g.slot) && !receiving.has(`${g.slot}:${to}`) && !used.has(`${from}:${g.slot}`));
    if (pick) {
      receiving.add(`${pick.slot}:${to}`);
      used.add(`${from}:${pick.slot}`);
      out.push({ relay: from, target: to, lane: pick.lane, slot: pick.slot });
    }
  }
  return out;
}
