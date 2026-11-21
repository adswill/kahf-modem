// The mesh brain (MESH-SPEC.md 4, 5.2, 6): what the one Durable Object behind the relays'
// WebSockets does. Pure logic with injected time and signer, so it runs under Node in the tests.
//   - one socket per relay key (Hub), heartbeats, items held 60 s in RAM for the HTTPS poll
//   - presence, routing, paging and duplicate control (Router), ids and sizes only
//   - epoch plans for the next four epochs, signed with the project key and pushed to every relay
// Message bodies never pass through: the Hub refuses any frame that carries one.
import { Hub } from "./hub.js";
import { Router, h24 } from "./router.js";
import { Learner, solarFactor } from "./propagation.js";
import { EPOCH_S, epochOf, plan, bond, hopGrants } from "./planner.js";

export const LOOKAHEAD_EPOCHS = 4;
export const HTTP_ONLINE_MS = 30e3;
/** Longest message blob (base64 characters) that may pass through: a chat or a short mail. */
export const MAX_BLOB_CHARS = 12_000;
/** Receipts wait for this long, and no more than this many are kept in RAM (oldest dropped first). */
const ORIGIN_KEEP_MS = 30 * 60_000;
const MAX_ORIGINS = 5000;

/**
 * The text that is signed and sent: "P1|epoch|seed|relay:lane:slot,...|relay:slot:first:count,...|from>to:lane:slot,...".
 * Grants first (same order as the planner), then the bonded runs, then the hop grants.
 */
export function planText(p, seed, bonds = [], hops = []) {
  const grants = p.grants.map((g) => `${g.relay}:${g.lane}:${g.slot}`).join(",");
  const runs = bonds.filter((b) => b.count > 1).map((b) => `${b.relay}:${b.slot}:${b.first}:${b.count}`).join(",");
  const jumps = hops.map((h) => `${h.relay}>${h.target}:${h.lane}:${h.slot}`).join(",");
  return `P1|${p.epoch}|${seed >>> 0}|${grants}|${runs}|${jumps}`;
}

export class Mesh {
  /**
   * @param {{ sign: (text: string) => Promise<string>, seed?: number }} opts
   */
  constructor({ sign, seed = 1, router, learner = new Learner() } = {}) {
    this.sign = sign;
    this.seed = seed;
    this.learner = learner;
    this.solar = {};
    this.dirty = false; // the learner changed since it was saved
    // How likely a relay is to deliver to a receiver: what the network measured (per relay, receiver and
    // hour of day), with a first guess from the sun's output and the field as the prior.
    this.router =
      router ??
      new Router({
        pOf: (relay, dest, now) => this.learner.p(relay, `h${dest}`, new Date(now).getUTCHours(), now, 0.5 * solarFactor(this.solar)),
      });
    this.names = new Map(); // relay id (h24) -> callsign
    this.queues = new Map(); // callsign -> { bytes, urgency }
    this.origin = new Map(); // message id -> origin relay callsign (RAM, for the receipt)
    this.published = new Set(); // epochs already pushed
    this.http = new Map(); // relay without a socket -> last time it called
    this.strikes = new Map(); // relay -> times other relays reported it off plan
    this.suspended = new Map(); // relay -> until (ms)
    this.hub = new Hub({
      onUp: (call) => this.router.relayUp(call, { queue: 0, capacity: 8 }),
      onDown: (call) => {
        this.router.relayDown(call);
        this.queues.delete(call);
      },
    });
  }

  /** A relay connected (its identity was checked by the caller). The second socket of a key kicks the first. */
  connect(call, socket, now) {
    this.names.set(h24(call), call);
    this.hub.connect(call, socket, now);
  }

  disconnect(call, socket) {
    this.hub.disconnect(call, socket);
  }

  /** One frame from a relay. Returns a reply frame to send back, or null. Only ids, sizes and hashes. */
  onFrame(call, msg, now) {
    switch (msg?.t) {
      case "beat":
        this.hub.heartbeat(call, now);
        return null;
      case "queue":
        this.queues.set(call, {
          bytes: Math.max(0, msg.bytes | 0),
          urgency: Math.max(0, msg.urgency | 0),
          lanes: Math.min(4, Math.max(1, msg.lanes | 0 || 1)), // lanes it would like to bond (a strong link, a big transfer)
          hops: Array.isArray(msg.hops) ? msg.hops.slice(0, 4).map((h) => h | 0) : [], // relays it wants to send to over the air
        });
        this.router.setQueue(call, Math.max(0, msg.items | 0));
        return null;
      case "heard":
        this.router.heard(call, msg.dest | 0, +msg.snr || 0, msg.lane | 0, now);
        this.learn(call, msg.dest | 0, true, now);
        return null;
      case "here": {
        // An operator answered a page at this relay.
        const r = this.router.reply(call, msg.dest | 0, +msg.snr || 0, msg.lane | 0, now);
        return { t: "found", dest: msg.dest | 0, via: r.relay };
      }
      case "submit": {
        if (!["string", "number"].includes(typeof msg.id) || String(msg.id).length > 40) return null;
        // A relay took a message from an operator; find who delivers it.
        if (!this.router.firstSight(msg.id, now)) return { t: "dup", id: msg.id };
        // The blob (the message, as the relay took it off the air) only passes through RAM to the delivering relay.
        const blob = typeof msg.blob === "string" && msg.blob.length <= MAX_BLOB_CHARS ? msg.blob : undefined;
        if (this.origin.size >= MAX_ORIGINS) this.origin.delete(this.origin.keys().next().value);
        this.origin.set(msg.id, { call, dest: msg.dest | 0, at: now });
        const r = this.router.route(msg.dest | 0, now);
        if (r.state === "unknown") {
          this.router.page(msg.dest | 0, now);
          return { t: "unknown", id: msg.id };
        }
        this.hub.push(r.relay, { t: "deliver", id: msg.id, dest: msg.dest | 0, size: msg.size | 0, from: call, fresh: !r.pageFirst, ...(blob ? { blob } : {}) }, now);
        return { t: "routed", id: msg.id, via: r.relay, second: r.second };
      }
      case "receipt": {
        const origin = this.origin.get(msg.id);
        if (origin) {
          const status = msg.status === "failed" ? "failed" : "delivered";
          this.learn(call, origin.dest, status === "delivered", now);
          this.hub.push(origin.call, { t: "receipt", id: msg.id, dest: origin.dest, status, reason: String(msg.reason ?? "").slice(0, 80), at: now, relay: call }, now);
          this.origin.delete(msg.id);
        }
        return null;
      }
      case "clash": {
        // This relay heard another KAHF transmitter on its own planned lane and slot. The other relay is
        // transmitting off plan (or its clock is wrong): three reports in ten minutes suspend it for an hour.
        const other = typeof msg.other === "string" ? msg.other.toUpperCase().slice(0, 13) : "";
        if (!other || other === call) return null;
        const list = (this.strikes.get(other) ?? []).filter((x) => now - x.at < 10 * 60e3);
        list.push({ at: now, by: call });
        this.strikes.set(other, list);
        // Three reports from at least two different relays: one relay alone cannot silence another.
        if (list.length >= 3 && new Set(list.map((x) => x.by)).size >= 2) {
          this.suspended.set(other, now + 3600e3);
          this.hub.push(other, { t: "suspended", until: now + 3600e3, why: "off plan" }, now);
          this.queues.delete(other);
        }
        return null;
      }
      default:
        return null;
    }
  }

  /** One measurement: relay `call` did (or did not) reach receiver hash `dest` at this hour. */
  learn(call, dest, ok, now) {
    this.learner.observe(call, `h${dest}`, new Date(now).getUTCHours(), ok, now);
    this.dirty = true;
  }

  /** Today's solar flux and K index (from NOAA, hourly), which move every first guess. */
  setSolar(solar) {
    this.solar = { sfi: Number.isFinite(solar?.sfi) ? solar.sfi : undefined, kp: Number.isFinite(solar?.kp) ? solar.kp : undefined };
  }

  /** HTTPS fallback: frames from a relay with no socket. It counts as online for 30 s after each call. */
  httpFrames(call, frames, now) {
    this.touch(call, now);
    return frames.map((f) => this.onFrame(call, f, now)).filter(Boolean);
  }

  touch(call, now) {
    if (this.hub.sockets.has(call)) return;
    this.names.set(h24(call), call);
    const r = this.router.relays.get(call);
    if (!r || !r.online) this.router.relayUp(call, { queue: 0, capacity: 8 });
    this.http.set(call, now);
  }

  /** HTTPS fallback: the frames waiting for a relay (nothing older than 60 s). */
  poll(call, now) {
    this.touch(call, now);
    return this.hub.poll(call, now);
  }

  /** Relays that asked for air time, for the planner. */
  relayRows(now = Date.now()) {
    const rows = [];
    for (const [call, q] of this.queues) {
      if (!this.hub.sockets.has(call) && !this.http.has(call)) continue;
      if ((this.suspended.get(call) ?? 0) > now) continue;
      rows.push({ id: h24(call), queueBytes: q.bytes, urgency: q.urgency });
    }
    return rows;
  }

  /** Signs and pushes the plans for the next epochs that were not pushed yet. Returns the epochs sent. */
  async publish(now) {
    const first = epochOf(now / 1000) + 1;
    const sent = [];
    for (let e = first; e < first + LOOKAHEAD_EPOCHS; e++) {
      if (this.published.has(e)) continue;
      const p = plan(e, this.seed, this.relayRows(now));
      const wants = [], requests = [];
      for (const [call, q] of this.queues) {
        if (q.lanes > 1) wants.push([h24(call), q.lanes]);
        for (const to of q.hops ?? []) requests.push([h24(call), to]);
      }
      const text = planText(p, this.seed, bond(p, wants), hopGrants(p, requests));
      const frame = { t: "plan", epoch: e, plan: text, sig: await this.sign(text) };
      const to = new Set([...this.hub.sockets.keys(), ...this.http.keys()]);
      for (const call of to) this.hub.push(call, frame, now);
      this.published.add(e);
      sent.push(e);
    }
    for (const e of [...this.published]) if (e < first) this.published.delete(e);
    return sent;
  }

  /** Housekeeping: dead sockets, expired holds, finished pages. Call every few seconds. */
  tick(now) {
    for (const [id, o] of [...this.origin]) if (now - o.at > ORIGIN_KEEP_MS) this.origin.delete(id);
    this.hub.tick(now);
    this.router.tick(now);
    for (const [call, at] of [...this.http]) {
      if (now - at > HTTP_ONLINE_MS) {
        this.http.delete(call);
        if (!this.hub.sockets.has(call)) {
          this.router.relayDown(call);
          this.queues.delete(call);
        }
      }
    }
  }
}

export { EPOCH_S };

/** One operation from the Worker (the HTTPS path), shared by the Durable Object and the test store. */
export function meshOp(mesh, op, args, now = Date.now()) {
  if (op === "frames") return { replies: mesh.httpFrames(args.call, args.frames ?? [], now) };
  if (op === "poll") return { frames: mesh.poll(args.call, now) };
  if (op === "status") return { relays: mesh.hub.sockets.size + mesh.http.size };
  if (op === "solar") {
    mesh.setSolar(args);
    return { ok: true };
  }
  throw new Error("unknown op");
}
