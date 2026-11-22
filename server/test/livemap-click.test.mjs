// Runs the real map script against stand-ins for the browser and MapLibre, clicks a marker, and
// checks the card it builds. No network and no WebGL needed.
import { test } from "node:test";
import assert from "node:assert/strict";
import vm from "node:vm";

const { LIVE_MAP_SCRIPT } = await import("../src/web/live-map.js");

class Node_ {
  constructor(tag) {
    this.tag = tag; this.children = []; this.attrs = {}; this.listeners = {}; this.className = ""; this.hidden = false;
    this._text = "";
    this.classList = { add() {}, remove() {}, contains: () => false };
  }
  set textContent(v) { this._text = String(v); this.children = []; }
  get textContent() { return this._text + this.children.map((c) => c.textContent).join(" "); }
  appendChild(c) { this.children.push(c); return c; }
  setAttribute(k, v) { this.attrs[k] = v; }
  getAttribute(k) { return this.attrs[k] ?? null; }
  addEventListener(ev, fn) { (this.listeners[ev] ||= []).push(fn); }
  querySelector() { return null; }
  querySelectorAll() { return []; }
  get lastChild() { return this.children[this.children.length - 1] ?? null; }
}

function run(data) {
  const byId = new Map();
  for (const id of ["map", "refresh-status", "count", "p-call", "p-role", "p-seen", "p-loc", "p-info", "map-empty"]) byId.set(id, new Node_("div"));
  byId.get("refresh-status").appendChild(new Node_("span"));
  byId.get("map-empty").hidden = true;
  const doc = {
    documentElement: new Node_("html"),
    getElementById: (id) => byId.get(id) ?? null,
    createElement: (t) => new Node_(t),
    createElementNS: (_n, t) => new Node_(t),
    querySelector: () => null, querySelectorAll: () => [],
    addEventListener() {},
  };
  const markers = [];
  const popups = [];
  class Marker {
    constructor(o) { this.el = o.element; markers.push(this); }
    setLngLat(c) { this.at = c; return this; }
    addTo() { return this; }
    remove() { this.removed = true; }
    getElement() { return this.el; }
  }
  class Popup {
    constructor(o) { this.opts = o; popups.push(this); }
    setLngLat(c) { this.at = c; return this; }
    setDOMContent(n) { this.content = n; return this; }
    addTo() { return this; }
    remove() { this.removed = true; }
    on() { return this; }
  }
  class Map_ {
    constructor() { this.handlers = {}; }
    on(ev, fn) { if (ev === "load") fn(); }
    addControl() {}
    addSource() {}
    addLayer() {}
    getSource() { return { setData() {} }; }
  }
  const window_ = {
    maplibregl: { Map: Map_, Marker, Popup, NavigationControl: class {} },
    location: { origin: "https://kahf.test" },
    localStorage: { getItem: () => null, setItem() {} },
    matchMedia: () => ({ matches: false, addEventListener() {}, addListener() {} }),
    fetch: async () => ({ ok: true, text: async () => JSON.stringify(data), json: async () => data }),
    setInterval: () => 1, setTimeout: () => 1, clearInterval() {},
    addEventListener() {},
  };
  const ctx = vm.createContext({ document: doc, window: window_, setInterval: () => 1, setTimeout: () => 1, Date, Math, JSON, isFinite, Number, Object, Array, Promise });
  vm.runInContext(LIVE_MAP_SCRIPT, ctx);
  return { markers, popups, byId };
}

const NOW = new Date().toISOString();
const DATA = {
  generated_at: NOW,
  relays: [{ callsign: "W2HUB", mode: "relay", last_seen: NOW, location: { mode: "area", latitude: 40.5, longitude: -73.5, radius_km: 79 } }],
  stations: [{
    reporter_call: "G4ABC", reporter_location: { mode: "area", latitude: 51.5, longitude: -0.1, radius_km: 20 },
    heard_call: "DL5XYZ", heard_location: { mode: "area", latitude: 51.5, longitude: -0.1, radius_km: 20 },
    heard_at: NOW, frequency_khz: 14103, mode: "KAHF", refreshed_at: NOW,
  }],
};

const settle = () => new Promise((r) => setTimeout(r, 20));

test("clicking a relay opens a card with its details", async () => {
  const { markers, popups } = run(DATA);
  await settle();
  const relay = markers.find((m) => /relay/.test(m.el.className));
  assert.ok(relay, "a marker for the relay");
  for (const fn of relay.el.listeners.click) fn();
  assert.equal(popups.length, 1, "a card opens on the map");
  const text = popups[0].content.textContent;
  assert.match(text, /W2HUB/);
  assert.match(text, /Relay/);
  assert.match(text, /Passes messages between stations/);
  assert.match(text, /circle about 79 km/);
  assert.match(text, /Last seen/);
  assert.equal(popups[0].opts.className, "nodepop");
});

test("nodes at the same point are listed together in one card", async () => {
  const { markers, popups } = run(DATA);
  await settle();
  const st = markers.find((m) => /station/.test(m.el.className));
  for (const fn of st.el.listeners.click) fn();
  const text = popups.at(-1).content.textContent;
  assert.match(text, /G4ABC/);
  assert.match(text, /Reported hearing DL5XYZ/);
  assert.match(text, /14\.103 MHz, KAHF/);
  assert.match(text, /DL5XYZ/);
  assert.match(text, /Known only because G4ABC heard it/);
  assert.match(text, /Heard only/);
});

test("the chosen marker is highlighted and the side panel is filled in", async () => {
  const { markers, byId } = run(DATA);
  await settle();
  const relay = markers.find((m) => /relay/.test(m.el.className));
  for (const fn of relay.el.listeners.click) fn();
  assert.match(relay.el.className, / sel$/);
  assert.equal(byId.get("p-call").textContent, "W2HUB");
  assert.match(byId.get("p-loc").textContent, /circle about 79 km/);
  assert.match(byId.get("p-info").textContent, /Passes messages/);
});

test("an empty map says why, and the message goes away once there is a node", async () => {
  const empty = run({ generated_at: NOW, relays: [], stations: [] });
  await settle();
  assert.equal(empty.byId.get("map-empty").hidden, false);
  const full = run(DATA);
  await settle();
  assert.equal(full.byId.get("map-empty").hidden, true);
});
