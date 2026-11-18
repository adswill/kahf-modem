// Tests for every route in src/routes.js, run against a fake in-memory store
// with a fixed clock. Run with: npm test (node --test). No wrangler, no network.
// Node 20 cannot import .ts files, so a small load hook reads page.ts as JavaScript.
import { test, mock } from "node:test";
import assert from "node:assert/strict";
import { register } from "node:module";
import { canonicalJson, encodeBase64, EXPIRY_MS, MAX_SCHEDULE_REQUESTS } from "../src/core.js";
import { fakeStore } from "../tools/fake-store.mjs";

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
const { handleRequest } = await import("../src/routes.js");

const NOW = Date.parse("2026-11-17T12:00:00Z");
const NOW_S = NOW / 1000;
const BASE = "https://kahf.test";
const enc = new TextEncoder();

// ---- helpers ---------------------------------------------------------------

async function newKey() {
  const pair = await crypto.subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"]);
  const raw = new Uint8Array(await crypto.subtle.exportKey("raw", pair.publicKey));
  return { privateKey: pair.privateKey, publicKey: encodeBase64(raw) };
}

/** The request body for `fields`, signed with `key` as the protocol requires. */
async function signed(key, fields) {
  const sig = new Uint8Array(
    await crypto.subtle.sign({ name: "Ed25519" }, key.privateKey, enc.encode(canonicalJson(fields))),
  );
  return { ...fields, public_key: key.publicKey, signature: encodeBase64(sig) };
}

const post = (path, body) =>
  new Request(BASE + path, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: typeof body === "string" ? body : JSON.stringify(body),
  });
const get = (path, method = "GET") => new Request(BASE + path, { method });

async function text(res) {
  return res.text();
}

async function sha256Base64(s) {
  const d = await crypto.subtle.digest("SHA-256", enc.encode(s));
  return Buffer.from(d).toString("base64");
}

const REPORT = {
  reporter_call: "K1ABC",
  reporter_location: { mode: "precise", latitude: 51.5074, longitude: -0.1278 },
  heard_call: "DL1XYZ",
  heard_location: { mode: "area", latitude: 48.8566, longitude: 2.3522, radius_km: 50 },
  heard_at: "2026-11-17T11:59:00Z",
  frequency_khz: 14_100,
  mode: "KAHF",
};

const REGISTER = { area: "N51W1", callsign: "K1ABC" };

// ---- pages -----------------------------------------------------------------

test("GET / serves the home page with only its hashed script and connect-src self", async () => {
  const res = await handleRequest(get("/"), fakeStore(), NOW);
  assert.equal(res.status, 200);
  assert.match(res.headers.get("content-type"), /^text\/html; charset=utf-8/);
  const csp = res.headers.get("content-security-policy");
  const body = await text(res);
  assert.match(csp, /default-src 'none'/);
  assert.match(body, /^<!doctype html>/i);
  assert.match(body, /KAHF/);

  // Exactly one script source: the hash of the home page's inline script.
  const { homeScriptSha256 } = await import("../src/page.ts");
  const expected = await homeScriptSha256();
  assert.equal((csp.match(/script-src/g) || []).length, 1);
  assert.ok(csp.includes(`script-src ${expected}`), `CSP should allow the home script hash: ${csp}`);
  const scripts = [...body.matchAll(/<script>([\s\S]*?)<\/script>/g)];
  assert.equal(scripts.length, 1);
  assert.equal(await sha256Base64(scripts[0][1]), expected.replace(/^'sha256-|'$/g, ""));

  // Only this server may be called from the page; nothing external, nothing framed.
  assert.match(csp, /connect-src 'self'/);
  assert.match(csp, /frame-ancestors 'none'/);
  assert.match(csp, /form-action 'none'/);
  assert.doesNotMatch(csp, /https?:|'unsafe-inline'[^;]*script|'unsafe-eval'/);
});

test("HEAD / is allowed; POST / is refused with the allowed methods", async () => {
  const head = await handleRequest(get("/", "HEAD"), fakeStore(), NOW);
  assert.equal(head.status, 200);
  const post_ = await handleRequest(post("/", {}), fakeStore(), NOW);
  assert.equal(post_.status, 405);
  assert.equal(post_.headers.get("allow"), "GET, HEAD");
  assert.match(post_.headers.get("content-type"), /application\/json/);
});

test("GET /map: the CSP allows the one inline script by hash, and only the map's own hosts", async () => {
  const store = fakeStore();
  await store.put({ ...REPORT, public_key: "x", refreshed_at: NOW });
  const res = await handleRequest(get("/map"), store, NOW);
  assert.equal(res.status, 200);
  assert.match(res.headers.get("content-type"), /^text\/html/);
  const body = await text(res);
  const inline = [...body.matchAll(/<script>([\s\S]*?)<\/script>/g)];
  assert.equal(inline.length, 1);
  const csp = res.headers.get("content-security-policy");
  const hash = await sha256Base64(inline[0][1]);
  assert.ok(csp.includes(`'sha256-${hash}'`), `CSP should allow the script hash: ${csp}`);
  assert.doesNotMatch(csp, /'unsafe-eval'|script-src[^;]*'unsafe-inline'|script-src[^;]*\*/);
  assert.match(csp, /script-src [^;]*https:\/\/unpkg\.com/);
  assert.match(csp, /connect-src 'self';/);
  assert.match(csp, /img-src 'self' data: blob:;/);
  assert.doesNotMatch(csp, /tile\.openstreetmap/);
});

test("GET /map with no reports still answers with a valid page", async () => {
  const res = await handleRequest(get("/map"), fakeStore(), NOW);
  assert.equal(res.status, 200);
  assert.match(await text(res), /0 relays, 0 stations/);
});

// ---- stations and zones ----------------------------------------------------

test("GET /stations lists live records only, without public keys", async () => {
  const store = fakeStore();
  await store.put({ ...REPORT, public_key: "KEYLIVE", refreshed_at: NOW });
  await store.put({ ...REPORT, reporter_call: "K2OLD", public_key: "KEYOLD", refreshed_at: NOW - EXPIRY_MS - 1 });
  const res = await handleRequest(get("/stations"), store, NOW);
  assert.equal(res.status, 200);
  assert.match(res.headers.get("content-type"), /^application\/json; charset=utf-8/);
  const body = await res.json();
  assert.equal(body.stations.length, 1);
  assert.equal(body.stations[0].reporter_call, "K1ABC");
  assert.equal(body.stations[0].refreshed_at, new Date(NOW).toISOString());
  assert.equal(body.stations[0].public_key, undefined);
  assert.equal(body.expires_after_hours, 48);
  assert.doesNotMatch(JSON.stringify(body), /KEYLIVE/);
});

test("GET /stations refuses POST", async () => {
  const res = await handleRequest(post("/stations", {}), fakeStore(), NOW);
  assert.equal(res.status, 405);
});

test("GET /zones: busy channels per area from live records, with the pool split", async () => {
  const store = fakeStore();
  // 14_100 kHz is channel floor(100 / 2.7) = 37 in the segment.
  await store.put({ ...REPORT, public_key: "k", refreshed_at: NOW });
  await store.put({
    ...REPORT,
    reporter_call: "K2BUSY",
    frequency_khz: 14_050,
    heard_at: "2026-11-17T11:55:00Z",
    public_key: "k",
    refreshed_at: NOW,
  });
  // Heard 10 minutes ago: outside the busy window.
  await store.put({
    ...REPORT,
    reporter_call: "K3OLD",
    heard_at: "2026-11-17T11:40:00Z",
    public_key: "k",
    refreshed_at: NOW,
  });
  const res = await handleRequest(get("/zones"), store, NOW);
  assert.equal(res.status, 200);
  const body = await res.json();
  assert.equal(body.busy_window_s, 600);
  // K1ABC at 51.5074, -0.1278 lies in N51W1; K2BUSY too.
  assert.deepEqual(body.zones, { N51W1: [18, 37] });
  assert.equal(body.pools.defaultPool, "open");
  assert.equal(body.pools.bandChannels, body.pools.online.length + body.pools.open.length);
  assert.ok(!body.pools.areas.N51W1.channels.includes(18));
  assert.equal(body.pools.areas.N51W1.pool, "online");
});

test("GET /zones with no reports has no busy channels", async () => {
  const body = await (await handleRequest(get("/zones"), fakeStore(), NOW)).json();
  assert.deepEqual(body.zones, {});
  assert.deepEqual(body.pools.areas, {});
});

// ---- reports and removals --------------------------------------------------

test("POST /report stores a signed report and makes it visible", async () => {
  const store = fakeStore();
  const key = await newKey();
  const res = await handleRequest(post("/report", await signed(key, REPORT)), store, NOW);
  assert.equal(res.status, 200);
  assert.deepEqual(await res.json(), { ok: true, expires_after_s: 172800 });
  assert.ok(store.recs.has("K1ABC>DL1XYZ"));
  assert.equal(store.recs.get("K1ABC>DL1XYZ").public_key, key.publicKey);
  assert.equal(store.recs.get("K1ABC>DL1XYZ").refreshed_at, NOW);
  const stations = await (await handleRequest(get("/stations"), store, NOW)).json();
  assert.equal(stations.stations.length, 1);
});

test("POST /report: bad JSON, oversize body, bad fields and bad signature are refused", async () => {
  const store = fakeStore();
  const key = await newKey();

  const notJson = await handleRequest(post("/report", "{nope"), store, NOW);
  assert.equal(notJson.status, 400);

  const big = await handleRequest(post("/report", "x".repeat(5000)), store, NOW);
  assert.equal(big.status, 413);

  const topLevel = await handleRequest(
    post("/report", { ...(await signed(key, REPORT)), latitude: 51.5 }),
    store,
    NOW,
  );
  assert.equal(topLevel.status, 400);
  assert.match((await topLevel.json()).error, /location/);

  const tampered = await signed(key, REPORT);
  tampered.frequency_khz = 14_200;
  const badSig = await handleRequest(post("/report", tampered), store, NOW);
  assert.equal(badSig.status, 401);
  assert.equal(store.recs.size, 0);
});

test("POST /report: a callsign already held by another key gets 409", async () => {
  const store = fakeStore();
  const first = await newKey();
  const other = await newKey();
  assert.equal((await handleRequest(post("/report", await signed(first, REPORT)), store, NOW)).status, 200);
  const res = await handleRequest(post("/report", await signed(other, REPORT)), store, NOW);
  assert.equal(res.status, 409);
  assert.equal(store.recs.get("K1ABC>DL1XYZ").public_key, first.publicKey);
});

test("POST /remove: the owning key removes its record", async () => {
  const store = fakeStore();
  const key = await newKey();
  await handleRequest(post("/report", await signed(key, REPORT)), store, NOW);
  const removal = { heard_call: "DL1XYZ", reporter_call: "K1ABC", signed_at: "2026-11-17T12:00:00Z" };
  const res = await handleRequest(post("/remove", await signed(key, removal)), store, NOW);
  assert.equal(res.status, 200);
  assert.deepEqual(await res.json(), { ok: true, removed: true });
  assert.equal(store.recs.size, 0);
});

test("POST /remove: another key gets 403, a missing record gets 404, a bad signature 401", async () => {
  const store = fakeStore();
  const owner = await newKey();
  const intruder = await newKey();
  const removal = { heard_call: "DL1XYZ", reporter_call: "K1ABC", signed_at: "2026-11-17T12:00:00Z" };

  const missing = await handleRequest(post("/remove", await signed(owner, removal)), store, NOW);
  assert.equal(missing.status, 404);

  await handleRequest(post("/report", await signed(owner, REPORT)), store, NOW);
  const wrongKey = await handleRequest(post("/remove", await signed(intruder, removal)), store, NOW);
  assert.equal(wrongKey.status, 403);

  const forged = await signed(owner, removal);
  forged.reporter_call = "K9ZZZ";
  const badSig = await handleRequest(post("/remove", forged), store, NOW);
  assert.equal(badSig.status, 401);
  assert.equal(store.recs.size, 1);
});

// ---- relays ----------------------------------------------------------------

test("POST /register stores a relay with its callsign, area and key", async () => {
  const store = fakeStore();
  const key = await newKey();
  const res = await handleRequest(post("/register", await signed(key, REGISTER)), store, NOW);
  assert.equal(res.status, 200);
  const body = await res.json();
  assert.equal(body.ok, true);
  assert.equal(body.callsign, "K1ABC");
  const rel = store.relays.get("K1ABC");
  assert.equal(rel.area, "N51W1");
  assert.equal(rel.public_key, key.publicKey);
  assert.equal(rel.refreshed_at, NOW);
});

test("POST /register refuses a callsign that fails the format check", async () => {
  const store = fakeStore();
  const key = await newKey();
  for (const bad of ["ABC DEF", "NOTACALL", "", "K1ABCDEFGHIJK"]) {
    const res = await handleRequest(post("/register", await signed(key, { ...REGISTER, callsign: bad })), store, NOW);
    assert.equal(res.status, 400, `callsign "${bad}" should be refused`);
    assert.match((await res.json()).error, /callsign/);
  }
  assert.equal(store.relays.size, 0);
});

test("POST /register refuses a bad area, a bad signature and another key", async () => {
  const store = fakeStore();
  const key = await newKey();
  const other = await newKey();

  const area = await handleRequest(post("/register", await signed(key, { ...REGISTER, area: "51W1" })), store, NOW);
  assert.equal(area.status, 400);

  const forged = await signed(key, REGISTER);
  forged.area = "N52W1";
  assert.equal((await handleRequest(post("/register", forged), store, NOW)).status, 401);

  assert.equal((await handleRequest(post("/register", await signed(key, REGISTER)), store, NOW)).status, 200);
  const clash = await handleRequest(post("/register", await signed(other, REGISTER)), store, NOW);
  assert.equal(clash.status, 409);
  assert.equal(store.relays.get("K1ABC").public_key, key.publicKey);
});

test("POST /register: an expired registration can be taken over", async () => {
  const store = fakeStore();
  const old = await newKey();
  const fresh = await newKey();
  store.relays.set("K1ABC", {
    callsign: "K1ABC",
    area: "N51W1",
    public_key: old.publicKey,
    refreshed_at: NOW - EXPIRY_MS - 1,
  });
  const res = await handleRequest(post("/register", await signed(fresh, REGISTER)), store, NOW);
  assert.equal(res.status, 200);
  assert.equal(store.relays.get("K1ABC").public_key, fresh.publicKey);
});

test("POST /schedule returns the planner output for the registered relay", async () => {
  const store = fakeStore();
  const key = await newKey();
  await handleRequest(post("/register", await signed(key, REGISTER)), store, NOW);

  const requests = [
    { channel: 3, duration: 60, direction: "tx" },
    { channel: 3, duration: 30, direction: "rx", urgent: true },
  ];
  const res = await handleRequest(post("/schedule", await signed(key, { relay: "K1ABC", requests })), store, NOW);
  assert.equal(res.status, 200);
  const body = await res.json();
  assert.equal(body.relay, "K1ABC");
  assert.equal(body.slots.length, 2);
  // Urgent first, then the other one after it on the same channel and relay.
  assert.deepEqual(body.slots[0], { channel: 3, start: NOW_S, duration: 30, direction: "rx" });
  assert.deepEqual(body.slots[1], { channel: 3, start: NOW_S + 30, duration: 60, direction: "tx" });
});

test("POST /schedule: an unregistered relay gets 404, another key 403, a bad signature 401", async () => {
  const store = fakeStore();
  const key = await newKey();
  const other = await newKey();
  const fields = { relay: "K1ABC", requests: [{ channel: 1, duration: 10, direction: "rx" }] };

  const unknown = await handleRequest(post("/schedule", await signed(key, fields)), store, NOW);
  assert.equal(unknown.status, 404);

  await handleRequest(post("/register", await signed(key, REGISTER)), store, NOW);
  const wrong = await handleRequest(post("/schedule", await signed(other, fields)), store, NOW);
  assert.equal(wrong.status, 403);

  const forged = await signed(key, fields);
  forged.requests[0].duration = 11;
  assert.equal((await handleRequest(post("/schedule", forged), store, NOW)).status, 401);
});

test("POST /schedule refuses bad requests with a plain message", async () => {
  const store = fakeStore();
  const key = await newKey();
  await handleRequest(post("/register", await signed(key, REGISTER)), store, NOW);
  const send = async (requests) =>
    handleRequest(post("/schedule", await signed(key, { relay: "K1ABC", requests })), store, NOW);

  const outOfBand = await send([{ channel: 10_000, duration: 10, direction: "tx" }]);
  assert.equal(outOfBand.status, 400);
  assert.match((await outOfBand.json()).error, /channel/);

  const tooLong = await send([{ channel: 0, duration: 301, direction: "tx" }]);
  assert.equal(tooLong.status, 400);

  const none = await send([]);
  assert.equal(none.status, 400);

  const many = await send(Array.from({ length: MAX_SCHEDULE_REQUESTS + 1 }, () => ({ channel: 0, duration: 1, direction: "rx" })));
  assert.equal(many.status, 400);

  const extra = await send([{ channel: 0, duration: 1, direction: "rx", note: "x" }]);
  assert.equal(extra.status, 400);
  assert.match((await extra.json()).error, /not allowed/);
});

test("POST /schedule: a missing relay in the body is refused before any lookup", async () => {
  const store = fakeStore();
  const key = await newKey();
  const res = await handleRequest(
    post("/schedule", await signed(key, { requests: [{ channel: 0, duration: 1, direction: "rx" }] })),
    store,
    NOW,
  );
  assert.equal(res.status, 400);
  assert.match((await res.json()).error, /missing field: relay/);
});

test("GET /schedule and GET /register are refused", async () => {
  assert.equal((await handleRequest(get("/schedule"), fakeStore(), NOW)).status, 405);
  assert.equal((await handleRequest(get("/register"), fakeStore(), NOW)).status, 405);
  assert.equal((await handleRequest(get("/report"), fakeStore(), NOW)).status, 405);
  assert.equal((await handleRequest(get("/remove"), fakeStore(), NOW)).status, 405);
});

test("POST /schedule: a second relay in the same area avoids the first relay's slots", async () => {
  const store = fakeStore();
  const a = await newKey();
  const b = await newKey();
  const regA = { area: "N50W3", callsign: "W0AAA" };
  const regB = { area: "N50W3", callsign: "W0AAB" };
  await handleRequest(post("/register", await signed(a, regA)), store, NOW);
  await handleRequest(post("/register", await signed(b, regB)), store, NOW);

  const reqA = { relay: "W0AAA", requests: [{ channel: 80, duration: 60, direction: "tx" }] };
  const first = await (await handleRequest(post("/schedule", await signed(a, reqA)), store, NOW)).json();
  assert.equal(first.slots[0].start, NOW_S);

  const reqB = { relay: "W0AAB", requests: [{ channel: 80, duration: 60, direction: "tx" }] };
  const second = await (await handleRequest(post("/schedule", await signed(b, reqB)), store, NOW)).json();
  const [s] = second.slots;
  assert.ok(s.start >= NOW_S + 60, `W0AAB starts at ${s.start}, inside W0AAA's slot`);

  // Same channel in another area does not clash.
  const regC = { area: "N51W3", callsign: "W0AAC" };
  const c = await newKey();
  await handleRequest(post("/register", await signed(c, regC)), store, NOW);
  const reqC = { relay: "W0AAC", requests: [{ channel: 80, duration: 60, direction: "tx" }] };
  const third = await (await handleRequest(post("/schedule", await signed(c, reqC)), store, NOW)).json();
  assert.equal(third.slots[0].start, NOW_S);
});

test("POST /schedule: a relay's slots that have already ended are not booked", async () => {
  const store = fakeStore();
  const a = await newKey();
  const b = await newKey();
  await handleRequest(post("/register", await signed(a, { area: "N50W3", callsign: "W0AAA" })), store, NOW);
  await handleRequest(post("/register", await signed(b, { area: "N50W3", callsign: "W0AAB" })), store, NOW);
  await handleRequest(
    post("/schedule", await signed(a, { relay: "W0AAA", requests: [{ channel: 80, duration: 60, direction: "tx" }] })),
    store,
    NOW,
  );
  const later = NOW + 61_000;
  const res = await handleRequest(
    post("/schedule", await signed(b, { relay: "W0AAB", requests: [{ channel: 80, duration: 60, direction: "tx" }] })),
    store,
    later,
  );
  assert.equal((await res.json()).slots[0].start, Math.floor(later / 1000));
});

// ---- unknown routes and failures -------------------------------------------

test("unknown paths get a JSON 404, including prototype names", async () => {
  for (const path of ["/nope", "/constructor", "/__proto__"]) {
    const res = await handleRequest(get(path), fakeStore(), NOW);
    assert.equal(res.status, 404, path);
    assert.match(res.headers.get("content-type"), /application\/json/);
    assert.deepEqual(await res.json(), { error: "not found" });
  }
});

test("an unexpected failure returns a plain JSON 500 with no details", async () => {
  const log = mock.method(console, "error", () => {});
  try {
    const store = fakeStore();
    store.listAll = async () => {
      throw new Error("secret detail /private/path");
    };
    const res = await handleRequest(get("/stations"), store, NOW);
    assert.equal(res.status, 500);
    assert.match(res.headers.get("content-type"), /application\/json/);
    const body = await text(res);
    assert.deepEqual(JSON.parse(body), { error: "internal error" });
    assert.doesNotMatch(body, /secret|private|at /);
    assert.equal(log.mock.calls.length, 1);
    assert.doesNotMatch(String(log.mock.calls[0].arguments.join(" ")), /secret/);
  } finally {
    mock.restoreAll();
  }
});

// ---- map tile proxy ---------------------------------------------------------

function tileOptions(calls, reply) {
  const stash = new Map();
  return {
    fetchImpl: async (url, init) => {
      calls.push({ url, init });
      return reply();
    },
    tileCache: {
      match: async (req) => stash.get(req.url)?.clone(),
      put: async (req, res) => void stash.set(req.url, res),
    },
  };
}

const PNG = new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10, 1, 2, 3]);
const okTile = () => new Response(PNG, { headers: { "content-type": "image/png" } });

test("GET /tiles/z/x/y.png fetches from OpenStreetMap with a KAHF user agent and caches", async () => {
  const calls = [];
  const options = tileOptions(calls, okTile);
  const res = await handleRequest(new Request(`${BASE}/tiles/3/4/2.png`), fakeStore(), NOW, options);
  assert.equal(res.status, 200);
  assert.equal(res.headers.get("content-type"), "image/png");
  assert.match(res.headers.get("cache-control"), /max-age=604800/);
  assert.equal(res.headers.get("x-content-type-options"), "nosniff");
  assert.deepEqual([...new Uint8Array(await res.arrayBuffer())], [...PNG]);
  assert.equal(calls[0].url, "https://tile.openstreetmap.org/3/4/2.png");
  assert.match(calls[0].init.headers["user-agent"], /^KAHF-live-map\//);
  const again = await handleRequest(new Request(`${BASE}/tiles/3/4/2.png`), fakeStore(), NOW, options);
  assert.equal(again.status, 200);
  assert.equal(calls.length, 1, "second request comes from the cache");
});

test("tile requests outside the grid or the allowed shape are refused without calling out", async () => {
  const calls = [];
  const options = tileOptions(calls, okTile);
  for (const path of ["/tiles/20/0/0.png", "/tiles/2/4/0.png", "/tiles/2/0/4.png", "/tiles/1/0/0.jpg", "/tiles/a/b/c.png", "/tiles/../x.png"]) {
    const res = await handleRequest(new Request(`${BASE}${path}`), fakeStore(), NOW, options);
    assert.ok([404].includes(res.status), `${path} gave ${res.status}`);
  }
  const post = await handleRequest(new Request(`${BASE}/tiles/1/0/0.png`, { method: "POST" }), fakeStore(), NOW, options);
  assert.equal(post.status, 405);
  assert.equal(calls.length, 0);
});

test("a failing, wrong-type or oversize upstream tile gives a plain 502", async () => {
  for (const reply of [
    () => new Response("no", { status: 403, headers: { "content-type": "text/plain" } }),
    () => new Response("<html>", { headers: { "content-type": "text/html" } }),
    () => new Response(new Uint8Array(300_000), { headers: { "content-type": "image/png" } }),
    () => {
      throw new Error("down");
    },
  ]) {
    const options = tileOptions([], reply);
    const res = await handleRequest(new Request(`${BASE}/tiles/1/0/0.png`), fakeStore(), NOW, options);
    assert.equal(res.status, 502);
    assert.deepEqual(await res.json(), { error: "tile unavailable" });
  }
});
