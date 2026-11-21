// Tests for plan section 5 additions: latency budgets in the planner, release
// announcements, relay index and map data. Uses the in-memory fake store from
// tools/ plus the extra store methods below (the fake itself is not edited).
// Run with: npm test (node --test). No wrangler, no network.
import { test } from "node:test";
import assert from "node:assert/strict";
import { register } from "node:module";
import { webcrypto } from "node:crypto";
import {
  areaCentre,
  buildMapData,
  canonicalJson,
  compareVersions,
  encodeBase64,
  makeSchedule,
  planSchedule,
  signedMessage,
  validateRelease,
  validateSchedule,
  MAX_LATENCY_BUDGET_S,
} from "../src/core.js";
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
const subtle = globalThis.crypto?.subtle ?? webcrypto.subtle;

async function newKey() {
  const pair = await subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"]);
  const raw = new Uint8Array(await subtle.exportKey("raw", pair.publicKey));
  return { privateKey: pair.privateKey, publicKey: encodeBase64(raw) };
}

/** Adds public_key and signature to fields, signed over canonical JSON. */
async function signed(key, fields) {
  const sig = new Uint8Array(await subtle.sign({ name: "Ed25519" }, key.privateKey, enc.encode(canonicalJson(fields))));
  return { ...fields, public_key: key.publicKey, signature: encodeBase64(sig) };
}

const post = (path, body) =>
  new Request(BASE + path, {
    method: "POST",
    headers: { "content-type": "application/json", "content-length": String(JSON.stringify(body).length) },
    body: JSON.stringify(body),
  });
const get = (path) => new Request(BASE + path, { method: "GET" });

/** The fake store plus the release and relay index methods the Worker store has. */
function store() {
  const base = fakeStore();
  let release = null;
  const index = new Map();
  return Object.assign(base, {
    async putRelayIndex(callsign, area, nowMs) { index.set(callsign, { callsign, area, refreshed_at: nowMs }); },
    async listRelays() { return [...index.values()].map((x) => structuredClone(x)); },
    async getRelease() { return release && structuredClone(release); },
    async putRelease(record) { release = structuredClone(record); },
  });
}

const sched = (relay, channel, duration, extra = {}) => ({ relay, channel, duration, direction: "tx", ...extra });
const RELAYS = [{ id: "K1AAA", area: "N51W1" }, { id: "K1BBB", area: "N51W1" }];

// ---- latency budgets (plan 5.4) -------------------------------------------

test("planner: a request that cannot start inside its budget is left unplaced and not booked", () => {
  const out = planSchedule(
    [{ id: "K1AAA", area: "N51W1" }],
    [sched("K1AAA", 1, 60), sched("K1AAA", 1, 60, { latency_budget_s: 0 })],
    NOW_S,
  );
  assert.equal(out.slots.K1AAA.length, 1);
  assert.equal(out.slots.K1AAA[0].start, NOW_S);
  assert.deepEqual(out.unplaced, [
    { index: 1, relay: "K1AAA", channel: 1, direction: "tx", reason: "no free time inside the latency budget" },
  ]);
});

test("planner: a budget large enough still places the request, later than now", () => {
  const out = planSchedule(
    [{ id: "K1AAA", area: "N51W1" }],
    [sched("K1AAA", 1, 60), sched("K1AAA", 1, 60, { latency_budget_s: 60 })],
    NOW_S,
  );
  assert.equal(out.unplaced.length, 0);
  assert.equal(out.slots.K1AAA[1].start, NOW_S + 60);
});

test("planner: no budget means no limit, as before", () => {
  const out = makeSchedule(RELAYS, [sched("K1AAA", 1, 300), sched("K1BBB", 1, 300)], NOW_S);
  assert.equal(out.K1BBB[0].start, NOW_S + 300);
});

test("planner: urgent requests are placed before others, so they win the channel", () => {
  const out = planSchedule(
    RELAYS,
    [sched("K1AAA", 1, 100, { latency_budget_s: 50 }), sched("K1BBB", 1, 100, { urgent: true, latency_budget_s: 0 })],
    NOW_S,
  );
  assert.equal(out.slots.K1BBB[0].start, NOW_S);
  assert.equal(out.unplaced.length, 1);
  assert.equal(out.unplaced[0].relay, "K1AAA");
});

test("planner: invalid latency budgets are refused", () => {
  for (const bad of [-1, 1.5, MAX_LATENCY_BUDGET_S + 1, "10", null]) {
    assert.throws(
      () => makeSchedule(RELAYS, [sched("K1AAA", 1, 10, { latency_budget_s: bad })], NOW_S),
      /latency_budget_s/,
      `budget ${String(bad)} should be refused`,
    );
  }
  assert.doesNotThrow(() => makeSchedule(RELAYS, [sched("K1AAA", 1, 10, { latency_budget_s: MAX_LATENCY_BUDGET_S })], NOW_S));
});

test("validateSchedule: latency_budget_s is an allowed request field", () => {
  // Wire requests carry no relay field: the relay is the signed top-level one.
  const wire = (extra) => ({ channel: 1, duration: 10, direction: "tx", ...extra });
  const ok = validateSchedule({
    public_key: encodeBase64(new Uint8Array(32).fill(7)),
    relay: "K1AAA",
    requests: [wire({ latency_budget_s: 30 })],
    signature: encodeBase64(new Uint8Array(64).fill(1)),
  });
  assert.equal(ok.ok, true);
  const bad = validateSchedule({
    public_key: encodeBase64(new Uint8Array(32).fill(7)),
    relay: "K1AAA",
    requests: [wire({ budget: 30 })],
    signature: encodeBase64(new Uint8Array(64).fill(1)),
  });
  assert.equal(bad.ok, false);
});

// ---- release announcements (plan 5.5) -------------------------------------

const RELEASE_FIELDS = {
  notes_url: "https://kahf.dev/releases/4-2-0",
  published_at: "2026-11-17T11:00:00Z",
  required: false,
  version: "4.2.0",
};

test("validateRelease: a well formed announcement is accepted", () => {
  const body = { ...RELEASE_FIELDS, public_key: encodeBase64(new Uint8Array(32).fill(9)), signature: encodeBase64(new Uint8Array(64).fill(2)) };
  const out = validateRelease(body);
  assert.equal(out.ok, true);
  assert.deepEqual(out.record, RELEASE_FIELDS);
});

test("validateRelease: refuses bad versions, links, flags and extra fields", () => {
  const base = { ...RELEASE_FIELDS, public_key: encodeBase64(new Uint8Array(32).fill(9)), signature: encodeBase64(new Uint8Array(64).fill(2)) };
  const cases = [
    { version: "4.2" },
    { version: "v4.2.0" },
    { notes_url: "http://kahf.dev/x" },
    { notes_url: "javascript:alert(1)" },
    { notes_url: "https://user:pw@kahf.dev/x" },
    { required: "yes" },
    { published_at: "2026-02-30T00:00:00Z" },
    { extra: 1 },
  ];
  for (const change of cases) {
    const out = validateRelease({ ...base, ...change });
    assert.equal(out.ok, false, `should refuse ${JSON.stringify(change)}`);
  }
});

test("compareVersions orders numbers, not text", () => {
  assert.ok(compareVersions("4.10.0", "4.9.0") > 0);
  assert.equal(compareVersions("4.2.0", "4.2.0"), 0);
  assert.ok(compareVersions("3.9.9", "4.0.0") < 0);
  assert.equal(compareVersions("x", "4.0.0"), null);
});

test("POST /release: refused when no release key is set, and GET shows nothing yet", async () => {
  const s = store();
  const key = await newKey();
  const body = await signed(key, RELEASE_FIELDS);
  const res = await handleRequest(post("/release", body), s, NOW, {});
  assert.equal(res.status, 503);
  const current = await (await handleRequest(get("/release"), s, NOW)).json();
  assert.equal(current.release, null);
});

test("POST /release: only the release key, with a valid signature, and no roll back", async () => {
  const s = store();
  const owner = await newKey();
  const other = await newKey();
  const options = { releaseKey: owner.publicKey };

  const stranger = await handleRequest(post("/release", await signed(other, RELEASE_FIELDS)), s, NOW, options);
  assert.equal(stranger.status, 403);

  const forged = await signed(owner, RELEASE_FIELDS);
  forged.version = "9.9.9";
  assert.equal((await handleRequest(post("/release", forged), s, NOW, options)).status, 401);

  const ok = await handleRequest(post("/release", await signed(owner, RELEASE_FIELDS)), s, NOW, options);
  assert.equal(ok.status, 200);
  const shown = await (await handleRequest(get("/release"), s, NOW)).json();
  assert.equal(shown.release.version, "4.2.0");
  assert.equal(shown.release.required, false);

  const older = { ...RELEASE_FIELDS, version: "4.1.9" };
  assert.equal((await handleRequest(post("/release", await signed(owner, older)), s, NOW, options)).status, 409);

  const newer = { ...RELEASE_FIELDS, version: "4.3.0", required: true };
  assert.equal((await handleRequest(post("/release", await signed(owner, newer)), s, NOW, options)).status, 200);
  assert.equal((await (await handleRequest(get("/release"), s, NOW)).json()).release.required, true);
});

test("POST /release: a bad release key setting refuses rather than accepting anyone", async () => {
  const s = store();
  const key = await newKey();
  const res = await handleRequest(post("/release", await signed(key, RELEASE_FIELDS)), s, NOW, { releaseKey: "not-a-key" });
  assert.equal(res.status, 503);
});

// ---- relay index and map data (plan 5.1, 5.2) -----------------------------

test("areaCentre: centre of a one-degree square, north and south, east and west", () => {
  assert.deepEqual(areaCentre("N51W1"), { latitude: 51.5, longitude: -0.5 });
  assert.deepEqual(areaCentre("S10E5"), { latitude: -9.5, longitude: 5.5 });
  assert.equal(areaCentre("51W1"), null);
});

test("buildMapData: live relays only, as area circles, with no keys", () => {
  const data = buildMapData(
    [
      { callsign: "K1BBB", area: "N51W1", refreshed_at: NOW },
      { callsign: "K1AAA", area: "S10E5", refreshed_at: NOW },
      { callsign: "OLD1", area: "N51W1", refreshed_at: NOW - 49 * 3600 * 1000 },
    ],
    [],
    NOW,
  );
  assert.deepEqual(data.relays.map((r) => r.callsign), ["K1AAA", "K1BBB"]);
  assert.equal(data.relays[1].location.mode, "area");
  assert.equal(data.relays[1].location.radius_km, 79);
  assert.doesNotMatch(JSON.stringify(data), /public_key/);
});

test("GET /map-data: a registered relay and a reported station appear; old ones drop out", async () => {
  const s = store();
  const relayKey = await newKey();
  const reporter = await newKey();
  await handleRequest(post("/register", await signed(relayKey, { area: "N51W1", callsign: "K1AAA" })), s, NOW);
  const report = {
    frequency_khz: 14074,
    heard_at: new Date(NOW).toISOString().replace(/\.\d{3}Z$/, "Z"),
    heard_call: "DL1XYZ",
    heard_location: { mode: "area", latitude: 48.8566, longitude: 2.3522, radius_km: 50 },
    mode: "FT8",
    reporter_call: "K1AAA",
    reporter_location: { mode: "precise", latitude: 51.5074, longitude: -0.1278 },
  };
  assert.equal((await handleRequest(post("/report", await signed(reporter, report)), s, NOW)).status, 200);

  const live = await (await handleRequest(get("/map-data"), s, NOW)).json();
  assert.equal(live.relays.length, 1);
  assert.equal(live.relays[0].callsign, "K1AAA");
  assert.equal(live.stations.length, 1);
  assert.doesNotMatch(JSON.stringify(live), /public_key/);

  const later = NOW + 49 * 3600 * 1000;
  const stale = await (await handleRequest(get("/map-data"), s, later)).json();
  assert.equal(stale.relays.length, 0);
  assert.equal(stale.stations.length, 0);
});

test("GET /map: a registered relay gets a panel and counts in the footer", async () => {
  const s = store();
  const relayKey = await newKey();
  await handleRequest(post("/register", await signed(relayKey, { area: "N51W1", callsign: "K1AAA" })), s, NOW);
  const html = await (await handleRequest(get("/map"), s, NOW)).text();
  assert.match(html, /1 relay, 0 stations reporting/);
  const feed = JSON.parse(await (await handleRequest(get("/map-data"), s, NOW)).text());
  assert.equal(feed.relays.length, 1);
  assert.equal(feed.relays[0].callsign, "K1AAA");
});

test("POST /schedule: budget misses come back in unplaced, not as slots", async () => {
  const s = store();
  const key = await newKey();
  await handleRequest(post("/register", await signed(key, { area: "N51W1", callsign: "K1AAA" })), s, NOW);
  const wire = (extra) => ({ channel: 1, duration: 60, direction: "tx", ...extra });
  const body = await signed(key, {
    relay: "K1AAA",
    requests: [wire({}), wire({ latency_budget_s: 0 })],
  });
  const res = await handleRequest(post("/schedule", body), s, NOW);
  assert.equal(res.status, 200);
  const out = await res.json();
  assert.equal(out.slots.length, 1);
  assert.equal(out.unplaced.length, 1);
  assert.equal(out.unplaced[0].index, 1);
});
