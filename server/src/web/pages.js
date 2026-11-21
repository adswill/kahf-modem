// The inner pages of kahf.dev: the relay network, getting started, the modem and
// the legal notes. Static HTML. Every claim here comes from the user guide, the
// release notes or the modem source; where a feature is not built yet, the text says so.

/** @type {Record<string, { title: string, description: string, body: () => string }>} */
export const SITE_PAGES = {
  "/network": {
    title: "KAHF relay network",
    description: "How KAHF relays pass messages between amateur stations that cannot hear each other, and what a relay operator agrees to.",
    body: () => `
<div class="wrap">
<div class="pagehead">
<h1>Out of range is not the <em>end</em> of the path.</h1>
<p class="lede">HF reaches a long way, but not everywhere at once. A relay hears two stations that cannot hear each other, and passes the message between them.</p>
</div>
<section class="sec tight" aria-labelledby="n-how">
<h2 id="n-how">How a relay works</h2>
<ol class="steps">
<li><b>A station sends.</b>The sender transmits a signed message on a lane. If the addressee does not answer, the sender's computer asks the relays that heard it.</li>
<li><b>A relay hears it.</b>A relay that received the frames checks them and tells the network only that it heard them: ids, sizes and routing facts, never the message itself.</li>
<li><b>The relay passes it on.</b>In its scheduled slot the relay sends the message to the addressee. The receiver checks the sender's signature, so a relay cannot change a message without being caught.</li>
</ol>
<p class="note">The network server stores no messages. It keeps who is online and routing facts in memory, and the sender's own computer keeps the outbox. If no station answers on any relay path within about two minutes, KAHF tells the sender.</p>
</section>
<section class="sec" aria-labelledby="n-sched">
<h2 id="n-sched">Slots, so relays do not talk over each other</h2>
<div class="grid even">
<div>
<p>A relay registers with the network server under its callsign, one-degree area and key. A registration lasts 48 hours unless the relay registers again.</p>
<p>It then asks for send and receive slots. The server hands out slots that avoid lanes already busy in the area, and a relay that cannot receive while it transmits takes turns.</p>
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
    body: () => `
<div class="wrap">
<div class="pagehead">
<h1>From download to first message.</h1>
<p class="lede">KAHF is pre-release. Test with a dummy load or a station you have agreed to test with, and transmit only where your licence allows it.</p>
</div>
<section class="sec tight" aria-labelledby="s-need">
<h2 id="s-need">What you need</h2>
<dl class="ledger">
<div><dt>Licence</dt><dd class="v">your callsign</dd><dd class="n">Your own amateur licence. Everything KAHF sends goes out under it.</dd></div>
<div><dt>Radio</dt><dd class="v">HF rig and interface</dd><dd class="n">A sound-card interface to your computer and a way to put the radio in a data mode. Or an SDR to receive.</dd></div>
<div><dt>Computer</dt><dd class="v">macOS or Linux</dd><dd class="n">Official 1.0 builds are for macOS on Apple silicon and Linux on x86-64. Windows, Intel Macs and ARM64 Linux follow later.</dd></div>
</dl>
</section>
<section class="sec" aria-labelledby="s-steps">
<h2 id="s-steps">First run</h2>
<ol class="steps">
<li><b>Install.</b>Download the build for your system. It is not signed with an Apple or Microsoft certificate yet, so your computer will warn you. On macOS, right-click and choose Open.</li>
<li><b>Set up.</b>Enter your callsign, choose the audio device for your radio, and set the radio to a data mode. KAHF starts in listen only, so nothing is sent.</li>
<li><b>Listen.</b>Tune the radio to the USB dial of a lane (twelve lanes sit inside 18.111 to 18.120 MHz) and watch the spectrum. Check the lane and its neighbours are clear before you connect.</li>
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
    body: () => `
<div class="wrap">
<div class="pagehead">
<h1>153 carriers inside one voice channel.</h1>
<p class="lede">KAHF is an OFDM modem. It runs over the audio of an ordinary SSB radio, so a 2.4 kHz channel is all it needs.</p>
</div>
<section class="sec tight" aria-labelledby="p-num">
<h2 id="p-num">In numbers</h2>
<dl class="ledger">
<div><dt>Audio</dt><dd class="v">8000 Hz</dd><dd class="n">Sample rate. The signal sits 312 Hz to 2.69 kHz above the tuned frequency.</dd></div>
<div><dt>Carriers</dt><dd class="v">153 at 15.625 Hz</dd><dd class="n">Every fourth carrier is a pilot.</dd></div>
<div><dt>Code</dt><dd class="v">LDPC 648/324</dd><dd class="n">324 bits in, 648 bits out.</dd></div>
<div><dt>Frame</dt><dd class="v">36 bytes</dd><dd class="n">Up to 36 bytes each, numbered.</dd></div>
<div><dt>Modulation</dt><dd class="v">QPSK to 64QAM</dd><dd class="n">Each frame says which one it uses.</dd></div>
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
    body: () => `
<div class="wrap">
<div class="pagehead">
<h1>The rules KAHF runs under.</h1>
</div>
<section class="sec tight" aria-labelledby="l-lic">
<div class="grid even">
<div>
<h2 id="l-lic">Licence</h2>
<p>KAHF is free software under the GNU General Public License, version 3 or later. Copyright (c) 2026 Kristijonas Adomaitis (adswill). You can use, study, change and share it. Changed versions, and programs built on it, must be shared under the same licence with their source.</p>
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
