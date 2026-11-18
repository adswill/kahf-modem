// A simulated week of the mesh (MESH-SPEC.md step 16). The real router, hub, learner, listening
// pattern, retry schedule and planner run; the air is a model: the chance a hop works is the
// propagation model's path probability (sun, distance, flux), cut by interferers and by relay
// crashes. So the sim tests the logic (no duplicates, no silent loss, retries, paging, flood
// handling). It does NOT measure radio speed: every time in it for a step on the air is an
// assumption written below.
import { Router, h24, nextRetry } from '../src/router.js';
import { Hub } from '../src/hub.js';
import { Learner, ListenPattern, pathModelP } from '../src/propagation.js';
import { plan } from '../src/planner.js';

export const EPOCH = 12e3;
const HOUR = 3600e3;
const DAY = 24 * HOUR;
const SOLAR = { sfi: 120, kp: 2 };
// Assumed air times (s): request+grant, message, downlink, receipt. These are the spec's own
// estimates (11), so the latencies below only repeat them; they are not measurements.
const T_UP = 8e3;
const T_DOWN = 8e3;
const T_RECEIPT = 6e3;

const CITIES = {
  HEL: [60.2, 25], MAD: [40.4, -3.7], CAI: [30, 31], NYC: [40.7, -74], JNB: [-26, 28], DEL: [28.6, 77], SYD: [-33.9, 151], SAO: [-23.5, -46.6],
};

class Rng {
  constructor(s) { this.s = s >>> 0; }
  u() { this.s = (Math.imul(this.s, 1664525) + 1013904223) >>> 0; return this.s / 4294967296; }
  int(n) { return Math.floor(this.u() * n); }
}

export function runWeek({ seed = 1, ops = 14, messages = 260, flood = 400, crashes = true, interferers = true, nights = true, days = 7, alwaysListening = false } = {}) {
  const rng = new Rng(seed);
  const T0 = Date.UTC(2026, 2, 20, 0, 0, 0);
  const END = T0 + days * DAY;
  const router = new Router();
  const learner = new Learner();
  const relayIds = Object.keys(CITIES);
  const pos = (id) => ({ lat: CITIES[id][0], lon: CITIES[id][1] });
  const hub = new Hub({ onDown: (r) => router.relayDown(r), onUp: (r) => router.relayUp(r) });
  const sockets = Object.fromEntries(relayIds.map((r) => [r, { send() {}, close() {} }]));
  for (const r of relayIds) { router.relayUp(r); hub.connect(r, sockets[r], T0); }
  const up = Object.fromEntries(relayIds.map((r) => [r, true]));
  // Message ids held in RAM at each relay, with the time each is dropped (spec 7.2: 60 s at most).
  const ram = Object.fromEntries(relayIds.map((r) => [r, new Map()]));
  const hold = (r, id, t) => ram[r].set(id, t + 60e3);

  // Operators: scattered, each listening about 8 hours a day at its own hours.
  const operators = Array.from({ length: ops }, (_, i) => {
    const base = relayIds[rng.int(relayIds.length)];
    const [la, lo] = CITIES[base];
    const call = `OP${String(i).padStart(2, '0')}X`;
    return { call, h: h24(call), lat: la + (rng.u() - 0.5) * 30, lon: lo + (rng.u() - 0.5) * 40, startH: rng.int(24), lenH: 6 + rng.int(6), pattern: new ListenPattern(), seen: new Set() };
  });
  const listening = (o, t) => alwaysListening || ((new Date(t).getUTCHours() - o.startH + 24) % 24) < o.lenH;

  // Crashes and interferer events, drawn up front so the same seed gives the same week.
  const crashList = [];
  if (crashes) for (const r of relayIds) for (let t = T0 + rng.u() * 2 * DAY; t < END; t += (1.5 + rng.u() * 2) * DAY) crashList.push({ r, from: t, to: t + (1 + rng.u() * 5) * HOUR });
  const events = [];
  if (interferers) for (let t = T0; t < END; t += 4 * HOUR * (0.5 + rng.u())) events.push({ from: t, to: t + 20 * 60e3, hit: rng.u() });
  const jammed = (t) => events.some((e) => t >= e.from && t < e.to);

  const p = (a, b, t) => (nights ? pathModelP(a, b, t, SOLAR) : 0.8) * 0.9;
  const isUp = (r, t) => !crashList.some((c) => c.r === r && t >= c.from && t < c.to);
  const bestRelay = (o, t, exclude = []) => {
    let best = null;
    for (const r of relayIds) {
      if (!up[r] || exclude.includes(r)) continue;
      const pp = p(o, pos(r), t);
      if (pp >= 0.3 && (!best || pp > best.p)) best = { r, p: pp };
    }
    return best;
  };

  // Messages.
  const msgs = [];
  const mk = (t, flooded = false, from = null) => {
    const a = from ?? operators[rng.int(ops)];
    let b = operators[rng.int(ops)];
    while (b === a) b = operators[rng.int(ops)];
    msgs.push({ id: `m${msgs.length}`, from: a, to: b, created: t, due: t, attempt: 0, state: 'waiting', flooded, deliveredAt: null, userSeen: 0, resent: 0, relayA: null, log: [] });
  };
  for (let i = 0; i < messages; i++) mk(T0 + rng.u() * (days - 2) * DAY);
  const floodAt = T0 + 3 * DAY + 5 * HOUR;
  for (let i = 0; i < flood; i++) mk(floodAt + rng.u() * 10 * 60e3, true, operators[0]);

  const stats = { duplicatesToUser: 0, serverDuplicatesStopped: 0, crashesLostRam: 0, refusedFull: 0, pages: 0, retries: 0 };
  const RAM_CAP = 30;
  const TERMINAL = new Set(['delivered', 'delivered-no-receipt', 'failed']);
  const pendingDown = []; // downlink jobs: {m, relay, at}
  const receipts = []; // {m, at}

  for (let t = T0; t < END; t += EPOCH) {
    // Relay crashes and returns.
    for (const r of relayIds) {
      const c = crashList.find((c) => c.r === r && t >= c.from && t < c.to);
      if (c && up[r]) {
        up[r] = false;
        hub.disconnect(r, sockets[r]);
        stats.crashesLostRam += ram[r].size;
        ram[r].clear(); // in-flight messages at a crashed relay are gone: the sender's retry repairs it
      } else if (!c && !up[r]) {
        up[r] = true;
        hub.connect(r, sockets[r], t);
      }
    }
    // Operators that are listening announce themselves to the relay they hear (HERE, every 10 min).
    if (t % (600e3) < EPOCH) {
      for (const o of operators) {
        if (!listening(o, t)) continue;
        const b = bestRelay(o, t);
        if (b) { router.heard(b.r, o.h, 10, 1, t); o.pattern.observe(t); learner.observe(b.r, o.call, new Date(t).getUTCHours(), true, t); }
      }
    }
    router.tick(t);
    for (const r of relayIds) for (const [id, until] of ram[r]) if (until <= t) ram[r].delete(id);
    // Downlinks that are due.
    for (let i = pendingDown.length - 1; i >= 0; i--) {
      const d = pendingDown[i];
      if (d.at > t) continue;
      pendingDown.splice(i, 1);
      const m = d.m;
      if (!up[d.relay]) { m.log.push(['relayB-down', t]); continue; } // lost: sender's retry timer will repair
      ram[d.relay].delete(m.id);
      const rcv = m.to;
      const ok = listening(rcv, t) && rng.u() < p(rcv, pos(d.relay), t) * (jammed(t) && rng.u() < 0.8 ? 0.2 : 1);
      learner.observe(d.relay, rcv.call, new Date(t).getUTCHours(), ok, t);
      if (!ok) { m.log.push(['downlink-fail', t]); continue; }
      // The receiver's app shows a message once, by id.
      if (rcv.seen.has(m.id)) { stats.serverDuplicatesStopped++; } else { rcv.seen.add(m.id); m.userSeen++; m.deliveredAt ??= t; }
      receipts.push({ m, at: t + T_RECEIPT });
    }
    // Receipts reach the sender (if its relay is still there and the air allows).
    for (let i = receipts.length - 1; i >= 0; i--) {
      const r = receipts[i];
      if (r.at > t) continue;
      receipts.splice(i, 1);
      const b = bestRelay(r.m.from, t);
      if (b && listening(r.m.from, t) && rng.u() < b.p) { r.m.state = 'delivered'; r.m.receiptAt = t; }
    }
    // Each message's own timer.
    for (const m of msgs) {
      if (TERMINAL.has(m.state) || m.due > t) continue;
      if (t - m.created >= DAY) {
        // Lifetime over. Delivered to the user but no receipt seen still counts as delivered.
        m.state = m.userSeen > 0 ? 'delivered-no-receipt' : 'failed';
        m.reason = m.userSeen > 0 ? 'receipt lost' : 'lifetime over';
        continue;
      }
      step(m, t);
    }
  }

  function retry(m, t, gentle = false) {
    m.attempt++;
    stats.retries++;
    const next = gentle ? nextRetry(m.attempt, t, m.created) : t + 90e3 + rng.u() * 60e3;
    m.due = next ?? Infinity;
    m.state = 'waiting';
  }

  function step(m, t) {
    const o = m.from;
    const a = bestRelay(o, t);
    if (!a) return retry(m, t, true);
    if (m.state === 'sent') m.resent++;
    // Uplink: the sender transmits in its window (needs a relay to hear); relays take a limited number at once.
    const jam = jammed(t) && rng.u() < 0.8;
    const ok = rng.u() < a.p * (jam ? 0.2 : 1);
    learner.observe(a.r, o.call, new Date(t).getUTCHours(), ok, t);
    if (!ok) return retry(m, t);
    if (ram[a.r].size >= RAM_CAP) { stats.refusedFull++; return retry(m, t); }
    // Relay A holds the message in RAM and asks the server where the receiver is.
    hold(a.r, m.id, t);
    const dest = m.to;
    const route = router.route(dest.call, t);
    let relayB = route.relay;
    if (route.state === 'unknown' || route.pageFirst) {
      // Paging: the receiver answers in a paging window if it is listening and hears some relay.
      const job = router.page(dest.call, t, { pattern: dest.pattern });
      stats.pages++;
      const w = job.windows.find((w) => w.start >= t) ?? job.windows[0];
      const when = w.start + 20e3;
      if (listening(dest, when)) {
        const b = bestRelay(dest, when);
        if (b) {
          const r = router.reply(b.r, dest.h, 8, 1, when);
          relayB = r.relay;
          m.log.push(['paged', when]);
          router.pages.delete(dest.h);
        }
      }
      if (!relayB || route.state === 'unknown' && !listening(dest, when)) {
        // Not found in this cycle: the sender keeps the message and tries again later.
        ram[a.r].delete(m.id);
        router.tick(when + 25 * 60e3);
        return retry(m, t, true);
      }
      m.due = when + 1000;
      pendingDown.push({ m, relay: relayB, at: when + T_DOWN });
      if (relayB) hold(relayB, m.id, when);
      m.state = 'sent';
      m.retryAt = when + 5 * 60e3;
      m.due = m.retryAt;
      return;
    }
    if (!up[relayB]) { ram[a.r].delete(m.id); return retry(m, t); }
    pendingDown.push({ m, relay: relayB, at: t + T_UP + T_DOWN });
    hold(relayB, m.id, t);
    m.state = 'sent';
    // If no receipt comes, the sender tries again (the hand-over rule: only the receipt ends it).
    m.due = t + T_UP + T_DOWN + 5 * 60e3;
  }

  return { msgs, operators, stats, relayIds, T0, END, p, pos, listening, isUp, crashList, events, router };
}
