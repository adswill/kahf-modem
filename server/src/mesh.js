// The mesh brain (MESH-SPEC.md 4, 5.2, 6): what the one Durable Object behind the relays'
// WebSockets does. Pure logic with injected time and signer, so it runs under Node in the tests.
//   - one socket per relay key (Hub), heartbeats, items held 60 s in RAM for the HTTPS poll
//   - presence, routing, paging and duplicate control (Router), ids and sizes only
//   - epoch plans for the next four epochs, signed with the project key and pushed to every relay
// Message bodies never pass through: the Hub refuses any frame that carries one.
import { Hub } from "./hub.js";
import { Router, h24 } from "./router.js";
import { EPOCH_S, epochOf, plan } from "./planner.js";

export const LOOKAHEAD_EPOCHS = 4;
export const HTTP_ONLINE_MS = 30e3;
/** Longest message blob (base64 characters) that may pass through: a chat or a short mail. */
export const MAX_BLOB_CHARS = 12_000;

/** The text that is signed and sent: "P1|epoch|seed|relay:lane:slot,...". Same order as the planner. */
export function planText(p, seed) {
  return `P1|${p.epoch}|${seed >>> 0}|${p.grants.map((g) => `${g.relay}:${g.lane}:${g.slot}`).join(",")}`;
}

export class Mesh {
  /**
   * @param {{ sign: (text: string) => Promise<string>, seed?: number }} opts
   */
  constructor({ sign, seed = 1, router = new Router() } = {}) {
    this.sign = sign;
    this.seed = seed;
    this.router = router;
    this.names = new Map(); // relay id (h24) -> callsign
    this.queues = new Map(); // callsign -> { bytes, urgency }
    this.origin = new Map(); // message id -> origin relay callsign (RAM, for the receipt)
    this.published = new Set(); // epochs already pushed
    this.http = new Map(); // relay without a socket -> last time it called
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
        this.queues.set(call, { bytes: Math.max(0, msg.bytes | 0), urgency: Math.max(0, msg.urgency | 0) });
        this.router.setQueue(call, Math.max(0, msg.items | 0));
        return null;
      case "heard":
        this.router.heard(call, msg.dest | 0, +msg.snr || 0, msg.lane | 0, now);
        return null;
      case "here": {
        // An operator answered a page at this relay.
        const r = this.router.reply(call, msg.dest | 0, +msg.snr || 0, msg.lane | 0, now);
        return { t: "found", dest: msg.dest | 0, via: r.relay };
      }
      case "submit": {
        // A relay took a message from an operator; find who delivers it.
        if (!this.router.firstSight(msg.id, now)) return { t: "dup", id: msg.id };
        // The blob (the message, as the relay took it off the air) only passes through RAM to the delivering relay.
        const blob = typeof msg.blob === "string" && msg.blob.length <= MAX_BLOB_CHARS ? msg.blob : undefined;
        this.origin.set(msg.id, { call, dest: msg.dest | 0 });
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
          this.hub.push(origin.call, { t: "receipt", id: msg.id, dest: origin.dest, status, reason: String(msg.reason ?? "").slice(0, 80), at: now, relay: call }, now);
          this.origin.delete(msg.id);
        }
        return null;
      }
      case "clash":
        return null; // the planner treats the two relays as one domain once the clash graph exists (WP6 later)
      default:
        return null;
    }
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
  relayRows() {
    const rows = [];
    for (const [call, q] of this.queues) {
      if (!this.hub.sockets.has(call) && !this.http.has(call)) continue;
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
      const p = plan(e, this.seed, this.relayRows());
      const text = planText(p, this.seed);
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
  throw new Error("unknown op");
}
