// Fake-station simulator for the kahf.dev Worker. Drives handleRequest in-process
// with an in-memory store and a fake clock. Fake relays and stations sign and send
// registrations, reception reports, schedule requests and removals, then each
// check prints PASS or FAIL. No network, no wrangler.
// Run: node tools/simulate.mjs [--stations=40] [--relays=6] [--seed=1]
import { register } from "node:module";
import { pathToFileURL } from "node:url";
import { fakeStore } from "./fake-store.mjs";

// Node 20 cannot import .ts files; page.ts is read as JavaScript (same hook as the tests).
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
const { canonicalJson, encodeBase64, BAND_SEGMENT_KHZ, CHANNEL_WIDTH_KHZ } = await import("../src/core.js");

// Constants written out here on purpose, so the checks do not reuse the server's own values.
const START_MS = Date.parse("2026-11-17T12:00:00Z");
const TICK_MS = 60_000; // one reception round per simulated minute
const TICKS = 20; // 20 simulated minutes of reports
const EXPIRY_MS = 48 * 3_600_000;
const BUSY_WINDOW_S = 600;
// Band layout comes from the one segment constant; the channel count is worked out here again.
const LOW_KHZ = BAND_SEGMENT_KHZ.low;
const CHANNEL_KHZ = CHANNEL_WIDTH_KHZ;
const CHANNELS = Math.floor(((BAND_SEGMENT_KHZ.high - BAND_SEGMENT_KHZ.low) * 10) / Math.round(CHANNEL_WIDTH_KHZ * 10));
const SLOT_MIN = 1;
const SLOT_MAX = 300;
const BASE = "https://kahf.test";
const enc = new TextEncoder();

// ---- fake store and signing (same shape as test/routes.test.mjs) -----------

async function newKey() {
  const pair = await crypto.subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"]);
  const raw = new Uint8Array(await crypto.subtle.exportKey("raw", pair.publicKey));
  return { privateKey: pair.privateKey, publicKey: encodeBase64(raw) };
}

/** The body for `fields`, with public_key and a signature over canonical JSON of fields. */
async function signed(key, fields) {
  const sig = new Uint8Array(
    await crypto.subtle.sign({ name: "Ed25519" }, key.privateKey, enc.encode(canonicalJson(fields))),
  );
  return { ...fields, public_key: key.publicKey, signature: encodeBase64(sig) };
}

/** Sends a request through handleRequest at the simulated time. */
function makeClient(store, clock) {
  return async function call(method, path, body) {
    const init = { method };
    if (body !== undefined) {
      init.headers = { "content-type": "application/json" };
      init.body = JSON.stringify(body);
    }
    const res = await handleRequest(new Request(BASE + path, init), store, clock.ms);
    let json = null;
    try { json = await res.json(); } catch { json = null; }
    return { status: res.status, body: json };
  };
}

// ---- helpers ---------------------------------------------------------------

function mulberry32(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** `len` capital letters that encode the number n (base 26). */
function letters(n, len) {
  let s = "";
  for (let k = 0; k < len; k++) {
    s = String.fromCharCode(65 + (n % 26)) + s;
    n = Math.floor(n / 26);
  }
  return s;
}

const round4 = (x) => Math.round(x * 1e4) / 1e4;

/** Area name such as "N51W1": the one-degree square holding the point. */
function areaName(lat, lon) {
  return `${lat >= 0 ? "N" : "S"}${Math.abs(Math.floor(lat))}${lon >= 0 ? "E" : "W"}${Math.abs(Math.floor(lon))}`;
}

/** Integer kHz inside channel c: 1 kHz above the channel start, always in the same channel. */
const freqOf = (c) => LOW_KHZ + 1 + Math.floor(CHANNEL_KHZ * c);

/** Heard-station callsign for channel c (fixed per channel). */
const heardCall = (c) => `R9${letters(c, 2)}`;

/** Busy channels per area from the reports whose age is inside the busy window. */
function expectedZones(log, nowS, removed = new Set()) {
  const sets = new Map();
  for (const e of log) {
    if (removed.has(e.reporter)) continue;
    const age = nowS - e.time;
    if (age < 0 || age >= BUSY_WINDOW_S) continue;
    if (!sets.has(e.area)) sets.set(e.area, new Set());
    sets.get(e.area).add(e.channel);
  }
  const out = {};
  for (const area of [...sets.keys()].sort()) out[area] = [...sets.get(area)].sort((a, b) => a - b);
  return out;
}

// ---- the simulation --------------------------------------------------------

/**
 * Runs the whole scenario and returns the results. Never prints.
 * @param {{ stations?: number, relays?: number, seed?: number }} [options]
 */
export async function runSimulation({ stations = 40, relays = 6, seed = 1 } = {}) {
  if (!Number.isInteger(stations) || stations < 1 || !Number.isInteger(relays) || relays < 1) {
    throw new RangeError("stations and relays must be whole numbers of at least 1");
  }
  const rnd = mulberry32(seed);
  const pick = (n) => Math.floor(rnd() * n);
  const clock = { ms: START_MS };
  const store = fakeStore();
  const call = makeClient(store, clock);

  /** Each check gets a recorder: rec(ok, detail). The first failure detail is kept. */
  const checks = [];
  const check = (name) => {
    const c = { name, total: 0, failed: 0, first: null };
    checks.push(c);
    return (ok, detail) => {
      c.total++;
      if (!ok) {
        c.failed++;
        if (c.first === null) c.first = detail;
      }
    };
  };

  // Regions: one cell per one-degree square, with some busy channels in each cell.
  const cellCount = Math.max(3, Math.ceil(relays / 2));
  const cells = Array.from({ length: cellCount }, (_, k) => ({ lat: 50 + k, lon: -3 + (k % 5) }));
  const busy = cells.map(() => {
    const chans = new Set();
    // Up to 4 busy channels per cell, fewer when the band has fewer channels.
    while (chans.size < Math.min(4, CHANNELS)) chans.add(pick(CHANNELS));
    // Silent after `until` ticks. The first channel of each cell stays busy to the end,
    // so every area has busy channels at the final check; the others may go quiet early.
    return [...chans].map((channel, idx) => ({ channel, until: idx === 0 ? TICKS : 1 + pick(TICKS) }));
  });

  // Stations: a precise or area location inside their cell.
  const stationsList = [];
  for (let i = 0; i < stations; i++) {
    const cell = i % cellCount;
    const lat = round4(cells[cell].lat + 0.05 + rnd() * 0.9);
    const lon = round4(cells[cell].lon + 0.05 + rnd() * 0.9);
    const location = i % 3 === 0
      ? { mode: "area", latitude: lat, longitude: lon, radius_km: 1 + pick(100) }
      : { mode: "precise", latitude: lat, longitude: lon };
    stationsList.push({
      kind: "station", call: `G7${letters(i, 3)}`, cell, lat, lon, location,
      area: areaName(lat, lon), key: await newKey(),
    });
  }

  // Relays: two per cell, so relays 0 and 1 share an area, as do 2 and 3, and so on.
  const relayList = [];
  for (let j = 0; j < relays; j++) {
    const cell = Math.floor(j / 2);
    const lat = cells[cell].lat + 0.5;
    const lon = cells[cell].lon + 0.5;
    relayList.push({
      kind: "relay", call: `W0${letters(j, 3)}`, cell, lat, lon,
      location: { mode: "precise", latitude: lat, longitude: lon },
      area: areaName(lat, lon), key: await newKey(),
    });
  }
  const reporters = [...stationsList, ...relayList];

  // 1. Relays register.
  const recRegister = check("relays registered (POST /register)");
  for (const r of relayList) {
    const res = await call("POST", "/register", await signed(r.key, { area: r.area, callsign: r.call }));
    recRegister(res.status === 200 && res.body?.ok === true, `${r.call}: HTTP ${res.status} ${JSON.stringify(res.body)}`);
  }

  // 2. Stations and relays send reception reports, one round per simulated minute.
  const recReport = check("reception reports accepted (POST /report)");
  const log = [];
  for (let t = 0; t < TICKS; t++) {
    clock.ms = START_MS + t * TICK_MS;
    for (const who of reporters) {
      for (const b of busy[who.cell]) {
        if (t >= b.until) continue;
        const res = await call("POST", "/report", await signed(who.key, {
          reporter_call: who.call,
          reporter_location: who.location,
          heard_call: heardCall(b.channel),
          heard_location: { mode: "precise", latitude: who.lat, longitude: who.lon },
          heard_at: new Date(clock.ms).toISOString(),
          frequency_khz: freqOf(b.channel),
          mode: "KAHF",
        }));
        recReport(res.status === 200, `${who.call} channel ${b.channel} tick ${t}: HTTP ${res.status} ${JSON.stringify(res.body)}`);
        log.push({ reporter: who.call, area: who.area, channel: b.channel, time: clock.ms / 1000 });
      }
    }
  }
  const endMs = clock.ms;
  const endS = endMs / 1000;

  // 3. Busy zones match what was reported (one check unit per area).
  const recZones = check("busy zones match reports (GET /zones)");
  const zonesRes = await call("GET", "/zones");
  const actualZones = zonesRes.body?.zones ?? {};
  const wantZones = expectedZones(log, endS);
  for (const area of [...new Set([...Object.keys(actualZones), ...Object.keys(wantZones)])].sort()) {
    const got = JSON.stringify(actualZones[area] ?? null);
    const want = JSON.stringify(wantZones[area] ?? null);
    recZones(got === want, `area ${area}: expected ${want}, server ${got}`);
  }

  // 4. Schedule requests from every relay. Co-located relays ask for the same busy channels.
  const recSched = check("schedule answers match the request (POST /schedule)");
  const slots = [];
  for (const [j, r] of relayList.entries()) {
    const chans = busy[r.cell].map((b) => b.channel);
    const requests = [0, 1, 2].map((k) => {
      const q = { channel: chans[k % chans.length], duration: 1 + pick(SLOT_MAX), direction: rnd() < 0.5 ? "rx" : "tx" };
      if (k === 0 && j % 2 === 0) q.urgent = true;
      return q;
    });
    const res = await call("POST", "/schedule", await signed(r.key, { relay: r.call, requests }));
    const nowS = clock.ms / 1000;
    const want = requests.map((q) => `${q.channel}/${q.duration}/${q.direction}`).sort().join(" ");
    const got = Array.isArray(res.body?.slots)
      ? res.body.slots.map((s) => `${s.channel}/${s.duration}/${s.direction}`).sort().join(" ")
      : null;
    const startsOk = (res.body?.slots ?? []).every((s) => Number.isInteger(s.start) && s.start >= nowS);
    const ok = res.status === 200 && got === want && startsOk;
    recSched(ok, `${r.call}: HTTP ${res.status}, want [${want}], got [${got}], starts ok ${startsOk}`);
    if (res.status === 200 && Array.isArray(res.body?.slots)) {
      for (const s of res.body.slots) slots.push({ relay: r.call, area: r.area, channel: s.channel, start: s.start, end: s.start + s.duration });
    }
  }

  // 5. Duration and channel limits: out-of-range values are refused, limits are accepted.
  const recDur = check("duration and channel limits (POST /schedule)");
  const c0 = busy[relayList[0].cell][0].channel;
  for (const [duration, channel, expect] of [
    [SLOT_MIN - 1, c0, 400], [SLOT_MAX + 1, c0, 400], [SLOT_MIN, c0, 200], [SLOT_MAX, c0, 200], [10, CHANNELS, 400],
  ]) {
    const res = await call("POST", "/schedule", await signed(relayList[0].key, {
      relay: relayList[0].call, requests: [{ channel, duration, direction: "rx" }],
    }));
    const durationOk = expect !== 200 || res.body?.slots?.[0]?.duration === duration;
    recDur(res.status === expect && durationOk, `duration ${duration}, channel ${channel}: want HTTP ${expect}, got ${res.status}`);
  }

  // 6. Urgent requests are placed first (same channel, so the normal one must wait).
  const recUrg = check("urgent request placed first (probe)");
  const probe = await call("POST", "/schedule", await signed(relayList[0].key, {
    relay: relayList[0].call,
    requests: [
      { channel: c0, duration: 10, direction: "rx" },
      { channel: c0, duration: 20, direction: "rx", urgent: true },
    ],
  }));
  const nowS = clock.ms / 1000;
  const urgentSlot = probe.body?.slots?.find((s) => s.duration === 20);
  const normalSlot = probe.body?.slots?.find((s) => s.duration === 10);
  const urgOk = urgentSlot && normalSlot && urgentSlot.start === nowS && normalSlot.start >= urgentSlot.start + 20;
  recUrg(Boolean(urgOk), `urgent start ${urgentSlot?.start}, normal start ${normalSlot?.start}, now ${nowS}`);

  // 7. Two relays in the same area must not share a channel in overlapping slots.
  const recOver = check("no relay overlap in same area and channel");
  for (let i = 0; i < slots.length; i++) {
    for (let k = i + 1; k < slots.length; k++) {
      const a = slots[i];
      const b = slots[k];
      if (a.relay === b.relay || a.area !== b.area || a.channel !== b.channel) continue;
      const overlap = a.start < b.end && b.start < a.end;
      recOver(!overlap, `${a.relay} and ${b.relay} in ${a.area} on channel ${a.channel}: [${a.start}, ${a.end}) and [${b.start}, ${b.end})`);
    }
  }

  // 8. Some stations are removed by signed request.
  const shuffled = [...stationsList];
  for (let i = shuffled.length - 1; i > 0; i--) {
    const k = pick(i + 1);
    [shuffled[i], shuffled[k]] = [shuffled[k], shuffled[i]];
  }
  const removedList = shuffled.slice(0, Math.max(1, Math.floor(stations / 8)));
  const removed = new Set(removedList.map((s) => s.call));
  const recRemoveOk = check("removal requests accepted (POST /remove)");
  for (const s of removedList) {
    for (const b of busy[s.cell]) {
      const res = await call("POST", "/remove", await signed(s.key, {
        reporter_call: s.call, heard_call: heardCall(b.channel), signed_at: new Date(clock.ms).toISOString().replace(/\.\d{3}Z$/, "Z"),
      }));
      recRemoveOk(res.status === 200, `${s.call} channel ${b.channel}: HTTP ${res.status} ${JSON.stringify(res.body)}`);
    }
  }

  // 9. After removal: removed stations are gone, the others remain, zones drop their reports.
  const recGone = check("removed stations gone, others remain (GET /stations)");
  const stationsRes = await call("GET", "/stations");
  const got = new Set((stationsRes.body?.stations ?? []).map((s) => `${s.reporter_call}|${s.heard_call}`));
  const want = new Set();
  for (const who of reporters) {
    if (removed.has(who.call)) continue;
    for (const b of busy[who.cell]) want.add(`${who.call}|${heardCall(b.channel)}`);
  }
  for (const key of [...new Set([...got, ...want])].sort()) {
    recGone(got.has(key) === want.has(key), `${key}: listed ${got.has(key)}, expected ${want.has(key)}`);
  }
  const recZonesAfter = check("busy zones after removal (GET /zones)");
  const zonesAfter = (await call("GET", "/zones")).body?.zones ?? {};
  const wantAfter = expectedZones(log, endS, removed);
  for (const area of [...new Set([...Object.keys(zonesAfter), ...Object.keys(wantAfter)])].sort()) {
    const g = JSON.stringify(zonesAfter[area] ?? null);
    const w = JSON.stringify(wantAfter[area] ?? null);
    recZonesAfter(g === w, `area ${area}: expected ${w}, server ${g}`);
  }

  // 10. Records are live at 47 h and gone after 48 h.
  const recLive = check("records still live before 48 h");
  clock.ms = endMs + 47 * 3_600_000;
  const at47 = (await call("GET", "/stations")).body?.stations ?? [];
  recLive(at47.length === want.size, `at 47 h: ${at47.length} records, expected ${want.size}`);

  const recExpired = check("records gone after 48 h (stations, zones, relay)");
  clock.ms = endMs + EXPIRY_MS + 60_000;
  const at48 = (await call("GET", "/stations")).body?.stations ?? [];
  recExpired(at48.length === 0, `after 48 h: ${at48.length} station records still listed`);
  const zones48 = (await call("GET", "/zones")).body?.zones ?? {};
  recExpired(Object.keys(zones48).length === 0, `after 48 h: zones still listed for ${Object.keys(zones48).join(", ")}`);
  const relayAfter = await call("POST", "/schedule", await signed(relayList[0].key, {
    relay: relayList[0].call, requests: [{ channel: c0, duration: 5, direction: "rx" }],
  }));
  recExpired(relayAfter.status === 404, `after 48 h: relay schedule HTTP ${relayAfter.status}, expected 404`);

  const failed = checks.reduce((n, c) => n + c.failed, 0);
  return { stations, relays, seed, reports: log.length, checks, failed };
}

/** Prints the report, one line per check. */
export function printReport(result) {
  console.log(`KAHF server simulator: ${result.stations} stations, ${result.relays} relays, seed ${result.seed}, ${result.reports} reports`);
  for (const c of result.checks) {
    const status = c.failed === 0 ? "PASS" : "FAIL";
    console.log(`${status}  ${c.name}: ${c.total - c.failed}/${c.total} ok`);
    if (c.failed > 0) console.log(`      first failure: ${c.first}`);
  }
  const passed = result.checks.filter((c) => c.failed === 0).length;
  console.log(`${passed}/${result.checks.length} checks passed: ${result.failed === 0 ? "ALL PASS" : "FAILED"}`);
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const opts = {};
  for (const arg of process.argv.slice(2)) {
    const m = arg.match(/^--(stations|relays|seed)=(\d+)$/);
    if (m) opts[m[1]] = Number(m[2]);
  }
  const result = await runSimulation(opts);
  printReport(result);
  process.exitCode = result.failed === 0 ? 0 : 1;
}
