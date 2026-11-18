// A simulated week (MESH-SPEC.md step 16). See soak-sim.mjs for what is real and what is a model.
import test from 'node:test';
import assert from 'node:assert/strict';
import { runWeek, EPOCH } from './soak-sim.mjs';

const MIN = 60e3;
const DAY = 86400e3;

// "Must be delivered": inside the 24 h lifetime there are at least three separate 10 minute
// moments when the sender can reach a working relay, the receiver is listening and can reach a
// working relay, all at path chance of 0.5 or better.
function deliverable(w, m) {
  let good = 0;
  for (let t = m.created; t < m.created + DAY && t < w.END; t += 10 * MIN) {
    const a = w.relayIds.some((r) => w.isUp(r, t) && w.p(m.from, w.pos(r), t) >= 0.5);
    const b = w.relayIds.some((r) => w.isUp(r, t) && w.p(m.to, w.pos(r), t) >= 0.5);
    if (a && b && w.listening(m.to, t)) good++;
  }
  return good >= 3;
}

for (const seed of [1, 2, 3]) {
  test(`a simulated week, seed ${seed}: no double delivery, no silent loss`, () => {
    const w = runWeek({ seed });
    const terminal = new Set(['delivered', 'delivered-no-receipt', 'failed']);
    // Every message ended in a named state.
    for (const m of w.msgs) assert.ok(terminal.has(m.state) || m.created > w.END - DAY, `${m.id} left in ${m.state}`);
    // The user never sees a message twice, whatever the retries did.
    assert.equal(w.msgs.filter((m) => m.userSeen > 1).length, 0);
    // Delivered means the receiver's app got it; a failure always has a reason.
    for (const m of w.msgs) if (m.state === 'failed') assert.equal(m.reason, 'lifetime over');
    const must = w.msgs.filter((m) => m.created <= w.END - DAY && deliverable(w, m));
    const got = must.filter((m) => m.userSeen === 1);
    const rate = got.length / Math.max(1, must.length);
    console.log(`seed ${seed}: ${w.msgs.length} messages, ${must.length} must-deliver, ${got.length} delivered (${(rate * 100).toFixed(1)}%), retries ${w.stats.retries}, re-sent ${w.msgs.reduce((n, m) => n + m.resent, 0)}, relay crashes losing RAM ${w.stats.crashesLostRam}, refused full ${w.stats.refusedFull}, pages ${w.stats.pages}, server duplicates stopped ${w.stats.serverDuplicatesStopped}`);
    assert.ok(rate >= 0.95, `only ${(rate * 100).toFixed(1)}% of the deliverable messages arrived`);
  });
}

test('a flood of 400 messages from one operator in 10 minutes still ends with nothing lost or doubled', () => {
  const w = runWeek({ seed: 4, flood: 400 });
  const fl = w.msgs.filter((m) => m.flooded);
  assert.equal(fl.length, 400);
  assert.equal(fl.filter((m) => m.userSeen > 1).length, 0);
  const must = fl.filter((m) => deliverable(w, m));
  const got = must.filter((m) => m.userSeen === 1);
  console.log(`flood: ${must.length} deliverable of 400, ${got.length} delivered, refused while a relay was full ${w.stats.refusedFull}`);
  assert.ok(got.length / Math.max(1, must.length) >= 0.9);
  assert.ok(w.stats.refusedFull > 0, 'the flood should have hit the RAM limit of a relay');
});

test('latency on good links: what the sim shows (it only repeats the assumed air times)', () => {
  const w = runWeek({ seed: 5, crashes: false, interferers: false, nights: false, flood: 0, messages: 200, alwaysListening: true });
  const first = w.msgs.filter((m) => m.userSeen === 1 && m.attempt === 0 && m.deliveredAt);
  const lat = first.map((m) => (m.deliveredAt - m.created) / 1000).sort((a, b) => a - b);
  assert.ok(lat.length > 60, `${lat.length} first-try deliveries`);
  const q = (f) => lat[Math.floor(lat.length * f)];
  console.log(`good links, first try: ${lat.length} of ${w.msgs.length} messages, median ${q(0.5).toFixed(0)} s, 90th percentile ${q(0.9).toFixed(0)} s, all delivered ${w.msgs.filter((m) => m.userSeen === 1).length}`);
  // The assumed steps add up to 22 s; anything else on top is a paging window (up to 10 min).
  assert.ok(q(0.5) >= 20);
});

test('the week is repeatable', () => {
  const a = runWeek({ seed: 9, messages: 60, flood: 0 });
  const b = runWeek({ seed: 9, messages: 60, flood: 0 });
  assert.deepEqual(a.msgs.map((m) => [m.state, m.userSeen, m.deliveredAt]), b.msgs.map((m) => [m.state, m.userSeen, m.deliveredAt]));
});
void EPOCH;
