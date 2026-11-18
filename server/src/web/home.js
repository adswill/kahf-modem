// Body of the kahf.dev home page. A printed chart of the idea: a plotting sheet with range
// rings and great-circle passages, a ruler of the twelve 17 m lanes, a relay's twelve second
// round, what you can send, and an honest list of what exists and what does not yet.
// Static HTML and SVG built here from numbers that come from the code (lanes.rs, referee.rs).
// Nothing loads from outside. The page script (script.js) adds the day and night line and
// the moving packets; the page is complete without it.

// ---------------------------------------------------------------------------------------
// Chart geometry: an equirectangular plotting sheet, longitude -30 to 100, latitude -35 to 75.
// ---------------------------------------------------------------------------------------
const W = 760, H = 560, M = 36;
const LON0 = -30, LON1 = 100, LAT0 = -35, LAT1 = 75;
const PW = W - 2 * M, PH = H - 2 * M;
const px = (lon) => M + ((lon - LON0) / (LON1 - LON0)) * PW;
const py = (lat) => M + ((LAT1 - lat) / (LAT1 - LAT0)) * PH;
const f1 = (n) => n.toFixed(1);
const rad = (d) => (d * Math.PI) / 180;
const deg = (r) => (r * 180) / Math.PI;
const R_EARTH = 6371;

/** Point `km` from (lat, lon) along `bearing` degrees. */
function destination(lat, lon, bearing, km) {
  const d = km / R_EARTH, b = rad(bearing), p1 = rad(lat), l1 = rad(lon);
  const p2 = Math.asin(Math.sin(p1) * Math.cos(d) + Math.cos(p1) * Math.sin(d) * Math.cos(b));
  const l2 = l1 + Math.atan2(Math.sin(b) * Math.sin(d) * Math.cos(p1), Math.cos(d) - Math.sin(p1) * Math.sin(p2));
  return [deg(p2), ((deg(l2) + 540) % 360) - 180];
}

/** Points along the great circle from a to b. */
function greatCircle(a, b, n) {
  const [p1, l1, p2, l2] = [rad(a[0]), rad(a[1]), rad(b[0]), rad(b[1])];
  const d = 2 * Math.asin(Math.sqrt(Math.sin((p2 - p1) / 2) ** 2 + Math.cos(p1) * Math.cos(p2) * Math.sin((l2 - l1) / 2) ** 2));
  const out = [];
  for (let i = 0; i <= n; i++) {
    const t = i / n, A = Math.sin((1 - t) * d) / Math.sin(d), B = Math.sin(t * d) / Math.sin(d);
    const x = A * Math.cos(p1) * Math.cos(l1) + B * Math.cos(p2) * Math.cos(l2);
    const y = A * Math.cos(p1) * Math.sin(l1) + B * Math.cos(p2) * Math.sin(l2);
    const z = A * Math.sin(p1) + B * Math.sin(p2);
    out.push([deg(Math.atan2(z, Math.hypot(x, y))), deg(Math.atan2(y, x))]);
  }
  return out;
}

const pathOf = (pts) => pts.map((p, i) => `${i ? "L" : "M"}${f1(px(p[1]))} ${f1(py(p[0]))}`).join("");
const ringOf = (c, km) => {
  const pts = [];
  for (let b = 0; b < 360; b += 4) pts.push(destination(c[0], c[1], b, km));
  return pathOf(pts) + "Z";
};

const VILNIUS = [54.69, 25.28];
const PLACES = [
  { name: "Dubai", at: [25.2, 55.27], arc: "p1" },
  { name: "Cairo", at: [30.04, 31.24], arc: "p2" },
  { name: "Reykjavik", at: [64.14, -21.9], arc: "p3" },
  { name: "Lagos", at: [6.52, 3.38], arc: "p4", far: true },
];

/** The plotting sheet. Static; script.js adds the night side and the packets. */
function plottingSheet() {
  let s = "";
  // Neatline: alternating blocks, one per ten degrees, like the border of a printed chart.
  for (let lon = LON0, i = 0; lon < LON1; lon += 10, i++) {
    const x = px(lon), w = (PW * 10) / (LON1 - LON0), cls = i % 2 ? "neat-b" : "neat-a";
    s += `<rect class="${cls}" x="${f1(x)}" y="${M - 9}" width="${f1(w)}" height="7"/><rect class="${cls}" x="${f1(x)}" y="${H - M + 2}" width="${f1(w)}" height="7"/>`;
  }
  for (let lat = LAT0, i = 0; lat < LAT1; lat += 10, i++) {
    const y = py(lat + 10), h = (PH * 10) / (LAT1 - LAT0), cls = i % 2 ? "neat-b" : "neat-a";
    s += `<rect class="${cls}" x="${M - 9}" y="${f1(y)}" width="7" height="${f1(h)}"/><rect class="${cls}" x="${W - M + 2}" y="${f1(y)}" width="7" height="${f1(h)}"/>`;
  }
  // Graticule and its labels, in the margin.
  let grat = "", labels = "";
  for (let lon = -20; lon <= 100; lon += 10) {
    grat += `<line class="grat" x1="${f1(px(lon))}" y1="${M}" x2="${f1(px(lon))}" y2="${H - M}"/>`;
    labels += `<text class="dim" x="${f1(px(lon))}" y="${H - 10}" font-size="11" text-anchor="middle">${Math.abs(lon)}${lon < 0 ? "W" : lon > 0 ? "E" : ""}</text>`;
  }
  for (let lat = -30; lat <= 70; lat += 10) {
    grat += `<line class="grat" x1="${M}" y1="${f1(py(lat))}" x2="${W - M}" y2="${f1(py(lat))}"/>`;
    labels += `<text class="dim" x="${M - 14}" y="${f1(py(lat) + 4)}" font-size="11" text-anchor="end">${Math.abs(lat)}${lat < 0 ? "S" : lat > 0 ? "N" : ""}</text>`;
  }
  // Range rings round the relay: 17 m often skips the first thousand kilometres and works
  // to about four thousand. These are rules of thumb, drawn as such.
  const band = `${ringOf(VILNIUS, 4000)}${ringOf(VILNIUS, 1000)}`;
  const rings = `<path class="band" fill-rule="evenodd" d="${band}"/><path class="skip-ring" d="${ringOf(VILNIUS, 1000)}"/><path class="hop-ring" d="${ringOf(VILNIUS, 4000)}"/>`;
  // Passages: great circles from the relay.
  let arcs = "", dots = "";
  PLACES.forEach((p, i) => {
    const pts = greatCircle(VILNIUS, p.at, 48);
    const d = `d${i + 1}`;
    arcs += `<path id="${p.arc}" class="arc${p.far ? " far" : " draw " + d}" pathLength="1" d="${pathOf(pts)}"/>`;
    dots += `<circle class="node" cx="${f1(px(p.at[1]))}" cy="${f1(py(p.at[0]))}" r="5"/>` +
      `<text class="it" x="${f1(px(p.at[1]) + 9)}" y="${f1(py(p.at[0]) - 7)}" font-size="14">${p.name}</text>`;
  });
  const rx = px(VILNIUS[1]), ry = py(VILNIUS[0]);
  const relay = `<circle class="relay" cx="${f1(rx)}" cy="${f1(ry)}" r="7.5"/><text class="it" x="${f1(rx + 11)}" y="${f1(ry - 9)}" font-size="15" font-weight="700">Vilnius</text>`;
  // Compass rose.
  const cx = W - M - 62, cy = H - M - 64, r = 38;
  let rose = `<g class="rose"><circle cx="${cx}" cy="${cy}" r="${r}" fill="none"/><circle cx="${cx}" cy="${cy}" r="${r - 7}" fill="none"/>`;
  for (let a = 0; a < 360; a += 22.5) {
    const len = a % 90 === 0 ? r + 9 : a % 45 === 0 ? r + 3 : r - 7;
    const x2 = cx + Math.sin(rad(a)) * len, y2 = cy - Math.cos(rad(a)) * len;
    rose += `<line x1="${cx}" y1="${cy}" x2="${f1(x2)}" y2="${f1(y2)}"/>`;
  }
  rose += `<path class="fill" d="M${cx} ${cy - r - 11} L${cx + 5} ${cy - 4} L${cx - 5} ${cy - 4} Z"/><text x="${cx}" y="${cy - r - 15}" font-size="12" font-weight="700" text-anchor="middle">N</text></g>`;
  // The title block.
  const cart = `<rect class="cart" x="${M + 12}" y="${M + 12}" width="226" height="64"/>` +
    `<text x="${M + 24}" y="${M + 34}" font-size="13" font-weight="700" letter-spacing="1.4">17 M PLOTTING SHEET</text>` +
    `<text class="dim" x="${M + 24}" y="${M + 52}" font-size="12">Example passages, not coverage.</text>` +
    `<text class="dim" x="${M + 24}" y="${M + 67}" font-size="12">Rings are rules of thumb.</text>`;
  return `<svg viewBox="0 0 ${W} ${H}" role="img" aria-labelledby="ch-t ch-d" id="sheet">
<title id="ch-t">A 17 metre plotting sheet</title>
<desc id="ch-d">A chart of Europe, Africa and western Asia with a graticule. A relay at Vilnius is ringed at 1000 and 4000 kilometres: 17 metres often skips the first thousand kilometres and works to about four thousand. Great-circle passages run from Vilnius to Dubai, Cairo and Reykjavik inside the working ring, and to Lagos beyond it. Example only.</desc>
${s}
<svg x="${M}" y="${M}" width="${PW}" height="${PH}" viewBox="${M} ${M} ${PW} ${PH}" id="plot">
<rect x="${M}" y="${M}" width="${PW}" height="${PH}" fill="none"/>
${grat}
<g id="night"></g>
${rings}${arcs}${dots}${relay}
<circle class="pkt" id="pk1" r="4.5" cx="-20" cy="-20"/><circle class="pkt" id="pk2" r="4.5" cx="-20" cy="-20"/><circle class="pkt" id="pk3" r="4.5" cx="-20" cy="-20"/>
</svg>
${labels}${rose}${cart}
</svg>`;
}

// ---------------------------------------------------------------------------------------
// The twelve lanes: 700 Hz apart, 480 Hz wide, from 18 111.2 kHz (MESH-SPEC 2.1 and 2.2).
// ---------------------------------------------------------------------------------------
const LANE_ROLES = ["relay", "relay", "relay", "relay", "spare", "calling", "calling", "spare", "relay", "relay", "relay", "relay"];
const LANE_NAMES = ["R1", "R2", "R3", "R4", "X1", "C1", "C2", "X2", "R5", "R6", "R7", "R8"];
const ROLE_WORDS = {
  relay: "Relay lane: relays send to operators here, and bigger transfers use it.",
  calling: "Calling lane: operators ask here, and relays announce themselves. Kept short.",
  spare: "Spare lane: a buffer between busy lanes, and room for a faster transfer when the plan allows it.",
};
const laneLower = (k) => 18111200 + 700 * k;
const mhz = (hz) => (hz / 1e6).toFixed(5);

const RX0 = 18109000, RX1 = 18120200, RW = 1000, RPAD = 30;
const rx = (hz) => RPAD + ((hz - RX0) / (RX1 - RX0)) * (RW - 2 * RPAD);

function laneRuler() {
  let s = "";
  // The beacon segment next to the lanes: hatched, kept clear.
  const bx0 = rx(18109000), bx1 = rx(18111000);
  s += `<rect x="${f1(bx0)}" y="64" width="${f1(bx1 - bx0)}" height="86" fill="none" stroke="var(--ink)" stroke-width="1.2"/>`;
  for (let x = bx0 + 6; x < bx1; x += 7) s += `<line class="hatch" x1="${f1(x)}" y1="150" x2="${f1(Math.min(x + 22, bx1))}" y2="${f1(150 - Math.min(22, bx1 - x) * 1.0)}"/>`;
  s += `<text x="${f1((bx0 + bx1) / 2)}" y="48" font-size="12" text-anchor="middle" class="dim">Beacons only. KAHF</text><text x="${f1((bx0 + bx1) / 2)}" y="62" font-size="12" text-anchor="middle" class="dim">never goes here.</text>`;
  // Axis and ticks, every 1 kHz, labelled in MHz.
  s += `<line class="axis" x1="${RPAD}" y1="150" x2="${RW - RPAD}" y2="150"/>`;
  for (let hz = 18109000; hz <= 18120000; hz += 1000) {
    const x = rx(hz);
    s += `<line class="tick" x1="${f1(x)}" y1="150" x2="${f1(x)}" y2="${hz % 5000 === 0 || hz === 18111000 || hz === 18120000 ? 164 : 158}"/>`;
    if (hz % 1000 === 0) s += `<text class="dim" x="${f1(x)}" y="182" font-size="11.5" text-anchor="middle">${(hz / 1e6).toFixed(3)}</text>`;
  }
  s += `<text class="dim" x="${RPAD}" y="204" font-size="12">MHz, transmitted frequency. Upper edge of the band plan block: 18.120.</text>`;
  // Lanes.
  LANE_ROLES.forEach((role, k) => {
    const x0 = rx(laneLower(k)), w = rx(laneLower(k) + 480) - x0, delay = (0.05 * k).toFixed(2);
    s += `<g class="lane ${role}" tabindex="0" role="button" data-k="${k}" aria-label="Lane ${LANE_NAMES[k]}, ${role}, ${mhz(laneLower(k) + 240)} megahertz">` +
      `<g class="rise" style="animation-delay:${delay}s"><rect class="bar" x="${f1(x0)}" y="64" width="${f1(w)}" height="86"/>` +
      `<text class="nm" x="${f1(x0 + w / 2)}" y="112" ${role === "spare" ? "" : ""}>${LANE_NAMES[k]}</text></g>` +
      `<rect class="hit" x="${f1(x0 - 4)}" y="40" width="${f1(w + 8)}" height="120"/></g>`;
  });
  return `<svg viewBox="0 0 ${RW} 214" role="group" aria-label="The twelve lanes between 18.111 and 18.120 megahertz">${s}</svg>`;
}

function laneTable() {
  const rows = LANE_ROLES.map((role, k) => {
    const centre = laneLower(k) + 240;
    return `<tr data-k="${k}"><td>${LANE_NAMES[k]}</td><td>${role[0].toUpperCase() + role.slice(1)}</td><td>${mhz(centre)}</td><td>${mhz(centre - 1500)}</td></tr>`;
  }).join("");
  return `<table class="lanes"><caption>All twelve lanes. The USB dial is where the radio is set so that the lane lands at 1500 Hz of audio.</caption>` +
    `<thead><tr><th scope="col">Lane</th><th scope="col">Role</th><th scope="col">Centre, MHz</th><th scope="col">USB dial, MHz</th></tr></thead><tbody>${rows}</tbody></table>`;
}

function roundOfSlots() {
  const slots = [
    ["b", "Beacon", "The relay says who it is, how busy it is, and answers last round's messages."],
    ["", "Ask", "Operators ask, one at a time, in a slot they pick at random."],
    ["", "Ask", "A second chance to ask. Two asking at once are both lost; each tries later."],
    ["g", "Grant", "The relay names who may send, and when. Everyone who hears it keeps quiet."],
    ["d", "Send", "Only the granted operator sends."],
    ["d", "Send", "Or the relay sends its own messages here."],
  ];
  return `<div class="round" id="round"><div class="head"><span>A relay's round, 12 seconds</span><span>Six slots of 2 seconds</span></div><div class="track">` +
    slots.map((s) => `<div class="slot ${s[0]}"><b>${s[1]}</b><span>${s[2]}</span></div>`).join("") +
    `<div class="play" aria-hidden="true"></div></div></div>`;
}

export function homeBody() {
  return `
<div class="sheet">
<section class="plate" aria-labelledby="top-title">
<div>
<h1 id="top-title">Messages by radio, <em>with no internet.</em></h1>
<p class="lede">KAHF is a desktop app that sends chat, mail, files and voice notes between computers over HF radio, on the 17 metre band, through a sound card or an SDR.</p>
<p class="state"><b>Early build.</b> It has not yet been tried on a real radio channel. What exists, and what does not, is listed below.</p>
<div class="go"><a class="main" href="#lanes">See how it shares the band</a><a class="alt" href="/map">Live map</a><a class="alt" href="https://github.com/adswill/kahf-modem" rel="noopener noreferrer">Source on GitHub</a></div>
</div>
<figure class="chart">${plottingSheet()}
<figcaption class="chart-cap" id="sun"><span><b>Dark side:</b> where it is night now. Needs JavaScript.</span><span>Dots are example paths, not stations.</span></figcaption>
</figure>
</section>

<section class="part" id="lanes" aria-labelledby="lanes-t">
<h2 id="lanes-t">One band, shared in twelve narrow lanes.</h2>
<div class="cols lean">
<div>
<p>Most amateur data modes take the whole 2.4 kHz of a channel. KAHF cuts a 9 kHz block of 17 metres into twelve lanes, each 480 Hz wide and 700 Hz apart, so many conversations fit and none sit on top of another.</p>
<p>Two lanes are for calling, eight are for relays, and two are spare. A station first listens, and does not send if the lane or either of its neighbours is busy. It yields to anyone, KAHF or not.</p>
</div>
<div>
<p>Each lane is the ordinary KAMI modem slowed five times so that it fits 480 Hz. That is slow, and it is the price of sharing: a short chat takes seconds, a long mail takes minutes. The speed table is in the project notes. No speed has been measured on a real radio.</p>
</div>
</div>
<div class="ruler wait" id="ruler">${laneRuler()}</div>
<dl class="read" id="read" aria-live="polite">
<div><dt>Lane</dt><dd id="r-name">C1</dd></div>
<div><dt>Centre</dt><dd id="r-centre">-</dd></div>
<div><dt>USB dial</dt><dd id="r-dial">-</dd></div>
<div><dt>Width</dt><dd id="r-width">480 Hz</dd></div>
<div style="grid-column: 1 / -1"><dt>Role</dt><dd class="sm" id="r-role">Click or tab to a lane to read it. The table below has all twelve.</dd></div>
</dl>
${laneTable()}

<h3>A relay keeps order on the calling lane</h3>
<p>Operators cannot hear each other, but all hear the relay. So the relay runs a round every 12 seconds and nobody sends data except in the slot they were granted. This part is built and tested in simulation with a handful of stations.</p>
${roundOfSlots()}
</section>

<section class="part" id="messages" aria-labelledby="messages-t">
<h2 id="messages-t">What you can send.</h2>
<p>Every message is signed by its sender, so the receiver can check who wrote it. Nothing is encrypted. Nothing is sent until the operator says so, and every message shows where it is: waiting, sending, delivered, or failed with the reason.</p>
<table class="log">
<tbody>
<tr><th scope="row">Chat</th><td class="m">up to 400 bytes</td><td>Goes out as numbered, error-protected frames. The receiver answers once for the whole burst, and the sender repeats only what was missed. Each message arrives once and in order.</td></tr>
<tr><th scope="row">Mail</th><td class="m">subject, text, attachments</td><td>Signed mail with attachments, sent over a direct link between two stations.</td></tr>
<tr><th scope="row">Files</th><td class="m">up to 100 kB</td><td>Direct only. The app shows how long it will take before you send.</td></tr>
<tr><th scope="row">Voice notes</th><td class="m">short clips</td><td>Recorded in the app, squeezed by the Codec2 voice codec, sent like a file and played at the other end.</td></tr>
</tbody>
</table>
<h3>Past the range of one radio</h3>
<p>A licensed relay station can pass a message on to a station you cannot hear. The design keeps no messages in the network: the sender's computer keeps its outbox and tries again until the message is delivered or its time is up. This relay network is being built; the pieces are written and tested apart, and are not yet joined to the live server.</p>
</section>

<section class="part" id="reach" aria-labelledby="reach-t">
<h2 id="reach-t">17 metres is a daytime, long-distance band.</h2>
<div class="cols">
<div>
<p>Over the first several hundred to about a thousand kilometres, 17 metre signals often pass overhead and are not heard (the skip zone). Paths of roughly 1000 to 4000 kilometres work well by day. At night the band usually closes.</p>
<p>So a relay is not for the next town. It is for the next country. When the band is closed on a path, the app says so, keeps your message, and listens for it to open.</p>
</div>
<div>
<p>These are rules of thumb, drawn as rings on the sheet above, not a forecast. The network is meant to measure its own paths and replace the guesses with what it hears. Other bands are not part of this version.</p>
<p class="note" id="sun2" style="color: var(--ink-2); font-size: 15px">Sheet above: the dark area is night at this moment, from the sun's position. The relay at Vilnius is an example.</p>
</div>
</div>
</section>

<section class="part" id="status" aria-labelledby="status-t">
<h2 id="status-t">What exists today, and what does not.</h2>
<p class="keyline"><span><i class="done"></i>Built and tested on a bench or in simulation</span><span><i class="gap"></i>Not done</span></p>
<div class="cols">
<div>
<h3>Working in tests</h3>
<ul class="ledger">
<li><span class="k done"></span><div><b>Chat between two stations on each of the twelve lanes.</b><br><span>On a simulated channel with background noise.</span></div></li>
<li><span class="k done"></span><div><b>Lane signal stays inside 480 Hz.</b><br><span>Measured on the modem's output: under -26 dB outside the lane.</span></div></li>
<li><span class="k done"></span><div><b>Listening before sending, lane by lane.</b><br><span>A voice-like or noise signal on the lane, or beside it, holds the transmit.</span></div></li>
<li><span class="k done"></span><div><b>Two calling lanes heard at once from one sound card.</b></div></li>
<li><span class="k done"></span><div><b>One relay keeping order for several operators who cannot hear each other.</b><br><span>Requests, grants and data slots, in simulation with a few stations.</span></div></li>
<li><span class="k done"></span><div><b>Mail, files and voice notes over a direct link.</b></div></li>
<li><span class="k done"></span><div><b>The live map.</b><br><span>Running now at <a href="/map">kahf.dev/map</a>. It shows only what has been reported to it.</span></div></li>
</ul>
</div>
<div>
<h3>Not done</h3>
<ul class="ledger">
<li><span class="k gap"></span><div><b>Any transmission on a real radio.</b><br><span>The app refuses to transmit until a person has checked the 17 metre table against the current band plan. That check has not been made.</span></div></li>
<li><span class="k gap"></span><div><b>Joining the relays through the server.</b><br><span>Routing, paging and planning exist as code and tests, not as a live service.</span></div></li>
<li><span class="k gap"></span><div><b>Speed on real HF.</b><br><span>Fading in a burst five times longer than usual is untested.</span></div></li>
<li><span class="k gap"></span><div><b>Wider signals for strong links</b> using several lanes at once.</div></li>
<li><span class="k gap"></span><div><b>An installer.</b><br><span>There is no download yet. When there is one, it will appear on this page.</span></div></li>
</ul>
</div>
</div>
</section>

<section class="part" id="licences" aria-labelledby="lic-t">
<h2 id="lic-t">Licence and responsibility. Read this before you transmit.</h2>
<div class="fine">
<div>
<h3>Licence</h3>
<p>Source-available. Copyright (c) 2026 adswill, all rights reserved. Nothing here grants a licence to copy or redistribute it.</p>
<h3>Your responsibility</h3>
<p>KAHF is for licensed amateur radio operators. You are responsible for staying inside your licence: bands, modes, power and rules on third-party traffic differ from country to country. KAHF asks you to confirm each transmission. It does not decide for you.</p>
</div>
<div>
<h3>Content</h3>
<p>No religious or political content. The project does not carry or promote either, and operators remain responsible for what they send.</p>
<h3>Map privacy</h3>
<p>Location sharing on the map is off by default. If you turn it on, you share a point accurate to about 200 m, or a circle of 1 to 100 km with your real position somewhere inside.</p>
</div>
</div>
</section>
</div>`;
}

/** The masthead and footer of the home page. */
export const HOME_HEAD = `<header class="mast">
<div class="in">
<a class="brand" href="/">KAHF</a>
<p class="sub">Radio messages on 17 metres</p>
<nav aria-label="Main">
<a href="#lanes">Lanes</a>
<a href="#messages">Messages</a>
<a href="#reach">Range</a>
<a href="#status">Status</a>
<a href="/map">Live map</a>
</nav>
<div class="tools"><button type="button" class="btn js-only" data-act="theme">Dark theme</button></div>
</div>
</header>`;

export const HOME_FOOT = `<footer class="base">
<div class="in">
<p>KAHF is pre-release software for licensed amateur radio operators. Copyright (c) 2026 adswill.</p>
<p class="sp"><a href="https://github.com/adswill/kahf-modem" rel="noopener noreferrer">Source on GitHub</a></p>
<p><a href="https://ko-fi.com/adswill" rel="noopener noreferrer">Support KAHF on Ko-fi</a></p>
</div>
</footer>`;
