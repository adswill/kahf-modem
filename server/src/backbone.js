// The HF mesh backbone: relays talk to each other over the internet through this server.
// Relays are always online; operators are radio-only. See docs/NETWORK-SCENARIOS.md, section H.
//
// Endpoints (all POST bodies are JSON, signed by a REGISTERED relay's Ed25519 key):
//   POST /relay/heard   {relay, heard:[{callsign, snr?, at}], ts}      which relay last heard which callsign
//   POST /msg/submit    {relay, id, origin, dest, kind, subject?, body, created, ttl_s, hops, urgent?}
//   POST /msg/pull      {relay, callsigns:[...], ts}                   queued messages and receipts for heard callsigns
//   POST /msg/receipt   {relay, id, status, reason?}                   delivered or failed: payload is removed
//   GET  /relay/wanted  public: destination callsigns that have queued messages
//
// Signing format (same as /register and /schedule): the body is the fields above plus
// `public_key` (base64, 32 bytes) and `signature` (base64, 64 bytes). The signature is Ed25519
// over canonicalJson(body without public_key and signature): keys sorted, no whitespace.
// The relay must be registered and public_key must be the key it registered with.
// `body` of a message is base64 of the message bytes (at most 1024 bytes for chat, 4096 for mail).
//
// This file holds the validators and the Durable Object operations. The routes are in
// routes.js and the storage wiring is in store.ts and objects.js.
import { decodeBase64, encodeBase64, normalizeCall, signedMessage } from "./core.js";

export const MAX_BACKBONE_BODY_BYTES = 16_384;
export const MAX_HEARD_ENTRIES = 200;
export const HEARD_TTL_S = 2 * 3600;
export const MAX_PULL_CALLSIGNS = 100;
export const MAX_PULL_MESSAGES = 20;
export const MAX_WANTED = 500;
export const CHAT_MAX_BYTES = 1024;
export const MAIL_MAX_BYTES = 4096;
export const TTL_MIN_S = 30 * 60;
export const TTL_MAX_S = 48 * 3600;
export const MAX_HOPS = 16;
/** A handed-out message with no receipt after this long can be pulled by any relay again. */
export const HANDOUT_WINDOW_MS = 10 * 60 * 1000;
export const RECEIPT_TTL_MS = 24 * 3600 * 1000;
/** Claims on ids are kept this long, so a late duplicate submit is still recognised. */
export const CLAIM_TTL_MS = 50 * 3600 * 1000;
export const TS_WINDOW_S = 600;
export const RELAY_RATE_PER_MINUTE = 600;
const MAX_HEARD_STORED = 20_000;

const ID_RE = /^([A-Z0-9/]{1,13})-(\d{1,10})$/;
const BASE64_RE = /^[A-Za-z0-9+/]+={0,2}$/;

const SUBMIT_KEYS = ["relay", "id", "origin", "dest", "kind", "body", "created", "ttl_s", "hops", "public_key", "signature"];
const SUBMIT_OPTIONAL = ["subject", "urgent"];

function shape(body, required, optional = []) {
  if (!body || typeof body !== "object" || Array.isArray(body)) return "body must be a JSON object";
  for (const key of Object.keys(body)) {
    if (!required.includes(key) && !optional.includes(key)) return "body has a field that is not allowed";
  }
  for (const key of required) if (!(key in body)) return `missing field: ${key}`;
  return null;
}

/** The key and signature fields every backbone request carries. */
function keyFields(b) {
  const publicKeyBytes = decodeBase64(b.public_key, 32);
  if (!publicKeyBytes) return { error: "public_key must be base64 of a 32 byte Ed25519 key" };
  if (!decodeBase64(b.signature, 64)) return { error: "signature must be base64 of a 64 byte Ed25519 signature" };
  return { publicKey: encodeBase64(publicKeyBytes), signature: b.signature, message: signedMessage(b) };
}

const isWhole = (v, min, max) => Number.isSafeInteger(v) && v >= min && v <= max;

function checkTs(ts, nowS) {
  if (!Number.isSafeInteger(ts) || Math.abs(ts - nowS) > TS_WINDOW_S) {
    return `ts must be the current time in unix seconds (within ${TS_WINDOW_S / 60} minutes)`;
  }
  return null;
}

/** Splits an id `ORIGIN-<counter>` into its origin and counter, or null. */
export function parseMessageId(id) {
  if (typeof id !== "string") return null;
  const m = ID_RE.exec(id);
  return m ? { origin: m[1], counter: m[2] } : null;
}

/** Byte length of a canonical base64 string, or null if it is not canonical base64. */
export function base64DecodedLength(value) {
  if (typeof value !== "string" || value.length === 0 || value.length % 4 !== 0 || !BASE64_RE.test(value)) return null;
  try {
    return atob(value).length;
  } catch {
    return null;
  }
}

export function validateHeard(body, nowMs) {
  const bad = shape(body, ["relay", "heard", "ts", "public_key", "signature"]);
  if (bad) return { ok: false, error: bad };
  const b = body;
  const relay = normalizeCall(b.relay);
  if (!relay) return { ok: false, error: "relay is not a valid callsign" };
  const nowS = Math.floor(nowMs / 1000);
  const tsError = checkTs(b.ts, nowS);
  if (tsError) return { ok: false, error: tsError };
  if (!Array.isArray(b.heard) || b.heard.length < 1 || b.heard.length > MAX_HEARD_ENTRIES) {
    return { ok: false, error: `heard must be a list of 1 to ${MAX_HEARD_ENTRIES} entries` };
  }
  const heard = [];
  for (let i = 0; i < b.heard.length; i++) {
    const e = b.heard[i];
    const err = shape(e, ["callsign", "at"], ["snr"]);
    if (err) return { ok: false, error: `heard[${i}]: ${err}` };
    const callsign = normalizeCall(e.callsign);
    if (!callsign) return { ok: false, error: `heard[${i}]: callsign is not valid` };
    if (!Number.isSafeInteger(e.at) || e.at > nowS + TS_WINDOW_S) {
      return { ok: false, error: `heard[${i}]: at must be a unix time in seconds, not in the future` };
    }
    if (e.snr !== undefined && !(typeof e.snr === "number" && Number.isFinite(e.snr) && e.snr >= -60 && e.snr <= 80)) {
      return { ok: false, error: `heard[${i}]: snr must be a number from -60 to 80` };
    }
    heard.push({ callsign, at: e.at, ...(e.snr !== undefined ? { snr: e.snr } : {}) });
  }
  const k = keyFields(b);
  if (k.error) return { ok: false, error: k.error };
  return { ok: true, relay, heard, ...k };
}

export function validateSubmit(body, nowMs) {
  const bad = shape(body, SUBMIT_KEYS, SUBMIT_OPTIONAL);
  if (bad) return { ok: false, error: bad };
  const b = body;
  const relay = normalizeCall(b.relay);
  if (!relay) return { ok: false, error: "relay is not a valid callsign" };
  const origin = normalizeCall(b.origin);
  if (!origin) return { ok: false, error: "origin is not a valid callsign" };
  const dest = normalizeCall(b.dest);
  if (!dest) return { ok: false, error: "dest is not a valid callsign" };
  if (origin === dest) return { ok: false, error: "origin and dest must differ" };
  const id = parseMessageId(b.id);
  if (!id || id.origin !== origin) return { ok: false, error: "id must look like ORIGIN-<counter> with the origin callsign" };
  if (b.kind !== "chat" && b.kind !== "mail") return { ok: false, error: 'kind must be "chat" or "mail"' };
  const length = base64DecodedLength(b.body);
  const limit = b.kind === "chat" ? CHAT_MAX_BYTES : MAIL_MAX_BYTES;
  if (length === null) return { ok: false, error: "body must be base64" };
  if (length < 1 || length > limit) return { ok: false, error: `body must be 1 to ${limit} bytes for ${b.kind}` };
  if (b.subject !== undefined && (typeof b.subject !== "string" || b.subject.length > 100 || /[\x00-\x1f]/.test(b.subject))) {
    return { ok: false, error: "subject must be text of at most 100 characters" };
  }
  if (b.urgent !== undefined && typeof b.urgent !== "boolean") return { ok: false, error: "urgent must be true or false" };
  const nowS = Math.floor(nowMs / 1000);
  if (!Number.isSafeInteger(b.created) || b.created < 0 || b.created > nowS + TS_WINDOW_S) {
    return { ok: false, error: "created must be a unix time in seconds, not in the future" };
  }
  if (!Number.isSafeInteger(b.ttl_s) || b.ttl_s < 1) return { ok: false, error: "ttl_s must be a whole number of seconds" };
  if (!isWhole(b.hops, 0, MAX_HOPS)) return { ok: false, error: `hops must be a whole number from 0 to ${MAX_HOPS}` };
  const k = keyFields(b);
  if (k.error) return { ok: false, error: k.error };
  return {
    ok: true,
    relay,
    message: {
      id: b.id,
      origin,
      dest,
      kind: b.kind,
      ...(b.subject !== undefined ? { subject: b.subject } : {}),
      body: b.body,
      created: b.created,
      ttl_s: Math.min(TTL_MAX_S, Math.max(TTL_MIN_S, b.ttl_s)),
      hops: b.hops,
      urgent: b.urgent === true,
    },
    ...k,
  };
}

export function validatePull(body, nowMs) {
  const bad = shape(body, ["relay", "callsigns", "ts", "public_key", "signature"]);
  if (bad) return { ok: false, error: bad };
  const b = body;
  const relay = normalizeCall(b.relay);
  if (!relay) return { ok: false, error: "relay is not a valid callsign" };
  const tsError = checkTs(b.ts, Math.floor(nowMs / 1000));
  if (tsError) return { ok: false, error: tsError };
  if (!Array.isArray(b.callsigns) || b.callsigns.length < 1 || b.callsigns.length > MAX_PULL_CALLSIGNS) {
    return { ok: false, error: `callsigns must be a list of 1 to ${MAX_PULL_CALLSIGNS} callsigns` };
  }
  const calls = new Set();
  for (let i = 0; i < b.callsigns.length; i++) {
    const c = normalizeCall(b.callsigns[i]);
    if (!c) return { ok: false, error: `callsigns[${i}] is not valid` };
    calls.add(c);
  }
  const k = keyFields(b);
  if (k.error) return { ok: false, error: k.error };
  return { ok: true, relay, callsigns: [...calls], ...k };
}

export function validateReceipt(body) {
  const bad = shape(body, ["relay", "id", "status", "public_key", "signature"], ["reason"]);
  if (bad) return { ok: false, error: bad };
  const b = body;
  const relay = normalizeCall(b.relay);
  if (!relay) return { ok: false, error: "relay is not a valid callsign" };
  if (!parseMessageId(b.id)) return { ok: false, error: "id must look like ORIGIN-<counter>" };
  if (b.status !== "delivered" && b.status !== "failed") return { ok: false, error: 'status must be "delivered" or "failed"' };
  if (b.reason !== undefined && (typeof b.reason !== "string" || b.reason.length > 120 || /[\x00-\x1f]/.test(b.reason))) {
    return { ok: false, error: "reason must be text of at most 120 characters" };
  }
  const k = keyFields(b);
  if (k.error) return { ok: false, error: k.error };
  return { ok: true, relay, id: b.id, status: b.status, ...(b.reason !== undefined ? { reason: b.reason } : {}), ...k };
}

// ---------------------------------------------------------------------------
// Durable Object operations. Same style as objects.js: (storage, args) -> result.
// ---------------------------------------------------------------------------

const MAIL = "mail";
const INDEX = "index";

const emptyMail = () => ({ queue: {}, receipts: {}, claims: {} });

async function readMail(s, nowMs) {
  const state = (await s.get(MAIL)) ?? emptyMail();
  for (const [id, c] of Object.entries(state.claims)) if (nowMs - c.at >= CLAIM_TTL_MS) delete state.claims[id];
  for (const [id, r] of Object.entries(state.receipts)) if (nowMs - r.at_ms >= RECEIPT_TTL_MS) delete state.receipts[id];
  return state;
}

const pullable = (m, nowMs) => m.handed_at === undefined || nowMs - m.handed_at >= HANDOUT_WINDOW_MS;

/** One object per callsign. As a destination it holds the queue; as an origin, claims and receipts. */
export const mailOps = {
  /** First submit of an id wins (kept on the origin's object). */
  async claim(s, { id, dest, nowMs }) {
    const state = await readMail(s, nowMs);
    if (Object.hasOwn(state.claims, id)) {
      await s.put(MAIL, state);
      return { ok: true, duplicate: true };
    }
    state.claims[id] = { dest, at: nowMs, status: "queued" };
    await s.put(MAIL, state);
    return { ok: true, duplicate: false };
  },

  async getClaim(s, { id, nowMs }) {
    const state = await readMail(s, nowMs);
    return { claim: state.claims[id] ?? null };
  },

  /** Stores a message in this destination's queue (nothing if the id is already there). */
  async enqueue(s, { message, nowMs }) {
    const state = await readMail(s, nowMs);
    if (!Object.hasOwn(state.queue, message.id)) {
      state.queue[message.id] = { ...message, queued_at: nowMs, expires_at: nowMs + message.ttl_s * 1000 };
    }
    await s.put(MAIL, state);
    const until = Math.max(...Object.values(state.queue).map((m) => m.expires_at));
    return { ok: true, until };
  },

  /** Messages that can be handed out now. Expired ones are dropped and reported. */
  async peek(s, { nowMs }) {
    const state = await readMail(s, nowMs);
    const expired = [];
    for (const [id, m] of Object.entries(state.queue)) {
      if (nowMs >= m.expires_at) {
        expired.push({ id, origin: m.origin, dest: m.dest });
        delete state.queue[id];
      }
    }
    await s.put(MAIL, state);
    return {
      messages: Object.values(state.queue).filter((m) => pullable(m, nowMs)),
      expired,
      remaining: Object.keys(state.queue).length,
    };
  },

  /** Hands the listed ids to a relay if they are still pullable (another relay may have won). */
  async handout(s, { ids, relay, nowMs }) {
    const state = await readMail(s, nowMs);
    const messages = [];
    for (const id of ids) {
      const m = state.queue[id];
      if (m && nowMs < m.expires_at && pullable(m, nowMs)) {
        m.handed_to = relay;
        m.handed_at = nowMs;
        messages.push(m);
      }
    }
    await s.put(MAIL, state);
    return { messages };
  },

  /** A relay reports the outcome. The payload goes away. */
  async complete(s, { id, relay, nowMs }) {
    const state = await readMail(s, nowMs);
    const m = state.queue[id];
    if (!m) {
      await s.put(MAIL, state);
      return { ok: true, found: false, remaining: Object.keys(state.queue).length };
    }
    if (m.handed_to !== relay && (m.handed_at === undefined || nowMs - m.handed_at < HANDOUT_WINDOW_MS)) {
      return { ok: false, error: "this message was not handed to this relay" };
    }
    const dest = m.dest;
    delete state.queue[id];
    await s.put(MAIL, state);
    return { ok: true, found: true, dest, remaining: Object.keys(state.queue).length };
  },

  /** Stores a short receipt for this origin and marks the claim finished. */
  async putReceipt(s, { id, dest, status, reason, nowMs }) {
    const state = await readMail(s, nowMs);
    state.receipts[id] = { id, dest, status, ...(reason ? { reason } : {}), at: Math.floor(nowMs / 1000), at_ms: nowMs };
    if (state.claims[id]) state.claims[id].status = status;
    await s.put(MAIL, state);
    return { ok: true };
  },

  /** Receipts this origin has not been given yet. Each is given once and kept for 24 h. */
  async takeReceipts(s, { nowMs }) {
    const state = await readMail(s, nowMs);
    const out = [];
    for (const r of Object.values(state.receipts)) {
      if (!r.fetched) {
        r.fetched = true;
        out.push({ id: r.id, dest: r.dest, status: r.status, ...(r.reason ? { reason: r.reason } : {}), at: r.at });
      }
    }
    await s.put(MAIL, state);
    return { receipts: out };
  },
};

const emptyIndex = () => ({ heard: {}, wanted: {}, rate: {} });

/** The single index object: who heard whom, which destinations have mail, per-relay rate counters. */
export const indexOps = {
  async noteHeard(s, { relay, heard, nowMs }) {
    const nowS = Math.floor(nowMs / 1000);
    const state = (await s.get(INDEX)) ?? emptyIndex();
    let recorded = 0;
    for (const [call, h] of Object.entries(state.heard)) if (nowS - h.at >= HEARD_TTL_S) delete state.heard[call];
    for (const e of heard) {
      if (nowS - e.at >= HEARD_TTL_S) continue;
      const old = state.heard[e.callsign];
      if (old && old.at > e.at) continue;
      state.heard[e.callsign] = { relay, at: e.at, ...(e.snr !== undefined ? { snr: e.snr } : {}) };
      recorded++;
    }
    const calls = Object.keys(state.heard);
    if (calls.length > MAX_HEARD_STORED) {
      calls.sort((a, b) => state.heard[a].at - state.heard[b].at);
      for (const c of calls.slice(0, calls.length - MAX_HEARD_STORED)) delete state.heard[c];
    }
    await s.put(INDEX, state);
    return { ok: true, recorded, ignored: heard.length - recorded };
  },

  async getHeard(s, { callsign, nowMs }) {
    const state = (await s.get(INDEX)) ?? emptyIndex();
    const h = state.heard[callsign];
    return { heard: h && Math.floor(nowMs / 1000) - h.at < HEARD_TTL_S ? { callsign, ...h } : null };
  },

  async want(s, { dest, until }) {
    const state = (await s.get(INDEX)) ?? emptyIndex();
    state.wanted[dest] = Math.max(until, state.wanted[dest] ?? 0);
    await s.put(INDEX, state);
    return { ok: true };
  },

  async unwant(s, { dest }) {
    const state = (await s.get(INDEX)) ?? emptyIndex();
    delete state.wanted[dest];
    await s.put(INDEX, state);
    return { ok: true };
  },

  async wanted(s, { nowMs }) {
    const state = (await s.get(INDEX)) ?? emptyIndex();
    const live = Object.entries(state.wanted).filter(([, until]) => until > nowMs).map(([dest]) => dest).sort();
    return { wanted: live.slice(0, MAX_WANTED) };
  },

  /** Counts one request for a relay in the current minute; refuses past the limit. */
  async rate(s, { relay, nowMs, limit }) {
    const state = (await s.get(INDEX)) ?? emptyIndex();
    const win = Math.floor(nowMs / 60_000);
    for (const [r, v] of Object.entries(state.rate)) if (v.win !== win) delete state.rate[r];
    const v = state.rate[relay] ?? { win, n: 0 };
    v.n++;
    state.rate[relay] = v;
    await s.put(INDEX, state);
    return { ok: v.n <= limit };
  },
};

