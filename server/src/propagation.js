// Propagation: a model first, measurements from the first hour (MESH-SPEC.md 6.5), and the
// listening pattern for predictive paging (10.5). Pure, in memory, `now` is always an argument.
// Every number in the model is an estimate (est.) until it is checked against measurements.

export const TAU_MS = 7 * 24 * 3600e3; // forgetting: about a week
export const PRIOR_WEIGHT = 4; // the model counts as this many observations

const rad = (d) => (d * Math.PI) / 180;
const deg = (r) => (r * 180) / Math.PI;
const mod = (a, n) => ((a % n) + n) % n;

export function greatCircleKm(lat1, lon1, lat2, lon2) {
  const dlat = rad(lat2 - lat1);
  const dlon = rad(lon2 - lon1);
  const a = Math.sin(dlat / 2) ** 2 + Math.cos(rad(lat1)) * Math.cos(rad(lat2)) * Math.sin(dlon / 2) ** 2;
  return 2 * 6371 * Math.asin(Math.min(1, Math.sqrt(a)));
}

// Midpoint of the great circle path (where the sky wave reflects, to a first guess).
export function midpoint(lat1, lon1, lat2, lon2) {
  const [p1, l1, p2, dl] = [rad(lat1), rad(lon1), rad(lat2), rad(lon2 - lon1)];
  const bx = Math.cos(p2) * Math.cos(dl);
  const by = Math.cos(p2) * Math.sin(dl);
  const lat = Math.atan2(Math.sin(p1) + Math.sin(p2), Math.hypot(Math.cos(p1) + bx, by));
  return { lat: deg(lat), lon: deg(l1 + Math.atan2(by, Math.cos(p1) + bx)) };
}

// Sun elevation in degrees (low precision astronomical formulas, good to about a degree).
export function sunElevation(lat, lon, ms) {
  const d = (ms - Date.UTC(2000, 0, 1, 12)) / 86400e3;
  const L = mod(280.46 + 0.9856474 * d, 360);
  const g = mod(357.528 + 0.9856003 * d, 360);
  const lambda = L + 1.915 * Math.sin(rad(g)) + 0.02 * Math.sin(rad(2 * g));
  const eps = 23.439 - 4e-7 * d;
  const dec = Math.asin(Math.sin(rad(eps)) * Math.sin(rad(lambda)));
  const ra = Math.atan2(Math.cos(rad(eps)) * Math.sin(rad(lambda)), Math.cos(rad(lambda)));
  const gmst = mod(280.46061837 + 360.98564736629 * d, 360);
  const ha = rad(gmst + lon) - ra;
  return deg(Math.asin(Math.sin(rad(lat)) * Math.sin(dec) + Math.cos(rad(lat)) * Math.cos(dec) * Math.cos(ha)));
}

const ramp = (x, x0, y0, x1, y1) => y0 + ((Math.min(Math.max(x, x0), x1) - x0) * (y1 - y0)) / (x1 - x0);

// First guess that 17 m works on a path (est.): daytime at the midpoint, 1000 to 4000 km best,
// skip zone below about 700 km, helped by solar flux, hurt by a disturbed field.
export function modelP({ distKm, elevMid, sfi = 100, kp = 2 }) {
  const dist = distKm < 700 ? 0.05 : distKm < 1000 ? ramp(distKm, 700, 0.05, 1000, 1) : distKm <= 4000 ? 1 : distKm <= 7000 ? ramp(distKm, 4000, 1, 7000, 0.4) : 0.2;
  const day = elevMid > 10 ? 1 : elevMid > 0 ? ramp(elevMid, 0, 0.6, 10, 1) : elevMid > -6 ? 0.3 : 0.05;
  const flux = ramp(sfi, 70, 0.7, 170, 1);
  const k = kp >= 5 ? 0.4 : kp >= 4 ? 0.7 : 1;
  return Math.min(1, dist * day * flux * k);
}

// How much the sun's output and the field's disturbance change any path today (1 = a normal day).
export function solarFactor({ sfi = 100, kp = 2 } = {}) {
  const flux = ramp(sfi, 70, 0.7, 170, 1);
  const k = kp >= 5 ? 0.4 : kp >= 4 ? 0.7 : 1;
  return flux * k;
}

export function pathModelP(a, b, ms, solar = {}) {
  const distKm = greatCircleKm(a.lat, a.lon, b.lat, b.lon);
  const m = midpoint(a.lat, a.lon, b.lat, b.lon);
  return modelP({ distKm, elevMid: sunElevation(m.lat, m.lon, ms), sfi: solar.sfi, kp: solar.kp });
}

// NOAA SWPC: 10cm-flux.json is [{flux, time_tag}], noaa-planetary-k-index.json is
// [{time_tag, Kp, ...}] oldest first. Shapes checked against the live service on 2026-11-18.
export function parseSolar(fluxJson, kpJson) {
  const f = Array.isArray(fluxJson) ? fluxJson.at(-1) : null;
  const k = Array.isArray(kpJson) ? kpJson.at(-1) : null;
  return { sfi: Number.isFinite(f?.flux) ? f.flux : undefined, kp: Number.isFinite(k?.Kp) ? k.Kp : undefined };
}

export async function fetchSolar(fetchFn) {
  const get = async (u) => (await fetchFn(u)).json();
  const [f, k] = await Promise.all([get('https://services.swpc.noaa.gov/products/summary/10cm-flux.json'), get('https://services.swpc.noaa.gov/products/noaa-planetary-k-index.json')]);
  return parseSolar(f, k);
}

// What the network has actually heard, per pair of places and hour of day, forgetting slowly.
export class Learner {
  constructor() {
    this.cells = new Map(); // "a|b|hour" -> {s, f, at}
    this.tokens = new Map(); // exploration budget per lane: air seconds used in the window
  }
  _key(a, b, hour) {
    return `${[a, b].sort().join('|')}|${hour}`;
  }
  // Aggregates only (counts per pair and hour): this is what the mesh object may keep on disk.
  toJSON() {
    return { cells: [...this.cells] };
  }
  static fromJSON(j) {
    const l = new Learner();
    for (const [k, c] of j?.cells ?? []) if (Number.isFinite(c?.s) && Number.isFinite(c?.f) && Number.isFinite(c?.at)) l.cells.set(k, { s: c.s, f: c.f, at: c.at });
    return l;
  }
  _decayed(c, now) {
    const k = Math.exp(-(now - c.at) / TAU_MS);
    return { s: c.s * k, f: c.f * k };
  }
  // One data point: a decoded frame, a beacon heard, or a grant outcome.
  observe(a, b, hour, success, now) {
    const key = this._key(a, b, hour);
    const c = this.cells.get(key);
    const d = c ? this._decayed(c, now) : { s: 0, f: 0 };
    if (success) d.s += 1;
    else d.f += 1;
    this.cells.set(key, { ...d, at: now });
  }
  count(a, b, hour, now) {
    const c = this.cells.get(this._key(a, b, hour));
    if (!c) return 0;
    const d = this._decayed(c, now);
    return d.s + d.f;
  }
  // The model is the prior, worth PRIOR_WEIGHT observations; measurements replace it as they come.
  p(a, b, hour, now, prior) {
    const c = this.cells.get(this._key(a, b, hour));
    if (!c) return prior;
    const d = this._decayed(c, now);
    return (PRIOR_WEIGHT * prior + d.s) / (PRIOR_WEIGHT + d.s + d.f);
  }
  // Should a short test beacon be scheduled for this pair now? Only where few samples exist, and
  // never more than 1 percent of a window's air time (`usedS` seconds already spent in `windowS`).
  exploreDue(a, b, hour, now, usedS, windowS, beaconS = 1) {
    return this.count(a, b, hour, now) < 5 && (usedS + beaconS) / windowS <= 0.01;
  }
}

// When an operator tends to listen: replies and HERE messages by hour of the week.
export class ListenPattern {
  constructor() {
    this.b = new Array(168).fill(0);
    this.n = 0;
  }
  static slot(ms) {
    const d = new Date(ms);
    return d.getUTCDay() * 24 + d.getUTCHours();
  }
  observe(ms) {
    this.b[ListenPattern.slot(ms)] += 1;
    this.n += 1;
  }
  // The `count` best paging windows (starts, ms, every 10th minute) within the horizon, in time
  // order; null until there are three observations to go by.
  bestWindows(now, count, horizonMs = 24 * 3600e3, period = 600e3) {
    if (this.n < 3) return null;
    const first = Math.ceil(now / period) * period;
    const cand = [];
    for (let t = first; t < now + horizonMs; t += period) cand.push({ t, score: this.b[ListenPattern.slot(t)] });
    cand.sort((x, y) => y.score - x.score || x.t - y.t);
    return cand.slice(0, count).map((c) => c.t).sort((x, y) => x - y);
  }
}
