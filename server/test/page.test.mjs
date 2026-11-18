// Tests for the home page and the live map in src/page.ts. Run with: npm test (node --test).
// Node 20 cannot import .ts files, so a small load hook reads page.ts as JavaScript.
// No wrangler, no network.
import { test } from "node:test";
import assert from "node:assert/strict";
import { register } from "node:module";

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
const { renderHome, renderMap, renderPage } = await import("../src/page.ts");

const GENERATED = "2026-11-17T12:00:00.000Z";

const STATIONS = [
  {
    reporter_call: "KA1RLY",
    reporter_location: { mode: "precise", latitude: 25.2048, longitude: 55.2708 },
    heard_call: "KA2STN",
    heard_location: { mode: "area", latitude: 24.4539, longitude: 54.3773, radius_km: 20 },
    heard_at: "2026-11-17T11:40:00Z",
    frequency_khz: 145500,
    mode: "KAHF",
    refreshed_at: "2026-11-17T11:55:00.000Z",
  },
];

const EXTERNAL = /https?:\/\/|\/\/[a-z0-9.-]+\.[a-z]{2,}|<script[^>]*\bsrc=|<link\b|@import/i;

test("home page has the statement, what it does, how it travels and downloads", () => {
  const html = renderHome();
  assert.match(html, /<h1[^>]*>KAHF sends messages, mail and files between amateur radio stations over HF\.<\/h1>/);
  assert.equal((html.match(/<h1\b/g) || []).length, 1);
  assert.match(html, /id="what"/);
  assert.match(html, /id="how"/);
  assert.match(html, /id="licences"/);
  assert.doesNotMatch(html, /id="download"/);
  assert.match(html, /href="\/map"/);
  assert.doesNotMatch(html, /no public release|Latest version|Required update|Planned builds/i);
  assert.match(html, /<img src="\/operate\.png" width="1512" height="940"/);
});

test("home page has no emoji and no external URL", () => {
  const html = renderHome();
  assert.doesNotMatch(html, /[\u{1F300}-\u{1FAFF}\u{2600}-\u{27BF}]/u);
  assert.doesNotMatch(html, EXTERNAL);
});

const MAP_HOSTS = /^(https:\/\/unpkg\.com\/maplibre-gl@5\.7\.3\/dist\/maplibre-gl\.(js|css)|https:\/\/tile\.openstreetmap\.org\/\{z\}\/\{x\}\/\{y\}\.png)$/;

test("map page loads the pinned MapLibre with integrity hashes, and one inline script", () => {
  const html = renderMap(STATIONS, GENERATED);
  assert.match(html, /<div class="livemap" id="map"/);
  assert.match(html, /integrity="sha384-[A-Za-z0-9+/=]+" crossorigin="anonymous"/);
  assert.match(html, /https:\/\/unpkg\.com\/maplibre-gl@5\.7\.3\/dist\/maplibre-gl\.js/);
  assert.equal((html.match(/<script>/g) || []).length, 1);
  assert.doesNotMatch(html, /<svg[ >]/);
});

test("map page status line and selected-node panel are there for the script to fill", () => {
  const html = renderMap(STATIONS, GENERATED);
  assert.match(html, /id="refresh-status" role="status"/);
  assert.match(html, /id="p-call"/);
  assert.match(html, /id="p-loc"/);
  assert.match(html, /id="count"/);
});

test("map page only loads the map library and map tiles from outside", () => {
  const html = renderMap(STATIONS, GENERATED);
  const urls = html.match(/https?:\/\/[^\s"'<>)]+/g) || [];
  assert.ok(urls.length > 0);
  for (const u of urls) assert.match(u, MAP_HOSTS, `unexpected URL ${u}`);
  assert.doesNotMatch(html, /<link\b(?![^>]*maplibre)/);
  assert.doesNotMatch(renderHome(), EXTERNAL);
});

test("stored text never appears in the page; it travels as data", () => {
  const evil = [{ ...STATIONS[0], reporter_call: "<img src=x onerror=alert(1)>" }];
  const html = renderMap(evil, GENERATED);
  assert.doesNotMatch(html, /<img src=x/);
  assert.doesNotMatch(html, /alert\(1\)/);
});

test("empty map says zero reports and still shows the map", () => {
  const html = renderMap([], GENERATED);
  assert.match(html, /<div class="livemap" id="map"/);
  assert.match(html, /0 relays, 0 stations/);
});

test("renderPage is the map for callers that still use the old name", () => {
  assert.equal(renderPage, renderMap);
});
