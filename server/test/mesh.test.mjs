// The mesh brain: sockets, routing, signed plans, no bodies. Pure logic under Node.
import { test } from "node:test";
import assert from "node:assert/strict";
import { Mesh, planText, LOOKAHEAD_EPOCHS } from "../src/mesh.js";
import { generateKeypair, signText, verifyText } from "../src/sign.js";
import { h24 } from "../src/router.js";
import { epochOf, plan } from "../src/planner.js";

const sock = () => {
  const s = { sent: [], closed: false, send(m) { s.sent.push(m); }, close() { s.closed = true; } };
  return s;
};

async function setup(seedNo = 7) {
  const kp = await generateKeypair();
  const mesh = new Mesh({ sign: (t) => signText(kp.seed, t), seed: seedNo });
  return { kp, mesh };
}

test("signatures verify, and a changed plan or the wrong key does not", async () => {
  const a = await generateKeypair(), b = await generateKeypair();
  const sig = await signText(a.seed, "P1|5|7|1:0:0");
  assert.equal(await verifyText(a.publicKey, "P1|5|7|1:0:0", sig), true);
  assert.equal(await verifyText(a.publicKey, "P1|5|7|1:0:1", sig), false);
  assert.equal(await verifyText(b.publicKey, "P1|5|7|1:0:0", sig), false);
  assert.equal(await verifyText("not base64!", "x", sig), false);
});

test("plans for the next four epochs are signed, identical to the planner, and pushed once", async () => {
  const { kp, mesh } = await setup();
  const a = sock(), b = sock();
  mesh.connect("AA1AAA", a, 1000);
  mesh.connect("BB1BBB", b, 1000);
  mesh.onFrame("AA1AAA", { t: "queue", bytes: 500, urgency: 1, items: 2 }, 1000);
  mesh.onFrame("BB1BBB", { t: "queue", bytes: 900, urgency: 0, items: 1 }, 1000);
  const now = 120_000;
  const sent = await mesh.publish(now);
  const first = epochOf(now / 1000) + 1;
  assert.deepEqual(sent, Array.from({ length: LOOKAHEAD_EPOCHS }, (_, i) => first + i));
  assert.equal(a.sent.filter((m) => m.t === "plan").length, LOOKAHEAD_EPOCHS);
  for (const m of a.sent.filter((x) => x.t === "plan")) {
    const want = planText(plan(m.epoch, 7, [{ id: h24("AA1AAA"), queueBytes: 500, urgency: 1 }, { id: h24("BB1BBB"), queueBytes: 900, urgency: 0 }]), 7);
    assert.equal(m.plan, want);
    assert.equal(await verifyText(kp.publicKey, m.plan, m.sig), true);
  }
  // Nothing is pushed twice; a new epoch adds one.
  assert.deepEqual(await mesh.publish(now + 1000), []);
  assert.deepEqual(await mesh.publish(now + 12_000), [first + LOOKAHEAD_EPOCHS]);
});

test("an idle relay holds no cells and no two relays share a lane and slot", async () => {
  const { mesh } = await setup();
  const a = sock(), b = sock(), idle = sock();
  mesh.connect("AA1AAA", a, 0);
  mesh.connect("BB1BBB", b, 0);
  mesh.connect("CC1CCC", idle, 0);
  mesh.onFrame("AA1AAA", { t: "queue", bytes: 1500, urgency: 0, items: 3 }, 0);
  mesh.onFrame("BB1BBB", { t: "queue", bytes: 1500, urgency: 0, items: 3 }, 0);
  mesh.onFrame("CC1CCC", { t: "queue", bytes: 0, urgency: 0, items: 0 }, 0);
  await mesh.publish(60_000);
  const text = a.sent.find((m) => m.t === "plan").plan;
  const cells = text.split("|")[3].split(",").filter(Boolean).map((c) => c.split(":"));
  assert.ok(cells.length > 0);
  assert.ok(!cells.some((c) => Number(c[0]) === h24("CC1CCC")), "the idle relay got a cell");
  const used = new Set(cells.map((c) => `${c[1]}/${c[2]}`));
  assert.equal(used.size, cells.length, "a lane and slot was given twice");
});

test("a message is routed to the relay that heard the receiver, with the sender's relay told; a receipt goes back", async () => {
  const { mesh } = await setup();
  const a = sock(), b = sock();
  mesh.connect("AA1AAA", a, 0);
  mesh.connect("BB1BBB", b, 0);
  mesh.onFrame("BB1BBB", { t: "heard", dest: h24("ZZ9ZZZ"), snr: 6, lane: 5 }, 1000);
  const r = mesh.onFrame("AA1AAA", { t: "submit", id: "m1", dest: h24("ZZ9ZZZ"), size: 90 }, 2000);
  assert.equal(r.t, "routed");
  assert.equal(r.via, "BB1BBB");
  const got = b.sent.find((m) => m.t === "deliver");
  assert.deepEqual({ id: got.id, dest: got.dest, size: got.size, from: got.from }, { id: "m1", dest: h24("ZZ9ZZZ"), size: 90, from: "AA1AAA" });
  // Same id again: a duplicate, not delivered twice.
  assert.equal(mesh.onFrame("AA1AAA", { t: "submit", id: "m1", dest: h24("ZZ9ZZZ"), size: 90 }, 3000).t, "dup");
  assert.equal(b.sent.filter((m) => m.t === "deliver").length, 1);
  mesh.onFrame("BB1BBB", { t: "receipt", id: "m1", status: "delivered" }, 4000);
  const rc = a.sent.find((m) => m.t === "receipt");
  assert.equal(rc.id, "m1");
  assert.equal(rc.status, "delivered");
  assert.equal(rc.dest, h24("ZZ9ZZZ"));
  assert.equal(rc.relay, "BB1BBB");
});

test("an unknown receiver starts paging; the first reply ends it", async () => {
  const { mesh } = await setup();
  const a = sock(), b = sock();
  mesh.connect("AA1AAA", a, 0);
  mesh.connect("BB1BBB", b, 0);
  const r = mesh.onFrame("AA1AAA", { t: "submit", id: "m2", dest: h24("NEW1AAA"), size: 10 }, 1000);
  assert.equal(r.t, "unknown");
  assert.equal(mesh.router.pageStatus("NEW1AAA"), "paging");
  const f = mesh.onFrame("BB1BBB", { t: "here", dest: h24("NEW1AAA"), snr: 3, lane: 6 }, 2000);
  assert.equal(f.t, "found");
  assert.equal(f.via, "BB1BBB");
  assert.equal(mesh.router.pageStatus("NEW1AAA"), "found");
});

test("one socket per relay key; a dead relay is dropped and its frames wait for the poll", async () => {
  const { mesh } = await setup();
  const first = sock(), second = sock();
  mesh.connect("AA1AAA", first, 0);
  mesh.connect("AA1AAA", second, 1000);
  assert.ok(first.closed);
  assert.deepEqual(first.sent.at(-1), { t: "kicked" });
  mesh.connect("BB1BBB", sock(), 1000);
  // AA1AAA stops sending heartbeats; BB1BBB is online. Delivering to AA1AAA while it has a socket pushes.
  mesh.onFrame("AA1AAA", { t: "heard", dest: h24("ZZ9ZZZ"), snr: 4, lane: 5 }, 1000);
  mesh.tick(1000 + 76_000);
  assert.ok(second.closed);
  mesh.hub.push("AA1AAA", { t: "deliver", id: "x", dest: 1, size: 1, from: "BB1BBB" }, 80_000);
  assert.deepEqual(mesh.poll("AA1AAA", 90_000).map((m) => m.id), ["x"]);
  assert.deepEqual(mesh.poll("AA1AAA", 91_000), []);
});

test("a frame that carries message text is refused", async () => {
  const { mesh } = await setup();
  mesh.connect("AA1AAA", sock(), 0);
  assert.throws(() => mesh.hub.push("AA1AAA", { t: "deliver", id: "m", text: "hello" }, 0), /would carry content/);
});

test("a relay that only uses HTTPS is routed to, planned for, and polled", async () => {
  const { mesh } = await setup();
  const sockRelay = sock();
  mesh.connect("AA1AAA", sockRelay, 0);
  // BB1BBB has no socket: it calls with frames and polls.
  mesh.httpFrames("BB1BBB", [{ t: "queue", bytes: 700, urgency: 0, items: 1 }, { t: "heard", dest: h24("ZZ9ZZZ"), snr: 5, lane: 5 }], 1000);
  const r = mesh.onFrame("AA1AAA", { t: "submit", id: "h1", dest: h24("ZZ9ZZZ"), size: 40 }, 2000);
  assert.equal(r.via, "BB1BBB");
  await mesh.publish(60_000);
  const waiting = mesh.poll("BB1BBB", 61_000);
  assert.ok(waiting.some((m) => m.t === "deliver" && m.id === "h1"));
  assert.ok(waiting.some((m) => m.t === "plan"));
  // Silent for over 30 s: no longer online, so no longer routed to.
  mesh.tick(100_000);
  assert.equal(mesh.router.route(h24("ZZ9ZZZ"), 100_000).state, "unknown");
});

test("a message blob passes through to the delivering relay and is kept nowhere", async () => {
  const { mesh } = await setup();
  const a = sock(), b = sock();
  mesh.connect("AA1AAA", a, 0);
  mesh.connect("BB1BBB", b, 0);
  mesh.onFrame("BB1BBB", { t: "heard", dest: h24("ZZ9ZZZ"), snr: 6, lane: 5 }, 1000);
  mesh.onFrame("AA1AAA", { t: "submit", id: "m9", dest: h24("ZZ9ZZZ"), size: 5, blob: "aGVsbG8=" }, 2000);
  assert.equal(b.sent.find((m) => m.t === "deliver").blob, "aGVsbG8=");
  // Too big to pass: delivered without it (the relay then asks again), never truncated.
  mesh.onFrame("AA1AAA", { t: "submit", id: "m10", dest: h24("ZZ9ZZZ"), size: 5, blob: "x".repeat(20_000) }, 2500);
  assert.equal(b.sent.filter((m) => m.t === "deliver").at(-1).blob, undefined);
  // Nothing of it remains in the mesh after the receipt.
  mesh.onFrame("BB1BBB", { t: "receipt", id: "m9", status: "delivered" }, 3000);
  assert.ok(!JSON.stringify([...mesh.origin.entries()]).includes("aGVsbG8="));
});
