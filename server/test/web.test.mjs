// Tests for the public pages: sections, the map with and without data, precise
// and area nodes, no external loads, the inline scripts, and the reduced-motion path.
import { test } from "node:test";
import assert from "node:assert/strict";
import { register } from "node:module";
import vm from "node:vm";

const HOOK = `
import { readFile } from "node:fs/promises";
export async function load(url, context, nextLoad) {
  if (url.endsWith(".ts")) {
    const source = await readFile(new URL(url), "utf8");
    return { format: "module", source, shortCircuit: true };
  }
  return nextLoad(url, context);
}`;
register("data:text/javascript," + encodeURIComponent(HOOK));
const { renderHome, renderMap, mapScriptSha256, homeScriptSha256 } = await import("../src/page.ts");
const { HOME_SCRIPT } = await import("../src/web/script.js");
const { LIVE_MAP_SCRIPT: MAP_SCRIPT } = await import("../src/web/live-map.js");
const { CSS } = await import("../src/web/styles.js");
const { handleRequest } = await import("../src/routes.js");
const { fakeStore } = await import("../tools/fake-store.mjs");
const { readFile } = await import("node:fs/promises");

const NOW = "2026-11-17T12:00:00.000Z";
const PRECISE = { mode: "precise", latitude: 25.2048, longitude: 55.2708 };
const AREA = { mode: "area", latitude: 24.4539, longitude: 54.3773, radius_km: 20 };
const STATION = {
  reporter_call: "A61RLY", reporter_location: PRECISE,
  heard_call: "A62STN", heard_location: AREA,
  heard_at: "2026-11-17T11:40:00Z", frequency_khz: 14074, mode: "KAHF",
  refreshed_at: "2026-11-17T11:55:00.000Z",
};
const RELAY = { callsign: "A64REL", last_seen: "2026-11-17T11:50:00.000Z", location: { mode: "area", latitude: 24.5, longitude: 54.5, radius_km: 79 } };

const EXTERNAL = /https?:\/\/|\/\/[a-z0-9.-]+\.[a-z]{2,}|<script[^>]*\bsrc=|<link\b|@import|url\(/i;

function scriptsOf(html) {
  return [...html.matchAll(/<script>([\s\S]*?)<\/script>/g)].map((m) => m[1]);
}


const EMOJI = /[\u{1F000}-\u{1FAFF}\u{2190}-\u{21FF}\u{2600}-\u{27BF}\u{2B00}-\u{2BFF}\u{FE0F}]/u;

test("home page: no downloads block without a release, and the real modem numbers", () => {
  const html = renderHome();
  for (const id of ["top-title", "what", "how", "licences"]) assert.match(html, new RegExp(`id="${id}"`), id);
  assert.equal((html.match(/<h1\b/g) || []).length, 1);
  assert.equal((html.match(/<section\b/g) || []).length, 3);
  assert.doesNotMatch(html, /no public release|Latest version|Required update|Planned builds|id="download"/i);
  assert.match(html, /No religious or political content/);
  assert.match(html, /Source-available/);
  assert.match(html, /class="skip" href="#main"/);
  assert.match(html, /<svg[^>]*aria-labelledby="dg-t dg-d"/);
  assert.match(html, /153 carriers, 15\.625 Hz apart/);
  assert.match(html, /312 Hz/);
  assert.match(html, /2\.69 kHz/);
  assert.equal((html.match(/<line class="(?:carr|pilot)"/g) || []).length, 153);
  assert.doesNotMatch(html, /<canvas|Simulation|Generated signal/);
});

test("the screenshot is one real file, shown at its true size", async () => {
  const html = renderHome();
  assert.equal((html.match(/<img\b/g) || []).length, 1);
  assert.match(html, /<img src="\/operate\.png" width="1512" height="940" alt="[^"]+"/);
  const png = await readFile(new URL("../src/web/operate.png", import.meta.url));
  assert.equal(png.readUInt32BE(16), 1512);
  assert.equal(png.readUInt32BE(20), 940);
  const res = await handleRequest(new Request("https://kahf.test/operate.png"), fakeStore(), Date.parse(NOW));
  assert.equal(res.status, 200);
  assert.equal(res.headers.get("content-type"), "image/png");
  assert.deepEqual(Buffer.from(await res.arrayBuffer()), png);
});

test("no gradient, shadow, filter, blur or emoji in the CSS or the pages", () => {
  assert.doesNotMatch(CSS, /gradient|shadow|backdrop-filter|filter\s*:|blur|glow/i);
  for (const html of [renderHome(), renderMap([STATION], NOW, [RELAY]), renderMap([], NOW, [])]) {
    assert.doesNotMatch(html.replace(/<style>[\s\S]*?<\/style>/, ""), /gradient|shadow|<filter/i);
    assert.doesNotMatch(html, EMOJI);
  }
  assert.doesNotMatch(renderHome(), /lorem|ipsum|TODO|TBD|coming soon/i);
});

test("every border-radius is 3 px or less", () => {
  const radii = [...CSS.matchAll(/border-radius\s*:\s*([^;}]+)/g)].map((m) => m[1]);
  for (const r of radii) {
    for (const n of r.matchAll(/([\d.]+)(px|%|em|rem)?/g)) {
      assert.ok(n[2] === "px" || n[1] === "0", `radius ${r}`);
      assert.ok(Number(n[1]) <= 3, `radius ${r}`);
    }
  }
  assert.doesNotMatch(CSS, /\brx\s*:/);
});

test("system fonts only, a serif display face and a mono face for data", () => {
  assert.doesNotMatch(CSS, /@font-face|fonts\.googleapis|url\(/);
  assert.match(CSS, /--serif: "Iowan Old Style"/);
  assert.match(CSS, /--mono: ui-monospace/);
});

const MAP_LIB = /https:\/\/unpkg\.com\/maplibre-gl@5\.7\.3\/dist\/maplibre-gl\.(js|css)/;
const TILES = 'window.location.origin + "/tiles/{z}/{x}/{y}.png"';

test("the map is MapLibre with OpenStreetMap raster tiles and the pinned library", () => {
  const html = renderMap([STATION], NOW, [RELAY]);
  assert.match(html, MAP_LIB);
  assert.match(MAP_SCRIPT, /maplibregl/);
  assert.ok(MAP_SCRIPT.includes(TILES));
  assert.match(MAP_SCRIPT, /attribution: "© OpenStreetMap contributors"/);
  assert.match(html, /<div class="livemap" id="map"/);
});

test("the map polls /map-data once a second and only sets data when it changed", () => {
  assert.doesNotThrow(() => new vm.Script(MAP_SCRIPT));
  assert.match(MAP_SCRIPT, /var POLL_MS = 1000;/);
  assert.match(MAP_SCRIPT, /window\.setInterval\(poll, POLL_MS\)/);
  assert.match(MAP_SCRIPT, /window\.fetch\("\/map-data", \{ cache: "no-store", credentials: "omit"/);
  assert.match(MAP_SCRIPT, /if \(text !== lastBody\)/);
  assert.match(MAP_SCRIPT, /syncMarkers\(\);/);
  assert.doesNotMatch(MAP_SCRIPT, /location\.reload|setInterval\(refresh, 60000\)/);
  assert.doesNotMatch(MAP_SCRIPT, /\beval\(|new Function|document\.write|sendBeacon|WebSocket|XMLHttpRequest|innerHTML/);
});

test("the map draws precise stations as points and area positions as dashed circles", () => {
  assert.match(MAP_SCRIPT, /function ring\(lat, lon, km\)/);
  assert.match(MAP_SCRIPT, /id: "area-line"/);
  assert.match(MAP_SCRIPT, /"line-dasharray": \[3, 2\]/);
  assert.match(MAP_SCRIPT, /new window\.maplibregl\.Marker\(\{ element: el, anchor: "center" \}\)/);
  assert.match(MAP_SCRIPT, /el\.className = "mk " \+ p\.role/);
});

test("the map script makes only the feed fetch and loads only the tiles it names", () => {
  const fetches = MAP_SCRIPT.match(/\bfetch\(/g) || [];
  assert.equal(fetches.length, 2, "the shared helper and the live poll");
  const urls = MAP_SCRIPT.match(/https?:\/\/[^\s"'`)]+/g) || [];
  assert.deepEqual([...new Set(urls)], [], "no external URL in the script");
});

test("the map page has its status line, the selected-node panel and the count", () => {
  const html = renderMap([STATION], NOW, [RELAY]);
  assert.match(html, /id="refresh-status" role="status"/);
  assert.match(html, /id="p-call"/);
  assert.match(html, /id="count"/);
});

test("no external URL except the map library and its tiles, and the only image is /operate.png", () => {
  const pages = [renderHome(), renderMap([STATION], NOW, [RELAY]), renderMap([], NOW, [])];
  for (const html of pages) {
    const allowed = /^(https:\/\/unpkg\.com\/maplibre-gl@5\.7\.3\/dist\/maplibre-gl\.(js|css)|https:\/\/tile\.openstreetmap\.org\/\{z\}\/\{x\}\/\{y\}\.png)$/;
    for (const u of html.match(/https?:\/\/[^\s"'<>)]+/g) || []) assert.match(u, allowed, u);
    assert.doesNotMatch(html, /<(?:iframe|object|embed|base|form|audio|video|source)\b/i);
    assert.doesNotMatch(html, /<meta[^>]+http-equiv/i);
    for (const m of html.matchAll(/<img\b[^>]*>/g)) assert.match(m[0], /src="\/operate\.png"/);
  }
  assert.doesNotMatch(HOME_SCRIPT, /https?:\/\//);
});

test("the strict CSP is still applied to both pages and the image", async () => {
  const get = (path) => handleRequest(new Request("https://kahf.test" + path), fakeStore(), Date.parse(NOW));
  const home = (await get("/")).headers.get("content-security-policy");
  const map = (await get("/map")).headers.get("content-security-policy");
  for (const csp of [home, map]) {
    assert.match(csp, /default-src 'none'/);
    assert.match(csp, /base-uri 'none'/);
    assert.match(csp, /form-action 'none'/);
    assert.match(csp, /frame-ancestors 'none'/);
    assert.match(csp, /style-src 'unsafe-inline'/);
    assert.doesNotMatch(csp, /'unsafe-eval'|script-src[^;]*'unsafe-inline'|script-src[^;]*\*/);
  }
  assert.match(home, /img-src 'self'/);
  assert.match(home, /connect-src 'self'/);
  assert.match(map, /script-src 'sha256-[^']+' https:\/\/unpkg\.com/);
  assert.match(map, /connect-src 'self'; /);
  assert.match(map, /img-src 'self' data: blob:;/);
  assert.doesNotMatch(map, /tile\.openstreetmap/);
  assert.ok(home.includes(`script-src ${await homeScriptSha256()}`));
  assert.ok(map.includes(`script-src ${await mapScriptSha256()}`));
});

test("the home page has one inline script; the map has its library and one inline script", async () => {
  const home = renderHome();
  const map = renderMap([STATION], NOW, []);
  assert.equal(scriptsOf(home).length, 1);
  assert.equal(scriptsOf(map).length, 1);
  assert.equal(scriptsOf(home)[0], HOME_SCRIPT);
  assert.equal(scriptsOf(map)[0], MAP_SCRIPT);
  assert.match(await homeScriptSha256(), /^'sha256-[A-Za-z0-9+/=]+'$/);
  assert.notEqual(await homeScriptSha256(), await mapScriptSha256());
});

test("the home script adds a downloads section only when /release reports a version", () => {
  assert.match(HOME_SCRIPT, /if \(!info \|\| !info\.version\) return;/);
  assert.match(HOME_SCRIPT, /sec\.id = "download"/);
  assert.match(HOME_SCRIPT, /insertBefore\(sec, lic\)/);
  assert.doesNotMatch(HOME_SCRIPT, /no public release/i);
});

test("the home script makes a single fetch, to /release", () => {
  assert.doesNotThrow(() => new vm.Script(HOME_SCRIPT));
  assert.equal((HOME_SCRIPT.match(/\bfetch\(/g) || []).length, 1);
  assert.deepEqual([...HOME_SCRIPT.matchAll(/getJson\("([^"]+)"\)/g)].map((m) => m[1]), ["/release"]);
  assert.doesNotMatch(HOME_SCRIPT, /\beval\(|new Function|document\.write|sendBeacon|WebSocket|XMLHttpRequest|innerHTML/);
});

test("motion is limited to a first-paint fade and one pulse, and stops when reduced", () => {
  const keyframes = [...CSS.matchAll(/@keyframes\s+(\w+)/g)].map((m) => m[1]);
  assert.deepEqual(keyframes.sort(), ["beat", "paint"]);
  const reduce = CSS.slice(CSS.indexOf("@media (prefers-reduced-motion: reduce)"));
  assert.match(reduce, /body \{ animation: none; \}/);
  assert.match(reduce, /\.dot \{ animation: none; \}/);
  assert.doesNotMatch(CSS, /bounce|spring|cubic-bezier/);
});

test("theme follows the system and keeps a manual choice; focus rings and skip link exist", () => {
  assert.match(CSS, /prefers-color-scheme: dark/);
  assert.match(CSS, /:root\[data-theme="dark"\]/);
  assert.match(CSS, /:root:not\(\[data-theme="light"\]\)/);
  assert.match(CSS, /:focus-visible/);
  assert.match(HOME_SCRIPT, /try \{[\s\S]*localStorage[\s\S]*\} catch/);
  assert.match(HOME_SCRIPT, /kahf-theme/);
  assert.match(renderHome(), /data-act="theme"/);
});

test("the colour tokens match the design: paper, ink, one green, one red", () => {
  for (const c of ["#f4f2ec", "#1d1b15", "#5c5a52", "#d9d5cb", "#1f8a65", "#cf2d56", "#14130f"]) {
    assert.ok(CSS.includes(c), c);
  }
});

test("pages stay small", () => {
  assert.ok(renderHome().length < 60_000);
  assert.ok(renderMap([STATION], NOW, [RELAY]).length < 60_000);
});
