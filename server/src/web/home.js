// Body of the kahf.dev home page: four sections. Static HTML from numbers that come
// from the modem source (phy.rs, ldpc.rs, link.rs). Nothing here loads anything from
// outside. The one picture is served by this Worker at /operate.png.

/** Width and height of the screenshot file, in pixels. It is shown at no more than this size. */
export const SHOT_W = 1512;
export const SHOT_H = 940;

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
<p class="label">KAHF</p>
<h1 id="top-title">KAHF sends messages, mail and files between amateur radio stations over HF and VHF.</h1>
<p class="status"><a href="/map">Open the live map</a></p>
</div>
</div>

<div class="wrap">
<section class="sec" id="what" aria-labelledby="what-t">
<p class="label">What it does</p>
<h2 id="what-t">A message goes in at one radio and comes out at another.</h2>
<div class="grid even">
<div>
<p>It works through an ordinary radio and sound card, or through an SDR. Every message is signed by its sender, so the receiver can check who sent it. Nothing is encrypted, and nothing is sent until the operator confirms.</p>
<p>A station that hears two others can pass messages between them, so a message can reach a station that is out of direct range.</p>
</div>
<div>
<p>Working today in software tests: the modem over audio, messages, mail and file transfer, a contact log with ADIF export, and the station server behind the map.</p>
<p>Still being built: sound-card and SDR adapters, and map reporting from the app. Until that ships, the map can be empty.</p>
</div>
</div>
<figure class="shot">
<img src="/operate.png" width="${SHOT_W}" height="${SHOT_H}" alt="The Operate screen of the KAHF desktop app">
<figcaption>The Operate screen, from a development build. Shown at its real size, ${SHOT_W} by ${SHOT_H} pixels.</figcaption>
</figure>
</section>

<section class="sec" id="how" aria-labelledby="how-t">
<p class="label">How a message travels</p>
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
</div>
</div>
</section>

<section class="sec" id="licences" aria-labelledby="lic-t">
<p class="label">Licence and responsibility</p>
<h2 id="lic-t">Read this before you transmit.</h2>
<div class="grid even">
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
