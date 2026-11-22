import test from 'node:test';
import assert from 'node:assert/strict';
import { greatCircleKm, midpoint, sunElevation, modelP, pathModelP, parseSolar, fetchSolar, Learner, ListenPattern, TAU_MS } from '../src/propagation.js';
import { Router, h24 } from '../src/router.js';

const HOUR = 3600e3;
const NOON = Date.UTC(2026, 2, 20, 12, 0, 0); // March equinox, noon UTC

test('distances and the sun are right to a degree or a few kilometres', () => {
  assert.ok(Math.abs(greatCircleKm(51.5, 0, 40.7, -74) - 5570) < 40); // London - New York
  assert.ok(sunElevation(0, 0, NOON) > 85);
  assert.ok(sunElevation(0, 180, NOON) < -85);
  assert.ok(Math.abs(sunElevation(60, 0, NOON) - 30) < 3);
  const m = midpoint(0, 0, 0, 90);
  assert.ok(Math.abs(m.lon - 45) < 0.5 && Math.abs(m.lat) < 0.5);
});

test('the model: skip zone, long day paths, night, disturbed field', () => {
  const base = { distKm: 2500, elevMid: 40, sfi: 150, kp: 1 };
  assert.ok(modelP(base) > 0.9);
  assert.ok(modelP({ ...base, distKm: 300 }) < 0.1, 'inside the skip zone');
  assert.ok(modelP({ ...base, elevMid: -30 }) < 0.1, 'night');
  assert.ok(modelP({ ...base, kp: 6 }) < modelP(base) * 0.5, 'storm');
  assert.ok(modelP({ ...base, distKm: 6000 }) < modelP(base));
  const a = { lat: 60, lon: 25 };
  const b = { lat: 40, lon: -75 };
  assert.ok(pathModelP(a, b, Date.UTC(2026, 2, 20, 16)) > pathModelP(a, b, Date.UTC(2026, 2, 20, 4)));
});

test('NOAA data is read in the shape the service returns', async () => {
  const flux = [{ flux: 121, time_tag: '2026-11-17T20:00:00' }];
  const kp = [{ time_tag: 'x', Kp: 1.67 }, { time_tag: 'y', Kp: 2.33 }];
  assert.deepEqual(parseSolar(flux, kp), { sfi: 121, kp: 2.33 });
  assert.deepEqual(parseSolar(null, 5), { sfi: undefined, kp: undefined });
  const seen = [];
  const out = await fetchSolar(async (u) => { seen.push(u); return { json: async () => (u.includes('flux') ? flux : kp) }; });
  assert.deepEqual(out, { sfi: 121, kp: 2.33 });
  assert.equal(seen.length, 2);
});

test('measurements replace the model, slowly forget, and the path is symmetric', () => {
  const l = new Learner();
  const t0 = Date.UTC(2026, 10, 18, 12);
  assert.equal(l.p('FI', 'US', 12, t0, 0.9), 0.9, 'no data: the model');
  for (let i = 0; i < 20; i++) l.observe('FI', 'US', 12, false, t0 + i * 60e3);
  const after = l.p('US', 'FI', 12, t0 + 20 * 60e3, 0.9);
  assert.ok(after < 0.2, `twenty failures should beat a 0.9 model, got ${after}`);
  assert.equal(l.p('FI', 'US', 13, t0, 0.9), 0.9, 'other hours are separate');
  // A month later the old failures are forgotten and the model is back.
  const later = l.p('FI', 'US', 12, t0 + 30 * 24 * HOUR, 0.9);
  assert.ok(later > 0.8, `forgotten, got ${later}`);
  assert.ok(Math.exp(-7 * 24 * HOUR / TAU_MS) < 0.4);
});

test('exploration only where there is little data, and never over one percent of air time', () => {
  const l = new Learner();
  const t = Date.UTC(2026, 10, 18, 12);
  assert.equal(l.exploreDue('FI', 'US', 12, t, 0, 600), true);
  assert.equal(l.exploreDue('FI', 'US', 12, t, 5.5, 600), false, 'budget spent');
  for (let i = 0; i < 6; i++) l.observe('FI', 'US', 12, true, t);
  assert.equal(l.exploreDue('FI', 'US', 12, t, 0, 600), false, 'enough samples');
});

test('the router uses what was learned to pick the relay', () => {
  const l = new Learner();
  const now = Date.UTC(2026, 10, 18, 12);
  const r = new Router({ pOf: (relay, destHash, t) => l.p(relay, 'AREA-X', 12, t, 0.5) });
  r.relayUp('A');
  r.relayUp('B');
  r.heard('A', h24('DD1DDD'), 10, 1, now - 60e3);
  r.heard('B', h24('DD1DDD'), 10, 1, now - 60e3);
  for (let i = 0; i < 10; i++) l.observe('B', 'AREA-X', 12, false, now - i * 1000);
  for (let i = 0; i < 10; i++) l.observe('A', 'AREA-X', 12, true, now - i * 1000);
  assert.equal(r.route('DD1DDD', now).relay, 'A');
});

test('predictive paging uses the hours the operator listens in', () => {
  const p = new ListenPattern();
  assert.equal(p.bestWindows(Date.UTC(2026, 10, 18, 9, 3), 3), null, 'too little data');
  // Replies on three evenings, 18 to 19 UTC.
  for (const d of [15, 16, 17]) p.observe(Date.UTC(2026, 10, d, 18, 20));
  const now = Date.UTC(2026, 10, 18, 9, 3);
  const w = p.bestWindows(now, 3);
  assert.equal(w.length, 3);
  // Same weekday-hour slots as the replies would be Sun/Mon/Tue 18:xx; the 18th is a Wednesday,
  // so the best upcoming windows are the first hour that matches, in time order.
  assert.ok(w.every((t) => t >= now && t % 600e3 === 0));
  const r = new Router();
  r.relayUp('A');
  const job = r.page('EE1EEE', now, { pattern: p });
  assert.deepEqual(job.windows.map((x) => x.start), w);
  assert.equal(r.page('FF1FFF', now).windows[0].start, Date.UTC(2026, 10, 18, 9, 10));
});

test('predictive paging picks the learned evening over the next three windows', () => {
  const p = new ListenPattern();
  const wed = Date.UTC(2026, 10, 18);
  for (const w of [-7, -14, -21]) p.observe(wed + w * 24 * HOUR + 18.5 * HOUR);
  const now = wed + 9.05 * HOUR;
  const w = p.bestWindows(now, 3);
  for (const t of w) assert.equal(new Date(t).getUTCHours(), 18, new Date(t).toISOString());
});
