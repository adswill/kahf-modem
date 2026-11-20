// Tests for the band segment, channel mapping, area keys and the register and
// schedule validators in src/core.js. Run with: npm test (node --test).
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  BAND_CHANNELS,
  BAND_SEGMENT_KHZ,
  CHANNEL_WIDTH_KHZ,
  MAX_SCHEDULE_REQUESTS,
  areaKey,
  busyZones,
  channelOf,
  encodeBase64,
  validateRegister,
  validateSchedule,
  zoneReports,
} from "../src/core.js";

const KEY = encodeBase64(new Uint8Array(32).fill(7));
const SIG = encodeBase64(new Uint8Array(64));

test("band: channel count is derived from the one segment constant", () => {
  const span = BAND_SEGMENT_KHZ.high - BAND_SEGMENT_KHZ.low;
  assert.equal(BAND_CHANNELS, Math.floor(span / CHANNEL_WIDTH_KHZ));
  assert.equal(BAND_CHANNELS, Math.floor((span * 10) / (CHANNEL_WIDTH_KHZ * 10)));
  assert.ok(BAND_CHANNELS > 0);
  assert.ok(Object.isFrozen(BAND_SEGMENT_KHZ));
});

test("band: channelOf maps the segment edges and rejects what lies outside", () => {
  assert.equal(channelOf(BAND_SEGMENT_KHZ.low), 0);
  // Every channel's own lower edge and centre map back to that channel.
  for (let n = 0; n < BAND_CHANNELS; n += 1) {
    assert.equal(channelOf(BAND_SEGMENT_KHZ.low + CHANNEL_WIDTH_KHZ * n), n);
    assert.equal(channelOf(BAND_SEGMENT_KHZ.low + CHANNEL_WIDTH_KHZ * (n + 0.5)), n);
  }
  assert.equal(channelOf(BAND_SEGMENT_KHZ.high - 1), BAND_CHANNELS - 1);
  assert.equal(channelOf(BAND_SEGMENT_KHZ.high), null);
  assert.equal(channelOf(BAND_SEGMENT_KHZ.low - 1), null);
  assert.equal(channelOf(NaN), null);
  assert.equal(channelOf("145000"), null);
});

test("area keys: one-degree squares named by the south-west corner", () => {
  assert.equal(areaKey(51.5074, -0.1278), "N51W1");
  assert.equal(areaKey(-33.9, 151.2), "S34E151");
  assert.equal(areaKey(0, 0), "N0E0");
  assert.equal(areaKey(-0.5, -0.5), "S1W1");
});

test("zone reports: out-of-band and undated records are skipped", () => {
  const base = { reporter_location: { mode: "precise", latitude: 51.5, longitude: -0.1 }, heard_at: "2026-11-17T11:59:00Z" };
  const reports = zoneReports([
    { ...base, frequency_khz: BAND_SEGMENT_KHZ.low + CHANNEL_WIDTH_KHZ * 1.5 },
    { ...base, frequency_khz: BAND_SEGMENT_KHZ.high + 10 },
    { ...base, frequency_khz: 145_000, heard_at: "not a time" },
    { ...base, frequency_khz: 145_000, reporter_location: null },
    null,
  ]);
  assert.deepEqual(reports, [{ channel: 1, area: "N51W1", time: Date.parse("2026-11-17T11:59:00Z") / 1000 }]);
  assert.deepEqual(zoneReports("nope"), []);
});

test("busy zones: level is optional, but a non-finite level is still skipped", () => {
  const NOW = 1_800_000_000;
  assert.deepEqual(busyZones([{ channel: 2, area: "N51W1", time: NOW }], NOW), { N51W1: [2] });
  assert.deepEqual(busyZones([{ channel: 2, area: "N51W1", time: NOW, level: NaN }], NOW), {});
});

test("register: a well-formed relay registration is accepted and normalised", () => {
  const r = validateRegister({ area: "N51W1", callsign: "k1abc", public_key: KEY, signature: SIG });
  assert.equal(r.ok, true);
  assert.equal(r.callsign, "K1ABC");
  assert.equal(r.area, "N51W1");
  assert.equal(r.publicKey, KEY);
  assert.match(r.message, /"area":"N51W1"/);
  assert.doesNotMatch(r.message, /public_key|signature/);
});

test("register: callsign format and area format are checked", () => {
  const base = { area: "N51W1", callsign: "K1ABC", public_key: KEY, signature: SIG };
  assert.match(validateRegister({ ...base, callsign: "NOTACALL" }).error, /callsign/);
  assert.match(validateRegister({ ...base, callsign: "K1 ABC" }).error, /callsign/);
  assert.match(validateRegister({ ...base, area: "51W1" }).error, /area/);
  assert.match(validateRegister({ ...base, area: "N51W1; alert" }).error, /area/);
  assert.match(validateRegister({ ...base, public_key: "short" }).error, /public_key/);
  assert.match(validateRegister({ ...base, extra: 1 }).error, /not allowed/);
  assert.match(validateRegister({ area: "N51W1", callsign: "K1ABC" }).error, /missing field/);
});

test("schedule: a well-formed request is accepted with the relay normalised", () => {
  const s = validateSchedule({
    relay: "k1abc",
    requests: [{ channel: BAND_CHANNELS - 1, duration: 30, direction: "rx", urgent: true }],
    public_key: KEY,
    signature: SIG,
  });
  assert.equal(s.ok, true);
  assert.equal(s.relay, "K1ABC");
  assert.equal(s.requests.length, 1);
  assert.doesNotMatch(s.message, /public_key|signature/);
});

test("schedule: request list shape is checked", () => {
  const base = { relay: "K1ABC", public_key: KEY, signature: SIG };
  assert.match(validateSchedule({ ...base, requests: "x" }).error, /requests must be a list/);
  assert.match(validateSchedule({ ...base, requests: [] }).error, /requests must be a list/);
  assert.match(
    validateSchedule({ ...base, requests: Array.from({ length: MAX_SCHEDULE_REQUESTS + 1 }, () => ({ channel: 0, duration: 1, direction: "rx" })) }).error,
    /requests must be a list/,
  );
  assert.match(validateSchedule({ ...base, requests: [null] }).error, /requests\[0\] must be an object/);
  assert.match(validateSchedule({ ...base, requests: [{ channel: 0, duration: 1 }] }).error, /missing field: direction/);
  assert.match(validateSchedule({ ...base, relay: "bad call!", requests: [{ channel: 0, duration: 1, direction: "rx" }] }).error, /relay/);
});
