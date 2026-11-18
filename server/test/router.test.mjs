import test from 'node:test';
import assert from 'node:assert/strict';
import { Router, h24, nextRetry, nextPagingWindow, PAGE_PERIOD_MS, PRESENCE_TTL_MS } from '../src/router.js';

const MIN = 60e3;
const T0 = Date.UTC(2026, 10, 18, 9, 3, 0);

function mesh() {
  const r = new Router();
  for (const id of ['A', 'B', 'C']) r.relayUp(id);
  return r;
}

test('hash is 24 bits, never 0, ignores case', () => {
  assert.equal(h24('aa1aaa'), h24(' AA1AAA '));
  for (const c of ['AA1AAA', 'OH2XYZ', 'K1ABC']) assert.ok(h24(c) > 0 && h24(c) <= 0xffffff);
});

test('known: the best relay is chosen, a second is kept warm, a fresh hearing needs no page', () => {
  const r = mesh();
  r.heard('B', h24('DD1DDD'), 12, 3, T0 - 30e3);
  r.heard('C', h24('DD1DDD'), 5, 4, T0 - 10 * MIN);
  const x = r.route('DD1DDD', T0);
  assert.deepEqual([x.state, x.relay, x.second, x.pageFirst], ['known', 'B', 'C', false]);
});

test('known: a busy relay loses to a quieter one that heard it a little earlier', () => {
  const r = mesh();
  r.heard('B', h24('DD1DDD'), 12, 3, T0 - 5 * MIN);
  r.heard('C', h24('DD1DDD'), 12, 3, T0 - 10 * MIN);
  r.setQueue('B', 40);
  assert.equal(r.route('DD1DDD', T0).relay, 'C');
});

test('stale: heard four hours ago means deliver through that relay but page first', () => {
  const r = mesh();
  r.heard('A', h24('DD1DDD'), 8, 2, T0 - 4 * 3600e3);
  const x = r.route('DD1DDD', T0);
  assert.deepEqual([x.state, x.relay, x.pageFirst], ['known', 'A', true]);
  // Older than six hours is forgotten.
  assert.equal(r.route('DD1DDD', T0 + PRESENCE_TTL_MS).state, 'unknown');
  // An offline relay is not used.
  r.relayDown('A');
  assert.equal(r.route('DD1DDD', T0).state, 'unknown');
});

test('unknown: relays page in windows at the start of every tenth minute, each in its own cell', () => {
  const r = new Router();
  for (let i = 0; i < 30; i++) r.relayUp('R' + String(i).padStart(2, '0'));
  const job = r.page('EE1EEE', T0);
  assert.equal(job.windows.length, 3);
  for (const w of job.windows) {
    assert.equal(w.start % PAGE_PERIOD_MS, 0);
    assert.ok(w.start >= T0);
    const cells = new Set(w.pages.map((p) => `${p.at}/${p.slot}`));
    assert.equal(cells.size, w.pages.length, 'two pages share a cell');
    assert.ok(w.pages.length <= 12);
  }
  // 30 relays do not fit one window: the windows take different groups.
  assert.notDeepEqual(job.windows[0].pages.map((p) => p.relay), job.windows[1].pages.map((p) => p.relay));
  assert.equal(nextPagingWindow(T0), Date.UTC(2026, 10, 18, 9, 10, 0));
});

test('unknown: the first relay that hears the answer ends the paging and gets the delivery', () => {
  const r = mesh();
  assert.equal(r.route('EE1EEE', T0).state, 'unknown');
  r.page('EE1EEE', T0);
  assert.equal(r.pageStatus('EE1EEE'), 'paging');
  const x = r.reply('C', h24('EE1EEE'), 6, 5, T0 + 8 * MIN);
  assert.deepEqual([x.state, x.relay], ['known', 'C']);
  assert.equal(r.pageStatus('EE1EEE'), 'found');
});

test('not reachable after three paging windows, and the sender keeps retrying from its own outbox', () => {
  const r = mesh();
  const dest = 'FF1FFF';
  let now = T0;
  const created = T0;
  r.page(dest, now);
  r.tick(now + 5 * MIN);
  assert.equal(r.pageStatus(dest), 'paging');
  r.tick(now + 40 * MIN);
  assert.equal(r.pageStatus(dest), 'not_reachable');
  // The sender's outbox: the message stays on the sender's side and is tried again.
  let attempt = 0;
  let retries = 0;
  let found = false;
  while (now !== null && !found) {
    now = nextRetry(attempt++, now, created);
    if (now === null) break;
    retries++;
    // The receiver switches on at hour 3: the next page finds it.
    if (now >= T0 + 3 * 3600e3) {
      r.page(dest, now);
      r.reply('A', h24(dest), 9, 2, now + 2 * MIN);
      found = r.pageStatus(dest) === 'found';
    }
  }
  assert.ok(found, 'delivered after the receiver came back');
  assert.ok(retries >= 3 && retries < 20);
});

test('a message never delivered stops at its lifetime', () => {
  let now = T0;
  let n = 0;
  while ((now = nextRetry(n++, now, T0)) !== null) assert.ok(n < 100);
  assert.ok(n > 5);
});

test('duplicate control keeps only a hash, for two hours', () => {
  const r = mesh();
  assert.equal(r.firstSight('msg-1', T0), true);
  assert.equal(r.firstSight('msg-1', T0 + 1 * MIN), false);
  assert.equal(r.firstSight('msg-1', T0 + 3 * 3600e3), true);
});
