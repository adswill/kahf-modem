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
  assert.match(html, /<h1[^>]*>KAHF sends messages, mail and files between amateur radio stations over HF and VHF\.<\/h1>/);
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

test("map page has an SVG, the panels and one inline script", () => {
  const html = renderMap(STATIONS, GENERATED);
  assert.match(html, /<svg[ >]/);
  assert.match(html, /class="panel card" id="r-0"/);
  assert.match(html, /class="panel card" id="h-0"/);
  assert.match(html, /data-panel="r-0"/);
  assert.equal((html.match(/<script>/g) || []).length, 1);
});

test("map panels list callsign, role, last seen, hops, and messages", () => {
  const html = renderMap(STATIONS, GENERATED);
  assert.match(html, /<h2 id="r-0-title">KA1RLY<\/h2>/);
  assert.match(html, /<dt>Role<\/dt><dd>Relay<\/dd>/);
  assert.match(html, /<dt>Role<\/dt><dd>Station<\/dd>/);
  assert.match(html, /<dt>Last seen \(UTC\)<\/dt><dd>2026-11-17T11:55:00.000Z<\/dd>/);
  assert.match(html, /<dt>Hops, last 24 hours<\/dt>/);
  assert.match(html, /<dt>Messages sent today<\/dt>/);
  assert.match(html, /<dt>Messages sent in total<\/dt>/);
});

test("map page has no external script, stylesheet, font or map tile", () => {
  assert.doesNotMatch(renderMap(STATIONS, GENERATED), EXTERNAL);
  assert.doesNotMatch(renderHome(), EXTERNAL);
  assert.doesNotMatch(renderMap([], GENERATED), EXTERNAL);
});

test("area markers are dashed circles, precise ones are squares", () => {
  const html = renderMap(STATIONS, GENERATED);
  assert.match(html, /<ellipse class="area station"/);
  assert.match(html, /<rect class="mk relay" x="[\d.]+" y="[\d.]+" width="10" height="10"\/>/);
});

test("callsigns from reports are escaped", () => {
  const evil = [{ ...STATIONS[0], reporter_call: "<img src=x onerror=alert(1)>" }];
  const html = renderMap(evil, GENERATED);
  assert.doesNotMatch(html, /<img src=x/);
  assert.match(html, /&lt;img src=x/);
});

test("empty map says so, says what to do and has no SVG", () => {
  const html = renderMap([], GENERATED);
  assert.match(html, /No relays or stations have reported/);
  assert.match(html, /location sharing/);
  assert.doesNotMatch(html, /<svg/);
  assert.match(html, /0 reports shown/);
});

test("renderPage is the map for callers that still use the old name", () => {
  assert.equal(renderPage, renderMap);
});
