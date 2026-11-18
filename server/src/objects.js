// Durable Objects for the claims that must not race.
//
//   CallsignObject  one per callsign: the reporter owner (key, removal replay guard)
//                   and the relay registration (key, area).
//   MailObject      one per callsign: backbone message queue, id claims, receipts.
//   IndexObject     one for the backbone: heard index, wanted list, rate counters.
//   AreaObject      one per one-degree area: which relays are live there, and the
//                   slots each one last planned. New slots are planned against them.
//
// Every operation below does its check and its write in one call. A Durable Object
// handles one event at a time, and its input gate holds back other events while a
// storage operation is pending, so no other request can run between the check and
// the write. The in-process fake (tools/fake-store.mjs) runs these same functions one
// at a time, so the tests exercise this logic.
//
// Operations are functions (storage, args) -> result. Failures are returned as
// { ok: false, error }; anything else thrown is an internal error.
import { REMOVE_WINDOW_MS, isExpired, planSchedule } from "./core.js";
import { indexOps, mailOps } from "./backbone.js";

const OWNER = "owner";
const RELAY = "relay";
const AREA = "state";

/** Storage: { get(key) -> value | null, put(key, value) }, both awaitable. */

export const callsignOps = {
  /** Claims a reporter callsign for a key, or refreshes the claim if the key already holds it. */
  async claimOwner(s, { publicKey, nowMs }) {
    const owner = await s.get(OWNER);
    if (owner && !isExpired(owner.refreshed_at, nowMs) && owner.public_key !== publicKey) {
      return { ok: false, error: "this callsign is reporting with another key" };
    }
    const kept = owner && owner.public_key === publicKey && owner.removed ? { removed: owner.removed } : {};
    await s.put(OWNER, { public_key: publicKey, refreshed_at: nowMs, ...kept });
    return { ok: true };
  },

  /** Records one use of a removal (heard callsign and signing time), refusing a replay. */
  async claimRemoval(s, { publicKey, heardCall, signedAtMs, nowMs }) {
    const owner = await s.get(OWNER);
    const removed = {};
    for (const [heard, at] of Object.entries(owner?.removed ?? {})) {
      if (nowMs - at <= 2 * REMOVE_WINDOW_MS) removed[heard] = at;
    }
    if (Object.hasOwn(removed, heardCall) && signedAtMs <= removed[heardCall]) {
      return { ok: false, error: "this removal was already used" };
    }
    removed[heardCall] = signedAtMs;
    await s.put(OWNER, {
      public_key: owner?.public_key ?? publicKey,
      refreshed_at: owner?.refreshed_at ?? nowMs,
      removed,
    });
    return { ok: true };
  },

  /** Registers a relay. previousArea is set when it moves from another area. */
  async claimRelay(s, { callsign, area, publicKey, nowMs }) {
    const relay = await s.get(RELAY);
    if (relay && !isExpired(relay.refreshed_at, nowMs) && relay.public_key !== publicKey) {
      return { ok: false, error: "this callsign is registered with another key" };
    }
    await s.put(RELAY, { callsign, area, public_key: publicKey, refreshed_at: nowMs });
    return { ok: true, previousArea: relay && relay.area !== area ? relay.area : null };
  },

  async getRelay(s) {
    return (await s.get(RELAY)) ?? null;
  },
};

const emptyArea = () => ({ members: {}, slots: {} });

/** Drops relays and slot records that have expired, and slots of relays no longer in the area. */
function prune(state, nowMs) {
  for (const [callsign, member] of Object.entries(state.members)) {
    if (isExpired(member.refreshed_at, nowMs)) delete state.members[callsign];
  }
  for (const callsign of Object.keys(state.slots)) {
    const rec = state.slots[callsign];
    if (!Object.hasOwn(state.members, callsign) || isExpired(rec.refreshed_at, nowMs)) delete state.slots[callsign];
  }
  return state;
}

async function readArea(s) {
  return (await s.get(AREA)) ?? emptyArea();
}

export const areaOps = {
  async join(s, { callsign, nowMs }) {
    const state = await readArea(s);
    state.members[callsign] = { refreshed_at: nowMs };
    await s.put(AREA, prune(state, nowMs));
    return { ok: true };
  },

  async leave(s, { callsign }) {
    const state = await readArea(s);
    delete state.members[callsign];
    delete state.slots[callsign];
    await s.put(AREA, state);
    return { ok: true };
  },

  /**
   * Plans the relay's requests against the slots already booked by the other live relays
   * in this area, and stores the result. `requests` carries { relay, channel, duration,
   * direction, urgent } as makeSchedule expects.
   */
  async book(s, { area, relay, requests, nowMs }) {
    const nowS = Math.floor(nowMs / 1000);
    const state = await readArea(s);
    state.members[relay] = { refreshed_at: nowMs };
    prune(state, nowMs);

    const members = Object.keys(state.members);
    const relays = members.map((id) => ({ id, area }));
    const booked = [];
    for (const other of members) {
      if (other === relay) continue;
      const rec = state.slots[other];
      for (const slot of rec?.slots ?? []) {
        if (slot.start + slot.duration > nowS) {
          // Every relay's slots are kept in this one object, so the planner sees them all:
          // same-area slots of any direction, and transmit slots of every area (HF is global).
          booked.push({
            area: rec.area ?? area,
            channel: slot.channel,
            start: slot.start,
            duration: slot.duration,
            direction: slot.direction,
          });
        }
      }
    }

    let plan;
    try {
      plan = planSchedule(relays, requests, nowS, booked);
    } catch (err) {
      if (err instanceof TypeError || err instanceof RangeError) return { ok: false, error: err.message };
      throw err;
    }
    const slots = plan.slots[relay];
    state.slots[relay] = {
      area,
      slots: slots.map((s2) => ({ channel: s2.channel, start: s2.start, duration: s2.duration, direction: s2.direction })),
      refreshed_at: nowMs,
    };
    await s.put(AREA, state);
    return { ok: true, slots, unplaced: plan.unplaced.filter((u) => u.relay === relay) };
  },
};

/** Runs one operation from a POST body of { op, args }. */
async function serve(storage, ops, request) {
  const reply = (body, status) =>
    new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json; charset=utf-8" } });
  if (request.method !== "POST") return reply({ error: "method not allowed" }, 405);
  let message;
  try {
    message = await request.json();
  } catch {
    return reply({ error: "bad request" }, 400);
  }
  const op = message?.op;
  if (typeof op !== "string" || !Object.hasOwn(ops, op)) return reply({ error: "unknown operation" }, 404);
  const s = {
    async get(key) {
      return (await storage.get(key)) ?? null;
    },
    put(key, value) {
      return storage.put(key, value);
    },
  };
  return reply(await ops[op](s, message.args ?? {}), 200);
}

export class CallsignObject {
  constructor(ctx) {
    this.storage = ctx.storage;
  }
  fetch(request) {
    return serve(this.storage, callsignOps, request);
  }
}

export class AreaObject {
  constructor(ctx) {
    this.storage = ctx.storage;
  }
  fetch(request) {
    return serve(this.storage, areaOps, request);
  }
}

/** One per callsign: the HF backbone queue, claims and receipts (see backbone.js). */
export class MailObject {
  constructor(ctx) {
    this.storage = ctx.storage;
  }
  fetch(request) {
    return serve(this.storage, mailOps, request);
  }
}

/** One for the whole backbone: heard index, wanted list, relay rate counters. */
export class IndexObject {
  constructor(ctx) {
    this.storage = ctx.storage;
  }
  fetch(request) {
    return serve(this.storage, indexOps, request);
  }
}
