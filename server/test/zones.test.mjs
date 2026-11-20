// Unit tests for busy zones, channel pools and relay schedules in src/core.js.
// Run with: npm test (node --test). No network, no dependencies.
import { test } from "node:test";
import assert from "node:assert/strict";
import { BAND_CHANNELS, BUSY_WINDOW_S, busyZones, makeSchedule, poolPlan } from "../src/core.js";

const NOW = 1_800_000_000;
// Last channel of the band; with 3 channels this is 2, and channels 0..LAST are all there are.
const LAST = BAND_CHANNELS - 1;

test("busy zones: reports older than 10 minutes are ignored", () => {
  const reports = [
    // The band has only a few channels, so the stale reports reuse channel 0.
    { channel: 0, area: "AB12", time: NOW - BUSY_WINDOW_S - 1, level: 12 },
    { channel: 1, area: "AB12", time: NOW - BUSY_WINDOW_S, level: 12 },
    { channel: LAST, area: "AB12", time: NOW - BUSY_WINDOW_S + 1, level: 12 },
    { channel: 0, area: "AB12", time: NOW - 60 * 60, level: 12 },
  ];
  assert.deepEqual(busyZones(reports, NOW), { AB12: [LAST] });
});

test("busy zones: groups channels per area, sorted and unique", () => {
  const reports = [
    { channel: LAST, area: "XY", time: NOW, level: 1 },
    { channel: 0, area: "XY", time: NOW - 10, level: 1 },
    { channel: 0, area: "XY", time: NOW - 20, level: 1 },
    { channel: 1, area: "ZZ", time: NOW, level: 1 },
  ];
  assert.deepEqual(busyZones(reports, NOW), { XY: [0, LAST], ZZ: [1] });
});

test("busy zones: malformed and far-future reports are skipped", () => {
  const reports = [
    { channel: BAND_CHANNELS, area: "AB", time: NOW, level: 1 },
    { channel: -1, area: "AB", time: NOW, level: 1 },
    { channel: 1, area: "", time: NOW, level: 1 },
    { channel: 1, area: "AB", time: NOW + 3600, level: 1 },
    { channel: 1, area: "AB", time: NOW, level: NaN },
    null,
    { channel: LAST, area: "AB", time: NOW, level: 1 },
  ];
  assert.deepEqual(busyZones(reports, NOW), { AB: [LAST] });
});

test("pools: lower third is online for listed areas, open pool for the rest", () => {
  const plan = poolPlan({ AB12: [1, 2] });
  assert.equal(plan.defaultPool, "open");
  assert.equal(plan.online.length, Math.floor(BAND_CHANNELS / 3));
  assert.equal(plan.online[0], 0);
  assert.equal(plan.open[0], plan.online.length);
  assert.equal(plan.open.at(-1), BAND_CHANNELS - 1);
  assert.deepEqual(plan.areas.AB12.channels, plan.online.filter((c) => c !== 1 && c !== 2));
  assert.equal(plan.areas.AB12.pool, "online");
});

test("schedule: no two relays in one area share a channel and time", () => {
  const relays = [
    { id: "r1", area: "AB12" },
    { id: "r2", area: "AB12" },
    { id: "r3", area: "AB12" },
    { id: "r4", area: "CD34" },
    { id: "r5", area: "CD34" },
  ];
  const requests = [];
  for (const relay of relays) {
    for (let k = 0; k < 3; k++) {
      requests.push({ relay: relay.id, channel: 0, duration: 60 + k * 30, direction: k % 2 ? "tx" : "rx", urgent: k === 2 });
    }
  }
  const schedule = makeSchedule(relays, requests, NOW);

  const byArea = new Map();
  for (const relay of relays) {
    const slots = schedule[relay.id];
    assert.equal(slots.length, 3);
    // one radio: a relay's own slots never overlap
    for (let i = 1; i < slots.length; i++) {
      assert.ok(slots[i - 1].start + slots[i - 1].duration <= slots[i].start);
    }
    for (const s of slots) {
      assert.ok(s.start >= NOW);
      if (!byArea.has(relay.area)) byArea.set(relay.area, []);
      byArea.get(relay.area).push({ ...s, relay: relay.id });
    }
  }
  for (const [, slots] of byArea) {
    for (let i = 0; i < slots.length; i++) {
      for (let j = i + 1; j < slots.length; j++) {
        const a = slots[i];
        const b = slots[j];
        if (a.channel !== b.channel) continue;
        const overlap = a.start < b.start + b.duration && b.start < a.start + a.duration;
        assert.equal(overlap, false, `${a.relay} and ${b.relay} overlap on channel ${a.channel}`);
      }
    }
  }
});

test("schedule: urgent requests are placed before others", () => {
  const relays = [{ id: "a", area: "AB" }, { id: "b", area: "AB" }];
  const requests = [
    { relay: "a", channel: 1, duration: 100, direction: "rx" },
    { relay: "b", channel: 1, duration: 100, direction: "rx", urgent: true },
  ];
  const schedule = makeSchedule(relays, requests, NOW);
  assert.equal(schedule.b[0].start, NOW);
  assert.equal(schedule.a[0].start, NOW + 100);
});

test("schedule: durations must be whole seconds from 1 to 300", () => {
  const relays = [{ id: "a", area: "AB" }];
  const at = (duration) => [{ relay: "a", channel: 0, duration, direction: "rx" }];
  for (const bad of [0, -5, 301, 1.5, "10", NaN, undefined]) {
    assert.throws(() => makeSchedule(relays, at(bad), NOW), RangeError, `duration ${bad} should be refused`);
  }
  assert.doesNotThrow(() => makeSchedule(relays, at(1), NOW));
  assert.doesNotThrow(() => makeSchedule(relays, at(300), NOW));
});

test("schedule: channels outside the band and unknown relays are refused", () => {
  const relays = [{ id: "a", area: "AB" }];
  assert.throws(
    () => makeSchedule(relays, [{ relay: "a", channel: BAND_CHANNELS, duration: 10, direction: "tx" }], NOW),
    RangeError,
  );
  assert.throws(
    () => makeSchedule(relays, [{ relay: "nobody", channel: 0, duration: 10, direction: "tx" }], NOW),
    RangeError,
  );
});
