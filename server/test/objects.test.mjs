// The Durable Object classes, driven directly with an in-memory storage: the POST
// { op, args } wrapper, the storage handling, and the claim rules through it.
// The fake store in tools/fake-store.mjs covers the logic; this covers the wrapper.
import { test } from "node:test";
import assert from "node:assert/strict";
import { CallsignObject, AreaObject } from "../src/objects.js";

const NOW = Date.parse("2026-11-17T12:00:00Z");

function memoryStorage() {
  const map = new Map();
  return {
    async get(key) { return map.has(key) ? structuredClone(map.get(key)) : undefined; },
    async put(key, value) { map.set(key, structuredClone(value)); },
  };
}

function call(object, op, args) {
  return object.fetch(new Request("https://object.internal/op", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ op, args }),
  }));
}

test("callsign object: a second key is refused, the first key may refresh", async () => {
  const obj = new CallsignObject({ storage: memoryStorage() });
  const first = await (await call(obj, "claimOwner", { publicKey: "A", nowMs: NOW })).json();
  assert.deepEqual(first, { ok: true });
  const second = await call(obj, "claimOwner", { publicKey: "B", nowMs: NOW + 1000 });
  assert.equal(second.status, 200);
  assert.equal((await second.json()).ok, false);
  assert.equal((await (await call(obj, "claimOwner", { publicKey: "A", nowMs: NOW + 2000 })).json()).ok, true);
});

test("callsign object: relay registration reports the previous area of a move", async () => {
  const obj = new CallsignObject({ storage: memoryStorage() });
  await call(obj, "claimRelay", { callsign: "K1ABC", area: "N51W1", publicKey: "A", nowMs: NOW });
  const moved = await (await call(obj, "claimRelay", { callsign: "K1ABC", area: "N52W1", publicKey: "A", nowMs: NOW })).json();
  assert.deepEqual(moved, { ok: true, previousArea: "N51W1" });
  const reg = await (await call(obj, "getRelay", {})).json();
  assert.equal(reg.area, "N52W1");
});

test("area object: a second booking of the same channel is pushed clear of the first", async () => {
  const obj = new AreaObject({ storage: memoryStorage() });
  const nowS = NOW / 1000;
  for (const relay of ["K1AAA", "K1BBB"]) {
    await call(obj, "join", { callsign: relay, nowMs: NOW });
  }
  const req = (relay) => [{ relay, channel: 0, duration: 30, direction: "tx", urgent: false }];
  const a = await (await call(obj, "book", { area: "N51W1", relay: "K1AAA", requests: req("K1AAA"), nowMs: NOW })).json();
  const b = await (await call(obj, "book", { area: "N51W1", relay: "K1BBB", requests: req("K1BBB"), nowMs: NOW })).json();
  assert.equal(a.ok, true);
  assert.equal(b.ok, true);
  const [x] = a.slots;
  const [y] = b.slots;
  assert.ok(y.start >= x.start + x.duration || x.start >= y.start + y.duration, "same channel slots overlap");
  assert.ok(x.start >= nowS && y.start >= nowS);
});

test("durable object wrapper: unknown operation, wrong method, and bad body", async () => {
  const obj = new CallsignObject({ storage: memoryStorage() });
  assert.equal((await call(obj, "claimEverything", {})).status, 404);
  assert.equal((await call(obj, "__proto__", {})).status, 404);
  const get = await obj.fetch(new Request("https://object.internal/op", { method: "GET" }));
  assert.equal(get.status, 405);
  const bad = await obj.fetch(new Request("https://object.internal/op", { method: "POST", body: "not json" }));
  assert.equal(bad.status, 400);
});
