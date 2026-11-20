// Request routing for the station map Worker. Pure apart from the store it is
// given: index.ts passes the Workers KV store, and the tests pass a fake one.
//
// Routes:
//   GET  /           home page (renderHome)
//   GET  /map        live map (renderMap)
//   GET  /network, /start, /protocol, /legal   inner pages (renderSite)
//   GET  /operate.png  the screenshot on the home page
//   GET  /fonts/display.woff2  the heading typeface
//   GET  /stations   current station records as JSON
//   GET  /zones      busy channels per area (busyZones) and channel pools
//   POST /report     signed reception report
//   POST /remove     signed removal by the key that made the report
//   POST /register   signed relay registration (callsign, area, key)
//   POST /schedule   signed slot request from a registered relay (planSchedule)
//   GET  /map-data   relays and live stations as JSON (buildMapData)
//   GET  /release    the current release announcement, or null
//   POST /release    signed release announcement from the owner's release key
//   POST /relay/heard, /msg/submit, /msg/pull, /msg/receipt  signed HF backbone calls (backbone.js)
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
  poolPlan,
  validateRegister,
  validateRelease,
  validateRemove,
  validateReport,
  validateSchedule,
  verifySignature,
  zoneReports,
} from "./core.js";
import {
  MAX_BACKBONE_BODY_BYTES,
  MAX_PULL_MESSAGES,
  RELAY_RATE_PER_MINUTE,
  parseMessageId,
  validateHeard,
  validatePull,
  validateReceipt,
  validateSubmit,
} from "./backbone.js";
import { OPERATE_PNG_BASE64 } from "./web/operate-png.js";
import { DISPLAY_FONT_BASE64 } from "./web/display-font.js";
import { homeScriptSha256, mapScriptSha256, renderHome, renderMap, renderSite, SITE_PATHS } from "./page.ts";

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
    "img-src 'self' data: blob: https://tile.openstreetmap.org",
    "font-src 'self'",
    "connect-src 'self' https://tile.openstreetmap.org",
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
  return HOME_CSP.replace("img-src 'none'", `script-src ${scriptSource}; img-src 'self'; font-src 'self'`).replace(
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

/** Inner pages (/network, /start, /protocol, /legal): static HTML under the home page policy. */
const sitePage = (path) => async () => html(renderSite(path), homeCsp(await homeScriptSha256()));

let displayFont = null;

/** The display face used by the headings. Static, so it may be cached for a long time. */
function getDisplayFont() {
  if (!displayFont) {
    const binary = atob(DISPLAY_FONT_BASE64);
    displayFont = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) displayFont[i] = binary.charCodeAt(i);
  }
  return new Response(displayFont, {
    headers: {
      "content-type": "font/woff2",
      "cache-control": "public, max-age=604800, immutable",
      "x-content-type-options": "nosniff",
      "content-security-policy": "default-src 'none'",
    },
  });
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

// ---- HF backbone ------------------------------------------------------------

/**
 * Reads and checks one signed backbone request: size cap, shape (validate), signature,
 * a registered live relay with that key, and the per-relay rate limit.
 * @returns {Promise<{ ok: true, check: any } | { ok: false, response: Response }>}
 */
async function backboneAuth({ request, store, nowMs, options }, validate) {
  const parsed = await readJson(request, MAX_BACKBONE_BODY_BYTES);
  if (!parsed.ok) return parsed;
  const check = validate(parsed.body, nowMs);
  if (!check.ok) return { ok: false, response: json({ error: check.error }, 400) };
  if (!(await verifySignature(check.publicKey, check.signature, check.canonical))) {
    return { ok: false, response: json({ error: "signature check failed" }, 401) };
  }
  const me = await store.getRelay(check.relay);
  if (!me || isExpired(me.refreshed_at, nowMs)) {
    return { ok: false, response: json({ error: "relay is not registered" }, 404) };
  }
  if (me.public_key !== check.publicKey) {
    return { ok: false, response: json({ error: "this callsign is registered with another key" }, 403) };
  }
  const limit = options.relayRateLimit ?? RELAY_RATE_PER_MINUTE;
  if (!(await store.idxRate(check.relay, nowMs, limit)).ok) {
    return { ok: false, response: json({ error: "too many requests from this relay" }, 429, { "retry-after": "60" }) };
  }
  return { ok: true, check };
}

async function postRelayHeard(ctx) {
  const auth = await backboneAuth(ctx, validateHeard);
  if (!auth.ok) return auth.response;
  const { relay, heard } = auth.check;
  const done = await ctx.store.idxNoteHeard(relay, heard, ctx.nowMs);
  return json({ ok: true, recorded: done.recorded, ignored: done.ignored });
}

async function postMsgSubmit(ctx) {
  const auth = await backboneAuth(ctx, validateSubmit);
  if (!auth.ok) return auth.response;
  const { message } = auth.check;
  const { store, nowMs } = ctx;
  // The first submit of an id wins; the claim lives on the origin's object.
  const claim = await store.msgClaim(message.origin, message.id, message.dest, nowMs);
  if (claim.duplicate) return json({ ok: true, duplicate: true });
  const queued = await store.msgEnqueue(message.dest, { ...message, submitted_by: auth.check.relay }, nowMs);
  await store.idxWant(message.dest, queued.until);
  return json({ ok: true, duplicate: false, expires_at: Math.floor((nowMs + message.ttl_s * 1000) / 1000) });
}

async function postMsgPull(ctx) {
  const auth = await backboneAuth(ctx, validatePull);
  if (!auth.ok) return auth.response;
  const { relay, callsigns } = auth.check;
  const { store, nowMs } = ctx;

  // Look at every heard callsign's queue; expired messages turn into "failed" receipts.
  const peeks = await Promise.all(callsigns.map(async (dest) => ({ dest, peek: await store.msgPeek(dest, nowMs) })));
  for (const { dest, peek } of peeks) {
    for (const e of peek.expired) {
      await store.msgPutReceipt(e.origin, { id: e.id, dest: e.dest, status: "failed", reason: "expired" }, nowMs);
    }
    if (peek.remaining === 0) await store.idxUnwant(dest);
  }

  // Oldest first, urgent first; at most MAX_PULL_MESSAGES per call.
  const candidates = peeks.flatMap(({ dest, peek }) => peek.messages.map((m) => ({ dest, m })));
  candidates.sort((a, b) => Number(b.m.urgent === true) - Number(a.m.urgent === true) || a.m.created - b.m.created || (a.m.id < b.m.id ? -1 : 1));
  const chosen = candidates.slice(0, MAX_PULL_MESSAGES);
  const byDest = new Map();
  for (const { dest, m } of chosen) byDest.set(dest, [...(byDest.get(dest) ?? []), m.id]);
  const handed = new Map();
  for (const [dest, ids] of byDest) {
    for (const m of (await store.msgHandout(dest, ids, relay, nowMs)).messages) handed.set(m.id, m);
  }
  const messages = chosen
    .filter(({ m }) => handed.has(m.id))
    .map(({ m }) => ({
      id: m.id,
      origin: m.origin,
      dest: m.dest,
      kind: m.kind,
      ...(m.subject !== undefined ? { subject: m.subject } : {}),
      body: m.body,
      created: m.created,
      hops: m.hops,
      urgent: m.urgent === true,
      expires_at: Math.floor(m.expires_at / 1000),
    }));

  // Receipts for the heard callsigns acting as origin.
  const receipts = (await Promise.all(callsigns.map((c) => store.msgTakeReceipts(c, nowMs)))).flatMap((r) => r.receipts);
  return json({ messages, receipts });
}

async function postMsgReceipt(ctx) {
  const auth = await backboneAuth(ctx, validateReceipt);
  if (!auth.ok) return auth.response;
  const { relay, id, status, reason } = auth.check;
  const { store, nowMs } = ctx;
  const origin = parseMessageId(id).origin;
  const { claim } = await store.msgGetClaim(origin, id, nowMs);
  if (!claim) return json({ error: "no such message" }, 404);
  const done = await store.msgComplete(claim.dest, id, relay, nowMs);
  if (!done.ok) return json({ error: done.error }, 409);
  if (!done.found) return json({ ok: true, duplicate: true });
  await store.msgPutReceipt(origin, { id, dest: claim.dest, status, reason }, nowMs);
  if (done.remaining === 0) await store.idxUnwant(claim.dest);
  return json({ ok: true, duplicate: false });
}

async function getRelayWanted({ store, nowMs }) {
  return json({ wanted: (await store.idxWanted(nowMs)).wanted });
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
  ...Object.fromEntries(SITE_PATHS.map((p) => [p, { GET: sitePage(p), HEAD: sitePage(p) }])),
  "/operate.png": { GET: getOperatePng, HEAD: getOperatePng },
  "/fonts/display.woff2": { GET: getDisplayFont, HEAD: getDisplayFont },
  "/map-data": { GET: getMapData },
  "/stations": { GET: getStations },
  "/zones": { GET: getZones },
  "/release": { GET: getRelease, POST: postRelease },
  "/report": { POST: postReport },
  "/remove": { POST: postRemove },
  "/register": { POST: postRegister },
  "/schedule": { POST: postSchedule },
  "/relay/heard": { POST: postRelayHeard },
  "/relay/wanted": { GET: getRelayWanted },
  "/msg/submit": { POST: postMsgSubmit },
  "/msg/pull": { POST: postMsgPull },
  "/msg/receipt": { POST: postMsgReceipt },
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
