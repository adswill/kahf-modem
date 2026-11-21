<script lang="ts">
  import { app } from "../lib/state.svelte";
  import { callProblem, normaliseCall, validCall } from "../lib/callsign";
  import { validLat, validLon } from "../lib/geo";
  import type { LocationMode } from "../lib/types";
  import AudioPicker from "../components/AudioPicker.svelte";
  import RigPicker from "../components/RigPicker.svelte";
  import LocationEditor from "../components/LocationEditor.svelte";
  import Spectrum from "../components/Spectrum.svelte";
  import Icon from "../components/Icon.svelte";
  import { BANDS } from "../lib/bands";

  // A copy of the settings that only becomes real when the person finishes.
  let draft = $state({ ...app.settings, location: { ...app.settings.location }, rig: { ...app.settings.rig } });
  let step = $state(0);
  let haveLicence = $state(app.settings.callsign !== "" || !app.settings.listenOnly);
  let kind = $state<"sound" | "sdr" | "none">(draft.sdr ? "sdr" : "sound");
  let previewing = $state(false);

  const STEPS = ["You", "Radio", "Channel", "Place", "Ready"];
  // 17 m only. A new station starts on the first channel; the Radio screen changes it later.

  const call = $derived(normaliseCall(draft.callsign));
  const callMsg = $derived(callProblem(draft.callsign));
  const callOk = $derived(validCall(call) && !callMsg);
  const posOk = $derived(validLat(draft.location.lat) && validLon(draft.location.lon));
  const canNext = $derived(step === 0 ? (haveLicence ? callOk : true) : step === 3 ? draft.location.mode === "off" || posOk : true);

  async function preview() {
    previewing = true;
    await app.engine.start($state.snapshot({ ...draft, callsign: call, audioIn: kind === "sound" ? draft.audioIn : "" }) as typeof draft);
  }

  async function finish() {
    await app.finishSetup({
      callsign: haveLicence ? call : "",
      listenOnly: !haveLicence,
      audioIn: kind === "sound" ? draft.audioIn : "",
      audioOut: kind === "sound" ? draft.audioOut : "",
      sdr: kind === "sdr" ? draft.sdr || "HackRF" : "",
      rig: { ...draft.rig },
      freqKhz: draft.freqKhz,
      location: {
        mode: draft.location.mode as LocationMode,
        lat: draft.location.mode === "off" ? null : draft.location.lat,
        lon: draft.location.mode === "off" ? null : draft.location.lon,
        radiusKm: draft.location.radiusKm,
      },
      autoLink: draft.autoLink,
      autoMode: draft.autoMode,
    });
    app.tourOpen = !app.settings.tourDone;
  }

  const next = () => (step < STEPS.length - 1 ? (step += 1) : finish());
</script>

<main class="setup">
  <header class="row">
    <span class="brand">KAHF</span>
    <span class="muted">Set up</span>
    <div class="spacer"></div>
    <button class="btn quiet sm" onclick={() => app.go(app.settings.setupDone ? "chats" : "welcome")}>Cancel</button>
  </header>

  <ol class="steps" aria-label="Progress">
    {#each STEPS as s, i}
      <li class:done={i < step} class:now={i === step}><span class="mono">{i + 1}</span> {s}</li>
    {/each}
  </ol>

  <section class="body">
    {#if step === 0}
      <h1>Who are you on the air?</h1>
      <p class="muted">Every radio amateur has a callsign, like <span class="call">KA1ABC</span>. It is how other stations know it is you.</p>
      <button class="pick" class:on={haveLicence} onclick={() => (haveLicence = true)}>
        <span class="dot"></span><span><b>I have an amateur radio licence</b><span class="s">I can transmit and I know my callsign.</span></span>
      </button>
      {#if haveLicence}
        <label class="field">
          <span class="t">Your callsign</span>
          <input type="text" placeholder="For example KA1ABC" autocapitalize="characters" spellcheck="false" bind:value={draft.callsign} oninput={() => (draft.callsign = draft.callsign.toUpperCase())} />
          {#if callMsg}<span class="err">{callMsg}</span>{:else if callOk}<span class="ok">Looks good.</span>{:else}<span class="hint">Letters and numbers, with at least one number.</span>{/if}
        </label>
        <div class="note warn">Only transmit if your licence allows it on the frequency you use. KAHF cannot check that for you.</div>
      {/if}
      <button class="pick" class:on={!haveLicence} onclick={() => (haveLicence = false)}>
        <span class="dot"></span><span><b>I only want to listen for now</b><span class="s">KAHF will not transmit. You can add a callsign later in Settings.</span></span>
      </button>
    {:else if step === 1}
      <h1>Connect your radio</h1>
      <p class="muted">Two separate things: how the sound reaches the computer, and, optionally, letting KAHF control the radio's frequency.</p>
      <h2>1. Sound</h2>
      <AudioPicker bind:kind bind:audioIn={draft.audioIn} bind:audioOut={draft.audioOut} bind:sdr={draft.sdr} />
      {#if kind !== "none"}
        <div class="row">
          <button class="btn" onclick={preview}>Check that I can hear it</button>
          <span class="faint small">Moving lines below mean it works.</span>
        </div>
        {#if previewing}<Spectrum height={200} compact />{/if}
      {/if}
      <h2>2. Radio control <span class="faint small">(optional)</span></h2>
      <RigPicker bind:value={draft.rig} />
    {:else if step === 2}
      <h1>Where do you listen?</h1>
      <p class="muted">KAHF uses one band, 17 metres (18.111 to 18.120 MHz). You can change channel later on the Radio screen.</p>
      {#each BANDS as ch (ch.k)}
        <button class="pick" class:on={Math.abs(draft.freqKhz - ch.khz) < 0.001} onclick={() => (draft.freqKhz = ch.khz)}>
          <span class="dot"></span>
          <span><b>17 metres, {ch.name} <span class="mono faint">{(ch.khz / 1000).toFixed(5)} MHz</span></b>{#if ch.k === 0}<span class="s">Tune your radio to this frequency. Best by day over long distances, roughly 1000 to 4000 km, and mostly closed at night.</span>{/if}</span>
        </button>
      {/each}
    {:else if step === 3}
      <h1>Do you want to be on the map?</h1>
      <p class="muted">The public map at kahf.dev shows stations that choose to share where they are. <b>It is off unless you turn it on.</b></p>
      <LocationEditor bind:value={draft.location} />
    {:else}
      <h1>You are ready</h1>
      <p class="muted">This is what KAHF will do. You can change all of it later.</p>
      <dl class="sum">
        <dt>Callsign</dt><dd class="call">{haveLicence ? call : "listening only"}</dd>
        <dt>Sound</dt><dd>{kind === "sound" ? "Sound card" : kind === "sdr" ? "SDR receiver" : "none yet"}</dd>
        <dt>Radio control</dt><dd>{draft.rig.model !== null ? draft.rig.name : "off"}</dd>
        <dt>Channel</dt><dd class="mono">{(draft.freqKhz / 1000).toFixed(5)} MHz, 17 m</dd>
        <dt>Map</dt><dd>{draft.location.mode === "off" ? "not shared" : draft.location.mode === "area" ? "a circle" : "exact position"}</dd>
      </dl>
    {/if}
  </section>

  <footer class="row">
    <button class="btn" disabled={step === 0} onclick={() => (step -= 1)}>Back</button>
    <div class="spacer"></div>
    <button class="btn primary lg" disabled={!canNext} onclick={next}>
      {step === STEPS.length - 1 ? "Start listening" : "Continue"} <Icon name="arrow" size={15} />
    </button>
  </footer>
</main>

<style>
  .setup {
    height: min(660px, calc(100% - 32px));
    width: min(780px, calc(100% - 32px));
    margin: 16px auto;
    padding: 0 22px 14px;
    display: flex;
    flex-direction: column;
    background: var(--panel);
    border: 1px solid var(--line-strong);
    border-radius: 8px;
    box-shadow: var(--shadow);
    overflow: hidden;
  }
  .setup > header {
    margin: 0 -22px;
    padding: 6px 10px 6px 14px;
    background: linear-gradient(var(--title-top), var(--title-bot));
    color: var(--title-ink);
  }
  .setup > header :global(.muted),
  .setup > header :global(.btn) {
    color: var(--title-ink);
  }
  .setup > header :global(.btn:hover) {
    background: rgba(255, 255, 255, 0.2);
    border-color: rgba(255, 255, 255, 0.5);
  }
  .brand {
    font-weight: 700;
    letter-spacing: 0.2em;
  }
  .steps {
    list-style: none;
    display: flex;
    margin: 16px 0 6px;
    padding: 0;
    border-bottom: 1px solid var(--line);
  }
  .steps li {
    flex: 1;
    padding: 8px 0;
    color: var(--faint);
    border-bottom: 2px solid transparent;
    margin-bottom: -1px;
  }
  .steps li.now {
    color: var(--text);
    border-bottom-color: var(--accent);
  }
  .steps li.done {
    color: var(--accent);
  }
  .body {
    flex: 1;
    overflow: auto;
    padding: 16px 2px 20px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  footer {
    padding-top: 12px;
    border-top: 1px solid var(--line);
  }
  .sum {
    display: grid;
    grid-template-columns: 140px 1fr;
    gap: 0;
    margin: 0;
    border: 1px solid var(--line);
    border-radius: 6px;
  }
  .sum dt,
  .sum dd {
    margin: 0;
    padding: 9px 14px;
    border-bottom: 1px solid var(--line);
  }
  .sum dt {
    color: var(--muted);
  }
</style>
