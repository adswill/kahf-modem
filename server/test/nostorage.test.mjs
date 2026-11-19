// No stored messages (MESH-SPEC.md section 7 and step 11). The mesh modules hold only ids,
// sizes, hashes and routing facts, in RAM. This test fails if one of them starts to use storage
// or lets message content through. The older queued-message backbone was removed (migration v3).
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { Hub } from '../src/hub.js';
import { Router, h24 } from '../src/router.js';
import { plan } from '../src/planner.js';

const MESH_MODULES = ['router.js', 'hub.js', 'planner.js', 'mesh.js', 'sign.js'];
const STORAGE_WORDS = /\b(storage|KV|STATIONS|MAILBOXES|BACKBONE|D1|R2|writeFile|appendFile|createWriteStream|localStorage|indexedDB|fetch\()|from ['"]node:fs/;

test('the mesh modules import nothing but node:crypto and name no storage', () => {
  for (const f of MESH_MODULES) {
    const src = readFileSync(new URL(`../src/${f}`, import.meta.url), 'utf8');
    const code = src.split('\n').filter((l) => !l.trim().startsWith('//')).join('\n');
    const imports = [...code.matchAll(/^import .* from ['"](.+)['"]/gm)].map((m) => m[1]);
    for (const i of imports) assert.ok(i === 'node:crypto' || /^\.\/(hub|router|planner)\.js$/.test(i), `${f} imports ${i}`);
    assert.ok(!STORAGE_WORDS.test(code), `${f} mentions storage`);
  }
});

test('the storage pattern does catch storage', () => {
  for (const bad of ['env.STATIONS.put(k, v)', 'this.state.storage.put(a)', "import fs from 'node:fs'", 'await fetch(url)']) assert.ok(STORAGE_WORDS.test(bad), bad);
});

const sock = () => ({ got: [], send(m) { this.got.push(m); }, close() {} });

test('a message run through router and hub moves only ids, sizes and hashes', () => {
  const SECRET = 'SECRET-BODY-TEXT-12345';
  const r = new Router();
  r.relayUp('A');
  r.relayUp('B');
  const hub = new Hub({ onDown: (i) => r.relayDown(i), onUp: (i) => r.relayUp(i) });
  const a = sock();
  const b = sock();
  hub.connect('A', a, 0);
  hub.connect('B', b, 0);
  r.heard('B', h24('DD1DDD'), 9, 2, 1000);
  const route = r.route('DD1DDD', 2000);
  assert.equal(route.relay, 'B');
  assert.ok(r.firstSight('id-77', 2000));
  hub.push('B', { t: 'deliver', id: 77, to: 'DD1DDD', size: SECRET.length }, 2000);
  hub.push('A', { t: 'plan', grants: plan(5, 1, [{ id: 1, queueBytes: 5, urgency: 0 }]).grants }, 2000);
  // Everything the server objects now hold, as text: the secret is nowhere, because it never came in.
  const all = JSON.stringify([r.relays, [...r.presence], [...r.seen], [...hub.held], a.got, b.got], (k, v) => (v instanceof Map ? [...v] : v));
  assert.ok(!all.includes(SECRET));
});

test('the hub refuses a message that carries content', () => {
  const hub = new Hub();
  for (const k of ['body', 'text', 'subject', 'payload', 'content', 'data', 'message']) {
    assert.throws(() => hub.push('A', { t: 'deliver', id: 1, [k]: 'hello' }, 0), /only ids and sizes/, k);
  }
  assert.equal(hub.push('A', { t: 'deliver', id: 1, size: 5 }, 0), 'held');
});
