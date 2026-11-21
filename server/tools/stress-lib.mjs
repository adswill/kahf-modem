// Scenario matrix for the KAHF station-map server. One list of scenarios, two runners:
//   - test/matrix.test.mjs runs every scenario in-process (node --test);
//   - tools/stress.mjs runs them in-process or against a running server URL.
// Apart from the band constants below, nothing is imported from ../src, so the same
// scenarios can drive a remote server. Expected values are worked out here on purpose, not
// read from the server's own logic.
import { register } from "node:module";
import { pathToFileURL } from "node:url";

// The band segment is the one value shared with the server: it comes from the single
// constant in src/core.js, so a segment change needs no edit here. Everything derived
// from it (channel count, frequencies, expected channels) is calculated again below.
// A remote run therefore expects the server under test to use the same segment as this
// checkout; a server deployed before the 17 m move fails every band scenario.
const { BAND_SEGMENT_KHZ, CHANNEL_WIDTH_KHZ } = await import("../src/core.js");
const LOW = BAND_SEGMENT_KHZ.low;
const HIGH = BAND_SEGMENT_KHZ.high;
const W10 = Math.round(CHANNEL_WIDTH_KHZ * 10); // channel width in tenths of a kHz
const NCH = Math.floor(((HIGH - LOW) * 10) / W10);
const LAST = NCH - 1;
/** Integer kHz inside channel n. */
const freqOf = (n) => Math.ceil(LOW + CHANNEL_WIDTH_KHZ * n);
/** A channel in the middle of the band (channel 1 of 3), used where any valid channel will do. */
const MID = Math.floor(NCH / 2);
/** The channel the default test report is heard on: channel 0, which is in the online pool. */
const BUSY_CH = 0;

const enc = new TextEncoder();
const BASE = "https://kahf.test";
export const NOW0 = Date.parse("2026-11-17T12:00:00Z");
const H = 3_600_000;
const MIN = 60_000;

// ---------------------------------------------------------------------------
// Harness: in-process (fake store, fake clock) and remote (fetch).
// ---------------------------------------------------------------------------

const CALL_BASE = Math.floor(Math.random() * 10 * 24 ** 4);
const CALL_SEQ = { n: 0 };

/** Shared parts of both harnesses: unique ids, signing, response logging. */
function baseHarness(kind) {
  let seq = 0;
  const rnd = (n, set) => Array.from({ length: n }, () => set[Math.floor(Math.random() * set.length)]).join("");
  const h = {
    kind,
    log: [],
    caps: { clock: false, seed: false, inproc: false, isolated: false },
    /** A fresh valid callsign, for example Z7QRTW. */
    call() {
      // Unique within the process (counter) and unlikely to repeat across runs (random start).
      let n = (CALL_BASE + CALL_SEQ.n++) % (10 * 24 ** 4);
      const letters = "ABCDEFGHJKLMNPQRSTUVWXYZ";
      let out = "";
      for (let i = 0; i < 4; i++) { out = letters[n % 24] + out; n = Math.floor(n / 24); }
      return `Z${n}${out}`;
    },
    /** A fresh valid one-degree area key. */
    area() { return `N${Math.floor(Math.random() * 90)}E${Math.floor(Math.random() * 180)}`; },
    async key() {
      const pair = await crypto.subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"]);
      const raw = new Uint8Array(await crypto.subtle.exportKey("raw", pair.publicKey));
      return { privateKey: pair.privateKey, publicKey: Buffer.from(raw).toString("base64") };
    },
    /** The body for `fields`, signed with `key` as the protocol requires. */
    async sign(key, fields) {
      const sig = new Uint8Array(await crypto.subtle.sign({ name: "Ed25519" }, key.privateKey, enc.encode(canonicalJson(fields))));
      return { ...fields, public_key: key.publicKey, signature: Buffer.from(sig).toString("base64") };
    },
    async send({ method = "GET", path, headers = {}, body }) {
      const res = await h._send({ method, path, headers, body });
      const text = await res.text();
      let json;
      try { json = JSON.parse(text); } catch { json = undefined; }
      const out = { status: res.status, headers: res.headers, text, json, contentType: res.headers.get("content-type") ?? "" };
      const preview = (b) => b === undefined ? "" : typeof b === "string" ? b : `<${b.length} bytes>`;
      h.log.push({
        req: { method, path: path.length > 120 ? path.slice(0, 120) + `...(${path.length} chars)` : path, headers, body: preview(body).length > 400 ? preview(body).slice(0, 400) + `...(${preview(body).length} chars)` : preview(body) },
        res: out,
      });
      h.last = h.log[h.log.length - 1];
      return out;
    },
    get(path, method = "GET") { return h.send({ method, path }); },
    /** POST JSON (object -> JSON.stringify, string -> as is). */
    post(path, body, headers = { "content-type": "application/json" }) {
      return h.send({ method: "POST", path, headers, body: typeof body === "string" || body instanceof Uint8Array ? body : JSON.stringify(body) });
    },
  };
  return h;
}

/** Runs the Worker's handleRequest in-process with a fake store and a fake clock. */
export async function makeInProcess({ slow = false } = {}) {
  const HOOK = `
import { readFile } from "node:fs/promises";
export async function load(url, context, nextLoad) {
  if (url.endsWith(".ts")) {
    const source = await readFile(new URL(url), "utf8");
    return { format: "module", source, shortCircuit: true };
  }
  return nextLoad(url, context);
}`;
  register("data:text/javascript," + encodeURIComponent(HOOK));
  const { handleRequest } = await import(pathToFileURL(new URL("../src/routes.js", import.meta.url).pathname).href);
  const { fakeStore } = await import("./fake-store.mjs");
  const h = baseHarness("in-process");
  let now = NOW0;
  const store = fakeStore({ slow });
  h.store = store;
  h.caps = { clock: true, seed: true, inproc: true, isolated: true };
  h.now = () => now;
  h.setNow = (ms) => { now = ms; };
  h.advance = (ms) => { now += ms; };
  h.seedRecord = (rec) => store.put(rec);
  h._send = ({ method, path, headers, body }) => {
    const init = { method, headers };
    if (body !== undefined && method !== "GET" && method !== "HEAD") init.body = body;
    return handleRequest(new Request(BASE + path, init), store, now);
  };
  return h;
}

/** Drives a running server over HTTP. The clock and the store are out of reach. */
export function makeRemote(baseUrl) {
  const h = baseHarness("remote");
  const base = baseUrl.replace(/\/+$/, "");
  h.now = () => Date.now();
  h._send = ({ method, path, headers, body }) => {
    const init = { method, headers, redirect: "manual" };
    if (body !== undefined && method !== "GET" && method !== "HEAD") init.body = body;
    return fetch(base + path, init);
  };
  return h;
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

export function canonicalJson(value) {
  if (value === null || typeof value !== "object") return JSON.stringify(value);
  if (Array.isArray(value)) return "[" + value.map(canonicalJson).join(",") + "]";
  const parts = Object.keys(value).sort().filter((k) => value[k] !== undefined)
    .map((k) => JSON.stringify(k) + ":" + canonicalJson(value[k]));
  return "{" + parts.join(",") + "}";
}

const iso = (ms) => new Date(Math.floor(ms / 1000) * 1000).toISOString().replace(/\.\d{3}Z$/, "Z");
const snip = (v, n = 160) => { const s = typeof v === "string" ? v : JSON.stringify(v); return s.length > n ? s.slice(0, n) + "..." : s; };
const brief = (r) => `${r.status} ${r.json?.error ?? (r.json?.ok ? "ok" : r.contentType.split(";")[0])}`;

class Fail extends Error {}
function check(cond, msg) { if (!cond) throw new Fail(msg); }
function eq(actual, want, label) {
  if (JSON.stringify(actual) !== JSON.stringify(want)) throw new Fail(`${label}: got ${snip(actual)}, want ${snip(want)}`);
}
function status(r, want, label = "status") {
  const ok = Array.isArray(want) ? want.includes(r.status) : r.status === want;
  if (!ok) throw new Fail(`${label}: got ${brief(r)}, want ${Array.isArray(want) ? want.join(" or ") : want}`);
}
function errMatches(r, rx) {
  check(typeof r.json?.error === "string" && rx.test(r.json.error), `error text ${snip(r.json?.error ?? r.text)} should match ${rx}`);
}
/** No response text may contain the raw hostile input (JSON-escaped or not). */
function noEcho(r, raw) {
  if (typeof raw !== "string" || raw.length < 3) return;
  check(!r.text.includes(raw) && !r.text.includes(JSON.stringify(raw).slice(1, -1)), `response echoes raw input ${snip(raw, 50)}: ${snip(r.text, 200)}`);
}

const LEAK = /\n\s+at\s|node:internal|\.(?:js|ts|mjs):\d+|\/Users\/|\bstack\b|TypeError|RangeError|SyntaxError|ReferenceError|Unexpected token|in JSON at position|wrangler/i;

/** Checks run on every response a scenario saw: JSON errors, no 5xx, no stack traces. */
function audit(h) {
  for (const { req, res } of h.log) {
    const where = `${req.method} ${req.path} -> ${res.status}`;
    check(res.status < 500, `${where}: server error ${snip(res.text)}`);
    check(!LEAK.test(res.text), `${where}: response looks like a leak: ${snip(res.text)}`);
    if (res.status >= 400 && req.method !== "HEAD") {
      check(/^application\/json/.test(res.contentType), `${where}: error is not JSON (content-type ${res.contentType})`);
      check(res.json && typeof res.json === "object" && Object.keys(res.json).join() === "error" && typeof res.json.error === "string", `${where}: error body must be {"error": string}: ${snip(res.text)}`);
      check(res.headers.get("x-content-type-options") === "nosniff", `${where}: missing nosniff`);
    }
    if (res.contentType.startsWith("text/html")) check(!!res.headers.get("content-security-policy"), `${where}: HTML without CSP`);
  }
}

// ---------------------------------------------------------------------------
// Request builders
// ---------------------------------------------------------------------------

/** A valid report for this harness's clock; `over` replaces fields (undefined removes one). */
function reportFields(h, over = {}) {
  const f = {
    reporter_call: h.call(),
    reporter_location: { mode: "precise", latitude: 51.5074, longitude: -0.1278 },
    heard_call: h.call(),
    heard_location: { mode: "area", latitude: 48.8566, longitude: 2.3522, radius_km: 50 },
    heard_at: iso(h.now() - MIN),
    frequency_khz: freqOf(BUSY_CH),
    mode: "KAHF",
  };
  for (const [k, v] of Object.entries(over)) { if (v === undefined) delete f[k]; else f[k] = v; }
  return f;
}

/** Signs reportFields and posts it. Returns { res, fields, key, body }. */
async function report(h, over = {}, key) {
  const k = key ?? await h.key();
  const fields = reportFields(h, over);
  const body = await h.sign(k, fields);
  const res = await h.post("/report", body);
  return { res, fields, key: k, body };
}

/** Post fields signed correctly except that a token is spliced raw into the JSON text. */
async function postRawToken(h, path, fields, pathInFields, token) {
  const copy = structuredClone(fields);
  let o = copy;
  for (const p of pathInFields.slice(0, -1)) o = o[p];
  o[pathInFields[pathInFields.length - 1]] = "@@RAW@@";
  const k = await h.key();
  const signed = await h.sign(k, copy);
  return h.post(path, JSON.stringify(signed).replace('"@@RAW@@"', token));
}

async function stationsOf(h, call) {
  const r = await h.get("/stations");
  status(r, 200);
  return r.json.stations.filter((s) => !call || s.reporter_call === call);
}

async function registerRelay(h, call, area, key) {
  const k = key ?? await h.key();
  const res = await h.post("/register", await h.sign(k, { area, callsign: call }));
  return { res, key: k };
}
async function schedule(h, relay, requests, key) {
  return h.post("/schedule", await h.sign(key, { relay, requests }));
}
const nowS = (h) => Math.floor(h.now() / 1000);
const tx = (channel, duration, extra = {}) => ({ channel, duration, direction: "tx", ...extra });

/** Independent channel number for an integer kHz, written without the server's constants. */
function expectedChannel(khz) {
  const tenths = Math.round((khz - LOW) * 10);
  if (tenths < 0) return null;
  const c = Math.floor(tenths / W10);
  return c < NCH ? c : null;
}

// ---------------------------------------------------------------------------
// Scenario registry
// ---------------------------------------------------------------------------

export const scenarios = [];
/** S(group, name, expected, fn, { needs, slow }) */
function S(group, name, expected, fn, opts = {}) { scenarios.push({ group, name, expected, fn, needs: [], ...opts }); }

const G = {
  sig: "1 Signatures", call: "2 Callsigns and fields", loc: "3 Locations", rad: "4 Radius and area",
  freq: "5 Frequencies and channels", sched: "6 Schedules", rem: "7 Removal", exp: "8 Expiry",
  conc: "9 Concurrency", size: "10 Size limits", http: "11 HTTP", page: "12 Map and homepage",
};

// ---- 1. Signatures --------------------------------------------------------

S(G.sig, "valid signature is accepted and listed without keys", "200 ok; /stations lists it, no public_key", async (h) => {
  const { res, fields } = await report(h);
  status(res, 200);
  eq(res.json, { ok: true, expires_after_s: 172800 }, "body");
  const list = await stationsOf(h, fields.reporter_call);
  eq(list.length, 1, "records for the reporter");
  check(!("public_key" in list[0]) && !("signature" in list[0]), "public record leaks key or signature");
  return brief(res);
});

S(G.sig, "key order and whitespace in the sent JSON do not matter", "200 (canonical JSON is signed)", async (h) => {
  const k = await h.key();
  const f = reportFields(h);
  const body = await h.sign(k, f);
  const shuffled = Object.fromEntries(Object.entries(body).reverse());
  status(await h.post("/report", JSON.stringify(shuffled, null, 4)), 200);
});

S(G.sig, "signature over pretty-printed (non-canonical) text is refused", "401", async (h) => {
  const k = await h.key();
  const f = reportFields(h);
  const sig = new Uint8Array(await crypto.subtle.sign({ name: "Ed25519" }, k.privateKey, enc.encode(JSON.stringify(f, null, 2))));
  const r = await h.post("/report", { ...f, public_key: k.publicKey, signature: Buffer.from(sig).toString("base64") });
  status(r, 401);
});

S(G.sig, "signature from the wrong key", "401 signature check failed", async (h) => {
  const a = await h.key(), b = await h.key();
  const body = await h.sign(a, reportFields(h));
  const r = await h.post("/report", { ...body, public_key: b.publicKey });
  status(r, 401);
  errMatches(r, /signature/);
});

S(G.sig, "one flipped bit in the signature", "401", async (h) => {
  const k = await h.key();
  const body = await h.sign(k, reportFields(h));
  const raw = Buffer.from(body.signature, "base64");
  raw[10] ^= 1;
  status(await h.post("/report", { ...body, signature: raw.toString("base64") }), 401);
});

S(G.sig, "signature over a different body (frequency changed after signing)", "401, nothing stored", async (h) => {
  const k = await h.key();
  const f = reportFields(h);
  const body = await h.sign(k, f);
  const r = await h.post("/report", { ...body, frequency_khz: LOW + 1 });
  status(r, 401);
  eq((await stationsOf(h, f.reporter_call)).length, 0, "stored records");
});

S(G.sig, "signature over a different body (callsign swapped)", "401", async (h) => {
  const k = await h.key();
  const body = await h.sign(k, reportFields(h));
  status(await h.post("/report", { ...body, heard_call: h.call() }), 401);
});

S(G.sig, "signature from another message (a removal signature on a report)", "401", async (h) => {
  const k = await h.key();
  const f = reportFields(h);
  const rem = await h.sign(k, { heard_call: f.heard_call, reporter_call: f.reporter_call, signed_at: iso(h.now()) });
  status(await h.post("/report", { ...f, public_key: k.publicKey, signature: rem.signature }), 401);
});

for (const [name, nbytes] of [["truncated (63 bytes)", 63], ["truncated (1 byte)", 1], ["oversized (65 bytes)", 65], ["oversized (128 bytes)", 128]]) {
  S(G.sig, `signature ${name}`, "400 signature must be base64 of 64 bytes", async (h) => {
    const k = await h.key();
    const f = reportFields(h);
    const body = await h.sign(k, f);
    const r = await h.post("/report", { ...body, signature: Buffer.alloc(nbytes, 7).toString("base64") });
    status(r, 400);
    errMatches(r, /signature/);
  });
}
for (const [name, val, want] of [
  ["zero-length", "", 400], ["not base64 (!!!)", "!!!!", 400], ["url-safe base64 alphabet", "-_" + "A".repeat(84) + "==", 400],
  ["a number", 12345, 400], ["null", null, 400],
  ["an array", ["a"], 400], ["an object", {}, 400], ["a boolean", true, 400],
  ["3000 characters (inside the body limit)", "A".repeat(3000), 400],
]) {
  S(G.sig, `signature ${name}`, `${want} ${want === 400 ? "refused as a bad signature" : ""}`, async (h) => {
    const k = await h.key();
    const body = await h.sign(k, reportFields(h));
    const r = await h.post("/report", { ...body, signature: val });
    status(r, want);
  });
}
S(G.sig, "signature base64 without its padding", "400", async (h) => {
  const k = await h.key();
  const body = await h.sign(k, reportFields(h));
  check(body.signature.endsWith("=="), "test premise: 64-byte base64 ends with ==");
  status(await h.post("/report", { ...body, signature: body.signature.replace(/=+$/, "") }), 400);
});
S(G.sig, "signature 5000 characters (over the body limit)", "413", async (h) => {
  const k = await h.key();
  const body = await h.sign(k, reportFields(h));
  status(await h.post("/report", { ...body, signature: "A".repeat(5000) }), 413);
});
S(G.sig, "signature field missing", "400 missing field: signature", async (h) => {
  const k = await h.key();
  const body = await h.sign(k, reportFields(h));
  delete body.signature;
  const r = await h.post("/report", body);
  status(r, 400);
  errMatches(r, /missing field: signature/);
});
S(G.sig, "public_key field missing", "400 missing field: public_key", async (h) => {
  const k = await h.key();
  const body = await h.sign(k, reportFields(h));
  delete body.public_key;
  const r = await h.post("/report", body);
  status(r, 400);
  errMatches(r, /missing field: public_key/);
});
for (const [name, bytes] of [["31 bytes", 31], ["33 bytes", 33], ["0 bytes", 0], ["64 bytes", 64]]) {
  S(G.sig, `public key of ${name}`, "400 public_key must be 32 bytes", async (h) => {
    const k = await h.key();
    const body = await h.sign(k, reportFields(h));
    const r = await h.post("/report", { ...body, public_key: Buffer.alloc(bytes, 9).toString("base64") });
    status(r, 400);
    errMatches(r, /public_key/);
  });
}
S(G.sig, "all-zero (small-order) public key with forged all-zero signature, 64 message variants", "never 200 (weak key must not verify)", async (h) => {
  // The all-zero key is a small-order point. A non-strict verifier accepts s=0, R=A for
  // about one message in four, so 64 variants find it if it is accepted.
  const zeroKey = Buffer.alloc(32).toString("base64");
  const zeroSig = Buffer.alloc(64).toString("base64");
  const call = h.call();
  for (let i = 0; i < 64; i++) {
    const f = reportFields(h, { reporter_call: call, frequency_khz: LOW + i });
    const r = await h.post("/report", { ...f, public_key: zeroKey, signature: zeroSig });
    check(r.status !== 200, `forged signature with the all-zero key accepted for frequency_khz ${LOW + i}: ${brief(r)}`);
  }
});
S(G.sig, "non-canonical signature (S + group order) is refused", "401 (malleable signature)", async (h) => {
  const k = await h.key();
  const body = await h.sign(k, reportFields(h));
  const raw = Buffer.from(body.signature, "base64");
  const L = 2n ** 252n + 27742317777372353535851937790883648493n;
  let s = 0n;
  for (let i = 31; i >= 0; i--) s = (s << 8n) | BigInt(raw[32 + i]);
  const s2 = s + L;
  check(s2 < 2n ** 256n, "test premise: S + L fits in 32 bytes");
  for (let i = 0; i < 32; i++) raw[32 + i] = Number((s2 >> BigInt(8 * i)) & 0xffn);
  const r = await h.post("/report", { ...body, signature: raw.toString("base64") });
  status(r, [400, 401]);
});

S(G.sig, "replay of the same signed report twice", "200 both times, one record (idempotent)", async (h) => {
  const k = await h.key();
  const f = reportFields(h);
  const body = await h.sign(k, f);
  status(await h.post("/report", body), 200);
  status(await h.post("/report", body), 200);
  eq((await stationsOf(h, f.reporter_call)).length, 1, "records");
});
S(G.sig, "replay of a 30-day-old signed report (heard_at must be within 10 minutes)", "400, nothing stored", async (h) => {
  const old = iso(h.now() - 30 * 24 * H);
  const { res, fields } = await report(h, { heard_at: old });
  status(res, 400);
  eq((await stationsOf(h, fields.reporter_call)).length, 0, "records after a refused old report");
});
S(G.sig, "[hardening] a stale (30-day-old) signed report must not refresh a record's 48 h life", "refused (400): captured reports can be replayed forever otherwise", async (h) => {
  const { res } = await report(h, { heard_at: iso(h.now() - 30 * 24 * H) });
  status(res, 400, "a 30-day-old heard_at");
});
S(G.sig, "[hardening] removal signature replayed after the station reports again", "refused (4xx); a captured removal must not delete a fresh record", async (h) => {
  const k = await h.key();
  const f = reportFields(h);
  status(await h.post("/report", await h.sign(k, f)), 200);
  const rem = await h.sign(k, { heard_call: f.heard_call, reporter_call: f.reporter_call, signed_at: iso(h.now()) });
  status(await h.post("/remove", rem), 200);
  status(await h.post("/report", await h.sign(k, f)), 200);
  const replay = await h.post("/remove", rem);
  check(replay.status >= 400, `replayed removal deleted the fresh record: ${brief(replay)}`);
  eq((await stationsOf(h, f.reporter_call)).length, 1, "records after replay");
});

// timestamps
const TS = [
  ["+5 min exactly (clock skew allowance)", (n) => iso(n + 5 * MIN), 200],
  ["+5 min 1 s", (n) => iso(n + 5 * MIN + 1000), 400],
  ["+5 min 1 ms (with milliseconds)", (n) => new Date(n + 5 * MIN + 1).toISOString(), 400],
  ["+4 min 59 s", (n) => iso(n + 5 * MIN - 1000), 200],
  ["+1 hour", (n) => iso(n + H), 400],
  ["+1 day", (n) => iso(n + 24 * H), 400],
  ["year 9999", () => "9999-12-31T23:59:59Z", 400],
  ["-5 min (client clock behind)", (n) => iso(n - 5 * MIN), 200],
  ["-1 hour", (n) => iso(n - H), 400],
  ["-47 hours", (n) => iso(n - 47 * H), 400],
  ["year 2000 (very old)", () => "2000-01-01T00:00:00Z", 400],
  ["year 1970 epoch", () => "1970-01-01T00:00:00Z", 400],
  ["with milliseconds", (n) => new Date(n - MIN).toISOString(), 200],
  ["four fractional digits", (n) => new Date(n - MIN).toISOString().replace("Z", "1Z"), 400],
  ["no Z suffix", (n) => iso(n - MIN).replace("Z", ""), 400],
  ["+00:00 offset instead of Z", (n) => iso(n - MIN).replace("Z", "+00:00"), 400],
  ["+04:00 offset", (n) => iso(n - MIN).replace("Z", "+04:00"), 400],
  ["date only", () => "2026-11-17", 400],
  ["lowercase t and z", (n) => iso(n - MIN).toLowerCase(), 400],
  ["space instead of T", (n) => iso(n - MIN).replace("T", " "), 400],
  ["impossible date 2026-02-30", () => "2026-02-30T00:00:00Z", 400],
  ["month 13", () => "2026-13-01T00:00:00Z", 400],
  ["hour 24", () => "2026-11-17T24:00:00Z", 400],
  ["leap second 23:59:60", () => "2026-06-30T23:59:60Z", 400],
  ["empty string", () => "", 400],
  ["epoch number", (n) => Math.floor((n - MIN) / 1000), 400],
  ["epoch milliseconds as number", (n) => n - MIN, 400],
  ["null", () => null, 400],
  ["trailing newline", (n) => iso(n - MIN) + "\n", 400],
  ["text", () => "yesterday", 400],
];
for (const [name, mk, want] of TS) {
  S(G.sig, `heard_at ${name}`, String(want), async (h) => {
    const { res } = await report(h, { heard_at: mk(h.now()) });
    status(res, want);
  }, { needs: name.includes("1 ms") || name.includes("exactly") ? ["clock"] : [] });
}

// ---- 2. Callsigns and fields ----------------------------------------------

/** Accepted forms as [transform of a fresh unique callsign, expected stored form]. */
const CALL_ACCEPT = [
  ["plain", (c) => [c, c]], ["lowercase", (c) => [c.toLowerCase(), c]], ["portable /P", (c) => [c + "/P", c + "/P"]], ["lowercase portable /p", (c) => [c.toLowerCase() + "/p", c + "/P"]],
  ["suffix /QRP", (c) => [c + "/QRP", c + "/QRP"]], ["mixed case", (c) => [c[0] + c.slice(1).toLowerCase(), c]],
];
/** Fixed shapes at the edges of the callsign pattern; heard_call only, because reporter callsigns are owned by a key for 48 h. */
const CALL_ACCEPT_FIXED = [["K1", "K1"], ["A1", "A1"], ["AAA1BBBB", "AAA1BBBB"], ["9A1ABC", "9A1ABC"], ["Dl1XyZ", "DL1XYZ"], ["k1abc", "K1ABC"]];
const CALL_REFUSE = [
  ["empty", ""], ["single space", " "], ["no digit", "K"], ["only a digit", "1"], ["only letters", "KAHFAB"], ["four letters before digit", "ABCD1"],
  ["trailing slash", "K1ABC/"], ["leading slash", "/P"], ["suffix too long", "K1ABC/PPPPP"], ["double slash", "K1ABC//P"], ["two suffixes", "K1ABC/P/Q"],
  ["too many characters (9)", "AAA1BBBBB"], ["too long (1000 digits)", "9".repeat(1000)], ["too long (K1 + 200 letters)", "K1" + "A".repeat(200)],
  ["inner space", "K1 ABC"], ["leading space", " K1ABC"], ["trailing space", "K1ABC "], ["trailing newline", "K1ABC\n"], ["CRLF", "K1ABC\r\n"],
  ["tab", "K1ABC\t"], ["NUL inside", "K1\u0000ABC"], ["NUL at end", "K1ABC\u0000"], ["control chars", "K1\u0001\u0002ABC"], ["escape char", "\u001b[31mK1ABC"],
  ["right-to-left override", "‮K1ABC"], ["zero-width space", "K1ABC​"], ["HTML script", "<script>alert(1)</script>"], ["HTML bold", "K1<b>"],
  ["attribute breakout", "K1ABC\"><img src=x onerror=alert(1)>"], ["SQL drop", "'; DROP TABLE stations;--"], ["SQL or", "K1ABC' OR '1'='1"],
  ["shell", "K1ABC;rm -rf /"], ["path traversal", "../../etc/passwd"], ["percent NUL", "K1ABC%00"], ["store key delimiter", "K1ABC>DL1XYZ"],
  ["store key prefix", "rec:K1ABC>X1"], ["umlaut", "K1ÄBC"], ["Cyrillic lookalikes", "К1АВС"], ["dotless i (upper-cases to I)", "K1ABı"],
  ["long s (upper-cases to S)", "K1ABſ"], ["ligature ff (upper-cases to FF)", "ﬀ1ABC"], ["fullwidth digit", "K１ABC"], ["emoji", "K1ABC😀"],
  ["combining accent", "K1ABĆ"], ["__proto__", "__proto__"], ["constructor", "constructor"], ["number", 123], ["null", null], ["true", true],
  ["empty array", []], ["array with callsign", ["K1ABC"]], ["object", {}], ["object with toString", { toString: "K1ABC" }],
];
const callBody = (field, value) => ({ [field]: value });

for (const field of ["reporter_call", "heard_call"]) {
  const accepted = [...CALL_ACCEPT.map(([n, f]) => [n, f]), ...(field === "heard_call" ? CALL_ACCEPT_FIXED.map(([i, w]) => [`fixed ${JSON.stringify(i)}`, () => [i, w]]) : [])];
  for (const [label, mk] of accepted) {
    S(G.call, `${field} ${label} normalised`, "200, stored in upper case", async (h) => {
      const [input, want] = mk(h.call());
      const { res, fields } = await report(h, callBody(field, input));
      status(res, 200);
      const list = await stationsOf(h);
      const mine = list.find((s) => (field === "reporter_call" ? s.heard_call === fields.heard_call : s.reporter_call === fields.reporter_call));
      check(mine, "record not listed");
      eq(mine[field], want, `stored ${field}`);
    });
  }
  for (const [label, input] of CALL_REFUSE) {
    S(G.call, `${field} ${label}`, "400, no echo of the input, nothing stored", async (h) => {
      const { res, fields } = await report(h, callBody(field, input));
      status(res, 400);
      errMatches(res, new RegExp(field.replace("_", "[_ ]")));
      noEcho(res, typeof input === "string" ? input : "");
      const other = field === "reporter_call" ? fields.heard_call : fields.reporter_call;
      const list = await stationsOf(h);
      check(!list.some((s) => s.heard_call === other || s.reporter_call === other), "record stored despite refusal");
    });
  }
}
for (const [label, input] of CALL_REFUSE.filter(([, v]) => typeof v === "string").slice(0, 60).filter((_, i) => i % 3 === 0)) {
  S(G.call, `register callsign ${label}`, "400", async (h) => {
    const k = await h.key();
    const r = await h.post("/register", await h.sign(k, { area: h.area(), callsign: input }));
    status(r, 400); noEcho(r, input);
  });
  S(G.call, `schedule relay ${label}`, "400", async (h) => {
    const k = await h.key();
    const r = await h.post("/schedule", await h.sign(k, { relay: input, requests: [tx(1, 5)] }));
    status(r, 400); noEcho(r, input);
  });
  S(G.call, `remove reporter_call ${label}`, "400", async (h) => {
    const k = await h.key();
    const r = await h.post("/remove", await h.sign(k, { heard_call: h.call(), reporter_call: input, signed_at: iso(h.now()) }));
    status(r, 400); noEcho(r, input);
  });
}

S(G.call, "lowercase callsign cannot bypass ownership held by another key", "409", async (h) => {
  const call = h.call();
  const a = await report(h, { reporter_call: call.toLowerCase() });
  status(a.res, 200);
  const b = await report(h, { reporter_call: call });
  status(b.res, 409);
});
S(G.call, "portable suffix is a separate callsign for ownership", "200 for the other key on K1ABC/P", async (h) => {
  const call = h.call();
  status((await report(h, { reporter_call: call })).res, 200);
  status((await report(h, { reporter_call: call + "/P" })).res, 200);
});

const MODE_ACCEPT = ["KAHF", "FT8", "A", "a+b", "A B", "A  B", "M.1_2/3-4", "K".repeat(24), "-KAHF", "KAHF-", "A+", "+A", "a.b"];
const MODE_REFUSE = [
  ["empty", ""], ["only space", " "], ["leading space", " KAHF"], ["trailing space", "KAHF "], ["25 characters", "K".repeat(25)], ["HTML bold", "<b>"],
  ["HTML script", "<script>alert(1)</script>"], ["trailing newline", "KAHF\n"], ["NUL", "KAHF\u0000"], ["fullwidth", "ＫＡＨＦ"], ["accented", "Ünï"],
  ["ampersand", "A&B"], ["semicolon", "A;B"], ["single quote", "A'B"], ["double quote", 'A"B'], ["percent", "A%20B"], ["SQL", "'; DROP TABLE x;--"],
  ["backslash", "A\\B"], ["emoji", "KAHF😀"], ["RTL override", "‮KAHF"], ["3000 characters", "K".repeat(3000)],
  ["number", 5], ["null", null], ["array", ["KAHF"]], ["object", {}], ["boolean", false],
];
for (const m of MODE_ACCEPT) {
  S(G.call, `radio mode ${JSON.stringify(m)} (README: letters, digits, space, + . _ / -)`, "200, stored unchanged (case kept)", async (h) => {
    const { res, fields } = await report(h, { mode: m });
    status(res, 200);
    const st = (await stationsOf(h)).find((s) => s.reporter_call === fields.reporter_call);
    eq(st.mode, m, "stored mode");
  });
}
for (const [label, m] of MODE_REFUSE) {
  S(G.call, `radio mode ${label}`, "400 mode is not valid, no echo", async (h) => {
    const { res } = await report(h, { mode: m });
    status(res, 400);
    errMatches(res, /mode/);
    noEcho(res, typeof m === "string" ? m : "");
  });
}

for (const [label, key] of [
  ["admin", "admin"], ["Admin", "Admin"], ["empty key", ""], ["__proto__", "__proto__"], ["constructor", "constructor"], ["HTML script key", "<script>alert(1)</script>"],
  ["quote key", 'a"b'], ["unicode key", "é"], ["top-level latitude", "latitude"], ["top-level LONGITUDE", "LONGITUDE"], ["top-level lng", "lng"], ["top-level lat", "lat"],
]) {
  S(G.call, `report with unknown field: ${label}`, "400 field not allowed, no echo of the key", async (h) => {
    const k = await h.key();
    const f = reportFields(h);
    const bodyText = JSON.stringify(await h.sign(k, f)).replace(/^\{/, `{${JSON.stringify(key)}:"x",`);
    const r = await h.post("/report", bodyText);
    status(r, 400);
    errMatches(r, /not allowed|inside a location/);
    if (/[<>"'&\\]|[^\x00-\x7f]/.test(key)) noEcho(r, key);
  });
}
for (const key of ["frequency_khz", "heard_at", "heard_call", "heard_location", "mode", "public_key", "reporter_call", "reporter_location", "signature"]) {
  S(G.call, `report missing ${key}`, `400 missing field: ${key}`, async (h) => {
    const k = await h.key();
    const body = await h.sign(k, reportFields(h));
    delete body[key];
    const r = await h.post("/report", body);
    status(r, 400);
    errMatches(r, new RegExp(`missing field: ${key}`));
  });
}
S(G.call, "duplicate JSON key: last value wins and is the one signed and stored", "200, mode = last value", async (h) => {
  const k = await h.key();
  const f = reportFields(h);
  const text = JSON.stringify(await h.sign(k, f)).replace(/^\{/, '{"mode":"FIRST",');
  const r = await h.post("/report", text);
  status(r, 200);
  eq((await stationsOf(h, f.reporter_call))[0].mode, "KAHF", "stored mode");
});
S(G.call, "duplicate JSON key with a bad last value", "400", async (h) => {
  const k = await h.key();
  const f = reportFields(h);
  const text = JSON.stringify(await h.sign(k, f)).replace(/,"mode":"KAHF"/, "").replace(/\}$/, ',"mode":"<bad>"}');
  status(await h.post("/report", text), 400);
});
S(G.call, "heard_call equal to reporter_call (self report)", "200 or 400, never 5xx", async (h) => {
  const c = h.call();
  const { res } = await report(h, { reporter_call: c, heard_call: c });
  status(res, [200, 400]);
});

// ---- 3. Locations ---------------------------------------------------------

const LOC_ACCEPT = [
  ["north pole", 90, 0], ["south pole", -90, 0], ["north pole, lon 180", 90, 180], ["south pole, lon -180", -90, -180],
  ["antimeridian +180", 0, 180], ["antimeridian -180", 0, -180], ["equator and prime meridian", 0, 0], ["negative zero", -0, -0],
  ["4 decimals", 51.1234, -0.1234], ["smallest step 0.0001", 0.0001, -0.0001], ["lat 89.9999 lon 179.9999", 89.9999, 179.9999],
  ["lat -89.9999 lon -179.9999", -89.9999, -179.9999],
];
const LOC_REFUSE = [
  ["lat 90.0001", 90.0001, 0], ["lat -90.0001", -90.0001, 0], ["lon 180.0001", 0, 180.0001], ["lon -180.0001", 0, -180.0001],
  ["lat 91", 91, 0], ["lat -91", -91, 0], ["lon 181", 0, 181], ["lon 360", 0, 360], ["lat 1e308", 1e308, 0],
  ["5 decimals in latitude", 51.12345, 0], ["6 decimals 50.999999 (just under a degree edge)", 50.999999, 0.5], ["5 decimals in longitude", 0, 0.00005],
  ["8 decimals", 51.12340001, 0], ["lat as numeric string", "51.5", 0], ["lon as numeric string", 0, "0.5"], ["lat null", null, 0], ["lon null", 0, null],
  ["lat true", true, 0], ["lat array", [51], 0], ["lat object", { v: 1 }, 0], ["lat empty string", "", 0], ["lat NaN as string", "NaN", 0],
  ["lat Infinity as string", "Infinity", 0], ["lat -Infinity as string", "-Infinity", 0], ["lat hex string", "0x10", 0],
];
for (const field of ["reporter_location", "heard_location"]) {
  for (const [label, lat, lon] of LOC_ACCEPT) {
    S(G.loc, `${field} precise ${label}`, "200", async (h) => {
      const { res } = await report(h, { [field]: { mode: "precise", latitude: lat, longitude: lon } });
      status(res, 200);
    });
  }
  for (const [label, lat, lon] of LOC_REFUSE) {
    S(G.loc, `${field} precise ${label}`, "400 location error", async (h) => {
      const { res } = await report(h, { [field]: { mode: "precise", latitude: lat, longitude: lon } });
      status(res, 400);
      errMatches(res, new RegExp(field));
    });
  }
  for (const [label, token] of [["NaN (raw)", "NaN"], ["Infinity (raw)", "Infinity"], ["-Infinity (raw)", "-Infinity"], ["1e999 (overflows to Infinity)", "1e999"], ["-1e999", "-1e999"], ["leading zero 051.5", "051.5"], ["plus sign +51.5", "+51.5"], ["bare dot .5", ".5"], ["hex 0x33", "0x33"]]) {
    S(G.loc, `${field} latitude ${label}`, "400 (invalid JSON or out of range), never 5xx", async (h) => {
      const r = await postRawToken(h, "/report", reportFields(h), [field, "latitude"], token);
      status(r, 400);
    });
  }
  for (const [label, value] of [["null", null], ["a string", "51,0"], ["an array", []], ["a number", 5], ["true", true]]) {
    S(G.loc, `${field} is ${label}`, `400 ${field} must be an object`, async (h) => {
      const { res } = await report(h, { [field]: value });
      status(res, 400);
      errMatches(res, new RegExp(`${field} must be an object`));
    });
  }
  for (const [label, loc] of [
    ["mode missing", { latitude: 1, longitude: 1 }], ["mode Precise (capital)", { mode: "Precise", latitude: 1, longitude: 1 }], ["mode exact", { mode: "exact", latitude: 1, longitude: 1 }],
    ["latitude missing", { mode: "precise", longitude: 1 }], ["longitude missing", { mode: "precise", latitude: 1 }], ["extra field altitude", { mode: "precise", latitude: 1, longitude: 1, altitude: 5 }],
    ["extra field accuracy", { mode: "precise", latitude: 1, longitude: 1, accuracy: 5 }], ["precise with radius_km", { mode: "precise", latitude: 1, longitude: 1, radius_km: 5 }],
    ["area without radius_km", { mode: "area", latitude: 1, longitude: 1 }], ["empty object", {}], ["grid square string instead of numbers", { mode: "precise", latitude: "JO01", longitude: "aa" }],
  ]) {
    S(G.loc, `${field} ${label}`, "400", async (h) => {
      const { res } = await report(h, { [field]: loc });
      status(res, 400);
    });
  }
}

// area keys at one-degree edges, seen through /zones
const EDGES = [
  [50.9999, 0.5, "N50E0"], [51.0, 0.5, "N51E0"], [51.0001, 0.5, "N51E0"], [0.9999, 0.9999, "N0E0"], [1.0, 1.0, "N1E1"], [0, 0, "N0E0"], [-0, -0, "N0E0"],
  [-0.0001, -0.0001, "S1W1"], [-0.9999, -0.9999, "S1W1"], [-1.0, -1.0, "S1W1"], [-1.0001, -1.0001, "S2W2"], [90, 180, "N90E180"], [-90, -180, "S90W180"],
  [-89.9999, -179.9999, "S90W180"], [89.9999, 179.9999, "N89E179"], [0, 180, "N0E180"], [0, -180, "N0W180"], [0, 179.9999, "N0E179"], [0, -179.9999, "N0W180"],
  [51.5074, -0.1278, "N51W1"], [25.2048, 55.2708, "N25E55"],
];
for (const [lat, lon, key] of EDGES) {
  S(G.loc, `area key at ${lat},${lon}`, `/zones lists ${key}`, async (h) => {
    const { res } = await report(h, { reporter_location: { mode: "precise", latitude: lat, longitude: lon } });
    status(res, 200);
    const z = await h.get("/zones");
    status(z, 200);
    check(Array.isArray(z.json.zones[key]) && z.json.zones[key].includes(BUSY_CH), `zone ${key} should hold channel ${BUSY_CH}, zones=${snip(z.json.zones)}`);
    if (h.caps.isolated) eq(Object.keys(z.json.zones), [key], "areas in /zones");
  });
}
S(G.loc, "antimeridian neighbours are different areas (E180 and W180)", "N0E180 and N0W180 both present", async (h) => {
  status((await report(h, { reporter_location: { mode: "precise", latitude: 0, longitude: 180 } })).res, 200);
  status((await report(h, { reporter_location: { mode: "precise", latitude: 0, longitude: -180 } })).res, 200);
  const z = (await h.get("/zones")).json.zones;
  check(z.N0E180 && z.N0W180, `zones ${snip(z)}`);
});

// register area strings
for (const [label, area, ok] of [
  ["N51W1", "N51W1", true], ["N0E0", "N0E0", true], ["S1W1", "S1W1", true], ["N90E180 (latitude 90 is not a square)", "N90E180", false], ["S90W180", "S90W180", true], ["N89E179", "N89E179", true],
  ["lowercase", "n51w1", false], ["empty", "", false], ["missing hemisphere", "51W1", false], ["N91 latitude out of range", "N91E0", false], ["S91 latitude out of range", "S91E0", false],
  ["N99E0", "N99E0", false], ["E181 longitude out of range", "N0E181", false], ["W181 longitude out of range", "N0W181", false], ["W999", "N5W999", false],
  ["leading zero N5W01 aliases N5W1", "N5W01", false], ["leading zero N05W1", "N05W1", false], ["S0 never produced by areaKey", "S0E0", false], ["W0 never produced by areaKey", "N0W0", false],
  ["grid square JO01", "JO01", false], ["trailing space", "N51W1 ", false], ["newline", "N51W1\n", false], ["HTML", "<b>N51W1</b>", false], ["SQL", "N51W1';--", false],
  ["a number", 5151, false], ["null", null, false], ["decimal N51.5W1", "N51.5W1", false], ["negative N-1W1", "N-1W1", false],
]) {
  S(G.loc, `relay area ${label}`, ok ? "200" : "400", async (h) => {
    const call = h.call();
    const { res } = await registerRelay(h, call, area);
    status(res, ok ? 200 : 400);
    if (!ok && typeof area === "string" && area.length > 6) noEcho(res, area);
  });
}

// ---- 4. Radius and area ---------------------------------------------------

const RADIUS_ACCEPT = [[1, "minimum"], [2, "2"], [50, "50"], [99, "99"], [100, "maximum"]];
const RADIUS_REFUSE = [
  [0, "zero"], [101, "one over the maximum"], [1000, "1000"], [1.5, "fraction 1.5"], [0.5, "fraction 0.5"], [99.9, "99.9"], [100.00000000000001, "100 plus epsilon"], [0.9999999, "just under 1"],
  [-1, "negative"], [-100, "-100"], ["50", "string"], ["", "empty string"], [null, "null"], [true, "true"], [[50], "array"], [{}, "object"], [2 ** 53, "2^53"], [9007199254740993, "unsafe integer"], [1e21, "1e21"],
];
for (const field of ["reporter_location", "heard_location"]) {
  for (const [r, label] of RADIUS_ACCEPT) {
    S(G.rad, `${field} area radius ${label}`, "200", async (h) => {
      status((await report(h, { [field]: { mode: "area", latitude: 10, longitude: 10, radius_km: r } })).res, 200);
    });
  }
  for (const [r, label] of RADIUS_REFUSE) {
    S(G.rad, `${field} area radius ${label}`, "400 radius_km must be a whole number 1 to 100", async (h) => {
      const { res } = await report(h, { [field]: { mode: "area", latitude: 10, longitude: 10, radius_km: r } });
      status(res, 400);
      errMatches(res, /radius_km/);
    });
  }
  for (const [label, token] of [["NaN (raw)", "NaN"], ["Infinity (raw)", "Infinity"], ["1e999", "1e999"], ["50.0 spelled with a fraction", "50.0"], ["1e2 exponent form", "1e2"], ["5e1", "5e1"], ["-0", "-0"], ["0.1e2 (=1)", "0.1e2"]]) {
    S(G.rad, `${field} radius ${label}`, token === "50.0" || token === "1e2" || token === "5e1" || token === "0.1e2" ? "200 (same number as an integer)" : "400, never 5xx", async (h) => {
      const r = await postRawToken(h, "/report", reportFields(h, { [field]: { mode: "area", latitude: 10, longitude: 10, radius_km: 5 } }), [field, "radius_km"], token);
      if (["50.0", "1e2", "5e1", "0.1e2"].includes(token)) {
        // signed over radius "@@RAW@@" so the signature cannot match; the point is that validation passes (401, not 400)
        status(r, 401);
      } else status(r, 400);
    });
  }
}
for (const [label, lat, lon, r] of [["north pole r100", 90, 0, 100], ["south pole r100", -90, 0, 100], ["lat 89.9999 r100", 89.9999, 0, 100], ["antimeridian +180 r100", 0, 180, 100], ["antimeridian -180 r100", 0, -180, 100], ["equator r1", 0, 0, 1]]) {
  S(G.rad, `map draws an area at ${label} with finite numbers`, "/map 200, no NaN/Infinity/undefined", async (h) => {
    const { res } = await report(h, { heard_location: { mode: "area", latitude: lat, longitude: lon, radius_km: r } });
    status(res, 200);
    const m = await h.get("/map");
    status(m, 200);
    check(!/NaN|Infinity|undefined|\bnull\b/.test(m.text.replace(/<script>[\s\S]*?<\/script>/, "")), "map page contains NaN, Infinity, undefined or null");
  });
}

// ---- 5. Frequencies and channels ------------------------------------------

// Just below, at and above both segment edges, plus a few inside and far away.
const FREQ_ACCEPT = [1, LOW - 1, LOW, LOW + 2, LOW + 3, LOW + 6, HIGH - 2, HIGH - 1, HIGH, HIGH + 1, 145_500, 1e9];
const FREQ_REFUSE = [
  [0, "zero"], [-1, "negative"], [-LOW, `-${LOW}`], [LOW + 0.5, `${LOW}.5`], [freqOf(MID) + 0.001, `${freqOf(MID)}.001`], [1e9 + 1, "one over 1 THz"], [2 ** 53, "2^53"], [9007199254740993, "unsafe integer"],
  [1e21, "1e21"], [1e308, "1e308"], [String(freqOf(MID)), "string"], ["", "empty string"], [null, "null"], [true, "true"], [[freqOf(MID)], "array"], [{}, "object"], [0.1, "0.1"],
];
for (const f of FREQ_ACCEPT) {
  S(G.freq, `frequency ${f} kHz`, `200, channel ${expectedChannel(f) ?? "none"} in /zones`, async (h) => {
    const { res } = await report(h, { frequency_khz: f });
    status(res, 200);
    const ch = expectedChannel(f);
    const z = (await h.get("/zones")).json.zones;
    const all = Object.values(z).flat();
    if (ch === null) {
      if (h.caps.isolated) eq(all, [], "busy channels for an out-of-band frequency");
    } else {
      check(all.includes(ch), `channel ${ch} should be busy, zones=${snip(z)}`);
      if (h.caps.isolated) eq(all, [ch], "busy channels");
    }
  });
}
for (const [f, label] of FREQ_REFUSE) {
  S(G.freq, `frequency ${label}`, "400 frequency_khz must be a positive integer", async (h) => {
    const { res } = await report(h, { frequency_khz: f });
    status(res, 400);
    errMatches(res, /frequency_khz/);
  });
}
for (const [label, token] of [["NaN (raw)", "NaN"], ["Infinity (raw)", "Infinity"], ["1e999", "1e999"], [`${LOW}.0 spelled with a fraction`, `${LOW}.0`]]) {
  S(G.freq, `frequency ${label}`, token === `${LOW}.0` ? "validation passes (401 since signed with a placeholder)" : "400", async (h) => {
    const r = await postRawToken(h, "/report", reportFields(h), ["frequency_khz"], token);
    status(r, token === `${LOW}.0` ? 401 : 400);
  });
}
S(G.freq, `every integer kHz from ${LOW - 5} to ${HIGH + 5} maps to the channel an independent calculation gives`, "/zones busy channels = expected set", async (h) => {
  const k = await h.key();
  const rep = h.call();
  const area = { mode: "precise", latitude: 33.3333, longitude: 66.6666 };
  const want = new Set();
  const calls = [];
  for (let f = LOW - 5; f <= HIGH + 5; f++) {
    const hc = "Q9" + (f - (LOW - 5)).toString(36).toUpperCase().padStart(3, "A");
    calls.push(hc);
    const body = await h.sign(k, reportFields(h, { reporter_call: rep, reporter_location: area, heard_call: hc, frequency_khz: f }));
    status(await h.post("/report", body), 200, `report f=${f}`);
    const c = expectedChannel(f);
    if (c !== null) want.add(c);
  }
  const z = (await h.get("/zones")).json.zones.N33E66 ?? [];
  const got = new Set(z);
  const missing = [...want].filter((c) => !got.has(c)), extra = [...got].filter((c) => !want.has(c));
  check(missing.length === 0 && extra.length === 0, `missing channels ${snip(missing)}, extra channels ${snip(extra)}`);
}, {});
S(G.freq, `channel pools are consistent (${NCH} channels, online and open split cleanly)`, `pools cover 0..${LAST} once; busy channel removed from the area's list`, async (h) => {
  status((await report(h, { frequency_khz: freqOf(BUSY_CH) })).res, 200);
  const z = (await h.get("/zones")).json;
  eq(z.pools.bandChannels, NCH, "bandChannels");
  const all = [...z.pools.online, ...z.pools.open].sort((a, b) => a - b);
  eq(all, Array.from({ length: NCH }, (_, i) => i), "online + open");
  check(z.pools.online.every((c) => !z.pools.open.includes(c)), "online and open overlap");
  const entry = z.pools.areas.N51W1;
  check(entry, "N51W1 missing from pools.areas");
  check(!entry.channels.includes(BUSY_CH) || !z.pools.online.includes(BUSY_CH), "busy channel is still offered");
  eq(z.busy_window_s, 600, "busy window");
});

// ---- 6. Schedules ---------------------------------------------------------

async function relayWith(h, area = h.area()) {
  const call = h.call();
  const { res, key } = await registerRelay(h, call, area);
  status(res, 200, "register");
  return { call, key, area };
}

S(G.sched, "empty request list", "400 requests must be a list of 1 to 20", async (h) => {
  const r = await relayWith(h);
  const res = await schedule(h, r.call, [], r.key);
  status(res, 400);
  errMatches(res, /requests/);
});
S(G.sched, "requests is not a list (object, string, null, number)", "400", async (h) => {
  const r = await relayWith(h);
  for (const v of [{}, "x", null, 5, true]) status(await h.post("/schedule", await h.sign(r.key, { relay: r.call, requests: v })), 400, `requests=${JSON.stringify(v)}`);
});
S(G.sched, "one relay alone gets its slot at the current second", "200, one slot: start = now, channel, duration, direction as asked", async (h) => {
  const r = await relayWith(h);
  const res = await schedule(h, r.call, [{ channel: LAST, duration: 10, direction: "rx" }], r.key);
  status(res, 200);
  eq(res.json.relay, r.call, "relay");
  eq(res.json.slots.length, 1, "slot count");
  const s = res.json.slots[0];
  eq([s.channel, s.duration, s.direction], [LAST, 10, "rx"], "slot fields");
  check(Math.abs(s.start - nowS(h)) <= 5, `start ${s.start} should be about ${nowS(h)}`);
});
S(G.sched, "20 requests accepted, 21 refused", "200 then 400", async (h) => {
  const r = await relayWith(h);
  status(await schedule(h, r.call, Array.from({ length: 20 }, (_, i) => tx(i % NCH, 5)), r.key), 200, "20 requests");
  status(await schedule(h, r.call, Array.from({ length: 21 }, (_, i) => tx(i % NCH, 5)), r.key), 400, "21 requests");
});
S(G.sched, "a relay's own slots never overlap (one radio), across different channels", "start times at least duration apart", async (h) => {
  const r = await relayWith(h);
  // One request per channel of the band (the band has few channels, so no more distinct ones exist).
  const res = await schedule(h, r.call, Array.from({ length: NCH }, (_, c) => tx(c, 60)), r.key);
  status(res, 200);
  const sl = res.json.slots;
  for (let i = 1; i < sl.length; i++) check(sl[i].start >= sl[i - 1].start + sl[i - 1].duration, `slot ${i} overlaps slot ${i - 1}: ${snip(sl)}`);
});
S(G.sched, "30 relays in one area, same channel, each asks 20 times in turn: no two slots overlap on that channel", "all slots disjoint", async (h) => {
  const area = h.area();
  const all = [];
  for (let i = 0; i < 30; i++) {
    const r = await relayWith(h, area);
    const res = await schedule(h, r.call, [tx(MID, 10), tx(MID, 10, { urgent: true })], r.key);
    status(res, 200, `relay ${i}`);
    for (const s of res.json.slots) all.push({ ...s, relay: r.call });
  }
  eq(all.length, 60, "total slots");
  all.sort((a, b) => a.start - b.start);
  for (let i = 1; i < all.length; i++) check(all[i].start >= all[i - 1].start + all[i - 1].duration, `overlap on channel ${MID}: ${snip(all[i - 1])} and ${snip(all[i])}`);
  check(all[0].start >= nowS(h) - 5, "first slot starts before now");
});
S(G.sched, "relays in different areas never transmit on one channel at the same time (HF is global)", "second transmit slot starts after the first", async (h) => {
  const a = await relayWith(h, "N10E10"), b = await relayWith(h, "N11E10");
  const ra = await schedule(h, a.call, [tx(MID, 30)], a.key);
  const rb = await schedule(h, b.call, [tx(MID, 30)], b.key);
  status(ra, 200); status(rb, 200);
  const x = ra.json.slots[0], y = rb.json.slots[0];
  check(y.start >= x.start + x.duration || x.start >= y.start + y.duration, `transmit slots overlap: ${snip(ra.json.slots)} ${snip(rb.json.slots)}`);
});
S(G.sched, "urgent request placed before earlier non-urgent ones on the same channel", "urgent slot (20 s) starts first, then non-urgent in request order", async (h) => {
  const r = await relayWith(h);
  const res = await schedule(h, r.call, [tx(1, 11), tx(1, 12), tx(1, 20, { urgent: true }), tx(1, 13)], r.key);
  status(res, 200);
  eq(res.json.slots.map((s) => s.duration), [20, 11, 12, 13], "durations in start order");
});
S(G.sched, "urgent false behaves as non-urgent; urgent mixed with a different channel", "200, order by request", async (h) => {
  const r = await relayWith(h);
  const res = await schedule(h, r.call, [tx(1, 11, { urgent: false }), tx(2, 12, { urgent: true })], r.key);
  status(res, 200);
  eq(res.json.slots.map((s) => s.duration), [12, 11], "durations in start order");
});
S(G.sched, "a later relay cannot jump ahead of already booked slots, even when urgent", "second relay starts after the first one's slot", async (h) => {
  const area = h.area();
  const a = await relayWith(h, area), b = await relayWith(h, area);
  const ra = await schedule(h, a.call, [tx(MID, 300)], a.key);
  const rb = await schedule(h, b.call, [tx(MID, 30, { urgent: true })], b.key);
  status(ra, 200); status(rb, 200);
  check(rb.json.slots[0].start >= ra.json.slots[0].start + 300, `urgent second relay overlaps the first: ${snip(ra.json.slots)} ${snip(rb.json.slots)}`);
});
S(G.sched, "scheduling again replaces the relay's own earlier plan", "second plan starts now again, not behind the first", async (h) => {
  const r = await relayWith(h);
  status(await schedule(h, r.call, [tx(2, 300)], r.key), 200);
  const second = await schedule(h, r.call, [tx(2, 300)], r.key);
  status(second, 200);
  check(Math.abs(second.json.slots[0].start - nowS(h)) <= 5, `second plan start ${second.json.slots[0].start} should be now`);
});
S(G.sched, "duplicate identical requests are placed one after the other", "slots spaced by duration, none overlapping", async (h) => {
  const r = await relayWith(h);
  const res = await schedule(h, r.call, [tx(MID, 50), tx(MID, 50), tx(MID, 50)], r.key);
  status(res, 200);
  eq(res.json.slots.length, 3, "slots");
  const s = res.json.slots;
  eq([s[1].start - s[0].start, s[2].start - s[1].start], [50, 50], "spacing");
});
S(G.sched, "20 duplicate 300 s requests", "20 disjoint slots", async (h) => {
  const r = await relayWith(h);
  const res = await schedule(h, r.call, Array.from({ length: 20 }, () => tx(MID, 300)), r.key);
  status(res, 200);
  const s = res.json.slots;
  for (let i = 1; i < s.length; i++) check(s[i].start >= s[i - 1].start + 300, "overlap");
});
for (const [label, d, ok] of [["1 s", 1, true], ["300 s", 300, true], ["150 s", 150, true], ["0 s", 0, false], ["301 s", 301, false], ["1.5 s", 1.5, false], ["-1 s", -1, false], ["-300 s", -300, false], ["string \"60\"", "60", false], ["null", null, false], ["true", true, false], ["array", [60], false], ["2^53", 2 ** 53, false], ["1e21", 1e21, false]]) {
  S(G.sched, `duration ${label}`, ok ? "200" : "400", async (h) => {
    const r = await relayWith(h);
    const res = await schedule(h, r.call, [{ channel: MID, duration: d, direction: "tx" }], r.key);
    status(res, ok ? 200 : 400);
    if (!ok) errMatches(res, /duration|requests/);
  });
}
for (const [label, c, ok] of [["0 (band edge)", 0, true], [`${LAST} (last channel)`, LAST, true], [`${MID} (middle)`, MID, true], [`${NCH} (one past the band)`, NCH, false], [`${NCH + 1}`, NCH + 1, false], ["1000", 1000, false], ["-1", -1, false], ["1.5", 1.5, false], [`${LAST - 0.0001}`, LAST - 0.0001, false], ["string \"5\"", "5", false], ["null", null, false], ["2^53", 2 ** 53, false], ["1e21", 1e21, false], ["array", [1], false]]) {
  S(G.sched, `channel ${label}`, ok ? "200" : `400 channel must be 0..${LAST}`, async (h) => {
    const r = await relayWith(h);
    const res = await schedule(h, r.call, [{ channel: c, duration: 5, direction: "rx" }], r.key);
    status(res, ok ? 200 : 400);
    if (!ok) errMatches(res, /channel/);
  });
}
for (const [label, d, ok] of [["tx", "tx", true], ["rx", "rx", true], ["TX", "TX", false], ["both", "both", false], ["empty", "", false], ["null", null, false], ["5", 5, false], ["array", ["tx"], false]]) {
  S(G.sched, `direction ${label}`, ok ? "200" : "400", async (h) => {
    const r = await relayWith(h);
    status(await schedule(h, r.call, [{ channel: MID, duration: 5, direction: d }], r.key), ok ? 200 : 400);
  });
}
for (const [label, u, ok] of [["true", true, true], ["false", false, true], ["\"true\"", "true", false], ["1", 1, false], ["null", null, false], ["array", [], false]]) {
  S(G.sched, `urgent ${label}`, ok ? "200" : "400", async (h) => {
    const r = await relayWith(h);
    status(await schedule(h, r.call, [{ channel: MID, duration: 5, direction: "tx", urgent: u }], r.key), ok ? 200 : 400);
  });
}
for (const [label, q] of [["null request", null], ["string request", "x"], ["number request", 5], ["array request", []], ["missing channel", { duration: 5, direction: "tx" }], ["missing duration", { channel: 1, direction: "tx" }], ["missing direction", { channel: 1, duration: 5 }], ["extra field", { channel: 1, duration: 5, direction: "tx", note: "x" }], ["extra field HTML key", { channel: 1, duration: 5, direction: "tx", "<script>alert(1)</script>": 1 }]]) {
  S(G.sched, `request shape: ${label}`, "400", async (h) => {
    const r = await relayWith(h);
    const res = await schedule(h, r.call, [q], r.key);
    status(res, 400);
    if (label.includes("HTML")) noEcho(res, "<script>alert(1)</script>");
  });
}
S(G.sched, "request with 600 levels of nested arrays", "400, never 5xx", async (h) => {
  const r = await relayWith(h);
  const nested = JSON.parse("[".repeat(600) + "]".repeat(600));
  const res = await h.post("/schedule", await h.sign(r.key, { relay: r.call, requests: [nested] }));
  status(res, 400);
});
S(G.sched, "request for a channel outside the band together with valid ones", "400, and no slots stored for the valid ones", async (h) => {
  const area = h.area();
  const a = await relayWith(h, area), b = await relayWith(h, area);
  status(await schedule(h, a.call, [tx(MID, 50), tx(500, 5)], a.key), 400);
  const rb = await schedule(h, b.call, [tx(MID, 50)], b.key);
  status(rb, 200);
  check(Math.abs(rb.json.slots[0].start - nowS(h)) <= 5, "refused plan still booked channel ${MID}");
});
S(G.sched, "unregistered relay", "404 relay is not registered", async (h) => {
  const k = await h.key();
  const res = await schedule(h, h.call(), [tx(1, 5)], k);
  status(res, 404);
});
S(G.sched, "relay registered by another key", "403", async (h) => {
  const r = await relayWith(h);
  const other = await h.key();
  status(await schedule(h, r.call, [tx(1, 5)], other), 403);
});
S(G.sched, "schedule with a bad signature / tampered requests", "401", async (h) => {
  const r = await relayWith(h);
  const body = await h.sign(r.key, { relay: r.call, requests: [tx(1, 5)] });
  status(await h.post("/schedule", { ...body, requests: [tx(1, 6)] }), 401);
  status(await h.post("/schedule", { ...body, signature: Buffer.alloc(64).toString("base64") }), 401);
});
S(G.sched, "lowercase relay callsign is normalised", "200", async (h) => {
  const r = await relayWith(h);
  const res = await schedule(h, r.call.toLowerCase(), [tx(1, 5)], r.key);
  status(res, 200);
  eq(res.json.relay, r.call, "relay in response");
});
S(G.sched, "register: same key again refreshes, other key refused, bad signature refused", "200, 409, 401", async (h) => {
  const r = await relayWith(h);
  status((await registerRelay(h, r.call, r.area, r.key)).res, 200);
  status((await registerRelay(h, r.call, r.area)).res, 409);
  const body = await h.sign(r.key, { area: r.area, callsign: r.call });
  status(await h.post("/register", { ...body, area: "N1E1" }), 401);
});
S(G.sched, "relay that moves area is planned against its new area only", "same-channel relay in the old area no longer blocks it", async (h) => {
  const x = h.area();
  const y = x === "N5E5" ? "N6E6" : "N5E5";
  const a = await relayWith(h, x);
  const m = await relayWith(h, x);
  status(await schedule(h, a.call, [{ channel: MID, duration: 300, direction: "rx" }], a.key), 200);
  status((await registerRelay(h, m.call, y, m.key)).res, 200);
  const res = await schedule(h, m.call, [{ channel: MID, duration: 30, direction: "rx" }], m.key);
  status(res, 200);
  check(Math.abs(res.json.slots[0].start - nowS(h)) <= 5, `moved relay still waits behind the old area: ${snip(res.json.slots)}`);
});
S(G.sched, "booked slots free up when they have ended", "second relay starts now after the first plan is over", async (h) => {
  const area = h.area();
  const a = await relayWith(h, area), b = await relayWith(h, area);
  status(await schedule(h, a.call, [tx(MID, 300)], a.key), 200);
  h.advance(301_000);
  const rb = await schedule(h, b.call, [tx(MID, 30)], b.key);
  status(rb, 200);
  check(Math.abs(rb.json.slots[0].start - nowS(h)) <= 1, `slot ${snip(rb.json.slots)} should start now`);
}, { needs: ["clock"] });

// ---- 7. Removal -----------------------------------------------------------

S(G.rem, "valid signed removal", "200 removed:true; gone from /stations", async (h) => {
  const k = await h.key();
  const f = reportFields(h);
  status(await h.post("/report", await h.sign(k, f)), 200);
  const r = await h.post("/remove", await h.sign(k, { heard_call: f.heard_call, reporter_call: f.reporter_call, signed_at: iso(h.now()) }));
  status(r, 200);
  eq(r.json, { ok: true, removed: true }, "body");
  eq((await stationsOf(h, f.reporter_call)).length, 0, "records after removal");
});
S(G.rem, "removal by a different key", "403, record stays", async (h) => {
  const f = reportFields(h);
  status(await h.post("/report", await h.sign(await h.key(), f)), 200);
  const r = await h.post("/remove", await h.sign(await h.key(), { heard_call: f.heard_call, reporter_call: f.reporter_call, signed_at: iso(h.now()) }));
  status(r, 403);
  eq((await stationsOf(h, f.reporter_call)).length, 1, "records after refused removal");
});
S(G.rem, "removal of a station that does not exist", "404 no such record", async (h) => {
  const r = await h.post("/remove", await h.sign(await h.key(), { heard_call: h.call(), reporter_call: h.call(), signed_at: iso(h.now()) }));
  status(r, 404);
});
S(G.rem, "second removal of the same record", "200 then 404", async (h) => {
  const k = await h.key();
  const f = reportFields(h);
  status(await h.post("/report", await h.sign(k, f)), 200);
  const rem = await h.sign(k, { heard_call: f.heard_call, reporter_call: f.reporter_call, signed_at: iso(h.now()) });
  status(await h.post("/remove", rem), 200);
  status(await h.post("/remove", rem), 404);
});
S(G.rem, "removal with a bad signature (tampered callsign)", "401, record stays", async (h) => {
  const k = await h.key();
  const f = reportFields(h);
  status(await h.post("/report", await h.sign(k, f)), 200);
  const rem = await h.sign(k, { heard_call: f.heard_call, reporter_call: f.reporter_call, signed_at: iso(h.now()) });
  status(await h.post("/remove", { ...rem, heard_call: h.call() }), 401);
  eq((await stationsOf(h, f.reporter_call)).length, 1, "records");
});
S(G.rem, "removal with swapped reporter and heard calls", "404 or 403, record stays", async (h) => {
  const k = await h.key();
  const f = reportFields(h);
  status(await h.post("/report", await h.sign(k, f)), 200);
  const r = await h.post("/remove", await h.sign(k, { heard_call: f.reporter_call, reporter_call: f.heard_call, signed_at: iso(h.now()) }));
  status(r, [403, 404]);
  eq((await stationsOf(h, f.reporter_call)).length, 1, "records");
});
S(G.rem, "removal removes only the named pair", "other pair of the same reporter stays", async (h) => {
  const k = await h.key();
  const rep = h.call();
  const f1 = reportFields(h, { reporter_call: rep }), f2 = reportFields(h, { reporter_call: rep });
  status(await h.post("/report", await h.sign(k, f1)), 200);
  status(await h.post("/report", await h.sign(k, f2)), 200);
  status(await h.post("/remove", await h.sign(k, { heard_call: f1.heard_call, reporter_call: rep, signed_at: iso(h.now()) })), 200);
  const left = await stationsOf(h, rep);
  eq(left.map((s) => s.heard_call), [f2.heard_call], "remaining pairs");
});
S(G.rem, "lowercase callsigns in a removal are normalised", "200", async (h) => {
  const k = await h.key();
  const f = reportFields(h);
  status(await h.post("/report", await h.sign(k, f)), 200);
  status(await h.post("/remove", await h.sign(k, { heard_call: f.heard_call.toLowerCase(), reporter_call: f.reporter_call.toLowerCase(), signed_at: iso(h.now()) })), 200);
});
S(G.rem, "after a removal the callsign still belongs to the first key", "other key gets 409 (owner kept for 48 h)", async (h) => {
  const k = await h.key();
  const f = reportFields(h);
  status(await h.post("/report", await h.sign(k, f)), 200);
  status(await h.post("/remove", await h.sign(k, { heard_call: f.heard_call, reporter_call: f.reporter_call, signed_at: iso(h.now()) })), 200);
  const r = await report(h, { reporter_call: f.reporter_call });
  status(r.res, 409);
});
for (const [label, mut] of [["extra field", (b) => ({ ...b, extra: 1 })], ["missing heard_call", (b) => { const c = { ...b }; delete c.heard_call; return c; }], ["missing signature", (b) => { const c = { ...b }; delete c.signature; return c; }], ["empty body object", () => ({})], ["array body", () => []], ["a report body sent to /remove", null]]) {
  S(G.rem, `removal shape: ${label}`, "400", async (h) => {
    const k = await h.key();
    const f = reportFields(h);
    let body = await h.sign(k, { heard_call: f.heard_call, reporter_call: f.reporter_call, signed_at: iso(h.now()) });
    body = mut ? mut(body) : await h.sign(k, f);
    status(await h.post("/remove", body), 400);
  });
}

// ---- 8. Expiry ------------------------------------------------------------

const AGES = [["47 h 59 min", 47 * H + 59 * MIN, true], ["48 h", 48 * H, false], ["48 h 1 min", 48 * H + MIN, false]];
for (const [label, age, live] of AGES) {
  S(G.exp, `record ${label} old: /stations`, live ? "still listed" : "gone", async (h) => {
    const { res, fields } = await report(h);
    status(res, 200);
    h.advance(age);
    eq((await stationsOf(h, fields.reporter_call)).length, live ? 1 : 0, "records listed");
  }, { needs: ["clock"] });
  S(G.exp, `record ${label} old: /map-data`, live ? "feed shows the callsign" : "feed does not show it", async (h) => {
    const { res, fields } = await report(h);
    status(res, 200);
    h.advance(age);
    const m = await h.get("/map-data");
    status(m, 200);
    eq(m.text.includes(`"${fields.reporter_call}"`), live, "callsign in the live feed");
  }, { needs: ["clock"] });
  S(G.exp, `record ${label} old: /zones`, live ? "busy channel still reported" : "not reported although heard_at is fresh", async (h) => {
    // heard_at is one minute before the check, so only the 48 h expiry decides.
    const k = await h.key();
    const f = reportFields(h);
    await h.seedRecord({ ...f, heard_at: iso(h.now() + age - MIN), public_key: k.publicKey, refreshed_at: h.now() });
    h.advance(age);
    const z = (await h.get("/zones")).json.zones;
    eq(Object.values(z).flat().includes(BUSY_CH), live, `channel ${BUSY_CH} busy`);
  }, { needs: ["seed"] });
  S(G.exp, `record ${label} old: /remove`, live ? "200 removed" : "404 no such record", async (h) => {
    const k = await h.key();
    const f = reportFields(h);
    status(await h.post("/report", await h.sign(k, f)), 200);
    h.advance(age);
    status(await h.post("/remove", await h.sign(k, { heard_call: f.heard_call, reporter_call: f.reporter_call, signed_at: iso(h.now()) })), live ? 200 : 404);
  }, { needs: ["clock"] });
  S(G.exp, `reporter ${label} silent: another key claims the callsign`, live ? "409" : "200 (owner expired)", async (h) => {
    const call = h.call();
    status((await report(h, { reporter_call: call })).res, 200);
    h.advance(age);
    status((await report(h, { reporter_call: call, heard_at: iso(h.now() - MIN) })).res, live ? 409 : 200);
  }, { needs: ["clock"] });
  S(G.exp, `relay ${label} after registering: /schedule`, live ? "200" : "404 relay is not registered", async (h) => {
    const r = await relayWith(h);
    h.advance(age);
    status(await schedule(h, r.call, [tx(1, 5)], r.key), live ? 200 : 404);
  }, { needs: ["clock"] });
  S(G.exp, `relay ${label} after registering: another key registers the callsign`, live ? "409" : "200", async (h) => {
    const r = await relayWith(h);
    h.advance(age);
    status((await registerRelay(h, r.call, r.area)).res, live ? 409 : 200);
  }, { needs: ["clock"] });
}
S(G.exp, "re-reporting resets the 48 h timer", "listed at 40 h + 40 h after the first report", async (h) => {
  const k = await h.key();
  const f = reportFields(h);
  status(await h.post("/report", await h.sign(k, f)), 200);
  h.advance(40 * H);
  status(await h.post("/report", await h.sign(k, { ...f, heard_at: iso(h.now() - MIN) })), 200);
  h.advance(40 * H);
  eq((await stationsOf(h, f.reporter_call)).length, 1, "records at 80 h");
}, { needs: ["clock"] });
S(G.exp, "/stations declares the 48 h life and a generated_at that follows the clock", "expires_after_hours 48", async (h) => {
  const r = await h.get("/stations");
  eq(r.json.expires_after_hours, 48, "expires_after_hours");
  check(!Number.isNaN(Date.parse(r.json.generated_at)), "generated_at not a date");
});
S(G.exp, "expired owner does not block a different key, and the record is replaced", "200 and one record", async (h) => {
  const call = h.call(), heard = h.call();
  status((await report(h, { reporter_call: call, heard_call: heard })).res, 200);
  h.advance(48 * H);
  status((await report(h, { reporter_call: call, heard_call: heard, frequency_khz: LOW + 1, heard_at: iso(h.now() - MIN) })).res, 200);
  const st = await stationsOf(h, call);
  eq(st.map((s) => s.frequency_khz), [LOW + 1], "records");
}, { needs: ["clock"] });

// ---- 9. Concurrency -------------------------------------------------------

S(G.conc, "50 reports for one station in the same second", "all 200; exactly one record; its fields come from one single report", async (h) => {
  const k = await h.key();
  const rep = h.call(), heard = h.call();
  const at = iso(h.now() - MIN);
  const bodies = [];
  for (let i = 0; i < 50; i++) bodies.push(await h.sign(k, reportFields(h, { reporter_call: rep, heard_call: heard, heard_at: at, frequency_khz: LOW + i, mode: `M${LOW + i}` })));
  const results = await Promise.all(bodies.map((b) => h.post("/report", b)));
  for (const r of results) status(r, 200);
  const st = await stationsOf(h, rep);
  eq(st.length, 1, "records for the pair");
  eq(st[0].mode, `M${st[0].frequency_khz}`, "mode and frequency come from the same report");
  check(st[0].frequency_khz >= LOW && st[0].frequency_khz < LOW + 50, "frequency not from the sent set");
});
S(G.conc, "20 reports in a row for one station: the last wins", "frequency of the 20th", async (h) => {
  const k = await h.key();
  const rep = h.call(), heard = h.call();
  for (let i = 0; i < 20; i++) status(await h.post("/report", await h.sign(k, reportFields(h, { reporter_call: rep, heard_call: heard, frequency_khz: LOW + i }))), 200);
  eq((await stationsOf(h, rep)).map((s) => s.frequency_khz), [LOW + 19], "frequency");
});
S(G.conc, "20 identical reports at once", "all 200, one record", async (h) => {
  const k = await h.key();
  const f = reportFields(h);
  const body = await h.sign(k, f);
  const rs = await Promise.all(Array.from({ length: 20 }, () => h.post("/report", body)));
  for (const r of rs) status(r, 200);
  eq((await stationsOf(h, f.reporter_call)).length, 1, "records");
});
S(G.conc, "100 different stations at once", "all 200; all listed, sorted by reporter then heard call", async (h) => {
  const calls = Array.from({ length: 100 }, () => h.call());
  const rs = await Promise.all(calls.map(async (c) => (await report(h, { reporter_call: c })).res));
  for (const r of rs) status(r, 200);
  const list = (await stationsOf(h)).map((s) => s.reporter_call);
  for (const c of calls) check(list.includes(c), `${c} missing from /stations`);
  const mine = list.filter((c) => calls.includes(c));
  eq(mine, [...mine].sort((a, b) => a.localeCompare(b)), "order");
  if (h.caps.isolated) eq(list.length, 100, "count");
});
S(G.conc, "two different keys claim one reporter callsign at once (store with KV-like latency)", "at most one 200; the other gets 409", async (h) => {
  const call = h.call();
  const keys = [await h.key(), await h.key()];
  const rs = await Promise.all(keys.map(async (k) => h.post("/report", await h.sign(k, reportFields(h, { reporter_call: call })))));
  const ok = rs.filter((r) => r.status === 200).length;
  check(ok <= 1, `${ok} keys were both told 200 for the same callsign: ${rs.map(brief).join(" | ")}`);
}, { slow: true });
S(G.conc, "ten keys claim one reporter callsign at once (store with KV-like latency)", "exactly one 200, nine 409", async (h) => {
  const call = h.call();
  const keys = await Promise.all(Array.from({ length: 10 }, () => h.key()));
  const rs = await Promise.all(keys.map(async (k) => h.post("/report", await h.sign(k, reportFields(h, { reporter_call: call })))));
  const ok = rs.filter((r) => r.status === 200).length;
  check(ok === 1, `${ok} keys got 200: ${rs.map((r) => r.status).join(",")}`);
}, { slow: true });
S(G.conc, "two keys register one relay callsign at once (store with KV-like latency)", "at most one 200", async (h) => {
  const call = h.call(), area = h.area();
  const rs = await Promise.all([await h.key(), await h.key()].map(async (k) => h.post("/register", await h.sign(k, { area, callsign: call }))));
  check(rs.filter((r) => r.status === 200).length <= 1, `both registrations accepted: ${rs.map(brief).join(" | ")}`);
}, { slow: true });
S(G.conc, "10 relays in one area schedule the same channel at once (store with KV-like latency)", "no two slots overlap on the channel", async (h) => {
  const area = h.area();
  const relays = [];
  for (let i = 0; i < 10; i++) relays.push(await relayWith(h, area));
  const rs = await Promise.all(relays.map((r) => schedule(h, r.call, [tx(MID, 30)], r.key)));
  for (const r of rs) status(r, 200);
  const slots = rs.map((r) => r.json.slots[0]).sort((a, b) => a.start - b.start);
  for (let i = 1; i < slots.length; i++) check(slots[i].start >= slots[i - 1].start + slots[i - 1].duration, `overlap: ${snip(slots[i - 1])} vs ${snip(slots[i])}`);
}, { slow: true });
S(G.conc, "report and removal of the same record at once", "no 5xx; both answers are 200 or 404; final state is present or absent, not corrupt", async (h) => {
  const k = await h.key();
  const f = reportFields(h);
  status(await h.post("/report", await h.sign(k, f)), 200);
  const [a, b] = await Promise.all([
    h.post("/report", await h.sign(k, f)),
    h.post("/remove", await h.sign(k, { heard_call: f.heard_call, reporter_call: f.reporter_call, signed_at: iso(h.now()) })),
  ]);
  status(a, 200); status(b, [200, 404]);
  check((await stationsOf(h, f.reporter_call)).length <= 1, "more than one record");
}, { slow: true });
S(G.conc, "200 concurrent GETs while reports arrive (10 over HTTP)", "all 200 JSON", async (h) => {
  const writes = Array.from({ length: 20 }, () => report(h));
  const reads = Array.from({ length: h.kind === "remote" ? 10 : 200 }, (_, i) => h.get(i % 2 ? "/stations" : "/zones"));
  const all = await Promise.all([...writes.map((p) => p.then((x) => x.res)), ...reads]);
  for (const r of all) status(r, 200);
});

// ---- 10. Size limits ------------------------------------------------------

/** A valid signed report body padded with trailing spaces to exactly `bytes` bytes. */
async function paddedReport(h, bytes) {
  const k = await h.key();
  const text = JSON.stringify(await h.sign(k, reportFields(h)));
  const have = enc.encode(text).length;
  check(have <= bytes, `test premise: report (${have} bytes) fits in ${bytes}`);
  return { text: text + " ".repeat(bytes - have), reporter: JSON.parse(text).reporter_call };
}
S(G.size, "body of exactly 4096 bytes", "200", async (h) => {
  const { text } = await paddedReport(h, 4096);
  eq(enc.encode(text).length, 4096, "body size");
  status(await h.post("/report", text), 200);
});
S(G.size, "body of 4097 bytes", "413 body too large", async (h) => {
  const { text } = await paddedReport(h, 4097);
  const r = await h.post("/report", text);
  status(r, 413);
  errMatches(r, /too large/);
});
S(G.size, "body of 4095 bytes", "200", async (h) => { status(await h.post("/report", (await paddedReport(h, 4095)).text), 200); });
S(G.size, "4097 bytes made of 2-byte characters (2100 chars)", "413 (limit counts bytes, not characters)", async (h) => {
  const text = '{"x":"' + "é".repeat(2100) + '"}';
  eq(text.length < 4097 && enc.encode(text).length > 4096, true, "test premise");
  status(await h.post("/report", text), 413);
});
S(G.size, "exactly 4096 bytes of multi-byte JSON is read, then refused as invalid input", "400 (not 413)", async (h) => {
  const base = '{"x":"';
  const tail = '"}';
  const fill = 4096 - enc.encode(base + tail).length;
  const text = base + "é".repeat(Math.floor(fill / 2)) + "a".repeat(fill % 2) + tail;
  eq(enc.encode(text).length, 4096, "premise");
  status(await h.post("/report", text), 400);
});
for (const path of ["/report", "/remove", "/register", "/schedule"]) {
  S(G.size, `${path} with a 1 MB body`, "413", async (h) => {
    status(await h.post(path, '{"a":"' + "A".repeat(1_000_000) + '"}'), 413);
  });
  S(G.size, `${path} with a 5000-byte body`, "413", async (h) => { status(await h.post(path, " ".repeat(5000) + "{}"), 413); });
}
S(G.size, "very long JSON: 100000-element array", "413", async (h) => { status(await h.post("/report", "[" + "1,".repeat(100_000) + "1]"), 413); });
S(G.size, "very long JSON: 350 tiny keys inside the limit", "400 field not allowed", async (h) => {
  const o = {}; for (let i = 0; i < 350; i++) o["k" + i] = 1;
  const text = JSON.stringify(o);
  check(text.length <= 4096, "premise");
  status(await h.post("/report", text), 400);
});
S(G.size, "declared Content-Length far above the limit with a small body", "413 without reading the body", async (h) => {
  const r = await h.send({ method: "POST", path: "/report", headers: { "content-type": "application/json", "content-length": "999999" }, body: "{}" });
  status(r, 413);
}, { needs: ["inproc"] });
S(G.size, "declared Content-Length of 0 with a large body", "413 (actual size counts)", async (h) => {
  const r = await h.send({ method: "POST", path: "/report", headers: { "content-type": "application/json", "content-length": "0" }, body: " ".repeat(5000) + "{}" });
  status(r, 413);
}, { needs: ["inproc"] });
S(G.size, "Content-Length that is not a number", "400 or 413, never 5xx", async (h) => {
  const r = await h.send({ method: "POST", path: "/report", headers: { "content-type": "application/json", "content-length": "abc" }, body: "{}" });
  status(r, [400, 413]);
}, { needs: ["inproc"] });
S(G.size, "GET with a 10 kB query string", "200 or 414, JSON/HTML, not echoed", async (h) => {
  const r = await h.get("/stations?q=" + "A".repeat(10_000));
  status(r, [200, 414]);
  check(!r.text.includes("AAAAAAAAAA"), "query echoed");
});

// ---- 11. HTTP -------------------------------------------------------------

const ALLOW = { "/report": "POST", "/remove": "POST", "/register": "POST", "/schedule": "POST", "/stations": "GET", "/zones": "GET", "/": "GET, HEAD", "/map": "GET, HEAD" };
for (const [path, allow] of Object.entries(ALLOW)) {
  const wrong = allow.includes("POST") ? ["GET", "PUT", "DELETE", "PATCH", "OPTIONS", "HEAD"] : ["POST", "PUT", "DELETE", "PATCH", "OPTIONS"];
  for (const method of wrong) {
    if (method === "HEAD" && allow.includes("HEAD")) continue;
    S(G.http, `${method} ${path}`, `405 JSON error, Allow: ${allow}`, async (h) => {
      const r = await h.send({ method, path, headers: { "content-type": "application/json" }, body: method === "GET" || method === "HEAD" ? undefined : "{}" });
      status(r, 405);
      if (method !== "HEAD") { errMatches(r, /method/); }
      eq(r.headers.get("allow"), allow, "Allow header");
    });
  }
  S(G.http, `PURGE ${path} (made-up method)`, "405", async (h) => { status(await h.send({ method: "PURGE", path }), 405); });
}
const BAD_PATHS = ["/nope", "/admin", "/.env", "/.git/config", "/../etc/passwd", "/%2e%2e/etc/passwd", "/map/", "/Map", "/MAP", "/stations.json", "/stations/", "/report/", "//report", "/zones/x",
  "/__proto__", "/constructor", "/toString", "/hasOwnProperty", "/valueOf", "/robots.txt", "/favicon.ico", "/%00", "/é", "/%F0%9F%98%80", "/<script>alert(1)</script>", "/" + "a".repeat(5000), "/api/stations", "/index.html", "/register/", "/%2fmap"];
for (const p of BAD_PATHS) {
  S(G.http, `GET ${p.length > 60 ? p.slice(0, 40) + "..." : p}`, "404 JSON not found, path not echoed", async (h) => {
    const r = await h.get(p);
    status(r, 404);
    errMatches(r, /not found/);
    if (p.length > 3) noEcho(r, p.replace(/^\//, ""));
  });
}
S(G.http, "POST to an unknown path", "404", async (h) => { status(await h.post("/nope", "{}"), 404); });
S(G.http, "query string and fragment do not change routing", "/stations?x=1 is 200", async (h) => { status(await h.get("/stations?x=1&y=<script>"), 200); });
S(G.http, "POST /report without any Content-Type", "200 or 415, JSON", async (h) => {
  const k = await h.key();
  const body = enc.encode(JSON.stringify(await h.sign(k, reportFields(h))));
  const r = await h.send({ method: "POST", path: "/report", headers: {}, body });
  status(r, [200, 415]);
  check(r.json, "not JSON");
});
for (const ct of ["text/plain", "application/x-www-form-urlencoded", "multipart/form-data", "text/html", "application/xml", "application/json; charset=utf-16"]) {
  S(G.http, `POST /report with Content-Type ${ct}`, "200 or 415, JSON", async (h) => {
    const k = await h.key();
    const r = await h.post("/report", JSON.stringify(await h.sign(k, reportFields(h))), { "content-type": ct });
    status(r, [200, 415]);
    check(r.json, "not JSON");
  });
}
const BAD_JSON = [
  ["empty body", ""], ["only whitespace", "   \n"], ["open brace", "{"], ["extra close brace", "{}}"], ["single quotes", "{'a':1}"], ["trailing comma in object", '{"a":1,}'], ["trailing comma in array", "[1,2,]"],
  ["undefined", "undefined"], ["NaN literal", "NaN"], ["Infinity literal", "Infinity"], ["leading zero number", '{"a":01}'], ["bare fraction", '{"a":.5}'], ["bad escape", '{"a":"\\x"}'],
  ["unquoted key", "{a:1}"], ["comment", "{/*x*/}"], ["two documents", '{"a":1}{"b":2}'], ["NUL byte", "\u0000"], ["lone surrogate escape is valid JSON but extra comma", '{"a":"\\ud800",}'],
  ["unterminated string", '{"a":"b'], ["truncated number", '{"a":1e'], ["plus sign number", '{"a":+1}'], ["hex number", '{"a":0x10}'], ["tab inside string", '{"a":"x\ty"}'], ["a word", "hello"],
];
for (const path of ["/report", "/remove", "/register", "/schedule"]) {
  for (const [label, text] of BAD_JSON) {
    S(G.http, `POST ${path} invalid JSON: ${label}`, "400 body is not valid JSON", async (h) => {
      const r = await h.post(path, text);
      status(r, 400);
      errMatches(r, /not valid JSON/);
    });
  }
  for (const [label, text] of [["null", "null"], ["array", "[]"], ["number", "42"], ["string", '"s"'], ["true", "true"], ["empty object", "{}"]]) {
    S(G.http, `POST ${path} valid JSON of the wrong shape: ${label}`, "400 body must be an object / missing field", async (h) => {
      const r = await h.post(path, text);
      status(r, 400);
      errMatches(r, /object|missing field/);
    });
  }
  S(G.http, `POST ${path} invalid UTF-8 bytes`, "400", async (h) => { status(await h.post(path, new Uint8Array([0x7b, 0xff, 0xfe, 0x7d])), 400); });
  S(G.http, `POST ${path} UTF-16 encoded JSON`, "400", async (h) => { status(await h.post(path, Buffer.from("{}", "utf16le")), 400); });
  S(G.http, `POST ${path} 2000 nested arrays`, "400, never 5xx", async (h) => { status(await h.post(path, "[".repeat(2000) + "]".repeat(2000)), 400); });
  S(G.http, `POST ${path} 600 nested objects`, "400, never 5xx", async (h) => { status(await h.post(path, '{"a":'.repeat(600) + "1" + "}".repeat(600)), 400); });
  S(G.http, `POST ${path} 4096 unclosed brackets`, "400, never 5xx", async (h) => { status(await h.post(path, "[".repeat(4096)), 400); });
  S(G.http, `POST ${path} 100000 nested arrays (over the size limit)`, "413", async (h) => { status(await h.post(path, "[".repeat(100_000) + "]".repeat(100_000)), 413); });
}
S(G.http, "report whose heard_location is 400 levels of nested objects", "400, never 5xx", async (h) => {
  const nested = JSON.parse('{"a":'.repeat(400) + "1" + "}".repeat(400));
  const { res } = await report(h, { heard_location: nested });
  status(res, 400);
});
S(G.http, "report whose mode is a nested array", "400", async (h) => {
  const { res } = await report(h, { mode: JSON.parse("[".repeat(500) + "]".repeat(500)) });
  status(res, 400);
});
S(G.http, "__proto__ as a top-level key does not pollute or crash", "400 field not allowed", async (h) => {
  const k = await h.key();
  const text = JSON.stringify(await h.sign(k, reportFields(h))).replace(/^\{/, '{"__proto__":{"polluted":true},');
  status(await h.post("/report", text), 400);
  eq({}.polluted, undefined, "Object.prototype.polluted");
});
S(G.http, "very long header value and many headers", "answered normally", async (h) => {
  const headers = { "x-a": "A".repeat(4000) };
  for (let i = 0; i < 40; i++) headers["x-h" + i] = "v" + i;
  status(await h.send({ method: "GET", path: "/stations", headers }), 200);
});
S(G.http, "HEAD / and HEAD /map", "200 with HTML content type", async (h) => {
  for (const p of ["/", "/map"]) {
    const r = await h.send({ method: "HEAD", path: p });
    status(r, 200);
    check(/text\/html/.test(r.contentType), `content-type ${r.contentType}`);
  }
});
S(G.http, "every JSON route sets content-type, no-store and nosniff", "headers present", async (h) => {
  for (const p of ["/stations", "/zones"]) {
    const r = await h.get(p);
    status(r, 200);
    check(/^application\/json/.test(r.contentType), `${p} content-type`);
    eq(r.headers.get("cache-control"), "no-store", `${p} cache-control`);
    eq(r.headers.get("x-content-type-options"), "nosniff", `${p} nosniff`);
  }
});

// ---- 12. Map and homepage -------------------------------------------------

const HOSTILE = [
  "<script>alert(1)</script>", '"><img src=x onerror=alert(1)>', "'><svg onload=alert(1)>", "</title><script>alert(1)</script>", "&amp;&lt;script&gt;", "javascript:alert(1)",
  "</section><script>alert(1)</script>", "‮<b>x", "<!--", "]]>", "${7*7}", "{{7*7}}",
];
const noExternal = (html, where) => {
  check(!/https?:\/\//i.test(html), `${where}: contains an http(s) URL: ${snip(html.match(/.{0,40}https?:\/\/.{0,40}/i)?.[0] ?? "")}`);
  check(!/(?:src|href|action|data)\s*=\s*["']?\s*\/\/[a-z0-9]/i.test(html), `${where}: protocol-relative URL`);
  check(!/<(?:link|iframe|object|embed|base|form|audio|video|source|frame)\b/i.test(html), `${where}: element that can load or post something`);
  for (const m of html.matchAll(/<img\b[^>]*>/gi)) check(/\ssrc="\/operate\.png"/.test(m[0]), `${where}: an image other than /operate.png: ${snip(m[0])}`);
  check(!/<meta[^>]+http-equiv/i.test(html), `${where}: meta http-equiv`);
  const noScript = html.replace(/<script>[\s\S]*?<\/script>/, "");
  check(!/<[a-z][^>]*\son[a-z]+\s*=/i.test(noScript), `${where}: inline event handler in a tag`);
  check(!/(?:href|src|action|xlink:href)\s*=\s*["']?\s*javascript:/i.test(noScript), `${where}: javascript: URL in an attribute`);
  check(!/url\(\s*["']?(?!#|\/fonts\/display\.woff2)/i.test(html), `${where}: CSS url() other than a fragment`);
  check(!/@import/i.test(html), `${where}: @import`);
  for (const m of html.matchAll(/\bhref\s*=\s*"([^"]*)"/g)) check(m[1].startsWith("#") || m[1].startsWith("/"), `${where}: href leaves the site: ${m[1]}`);
};
async function sha256B64(s) { return Buffer.from(await crypto.subtle.digest("SHA-256", enc.encode(s))).toString("base64"); }

// Page scripts may reach the server only through the one getJson helper (window.fetch with
// credentials omitted), and only for /map-data and /release. Everything else stays forbidden.
const scriptCalls = (js, where) => {
  check(!/https?:\/\/|XMLHttpRequest|eval\(|new Function|document\.write|innerHTML|WebSocket|sendBeacon|import\(|http-equiv|<meta/i.test(js), `${where} script calls out or builds HTML: ${snip(js.match(/https?:\/\/|XMLHttpRequest|eval\(|new Function|document\.write|innerHTML|WebSocket|sendBeacon|import\(/)?.[0] ?? "")}`);
  const fetches = js.match(/\bfetch\(/g) || [];
  check(fetches.length <= 1, `${where} script has ${fetches.length} fetch calls`);
  if (fetches.length) check(/window\.fetch\(url, \{.{0,120}credentials: "omit"/.test(js), `${where} fetch must go through the credentials-omitted helper`);
  for (const m of js.matchAll(/getJson\(\s*([^)]*?)\s*\)/g)) {
    if (/^(function|url)\b/.test(m[1])) continue; // the helper's own definition
    check(/^"\/(map-data|release)"$/.test(m[1]), `${where} script fetches something other than /map-data or /release: ${m[1]}`);
  }
};
S(G.page, "home page: one hashed inline script, no external URL, strict CSP", "200 HTML; CSP script-src is exactly the script's hash; connect-src 'self'", async (h) => {
  const r = await h.get("/");
  status(r, 200);
  check(/^text\/html/.test(r.contentType), "content type");
  const csp = r.headers.get("content-security-policy") ?? "";
  check(/default-src 'none'/.test(csp), `CSP ${csp}`);
  const scripts = [...r.text.matchAll(/<script\b[^>]*>([\s\S]*?)<\/script>/gi)];
  eq(scripts.length, 1, "script elements");
  check(/^<script>/.test(scripts[0][0]), "script has attributes or src");
  const hash = await sha256B64(scripts[0][1]);
  check(csp.includes(`script-src 'sha256-${hash}'`), `CSP does not carry the script's hash (${hash}): ${csp}`);
  eq((csp.match(/script-src/g) || []).length, 1, "script-src directives");
  check(/connect-src 'self'(;|$)/.test(csp), `CSP lacks connect-src 'self': ${csp}`);
  check(!/unsafe-eval|unsafe-inline[^;]*script|script-src[^;]*(unsafe-inline|\*)|https?:|data:|wasm-unsafe/.test(csp.replace(/style-src[^;]*;?/, "")), `CSP too loose: ${csp}`);
  for (const d of ["default-src 'none'", "frame-ancestors 'none'", "base-uri 'none'", "form-action 'none'"]) check(csp.includes(d), `CSP lacks ${d}`);
  // The home page links to the project source and the support page; nothing else leaves the site.
  noExternal(r.text.replace(/<script>[\s\S]*?<\/script>/, "").replace(/href="https:\/\/(?:github\.com\/adswill\/kahf-modem|ko-fi\.com\/adswill)" rel="noopener noreferrer"/g, 'href="#"'), "home");
  check(!/http-equiv\s*=\s*["']?refresh/i.test(r.text), "meta refresh");
  scriptCalls(scripts[0][1], "home");
  eq(r.headers.get("x-content-type-options"), "nosniff", "nosniff");
});
S(G.page, "home page ignores query string content", "query not echoed", async (h) => {
  const r = await h.get("/?q=%3Cscript%3Ealert(1)%3C/script%3E&x=ZZZMARKERZZZ");
  status(r, 200);
  check(!r.text.includes("ZZZMARKERZZZ") && !r.text.includes("alert(1)"), "query echoed in home page");
});
const MAP_LIBRARY_RE = /^https:\/\/unpkg\.com\/maplibre-gl@5\.7\.3\/dist\/maplibre-gl\.(js|css)$/;
const MAP_TILE_URL = "https://tile.openstreetmap.org/{z}/{x}/{y}.png";
S(G.page, "map page: MapLibre with integrity, one hashed inline script, only the map's own hosts", "two scripts (library, then the hashed inline one); CSP lists only the map hosts", async (h) => {
  status(await report(h).then((x) => x.res), 200);
  const r = await h.get("/map");
  status(r, 200);
  const all = [...r.text.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script>/gi)];
  eq(all.length, 2, "script elements");
  const lib = all[0][1];
  const src = (lib.match(/\bsrc="([^"]+)"/) || [])[1] ?? "";
  check(MAP_LIBRARY_RE.test(src), `library script is not the pinned MapLibre 5.7.3 from unpkg: ${src}`);
  check(/integrity="sha384-[A-Za-z0-9+/=]+"/.test(lib) && /crossorigin="anonymous"/.test(lib), "library script lacks integrity or crossorigin");
  const inline = all[1];
  check(!/\bsrc\s*=/i.test(inline[1]), "inline script has a src");
  const csp = r.headers.get("content-security-policy") ?? "";
  const hash = await sha256B64(inline[2]);
  check(csp.includes(`'sha256-${hash}'`), `CSP does not carry the inline script's hash (${hash}): ${csp}`);
  check(csp.includes("https://unpkg.com"), "CSP does not allow the library host");
  check(!/unsafe-eval|script-src[^;]*unsafe-inline|script-src[^;]*\*/.test(csp), `CSP too loose: ${csp}`);
  check(/default-src 'none'/.test(csp), "CSP lacks default-src 'none'");
  for (const d of ["frame-ancestors 'none'", "base-uri 'none'", "form-action 'none'"]) check(csp.includes(d), `CSP lacks ${d}`);
  // every URL in the page is one of the map's own
  const urls = [...(r.text.match(/https?:\/\/[^\s"'<>)]+/g) || [])];
  for (const u of urls) check(MAP_LIBRARY_RE.test(u) || u === MAP_TILE_URL || u.startsWith("https://kahf.dev") || u.startsWith("https://www.kahf.dev") || u.startsWith("https://github.com/adswill/"), `map page contains an unexpected URL: ${u}`);
  // our script: one fetch, to /map-data, and the one tile URL
  const js = inline[2];
  const fetches = js.match(/\bfetch\(/g) || [];
  eq(fetches.length, 2, "fetch calls: the shared helper and the live poll");
  check(/window\.fetch\("\/map-data"/.test(js), "live script does not fetch /map-data");
  check(!/XMLHttpRequest|eval\(|new Function|document\.write|innerHTML|WebSocket|sendBeacon|import\(|http-equiv/i.test(js), "live script uses a forbidden API");
  check(!/https?:\/\//.test(js.replace(MAP_TILE_URL, "")), "live script contains a URL other than the tile URL");
  check(!/<meta[^>]+http-equiv/i.test(r.text) && !/http-equiv\s*=\s*["']?refresh/i.test(r.text), "meta refresh");
  eq(r.headers.get("referrer-policy"), "no-referrer", "referrer-policy");
});
S(G.page, "map page with no stations", "200 valid page with at most two scripts", async (h) => {
  const r = await h.get("/map");
  status(r, 200);
  check([...r.text.matchAll(/<script\b/gi)].length <= 2, "more than two scripts");
  check(/default-src 'none'/.test(r.headers.get("content-security-policy") ?? ""), "CSP");
});
for (const [i, evil] of HOSTILE.entries()) {
  S(G.page, `hostile string #${i + 1} ${snip(evil, 30)} in stored fields (callsigns, mode, times), via the store`, "/map carries no stored text; two scripts", async (h) => {
    const k = await h.key();
    const f = reportFields(h);
    await h.seedRecord({
      ...f, reporter_call: evil, heard_call: evil + "X", mode: evil, heard_at: evil, frequency_khz: evil,
      reporter_location: { mode: "area", latitude: 1, longitude: 1, radius_km: 5 }, heard_location: { mode: "precise", latitude: 2, longitude: 2 },
      public_key: k.publicKey, refreshed_at: h.now(),
    });
    const r = await h.get("/map");
    status(r, 200);
    const stripped = r.text.replace(/<script\b[\s\S]*?<\/script>/gi, "");
    eq([...r.text.matchAll(/<script\b/gi)].length, 2, "script elements");
    for (const marker of ["<script>alert", "<img src=x", "<svg onload", "</title><script", "</section><script", "<!--"]) {
      if (evil.includes(marker)) check(!r.text.includes(marker), `raw ${marker} in map HTML`);
    }
    check(!stripped.includes(evil), `stored text appears in map HTML: ${snip(evil, 40)}`);
    check(!/\son[a-z]+\s*=/i.test(stripped), "map with hostile data has an inline event handler");
    const st = await h.get("/stations");
    status(st, 200);
    check(/^application\/json/.test(st.contentType) && st.headers.get("x-content-type-options") === "nosniff", "stations JSON must be JSON with nosniff");
  }, { needs: ["seed"] });
}
S(G.page, "legal radio-mode and callsign characters appear in the live feed exactly", "mode 'A+B/C.D_E-F G' and call with /P in /map-data", async (h) => {
  const call = h.call() + "/P";
  const { res } = await report(h, { reporter_call: call, mode: "A+B/C.D_E-F G" });
  status(res, 200);
  const m = await h.get("/map-data");
  status(m, 200);
  const feed = JSON.parse(m.text);
  check(feed.stations.some((x) => x.reporter_call === call && x.mode === "A+B/C.D_E-F G"), "values not in the feed as sent");
  const s = (await stationsOf(h, call))[0];
  eq(s.mode, "A+B/C.D_E-F G", "stored mode");
});
S(G.page, "map with 300 stations", "200, one script, no NaN/Infinity, every href local", async (h) => {
  const k = await h.key();
  for (let i = 0; i < 300; i++) {
    await h.seedRecord({
      ...reportFields(h), reporter_call: "R" + (i % 9) + "AA" + i.toString(36).toUpperCase().padStart(3, "A"), heard_call: "H" + (i % 9) + "BB" + i.toString(36).toUpperCase().padStart(3, "A"),
      reporter_location: { mode: "area", latitude: ((i * 7) % 180) - 90, longitude: ((i * 13) % 360) - 180, radius_km: 1 + (i % 100) },
      heard_location: { mode: "precise", latitude: ((i * 11) % 180) - 90, longitude: ((i * 17) % 360) - 180 },
      public_key: k.publicKey, refreshed_at: h.now(),
    });
  }
  const page = await h.get("/map");
  status(page, 200);
  eq([...page.text.matchAll(/<script\b/gi)].length, 2, "scripts");
  const r = await h.get("/map-data");
  status(r, 200);
  check(!/NaN|Infinity|undefined/.test(r.text), "NaN/Infinity/undefined in the feed");
  check(r.text.length < 3_000_000, `feed is ${r.text.length} bytes`);
  eq(JSON.parse(r.text).stations.length, 300, "stations in the feed");
}, { needs: ["seed"] });
S(G.page, "map and stations at the poles and the antimeridian", "no NaN, no negative radius, ellipse sizes finite", async (h) => {
  const k = await h.key();
  const spots = [[90, 0], [-90, 0], [90, 180], [-90, -180], [0, 180], [0, -180], [89.9999, 179.9999], [0, 0]];
  for (const [i, [lat, lon]] of spots.entries()) {
    await h.seedRecord({
      ...reportFields(h), reporter_call: "P" + i + "AAA", heard_call: "Q" + i + "BBB",
      reporter_location: { mode: "area", latitude: lat, longitude: lon, radius_km: 100 }, heard_location: { mode: "area", latitude: -lat, longitude: -lon, radius_km: 100 },
      public_key: k.publicKey, refreshed_at: h.now(),
    });
  }
  const r = await h.get("/map-data");
  status(r, 200);
  check(!/NaN|Infinity|undefined/.test(r.text), "NaN/Infinity/undefined in the feed");
  const feed = JSON.parse(r.text);
  const points = [...feed.stations.map((x) => x.reporter_location), ...feed.stations.map((x) => x.heard_location)].filter(Boolean);
  for (const loc of points) {
    check(Math.abs(loc.latitude) <= 90 && Math.abs(loc.longitude) <= 180, `out of range: ${loc.latitude},${loc.longitude}`);
    if (loc.radius_km !== undefined) check(loc.radius_km >= 0 && Number.isFinite(loc.radius_km), `bad radius ${loc.radius_km}`);
  }
}, { needs: ["seed"] });
S(G.page, "/stations never exposes public keys or signatures", "no key material in the JSON", async (h) => {
  const { res, body } = await report(h);
  status(res, 200);
  const r = await h.get("/stations");
  check(!r.text.includes(body.public_key) && !r.text.includes(body.signature), "key or signature in /stations");
  check(!/public_key|signature/.test(r.text), "key field names in /stations");
  const z = await h.get("/zones");
  check(!z.text.includes(body.public_key), "key in /zones");
  const m = await h.get("/map");
  check(!m.text.includes(body.public_key), "key in /map");
});
S(G.page, "/map ignores query string content", "query not echoed", async (h) => {
  const r = await h.get("/map?q=%3Cscript%3Ealert(1)%3C/script%3E&x=ZZZMARKERZZZ");
  status(r, 200);
  check(!r.text.includes("ZZZMARKERZZZ") && !r.text.includes("alert(1)"), "query echoed in map page");
});

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

/** Runs one scenario with a fresh harness. Returns { result, actual, detail }. */
export async function runScenario(sc, makeHarness) {
  const h = await makeHarness(sc);
  const missing = sc.needs.filter((n) => !h.caps[n]);
  if (missing.length) return { result: "SKIP", actual: `needs ${missing.join(", ")} (in-process only)`, h };
  try {
    const out = await sc.fn(h);
    audit(h);
    return { result: "PASS", actual: typeof out === "string" ? out : h.log.length ? brief(h.log[h.log.length - 1].res) : "ok", h };
  } catch (err) {
    const last = h.last;
    const detail = last ? `last request: ${last.req.method} ${last.req.path} ${snip(last.req.body, 300)} -> ${last.res.status} ${snip(last.res.text, 300)}` : "";
    return { result: "FAIL", actual: err instanceof Error ? err.message : String(err), detail, h };
  }
}
