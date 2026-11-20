// Body of the kahf.dev home page. Static HTML from numbers that come from the modem
// source (phy.rs, ldpc.rs, link.rs) and the 17 m allocation in bandplan.rs. Nothing
// here loads anything from outside. The one picture is served by this Worker at /operate.png.

/** Width and height of the screenshot file, in pixels. It is shown at no more than this size. */
export const SHOT_W = 1512;
export const SHOT_H = 940;

/** The 17 m amateur allocation, the same in all three ITU regions, in kHz. */
const BAND_LO = 18068;
const BAND_HI = 18168;

/** A small seeded generator, so the picture is the same on every request. */
function rng(seed) {
  let s = seed;
  return () => {
    s = (s * 1664525 + 1013904223) % 4294967296;
    return s / 4294967296;
  };
}

/**
 * The 17 m band as an instrument: a frequency scale across the whole allocation
 * and a waterfall of a few KAHF-width signals. It is an illustration, and says so.
 */
export function bandStrip() {
  const W = 1000, scaleY = 176, top = 12, rows = 54, rowH = 3;
  const x = (khz) => ((khz - BAND_LO) / (BAND_HI - BAND_LO)) * W;
  const rand = rng(1817);
  // Centre kHz, first row it is on air, last row, strength 0 to 1.
  const bursts = [[18091.4, 4, 22, 0.9], [18103.2, 14, 40, 0.7], [18127.8, 0, 12, 0.5], [18139.1, 26, 53, 0.85], [18112.6, 30, 44, 0.6]];
  let wf = "";
  for (let r = 0; r < rows; r++) {
    for (const [c, a, b, p] of bursts) {
      if (r < a || r > b) continue;
      const w = x(BAND_LO + 2.4);
      const o = (0.25 + 0.6 * rand()) * p;
      wf += `<rect class="wf" x="${(x(c) - w / 2).toFixed(1)}" y="${(top + r * rowH).toFixed(1)}" width="${w.toFixed(1)}" height="${rowH}" opacity="${o.toFixed(2)}"/>`;
    }
  }
  let ticks = "";
  for (let k = 18070; k <= 18160; k += 10) {
    ticks += `<line class="tick big" x1="${x(k).toFixed(1)}" y1="${scaleY}" x2="${x(k).toFixed(1)}" y2="${scaleY + 10}"/>` +
      `<text x="${x(k).toFixed(1)}" y="${scaleY + 26}" text-anchor="middle">${(k / 1000).toFixed(2)}</text>`;
  }
  for (let k = BAND_LO + 2; k < BAND_HI; k += 2) {
    if (k % 10 === 0) continue;
    ticks += `<line class="tick" x1="${x(k).toFixed(1)}" y1="${scaleY}" x2="${x(k).toFixed(1)}" y2="${scaleY + 5}"/>`;
  }
  return `<figure class="band">
<svg viewBox="0 0 ${W} 216" role="img" aria-labelledby="bs-t bs-d">
<title id="bs-t">The 17 metre band, 18.068 to 18.168 megahertz</title>
<desc id="bs-d">A frequency scale across the 100 kilohertz of the 17 metre amateur band with a waterfall of five signals, each about 2.4 kilohertz wide. An illustration, not a recording.</desc>
${wf}
<line class="edge" x1="0" y1="${scaleY}" x2="${W}" y2="${scaleY}"/>
${ticks}
</svg>
<figcaption>17 m, 18.068 to 18.168 MHz. Each trace is 2.4 kHz wide, the width of one KAHF signal. An illustration, not a recording.</figcaption>
</figure>`;
}

/**
 * A relay passing a message between two stations that cannot hear each other.
 */
function relayDiagram() {
  const node = (cx, label, sub, hot) =>
    `<rect class="${hot ? "hotfill" : "box"}" x="${cx - 70}" y="60" width="140" height="64"/>` +
    `<text class="${hot ? "hottxt" : ""}" x="${cx}" y="88" font-size="18" font-weight="700" text-anchor="middle">${label}</text>` +
    `<text class="${hot ? "hottxt" : "dim"}" x="${cx}" y="110" font-size="13" text-anchor="middle">${sub}</text>`;
  return `<svg viewBox="0 0 760 190" role="img" aria-labelledby="rl-t rl-d">
<title id="rl-t">A relay between two stations</title>
<desc id="rl-d">Station A and station B are too far apart to hear each other, marked by a crossed-out direct path. A relay between them hears both and passes the message on.</desc>
<path class="cut" d="M110 40 H650"/>
<path class="cut" d="M370 30 L390 50 M390 30 L370 50"/>
<text class="dim" x="380" y="22" font-size="13" text-anchor="middle">out of range</text>
${node(110, "A", "sender", false)}${node(380, "RELAY", "hears both", true)}${node(650, "B", "receiver", false)}
<path class="air" d="M180 92 H310"/><path class="air" d="M450 92 H580"/>
<text class="dim" x="245" y="148" font-size="13" text-anchor="middle">17 m, 1 hop</text>
<text class="dim" x="515" y="148" font-size="13" text-anchor="middle">17 m, 1 hop</text>
</svg>`;
}

/**
 * How a message travels, as one plain diagram: five steps, then the audio
 * spectrum of the signal with its 153 carriers.
 */
function travelDiagram() {
  const steps = [
    ["1", "Message", "text, mail or a file, signed by the sender"],
    ["2", "Frames", "up to 36 bytes each, numbered"],
    ["3", "LDPC code", "324 bits in, 648 bits out"],
    ["4", "OFDM audio", "153 carriers, 15.625 Hz apart"],
    ["5", "Radio", "sound card to a transmitter, or an SDR"],
  ];
  let boxes = "";
  steps.forEach((s, i) => {
    const y = 8 + i * 74;
    boxes += `<rect class="box" x="8" y="${y}" width="544" height="52"/>` +
      `<text x="22" y="${y + 23}" font-size="18" font-weight="700">${s[0]}  ${s[1]}</text>` +
      `<text class="dim" x="22" y="${y + 43}" font-size="14">${s[2]}</text>`;
    if (i < steps.length - 1) {
      boxes += `<path class="arrow" d="M280 ${y + 52} V${y + 74} M275 ${y + 68} L280 ${y + 74} L285 ${y + 68}"/>`;
    }
  });
  // Spectrum strip: 0 to 3000 Hz across 544 px. Carriers 20 to 172, every 4th a pilot.
  const X0 = 8, W = 544, top = 430, base = 490;
  const x = (hz) => (X0 + (hz / 3000) * W).toFixed(2);
  let carriers = "";
  for (let bin = 20; bin <= 172; bin++) {
    const pilot = (bin - 20) % 4 === 0;
    carriers += `<line class="${pilot ? "pilot" : "carr"}" x1="${x(bin * 15.625)}" y1="${base}" x2="${x(bin * 15.625)}" y2="${pilot ? top : top + 14}"/>`;
  }
  return `<svg viewBox="0 0 560 560" role="img" aria-labelledby="dg-t dg-d">
<title id="dg-t">How a message travels</title>
<desc id="dg-d">Five steps from a message to a radio signal: message, frames of up to 36 bytes, an LDPC code that turns 324 bits into 648, 153 OFDM carriers 15.625 hertz apart, and the radio. Below, the audio spectrum from 312 hertz to 2.69 kilohertz, with every fourth carrier drawn taller as a pilot.</desc>
${boxes}
<line class="rule" x1="${x(0)}" y1="${base}" x2="${x(3000)}" y2="${base}"/>
${carriers}
<line class="rule" x1="${x(312.5)}" y1="${base + 6}" x2="${x(312.5)}" y2="${base + 12}"/>
<line class="rule" x1="${x(2687.5)}" y1="${base + 6}" x2="${x(2687.5)}" y2="${base + 12}"/>
<text x="${x(312.5)}" y="${base + 30}" font-size="14">312 Hz</text>
<text x="${x(2687.5)}" y="${base + 30}" font-size="14" text-anchor="end">2.69 kHz</text>
<text class="dim" x="${x(1500)}" y="${base + 30}" font-size="14" text-anchor="middle">audio</text>
<text class="dim" x="${X0}" y="${top - 8}" font-size="14">taller lines are pilots</text>
</svg>`;
}

export function homeBody() {
  return `
<div class="wrap">
<div class="hero">
<div>
<h1 id="top-title">Messages, mail and files over <em>HF</em>, relayed when the path is not there.</h1>
<p class="lede">KAHF is an amateur radio modem with a relay network, built for 17 metres. It runs from a sound card and an ordinary radio, or from an SDR.</p>
<div class="cta"><a class="primary" href="/start">Get started</a><a href="/map">Open the live map</a><a href="/network">How the network works</a></div>
</div>
<div class="dial" aria-hidden="true"><span>18</span>.100<small>MHz &middot; 17 m</small></div>
</div>
${bandStrip()}
</div>

<div class="wrap">
<section class="sec" id="why17" aria-labelledby="why17-t">
<h2 id="why17-t">A quiet band that is open to the whole world.</h2>
<dl class="ledger">
<div><dt>Allocation</dt><dd class="v">18.068 to 18.168 MHz</dd><dd class="n">The same in all three ITU regions, so one frequency plan works for every operator.</dd></div>
<div><dt>Width</dt><dd class="v">100 kHz</dd><dd class="n">One KAHF signal is 2.4 kHz wide. The band holds dozens side by side, with space between.</dd></div>
<div><dt>Paths</dt><dd class="v">daytime, long</dd><dd class="n">17 m carries over intercontinental distances when the sun is up, and it is less crowded than 20 m.</dd></div>
</dl>
<p class="note">KAHF is not limited to 17 m. It works on any HF or VHF channel your licence allows, and warns when a frequency is outside the amateur segments.</p>
</section>

<section class="sec" id="what" aria-labelledby="what-t">
<h2 id="what-t">A message goes in at one radio and comes out at another.</h2>
<div class="grid even">
<div>
<p>Every message is signed by its sender, so the receiver can check who sent it. Nothing is encrypted, and nothing is sent until the operator confirms.</p>
<p>A station that hears two others can pass messages between them, so a message can reach a station that is out of direct range.</p>
</div>
<div>
<p>Version 1.0 includes the modem, messages, mail and file transfer, a contact log with ADIF export, receive from HackRF, RTL-SDR and other radios, transmit through HackRF or a rig, and the live map.</p>
<p>Not in 1.0 yet: reporting your station to the map from the app. Until that ships, the map shows only what has been reported to it.</p>
</div>
</div>
<figure class="shot">
<img src="/operate.png" width="${SHOT_W}" height="${SHOT_H}" alt="The Operate screen of the KAHF desktop app">
<figcaption>The Operate screen, from a development build. Shown at its real size, ${SHOT_W} by ${SHOT_H} pixels.</figcaption>
</figure>
</section>

<section class="sec" id="relays" aria-labelledby="relays-t">
<h2 id="relays-t">Stations that cannot hear each other can still talk.</h2>
<div class="grid two">
<div>
<p>A relay is an ordinary KAHF station that agrees to keep its radio on and pass traffic along. It works to a schedule, so a relay with one radio takes turns between receiving and sending.</p>
<p>Relays register with the network under their callsign. The live map shows which ones are up.</p>
<p><a href="/network">Read how relays work</a></p>
</div>
<figure class="diagram wide">${relayDiagram()}</figure>
</div>
</section>

<section class="sec" id="how" aria-labelledby="how-t">
<h2 id="how-t">Five steps, 153 carriers, 2.4 kHz of audio.</h2>
<div class="grid two">
<figure class="diagram">${travelDiagram()}</figure>
<div>
<p>The message is cut into frames. Each frame is protected by an error-correcting code and sent as audio. The receiver checks every frame, asks again for the ones that failed, and puts the message back together.</p>
<dl class="kv">
<dt>Audio rate</dt><dd>8000 Hz</dd>
<dt>Carriers</dt><dd>153, spaced 15.625 Hz</dd>
<dt>Band</dt><dd>312 Hz to 2.69 kHz</dd>
<dt>Pilots</dt><dd>every 4th carrier</dd>
<dt>Modulation</dt><dd>QPSK, 16QAM, 64QAM</dd>
<dt>Code</dt><dd>LDPC, 648 bits carry 324</dd>
<dt>Frame</dt><dd>up to 36 bytes</dd>
</dl>
<p><a href="/protocol">Full modem details</a></p>
</div>
</div>
</section>

<section class="sec" id="licences" aria-labelledby="lic-t">
<h2 id="lic-t">Read this before you transmit.</h2>
<div class="grid even">
<div>
<h3>Licence</h3>
<p>Free software under the GNU General Public License, version 3 or later. Copyright (c) 2026 Kristijonas Adomaitis (adswill). You can use, study, change and share it. Changed versions, and programs built on it, must be shared under the same licence with their source.</p>
<h3>Your responsibility</h3>
<p>KAHF is for licensed amateur radio operators. You are responsible for staying inside your licence: bands, modes, power and rules on third-party traffic differ from country to country. KAHF asks you to confirm each transmission. It does not decide for you.</p>
</div>
<div>
<h3>Content</h3>
<p>No religious or political content. The project does not carry or promote either, and operators remain responsible for what they send.</p>
<h3>Map privacy</h3>
<p>Location sharing on the map is off by default. If you turn it on, you share a point accurate to about 200 m, or a circle of 1 to 100 km with your real position somewhere inside.</p>
<p><a href="/legal">Licence, privacy and terms</a></p>
</div>
</div>
</section>
</div>`;
}
