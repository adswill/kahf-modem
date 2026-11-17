// Unit tests for the pure logic in src/core.js. Run with: npm test (node --test).
// No wrangler, no network, no dependencies beyond Node 20.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  EXPIRY_MS,
  canonicalJson,
  decodeBase64,
  encodeBase64,
  isExpired,
  normalizeCall,
  radiusDegrees,
  signedMessage,
  validateLocation,
  validateRemove,
  validateReport,
  verifySignature,
} from "../src/core.js";

const NOW = Date.parse("2026-11-17T12:00:00Z");
const enc = new TextEncoder();

async function newKey() {
  const pair = await crypto.subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"]);
  const raw = new Uint8Array(await crypto.subtle.exportKey("raw", pair.publicKey));
  return { privateKey: pair.privateKey, publicKey: encodeBase64(raw) };
}

/** Signs canonical JSON of `fields` and returns the full request body. */
async function signed(key, fields) {
  const sig = new Uint8Array(
    await crypto.subtle.sign({ name: "Ed25519" }, key.privateKey, enc.encode(canonicalJson(fields))),
  );
  return { ...fields, public_key: key.publicKey, signature: encodeBase64(sig) };
}

const PRECISE_REPORTER = { mode: "precise", latitude: 51.5074, longitude: -0.1278 };
const AREA_HEARD = { mode: "area", latitude: 48.8566, longitude: 2.3522, radius_km: 50 };

const REPORT_FIELDS = {
  reporter_call: "K1ABC",
  reporter_location: PRECISE_REPORTER,
  heard_call: "DL1XYZ",
  heard_location: AREA_HEARD,
  heard_at: "2026-11-17T11:59:00Z",
  frequency_khz: 14074,
  mode: "FT8",
};

// ---- callsigns -------------------------------------------------------------

test("callsign: normalises case and rejects junk", () => {
  assert.equal(normalizeCall("k1abc"), "K1ABC");
  assert.equal(normalizeCall("W1AW/4"), "W1AW/4");
  assert.equal(normalizeCall("ABC DEF"), null);
  assert.equal(normalizeCall(""), null);
  assert.equal(normalizeCall("K1ABCDEFGHIJK"), null);
});

// ---- locations -------------------------------------------------------------

test("location: precise points and area circles are accepted", () => {
  assert.equal(validateLocation(PRECISE_REPORTER, "x").ok, true);
  assert.equal(validateLocation(AREA_HEARD, "x").ok, true);
});

test("location: area radius must be a whole number from 1 to 100", () => {
  for (const r of [1, 50, 100]) {
    assert.equal(validateLocation({ ...AREA_HEARD, radius_km: r }, "x").ok, true, `radius ${r}`);
  }
  for (const r of [0, -5, 101, 1000, 2.5, "50", null]) {
    const result = validateLocation({ ...AREA_HEARD, radius_km: r }, "x");
    assert.equal(result.ok, false, `radius ${r}`);
    assert.match(result.error, /radius_km/);
  }
});

test("location: out-of-range coordinates are refused", () => {
  const bad = [
    { latitude: 90.0001, longitude: 0 },
    { latitude: -91, longitude: 0 },
    { latitude: 0, longitude: 180.5 },
    { latitude: 0, longitude: -181 },
    { latitude: Number.NaN, longitude: 0 },
    { latitude: "51.5", longitude: 0 },
  ];
  for (const coords of bad) {
    const result = validateLocation({ mode: "precise", ...coords }, "x");
    assert.equal(result.ok, false, JSON.stringify(coords));
  }
  assert.equal(validateLocation({ mode: "precise", latitude: 90, longitude: 180 }, "x").ok, true, "limits are inclusive");
});

test("location: more than 4 decimal places is refused", () => {
  const result = validateLocation({ mode: "precise", latitude: 51.50741, longitude: 0 }, "x");
  assert.equal(result.ok, false);
  assert.match(result.error, /4 decimal/);
  assert.equal(validateLocation({ mode: "precise", latitude: 51.5074, longitude: -0.1278 }, "x").ok, true);
});

test("location: mode must be precise or area, and fields must match the mode", () => {
  assert.equal(validateLocation({ mode: "grid", latitude: 1, longitude: 1 }, "x").ok, false);
  assert.equal(validateLocation({ mode: "precise", latitude: 1, longitude: 1, radius_km: 5 }, "x").ok, false, "no radius in precise");
  assert.equal(validateLocation({ mode: "area", latitude: 1, longitude: 1 }, "x").ok, false, "area needs radius");
  assert.equal(validateLocation({ mode: "precise", latitude: 1 }, "x").ok, false, "longitude missing");
  assert.equal(validateLocation({ mode: "precise", latitude: 1, longitude: 1, altitude: 9 }, "x").ok, false);
  assert.equal(validateLocation("51.5,-0.1", "x").ok, false, "text is not a location");
});

test("radius degrees: longitude span grows toward the poles", () => {
  const equator = radiusDegrees(111.32, 0);
  const high = radiusDegrees(111.32, 60);
  assert.ok(Math.abs(equator.latDeg - 1) < 1e-9);
  assert.ok(Math.abs(equator.lonDeg - 1) < 1e-9);
  assert.ok(Math.abs(high.lonDeg - 2) < 1e-9, "twice the longitude span at 60 degrees");
});

// ---- canonical JSON -------------------------------------------------------

test("canonical JSON: sorted keys at every level, no spaces", () => {
  const value = { b: 1, a: { d: [2, { z: 1, y: "x" }], c: null } };
  assert.equal(canonicalJson(value), '{"a":{"c":null,"d":[2,{"y":"x","z":1}]},"b":1}');
});

test("canonical JSON: drops undefined fields and keeps strings escaped", () => {
  assert.equal(canonicalJson({ a: undefined, b: 'say "hi"' }), '{"b":"say \\"hi\\""}');
});

test("signed message: nested locations sorted, public_key and signature excluded", () => {
  const body = { ...REPORT_FIELDS, public_key: "x", signature: "y" };
  assert.equal(
    signedMessage(body),
    '{"frequency_khz":14074,"heard_at":"2026-11-17T11:59:00Z","heard_call":"DL1XYZ",' +
      '"heard_location":{"latitude":48.8566,"longitude":2.3522,"mode":"area","radius_km":50},' +
      '"mode":"FT8","reporter_call":"K1ABC",' +
      '"reporter_location":{"latitude":51.5074,"longitude":-0.1278,"mode":"precise"}}',
  );
});

// ---- expiry ---------------------------------------------------------------

test("expiry: records live for 48 hours and then expire", () => {
  assert.equal(EXPIRY_MS, 48 * 60 * 60 * 1000);
  assert.equal(isExpired(NOW - EXPIRY_MS + 1, NOW), false, "1 ms before the limit");
  assert.equal(isExpired(NOW - EXPIRY_MS, NOW), true, "exactly at the limit");
  assert.equal(isExpired(NOW - 3 * EXPIRY_MS, NOW), true, "long past");
  assert.equal(isExpired(NOW, NOW), false, "just refreshed");
});

test("expiry: a missing or invalid refresh time counts as expired", () => {
  assert.equal(isExpired(Number.NaN, NOW), true);
  assert.equal(isExpired(undefined, NOW), true);
});

// ---- base64 and signatures -------------------------------------------------

test("base64: decodes only the exact length", () => {
  const bytes = new Uint8Array(32).fill(7);
  const b64 = encodeBase64(bytes);
  assert.deepEqual(decodeBase64(b64, 32), bytes);
  assert.equal(decodeBase64(b64, 64), null, "wrong length");
  assert.equal(decodeBase64("not base64!", 32), null);
  assert.equal(decodeBase64("", 32), null);
});

test("signature: valid signature verifies, tampered location does not", async () => {
  const key = await newKey();
  const body = await signed(key, REPORT_FIELDS);
  assert.equal(await verifySignature(body.public_key, body.signature, signedMessage(body)), true);

  const tampered = { ...body, heard_location: { ...AREA_HEARD, latitude: 49 } };
  assert.equal(await verifySignature(tampered.public_key, tampered.signature, signedMessage(tampered)), false);
});

test("signature: a different key cannot verify", async () => {
  const key = await newKey();
  const other = await newKey();
  const body = await signed(key, REPORT_FIELDS);
  assert.equal(await verifySignature(other.publicKey, body.signature, signedMessage(body)), false);
});

test("signature: malformed inputs return false instead of throwing", async () => {
  assert.equal(await verifySignature("AAAA", "AAAA", "x"), false);
  assert.equal(await verifySignature(undefined, undefined, "x"), false);
});

// ---- report validation -----------------------------------------------------

test("report: a correctly signed report is accepted and normalised", async () => {
  const key = await newKey();
  const body = await signed(key, REPORT_FIELDS);
  const result = validateReport(body, NOW);
  assert.equal(result.ok, true, result.error);
  assert.deepEqual(result.record, {
    reporter_call: "K1ABC",
    reporter_location: PRECISE_REPORTER,
    heard_call: "DL1XYZ",
    heard_location: AREA_HEARD,
    heard_at: "2026-11-17T11:59:00Z",
    frequency_khz: 14074,
    mode: "FT8",
  });
  assert.equal(result.publicKey, key.publicKey);
  assert.equal(await verifySignature(result.publicKey, result.signature, result.message), true);
});

test("report: top-level latitude and longitude are refused with a clear message", async () => {
  const key = await newKey();
  for (const field of ["latitude", "longitude", "lat", "lon", "lng"]) {
    const result = validateReport(await signed(key, { ...REPORT_FIELDS, [field]: 51.5 }), NOW);
    assert.equal(result.ok, false, field);
    assert.match(result.error, /location object/, field);
  }
});

test("report: unknown and missing fields are refused", async () => {
  const key = await newKey();
  const extra = validateReport(await signed(key, { ...REPORT_FIELDS, note: "hi" }), NOW);
  assert.equal(extra.ok, false);
  assert.match(extra.error, /not allowed/);

  const { mode, ...missing } = REPORT_FIELDS;
  const lacking = validateReport(await signed(key, missing), NOW);
  assert.equal(lacking.ok, false);
  assert.match(lacking.error, /missing field: mode/);
});

test("report: a bad location in either party is refused", async () => {
  const key = await newKey();
  const badRadius = validateReport(
    await signed(key, { ...REPORT_FIELDS, heard_location: { ...AREA_HEARD, radius_km: 150 } }),
    NOW,
  );
  assert.equal(badRadius.ok, false);
  assert.match(badRadius.error, /heard_location\.radius_km/);

  const badReporter = validateReport(
    await signed(key, { ...REPORT_FIELDS, reporter_location: { mode: "precise", latitude: 95, longitude: 0 } }),
    NOW,
  );
  assert.equal(badReporter.ok, false);
  assert.match(badReporter.error, /reporter_location\.latitude/);
});

test("report: bad values are refused", async () => {
  const key = await newKey();
  const cases = [
    [{ reporter_call: "no" }, /reporter_call/],
    [{ heard_call: "bad call!" }, /heard_call/],
    [{ heard_at: "17/11/2026 11:59" }, /heard_at/],
    [{ heard_at: "2026-11-17T13:00:00Z" }, /heard_at/],
    [{ frequency_khz: 14074.5 }, /frequency_khz/],
    [{ frequency_khz: 0 }, /frequency_khz/],
    [{ frequency_khz: "14074" }, /frequency_khz/],
    [{ mode: "" }, /mode/],
    [{ mode: " FT8" }, /mode/],
  ];
  for (const [change, pattern] of cases) {
    const result = validateReport(await signed(key, { ...REPORT_FIELDS, ...change }), NOW);
    assert.equal(result.ok, false, JSON.stringify(change));
    assert.match(result.error, pattern);
  }
});

test("report: a public key or signature of the wrong size is refused", async () => {
  const key = await newKey();
  const body = await signed(key, REPORT_FIELDS);
  const shortKey = validateReport({ ...body, public_key: encodeBase64(new Uint8Array(16)) }, NOW);
  assert.equal(shortKey.ok, false);
  assert.match(shortKey.error, /public_key/);
  const shortSig = validateReport({ ...body, signature: encodeBase64(new Uint8Array(32)) }, NOW);
  assert.equal(shortSig.ok, false);
  assert.match(shortSig.error, /signature/);
});

test("report: a body that is not an object is refused", () => {
  assert.equal(validateReport(null, NOW).ok, false);
  assert.equal(validateReport([], NOW).ok, false);
  assert.equal(validateReport("text", NOW).ok, false);
});

// ---- removal validation ----------------------------------------------------

test("remove: a signed removal is accepted", async () => {
  const key = await newKey();
  const body = await signed(key, { reporter_call: "K1ABC", heard_call: "DL1XYZ", signed_at: "2026-11-17T12:00:00Z" });
  const result = validateRemove(body, Date.parse("2026-11-17T12:00:00Z"));
  assert.equal(result.ok, true, result.error);
  assert.equal(result.reporterCall, "K1ABC");
  assert.equal(result.heardCall, "DL1XYZ");
  assert.equal(await verifySignature(result.publicKey, result.signature, result.message), true);
});

test("remove: extra fields and missing fields are refused", async () => {
  const key = await newKey();
  const body = await signed(key, { reporter_call: "K1ABC", heard_call: "DL1XYZ", signed_at: "2026-11-17T12:00:00Z" });
  assert.equal(validateRemove({ ...body, latitude: 51.5 }).ok, false);
  assert.equal(validateRemove({ reporter_call: "K1ABC", public_key: body.public_key, signature: body.signature }).ok, false);
});
