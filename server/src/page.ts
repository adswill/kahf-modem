// Server-rendered pages for kahf.dev: the home page (renderHome) and the live
// map (renderMap). Plain HTML, CSS and SVG with one small inline script shared
// by both pages (src/web/script.js). No external scripts, stylesheets, fonts,
// map tiles. The home page has one image, served by this Worker at /operate.png. Both pages are complete without the script.
//
// Written without type annotations: the types are given in JSDoc, so the file
// also runs as plain JavaScript (the tests load it under Node 20).
import { encodeBase64, radiusDegrees } from "./core.js";
import { CSS } from "./web/styles.js";
import { HOME_SCRIPT } from "./web/script.js";
import { LIVE_MAP_SCRIPT } from "./web/live-map.js";
import { homeBody } from "./web/home.js";
import { SITE_PAGES } from "./web/pages.js";

/**
 * @typedef {import("./types").Location} Location
 * @typedef {import("./types").PublicStation} PublicStation
 * @typedef {{ callsign: string, last_seen: string, location: Location }} MapRelay
 * @typedef {{ id: string, role: "relay" | "station", call: string, loc: Location, lastSeen: string, station: PublicStation | null }} MapNode
 */

const VIEW_W = 1000;
const VIEW_H = 500;

/** Escapes text for HTML element content and attribute values. */
function esc(value) {
  return String(value).replace(/[&<>"']/g, (c) => {
    switch (c) {
      case "&": return "&amp;";
      case "<": return "&lt;";
      case ">": return "&gt;";
      case '"': return "&quot;";
      default: return "&#39;";
    }
  });
}

const NA = `<span class="na">Not available yet</span>`;

// ---------------------------------------------------------------------------
// Map window: the map zooms to the nodes it has, so a circle of tens of
// kilometres is big enough to see.
// ---------------------------------------------------------------------------

/**
 * @param {MapNode[]} nodes
 * @returns {{ lon0: number, lat1: number, span: number }} west edge, north edge, width in degrees
 */
function mapWindow(nodes) {
  let minLon = 180, maxLon = -180, minLat = 90, maxLat = -90;
  for (const n of nodes) {
    const { latDeg, lonDeg } = n.loc.mode === "area" ? radiusDegrees(n.loc.radius_km, n.loc.latitude) : { latDeg: 0, lonDeg: 0 };
    minLon = Math.min(minLon, n.loc.longitude - lonDeg);
    maxLon = Math.max(maxLon, n.loc.longitude + lonDeg);
    minLat = Math.min(minLat, n.loc.latitude - latDeg);
    maxLat = Math.max(maxLat, n.loc.latitude + latDeg);
  }
  if (nodes.length === 0) return { lon0: -180, lat1: 90, span: 360 };
  let span = Math.max(8, (maxLon - minLon) * 1.5, (maxLat - minLat) * 3);
  span = Math.min(360, span);
  const cLon = (minLon + maxLon) / 2;
  const cLat = (minLat + maxLat) / 2;
  const lon0 = Math.max(-180, Math.min(180 - span, cLon - span / 2));
  const lat1 = Math.max(-90 + span / 2, Math.min(90, cLat + span / 4));
  return { lon0, lat1, span };
}

/** A round step for graticule lines, giving about four to eight lines across. */
function gridStep(span) {
  for (const s of [30, 10, 5, 2, 1, 0.5, 0.2, 0.1]) if (span / s >= 4) return s;
  return 0.1;
}

/** Plain-words position for the panels. Coordinates only when the node shared a precise point. */
function positionText(loc) {
  if (loc.mode === "precise") return `${loc.latitude.toFixed(4)}, ${loc.longitude.toFixed(4)}`;
  return `Area, radius ${loc.radius_km} km`;
}

function positionNote(loc) {
  if (loc.mode === "precise") {
    return "This position was shared as a point. Anyone who can see the map can see it to about 200 m.";
  }
  return `This position was shared as an area. The map shows a circle of radius ${loc.radius_km} km, and the station is somewhere inside it. The circle's centre is not the station's position.`;
}

/** Age in whole seconds, for the filters. A time that does not parse counts as very old. */
function ageSeconds(iso, nowIso) {
  const s = Math.round((Date.parse(nowIso) - Date.parse(iso)) / 1000);
  return Number.isFinite(s) ? Math.max(0, s) : 999999999;
}

/** Age in plain words, from two ISO times. */
function ageText(iso, nowIso) {
  const then = Date.parse(iso);
  const now = Date.parse(nowIso);
  if (!Number.isFinite(then) || !Number.isFinite(now)) return "unknown";
  const s = Math.max(0, Math.round((now - then) / 1000));
  if (s < 90) return "under 2 minutes ago";
  if (s < 5400) return `${Math.round(s / 60)} minutes ago`;
  if (s < 172800) return `${Math.round(s / 3600)} hours ago`;
  return `${Math.round(s / 86400)} days ago`;
}

/**
 * Nodes for the map: each relay and heard station once, by callsign. The first
 * report for a callsign supplies its panel. A relay that registered adds a node
 * only when no report already shows it.
 * @param {PublicStation[]} stations
 * @param {MapRelay[]} relays
 * @returns {MapNode[]}
 */
function buildNodes(stations, relays) {
  /** @type {MapNode[]} */
  const nodes = [];
  const seen = new Set();
  stations.forEach((s, i) => {
    if (!seen.has(`relay:${s.reporter_call}`)) {
      seen.add(`relay:${s.reporter_call}`);
      nodes.push({ id: `r-${i}`, role: "relay", call: s.reporter_call, loc: s.reporter_location, lastSeen: s.refreshed_at, station: s });
    }
    if (!seen.has(`station:${s.heard_call}`)) {
      seen.add(`station:${s.heard_call}`);
      nodes.push({ id: `h-${i}`, role: "station", call: s.heard_call, loc: s.heard_location, lastSeen: s.heard_at, station: s });
    }
  });
  relays.forEach((r, i) => {
    if (seen.has(`relay:${r.callsign}`)) return;
    seen.add(`relay:${r.callsign}`);
    nodes.push({ id: `m-${i}`, role: "relay", call: r.callsign, loc: r.location, lastSeen: r.last_seen, station: null });
  });
  return nodes;
}

/**
 * The map as one SVG. Each node is a link to its panel.
 * A precise node is a small square. An area node is a dashed circle at true scale
 * and no mark at its centre, because the centre of the circle is not where the
 * station is.
 * @param {MapNode[]} nodes
 * @param {PublicStation[]} stations
 * @param {string} nowIso
 */
function mapSvg(nodes, stations, nowIso) {
  const win = mapWindow(nodes);
  const latSpan = win.span / 2;
  const X = (lon) => ((lon - win.lon0) / win.span) * VIEW_W;
  const Y = (lat) => ((win.lat1 - lat) / latSpan) * VIEW_H;
  const step = gridStep(win.span);
  const dec = step < 1 ? 1 : 0;

  let grid = "";
  for (let lon = Math.ceil(win.lon0 / step) * step; lon <= win.lon0 + win.span + 1e-9; lon += step) {
    const x = X(lon);
    grid += `<line class="graticule" x1="${x.toFixed(2)}" y1="0" x2="${x.toFixed(2)}" y2="${VIEW_H}"/>` +
      `<text class="glabel" x="${(x + 4).toFixed(2)}" y="${VIEW_H - 6}">${lon.toFixed(dec)}</text>`;
  }
  for (let lat = Math.ceil((win.lat1 - latSpan) / step) * step; lat <= win.lat1 + 1e-9; lat += step) {
    const y = Y(lat);
    grid += `<line class="graticule" x1="0" y1="${y.toFixed(2)}" x2="${VIEW_W}" y2="${y.toFixed(2)}"/>` +
      `<text class="glabel" x="4" y="${(y - 4).toFixed(2)}">${lat.toFixed(dec)}</text>`;
  }

  const byCall = new Map(nodes.map((n) => [`${n.role}:${n.call}`, n]));
  let links = "";
  for (const s of stations) {
    const a = byCall.get(`relay:${s.reporter_call}`);
    const b = byCall.get(`station:${s.heard_call}`);
    if (!a || !b || a.loc.mode !== "precise" || b.loc.mode !== "precise") continue;
    links += `<line class="link" x1="${X(a.loc.longitude).toFixed(2)}" y1="${Y(a.loc.latitude).toFixed(2)}" x2="${X(b.loc.longitude).toFixed(2)}" y2="${Y(b.loc.latitude).toFixed(2)}"/>`;
  }

  let markers = "";
  for (const n of nodes) {
    const x = X(n.loc.longitude);
    const y = Y(n.loc.latitude);
    const cx = x.toFixed(2);
    const cy = y.toFixed(2);
    let shape;
    if (n.loc.mode === "precise") {
      shape = `<rect class="mk ${n.role}" x="${(x - 5).toFixed(2)}" y="${(y - 5).toFixed(2)}" width="10" height="10"/>` +
        `<rect class="hit" x="${(x - 14).toFixed(2)}" y="${(y - 14).toFixed(2)}" width="28" height="28"/>` +
        `<text class="lbl" x="${(x + 12).toFixed(2)}" y="${(y - 8).toFixed(2)}">${esc(n.call)}</text>`;
    } else {
      const { latDeg, lonDeg } = radiusDegrees(n.loc.radius_km, n.loc.latitude);
      const rx = (lonDeg / win.span) * VIEW_W;
      const ry = (latDeg / latSpan) * VIEW_H;
      shape = `<ellipse class="area ${n.role}" cx="${cx}" cy="${cy}" rx="${rx.toFixed(2)}" ry="${ry.toFixed(2)}"/>` +
        `<text class="lbl" x="${(x + 8).toFixed(2)}" y="${(y - ry - 6).toFixed(2)}">${esc(n.call)}</text>`;
    }
    markers += `<a class="marker" href="#${n.id}" data-panel="${n.id}" data-role="${n.role}" data-age="${ageSeconds(n.lastSeen, nowIso)}">` +
      `<title>${esc(n.call)}, ${n.role}</title>${shape}</a>`;
  }

  return `<svg viewBox="0 0 ${VIEW_W} ${VIEW_H}" role="group" aria-label="Map of relays and stations, latitude and longitude in degrees">` +
    `<g>${grid}</g><g>${links}</g><g>${markers}</g></svg>`;
}

/**
 * One panel. Hops and message counts are not in the station store yet, so they
 * say so instead of showing a number.
 * @param {MapNode} n
 * @param {string} nowIso
 */
function panel(n, nowIso) {
  const s = n.station;
  const roleName = n.role === "relay" ? "Relay" : "Station";
  const radio = s ? `<dt>Frequency and mode</dt><dd>${esc(s.frequency_khz)} kHz, ${esc(s.mode)}</dd>\n` : "";
  return `<section class="panel card" id="${n.id}" aria-labelledby="${n.id}-title">
<h2 id="${n.id}-title">${esc(n.call)}</h2>
<p class="role">${roleName}</p>
<dl class="kv">
<dt>Role</dt><dd>${roleName}</dd>
<dt>Last seen (UTC)</dt><dd>${esc(n.lastSeen)}</dd>
<dt>Age</dt><dd>${esc(ageText(n.lastSeen, nowIso))}</dd>
<dt>Position</dt><dd>${esc(positionText(n.loc))}</dd>
${radio}<dt>Hops, last 24 hours</dt><dd>${NA}</dd>
<dt>Messages sent today</dt><dd>${NA}</dd>
<dt>Messages sent in total</dt><dd>${NA}</dd>
</dl>
<p class="note">${esc(positionNote(n.loc))}</p>
<p><a href="#map" data-close>Close</a></p>
</section>`;
}

/** @param {MapNode[]} nodes */
function nodeList(nodes, nowIso) {
  const rows = nodes
    .slice()
    .sort((a, b) => a.call.localeCompare(b.call))
    .map((n) =>
      `<li data-row="${n.id}" data-role="${n.role}" data-age="${ageSeconds(n.lastSeen, nowIso)}"><a href="#${n.id}" data-panel="${n.id}">` +
      `<span class="cs">${esc(n.call)}</span><span class="meta">${n.role === "relay" ? "Relay" : "Station"}</span>` +
      `<span class="meta">${n.loc.mode === "precise" ? "Point" : `Circle, ${esc(n.loc.radius_km)} km`}</span>` +
      `<span class="meta">${esc(n.lastSeen.replace(/\.\d+Z$/, "Z"))}</span></a></li>`)
    .join("\n");
  return `<ul class="list" aria-label="All nodes">
${rows}
</ul>`;
}

const EMPTY_STATE = `<p>No relays or stations have reported in the last 48 hours.</p>
<p>A node shows here after its operator turns on location sharing and a relay sends a signed report for it. Station reporting is still being built into the app, so an empty map is normal at this stage. Come back after the first release.</p>`;

/**
 * Wraps a page body in the shared header, footer and script.
 * @param {string} title
 * @param {string} description
 * @param {string} current the page key: "home", "map" or an inner page name
 * @param {string} body
 * @param {string} script the inline script text for this page
 * @param {boolean} [wide] let the page use the full window width
 */
function shell(title, description, current, body, script, wide = false, head = "", beforeScript = "") {
  const cur = (name) => (current === name ? ' aria-current="page"' : "");
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="description" content="${esc(description)}">
<meta name="color-scheme" content="light dark">
<title>${esc(title)}</title>
<style>${CSS}</style>
${head}
</head>
<body data-page="${current}">
<a class="skip" href="#main">Skip to content</a>
<header class="top">
<div class="wrap${wide ? " wide" : ""}">
<a class="brand" href="/">KAHF</a>
<nav aria-label="Main">
<a href="/network"${cur("network")}>Network</a>
<a href="/map"${cur("map")}>Live map</a>
<a href="/protocol"${cur("protocol")}>Modem</a>
<a href="/start"${cur("start")}>Get started</a>
<a href="/legal"${cur("legal")}>Legal</a>
</nav>
<div class="tools"><button type="button" class="btn js-only" data-act="theme">Dark theme</button></div>
</div>
</header>
<main id="main" tabindex="-1">
${body}
</main>
<footer class="foot">
<div class="wrap${wide ? " wide" : ""}">
<div class="cols">
<div><h3>KAHF</h3><p>An HF digital modem and relay network for licensed amateur radio operators, with a focus on 17 metres. Pre-release software. Copyright (c) 2026 adswill.</p></div>
<div><h3>Site</h3><ul><li><a href="/">Home</a></li><li><a href="/network">Relay network</a></li><li><a href="/map">Live map</a></li></ul></div>
<div><h3>Project</h3><ul><li><a href="/start">Get started</a></li><li><a href="/protocol">Modem details</a></li><li><a href="/legal">Licence and privacy</a></li></ul></div>
</div>
</div>
</footer>
${beforeScript}
<script>${script}</script>
</body>
</html>`;
}

/**
 * An inner page, by path (for example "/network"), or null when there is none.
 * @param {string} path
 * @returns {string | null}
 */
export function renderSite(path) {
  const page = SITE_PAGES[path];
  if (!page) return null;
  return shell(page.title, page.description, path.slice(1), page.body(), HOME_SCRIPT);
}

/** The paths of the inner pages. */
export const SITE_PATHS = Object.keys(SITE_PAGES);

/** The kahf.dev home page. */
export function renderHome() {
  return shell(
    "KAHF: HF data modem and relay network for 17 metres",
    "KAHF sends messages, mail and files between amateur radio stations over HF, relayed across a network of volunteer stations. Built for 17 metres, works with ordinary radios or SDRs.",
    "home",
    homeBody(),
    HOME_SCRIPT,
  );
}

/**
 * The live map for the given non-expired reports and registered relays.
 * Precise points and area circles only; the data holds nothing finer.
 * @param {PublicStation[]} stations
 * @param {string} generatedIso
 * @param {MapRelay[]} [relays]
 */
/** Map library: pinned version, with its integrity hashes checked by the browser. */
const MAPLIBRE = "https://unpkg.com/maplibre-gl@5.7.3/dist";
const MAPLIBRE_CSS_SRI = "sha384-gNYNsUmuZqDYiT3gbirWTV5K7rt71RoveS/yXAaU09d4ZUmeDVTD3XoqB6uJAIFR";
const MAPLIBRE_JS_SRI = "sha384-58glfsJ+cxARGnts6etFXqTluJx24YUgVY1WQxNLIfLkwT7yJfBORU1dKLD/YFTG";

/** The live map page. The map is drawn by MapLibre; data comes from /map-data once a second. */
export function renderMap(stations, generatedIso, relays = []) {
  const reporters = new Set(stations.map((s) => s.reporter_call)).size;
  const head = `<link rel="stylesheet" href="${MAPLIBRE}/maplibre-gl.css" integrity="${MAPLIBRE_CSS_SRI}" crossorigin="anonymous">`;
  const beforeScript = `<script src="${MAPLIBRE}/maplibre-gl.js" integrity="${MAPLIBRE_JS_SRI}" crossorigin="anonymous"></script>`;
  const body = `
<div class="wrap wide">
<div class="maphead">
<h1 id="map-t">Live map</h1>
<p class="status" id="refresh-status" role="status"><span class="dot" aria-hidden="true"></span><span>Connecting to the live feed.</span></p>
</div>
<div class="livestage">
<div class="livemap" id="map" role="region" aria-label="Map of relays and stations"><noscript>The live map needs JavaScript.</noscript></div>
<aside class="liveside">
<section class="panel" aria-live="polite" aria-label="Selected node">
<h2>Selected</h2>
<dl class="kv">
<dt>Callsign</dt><dd id="p-call">None selected</dd>
<dt>Role</dt><dd id="p-role">-</dd>
<dt>Last seen</dt><dd id="p-seen">-</dd>
<dt>Position</dt><dd id="p-loc">-</dd>
</dl>
<p class="note">Choose a point on the map to see its details.</p>
</section>
<p class="count" id="count">${relays.length} relay${relays.length === 1 ? "" : "s"}, ${reporters} station${reporters === 1 ? "" : "s"} reporting. Updated ${esc(generatedIso.replace(/\.\d+Z$/, "Z"))}.</p>
<p class="note">Map tiles from OpenStreetMap contributors. Green circles are relays, dark circles are stations, grey circles are stations heard by others. A dashed ring marks a position given only as an area.</p>
</aside>
</div>
</div>`;
  return shell(
    "KAHF live map",
    "Relays and stations on the KAHF map, updated every second.",
    "map",
    body,
    LIVE_MAP_SCRIPT,
    true,
    head,
    beforeScript,
  );
}

/** Kept so existing callers of the old name still get the map. */
export const renderPage = renderMap;

const hashes = new Map();

/** @param {string} text @returns {Promise<string>} */
async function cspHash(text) {
  if (!hashes.has(text)) {
    const digest = await globalThis.crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
    hashes.set(text, `'sha256-${encodeBase64(new Uint8Array(digest))}'`);
  }
  return hashes.get(text);
}

/**
 * The CSP source for the map page's one inline script, e.g. "'sha256-...'".
 * The hash covers the exact text between the script tags.
 * @returns {Promise<string>}
 */
export function mapScriptSha256() {
  return cspHash(LIVE_MAP_SCRIPT);
}

/**
 * The CSP source for the home page's one inline script. The home page also
 * calls GET /map-data and GET /release, so its policy needs connect-src 'self'.
 * @returns {Promise<string>}
 */
export function homeScriptSha256() {
  return cspHash(HOME_SCRIPT);
}
