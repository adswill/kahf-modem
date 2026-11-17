// The live map script for kahf.dev. It loads MapLibre GL (from unpkg, with a
// checked hash in the page) and OpenStreetMap raster tiles, then asks /map-data
// for the current reports once a second. Stations are placed where they report
// from; a circle marks a position given only as an area.
import { COMMON, wrap } from "./script.js";

const LIVE = String.raw`
  var POLL_MS = 1000;
  var AREA_KM = 79;
  var EMPTY = { type: "FeatureCollection", features: [] };
  var mapEl = doc.getElementById("map");
  var statusEl = doc.getElementById("refresh-status");
  var statusText = statusEl ? statusEl.lastChild : null;
  var map = null;
  var loaded = false;
  var lastBody = "";
  var inflight = false;
  var selected = null;
  var features = { nodes: [], areas: [] };
  var markers = {};

  function clock() {
    var d = new Date();
    return pad2(d.getUTCHours()) + ":" + pad2(d.getUTCMinutes()) + ":" + pad2(d.getUTCSeconds()) + " UTC";
  }
  function setStatus(ok) {
    if (!statusText) return;
    statusText.textContent = ok
      ? "Live. Updated " + clock() + ". Checks every second while this page is open."
      : "Connection lost. Showing the last update. Retrying every second.";
    if (statusEl) statusEl.className = ok ? "status" : "status warn";
  }

  function ring(lat, lon, km) {
    var out = [], n = 48, R = 6371, d = km / R;
    var la = lat * Math.PI / 180, lo = lon * Math.PI / 180;
    for (var i = 0; i <= n; i++) {
      var b = 2 * Math.PI * i / n;
      var p = Math.asin(Math.sin(la) * Math.cos(d) + Math.cos(la) * Math.sin(d) * Math.cos(b));
      var l = lo + Math.atan2(Math.sin(b) * Math.sin(d) * Math.cos(la), Math.cos(d) - Math.sin(la) * Math.sin(p));
      out.push([l * 180 / Math.PI, p * 180 / Math.PI]);
    }
    return out;
  }

  function validPoint(loc) {
    return loc && isFinite(loc.latitude) && isFinite(loc.longitude) &&
      Math.abs(loc.latitude) <= 90 && Math.abs(loc.longitude) <= 180;
  }

  function addNode(list, id, call, role, loc, seen, extra) {
    if (!validPoint(loc)) return;
    var area = loc.mode === "area";
    var radius = area ? (loc.radius_km || AREA_KM) : 0;
    var props = { id: id, call: call, role: role, area: area ? 1 : 0, seen: seen || "", radius: radius,
      lat: loc.latitude, lon: loc.longitude, detail: extra || "" };
    list.push({ type: "Feature", properties: props, geometry: { type: "Point", coordinates: [loc.longitude, loc.latitude] } });
  }

  function build(data) {
    var nodes = [], areas = [];
    var relays = Array.isArray(data.relays) ? data.relays : [];
    relays.forEach(function (r) {
      addNode(nodes, "r:" + r.callsign, r.callsign, "relay", r.location, r.last_seen, "Relay");
    });
    var stations = Array.isArray(data.stations) ? data.stations : [];
    var seen = {};
    stations.forEach(function (s, i) {
      var key = "s:" + s.reporter_call;
      if (!seen[key]) {
        seen[key] = 1;
        addNode(nodes, key, s.reporter_call, "station", s.reporter_location, s.refreshed_at, "Station");
      }
      var hkey = "h:" + s.heard_call + ":" + i;
      addNode(nodes, hkey, s.heard_call, "heard", s.heard_location, s.heard_at, "Heard");
    });
    nodes.forEach(function (f) {
      if (f.properties.area) {
        areas.push({ type: "Feature", properties: { id: f.properties.id, role: f.properties.role },
          geometry: { type: "Polygon", coordinates: [ring(f.properties.lat, f.properties.lon, f.properties.radius)] } });
      }
    });
    return { nodes: nodes, areas: areas, relays: relays.length, stations: Object.keys(seen).length };
  }

  function syncMarkers() {
    var keep = {};
    features.nodes.forEach(function (f) {
      var p = f.properties, id = p.id;
      keep[id] = true;
      var m = markers[id];
      if (!m) {
        var el = doc.createElement("button");
        el.type = "button";
        el.className = "mk " + p.role;
        el.setAttribute("aria-label", p.call + ", " + p.detail);
        el.addEventListener("click", function () { selected = id; showSelected(); });
        m = new window.maplibregl.Marker({ element: el, anchor: "center" });
        markers[id] = m;
        m.setLngLat(f.geometry.coordinates).addTo(map);
      } else {
        m.setLngLat(f.geometry.coordinates);
      }
    });
    Object.keys(markers).forEach(function (id) {
      if (!keep[id]) { markers[id].remove(); delete markers[id]; }
    });
  }

  function render(data) {
    features = build(data);
    if (loaded) {
      map.getSource("areas").setData({ type: "FeatureCollection", features: features.areas });
      syncMarkers();
    }
    var count = doc.getElementById("count");
    if (count) {
      count.textContent = features.relays + " relay" + (features.relays === 1 ? "" : "s") + ", " +
        features.stations + " station" + (features.stations === 1 ? "" : "s") + " reporting. Updated " + clock() + ".";
    }
    showSelected();
  }

  function showSelected() {
    var set = function (id, text) { var n = doc.getElementById(id); if (n) n.textContent = text; };
    var f = null;
    features.nodes.forEach(function (x) { if (x.properties.id === selected) f = x; });
    if (!f) {
      set("p-call", "None selected");
      set("p-role", "-");
      set("p-seen", "-");
      set("p-loc", "-");
      return;
    }
    var p = f.properties;
    set("p-call", p.call);
    set("p-role", p.detail);
    set("p-seen", p.seen ? new Date(p.seen).toISOString().replace(/\.\d+Z$/, "Z") : "-");
    set("p-loc", p.area ? "Area, about " + p.radius + " km across. The station is somewhere inside." :
      p.lat.toFixed(4) + ", " + p.lon.toFixed(4));
  }

  function poll() {
    if (inflight) return;
    inflight = true;
    window.fetch("/map-data", { cache: "no-store", credentials: "omit", headers: { accept: "application/json" } })
      .then(function (r) { if (!r.ok) throw new Error("HTTP " + r.status); return r.text(); })
      .then(function (text) {
        if (text !== lastBody) { lastBody = text; render(JSON.parse(text)); }
        setStatus(true);
      })
      .catch(function () { setStatus(false); })
      .then(function () { inflight = false; });
  }

  function start() {
    if (!mapEl || !window.maplibregl) {
      setStatus(false);
      if (mapEl) mapEl.textContent = "The map library did not load. Check your connection and reload the page.";
      return;
    }
    if (!window.maplibregl.supported || !window.maplibregl.supported()) {
      setStatus(false);
      mapEl.textContent = "This browser cannot draw the map (WebGL is off). The station list still updates in the panel.";
    }
    // The feed starts now, not when the map has loaded.
    poll();
    window.setInterval(poll, POLL_MS);
    map = new window.maplibregl.Map({
      container: mapEl,
      style: {
        version: 8,
        sources: { osm: { type: "raster", tiles: ["https://tile.openstreetmap.org/{z}/{x}/{y}.png"], tileSize: 256,
          attribution: "© OpenStreetMap contributors" } },
        layers: [{ id: "osm", type: "raster", source: "osm" }]
      },
      center: [10, 25],
      zoom: 1.6,
      attributionControl: { compact: true }
    });
    map.addControl(new window.maplibregl.NavigationControl({ showCompass: false }), "top-right");
    map.on("load", function () {
      loaded = true;
      map.addSource("areas", { type: "geojson", data: EMPTY });

      map.addLayer({ id: "area-fill", type: "fill", source: "areas",
        paint: { "fill-color": "#1F8A65", "fill-opacity": 0.14 } });
      map.addLayer({ id: "area-line", type: "line", source: "areas",
        paint: { "line-color": "#1F8A65", "line-width": 1, "line-dasharray": [3, 2] } });
      if (lastBody) render(JSON.parse(lastBody));
    });
  }

  start();
`;

export const LIVE_MAP_SCRIPT = wrap(COMMON + LIVE);
