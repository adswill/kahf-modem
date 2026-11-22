// The inline scripts for kahf.dev. Plain ES5, no libraries. Each page carries one
// script: the home page gets COMMON + HOME, the map gets COMMON + MAP. The pages
// are complete without them. Each script makes exactly one kind of network call
// (getJson), and builds no HTML strings.
// Written with String.raw so the text that is hashed for the CSP is exactly the
// text that is sent. Do not use backticks or dollar-brace in the parts below.
export const COMMON = String.raw`
  "use strict";
  var doc = document, root = doc.documentElement;
  root.classList.add("js");

  function store(key, value) {
    try {
      if (value === undefined) return window.localStorage.getItem(key);
      window.localStorage.setItem(key, value);
    } catch (e) { /* storage can be blocked */ }
    return null;
  }
  function $(sel, from) { return (from || doc).querySelector(sel); }
  function $$(sel, from) { return Array.prototype.slice.call((from || doc).querySelectorAll(sel)); }
  function pad2(n) { return (n < 10 ? "0" : "") + n; }

  var SVGNS = ["http:", "", "www.w3.org", "2000", "svg"].join("/");
  function el(name, attrs, parent) {
    var n = doc.createElementNS(SVGNS, name);
    for (var k in attrs) if (Object.prototype.hasOwnProperty.call(attrs, k)) n.setAttribute(k, attrs[k]);
    if (parent) parent.appendChild(n);
    return n;
  }
  function getJson(url) {
    return window.fetch(url, { headers: { accept: "application/json" }, cache: "no-store", credentials: "omit" }).then(function (r) {
      if (!r.ok) throw new Error("status " + r.status);
      return r.json();
    });
  }

  /* ---- theme: the system setting, or the remembered manual choice ---- */
  function effectiveTheme() {
    var t = root.getAttribute("data-theme");
    if (t) return t;
    return window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  }
  function labelTheme() {
    $$("[data-act=theme]").forEach(function (b) { b.textContent = effectiveTheme() === "dark" ? "Light theme" : "Dark theme"; });
  }
  $$("[data-act=theme]").forEach(function (b) {
    b.addEventListener("click", function () {
      var next = effectiveTheme() === "dark" ? "light" : "dark";
      root.setAttribute("data-theme", next);
      store("kahf-theme", next);
      labelTheme();
    });
  });
  var saved = store("kahf-theme");
  if (saved === "light" || saved === "dark") root.setAttribute("data-theme", saved);
  labelTheme();
`;

const HOME = String.raw`
  /* ---- downloads: this section exists only when a release has been announced ---- */
  function showRelease(info) {
    if (!info || !info.version) return;
    var safe = false;
    try { safe = new window.URL(String(info.notes_url)).protocol === "https:"; } catch (e) { safe = false; }
    var when = String(info.published_at || "").slice(0, 10);
    var sec = doc.createElement("section"), lic = $("#licences"), dl, p, a;
    sec.className = "part"; sec.id = "download"; sec.setAttribute("aria-labelledby", "download-t");
    var h2 = doc.createElement("h2"); h2.id = "download-t"; h2.textContent = "Version " + info.version; sec.appendChild(h2);
    dl = doc.createElement("dl"); dl.className = "kv";
    [["Version", info.version], ["Date", when]].forEach(function (r) {
      if (!r[1]) return;
      var dt = doc.createElement("dt"), dd = doc.createElement("dd");
      dt.textContent = r[0]; dd.textContent = r[1]; dl.appendChild(dt); dl.appendChild(dd);
    });
    sec.appendChild(dl);
    if (safe) {
      p = doc.createElement("p"); a = doc.createElement("a");
      a.href = info.notes_url; a.rel = "noopener noreferrer"; a.textContent = "Download version " + info.version;
      p.appendChild(a); sec.appendChild(p);
    }
    if (lic && lic.parentNode) lic.parentNode.insertBefore(sec, lic);
  }
  getJson("/release").then(function (d) { if (d && d.release) showRelease(d.release); }).catch(function () { /* keep the honest default */ });

  /* ---- copy buttons on the command blocks ---- */
  $$("[data-copy]").forEach(function (box) {
    var btn = $("[data-cp]", box), code = $("code", box);
    if (!btn || !code) return;
    btn.addEventListener("click", function () {
      var text = code.textContent.replace(/^\$ /gm, "");
      var done = function () { btn.textContent = "Copied"; btn.classList.add("done"); window.setTimeout(function () { btn.textContent = "Copy"; btn.classList.remove("done"); }, 1600); };
      if (window.navigator.clipboard && window.navigator.clipboard.writeText) window.navigator.clipboard.writeText(text).then(done, function () {});
    });
  });

  var reduce = !!(window.matchMedia && window.matchMedia("(prefers-reduced-motion: reduce)").matches);

  /* ---- the lane ruler: read a lane, from its row in the table ---- */
  var ROLE_TEXT = {
    Relay: "Relay lane. Relays send to operators here, and bigger transfers use it.",
    Calling: "Calling lane. Operators ask here and relays announce themselves. Kept short.",
    Spare: "Spare lane. A buffer between busy lanes, and room for a faster transfer when the plan allows it."
  };
  function pickLane(k) {
    var row = $("table.lanes tr[data-k=\"" + k + "\"]");
    if (!row) return;
    var c = row.getElementsByTagName("td");
    $("#r-name").textContent = c[0].textContent;
    $("#r-centre").textContent = c[2].textContent + " MHz";
    $("#r-dial").textContent = c[3].textContent + " MHz";
    $("#r-role").textContent = ROLE_TEXT[c[1].textContent] || "";
    $$("#ruler .lane").forEach(function (g) { g.classList.toggle("sel", g.getAttribute("data-k") === String(k)); });
    $$("table.lanes tr[data-k]").forEach(function (r) { r.classList.toggle("sel", r === row); });
  }
  $$("#ruler .lane").forEach(function (g) {
    var k = g.getAttribute("data-k");
    g.addEventListener("click", function () { pickLane(k); });
    g.addEventListener("keydown", function (e) { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); pickLane(k); } });
  });
  $$("table.lanes tr[data-k]").forEach(function (r) {
    r.addEventListener("click", function () { pickLane(r.getAttribute("data-k")); });
  });
  pickLane(5);
  var ruler = $("#ruler");
  if (ruler) {
    if (reduce || !("IntersectionObserver" in window)) { ruler.classList.remove("wait"); }
    else {
      var io = new window.IntersectionObserver(function (list) {
        if (list[0].isIntersecting) { ruler.classList.remove("wait"); io.disconnect(); }
      }, { threshold: 0.25 });
      io.observe(ruler);
    }
  }

  /* ---- the plotting sheet: night side from the sun, and packets along the passages ---- */
  var LON0 = -30, LON1 = 100, LAT0 = -35, LAT1 = 75, M = 36, PW = 688, PH = 488;
  function px(lon) { return M + (lon - LON0) / (LON1 - LON0) * PW; }
  function py(lat) { return M + (LAT1 - lat) / (LAT1 - LAT0) * PH; }
  function rad(d) { return d * Math.PI / 180; }
  function sunNow(d) {
    var start = Date.UTC(d.getUTCFullYear(), 0, 1), day = (d - start) / 864e5;
    var g = 2 * Math.PI / 365 * (day - 0.5);
    var dec = 0.006918 - 0.399912 * Math.cos(g) + 0.070257 * Math.sin(g) - 0.006758 * Math.cos(2 * g) + 0.000907 * Math.sin(2 * g) - 0.002697 * Math.cos(3 * g) + 0.00148 * Math.sin(3 * g);
    var eq = 229.18 * (0.000075 + 0.001868 * Math.cos(g) - 0.032077 * Math.sin(g) - 0.014615 * Math.cos(2 * g) - 0.040849 * Math.sin(2 * g));
    var utcMin = d.getUTCHours() * 60 + d.getUTCMinutes() + d.getUTCSeconds() / 60;
    return { dec: dec, lon: (720 - utcMin - eq) / 4 };
  }
  function elevation(sun, lat, lon) {
    var h = rad(lon - sun.lon), p = rad(lat);
    var s = Math.sin(p) * Math.sin(sun.dec) + Math.cos(p) * Math.cos(sun.dec) * Math.cos(h);
    return Math.asin(Math.max(-1, Math.min(1, s))) * 180 / Math.PI;
  }
  function drawNight(sun) {
    var g = $("#night");
    if (!g) return;
    while (g.firstChild) g.removeChild(g.firstChild);
    var d = "", lon, t, tanDec = Math.tan(sun.dec);
    if (Math.abs(tanDec) < 0.0005) tanDec = 0.0005;
    for (lon = LON0; lon <= LON1; lon += 1) {
      t = Math.atan(-Math.cos(rad(lon - sun.lon)) / tanDec) * 180 / Math.PI;
      t = Math.max(LAT0 - 5, Math.min(LAT1 + 5, t));
      d += (lon === LON0 ? "M" : "L") + px(lon).toFixed(1) + " " + py(t).toFixed(1);
    }
    /* Which pole is dark: the south one while the sun is north of the equator, and the reverse. */
    var edge = sun.dec < 0 ? M - 4 : M + PH + 4;
    d += "L" + px(LON1).toFixed(1) + " " + edge + "L" + px(LON0).toFixed(1) + " " + edge + "Z";
    el("path", { "class": "night", d: d }, g);
  }
  function midpoint(a, b) {
    var p1 = rad(a[0]), l1 = rad(a[1]), p2 = rad(b[0]), l2 = rad(b[1]);
    var bx = Math.cos(p2) * Math.cos(l2 - l1), by = Math.cos(p2) * Math.sin(l2 - l1);
    var p3 = Math.atan2(Math.sin(p1) + Math.sin(p2), Math.sqrt((Math.cos(p1) + bx) * (Math.cos(p1) + bx) + by * by));
    var l3 = l1 + Math.atan2(by, Math.cos(p1) + bx);
    return [p3 * 180 / Math.PI, ((l3 * 180 / Math.PI) + 540) % 360 - 180];
  }
  function sunLine() {
    var now = new Date(), sun = sunNow(now);
    drawNight(sun);
    var mid = midpoint([54.69, 25.28], [25.2, 55.27]), el0 = elevation(sun, mid[0], mid[1]);
    var cap = $("#sun");
    if (cap) {
      while (cap.firstChild) cap.removeChild(cap.firstChild);
      var a = doc.createElement("span"), b = doc.createElement("span"), bold = doc.createElement("b");
      bold.textContent = "UTC " + pad2(now.getUTCHours()) + ":" + pad2(now.getUTCMinutes()) + ". ";
      a.appendChild(bold);
      a.appendChild(doc.createTextNode("The dark area is where it is night now."));
      b.appendChild(doc.createTextNode("Vilnius to Dubai, sun at the middle of the path: "));
      var n = doc.createElement("span"); n.className = "num"; n.textContent = (el0 >= 0 ? "+" : "") + el0.toFixed(0) + " degrees";
      b.appendChild(n);
      b.appendChild(doc.createTextNode(el0 > 5 ? ". Daylight: 17 m usually works on a path like this." : el0 > -6 ? ". Dawn or dusk: it can go either way." : ". Night: 17 m is usually closed on this path."));
      cap.appendChild(a); cap.appendChild(b);
    }
  }
  sunLine();
  window.setInterval(sunLine, 60000);

  var packets = [["p1", "pk1", 0], ["p2", "pk2", 2800], ["p3", "pk3", 5600]].map(function (r) {
    return { path: $("#" + r[0]), dot: $("#" + r[1]), offset: r[2] };
  }).filter(function (p) { return p.path && p.dot && p.path.getTotalLength; });
  function tick(ms) {
    packets.forEach(function (p) {
      var t = ((ms + 1800 + p.offset) % 9000) / 3600;
      if (t > 1) { p.dot.setAttribute("opacity", "0"); return; }
      var e = t < 0.5 ? 2 * t * t : 1 - Math.pow(-2 * t + 2, 2) / 2;
      var pt = p.path.getPointAtLength(e * p.path.getTotalLength());
      p.dot.setAttribute("cx", pt.x.toFixed(1)); p.dot.setAttribute("cy", pt.y.toFixed(1));
      p.dot.setAttribute("opacity", t < 0.06 ? String(t / 0.06) : t > 0.94 ? String((1 - t) / 0.06) : "1");
    });
    window.requestAnimationFrame(tick);
  }
  if (!reduce && packets.length) window.requestAnimationFrame(tick);
`;

const MAP = String.raw`
  /* ---- map page: select a node, filter, refresh from /map-data ---- */
  var KM = 111.32, W = 1000, HT = 500, selectedKey = "", roleFilter = "all", ageFilter = "all";
  function mapRoot() { return $("#live"); }
  function h(tag, cls, text, parent) {
    var n = doc.createElement(tag);
    if (cls) n.className = cls;
    if (text !== undefined && text !== null) n.textContent = text;
    if (parent) parent.appendChild(n);
    return n;
  }
  function radDeg(km, lat) {
    var c = Math.max(Math.cos(lat * Math.PI / 180), 0.05);
    return { lat: km / KM, lon: km / (KM * c) };
  }
  function ageText(iso, nowIso) {
    var s = Math.max(0, Math.round((Date.parse(nowIso) - Date.parse(iso)) / 1000));
    if (isNaN(s)) return "unknown";
    if (s < 90) return "under 2 minutes ago";
    if (s < 5400) return Math.round(s / 60) + " minutes ago";
    if (s < 172800) return Math.round(s / 3600) + " hours ago";
    return Math.round(s / 86400) + " days ago";
  }
  function ageSec(iso, nowIso) {
    var s = Math.round((Date.parse(nowIso) - Date.parse(iso)) / 1000);
    return isFinite(s) ? Math.max(0, s) : 999999999;
  }
  function short(iso) { return String(iso).replace(/\.\d+Z$/, "Z"); }
  function nodesOf(data) {
    var nodes = [], seen = {};
    (data.stations || []).forEach(function (s, i) {
      if (!seen["relay:" + s.reporter_call]) { seen["relay:" + s.reporter_call] = 1; nodes.push({ id: "r-" + i, role: "relay", call: s.reporter_call, loc: s.reporter_location, last: s.refreshed_at, st: s }); }
      if (!seen["station:" + s.heard_call]) { seen["station:" + s.heard_call] = 1; nodes.push({ id: "h-" + i, role: "station", call: s.heard_call, loc: s.heard_location, last: s.heard_at, st: s }); }
    });
    (data.relays || []).forEach(function (r, i) {
      if (seen["relay:" + r.callsign]) return;
      seen["relay:" + r.callsign] = 1;
      nodes.push({ id: "m-" + i, role: "relay", call: r.callsign, loc: r.location, last: r.last_seen, st: null });
    });
    return nodes;
  }
  function windowOf(nodes) {
    var minLon = 180, maxLon = -180, minLat = 90, maxLat = -90;
    nodes.forEach(function (n) {
      var d = n.loc.mode === "area" ? radDeg(n.loc.radius_km, n.loc.latitude) : { lat: 0, lon: 0 };
      minLon = Math.min(minLon, n.loc.longitude - d.lon); maxLon = Math.max(maxLon, n.loc.longitude + d.lon);
      minLat = Math.min(minLat, n.loc.latitude - d.lat); maxLat = Math.max(maxLat, n.loc.latitude + d.lat);
    });
    var span = Math.min(360, Math.max(8, (maxLon - minLon) * 1.5, (maxLat - minLat) * 3));
    return {
      span: span,
      lon0: Math.max(-180, Math.min(180 - span, (minLon + maxLon) / 2 - span / 2)),
      lat1: Math.max(-90 + span / 2, Math.min(90, (minLat + maxLat) / 2 + span / 4))
    };
  }
  function stepOf(span) {
    var list = [30, 10, 5, 2, 1, 0.5, 0.2, 0.1];
    for (var i = 0; i < list.length; i++) if (span / list[i] >= 4) return list[i];
    return 0.1;
  }
  function drawMap(nodes, stations, parent, now) {
    var w = windowOf(nodes), latSpan = w.span / 2, step = stepOf(w.span), dec = step < 1 ? 1 : 0;
    function X(lon) { return (lon - w.lon0) / w.span * W; }
    function Y(lat) { return (w.lat1 - lat) / latSpan * HT; }
    var svg = el("svg", { viewBox: "0 0 " + W + " " + HT, role: "group", "aria-label": "Map of relays and stations, latitude and longitude in degrees" }, parent);
    var g1 = el("g", {}, svg), g2 = el("g", {}, svg), g3 = el("g", {}, svg), t;
    for (var lo = Math.ceil(w.lon0 / step) * step; lo <= w.lon0 + w.span + 1e-9; lo += step) {
      el("line", { "class": "graticule", x1: X(lo).toFixed(2), y1: 0, x2: X(lo).toFixed(2), y2: HT }, g1);
      t = el("text", { "class": "glabel", x: (X(lo) + 4).toFixed(2), y: HT - 6 }, g1); t.textContent = lo.toFixed(dec);
    }
    for (var la = Math.ceil((w.lat1 - latSpan) / step) * step; la <= w.lat1 + 1e-9; la += step) {
      el("line", { "class": "graticule", x1: 0, y1: Y(la).toFixed(2), x2: W, y2: Y(la).toFixed(2) }, g1);
      t = el("text", { "class": "glabel", x: 4, y: (Y(la) - 4).toFixed(2) }, g1); t.textContent = la.toFixed(dec);
    }
    var by = {};
    nodes.forEach(function (n) { by[n.role + ":" + n.call] = n; });
    stations.forEach(function (s) {
      var a = by["relay:" + s.reporter_call], b = by["station:" + s.heard_call];
      if (a && b && a.loc.mode === "precise" && b.loc.mode === "precise") el("line", { "class": "link", x1: X(a.loc.longitude).toFixed(2), y1: Y(a.loc.latitude).toFixed(2), x2: X(b.loc.longitude).toFixed(2), y2: Y(b.loc.latitude).toFixed(2) }, g2);
    });
    nodes.forEach(function (n) {
      var cx = X(n.loc.longitude), cy = Y(n.loc.latitude);
      var a = el("a", { "class": "marker", href: "#" + n.id, "data-panel": n.id, "data-role": n.role, "data-age": ageSec(n.last, now) }, g3);
      t = el("title", {}, a); t.textContent = n.call + ", " + n.role;
      if (n.loc.mode === "precise") {
        el("rect", { "class": "mk " + n.role, x: (cx - 5).toFixed(2), y: (cy - 5).toFixed(2), width: 10, height: 10 }, a);
        el("rect", { "class": "hit", x: (cx - 14).toFixed(2), y: (cy - 14).toFixed(2), width: 28, height: 28 }, a);
        t = el("text", { "class": "lbl", x: (cx + 12).toFixed(2), y: (cy - 8).toFixed(2) }, a); t.textContent = n.call;
      } else {
        var d = radDeg(n.loc.radius_km, n.loc.latitude);
        el("ellipse", { "class": "area " + n.role, cx: cx.toFixed(2), cy: cy.toFixed(2), rx: (d.lon / w.span * W).toFixed(2), ry: (d.lat / latSpan * HT).toFixed(2) }, a);
        t = el("text", { "class": "lbl", x: (cx + 8).toFixed(2), y: (cy - (d.lat / latSpan * HT) - 6).toFixed(2) }, a); t.textContent = n.call;
      }
    });
    return svg;
  }
  function drawPanel(n, nowIso, parent) {
    var p = h("section", "panel card", null, parent), roleName = n.role === "relay" ? "Relay" : "Station", dl;
    p.id = n.id; p.setAttribute("aria-labelledby", n.id + "-title");
    h("h2", "", n.call, p).id = n.id + "-title";
    h("p", "role", roleName, p);
    dl = h("dl", "kv", null, p);
    function row(k, v, na) { h("dt", "", k, dl); var dd = h("dd", "", na ? "" : v, dl); if (na) h("span", "na", "Not available yet", dd); }
    row("Role", roleName);
    row("Last seen (UTC)", n.last);
    row("Age", ageText(n.last, nowIso));
    row("Position", n.loc.mode === "precise" ? n.loc.latitude.toFixed(4) + ", " + n.loc.longitude.toFixed(4) : "Area, radius " + n.loc.radius_km + " km");
    if (n.st) row("Frequency and mode", n.st.frequency_khz + " kHz, " + n.st.mode);
    row("Hops, last 24 hours", "", true);
    row("Messages sent today", "", true);
    row("Messages sent in total", "", true);
    h("p", "note", n.loc.mode === "precise"
      ? "This position was shared as a point. Anyone who can see the map can see it to about 200 m."
      : "This position was shared as an area. The map shows a circle of radius " + n.loc.radius_km + " km, and the station is somewhere inside it. The circle's centre is not the station's position.", p);
    var close = h("p", "", null, p), a = h("a", "", "Close", close);
    a.href = "#map"; a.setAttribute("data-close", "");
  }
  function option(sel, value, label) { var o = h("option", "", label, sel); o.value = value; }
  function filterBox(parent) {
    var box = h("div", "filters js-only", null, parent), l1 = h("label", "", "Role", box), l2 = h("label", "", "Last seen", box);
    var s1 = h("select", "", null, l1), s2 = h("select", "", null, l2);
    s1.id = "f-role"; s2.id = "f-age";
    option(s1, "all", "All"); option(s1, "relay", "Relays"); option(s1, "station", "Stations");
    option(s2, "all", "Any time"); option(s2, "1", "Last hour"); option(s2, "6", "Last 6 hours"); option(s2, "24", "Last 24 hours");
    s1.value = roleFilter; s2.value = ageFilter;
  }
  function render(data) {
    var live = mapRoot(), nodes = nodesOf(data), now = data.generated_at, relays = 0, i;
    while (live.firstChild) live.removeChild(live.firstChild);
    live.setAttribute("data-now", now);
    nodes.forEach(function (n) { if (n.role === "relay") relays++; });
    if (!nodes.length) {
      var card = h("div", "empty-state", null, live); card.id = "map";
      h("p", "", "No relays or stations have reported in the last 48 hours.", card);
      h("p", "", "A node shows here after its operator turns on location sharing and a relay sends a signed report for it. Station reporting is still being built into the app, so an empty map is normal at this stage. Come back after the first release.", card);
      h("p", "note", "0 reports shown. Generated " + now + ".", card);
      return;
    }
    var stage = h("div", "stage", null, live), left = h("div", "", null, stage), mc = h("div", "mapbox", null, left);
    mc.id = "map"; mc.setAttribute("tabindex", "0"); mc.setAttribute("role", "region"); mc.setAttribute("aria-label", "Map area, scrolls sideways on small screens");
    drawMap(nodes, data.stations || [], mc, now);
    var lg = h("div", "legend", null, left);
    h("span", "", "Filled square: relay", lg); h("span", "", "Open square: station", lg); h("span", "", "Dashed circle: area, the station is somewhere inside", lg);
    var side = h("aside", "side", null, stage), panels = h("div", "sidesec panels", null, side);
    nodes.forEach(function (n) { drawPanel(n, now, panels); });
    var em = h("section", "panel empty", null, panels);
    em.id = "panel-empty"; em.setAttribute("aria-label", "Node details");
    h("h2", "", "No node selected", em);
    h("p", "note", "Choose a square or a circle on the map, or a callsign in the list, to see its role, when it was last seen and what it shared about its position.", em);
    var fs = h("div", "sidesec", null, side);
    filterBox(fs);
    var cnt = h("p", "count", null, fs); cnt.id = "count";
    var ul = h("ul", "list", null, side);
    ul.setAttribute("aria-label", "All nodes");
    nodes.slice().sort(function (a, c) { return a.call < c.call ? -1 : a.call > c.call ? 1 : 0; }).forEach(function (n) {
      var li = h("li", "", null, ul), a = h("a", "", null, li);
      li.setAttribute("data-row", n.id); li.setAttribute("data-role", n.role); li.setAttribute("data-age", ageSec(n.last, now));
      a.href = "#" + n.id; a.setAttribute("data-panel", n.id);
      h("span", "cs", n.call, a);
      h("span", "meta", n.role === "relay" ? "Relay" : "Station", a);
      h("span", "meta", n.loc.mode === "precise" ? "Point" : "Circle, " + n.loc.radius_km + " km", a);
      h("span", "meta", short(n.last), a);
    });
    applyFilter();
    for (i = 0; i < nodes.length; i++) if (nodes[i].role + ":" + nodes[i].call === selectedKey) { showPanel(nodes[i].id); break; }
  }
  function applyFilter() {
    var live = mapRoot(), shown = 0, total = 0, hours = ageFilter === "all" ? 0 : Number(ageFilter);
    $$("[data-role]", live).forEach(function (n) {
      var ok = (roleFilter === "all" || n.getAttribute("data-role") === roleFilter) &&
        (!hours || Number(n.getAttribute("data-age")) <= hours * 3600);
      if (n.tagName.toLowerCase() === "li") { n.hidden = !ok; total++; if (ok) shown++; }
      else n.setAttribute("style", ok ? "" : "display:none");
    });
    var cnt = $("#count", live);
    if (cnt) cnt.textContent = shown + " of " + total + " report" + (total === 1 ? "" : "s") + " shown. Reports older than 48 hours are removed.";
  }
  function showPanel(id) {
    var live = mapRoot();
    if (!live) return;
    var any = false;
    $$(".panel[id]", live).forEach(function (p) {
      var on = p.id === id && id !== "" && p.id !== "panel-empty";
      p.classList.toggle("open", on);
      if (on) any = true;
    });
    if (!any) selectedKey = "";
    var wrap = $(".panels", live);
    if (wrap) wrap.classList.toggle("sel", any);
    $$("a.marker", live).forEach(function (m) { m.classList.toggle("sel", any && m.getAttribute("data-panel") === id); });
    $$("li[data-row]", live).forEach(function (r) { r.classList.toggle("sel", any && r.getAttribute("data-row") === id); });
    try { window.history.replaceState(null, "", any ? "#" + id : window.location.pathname); } catch (e) { /* ignore */ }
    if (any) {
      var p = doc.getElementById(id), title = p && p.querySelector("h2");
      if (title) selectedKey = (p.querySelector(".role").textContent === "Relay" ? "relay:" : "station:") + title.textContent;
    }
  }
  doc.addEventListener("click", function (e) {
    var t = e.target, a = t && t.closest ? t.closest("a[data-panel], a[data-close]") : null;
    if (!a) return;
    e.preventDefault();
    showPanel(a.hasAttribute("data-close") ? "" : a.getAttribute("data-panel"));
  });
  doc.addEventListener("change", function (e) {
    if (e.target.id === "f-role") roleFilter = e.target.value;
    else if (e.target.id === "f-age") ageFilter = e.target.value;
    else return;
    applyFilter();
  });
  function utc(d) { return pad2(d.getUTCHours()) + ":" + pad2(d.getUTCMinutes()) + ":" + pad2(d.getUTCSeconds()) + " UTC"; }
  function refresh() {
    var status = $("#refresh-status");
    if (!mapRoot() || doc.hidden) return;
    getJson("/map-data").then(function (data) {
      render(data);
      if (status) { status.className = "status"; status.lastChild.textContent = "Checked " + utc(new Date()) + ". Checks again every 60 seconds."; }
    }).catch(function () {
      if (status) { status.className = "status warn"; status.lastChild.textContent = "Could not refresh the map at " + utc(new Date()) + ". The data shown is from the last successful load. Trying again in 60 seconds."; }
    });
  }
  if (mapRoot()) {
    var wanted = window.location.hash.replace("#", "");
    if (/^[rhm]-[0-9]+$/.test(wanted) && doc.getElementById(wanted)) showPanel(wanted);
    $$("select").forEach(function (s) { s.value = s.id === "f-role" ? roleFilter : ageFilter; });
    if ($("#count")) applyFilter();
    window.setInterval(refresh, 60000);
  }
`;

export function wrap(parts) {
  return "\n(function () {\n  \"use strict\";\n" + parts + "})();";
}

export const HOME_SCRIPT = wrap(COMMON + HOME);
export const MAP_SCRIPT = wrap(COMMON + MAP);
