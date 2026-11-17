// Pure logic for the station map Worker.
//
// Plain JavaScript on purpose: `node --test` (Node 20) runs these files
// directly, with no TypeScript compiler or test dependency. The Worker
// (src/index.ts) imports them; Wrangler bundles them with esbuild.
//
// Nothing here touches storage or the network. Signature checks use
// crypto.subtle, which exists in Workers and in Node 20 as globalThis.crypto.

/** Records expire this long after their last refresh. */
export const EXPIRY_MS = 48 * 60 * 60 * 1000;

/** Largest request body accepted, in bytes. */
export const MAX_BODY_BYTES = 4096;

/** Area mode radius limits, in km (whole numbers). */
export const RADIUS_MIN_KM = 1;
export const RADIUS_MAX_KM = 100;

/** Coordinates may have at most this many decimal places (about 11 m). */
const MAX_DECIMALS = 4;

/** Largest frequency accepted, in kHz (1 THz). */
const MAX_FREQUENCY_KHZ = 1e9;

/** A heard_at this far in the future is rejected (clock skew allowance). */
const FUTURE_SKEW_MS = 5 * 60 * 1000;

/** Kilometres per degree of latitude (approximate). */
const KM_PER_DEG = 111.32;

const REPORT_KEYS = [
  "frequency_khz",
  "heard_at",
  "heard_call",
  "heard_location",
  "mode",
  "public_key",
  "reporter_call",
  "reporter_location",
  "signature",
];

const REMOVE_KEYS = ["heard_call", "public_key", "reporter_call", "signature", "signed_at"];

/** A report's heard_at may be at most this old (replay protection). */
export const MAX_REPORT_AGE_MS = 10 * 60 * 1000;

/** A removal's signed_at must be within this of the server clock, either way. */
export const REMOVE_WINDOW_MS = 5 * 60 * 1000;

/**
 * Public keys of small order (the 8 torsion points of Curve25519, with
 * non-canonical and sign-bit variants), as base64. Such keys let anyone
 * forge signatures and must never be accepted.
 */
const SMALL_ORDER_KEYS = new Set([
  "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
  "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAIA=",
  "AQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
  "AQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAIA=",
  "7P////////////////////////////////////////8=",
  "7P///////////////////////////////////////38=",
  "7f////////////////////////////////////////8=",
  "7f///////////////////////////////////////38=",
  "7v////////////////////////////////////////8=",
  "7v///////////////////////////////////////38=",
  "JuiVj8KyJ7BFw/SJ8u+Y8NXfrAXTxjM5sTgCiG1T/AU=",
  "JuiVj8KyJ7BFw/SJ8u+Y8NXfrAXTxjM5sTgCiG1T/IU=",
  "xxdqcD1N2E+6PAt2DRBnDyogU/osOczGTsf9d5KsA/o=",
  "xxdqcD1N2E+6PAt2DRBnDyogU/osOczGTsf9d5KsA3o=",
]);

/**
 * Decode a public key: 32 bytes, not a small-order point.
 * @param {unknown} value
 * @returns {Uint8Array | null}
 */
function decodePublicKey(value) {
  const bytes = decodeBase64(value, 32);
  if (!bytes || SMALL_ORDER_KEYS.has(encodeBase64(bytes))) return null;
  return bytes;
}

/**
 * Parse an ISO 8601 UTC time ending in Z. Returns milliseconds, or null when
 * the text is malformed or names a date that does not exist (2026-02-30).
 * @param {unknown} text
 * @returns {number | null}
 */
function parseIsoUtc(text) {
  if (typeof text !== "string" || !ISO_UTC_RE.test(text)) return null;
  const hh = Number(text.slice(11, 13));
  const mm = Number(text.slice(14, 16));
  const ss = Number(text.slice(17, 19));
  if (hh > 23 || mm > 59 || ss > 59) return null;
  const day = text.slice(0, 10);
  const d = new Date(`${day}T00:00:00.000Z`);
  if (!Number.isFinite(d.getTime()) || d.toISOString().slice(0, 10) !== day) return null;
  const ms = Date.parse(text);
  return Number.isFinite(ms) ? ms : null;
}

const PRECISE_KEYS = ["latitude", "longitude", "mode"];
const AREA_KEYS = ["latitude", "longitude", "mode", "radius_km"];

const CALL_RE = /^[A-Z0-9]{1,3}[0-9][A-Z0-9]{0,4}(\/[A-Z0-9]{1,4})?$/;
const MODE_RE = /^[A-Za-z0-9+._/-](?:[A-Za-z0-9 +._/-]{0,22}[A-Za-z0-9+._/-])?$/;
const ISO_UTC_RE = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d{1,3})?Z$/;
const BASE64_RE = /^[A-Za-z0-9+/]+={0,2}$/;

/**
 * Normalise a callsign to upper case. Returns null if it is not a
 * plausible amateur callsign (letters, digits, optional /suffix).
 * @param {unknown} value
 * @returns {string | null}
 */
export function normalizeCall(value) {
  if (typeof value !== "string" || /[^\x00-\x7f]/.test(value)) return null;
  const upper = value.toUpperCase();
  return CALL_RE.test(upper) ? upper : null;
}

/**
 * Canonical JSON: object keys sorted alphabetically (recursively), no
 * whitespace. This is the exact byte string that is signed.
 * Clients must produce the same string; integers are recommended for numbers.
 * @param {unknown} value
 * @returns {string}
 */
export function canonicalJson(value) {
  if (value === null || typeof value !== "object") {
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) {
    return "[" + value.map(canonicalJson).join(",") + "]";
  }
  const parts = Object.keys(value)
    .sort()
    .filter((key) => value[key] !== undefined)
    .map((key) => JSON.stringify(key) + ":" + canonicalJson(value[key]));
  return "{" + parts.join(",") + "}";
}

/**
 * True when a record last refreshed at `refreshedAtMs` is past its 48 h life.
 * @param {number} refreshedAtMs
 * @param {number} [nowMs]
 * @returns {boolean}
 */
export function isExpired(refreshedAtMs, nowMs = Date.now()) {
  return !Number.isFinite(refreshedAtMs) || nowMs - refreshedAtMs >= EXPIRY_MS;
}

/**
 * Half-width and half-height of a circle of `radiusKm` at `latitude`, in degrees.
 * Used only to draw the area on the map.
 * @param {number} radiusKm
 * @param {number} latitude
 * @returns {{ latDeg: number, lonDeg: number }}
 */
export function radiusDegrees(radiusKm, latitude) {
  const cos = Math.max(Math.cos((latitude * Math.PI) / 180), 0.05);
  return {
    latDeg: radiusKm / KM_PER_DEG,
    lonDeg: radiusKm / (KM_PER_DEG * cos),
  };
}

/**
 * Decode standard base64 into exactly `length` bytes, or return null.
 * @param {unknown} value
 * @param {number} length
 * @returns {Uint8Array | null}
 */
export function decodeBase64(value, length) {
  if (typeof value !== "string" || value.length === 0) return null;
  if (value.length % 4 !== 0 || !BASE64_RE.test(value)) return null;
  let binary;
  try {
    binary = atob(value);
  } catch {
    return null;
  }
  if (binary.length !== length) return null;
  const bytes = new Uint8Array(length);
  for (let i = 0; i < length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

/**
 * @param {Uint8Array} bytes
 * @returns {string}
 */
export function encodeBase64(bytes) {
  let binary = "";
  for (const b of bytes) binary += String.fromCharCode(b);
  return btoa(binary);
}

/**
 * Verify an Ed25519 signature over `message` (UTF-8) with a raw 32-byte
 * public key. Any malformed input or failed check returns false.
 * @param {string} publicKeyB64
 * @param {string} signatureB64
 * @param {string} message
 * @returns {Promise<boolean>}
 */
export async function verifySignature(publicKeyB64, signatureB64, message) {
  const publicKey = decodeBase64(publicKeyB64, 32);
  const signature = decodeBase64(signatureB64, 64);
  if (!publicKey || !signature) return false;
  try {
    const key = await globalThis.crypto.subtle.importKey(
      "raw",
      publicKey,
      { name: "Ed25519" },
      false,
      ["verify"],
    );
    return await globalThis.crypto.subtle.verify(
      { name: "Ed25519" },
      key,
      signature,
      new TextEncoder().encode(message),
    );
  } catch {
    return false;
  }
}

/**
 * Reject unknown fields and missing fields. Top-level position fields
 * (latitude, longitude, lat, lon, lng) get a specific message: positions
 * belong inside reporter_location or heard_location.
 * @param {unknown} body
 * @param {string[]} allowed
 * @returns {string | null} error message, or null when the shape is fine
 */
function checkShape(body, allowed) {
  if (!body || typeof body !== "object" || Array.isArray(body)) {
    return "body must be a JSON object";
  }
  for (const key of Object.keys(body)) {
    if (!allowed.includes(key)) {
      if (/lat|lon|lng/i.test(key)) {
        return "position field must sit inside a location object";
      }
      return "body has a field that is not allowed";
    }
  }
  for (const key of allowed) {
    if (!(key in body)) return `missing field: ${key}`;
  }
  return null;
}

/**
 * True when x has at most MAX_DECIMALS decimal places.
 * @param {number} x
 * @returns {boolean}
 */
function withinDecimals(x) {
  const scaled = x * 10 ** MAX_DECIMALS;
  return Math.abs(scaled - Math.round(scaled)) < 1e-6;
}

/**
 * Validate a location object. Precise: latitude and longitude only.
 * Area: centre latitude and longitude plus a whole-number radius_km
 * from 1 to 100. Coordinates must be in range and have at most 4 decimals.
 * @param {unknown} value
 * @param {string} label used in error messages
 * @returns {{ ok: true, value: object } | { ok: false, error: string }}
 */
export function validateLocation(value, label) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return { ok: false, error: `${label} must be an object` };
  }
  const v = /** @type {Record<string, any>} */ (value);
  if (v.mode !== "precise" && v.mode !== "area") {
    return { ok: false, error: `${label}.mode must be "precise" or "area"` };
  }
  const allowed = v.mode === "precise" ? PRECISE_KEYS : AREA_KEYS;
  for (const key of Object.keys(v)) {
    if (!allowed.includes(key)) return { ok: false, error: `${label} has a field that is not allowed` };
  }
  for (const key of allowed) {
    if (!(key in v)) return { ok: false, error: `${label} missing field: ${key}` };
  }

  const { latitude, longitude } = v;
  if (typeof latitude !== "number" || !Number.isFinite(latitude) || latitude < -90 || latitude > 90) {
    return { ok: false, error: `${label}.latitude must be a number from -90 to 90` };
  }
  if (typeof longitude !== "number" || !Number.isFinite(longitude) || longitude < -180 || longitude > 180) {
    return { ok: false, error: `${label}.longitude must be a number from -180 to 180` };
  }
  if (!withinDecimals(latitude) || !withinDecimals(longitude)) {
    return { ok: false, error: `${label} coordinates may have at most ${MAX_DECIMALS} decimal places` };
  }

  /** @type {Record<string, unknown>} */
  const out = { mode: v.mode, latitude, longitude };
  if (v.mode === "area") {
    const r = v.radius_km;
    if (!Number.isSafeInteger(r) || r < RADIUS_MIN_KM || r > RADIUS_MAX_KM) {
      return { ok: false, error: `${label}.radius_km must be a whole number from ${RADIUS_MIN_KM} to ${RADIUS_MAX_KM}` };
    }
    out.radius_km = r;
  }
  return { ok: true, value: out };
}

/**
 * The bytes that the signature covers: canonical JSON of every field
 * except public_key and signature, exactly as the client sent them.
 * @param {Record<string, unknown>} body
 * @returns {string}
 */
export function signedMessage(body) {
  const rest = {};
  for (const key of Object.keys(body)) {
    if (key !== "public_key" && key !== "signature") rest[key] = body[key];
  }
  return canonicalJson(rest);
}

/**
 * Validate a report body (shape, callsigns, locations, time, frequency,
 * mode, key and signature sizes). Does not verify the signature; call
 * verifySignature with `message` for that.
 * @param {unknown} body
 * @param {number} [nowMs]
 * @returns {{ ok: true, record: object, publicKey: string, signature: string, message: string } | { ok: false, error: string }}
 */
export function validateReport(body, nowMs = Date.now()) {
  const shape = checkShape(body, REPORT_KEYS);
  if (shape) return { ok: false, error: shape };
  const b = /** @type {Record<string, any>} */ (body);

  const reporterCall = normalizeCall(b.reporter_call);
  if (!reporterCall) return { ok: false, error: "reporter_call is not a valid callsign" };
  const heardCall = normalizeCall(b.heard_call);
  if (!heardCall) return { ok: false, error: "heard_call is not a valid callsign" };

  const reporterLoc = validateLocation(b.reporter_location, "reporter_location");
  if (!reporterLoc.ok) return reporterLoc;
  const heardLoc = validateLocation(b.heard_location, "heard_location");
  if (!heardLoc.ok) return heardLoc;

  const heardMs = parseIsoUtc(b.heard_at);
  if (heardMs === null) {
    return { ok: false, error: "heard_at must be an ISO 8601 UTC time ending in Z" };
  }
  if (heardMs > nowMs + FUTURE_SKEW_MS || heardMs < nowMs - MAX_REPORT_AGE_MS) {
    return { ok: false, error: "heard_at must be within the last 10 minutes" };
  }

  const frequency = b.frequency_khz;
  if (!Number.isSafeInteger(frequency) || frequency <= 0 || frequency > MAX_FREQUENCY_KHZ) {
    return { ok: false, error: "frequency_khz must be a positive integer" };
  }

  if (typeof b.mode !== "string" || !MODE_RE.test(b.mode)) {
    return { ok: false, error: "mode is not valid" };
  }

  const publicKeyBytes = decodePublicKey(b.public_key);
  if (!publicKeyBytes) return { ok: false, error: "public_key must be base64 of a 32 byte Ed25519 key" };
  const signatureBytes = decodeBase64(b.signature, 64);
  if (!signatureBytes) return { ok: false, error: "signature must be base64 of a 64 byte Ed25519 signature" };

  return {
    ok: true,
    record: {
      reporter_call: reporterCall,
      reporter_location: reporterLoc.value,
      heard_call: heardCall,
      heard_location: heardLoc.value,
      heard_at: b.heard_at,
      frequency_khz: frequency,
      mode: b.mode,
    },
    publicKey: encodeBase64(publicKeyBytes),
    signature: b.signature,
    message: signedMessage(b),
  };
}

/**
 * Validate a removal body. The signed message covers reporter_call,
 * heard_call and signed_at, which must be within REMOVE_WINDOW_MS of now.
 * @param {unknown} body
 * @param {number} [nowMs]
 * @returns {{ ok: true, reporterCall: string, heardCall: string, signedAtMs: number, publicKey: string, signature: string, message: string } | { ok: false, error: string }}
 */
export function validateRemove(body, nowMs = Date.now()) {
  const shape = checkShape(body, REMOVE_KEYS);
  if (shape) return { ok: false, error: shape };
  const b = /** @type {Record<string, any>} */ (body);

  const reporterCall = normalizeCall(b.reporter_call);
  if (!reporterCall) return { ok: false, error: "reporter_call is not a valid callsign" };
  const heardCall = normalizeCall(b.heard_call);
  if (!heardCall) return { ok: false, error: "heard_call is not a valid callsign" };
  const signedAtMs = parseIsoUtc(b.signed_at);
  if (signedAtMs === null) {
    return { ok: false, error: "signed_at must be an ISO 8601 UTC time ending in Z" };
  }
  if (Math.abs(signedAtMs - nowMs) > REMOVE_WINDOW_MS) {
    return { ok: false, error: "signed_at must be within 5 minutes of the server time" };
  }
  const publicKeyBytes = decodePublicKey(b.public_key);
  if (!publicKeyBytes) return { ok: false, error: "public_key must be base64 of a 32 byte Ed25519 key" };
  if (!decodeBase64(b.signature, 64)) {
    return { ok: false, error: "signature must be base64 of a 64 byte Ed25519 signature" };
  }

  return {
    ok: true,
    reporterCall,
    heardCall,
    signedAtMs,
    publicKey: encodeBase64(publicKeyBytes),
    signature: b.signature,
    message: signedMessage(b),
  };
}

const REGISTER_KEYS = ["area", "callsign", "public_key", "signature"];
const SCHEDULE_KEYS = ["public_key", "relay", "requests", "signature"];
const SCHEDULE_REQUEST_KEYS = ["channel", "direction", "duration"];
const SCHEDULE_REQUEST_OPTIONAL = ["urgent", "latency_budget_s"];

/** Most slot requests one relay can send in a single schedule call. */
export const MAX_SCHEDULE_REQUESTS = 20;

/** Largest latency budget a sender may set, in seconds (one hour). */
export const MAX_LATENCY_BUDGET_S = 3600;

/**
 * Validate a relay registration: callsign, area and the relay's public key,
 * signed by that key. Only the format of the callsign is checked.
 * @param {unknown} body
 * @returns {{ ok: true, callsign: string, area: string, publicKey: string, signature: string, message: string } | { ok: false, error: string }}
 */
export function validateRegister(body) {
  const shape = checkShape(body, REGISTER_KEYS);
  if (shape) return { ok: false, error: shape };
  const b = /** @type {Record<string, any>} */ (body);

  const callsign = normalizeCall(b.callsign);
  if (!callsign) return { ok: false, error: "callsign is not a valid callsign" };
  if (typeof b.area !== "string" || !AREA_KEY_RE.test(b.area)) {
    return { ok: false, error: 'area must be a one-degree square such as "N51W1"' };
  }
  const publicKeyBytes = decodePublicKey(b.public_key);
  if (!publicKeyBytes) return { ok: false, error: "public_key must be base64 of a 32 byte Ed25519 key" };
  if (!decodeBase64(b.signature, 64)) {
    return { ok: false, error: "signature must be base64 of a 64 byte Ed25519 signature" };
  }

  return {
    ok: true,
    callsign,
    area: b.area,
    publicKey: encodeBase64(publicKeyBytes),
    signature: b.signature,
    message: signedMessage(b),
  };
}

/**
 * Validate a schedule request from a registered relay. The signed message
 * covers relay and requests. Each request has channel, duration and direction,
 * and may have urgent. The values themselves are checked by makeSchedule.
 * @param {unknown} body
 * @returns {{ ok: true, relay: string, requests: object[], publicKey: string, signature: string, message: string } | { ok: false, error: string }}
 */
export function validateSchedule(body) {
  const shape = checkShape(body, SCHEDULE_KEYS);
  if (shape) return { ok: false, error: shape };
  const b = /** @type {Record<string, any>} */ (body);

  const relay = normalizeCall(b.relay);
  if (!relay) return { ok: false, error: "relay is not a valid callsign" };
  if (!Array.isArray(b.requests) || b.requests.length < 1 || b.requests.length > MAX_SCHEDULE_REQUESTS) {
    return { ok: false, error: `requests must be a list of 1 to ${MAX_SCHEDULE_REQUESTS} slot requests` };
  }
  for (let i = 0; i < b.requests.length; i++) {
    const q = b.requests[i];
    if (!q || typeof q !== "object" || Array.isArray(q)) {
      return { ok: false, error: `requests[${i}] must be an object` };
    }
    for (const key of Object.keys(q)) {
      if (!SCHEDULE_REQUEST_KEYS.includes(key) && !SCHEDULE_REQUEST_OPTIONAL.includes(key)) {
        return { ok: false, error: `requests[${i}] has a field that is not allowed` };
      }
    }
    for (const key of SCHEDULE_REQUEST_KEYS) {
      if (!(key in q)) return { ok: false, error: `requests[${i}] missing field: ${key}` };
    }
  }
  const publicKeyBytes = decodePublicKey(b.public_key);
  if (!publicKeyBytes) return { ok: false, error: "public_key must be base64 of a 32 byte Ed25519 key" };
  if (!decodeBase64(b.signature, 64)) {
    return { ok: false, error: "signature must be base64 of a 64 byte Ed25519 signature" };
  }

  return {
    ok: true,
    relay,
    requests: b.requests,
    publicKey: encodeBase64(publicKeyBytes),
    signature: b.signature,
    message: signedMessage(b),
  };
}

// ---------------------------------------------------------------------------
// Busy zones, channel pools and relay slots (plan sections 2.4, 5.3, 5.4).
// ---------------------------------------------------------------------------

/**
 * The one band segment KAHF uses, in kHz. The lower edge is included and the
 * upper edge is not. Every channel count and channel number is derived from
 * this constant and CHANNEL_WIDTH_KHZ, so change the segment here only.
 */
export const BAND_SEGMENT_KHZ = Object.freeze({ low: 14_000, high: 14_350 });

/** Width of one channel in kHz (plan section 2.2). */
export const CHANNEL_WIDTH_KHZ = 2.7;

/** Whole channels that fit in the segment. */
export const BAND_CHANNELS = Math.floor(((BAND_SEGMENT_KHZ.high - BAND_SEGMENT_KHZ.low) * 10) / (CHANNEL_WIDTH_KHZ * 10));

/**
 * Channel number for a frequency in kHz, or null when it is outside the
 * channel grid. Integer arithmetic in tenths of a kHz keeps the edges exact.
 * @param {number} frequencyKhz
 * @returns {number | null}
 */
export function channelOf(frequencyKhz) {
  if (!Number.isFinite(frequencyKhz)) return null;
  const tenths = Math.round((frequencyKhz - BAND_SEGMENT_KHZ.low) * 10);
  const width = Math.round(CHANNEL_WIDTH_KHZ * 10);
  if (tenths < 0) return null;
  const channel = Math.floor(tenths / width);
  return channel < BAND_CHANNELS ? channel : null;
}

/** An area is a one-degree square, named by its south-west corner, e.g. "N51W1". */
const AREA_KEY_RE = /^(?:N(?:0|[1-9]|[1-8]\d)|S(?:[1-9]|[1-8]\d|90))(?:E(?:0|[1-9]\d?|1[0-7]\d)|W(?:[1-9]\d?|1[0-7]\d|180))$/;

/**
 * Area key for a position: the one-degree square that holds it.
 * @param {number} latitude
 * @param {number} longitude
 * @returns {string}
 */
export function areaKey(latitude, longitude) {
  const lat = Math.floor(latitude);
  const lon = Math.floor(longitude);
  return `${lat >= 0 ? "N" : "S"}${Math.abs(lat)}${lon >= 0 ? "E" : "W"}${Math.abs(lon)}`;
}

/**
 * Turn stored station records into reception reports for busyZones: the
 * reporter heard a signal on a channel, at its position, at heard_at.
 * Records outside the band are skipped.
 * @param {Array<object>} records
 * @returns {Array<{ channel: number, area: string, time: number }>} time in Unix seconds
 */
export function zoneReports(records) {
  if (!Array.isArray(records)) return [];
  const out = [];
  for (const r of records) {
    if (!r || typeof r !== "object") continue;
    const channel = channelOf(r.frequency_khz);
    const loc = r.reporter_location;
    if (channel === null || !loc || !Number.isFinite(loc.latitude) || !Number.isFinite(loc.longitude)) continue;
    const time = Date.parse(r.heard_at) / 1000;
    if (!Number.isFinite(time)) continue;
    out.push({ channel, area: areaKey(loc.latitude, loc.longitude), time });
  }
  return out;
}

/** A channel counts as busy in an area for this long after its last report (seconds). */
export const BUSY_WINDOW_S = 10 * 60;

/** Relay slot duration limits, in whole seconds. */
export const SLOT_MIN_S = 1;
export const SLOT_MAX_S = 300;

/** Reports this far ahead of the server clock are ignored (seconds). */
const ZONE_FUTURE_SKEW_S = 5 * 60;

const DIRECTIONS = ["rx", "tx"];

/** @param {unknown} c @returns {boolean} */
function isChannel(c) {
  return Number.isSafeInteger(c) && c >= 0 && c < BAND_CHANNELS;
}

/** @param {unknown} a @returns {boolean} */
function isArea(a) {
  return typeof a === "string" && a.length > 0 && a.length <= 64;
}

/**
 * Busy channels per area, from signed reception reports. A report counts
 * only if it is less than BUSY_WINDOW_S old. Malformed reports are skipped.
 * @param {Array<{ channel: number, area: string, time: number, level: number }>} reports
 *   time and nowUnix are Unix seconds. level is optional; when given it must be finite.
 * @param {number} nowUnix
 * @returns {Record<string, number[]>} area -> sorted, unique channels
 */
export function busyZones(reports, nowUnix) {
  if (!Array.isArray(reports) || !Number.isFinite(nowUnix)) return {};
  /** @type {Map<string, Set<number>>} */
  const zones = new Map();
  for (const r of reports) {
    if (!r || typeof r !== "object") continue;
    if (!isChannel(r.channel) || !isArea(r.area)) continue;
    if (!Number.isFinite(r.time)) continue;
    if (r.level !== undefined && !Number.isFinite(r.level)) continue;
    const age = nowUnix - r.time;
    if (age < -ZONE_FUTURE_SKEW_S || age >= BUSY_WINDOW_S) continue;
    if (!zones.has(r.area)) zones.set(r.area, new Set());
    zones.get(r.area).add(r.channel);
  }
  /** @type {Record<string, number[]>} */
  const out = {};
  for (const area of [...zones.keys()].sort()) {
    out[area] = [...zones.get(area)].sort((a, b) => a - b);
  }
  return out;
}

/**
 * Channel pools for the client. The lower third of the band is the online
 * pool, kept for areas where stations are online (every area in zoneBusy).
 * The rest is the open pool, used everywhere else (defaultPool). Each listed
 * area gets its online channels minus the ones busy there.
 * @param {Record<string, number[]>} zoneBusy output of busyZones
 * @returns {{
 *   bandChannels: number,
 *   online: number[],
 *   open: number[],
 *   defaultPool: "open",
 *   areas: Record<string, { pool: "online", channels: number[] }>
 * }}
 */
export function poolPlan(zoneBusy) {
  const onlineCount = Math.floor(BAND_CHANNELS / 3);
  const online = Array.from({ length: onlineCount }, (_, i) => i);
  const open = Array.from({ length: BAND_CHANNELS - onlineCount }, (_, i) => onlineCount + i);
  /** @type {Record<string, { pool: "online", channels: number[] }>} */
  const areas = {};
  const zones = zoneBusy && typeof zoneBusy === "object" ? zoneBusy : {};
  for (const area of Object.keys(zones).sort()) {
    const busy = new Set(Array.isArray(zones[area]) ? zones[area] : []);
    areas[area] = { pool: "online", channels: online.filter((c) => !busy.has(c)) };
  }
  return { bandChannels: BAND_CHANNELS, online, open, defaultPool: "open", areas };
}

/**
 * Plan relay slots and say which requests could not be placed. Each request
 * asks one relay for a channel, a duration and a direction. Urgent requests are
 * placed first, then in request order. A slot never shares a channel and time
 * with another relay in the same area, and a relay never has two slots at once
 * (one radio). Each slot starts at the earliest free time at or after nowUnix
 * (greedy).
 * A request may carry latency_budget_s: the sender's limit, in seconds from
 * nowUnix, on when the slot may start. A request that cannot start inside its
 * budget is not booked and is listed in `unplaced` instead. A request without
 * a budget has no limit.
 * `booked` holds slots already planned for other relays (for example, the
 * live relays' stored slots). They are treated as taken, so a new slot never
 * shares a channel with them in the same area.
 * Throws a TypeError or RangeError on invalid input.
 * @param {Array<{ id: string, area: string }>} relays
 * @param {Array<{ relay: string, channel: number, duration: number, direction: "rx" | "tx", urgent?: boolean, latency_budget_s?: number }>} requests
 * @param {number} nowUnix Unix seconds
 * @param {Array<{ area: string, channel: number, start: number, duration: number }>} [booked]
 * @returns {{
 *   slots: Record<string, Array<{ channel: number, start: number, duration: number, direction: "rx" | "tx" }>>,
 *   unplaced: Array<{ index: number, relay: string, channel: number, direction: "rx" | "tx", reason: string }>
 * }} slots per relay id, sorted by start; unplaced requests in request order
 */
export function planSchedule(relays, requests, nowUnix, booked = []) {
  if (!Array.isArray(relays) || !Array.isArray(requests) || !Array.isArray(booked)) {
    throw new TypeError("relays, requests and booked must be arrays");
  }
  if (!Number.isSafeInteger(nowUnix)) throw new TypeError("nowUnix must be a whole number of seconds");

  /** @type {Map<string, string>} relay id -> area */
  const areaOf = new Map();
  for (const r of relays) {
    if (!r || typeof r !== "object" || typeof r.id !== "string" || r.id.length === 0 || !isArea(r.area)) {
      throw new TypeError("each relay needs a non-empty id and an area");
    }
    if (areaOf.has(r.id)) throw new TypeError("duplicate relay id");
    areaOf.set(r.id, r.area);
  }

  const checked = requests.map((q, index) => {
    if (!q || typeof q !== "object") throw new TypeError(`request ${index} must be an object`);
    if (!areaOf.has(q.relay)) throw new RangeError(`request ${index}: unknown relay`);
    if (!isChannel(q.channel)) {
      throw new RangeError(`request ${index}: channel must be a whole number from 0 to ${BAND_CHANNELS - 1}`);
    }
    if (!Number.isSafeInteger(q.duration) || q.duration < SLOT_MIN_S || q.duration > SLOT_MAX_S) {
      throw new RangeError(`request ${index}: duration must be a whole number of seconds from ${SLOT_MIN_S} to ${SLOT_MAX_S}`);
    }
    if (!DIRECTIONS.includes(q.direction)) throw new RangeError(`request ${index}: direction must be "rx" or "tx"`);
    if (q.urgent !== undefined && typeof q.urgent !== "boolean") {
      throw new TypeError(`request ${index}: urgent must be true or false`);
    }
    const budget = q.latency_budget_s;
    if (budget !== undefined && (!Number.isSafeInteger(budget) || budget < 0 || budget > MAX_LATENCY_BUDGET_S)) {
      throw new RangeError(`request ${index}: latency_budget_s must be a whole number of seconds from 0 to ${MAX_LATENCY_BUDGET_S}`);
    }
    return {
      index,
      relay: q.relay,
      channel: q.channel,
      duration: q.duration,
      direction: q.direction,
      urgent: q.urgent === true,
      budget,
    };
  });

  checked.sort((a, b) => Number(b.urgent) - Number(a.urgent) || a.index - b.index);

  /** @type {Map<string, Array<[number, number]>>} "area|channel" -> busy intervals */
  const channelBusy = new Map();
  /** @type {Map<string, Array<[number, number]>>} relay id -> busy intervals */
  const relayBusy = new Map();
  for (const b of booked) {
    if (!b || typeof b !== "object" || !isArea(b.area) || !isChannel(b.channel)) {
      throw new TypeError("each booked slot needs an area and a channel in the band");
    }
    if (!Number.isSafeInteger(b.start) || !Number.isSafeInteger(b.duration) || b.duration < 0) {
      throw new TypeError("each booked slot needs a whole start and duration");
    }
    const key = `${b.area}|${b.channel}`;
    if (!channelBusy.has(key)) channelBusy.set(key, []);
    channelBusy.get(key).push([b.start, b.start + b.duration]);
  }
  /** @type {Record<string, Array<{ channel: number, start: number, duration: number, direction: "rx" | "tx" }>>} */
  const out = {};
  for (const r of relays) out[r.id] = [];
  /** @type {Array<{ index: number, relay: string, channel: number, direction: "rx" | "tx", reason: string }>} */
  const unplaced = [];

  for (const q of checked) {
    const channelKey = `${areaOf.get(q.relay)}|${q.channel}`;
    if (!channelBusy.has(channelKey)) channelBusy.set(channelKey, []);
    if (!relayBusy.has(q.relay)) relayBusy.set(q.relay, []);
    const busy = [...channelBusy.get(channelKey), ...relayBusy.get(q.relay)];

    // Push the start past any overlapping interval until none overlap.
    let start = nowUnix;
    let moved = true;
    while (moved) {
      moved = false;
      for (const [s, e] of busy) {
        if (start < e && start + q.duration > s) {
          start = e;
          moved = true;
        }
      }
    }

    if (q.budget !== undefined && start > nowUnix + q.budget) {
      unplaced.push({
        index: q.index,
        relay: q.relay,
        channel: q.channel,
        direction: q.direction,
        reason: "no free time inside the latency budget",
      });
      continue;
    }

    const interval = [start, start + q.duration];
    channelBusy.get(channelKey).push(interval);
    relayBusy.get(q.relay).push(interval);
    out[q.relay].push({ channel: q.channel, start, duration: q.duration, direction: q.direction });
  }

  for (const id of Object.keys(out)) out[id].sort((a, b) => a.start - b.start);
  unplaced.sort((a, b) => a.index - b.index);
  return { slots: out, unplaced };
}

/**
 * Plan relay slots (see planSchedule) and return only the slots per relay.
 * Requests that cannot meet their latency budget are left out.
 * @param {Parameters<typeof planSchedule>[0]} relays
 * @param {Parameters<typeof planSchedule>[1]} requests
 * @param {number} nowUnix
 * @param {Parameters<typeof planSchedule>[3]} [booked]
 * @returns {Record<string, Array<{ channel: number, start: number, duration: number, direction: "rx" | "tx" }>>}
 */
export function makeSchedule(relays, requests, nowUnix, booked = []) {
  return planSchedule(relays, requests, nowUnix, booked).slots;
}

// ---------------------------------------------------------------------------
// Release announcements (plan section 5.5).
// ---------------------------------------------------------------------------

const RELEASE_KEYS = ["notes_url", "public_key", "published_at", "required", "signature", "version"];
const VERSION_RE = /^\d{1,3}\.\d{1,3}\.\d{1,3}$/;

/**
 * Parse "major.minor.patch" into numbers, or null.
 * @param {unknown} version
 * @returns {number[] | null}
 */
export function parseVersion(version) {
  if (typeof version !== "string" || !VERSION_RE.test(version)) return null;
  return version.split(".").map(Number);
}

/**
 * Compare two version strings. Returns a negative, zero or positive number, or
 * null when either is not a version.
 * @param {string} a
 * @param {string} b
 * @returns {number | null}
 */
export function compareVersions(a, b) {
  const pa = parseVersion(a);
  const pb = parseVersion(b);
  if (!pa || !pb) return null;
  for (let i = 0; i < 3; i++) {
    if (pa[i] !== pb[i]) return pa[i] - pb[i];
  }
  return 0;
}

/**
 * Validate a release announcement signed by the owner's release key. The
 * caller checks that publicKey is the configured release key and verifies the
 * signature. Only https links are accepted for the release notes.
 * @param {unknown} body
 * @returns {{ ok: true, record: { version: string, published_at: string, required: boolean, notes_url: string }, publicKey: string, signature: string, message: string } | { ok: false, error: string }}
 */
export function validateRelease(body) {
  const shape = checkShape(body, RELEASE_KEYS);
  if (shape) return { ok: false, error: shape };
  const b = /** @type {Record<string, any>} */ (body);

  if (!parseVersion(b.version)) return { ok: false, error: 'version must look like "4.2.0"' };
  if (parseIsoUtc(b.published_at) === null) {
    return { ok: false, error: "published_at must be an ISO 8601 UTC time ending in Z" };
  }
  if (typeof b.required !== "boolean") return { ok: false, error: "required must be true or false" };
  if (typeof b.notes_url !== "string" || b.notes_url.length > 200) {
    return { ok: false, error: "notes_url must be a link of at most 200 characters" };
  }
  let url;
  try {
    url = new URL(b.notes_url);
  } catch {
    url = null;
  }
  if (!url || url.protocol !== "https:" || url.username || url.password) {
    return { ok: false, error: "notes_url must be an https link" };
  }
  const publicKeyBytes = decodePublicKey(b.public_key);
  if (!publicKeyBytes) return { ok: false, error: "public_key must be base64 of a 32 byte Ed25519 key" };
  if (!decodeBase64(b.signature, 64)) {
    return { ok: false, error: "signature must be base64 of a 64 byte Ed25519 signature" };
  }

  return {
    ok: true,
    record: {
      version: b.version,
      published_at: b.published_at,
      required: b.required,
      notes_url: b.notes_url,
    },
    publicKey: encodeBase64(publicKeyBytes),
    signature: b.signature,
    message: signedMessage(b),
  };
}

// ---------------------------------------------------------------------------
// Map data (plan sections 5.1 and 5.2).
// ---------------------------------------------------------------------------

/** A relay registers only its one-degree area, so its map circle covers that square. */
const AREA_CIRCLE_KM = 79;

/**
 * The centre of a one-degree area such as "S10E5" or "N51W1". The area key holds
 * the floor of each coordinate, so the centre is 0.5 degrees on from it.
 * @param {string} area
 * @returns {{ latitude: number, longitude: number } | null}
 */
export function areaCentre(area) {
  const m = /^([NS])(\d+)([EW])(\d+)$/.exec(typeof area === "string" ? area : "");
  if (!m) return null;
  const lat = (m[1] === "N" ? 1 : -1) * Number(m[2]) + 0.5;
  const lon = (m[3] === "E" ? 1 : -1) * Number(m[4]) + 0.5;
  return { latitude: lat, longitude: lon };
}

/**
 * Data for the map page and GET /map-data: the relays that registered (as
 * area circles) and the stations whose reports are still live. Nothing here
 * is finer than the station chose to share. No public keys.
 * @param {Array<{ callsign: string, area: string, refreshed_at: number }>} relays
 * @param {Array<object>} stations public station records (see toPublic in routes.js)
 * @param {number} nowMs
 */
export function buildMapData(relays, stations, nowMs) {
  const out = [];
  for (const r of Array.isArray(relays) ? relays : []) {
    const centre = areaCentre(r.area);
    if (!centre || isExpired(r.refreshed_at, nowMs)) continue;
    out.push({
      callsign: r.callsign,
      mode: "relay",
      last_seen: new Date(r.refreshed_at).toISOString(),
      location: { mode: "area", ...centre, radius_km: AREA_CIRCLE_KM },
    });
  }
  out.sort((a, b) => a.callsign.localeCompare(b.callsign));
  return {
    generated_at: new Date(nowMs).toISOString(),
    relays: out,
    stations: Array.isArray(stations) ? stations : [],
  };
}
