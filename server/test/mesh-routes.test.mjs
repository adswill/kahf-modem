// The relay mesh over HTTPS: who may call, what may be sent, and a full route-and-poll round.
import { test } from "node:test";
import assert from "node:assert/strict";
import { register } from "node:module";

register("data:text/javascript," + encodeURIComponent(`
import { readFile } from "node:fs/promises";
export async function load(url, context, nextLoad) {
  if (url.endsWith(".ts")) return { format: "module", source: await readFile(new URL(url), "utf8"), shortCircuit: true };
  return nextLoad(url, context);
}`));
const { handleRequest } = await import("../src/routes.js");
const { fakeStore } = await import("../tools/fake-store.mjs");
const { generateKeypair, signText } = await import("../src/sign.js");
const { h24 } = await import("../src/router.js");

const NOW = Date.parse("2026-11-19T10:00:00Z");

async function relayWithKey(store, call) {
  const kp = await generateKeypair();
  store.relays.set(call, { callsign: call, area: "54N25E", public_key: kp.publicKey, refreshed_at: NOW - 1000 });
  return kp;
}

async function post(store, path, body) {
  const res = await handleRequest(new Request("https://kahf.test" + path, { method: "POST", body: JSON.stringify(body) }), store, NOW);
  return { status: res.status, body: await res.json() };
}

/** A signed frames call. */
async function frames(store, call, kp, list, { ts = Math.floor(NOW / 1000), sign = true } = {}) {
  const frames_json = JSON.stringify(list);
  const signature = sign ? await signText(kp.seed, `mesh1|${call}|${ts}|frames|${frames_json}`) : "A".repeat(86) + "==";
  return post(store, "/mesh/frames", { relay: call, ts, public_key: kp.publicKey, signature, frames_json });
}

async function poll(store, call, kp) {
  const ts = Math.floor(NOW / 1000);
  return post(store, "/mesh/poll", { relay: call, ts, public_key: kp.publicKey, signature: await signText(kp.seed, `mesh1|${call}|${ts}|poll`) });
}

test("a registered relay routes a message to the relay that heard the receiver, and that relay polls it", async () => {
  const store = fakeStore();
  const a = await relayWithKey(store, "AA1AAA");
  const b = await relayWithKey(store, "BB1BBB");
  const dest = h24("ZZ9ZZZ");
  assert.equal((await frames(store, "BB1BBB", b, [{ t: "heard", dest, snr: 6, lane: 5 }])).status, 200);
  const r = await frames(store, "AA1AAA", a, [{ t: "submit", id: "m1", dest, size: 80 }]);
  assert.equal(r.status, 200);
  assert.equal(r.body.replies[0].t, "routed");
  assert.equal(r.body.replies[0].via, "BB1BBB");
  const got = await poll(store, "BB1BBB", b);
  assert.deepEqual(got.body.frames.map((f) => f.t), ["deliver"]);
  assert.equal(got.body.frames[0].id, "m1");
  assert.equal(got.body.frames[0].size, 80);
  assert.equal((await poll(store, "BB1BBB", b)).body.frames.length, 0);
});

test("who may call: a bad signature, an old time, an unregistered relay and another relay's key are refused", async () => {
  const store = fakeStore();
  const a = await relayWithKey(store, "AA1AAA");
  const other = await generateKeypair();
  assert.equal((await frames(store, "AA1AAA", a, [{ t: "beat" }], { sign: false })).status, 401);
  assert.equal((await frames(store, "AA1AAA", a, [{ t: "beat" }], { ts: Math.floor(NOW / 1000) - 1200 })).status, 400);
  assert.equal((await frames(store, "CC1CCC", other, [{ t: "beat" }])).status, 404);
  assert.equal((await frames(store, "AA1AAA", other, [{ t: "beat" }])).status, 403);
  assert.equal((await post(store, "/mesh/poll", { relay: "AA1AAA", ts: 1, public_key: a.publicKey, signature: "x" })).status, 400);
});

test("frames carry ids and sizes only: any field that could hold message content is refused", async () => {
  const store = fakeStore();
  const a = await relayWithKey(store, "AA1AAA");
  for (const key of ["body", "text", "subject", "payload", "content", "data", "message"]) {
    const r = await frames(store, "AA1AAA", a, [{ t: "submit", id: "x", dest: 1, size: 1, [key]: "hello" }]);
    assert.equal(r.status, 400, key);
    assert.match(r.body.error, /never message content/);
  }
  assert.equal((await frames(store, "AA1AAA", a, "nope")).status, 400);
  assert.equal((await frames(store, "AA1AAA", a, Array.from({ length: 51 }, () => ({ t: "beat" })))).status, 400);
});

test("the WebSocket endpoint needs an upgrade and a valid signed hello", async () => {
  const store = fakeStore();
  const a = await relayWithKey(store, "AA1AAA");
  const plain = await handleRequest(new Request("https://kahf.test/mesh/ws"), store, NOW);
  assert.equal(plain.status, 426);
  const ts = Math.floor(NOW / 1000);
  const q = (sig) => `?relay=AA1AAA&ts=${ts}&key=${encodeURIComponent(a.publicKey)}&sig=${encodeURIComponent(sig)}`;
  const bad = await handleRequest(new Request("https://kahf.test/mesh/ws" + q("A".repeat(86) + "=="), { headers: { upgrade: "websocket" } }), store, NOW);
  assert.equal(bad.status, 401);
  const good = await handleRequest(new Request("https://kahf.test/mesh/ws" + q(await signText(a.seed, `mesh1|AA1AAA|${ts}|ws`)), { headers: { upgrade: "websocket" } }), store, NOW);
  // The test store has no WebSocket of its own: it answers once the hello was authenticated.
  assert.equal(good.status, 200);
  assert.deepEqual(await good.json(), { call: "AA1AAA" });
});

test("the old queued-message routes are gone", async () => {
  const store = fakeStore();
  for (const path of ["/msg/submit", "/msg/pull", "/msg/receipt", "/relay/heard"]) {
    const res = await handleRequest(new Request("https://kahf.test" + path, { method: "POST", body: "{}" }), store, NOW);
    assert.equal(res.status, 404, path);
  }
  assert.equal((await handleRequest(new Request("https://kahf.test/relay/wanted"), store, NOW)).status, 404);
});
