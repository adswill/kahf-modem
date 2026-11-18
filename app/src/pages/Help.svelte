<script lang="ts">
  import { app } from "../lib/state.svelte";
  import { MODE_TABLE } from "../lib/defaults";
  import Icon from "../components/Icon.svelte";

  const topics = [
    { id: "how", title: "How KAHF works" },
    { id: "first", title: "Send your first message" },
    { id: "signal", title: "Signal quality" },
    { id: "speed", title: "Speeds explained" },
    { id: "channel", title: "Channels and the band" },
    { id: "relay", title: "Relays and the map" },
    { id: "privacy", title: "Privacy and location" },
    { id: "trouble", title: "When something is wrong" },
    { id: "words", title: "Glossary" },
  ];
  let open = $state(app.helpTopic || "how");
  $effect(() => {
    if (app.helpTopic) open = app.helpTopic;
  });
</script>

<div class="page" data-tour="help">
  <header class="head">
    <div>
      <h1>Help</h1>
      <p class="muted">Short answers in plain words. Nothing here assumes you know radio.</p>
    </div>
    <button class="btn primary" onclick={() => (app.tourOpen = true)}><Icon name="play" size={15} /> Take the tour</button>
  </header>

  <div class="help">
    <nav class="toc" aria-label="Help topics">
      {#each topics as t}
        <button class:on={open === t.id} onclick={() => (open = t.id)}>{t.title}</button>
      {/each}
    </nav>

    <article class="card body">
      {#if open === "how"}
        <h2>How KAHF works</h2>
        <p>
          KAHF sends text, mail and small files over amateur radio instead of the internet. You write a message on
          your computer. KAHF turns it into a short burst of sound, your radio sends that sound into the air, and
          another KAHF station far away turns it back into your words.
        </p>
        <div class="diagram" aria-hidden="true">
          <div><Icon name="message" size={26} /><span>Your message</span></div>
          <Icon name="arrow" />
          <div><Icon name="wave" size={26} /><span>A burst of sound</span></div>
          <Icon name="arrow" />
          <div><Icon name="radio" size={26} /><span>Your radio</span></div>
          <Icon name="arrow" />
          <div><Icon name="network" size={26} /><span>Through the air</span></div>
          <Icon name="arrow" />
          <div><Icon name="message" size={26} /><span>Their screen</span></div>
        </div>
        <p>Three things matter: a <strong>callsign</strong> (your name on the air), a <strong>frequency</strong> (where everyone meets), and the <strong>signal</strong> (how well you hear each other).</p>
      {:else if open === "first"}
        <h2>Send your first message</h2>
        <ol>
          <li><strong>Leave KAHF listening</strong> for a few minutes. Stations that are on the air appear on the Home and Network pages.</li>
          <li>Open <button class="link" onclick={() => app.go("messages")}>Messages</button> and type the callsign of a station you heard.</li>
          <li>Write a line and press <kbd>Enter</kbd>. KAHF calls them, sends it, and tells you what happened.</li>
        </ol>
        <div class="notice">
          <Icon name="info" />
          <span>
            You never press "Connect". If the other station does not answer, KAHF tries again by itself and
            explains why it failed.
          </span>
        </div>
      {:else if open === "signal"}
        <h2>Signal quality</h2>
        <p>
          Radio signals are always mixed with some noise. KAHF measures how strong the signal is compared with the
          noise, in <strong>decibels (dB)</strong>. Higher is better.
        </p>
        <table>
          <tbody>
            <tr><td><span class="chip good">Excellent</span></td><td>10 dB or more</td><td>Fast speeds work.</td></tr>
            <tr><td><span class="chip good">Good</span></td><td>3 to 10 dB</td><td>Messages go through quickly.</td></tr>
            <tr><td><span class="chip warn">Fair</span></td><td>-5 to 3 dB</td><td>Works, at a moderate speed.</td></tr>
            <tr><td><span class="chip warn">Weak</span></td><td>-12 to -5 dB</td><td>Slow speeds only. Still readable.</td></tr>
            <tr><td><span class="chip bad">Very weak</span></td><td>below -12 dB</td><td>Only the slowest speed can work.</td></tr>
          </tbody>
        </table>
        <p class="muted">A negative number is normal: KAHF can read a signal that is weaker than the noise around it.</p>
      {:else if open === "speed"}
        <h2>Speeds explained</h2>
        <p>
          KAHF has several speeds, named KAMI-256 down to KAMI-1d. The bigger the number, the slower and the more
          sure. <strong>You do not need to choose</strong>: Automatic mode picks the fastest one your signal can carry.
        </p>
        <table>
          <thead><tr><th>Mode</th><th>Needs at least</th><th>Speed</th></tr></thead>
          <tbody>
            {#each MODE_TABLE as m}
              <tr><td class="mono">{m.name}</td><td>{m.needSnr} dB</td><td>{m.netBps} bit/s</td></tr>
            {/each}
          </tbody>
        </table>
        <p class="muted small">1 bit per second is about one letter every 8 seconds. All speeds use the same narrow channel.</p>
      {:else if open === "channel"}
        <h2>Channels and the band</h2>
        <p>
          Radio is shared. The <strong>band</strong> is a stretch of frequencies, and a <strong>channel</strong> is a
          slice of it. A KAHF transmission is about 2.4 kHz wide, which fits one channel (2.7 kHz). Every speed uses the
          same width, so one message never takes the whole band.
        </p>
        <p>
          On the Radio page the shaded area is your channel. Bright lines inside it are KAHF signals. Anything outside it
          is other users of the band, and that is normal.
        </p>
      {:else if open === "relay"}
        <h2>Relays and the map</h2>
        <p>
          A <strong>relay</strong> is a station that stays on the air and passes messages along, so people who cannot
          hear each other can still talk. KAHF uses relays for you when a station does not answer.
        </p>
        <p>
          The <strong>map</strong> at kahf.dev shows stations that chose to share their location. It also lists relays so
          you can see where help is nearby. You are never on the map unless you turn it on.
        </p>
      {:else if open === "privacy"}
        <h2>Privacy and location</h2>
        <ul>
          <li><strong>Messages</strong> are sent over open radio. Anyone with a receiver can hear that something was sent. Use <em>Private</em> so only the addressed station keeps it. KAHF does not hide the radio signal itself.</li>
          <li><strong>Your location</strong> is off by default. You can share a circle (your exact spot stays private) or your position to about 200 metres.</li>
          <li><strong>Your callsign</strong> is public by nature. It is how amateur radio works.</li>
        </ul>
      {:else if open === "trouble"}
        <h2>When something is wrong</h2>
        <details open><summary>Nobody appears on the Network page</summary>
          <p>Give it a few minutes. Then check the Radio page: if the picture is flat, KAHF cannot hear sound. Check the cable, the radio's volume, and the sound device in Settings.</p></details>
        <details><summary>"Did not answer"</summary>
          <p>The other station may be off, too far, or listening on another frequency. KAHF tries again and uses a relay if it knows one. Try again later, or write to a station you have heard.</p></details>
        <details><summary>My message says "Waiting"</summary>
          <p>KAHF waits for a quiet moment on the channel before it transmits, so it does not talk over someone else. It sends as soon as the channel is clear.</p></details>
        <details><summary>I cannot send anything</summary>
          <p>Listen-only may be on. Turn it off in Settings. You also need a callsign saved there.</p></details>
        <details><summary>The speed seems slow</summary>
          <p>Weak signals need slow, sure speeds. That is by design. Moving your antenna or waiting for better conditions helps more than changing settings.</p></details>
      {:else}
        <h2>Glossary</h2>
        <dl>
          <dt>Callsign</dt><dd>Your unique name on the air, issued with your amateur licence.</dd>
          <dt>Band</dt><dd>A range of radio frequencies, like 20 metres (14 MHz).</dd>
          <dt>Channel</dt><dd>A 2.7 kHz slice of a band. One KAHF message fits in one.</dd>
          <dt>Frequency</dt><dd>Where on the dial the radio is tuned, in MHz.</dd>
          <dt>SNR</dt><dd>Signal-to-noise ratio. How much stronger the signal is than the noise, in dB.</dd>
          <dt>dB</dt><dd>Decibel. A unit for comparing signal strengths. +3 dB is twice the power.</dd>
          <dt>Relay</dt><dd>A station that passes messages on for others.</dd>
          <dt>KAMI</dt><dd>The way KAHF turns data into sound. The number is the speed step.</dd>
          <dt>Waterfall</dt><dd>The scrolling picture on the Radio page. Time runs down, brighter means stronger.</dd>
          <dt>Grid square</dt><dd>A short code, like JN45, that names an area of the Earth.</dd>
        </dl>
      {/if}
    </article>
  </div>
</div>

<style>
  .help {
    display: grid;
    grid-template-columns: 240px 1fr;
    gap: 20px;
    align-items: start;
  }
  .toc {
    display: flex;
    flex-direction: column;
    gap: 3px;
    position: sticky;
    top: 0;
  }
  .toc button {
    text-align: left;
    background: none;
    border: 0;
    color: var(--muted);
    font: inherit;
    font-weight: 600;
    padding: 10px 12px;
    border-radius: 10px;
    cursor: pointer;
  }
  .toc button:hover {
    background: var(--raised);
    color: var(--text);
  }
  .toc button.on {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .body {
    display: flex;
    flex-direction: column;
    gap: 14px;
    line-height: 1.6;
  }
  .body ol,
  .body ul {
    padding-left: 22px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin: 0;
  }
  .diagram {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
    padding: 18px;
    border-radius: 14px;
    background: var(--panel-2);
    color: var(--faint);
  }
  .diagram div {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    color: var(--text);
    font-size: 13px;
    min-width: 96px;
    text-align: center;
  }
  table {
    border-collapse: collapse;
    width: 100%;
  }
  td,
  th {
    padding: 8px 10px;
    border-top: 1px solid var(--line-soft);
    text-align: left;
  }
  dl {
    display: grid;
    grid-template-columns: 150px 1fr;
    gap: 10px 18px;
    margin: 0;
  }
  dt {
    font-weight: 700;
  }
  dd {
    margin: 0;
    color: var(--muted);
  }
  details {
    border: 1px solid var(--line-soft);
    border-radius: 12px;
    padding: 10px 14px;
    background: var(--panel-2);
  }
  summary {
    cursor: pointer;
    font-weight: 600;
  }
  details p {
    margin-top: 8px;
    color: var(--muted);
  }
  button.link {
    background: none;
    border: 0;
    color: var(--accent);
    font: inherit;
    cursor: pointer;
    padding: 0;
    text-decoration: underline;
  }
</style>
