import test from 'node:test';
import assert from 'node:assert/strict';
import { Hub, HOLD_MS, DEAD_AFTER_MS } from '../src/hub.js';
import { Router } from '../src/router.js';

const sock = () => ({ got: [], closed: false, send(m) { this.got.push(m); }, close() { this.closed = true; } });

test('a push reaches a connected relay at once and is not kept', () => {
  const h = new Hub();
  const s = sock();
  h.connect('RB', s, 0);
  assert.equal(h.push('RB', { t: 'deliver', id: 1 }, 5), 'pushed');
  assert.deepEqual(s.got, [{ t: 'deliver', id: 1 }]);
  assert.deepEqual(h.poll('RB', 6), []);
});

test('one socket per relay key: the second connection stops the first', () => {
  const h = new Hub();
  const a = sock();
  const b = sock();
  h.connect('RA', a, 0);
  h.connect('RA', b, 10);
  assert.deepEqual(a.got, [{ t: 'kicked' }]);
  assert.ok(a.closed);
  h.push('RA', { t: 'plan' }, 11);
  assert.deepEqual(b.got, [{ t: 'plan' }]);
  // The old socket closing later must not drop the new one.
  h.disconnect('RA', a);
  assert.equal(h.push('RA', { t: 'x' }, 12), 'pushed');
});

test('without a socket items wait in RAM for the poll, for 60 s at most', () => {
  const h = new Hub();
  assert.equal(h.push('RB', { t: 'deliver', id: 1 }, 0), 'held');
  h.push('RB', { t: 'deliver', id: 2 }, 30e3);
  assert.deepEqual(h.poll('RB', HOLD_MS + 1000).map((m) => m.id), [2]);
  assert.deepEqual(h.poll('RB', HOLD_MS + 1000), []);
});

test('a relay whose heartbeat stopped is dropped and the router stops using it', () => {
  const r = new Router();
  r.relayUp('RB');
  const h = new Hub({ onDown: (id) => r.relayDown(id), onUp: (id) => r.relayUp(id) });
  const s = sock();
  h.connect('RB', s, 0);
  h.heartbeat('RB', 30e3);
  h.tick(30e3 + DEAD_AFTER_MS - 1);
  assert.equal(r.relays.get('RB').online, true);
  h.tick(30e3 + DEAD_AFTER_MS + 1);
  assert.equal(r.relays.get('RB').online, false);
  assert.ok(s.closed);
});
