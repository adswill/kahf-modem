// Request routing for the station map Worker. Pure apart from the store it is
// given: index.ts passes the Workers KV store, and the tests pass a fake one.
//
// Routes:
//   GET  /           home page (renderHome)
//   GET  /map        live map (renderMap)
//   GET  /operate.png  the screenshot on the home page
//   GET  /stations   current station records as JSON
//   GET  /zones      busy channels per area (busyZones) and channel pools
//   POST /report     signed reception report
//   POST /remove     signed removal by the key that made the report
//   POST /register   signed relay registration (callsign, area, key)
//   POST /schedule   signed slot request from a registered relay (planSchedule)
//   GET  /map-data   relays and live stations as JSON (buildMapData)
//   GET  /release    the current release announcement, or null
//   POST /release    signed release announcement from the owner's release key
//   GET  /mesh/ws, POST /mesh/frames, /mesh/poll  signed relay mesh (mesh.js)
//   GET  /relay/wanted  destination callsigns that have queued messages
import {
  BUSY_WINDOW_S,
  EXPIRY_MS,
  MAX_BODY_BYTES,
  buildMapData,
  busyZones,
  compareVersions,
  decodeBase64,
  encodeBase64,
  isExpired,
  normalizeCall,
  poolPlan,
  validateRegister,
  validateRelease,
  validateRemove,
  validateReport,
  validateSchedule,
  verifySignature,
  zoneReports,
} from "./core.js";
import { FORBIDDEN_KEYS } from "./hub.js";
import { OPERATE_PNG_BASE64 } from "./web/operate-png.js";
import { homeScriptSha256, mapScriptSha256, renderHome, renderMap } from "./page.ts";

const HOME_CSP =
  "default-src 'none'; style-src 'unsafe-inline'; img-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/**
 * The live map's policy: its one hashed inline script; MapLibre from unpkg (pinned,
 * with integrity hashes on the tags); and map tiles from this server's own /tiles proxy.
 * Nothing else.
 */
function mapCsp(scriptSource) {
  return [
    "default-src 'none'",
    `script-src ${scriptSource} https://unpkg.com`,
    "style-src 'unsafe-inline' https://unpkg.com",
    "img-src 'self' data: blob:",
    "connect-src 'self'",
    "worker-src blob:",
    "base-uri 'none'",
    "form-action 'none'",
    "frame-ancestors 'none'",
  ].join("; ");
}

/**
 * The home page policy: the one hashed inline script, and connect-src 'self' so
 * that script can call GET /map-data and GET /release on this server only.
 */
function homeCsp(scriptSource) {
  return HOME_CSP.replace("img-src 'none'", `script-src ${scriptSource}; img-src 'self'`).replace(
    "base-uri",
    "connect-src 'self'; base-uri",
  );
}

/**
 * @param {unknown} body
 * @param {number} [status]
 * @param {Record<string, string>} [extra]
 * @returns {Response}
 */
function json(body, status = 200, extra = {}) {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      "content-type": "application/json; charset=utf-8",
      "cache-control": "no-store",
      "x-content-type-options": "nosniff",
      ...extra,
    },
  });
}

/**
 * @param {string} body
 * @param {string} csp the Content-Security-Policy value for this page
 * @returns {Response}
 */
function html(body, csp) {
  return new Response(body, {
    headers: {
      "content-type": "text/html; charset=utf-8",
      "cache-control": "no-store",
      "content-security-policy": csp,
      "x-content-type-options": "nosniff",
      "referrer-policy": "no-referrer",
    },
  });
}

/**
 * Reads a JSON body under MAX_BODY_BYTES.
 * @param {Request} request
 * @returns {Promise<{ ok: true, body: unknown } | { ok: false, response: Response }>}
 */
async function readJson(request, cap = MAX_BODY_BYTES) {
  const declared = Number(request.headers.get("content-length") ?? "0");
  if (declared > cap) {
    return { ok: false, response: json({ error: "body too large" }, 413) };
  }
  const text = await request.text();
  if (new TextEncoder().encode(text).length > cap) {
    return { ok: false, response: json({ error: "body too large" }, 413) };
  }
  try {
    return { ok: true, body: JSON.parse(text) };
  } catch {
    return { ok: false, response: json({ error: "body is not valid JSON" }, 400) };
  }
}

/**
 * @param {import("./types").StationRecord} r
 * @returns {import("./types").PublicStation}
 */
function toPublic(r) {
  return {
    reporter_call: r.reporter_call,
    reporter_location: r.reporter_location,
    heard_call: r.heard_call,
    heard_location: r.heard_location,
    heard_at: r.heard_at,
    frequency_khz: r.frequency_khz,
    mode: r.mode,
    refreshed_at: new Date(r.refreshed_at).toISOString(),
  };
}

/** Non-expired records, as stored (no public keys stripped yet). */
async function liveRecords(store, nowMs) {
  return (await store.listAll()).filter((r) => !isExpired(r.refreshed_at, nowMs));
}

async function currentStations(store, nowMs) {
  return (await liveRecords(store, nowMs))
    .map(toPublic)
    .sort((a, b) => a.reporter_call.localeCompare(b.reporter_call) || a.heard_call.localeCompare(b.heard_call));
}

async function getHome() {
  return html(renderHome(), homeCsp(await homeScriptSha256()));
}

let operatePng = null;

/** The Operate screenshot on the home page. Static, so it may be cached. */
function getOperatePng() {
  if (!operatePng) {
    const binary = atob(OPERATE_PNG_BASE64);
    operatePng = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) operatePng[i] = binary.charCodeAt(i);
  }
  return new Response(operatePng, {
    headers: {
      "content-type": "image/png",
      "cache-control": "public, max-age=3600",
      "x-content-type-options": "nosniff",
      "content-security-policy": "default-src 'none'",
    },
  });
}

/** Relays that registered and are still live. Stores without a relay index show none. */
async function liveRelays(store, nowMs) {
  if (typeof store.listRelays !== "function") return [];
  return (await store.listRelays()).filter((r) => !isExpired(r.refreshed_at, nowMs));
}

async function getMap({ store, nowMs }) {
  const stations = await currentStations(store, nowMs);
  const relays = buildMapData(await liveRelays(store, nowMs), [], nowMs).relays;
  const page = renderMap(stations, new Date(nowMs).toISOString(), relays);
  return html(page, mapCsp(await mapScriptSha256()));
}

async function getMapData({ store, nowMs }) {
  const stations = await currentStations(store, nowMs);
  const relays = await liveRelays(store, nowMs);
  return json(buildMapData(relays, stations, nowMs));
}

async function getRelease({ store, nowMs }) {
  const current = typeof store.getRelease === "function" ? await store.getRelease() : null;
  return json({ generated_at: new Date(nowMs).toISOString(), release: current ?? null });
}

/**
 * Stores a signed release announcement. Only the owner's release key (set as
 * RELEASE_PUBLIC_KEY, never in the code) may announce. An announcement for an
 * older version than the current one is refused, so an old signed message
 * cannot roll the announcement back.
 */
async function postRelease({ request, store, nowMs, options }) {
  const configured = typeof options.releaseKey === "string" ? decodeBase64(options.releaseKey, 32) : null;
  if (!configured || typeof store.putRelease !== "function") {
    return json({ error: "release announcements are not set up" }, 503);
  }
  const parsed = await readJson(request);
  if (!parsed.ok) return parsed.response;

  const check = validateRelease(parsed.body);
  if (!check.ok) return json({ error: check.error }, 400);
  const { record, publicKey, signature, message } = check;

  if (publicKey !== encodeBase64(configured)) return json({ error: "not the release key" }, 403);
  if (!(await verifySignature(publicKey, signature, message))) {
    return json({ error: "signature check failed" }, 401);
  }

  const current = typeof store.getRelease === "function" ? await store.getRelease() : null;
  if (current && compareVersions(record.version, current.version) < 0) {
    return json({ error: "a newer release is already announced" }, 409);
  }
  await store.putRelease(record);
  return json({ ok: true, release: record });
}

async function getStations({ store, nowMs }) {
  return json({
    generated_at: new Date(nowMs).toISOString(),
    expires_after_hours: EXPIRY_MS / 3_600_000,
    stations: await currentStations(store, nowMs),
  });
}

async function getZones({ store, nowMs }) {
  const zones = busyZones(zoneReports(await liveRecords(store, nowMs)), Math.floor(nowMs / 1000));
  return json({
    generated_at: new Date(nowMs).toISOString(),
    busy_window_s: BUSY_WINDOW_S,
    zones,
    pools: poolPlan(zones),
  });
}

async function postReport({ request, store, nowMs }) {
  const parsed = await readJson(request);
  if (!parsed.ok) return parsed.response;

  const check = validateReport(parsed.body, nowMs);
  if (!check.ok) return json({ error: check.error }, 400);
  const { record, publicKey, signature, message } = check;

  if (!(await verifySignature(publicKey, signature, message))) {
    return json({ error: "signature check failed" }, 401);
  }

  // A reporter callsign belongs to the first key that reports it, until
  // that key goes silent for 48 h. This stops another key from posting
  // reports under someone else's callsign. The claim is checked and written
  // atomically; a refused claim writes nothing.
  const claim = await store.claimOwner(record.reporter_call, publicKey, nowMs);
  if (!claim.ok) return json({ error: claim.error }, 409);

  await store.put({ ...record, public_key: publicKey, refreshed_at: nowMs });
  return json({ ok: true, expires_after_s: EXPIRY_MS / 1000 });
}

async function postRemove({ request, store, nowMs }) {
  const parsed = await readJson(request);
  if (!parsed.ok) return parsed.response;

  const check = validateRemove(parsed.body, nowMs);
  if (!check.ok) return json({ error: check.error }, 400);
  const { reporterCall, heardCall, signedAtMs, publicKey, signature, message } = check;

  if (!(await verifySignature(publicKey, signature, message))) {
    return json({ error: "signature check failed" }, 401);
  }

  const existing = await store.get(reporterCall, heardCall);
  if (!existing || isExpired(existing.refreshed_at, nowMs)) {
    return json({ error: "no such record" }, 404);
  }
  if (existing.public_key !== publicKey) {
    return json({ error: "this record belongs to another key" }, 403);
  }

  // A removal can be used once: the store remembers its signing time per
  // record and refuses the same or an older one, in the same atomic step.
  const claim = await store.claimRemoval(reporterCall, heardCall, publicKey, signedAtMs, nowMs);
  if (!claim.ok) return json({ error: claim.error }, 409);
  await store.remove(reporterCall, heardCall);
  return json({ ok: true, removed: true });
}

async function postRegister({ request, store, nowMs }) {
  const parsed = await readJson(request);
  if (!parsed.ok) return parsed.response;

  const check = validateRegister(parsed.body);
  if (!check.ok) return json({ error: check.error }, 400);
  const { callsign, area, publicKey, signature, message } = check;

  if (!(await verifySignature(publicKey, signature, message))) {
    return json({ error: "signature check failed" }, 401);
  }

  // Checked and written atomically in the callsign's object.
  const claim = await store.claimRelay(callsign, area, publicKey, nowMs);
  if (!claim.ok) return json({ error: claim.error }, 409);
  if (claim.previousArea) await store.leaveArea(claim.previousArea, callsign);
  await store.joinArea(area, callsign, nowMs);
  // The relay index lets the map list relays. Stores without one skip it.
  if (typeof store.putRelayIndex === "function") await store.putRelayIndex(callsign, area, nowMs);
  return json({ ok: true, callsign, area, expires_after_s: EXPIRY_MS / 1000 });
}

async function postSchedule({ request, store, nowMs }) {
  const parsed = await readJson(request);
  if (!parsed.ok) return parsed.response;

  const check = validateSchedule(parsed.body);
  if (!check.ok) return json({ error: check.error }, 400);
  const { relay, requests, publicKey, signature, message } = check;

  if (!(await verifySignature(publicKey, signature, message))) {
    return json({ error: "signature check failed" }, 401);
  }

  const me = await store.getRelay(relay);
  if (!me || isExpired(me.refreshed_at, nowMs)) {
    return json({ error: "relay is not registered" }, 404);
  }
  if (me.public_key !== publicKey) {
    return json({ error: "this callsign is registered with another key" }, 403);
  }

  // The area's object plans this relay's requests against the slots already
  // booked by the other live relays there, and stores the result, in one step.
  // This relay's earlier plan is replaced.
  const own = requests.map((q) => ({
    relay,
    channel: q.channel,
    duration: q.duration,
    direction: q.direction,
    urgent: q.urgent,
    latency_budget_s: q.latency_budget_s,
  }));
  const plan = await store.bookSchedule(me.area, relay, own, nowMs);
  if (!plan.ok) return json({ error: plan.error }, 400);
  return json({
    relay,
    generated_at: new Date(nowMs).toISOString(),
    slots: plan.slots,
    unplaced: plan.unplaced ?? [],
  });
}

// ---- the relay mesh: signed WebSocket and HTTPS fallback (mesh.js) -----------------------------

const MESH_TS_WINDOW_S = 600;
const MESH_MAX_BODY_BYTES = 16_384;
const MESH_MAX_FRAMES = 50;

/**
 * Checks who is calling: a plausible callsign, a signing time within ten minutes, a valid Ed25519
 * signature over `mesh1|CALL|ts|kind[|extra]`, and a live registration with that very key.
 * @returns {Promise<{ ok: true, call: string } | { ok: false, response: Response }>}
 */
async function meshAuth({ store, nowMs }, who, kind, extra = "") {
  const call = normalizeCall(who.relay);
  if (!call) return { ok: false, response: json({ error: "relay is not a valid callsign" }, 400) };
  const ts = Number(who.ts);
  if (!Number.isSafeInteger(ts) || Math.abs(Math.floor(nowMs / 1000) - ts) > MESH_TS_WINDOW_S) {
    return { ok: false, response: json({ error: "ts is not within ten minutes of the server clock" }, 400) };
  }
  const canonical = `mesh1|${call}|${ts}|${kind}${extra ? "|" + extra : ""}`;
  if (!(await verifySignature(String(who.key ?? ""), String(who.sig ?? ""), canonical))) {
    return { ok: false, response: json({ error: "signature check failed" }, 401) };
  }
  const me = await store.getRelay(call);
  if (!me || isExpired(me.refreshed_at, nowMs)) return { ok: false, response: json({ error: "relay is not registered" }, 404) };
  if (me.public_key !== who.key) return { ok: false, response: json({ error: "this callsign is registered with another key" }, 403) };
  return { ok: true, call };
}

async function getMeshWs(ctx) {
  const { request, store } = ctx;
  if (request.headers.get("upgrade") !== "websocket") return json({ error: "this is a WebSocket endpoint" }, 426);
  const q = new URL(request.url).searchParams;
  const auth = await meshAuth(ctx, { relay: q.get("relay"), ts: q.get("ts"), key: q.get("key"), sig: q.get("sig") }, "ws");
  if (!auth.ok) return auth.response;
  return store.meshSocket(auth.call, request);
}

async function postMeshFrames(ctx) {
  const parsed = await readJson(ctx.request, MESH_MAX_BODY_BYTES);
  if (!parsed.ok) return parsed.response ?? json({ error: parsed.error ?? "bad request" }, 400);
  const b = parsed.body;
  if (typeof b?.frames_json !== "string") return json({ error: "frames_json must be a string" }, 400);
  const auth = await meshAuth(ctx, { relay: b.relay, ts: b.ts, key: b.public_key, sig: b.signature }, "frames", b.frames_json);
  if (!auth.ok) return auth.response;
  let frames;
  try { frames = JSON.parse(b.frames_json); } catch { frames = null; }
  if (!Array.isArray(frames) || frames.length > MESH_MAX_FRAMES || frames.some((f) => !f || typeof f.t !== "string")) {
    return json({ error: `frames must be a list of up to ${MESH_MAX_FRAMES} objects with a type` }, 400);
  }
  if (frames.some((f) => Object.keys(f).some((k) => FORBIDDEN_KEYS.includes(k)))) {
    return json({ error: "frames carry ids and sizes only, never message content" }, 400);
  }
  const out = await ctx.store.meshOp("frames", { call: auth.call, frames, nowMs: ctx.nowMs });
  return out.error ? json(out, 429, { "retry-after": "60" }) : json(out);
}

async function postMeshPoll(ctx) {
  const parsed = await readJson(ctx.request, MESH_MAX_BODY_BYTES);
  if (!parsed.ok) return parsed.response ?? json({ error: parsed.error ?? "bad request" }, 400);
  const b = parsed.body;
  const auth = await meshAuth(ctx, { relay: b?.relay, ts: b?.ts, key: b?.public_key, sig: b?.signature }, "poll");
  if (!auth.ok) return auth.response;
  return json(await ctx.store.meshOp("poll", { call: auth.call, nowMs: ctx.nowMs }));
}

const TILE_PATH = /^\/tiles\/(\d{1,2})\/(\d{1,7})\/(\d{1,7})\.png$/;
const TILE_UPSTREAM = "https://tile.openstreetmap.org";
/** OpenStreetMap's tile policy asks for a User-Agent that names the app. */
const TILE_USER_AGENT = "KAHF-live-map/0.1 (+https://kahf.dev)";
const TILE_CACHE_SECONDS = 7 * 24 * 3600;
const TILE_MAX_BYTES = 200_000;

/**
 * GET /tiles/{z}/{x}/{y}.png: an OpenStreetMap tile fetched by this server, so browsers never
 * call tile.openstreetmap.org directly (it refuses that with 403). Answers are cached for a week.
 * @param {{ request: Request, options: { fetchImpl?: Function, tileCache?: { match: Function, put: Function } } }} ctx
 * @param {RegExpMatchArray} m
 */
async function getTile({ request, options }, m) {
  const [z, x, y] = [Number(m[1]), Number(m[2]), Number(m[3])];
  if (z > 19 || x >= 2 ** z || y >= 2 ** z) return json({ error: "not found" }, 404);
  const key = new Request(`${TILE_UPSTREAM}/${z}/${x}/${y}.png`);
  const cache = options.tileCache ?? (typeof caches !== "undefined" ? caches.default : undefined);
  const hit = cache ? await cache.match(key) : undefined;
  if (hit) return request.method === "HEAD" ? new Response(null, hit) : hit;
  const doFetch = options.fetchImpl ?? fetch;
  let up;
  try {
    up = await doFetch(key.url, { headers: { "user-agent": TILE_USER_AGENT, accept: "image/png" } });
  } catch {
    return json({ error: "tile unavailable" }, 502);
  }
  const type = up.headers.get("content-type") ?? "";
  if (!up.ok || !type.startsWith("image/png")) return json({ error: "tile unavailable" }, 502);
  const body = await up.arrayBuffer();
  if (body.byteLength > TILE_MAX_BYTES) return json({ error: "tile unavailable" }, 502);
  const res = new Response(request.method === "HEAD" ? null : body, {
    status: 200,
    headers: {
      "content-type": "image/png",
      "cache-control": `public, max-age=${TILE_CACHE_SECONDS}`,
      "x-content-type-options": "nosniff",
    },
  });
  if (cache) await cache.put(key, new Response(body, res));
  return res;
}


/** Path to method to handler. A path or method missing here gets 404 or 405. */
const ROUTES = {
  "/": { GET: getHome, HEAD: getHome },
  "/map": { GET: getMap, HEAD: getMap },
  "/operate.png": { GET: getOperatePng, HEAD: getOperatePng },
  "/map-data": { GET: getMapData },
  "/stations": { GET: getStations },
  "/zones": { GET: getZones },
  "/release": { GET: getRelease, POST: postRelease },
  "/report": { POST: postReport },
  "/remove": { POST: postRemove },
  "/register": { POST: postRegister },
  "/schedule": { POST: postSchedule },
  "/mesh/ws": { GET: getMeshWs },
  "/mesh/frames": { POST: postMeshFrames },
  "/mesh/poll": { POST: postMeshPoll },
};

/**
 * Answers one request. Never throws: an unexpected error gives a JSON 500
 * with no details, and the error goes to the log.
 * @param {Request} request
 * @param {{ get: Function, put: Function, remove: Function, listAll: Function, claimOwner: Function, claimRemoval: Function, claimRelay: Function, getRelay: Function, joinArea: Function, leaveArea: Function, bookSchedule: Function }} store
 * @param {number} [nowMs]
 * @param {{ releaseKey?: string, relayRateLimit?: number }} [options] relayRateLimit: backbone calls per relay per minute; releaseKey: base64 Ed25519 key allowed to post releases
 * @returns {Promise<Response>}
 */
export async function handleRequest(request, store, nowMs = Date.now(), options = {}) {
  try {
    const url = new URL(request.url);
    const tile = TILE_PATH.exec(url.pathname);
    if (tile) {
      if (request.method !== "GET" && request.method !== "HEAD") {
        return json({ error: "method not allowed" }, 405, { allow: "GET, HEAD" });
      }
      return await getTile({ request, options }, tile);
    }
    if (!Object.hasOwn(ROUTES, url.pathname)) return json({ error: "not found" }, 404);
    const methods = ROUTES[url.pathname];
    if (!Object.hasOwn(methods, request.method)) {
      return json({ error: "method not allowed" }, 405, { allow: Object.keys(methods).join(", ") });
    }
    return await methods[request.method]({ request, store, nowMs, options });
  } catch (err) {
    console.error("request failed", err instanceof Error ? err.name : "error");
    return json({ error: "internal error" }, 500);
  }
}
