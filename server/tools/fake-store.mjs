// In-memory stand-in for the Worker's store (src/store.ts), for the tests and tools.
// Every store call waits for the previous one to finish, so calls run one at a time,
// the way a Durable Object runs its events. The claim and booking calls use the same
// operations as src/objects.js, so the in-process tests exercise the real logic.
// `slow` adds random latency, like KV, to shake out ordering bugs.
// The maps are exposed (recs, owners, relays, areas) so tests can seed and inspect them.
import { areaOps, callsignOps } from "../src/objects.js";
import { Mesh, meshOp } from "../src/mesh.js";

export function fakeStore({ slow = false } = {}) {
  const recs = new Map();
  const owners = new Map();
  const relays = new Map();
  const areas = new Map();
  // The relay mesh runs in-process; plans are signed with a stand-in (the real signing is tested in mesh.test.mjs).
  const mesh = new Mesh({ sign: async (t) => `test-signature:${t.length}` });
  let chain = Promise.resolve();
  const lag = () => (slow ? new Promise((r) => setTimeout(r, Math.random() * 4)) : Promise.resolve());
  /** Runs fn after every earlier call has finished. */
  const one = (fn) => {
    const run = chain.then(async () => {
      await lag();
      const v = await fn();
      await lag();
      return v;
    });
    chain = run.catch(() => {});
    return run;
  };
  const copy = (v) => (v === undefined ? null : structuredClone(v));

  /** Storage for one callsign object: the owner and relay records of that callsign. */
  const callsignStorage = (name) => {
    const mapFor = (key) => (key === "owner" ? owners : relays);
    return {
      async get(key) { return copy(mapFor(key).get(name)); },
      async put(key, value) { mapFor(key).set(name, structuredClone(value)); },
    };
  };
  /** Storage for one area object. */
  const areaStorage = (area) => ({
    async get() { return copy(areas.get(area)); },
    async put(_key, value) { areas.set(area, structuredClone(value)); },
  });

  return {
    mesh,
    meshOp: (op, args) => one(async () => meshOp(mesh, op, args, args.nowMs)),
    // The test store has no socket of its own: it answers "authenticated" and tools/serve-local.mjs runs the socket.
    meshSocket: async (call) => new Response(JSON.stringify({ call }), { status: 200 }),
    recs,
    owners,
    relays,
    areas,
    get: (r, h) => one(() => copy(recs.get(`${r}>${h}`))),
    put: (rec) => one(() => { recs.set(`${rec.reporter_call}>${rec.heard_call}`, structuredClone(rec)); }),
    remove: (r, h) => one(() => { recs.delete(`${r}>${h}`); }),
    listAll: () => one(() => [...recs.values()].map((x) => structuredClone(x))),
    claimOwner: (reporter, publicKey, nowMs) =>
      one(() => callsignOps.claimOwner(callsignStorage(reporter), { publicKey, nowMs })),
    claimRemoval: (reporter, heard, publicKey, signedAtMs, nowMs) =>
      one(() => callsignOps.claimRemoval(callsignStorage(reporter), { publicKey, heardCall: heard, signedAtMs, nowMs })),
    claimRelay: (callsign, area, publicKey, nowMs) =>
      one(() => callsignOps.claimRelay(callsignStorage(callsign), { callsign, area, publicKey, nowMs })),
    getRelay: (callsign) => one(() => callsignOps.getRelay(callsignStorage(callsign))),
    joinArea: (area, callsign, nowMs) => one(() => areaOps.join(areaStorage(area), { callsign, nowMs })),
    leaveArea: (area, callsign) =>
      one(async () => {
        await areaOps.leave(areaStorage(area), { callsign });
        await areaOps.leave(areaStorage("world-slots"), { callsign });
      }),
    // One object for the whole world, like src/store.ts: HF transmit slots are global.
    bookSchedule: (area, relay, requests, nowMs) =>
      one(() => areaOps.book(areaStorage("world-slots"), { area, relay, requests, nowMs })),
  };
}
