<script lang="ts">
  import { modal } from "../lib/modal";
  import { app } from "../lib/state.svelte";
  import { MODE_TABLE } from "../lib/defaults";
  import Icon from "./Icon.svelte";

  interface Topic {
    id: string;
    title: string;
    body: string[];
  }

  // Plain words. Each paragraph stands alone, so the search can show the right topic.
  const TOPICS: Topic[] = [
    {
      id: "how",
      title: "How KAHF works",
      body: [
        "KAHF sends messages, mail, files and voice notes over amateur radio instead of the internet. You write on your computer. KAHF turns it into a short burst of sound, your radio sends that into the air, and another KAHF station turns it back into your words.",
        "Three things matter: a callsign (your name on the air), a frequency (where everyone meets) and the signal (how well you hear each other).",
      ],
    },
    {
      id: "first",
      title: "Send your first message",
      body: [
        "1. Leave KAHF running for a few minutes. Stations that are on the air appear under Chats, in the list on the left.",
        "2. Click one, or type a callsign in the To box.",
        "3. Write a line and press Enter. KAHF reaches the station, sends it, and shows what happened under your message.",
        "You never press Connect. If the other station does not answer, KAHF says why and offers to try again.",
      ],
    },
    {
      id: "kinds",
      title: "Messages, mail, files and voice",
      body: [
        "Chats are for quick lines. Under the box you can send a message as Normal, Private, Signed or Urgent.",
        "Voice notes: press the microphone, speak, press Send. A voice note is squeezed to a few hundred bytes, so ten seconds is under one kilobyte. It travels as a small file and the other station plays it.",
        "Files: press the paperclip. A progress bar shows how much has gone out. Big files take a long time on a weak signal.",
        "Mail is for longer messages with a subject and attachments. It has an inbox and a sent folder.",
      ],
    },
    {
      id: "signal",
      title: "Signal quality",
      body: [
        "KAHF measures how strong the signal is compared with the noise, in decibels (dB). Higher is better. Excellent is 10 dB or more, Good is 3 to 10, Fair is -5 to 3, Weak is -12 to -5, Very weak is below that.",
        "A negative number is normal. KAHF can read a signal that is weaker than the noise around it.",
      ],
    },
    {
      id: "speed",
      title: "Speeds",
      body: [
        "KAHF has speeds named KAMI-256 down to KAMI-1d. The bigger the number, the slower and surer. You do not need to choose: Automatic picks the fastest speed your signal can carry.",
        MODE_TABLE.map((m) => `${m.name}: needs ${m.needSnr} dB, about ${m.netBps} bit/s`).join("; ") + ".",
        "All speeds use the same narrow channel, so a message never takes more of the band.",
      ],
    },
    {
      id: "channel",
      title: "Channels and the band",
      body: [
        "The band is a stretch of frequencies. A channel is a slice of it. KAHF uses one channel about 2.4 kHz wide. On the Radio screen that is the shaded area. Bright lines outside it are other users of the band, which is normal.",
      ],
    },
    {
      id: "rig",
      title: "Controlling your radio from the computer",
      body: [
        "On the Radio screen, under Radio control, search for your radio by name, such as IC-7300 or FT-991. Choose the serial port (the USB cable from the radio) and the speed, then press Test. KAHF reads your radio's frequency to prove it works.",
        "This uses Hamlib, a free program that talks to more than 300 radios. Install it with brew install hamlib on a Mac. Sound still comes through your sound card or interface.",
        "If the test fails: check that the radio is on, the cable is in, the speed matches the radio's menu, and, on an Icom, the CI-V address matches.",
      ],
    },
    {
      id: "relay",
      title: "Stations, relays and the map",
      body: [
        "A station is one person's KAHF setup, named by its callsign. Every KAHF user runs a station.",
        "A relay is a station that stays on the air and passes messages between stations that cannot hear each other. A relay is always a station, but most stations are not relays.",
        "KAHF does not need the internet. The relay list comes from kahf.dev when you are online and is remembered for when you are not. You can also add a relay by callsign. When a station does not answer, KAHF tries your chosen relays in order.",
        "The map at kahf.dev shows relays, and stations that chose to share a location. You are never on it unless you turn it on.",
      ],
    },
    {
      id: "privacy",
      title: "Privacy and location",
      body: [
        "Messages go out over open radio: anyone with a receiver can hear that something was sent. Private tells other stations to ignore a message, but it does not hide the radio signal.",
        "Your location is off by default. You can share a circle (your exact spot stays private) or your position to about 200 metres.",
        "Your callsign is public by nature. It is how amateur radio works.",
      ],
    },
    {
      id: "trouble",
      title: "When something is wrong",
      body: [
        "Nobody appears: give it a few minutes, then look at the picture on the Radio screen. If it is flat, KAHF is not hearing sound: check the cable, the radio volume and the sound device.",
        "Did not answer: the other station may be off, too far, or on another frequency. KAHF tries again and uses a relay if it knows one.",
        "Message says waiting: KAHF waits for a quiet moment on the channel so it does not talk over someone. It sends as soon as the channel is clear.",
        "Cannot send anything: listen-only may be on (Settings, Sending), or no callsign is saved.",
        "Voice note will not record: choose your microphone in Settings, under Sending.",
      ],
    },
    {
      id: "words",
      title: "Glossary",
      body: [
        "Callsign: your unique name on the air, issued with your amateur licence.",
        "Station: one person's KAHF setup. Relay: a station that passes messages on for others.",
        "Band: a range of frequencies, like 20 metres (14 MHz). Channel: a 2.7 kHz slice of a band.",
        "SNR: signal-to-noise ratio, in dB. KAMI: the way KAHF turns data into sound.",
        "Waterfall: the scrolling picture on the Radio screen. Time runs down, brighter is stronger.",
        "Grid square: a short code, like JN45, that names an area of the Earth.",
        "Connect: your station sends a short signal asking another to answer. KAHF does this for you when you send.",
      ],
    },
  ];

  let q = $state("");
  let open = $state(app.helpTopic || "how");
  const shown = $derived.by(() => {
    const t = q.trim().toLowerCase();
    if (!t) return TOPICS;
    return TOPICS.filter((x) => (x.title + " " + x.body.join(" ")).toLowerCase().includes(t));
  });
  const topic = $derived(TOPICS.find((t) => t.id === open) ?? shown[0] ?? TOPICS[0]);
  $effect(() => {
    if (shown.length && !shown.some((t) => t.id === open)) open = shown[0].id;
  });
  const close = () => (app.helpOpen = false);
</script>

<div class="scrim" role="presentation" onclick={close}></div>
<div class="help" role="dialog" aria-label="Help" use:modal={{ onClose: close }}>
  <header class="row">
    <h1>Help</h1>
    <div class="spacer"></div>
    <button class="btn sm" onclick={() => { close(); app.tourOpen = true; }}><Icon name="play" size={13} /> Take the tour</button>
    <button class="btn quiet icon" onclick={close} aria-label="Close"><Icon name="close" size={17} /></button>
  </header>
  <div class="find">
    <Icon name="search" size={15} />
    <input type="search" placeholder="Search help: relay, voice, signal…" bind:value={q} aria-label="Search help" />
  </div>
  <div class="cols">
    <nav>
      {#each shown as t}
        <button class:on={topic.id === t.id} onclick={() => (open = t.id)}>{t.title}</button>
      {/each}
      {#if shown.length === 0}<p class="faint small pad">Nothing matches "{q}".</p>{/if}
    </nav>
    <article>
      {#if shown.length}
        <h2>{topic.title}</h2>
        {#each topic.body as p}<p>{p}</p>{/each}
      {/if}
    </article>
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    background: var(--scrim);
    z-index: 60;
  }
  .help {
    position: fixed;
    z-index: 61;
    right: 0;
    top: 0;
    bottom: 0;
    width: min(620px, 96vw);
    background: var(--panel);
    border-left: 1px solid var(--line-strong);
    box-shadow: var(--shadow);
    display: flex;
    flex-direction: column;
  }
  header {
    padding: 6px 8px 6px 14px;
    background: linear-gradient(var(--title-top), var(--title-bot));
    color: var(--title-ink);
    border-radius: 0;
  }
  header :global(h2) {
    color: inherit;
    font-size: 13px;
  }
  header :global(.btn) {
    color: var(--title-ink);
  }
  header :global(.btn:hover) {
    background: rgba(255, 255, 255, 0.2);
    border-color: rgba(255, 255, 255, 0.5);
  }
  .find {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 16px;
    border-bottom: 1px solid var(--line);
    color: var(--faint);
  }
  .find input {
    border: 0;
    background: none;
    padding: 4px 0;
  }
  .cols {
    flex: 1;
    display: grid;
    grid-template-columns: 190px 1fr;
    min-height: 0;
  }
  nav {
    border-right: 1px solid var(--line);
    overflow: auto;
    display: flex;
    flex-direction: column;
  }
  nav button {
    text-align: left;
    background: none;
    border: 0;
    color: var(--muted);
    font: inherit;
    padding: 9px 14px;
    cursor: pointer;
  }
  nav button:hover {
    background: var(--hover);
    color: var(--text);
  }
  nav button.on {
    color: var(--text);
    background: var(--hover);
    box-shadow: inset 0 0 0 1px var(--accent);
  }
  .pad {
    padding: 12px;
  }
  article {
    overflow: auto;
    padding: 18px 22px;
    display: flex;
    flex-direction: column;
    gap: 12px;
    line-height: 1.6;
  }
</style>
