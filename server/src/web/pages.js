// The inner pages of kahf.dev: the relay network, getting started, the modem and
// the legal notes. Static HTML. Every claim here comes from the user guide, the
// release notes or the modem source; where a feature is not built yet, the text says so.

/** @type {Record<string, { title: string, description: string, nav: string, body: () => string }>} */
export const SITE_PAGES = {
  "/network": {
    title: "KAHF relay network",
    description: "How KAHF relays pass messages between amateur stations that cannot hear each other, and what a relay operator agrees to.",
    nav: "Network",
    body: () => `
<div class="wrap">
<div class="pagehead">
<p class="label">The relay network</p>
<h1>Out of range is not the end of the path.</h1>
<p class="lede">HF reaches a long way, but not everywhere at once. A relay hears two stations that cannot hear each other, and passes the message between them.</p>
</div>
<section class="sec tight" aria-labelledby="n-how">
<h2 id="n-how">How a relay works</h2>
<ol class="steps">
<li><b>A station sends.</b>The sender transmits a signed message on the channel. If the addressee does not answer, the link layer asks relays that heard it.</li>
<li><b>A relay hears it.</b>A relay that received the frames checks them, and holds the message for the addressee.</li>
<li><b>The relay passes it on.</b>In its own scheduled slot the relay sends the message to the addressee. The receiver checks the sender's signature, so a relay cannot change a message without being caught.</li>
</ol>
<p class="note">If no station answers on any relay path within about two minutes, KAHF tells the sender and keeps the message in the outbox.</p>
</section>
<section class="sec" aria-labelledby="n-sched">
<h2 id="n-sched">Slots, so relays do not talk over each other</h2>
<div class="grid even">
<div>
<p>A relay registers with the network server under its callsign, one-degree area and key. A registration lasts 48 hours unless the relay registers again.</p>
<p>It then asks for send and receive slots. The server hands out slots that avoid channels already busy in the area, and a relay that cannot receive while it transmits takes turns.</p>
</div>
<div>
<dl class="kv">
<dt>Registration</dt><dd>signed with the relay's key, lasts 48 h</dd>
<dt>Callsign</dt><dd>belongs to the first key that registers it</dd>
<dt>Area</dt><dd>one degree of latitude and longitude</dd>
<dt>Map records</dt><dd>expire 48 h after the last refresh</dd>
</dl>
</div>
</div>
</section>
<section class="sec" aria-labelledby="n-op">
<h2 id="n-op">Running a relay</h2>
<div class="grid even">
<div>
<ul class="plain">
<li>Keep the radio on for the times in the schedule.</li>
<li>Accept or decline a schedule before it runs.</li>
<li>Relay mode starts only after you confirm your licence and accept the terms, each time it starts.</li>
</ul>
</div>
<div class="callout">
<p><strong>Check your rules first.</strong> Some countries require a licence or permit for automatic stations. In Finland, an unattended relay needs one. Relay mode is your responsibility.</p>
<p class="note">The desktop app does not show the relay schedule yet. The network server side is in place and tested.</p>
</div>
</div>
<div class="cta"><a class="primary" href="/map">See relays on the live map</a><a href="/start">Get started</a></div>
</section>
</div>`,
  },

  "/start": {
    title: "Get started with KAHF",
    description: "What you need, how to install KAHF and how to send a first message.",
    nav: "Get started",
    body: () => `
<div class="wrap">
<div class="pagehead">
<p class="label">Get started</p>
<h1>From download to first message.</h1>
<p class="lede">KAHF is pre-release. Test with a dummy load or a station you have agreed to test with, and transmit only where your licence allows it.</p>
</div>
<section class="sec tight" aria-labelledby="s-need">
<h2 id="s-need">What you need</h2>
<div class="grid three">
<div class="fact"><h3>A licence</h3><p>Your own amateur licence and callsign. Everything KAHF sends goes out under it.</p></div>
<div class="fact"><h3>A radio and an interface</h3><p>An HF radio, a sound-card interface to your computer, and a way to put the radio in a data mode. Or an SDR to receive.</p></div>
<div class="fact"><h3>A computer</h3><p>Official 1.0 builds are for macOS on Apple silicon and Linux on x86-64. Windows, Intel Macs and ARM64 Linux follow later.</p></div>
</div>
</section>
<section class="sec" aria-labelledby="s-steps">
<h2 id="s-steps">First run</h2>
<ol class="steps">
<li><b>Install.</b>Download the build for your system. It is not signed with an Apple or Microsoft certificate yet, so your computer will warn you. On macOS, right-click and choose Open. On Windows, choose More info, then Run anyway.</li>
<li><b>Set up.</b>Enter your callsign, choose the audio device for your radio, and set the radio to a data mode. KAHF starts in listen only, so nothing is sent.</li>
<li><b>Listen.</b>Tune a 17 m channel inside 18.068 to 18.168 MHz and watch the spectrum. Check the channel is clear before you connect.</li>
<li><b>Send.</b>Connect to a station, write a message, and confirm. KAHF asks you before every transmission.</li>
</ol>
<p class="note">To build from source instead, see the build guide in the repository.</p>
</section>
<section class="sec" aria-labelledby="s-hw">
<h2 id="s-hw">Hardware status</h2>
<dl class="kv">
<dt>Tested on real hardware</dt><dd>RTL-SDR v4, HackRF Pro</dd>
<dt>Transmit</dt><dd>HackRF or a rig</dd>
<dt>Receive only</dt><dd>the other SDR drivers, tested against fake servers</dd>
<dt>Default</dt><dd>listen only is on</dd>
</dl>
</section>
</div>`,
  },

  "/protocol": {
    title: "KAHF modem details",
    description: "The KAHF modem in numbers: OFDM carriers, LDPC code, frame size, and what a message costs on air.",
    nav: "Modem",
    body: () => `
<div class="wrap">
<div class="pagehead">
<p class="label">The modem</p>
<h1>153 carriers inside one voice channel.</h1>
<p class="lede">KAHF is an OFDM modem. It runs over the audio of an ordinary SSB radio, so a 2.4 kHz channel is all it needs.</p>
</div>
<section class="sec tight" aria-labelledby="p-num">
<h2 id="p-num">In numbers</h2>
<dl class="spec four">
<div><dt>Audio rate</dt><dd>8000 Hz</dd></div>
<div><dt>Carriers</dt><dd>153</dd></div>
<div><dt>Spacing</dt><dd>15.625 Hz</dd></div>
<div><dt>Band</dt><dd>312 Hz to 2.69 kHz</dd></div>
<div><dt>Code</dt><dd>LDPC 648/324</dd></div>
<div><dt>Frame</dt><dd>36 bytes</dd></div>
<div><dt>Pilots</dt><dd>every 4th</dd></div>
<div><dt>Modulation</dt><dd>QPSK to 64QAM</dd></div>
</dl>
</section>
<section class="sec" aria-labelledby="p-how">
<h2 id="p-how">What each layer does</h2>
<div class="grid even">
<div>
<h3>Error correction</h3>
<p>Each frame of up to 36 bytes goes through an LDPC code: 324 bits in, 648 bits out. The decoder repairs bit errors from noise and fading before the frame is checked.</p>
<h3>Adaptive modulation</h3>
<p>Frames carry their own modulation, and the receiver accepts any. Strong links can step up to denser modulation, and a weak mode works for links that cannot reply, down to about -4 dB.</p>
</div>
<div>
<h3>Fading</h3>
<p>The 1.0 modem decodes two-path fading at 1 dB with under 1 percent frame loss, measured in the channel simulator. Real-air results will be published when there are enough of them.</p>
<h3>Integrity and signatures</h3>
<p>Every frame is checked, failed ones are asked for again, and files carry a checksum. Signed messages show who signed them. Nothing is encrypted.</p>
</div>
</div>
</section>
</div>`,
  },

  "/legal": {
    title: "KAHF licence, privacy and responsibility",
    description: "Licence, your responsibility as a licensed operator, map privacy and content rules for KAHF.",
    nav: "Legal",
    body: () => `
<div class="wrap">
<div class="pagehead">
<p class="label">Licence and responsibility</p>
<h1>The rules KAHF runs under.</h1>
</div>
<section class="sec tight" aria-labelledby="l-lic">
<div class="grid even">
<div>
<h2 id="l-lic">Licence</h2>
<p>KAHF Source Licence, version 1.0. The source can be read and built, and official builds can be installed by anyone. Copyright (c) 2026 adswill, all rights reserved. Nothing here grants a licence to copy or redistribute the code.</p>
<h3>Your responsibility</h3>
<p>KAHF is for licensed amateur radio operators. Bands, modes, power and rules on third-party traffic differ from country to country, and you are responsible for staying inside yours. KAHF asks you to confirm each transmission. It does not decide for you.</p>
<h3>Content</h3>
<p>No religious or political content. The project does not carry or promote either, and operators remain responsible for what they send.</p>
</div>
<div>
<h2>Privacy</h2>
<p>Location sharing on the map is off by default. If you turn it on, you share a point accurate to about 200 m, or a circle of 1 to 100 km with your real position somewhere inside.</p>
<p>The server keeps your callsign, role, the position you chose to share and when you were last seen. Records expire 48 hours after the last refresh. Your messages, files, logbook and station key stay on your computer.</p>
<div class="callout"><p><strong>Not encrypted.</strong> Messages on air are readable by anyone with a receiver. A private message is a routing rule, not encryption.</p></div>
<p class="note">The full terms of use and privacy notice are being reviewed and will be published here with the first public release.</p>
</div>
</div>
</section>
</div>`,
  },
};
