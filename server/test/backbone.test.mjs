// Tests for the HF backbone: /relay/heard, /msg/submit, /msg/pull, /msg/receipt,
// /relay/wanted, and worldwide slot planning. In-memory store, injectable clock.
import { test } from "node:test";
import assert from "node:assert/strict";
import { register } from "node:module";
import { canonicalJson, encodeBase64, makeSchedule, planSchedule } from "../src/core.js";
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

const T0 = Date.parse("2026-11-17T12:00:00Z");
const BASE = "https://kahf.test";
const enc = new TextEncoder();
const b64 = (text) => Buffer.from(text).toString("base64");

async function newKey() {
  const pair = await crypto.subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"]);
  const raw = new Uint8Array(await crypto.subtle.exportKey("raw", pair.publicKey));
  return { privateKey: pair.privateKey, publicKey: encodeBase64(raw) };
}
async function signed(key, fields) {
  const sig = new Uint8Array(await crypto.subtle.sign({ name: "Ed25519" }, key.privateKey, enc.encode(canonicalJson(fields))));
  return { ...fields, public_key: key.publicKey, signature: encodeBase64(sig) };
}
const post = (path, body) =>
  new Request(BASE + path, { method: "POST", headers: { "content-type": "application/json" }, body: typeof body === "string" ? body : JSON.stringify(body) });

/** A world: a store, a clock, and helpers to register relays and call routes. */
async function world(options = {}) {
  const store = fakeStore();
  let now = T0;
  const w = {
    store,
    advance: (ms) => { now += ms; },
    nowS: () => Math.floor(now / 1000),
    async call(path, body) {
      const res = await handleRequest(post(path, body), store, now, options);
      return { status: res.status, json: await res.json() };
    },
    async wanted() {
      const res = await handleRequest(new Request(BASE + "/relay/wanted"), store, now, options);
      return { status: res.status, json: await res.json() };
    },
    async relay(call, area = "N51W1") {
      const key = await newKey();
      const res = await handleRequest(post("/register", await signed(key, { area, callsign: call })), store, now);
      assert.equal(res.status, 200);
      return key;
    },
    /** Signed call as `relay` with `key`. */
    async as(key, path, fields) {
      return w.call(path, await signed(key, fields));
    },
  };
  return w;
}

const msg = (w, over = {}) => ({
  relay: "R1AAA", id: "K1ABC-1", origin: "K1ABC", dest: "DL1XYZ", kind: "chat", body: b64("hello"),
  created: w.nowS(), ttl_s: 3600, hops: 0, ...over,
});
const pullBody = (w, relay, callsigns) => ({ relay, callsigns, ts: w.nowS() });

// ---- authentication, shared by the four signed endpoints --------------------

const SIGNED = [
  ["/relay/heard", (w) => ({ relay: "R1AAA", heard: [{ callsign: "K1ABC", at: w.nowS() }], ts: w.nowS() })],
  ["/msg/submit", (w) => msg(w)],
  ["/msg/pull", (w) => pullBody(w, "R1AAA", ["K1ABC"])],
  ["/msg/receipt", () => ({ relay: "R1AAA", id: "K1ABC-1", status: "delivered" })],
];

for (const [path, make] of SIGNED) {
  test(`${path}: unsigned, bad signature, wrong key and unregistered relay are refused`, async () => {
    const w = await world();
    const key = await w.relay("R1AAA");
    const fields = make(w);
    // unsigned
    assert.equal((await w.call(path, fields)).status, 400);
    // signature of other content
    const forged = await signed(key, { ...fields, relay: "R1AAA" });
    forged.signature = (await signed(key, { other: 1 })).signature;
    assert.equal((await w.call(path, forged)).status, 401);
    // signed with a key that is not the registered one
    const stranger = await newKey();
    const wrong = await w.call(path, await signed(stranger, fields));
    assert.equal(wrong.status, 403);
    // a relay that never registered
    const lone = await newKey();
    const unreg = await w.call(path, await signed(lone, { ...fields, relay: "R9ZZZ" }));
    assert.equal(unreg.status, 404);
    // an expired registration counts as unregistered
    w.advance(49 * 3600 * 1000);
    const late = await w.call(path, await signed(key, make(w)));
    assert.equal(late.status, 404);
  });

  test(`${path}: not JSON, unknown field, and GET are refused`, async () => {
    const w = await world();
    const key = await w.relay("R1AAA");
    assert.equal((await w.call(path, "not json")).status, 400);
    const extra = await w.call(path, await signed(key, { ...make(w), surprise: 1 }));
    assert.equal(extra.status, 400);
    const res = await handleRequest(new Request(BASE + path), w.store, T0);
    assert.equal(res.status, 405);
  });
}

test("body size cap: an oversized request is refused with 413", async () => {
  const w = await world();
  const key = await w.relay("R1AAA");
  const big = await signed(key, msg(w, { body: "A".repeat(20_000) }));
  assert.equal((await w.call("/msg/submit", big)).status, 413);
});

// ---- /relay/heard -----------------------------------------------------------

test("/relay/heard records the latest relay per callsign and expires after 2 hours", async () => {
  const w = await world();
  const a = await w.relay("R1AAA");
  const b = await w.relay("R1BBB");
  const r1 = await w.as(a, "/relay/heard", { relay: "R1AAA", heard: [{ callsign: "k1abc", snr: -12, at: w.nowS() - 60 }, { callsign: "DL1XYZ", at: w.nowS() - 60 }], ts: w.nowS() });
  assert.deepEqual(r1.json, { ok: true, recorded: 2, ignored: 0 });
  assert.deepEqual((await w.store.idxGetHeard("K1ABC", T0)).heard, { callsign: "K1ABC", relay: "R1AAA", at: w.nowS() - 60, snr: -12 });
  // A newer hearing by another relay replaces it; an older one does not.
  await w.as(b, "/relay/heard", { relay: "R1BBB", heard: [{ callsign: "K1ABC", at: w.nowS() - 10 }], ts: w.nowS() });
  assert.equal((await w.store.idxGetHeard("K1ABC", T0)).heard.relay, "R1BBB");
  const old = await w.as(a, "/relay/heard", { relay: "R1AAA", heard: [{ callsign: "K1ABC", at: w.nowS() - 500 }], ts: w.nowS() });
  assert.equal(old.json.ignored, 1);
  assert.equal((await w.store.idxGetHeard("K1ABC", T0)).heard.relay, "R1BBB");
  // TTL 2 h from `at`.
  assert.ok((await w.store.idxGetHeard("K1ABC", T0 + 7000 * 1000)).heard);
  assert.equal((await w.store.idxGetHeard("K1ABC", T0 + 7300 * 1000)).heard, null);
  // An entry already older than 2 h is ignored.
  const stale = await w.as(a, "/relay/heard", { relay: "R1AAA", heard: [{ callsign: "W1OLD", at: w.nowS() - 7300 }], ts: w.nowS() });
  assert.equal(stale.json.recorded, 0);
});

test("/relay/heard: 200 entries are fine, 201 are refused; bad entries and bad ts are refused", async () => {
  const w = await world();
  const key = await w.relay("R1AAA");
  const entries = (n) => Array.from({ length: n }, (_, i) => ({ callsign: `K${i % 10}ABC`, at: w.nowS() }));
  const ok = await w.as(key, "/relay/heard", { relay: "R1AAA", heard: entries(200), ts: w.nowS() });
  assert.equal(ok.status, 200);
  assert.equal((await w.as(key, "/relay/heard", { relay: "R1AAA", heard: entries(201), ts: w.nowS() })).status, 400);
  assert.equal((await w.as(key, "/relay/heard", { relay: "R1AAA", heard: [], ts: w.nowS() })).status, 400);
  for (const bad of [
    { callsign: "not a call", at: w.nowS() },
    { callsign: "K1ABC", at: w.nowS() + 3600 },
    { callsign: "K1ABC", at: 1.5 },
    { callsign: "K1ABC", at: w.nowS(), snr: "loud" },
    { callsign: "K1ABC", at: w.nowS(), extra: 1 },
  ]) {
    assert.equal((await w.as(key, "/relay/heard", { relay: "R1AAA", heard: [bad], ts: w.nowS() })).status, 400, JSON.stringify(bad));
  }
  assert.equal((await w.as(key, "/relay/heard", { relay: "R1AAA", heard: entries(1), ts: w.nowS() - 3600 })).status, 400);
});

// ---- /msg/submit ------------------------------------------------------------

test("/msg/submit: the first submit of an id wins; a second relay gets duplicate and stores nothing", async () => {
  const w = await world();
  const a = await w.relay("R1AAA");
  const b = await w.relay("R1BBB");
  const first = await w.as(a, "/msg/submit", msg(w));
  assert.equal(first.status, 200);
  assert.equal(first.json.duplicate, false);
  const second = await w.as(b, "/msg/submit", msg(w, { relay: "R1BBB", body: b64("different body") }));
  assert.deepEqual(second.json, { ok: true, duplicate: true });
  const queue = (await w.store.msgPeek("DL1XYZ", T0)).messages;
  assert.equal(queue.length, 1);
  assert.equal(queue[0].body, b64("hello"));
  // Same relay again: still a duplicate.
  assert.equal((await w.as(a, "/msg/submit", msg(w))).json.duplicate, true);
  // Two concurrent submits: exactly one stores.
  const [x, y] = await Promise.all([
    w.as(a, "/msg/submit", msg(w, { id: "K1ABC-2" })),
    w.as(b, "/msg/submit", msg(w, { id: "K1ABC-2", relay: "R1BBB" })),
  ]);
  assert.deepEqual([x.json.duplicate, y.json.duplicate].sort(), [false, true]);
  assert.equal((await w.store.msgPeek("DL1XYZ", T0)).messages.length, 2);
});

test("/msg/submit: validation of ids, callsigns, kind, subject and numbers", async () => {
  const w = await world();
  const key = await w.relay("R1AAA");
  const bad = {
    "id without counter": { id: "K1ABC" },
    "id of another origin": { id: "W1XYZ-1" },
    "id with letters in counter": { id: "K1ABC-x" },
    "bad origin": { origin: "nope!", id: "NOPE-1" },
    "bad dest": { dest: "??" },
    "dest equals origin": { dest: "K1ABC" },
    "bad kind": { kind: "fax" },
    "body not base64": { body: "***" },
    "empty body": { body: "" },
    "subject too long": { subject: "s".repeat(101) },
    "subject with control character": { subject: "a\nb" },
    "urgent not boolean": { urgent: "yes" },
    "created in the future": { created: w.nowS() + 7200 },
    "created not a number": { created: "now" },
    "ttl not a number": { ttl_s: "long" },
    "negative hops": { hops: -1 },
    "too many hops": { hops: 17 },
  };
  for (const [name, over] of Object.entries(bad)) {
    const res = await w.as(key, "/msg/submit", msg(w, over));
    assert.equal(res.status, 400, `${name}: ${JSON.stringify(res.json)}`);
  }
  assert.equal((await w.store.idxWanted(T0)).wanted.length, 0);
  // Lower-case callsigns are normalised; subject and urgent are accepted.
  const ok = await w.as(key, "/msg/submit", msg(w, { id: "K1ABC-9", origin: "k1abc", dest: "dl1xyz/p", subject: "Hi", urgent: true }));
  assert.equal(ok.status, 200);
  const [queued] = (await w.store.msgPeek("DL1XYZ/P", T0)).messages;
  assert.deepEqual([queued.origin, queued.dest, queued.subject, queued.urgent], ["K1ABC", "DL1XYZ/P", "Hi", true]);
});

test("/msg/submit: body size limits are 1024 bytes for chat and 4096 for mail", async () => {
  const w = await world();
  const key = await w.relay("R1AAA");
  const chat = (n, id) => w.as(key, "/msg/submit", msg(w, { id, kind: "chat", body: b64("x".repeat(n)) }));
  const mail = (n, id) => w.as(key, "/msg/submit", msg(w, { id, kind: "mail", body: b64("y".repeat(n)) }));
  assert.equal((await chat(1024, "K1ABC-1")).status, 200);
  assert.equal((await chat(1025, "K1ABC-2")).status, 400);
  assert.equal((await mail(4096, "K1ABC-3")).status, 200);
  assert.equal((await mail(4097, "K1ABC-4")).status, 400);
  // A refused message does not burn its id.
  assert.equal((await chat(10, "K1ABC-2")).json.duplicate, false);
});

test("/msg/submit: ttl_s is clamped to 30 minutes .. 48 hours", async () => {
  const w = await world();
  const key = await w.relay("R1AAA");
  const lo = await w.as(key, "/msg/submit", msg(w, { id: "K1ABC-1", ttl_s: 1 }));
  const hi = await w.as(key, "/msg/submit", msg(w, { id: "K1ABC-2", ttl_s: 10 * 24 * 3600 }));
  assert.equal(lo.json.expires_at, w.nowS() + 1800);
  assert.equal(hi.json.expires_at, w.nowS() + 48 * 3600);
});

// ---- /msg/pull --------------------------------------------------------------

test("/msg/pull returns only messages for the callsigns the relay hears", async () => {
  const w = await world();
  const a = await w.relay("R1AAA");
  const b = await w.relay("R1BBB");
  await w.as(a, "/msg/submit", msg(w, { id: "K1ABC-1", dest: "DL1XYZ" }));
  await w.as(a, "/msg/submit", msg(w, { id: "K1ABC-2", dest: "F4AAA" }));
  const pulled = await w.as(b, "/msg/pull", pullBody(w, "R1BBB", ["DL1XYZ", "W9NONE"]));
  assert.equal(pulled.status, 200);
  assert.deepEqual(pulled.json.messages.map((m) => m.id), ["K1ABC-1"]);
  const m = pulled.json.messages[0];
  assert.deepEqual([m.origin, m.dest, m.kind, m.body, m.hops], ["K1ABC", "DL1XYZ", "chat", b64("hello"), 0]);
  // The other destination's message stays queued.
  assert.equal((await w.store.msgPeek("F4AAA", T0)).messages.length, 1);
  // A relay hearing nobody with mail gets nothing.
  const none = await w.as(b, "/msg/pull", pullBody(w, "R1BBB", ["W9NONE"]));
  assert.deepEqual(none.json, { messages: [], receipts: [] });
});

test("/msg/pull: handed message is not given to another relay, then becomes pullable after 10 minutes", async () => {
  const w = await world();
  const a = await w.relay("R1AAA");
  const b = await w.relay("R1BBB");
  await w.as(a, "/msg/submit", msg(w));
  const p1 = await w.as(a, "/msg/pull", pullBody(w, "R1AAA", ["DL1XYZ"]));
  assert.equal(p1.json.messages.length, 1);
  // Nobody else can take it, nor can the same relay take it twice.
  assert.equal((await w.as(b, "/msg/pull", pullBody(w, "R1BBB", ["DL1XYZ"]))).json.messages.length, 0);
  w.advance(9 * 60 * 1000 + 59_000);
  assert.equal((await w.as(b, "/msg/pull", pullBody(w, "R1BBB", ["DL1XYZ"]))).json.messages.length, 0);
  // The relay that crashed gets no receipt in time: any relay may take it.
  w.advance(1000);
  const p2 = await w.as(b, "/msg/pull", pullBody(w, "R1BBB", ["DL1XYZ"]));
  assert.deepEqual(p2.json.messages.map((m) => m.id), ["K1ABC-1"]);
  // The old holder can no longer report on it; the new one can.
  const late = await w.as(a, "/msg/receipt", { relay: "R1AAA", id: "K1ABC-1", status: "delivered" });
  assert.equal(late.status, 409);
  const ok = await w.as(b, "/msg/receipt", { relay: "R1BBB", id: "K1ABC-1", status: "delivered" });
  assert.equal(ok.status, 200);
});

test("/msg/pull: at most 20 per call, urgent first, then oldest first", async () => {
  const w = await world();
  const a = await w.relay("R1AAA");
  for (let i = 1; i <= 25; i++) {
    const res = await w.as(a, "/msg/submit", msg(w, { id: `K1ABC-${i}`, created: w.nowS() - 1000 + i, dest: i % 2 ? "DL1XYZ" : "F4AAA" }));
    assert.equal(res.status, 200);
  }
  await w.as(a, "/msg/submit", msg(w, { id: "K1ABC-99", created: w.nowS(), urgent: true }));
  const p = await w.as(a, "/msg/pull", pullBody(w, "R1AAA", ["DL1XYZ", "F4AAA"]));
  const ids = p.json.messages.map((m) => m.id);
  assert.equal(ids.length, 20);
  assert.equal(ids[0], "K1ABC-99");
  assert.deepEqual(ids.slice(1), Array.from({ length: 19 }, (_, i) => `K1ABC-${i + 1}`));
  // The rest comes on the next call, and nothing is handed twice.
  const p2 = await w.as(a, "/msg/pull", pullBody(w, "R1AAA", ["DL1XYZ", "F4AAA"]));
  assert.deepEqual(p2.json.messages.map((m) => m.id), Array.from({ length: 6 }, (_, i) => `K1ABC-${i + 20}`));
});

test("/msg/pull: callsign list limits and validation", async () => {
  const w = await world();
  const key = await w.relay("R1AAA");
  const calls = (n) => Array.from({ length: n }, (_, i) => `K${i % 10}A${String.fromCharCode(65 + Math.floor(i / 10))}`);
  assert.equal((await w.as(key, "/msg/pull", pullBody(w, "R1AAA", calls(100)))).status, 200);
  assert.equal((await w.as(key, "/msg/pull", pullBody(w, "R1AAA", calls(101)))).status, 400);
  assert.equal((await w.as(key, "/msg/pull", pullBody(w, "R1AAA", []))).status, 400);
  assert.equal((await w.as(key, "/msg/pull", pullBody(w, "R1AAA", ["bad call"]))).status, 400);
  assert.equal((await w.as(key, "/msg/pull", { relay: "R1AAA", callsigns: ["K1ABC"], ts: w.nowS() - 3600 })).status, 400);
});

// ---- /msg/receipt and expiry --------------------------------------------------

test("/msg/receipt removes the payload and gives the origin a receipt once", async () => {
  const w = await world();
  const a = await w.relay("R1AAA");
  const b = await w.relay("R1BBB");
  await w.as(a, "/msg/submit", msg(w, { subject: "S" }));
  assert.deepEqual((await w.wanted()).json.wanted, ["DL1XYZ"]);
  // Not handed yet: a receipt is refused.
  assert.equal((await w.as(a, "/msg/receipt", { relay: "R1AAA", id: "K1ABC-1", status: "delivered" })).status, 409);
  await w.as(a, "/msg/pull", pullBody(w, "R1AAA", ["DL1XYZ"]));
  // Another relay cannot close it while A holds it.
  assert.equal((await w.as(b, "/msg/receipt", { relay: "R1BBB", id: "K1ABC-1", status: "delivered" })).status, 409);
  const done = await w.as(a, "/msg/receipt", { relay: "R1AAA", id: "K1ABC-1", status: "delivered" });
  assert.deepEqual(done.json, { ok: true, duplicate: false });
  // Payload is gone from storage.
  assert.equal(JSON.stringify([...w.store.mailboxes.entries()]).includes(b64("hello")), false);
  assert.equal((await w.store.msgPeek("DL1XYZ", T0)).messages.length, 0);
  assert.deepEqual((await w.wanted()).json.wanted, []);
  // The origin gets the receipt on its next pull, once.
  const p = await w.as(b, "/msg/pull", pullBody(w, "R1BBB", ["K1ABC"]));
  assert.equal(p.json.receipts.length, 1);
  assert.deepEqual(p.json.receipts[0], { id: "K1ABC-1", dest: "DL1XYZ", status: "delivered", at: w.nowS() });
  assert.equal((await w.as(b, "/msg/pull", pullBody(w, "R1BBB", ["K1ABC"]))).json.receipts.length, 0);
  // A second receipt is a harmless duplicate; unknown ids are 404.
  assert.deepEqual((await w.as(a, "/msg/receipt", { relay: "R1AAA", id: "K1ABC-1", status: "delivered" })).json, { ok: true, duplicate: true });
  assert.equal((await w.as(a, "/msg/receipt", { relay: "R1AAA", id: "K1ABC-77", status: "delivered" })).status, 404);
  // A duplicate submit of a finished id does not bring the message back.
  assert.equal((await w.as(a, "/msg/submit", msg(w))).json.duplicate, true);
  assert.equal((await w.store.msgPeek("DL1XYZ", T0)).messages.length, 0);
});

test("/msg/receipt: failed with a reason, and field validation", async () => {
  const w = await world();
  const a = await w.relay("R1AAA");
  await w.as(a, "/msg/submit", msg(w));
  await w.as(a, "/msg/pull", pullBody(w, "R1AAA", ["DL1XYZ"]));
  for (const bad of [
    { status: "maybe" },
    { status: "failed", reason: "r".repeat(121) },
    { status: "failed", reason: 5 },
    { id: "nonsense" },
  ]) {
    assert.equal((await w.as(a, "/msg/receipt", { relay: "R1AAA", id: "K1ABC-1", status: "delivered", ...bad })).status, 400, JSON.stringify(bad));
  }
  assert.equal((await w.as(a, "/msg/receipt", { relay: "R1AAA", id: "K1ABC-1", status: "failed", reason: "no answer" })).status, 200);
  const p = await w.as(a, "/msg/pull", pullBody(w, "R1AAA", ["K1ABC"]));
  assert.deepEqual(p.json.receipts.map((r) => [r.status, r.reason]), [["failed", "no answer"]]);
});

test("receipts expire after 24 hours", async () => {
  const w = await world();
  const a = await w.relay("R1AAA");
  await w.as(a, "/msg/submit", msg(w, { ttl_s: 48 * 3600 }));
  await w.as(a, "/msg/pull", pullBody(w, "R1AAA", ["DL1XYZ"]));
  await w.as(a, "/msg/receipt", { relay: "R1AAA", id: "K1ABC-1", status: "delivered" });
  w.advance(24 * 3600 * 1000 + 1000);
  const key = await w.relay("R1CCC");
  const p = await w.as(key, "/msg/pull", pullBody(w, "R1CCC", ["K1ABC"]));
  assert.deepEqual(p.json.receipts, []);
});

test("an expired message is dropped, never handed out, and the origin gets a failed receipt", async () => {
  const w = await world();
  const a = await w.relay("R1AAA");
  await w.as(a, "/msg/submit", msg(w, { ttl_s: 1800 }));
  assert.deepEqual((await w.wanted()).json.wanted, ["DL1XYZ"]);
  w.advance(1800 * 1000);
  assert.deepEqual((await w.wanted()).json.wanted, [], "wanted drops it at expiry");
  const gone = await w.as(await w.relay("R1BBB"), "/msg/pull", pullBody(w, "R1BBB", ["DL1XYZ", "K1ABC"]));
  assert.equal(gone.json.messages.length, 0);
  assert.deepEqual(gone.json.receipts.map((r) => [r.id, r.status, r.reason]), [["K1ABC-1", "failed", "expired"]]);
  assert.equal(JSON.stringify([...w.store.mailboxes.entries()]).includes(b64("hello")), false);
});

// ---- /relay/wanted ----------------------------------------------------------

test("GET /relay/wanted is public, lists destinations sorted, and is capped at 500", async () => {
  const w = await world();
  const a = await w.relay("R1AAA");
  assert.deepEqual(await w.wanted(), { status: 200, json: { wanted: [] } });
  await w.as(a, "/msg/submit", msg(w, { id: "K1ABC-1", dest: "F4AAA" }));
  await w.as(a, "/msg/submit", msg(w, { id: "K1ABC-2", dest: "DL1XYZ" }));
  await w.as(a, "/msg/submit", msg(w, { id: "K1ABC-3", dest: "DL1XYZ" }));
  assert.deepEqual((await w.wanted()).json.wanted, ["DL1XYZ", "F4AAA"]);
  // POST is refused.
  assert.equal((await handleRequest(post("/relay/wanted", {}), w.store, T0)).status, 405);
  // Cap: fill the index directly.
  for (let i = 0; i < 600; i++) await w.store.idxWant(`K${i}ABC`, T0 + 3600_000);
  assert.equal((await w.wanted()).json.wanted.length, 500);
});

test("/relay/wanted keeps a destination while it still has a queued message", async () => {
  const w = await world();
  const a = await w.relay("R1AAA");
  await w.as(a, "/msg/submit", msg(w, { id: "K1ABC-1" }));
  await w.as(a, "/msg/submit", msg(w, { id: "K1ABC-2" }));
  await w.as(a, "/msg/pull", pullBody(w, "R1AAA", ["DL1XYZ"]));
  await w.as(a, "/msg/receipt", { relay: "R1AAA", id: "K1ABC-1", status: "delivered" });
  assert.deepEqual((await w.wanted()).json.wanted, ["DL1XYZ"]);
  await w.as(a, "/msg/receipt", { relay: "R1AAA", id: "K1ABC-2", status: "delivered" });
  assert.deepEqual((await w.wanted()).json.wanted, []);
});

// ---- rate cap ---------------------------------------------------------------

test("a relay that sends too many backbone calls in a minute gets 429, and the next minute is fine", async () => {
  const w = await world({ relayRateLimit: 5 });
  const key = await w.relay("R1AAA");
  const call = () => w.as(key, "/msg/pull", pullBody(w, "R1AAA", ["K1ABC"]));
  for (let i = 0; i < 5; i++) assert.equal((await call()).status, 200);
  const over = await call();
  assert.equal(over.status, 429);
  w.advance(60_000);
  assert.equal((await call()).status, 200);
  // Another relay has its own budget.
  const other = await w.relay("R1BBB");
  assert.equal((await w.as(other, "/msg/pull", pullBody(w, "R1BBB", ["K1ABC"]))).status, 200);
});

// ---- worldwide slot planning ----------------------------------------------------

const txReq = (relay, channel, duration, direction = "tx") => ({ relay, channel, duration, direction, urgent: false });

test("planSchedule: transmit slots on one channel never overlap, even in different areas", () => {
  const relays = [{ id: "R1", area: "N10E10" }, { id: "R2", area: "S30W60" }, { id: "R3", area: "N51W1" }];
  const requests = relays.map((r) => txReq(r.id, 9, 60));
  const slots = makeSchedule(relays, requests, 1_000_000);
  const all = relays.flatMap((r) => slots[r.id]).sort((a, b) => a.start - b.start);
  assert.equal(all.length, 3);
  for (let i = 1; i < all.length; i++) assert.ok(all[i].start >= all[i - 1].start + all[i - 1].duration);
  // Different channels are independent.
  const two = makeSchedule(relays.slice(0, 2), [txReq("R1", 9, 60), txReq("R2", 10, 60)], 1_000_000);
  assert.equal(two.R1[0].start, two.R2[0].start);
  // Receive slots do not occupy the air.
  const rx = makeSchedule(relays.slice(0, 2), [txReq("R1", 9, 60, "rx"), txReq("R2", 9, 60, "tx")], 1_000_000);
  assert.equal(rx.R1[0].start, rx.R2[0].start);
});

test("planSchedule: a request that overlaps a booked slot elsewhere is shifted, or refused when its latency budget is too short", () => {
  const relays = [{ id: "R1", area: "S30W60" }];
  const booked = [{ area: "N10E10", channel: 9, start: 1_000_000, duration: 120, direction: "tx" }];
  const shifted = makeSchedule(relays, [txReq("R1", 9, 30)], 1_000_000, booked);
  assert.equal(shifted.R1[0].start, 1_000_120);
  const refused = planSchedule(relays, [{ ...txReq("R1", 9, 30), latency_budget_s: 60 }], 1_000_000, booked);
  assert.equal(refused.slots.R1.length, 0);
  assert.equal(refused.unplaced[0].reason, "no free time inside the latency budget");
});

test("POST /schedule: relays in different areas get non-overlapping transmit slots on one channel", async () => {
  const w = await world();
  const a = await w.relay("R1AAA", "N10E10");
  const b = await w.relay("R1BBB", "S30W60");
  const req = (relay) => ({ relay, requests: [{ channel: 40, duration: 60, direction: "tx" }] });
  const x = (await w.as(a, "/schedule", req("R1AAA"))).json.slots[0];
  const y = (await w.as(b, "/schedule", req("R1BBB"))).json.slots[0];
  assert.ok(y.start >= x.start + x.duration || x.start >= y.start + y.duration, JSON.stringify([x, y]));
  assert.equal(x.start, w.nowS());
  // Replanning the first relay does not collide with the second.
  const x2 = (await w.as(a, "/schedule", req("R1AAA"))).json.slots[0];
  assert.ok(x2.start >= y.start + y.duration || x2.start + x2.duration <= y.start);
  // A relay that moves area, and a third relay, still never overlap on the air.
  await w.call("/register", await signed(b, { area: "N10E10", callsign: "R1BBB" }));
  const c = await w.relay("R1CCC", "N50E50");
  await w.as(c, "/schedule", req("R1CCC"));
  await w.as(b, "/schedule", req("R1BBB"));
  const world_ = (await w.store.areas.get("world-slots")).slots;
  const txs = Object.values(world_).flatMap((r) => r.slots).filter((s) => s.channel === 40 && s.direction === "tx");
  txs.sort((p, q) => p.start - q.start);
  assert.equal(txs.length, 3);
  for (let i = 1; i < txs.length; i++) assert.ok(txs[i].start >= txs[i - 1].start + txs[i - 1].duration, JSON.stringify(txs));
});
