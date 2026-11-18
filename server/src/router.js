// Mesh routing and presence (MESH-SPEC.md section 6 and 7). Pure and in memory: every function
// takes `now` (ms). No message body is ever an input: only ids, sizes, hashes and routing
// facts, so there is nothing to store (spec 7.1). The old queued-message backbone (backbone.js)
// is a different, older design and is not touched here.
import { createHash } from "node:crypto";

export const PRESENCE_TTL_MS = 6 * 3600e3;
export const FRESH_MS = 2 * 60e3; // heard this recently: no page needed
export const DEDUPE_MS = 2 * 3600e3;
export const PAGE_CYCLES = 3;
export const PAGE_WINDOW_S = 20;
export const PAGE_PERIOD_MS = 10 * 60e3;
export const EPOCH_MS = 12e3;
export const PAGE_CELLS_PER_CYCLE = 12; // 2 epochs x 6 slots in a 20 s window

// 24 bit hash of a callsign: first three bytes of SHA-256 of the upper-case callsign (never 0).
export function h24(call) {
  const d = createHash("sha256").update(String(call).trim().toUpperCase()).digest();
  return (d[0] << 16) | (d[1] << 8) | d[2] || 1;
}

// Start (ms) of the next paging window: the first 20 s of every 10th minute of the UTC hour.
export function nextPagingWindow(nowMs) {
  return Math.ceil(nowMs / PAGE_PERIOD_MS) * PAGE_PERIOD_MS;
}

// When a sender should try again, and whether to give up (default lifetime 24 h). Attempts
// start at 0. The sender's own outbox keeps the message; the network keeps nothing.
export function nextRetry(attempt, nowMs, createdMs, lifetimeMs = 24 * 3600e3) {
  if (nowMs - createdMs >= lifetimeMs) return null;
  const wait = Math.min(10 * 60e3 * 2 ** Math.min(attempt, 3), 3600e3);
  return Math.min(nowMs + wait, createdMs + lifetimeMs);
}

export class Router {
  // opts.pOf(relayId, destHash, now) -> probability a delivery works (learner later, step 14).
  // opts.plausible(relayId, destHash, now) -> may this relay reach the destination's region.
  constructor(opts = {}) {
    this.pOf = opts.pOf ?? (() => 0.5);
    this.plausible = opts.plausible ?? (() => true);
    this.relays = new Map(); // id -> {queue, capacity, online}
    this.presence = new Map(); // hash -> Map(relayId -> {snr, lane, at})
    this.seen = new Map(); // hash of message id -> at
    this.pages = new Map(); // destHash -> page job
  }

  relayUp(id, { queue = 0, capacity = 8 } = {}) {
    this.relays.set(id, { queue, capacity, online: true });
  }
  relayDown(id) {
    const r = this.relays.get(id);
    if (r) r.online = false;
  }
  setQueue(id, queue) {
    const r = this.relays.get(id);
    if (r) r.queue = queue;
  }

  // A relay heard an operator (by hash; the server never learns a callsign it was not given).
  heard(relayId, destHash, snr, lane, now) {
    let m = this.presence.get(destHash);
    if (!m) this.presence.set(destHash, (m = new Map()));
    m.set(relayId, { snr, lane, at: now });
  }

  _fresh(now, at) {
    return Math.exp(-(now - at) / 3600e3);
  }

  // score(B) = P(deliver) * freshness / (1 + queue / capacity)
  _score(relayId, destHash, rec, now) {
    const r = this.relays.get(relayId);
    return (this.pOf(relayId, destHash, now) * this._fresh(now, rec.at)) / (1 + r.queue / r.capacity);
  }

  // Where does a message for `dest` go? { state, relay, second, pageFirst } where state is
  //   "known"   some online relay heard it in the last 6 h: deliver through the best, second warm;
  //   "unknown" nobody did: the caller must start paging (page()).
  route(dest, now) {
    const h = typeof dest === "number" ? dest : h24(dest);
    const m = this.presence.get(h);
    const ranked = [];
    if (m) {
      for (const [id, rec] of m) {
        const r = this.relays.get(id);
        if (!r?.online || now - rec.at > PRESENCE_TTL_MS) continue;
        ranked.push({ id, rec, score: this._score(id, h, rec, now) });
      }
    }
    ranked.sort((a, b) => b.score - a.score || a.id.localeCompare(b.id));
    if (!ranked.length) return { state: "unknown", relay: null, second: null, pageFirst: true };
    const best = ranked[0];
    return {
      state: "known",
      relay: best.id,
      second: ranked[1]?.id ?? null,
      // Heard 4 hours ago does not mean listening now: page unless it was very recent.
      pageFirst: now - best.rec.at > FRESH_MS,
    };
  }

  // Start paging `dest`. Plausible online relays take their own (epoch, slot) cells in the next
  // paging windows, 12 per window, so pages never share a cell. Up to three windows (about 30
  // minutes); after that the destination is "not reachable".
  // `opts.pattern` (a ListenPattern) makes the windows the ones the operator usually listens in.
  page(dest, now, opts = {}) {
    const h = typeof dest === "number" ? dest : h24(dest);
    const existing = this.pages.get(h);
    if (existing && !existing.done) return existing;
    const relays = [...this.relays].filter(([id, r]) => r.online && this.plausible(id, h, now)).map(([id]) => id).sort();
    const first = nextPagingWindow(now);
    // 12 cells per window (2 epochs x 6 slots). With more relays than cells the relays are split
    // into groups and the windows take the groups in turn; each page has its own cell.
    const groups = Math.max(1, Math.ceil(relays.length / PAGE_CELLS_PER_CYCLE));
    const job = { hash: h, started: now, windows: [], found: null, done: false, notReachable: false };
    const starts = opts.pattern?.bestWindows(now, PAGE_CYCLES) ?? Array.from({ length: PAGE_CYCLES }, (_, w) => first + w * PAGE_PERIOD_MS);
    for (let w = 0; w < PAGE_CYCLES; w++) {
      const g = w % groups;
      const members = relays.slice(g * PAGE_CELLS_PER_CYCLE, (g + 1) * PAGE_CELLS_PER_CYCLE);
      const start = starts[w];
      job.windows.push({
        start,
        pages: members.map((id, c) => ({ relay: id, at: start + Math.floor(c / 6) * EPOCH_MS, slot: c % 6 })),
      });
    }
    this.pages.set(h, job);
    return job;
  }

  // The operator answered a page ("here I am") at `relayId`. The other pages are cancelled and
  // delivery goes through the relay that heard the answer.
  reply(relayId, destHash, snr, lane, now) {
    this.heard(relayId, destHash, snr, lane, now);
    const job = this.pages.get(destHash);
    if (job && !job.done) {
      job.done = true;
      job.found = relayId;
    }
    return this.route(destHash, now);
  }

  // Call as time passes. After the third window with no answer the job ends not reachable.
  tick(now) {
    for (const job of this.pages.values()) {
      if (job.done) continue;
      const last = job.windows[job.windows.length - 1];
      if (now >= last.start + PAGE_WINDOW_S * 1000) {
        job.done = true;
        job.notReachable = true;
      }
    }
  }

  pageStatus(dest) {
    const job = this.pages.get(typeof dest === "number" ? dest : h24(dest));
    if (!job) return "none";
    return job.found ? "found" : job.notReachable ? "not_reachable" : "paging";
  }

  // Duplicate control without storage: only a hash of the message id, for two hours.
  firstSight(msgId, now) {
    for (const [k, at] of this.seen) if (now - at > DEDUPE_MS) this.seen.delete(k);
    const k = createHash("sha256").update(String(msgId)).digest("hex").slice(0, 16);
    if (this.seen.has(k)) return false;
    this.seen.set(k, now);
    return true;
  }
}
