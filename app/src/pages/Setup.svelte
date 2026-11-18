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

  // A copy of the settings that only becomes real when the person finishes.
  let draft = $state({ ...app.settings, location: { ...app.settings.location }, rig: { ...app.settings.rig } });
  let step = $state(0);
  let haveLicence = $state(app.settings.callsign !== "" || !app.settings.listenOnly);
  let kind = $state<"sound" | "sdr" | "none">(draft.sdr ? "sdr" : "sound");
  let previewing = $state(false);

  const STEPS = ["You", "Radio", "Channel", "Place", "Ready"];
  const BANDS = [
    { khz: 14103, name: "20 metres", note: "Most active. Best by day, reaches across continents." },
    { khz: 7103, name: "40 metres", note: "Good in the evening and at night, medium distances." },
    { khz: 3603, name: "80 metres", note: "Night-time and short distances, such as inside one country." },
  ];

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
    <span class="brand mono">KAHF</span>
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
      <p class="muted">KAHF stations meet on one frequency, like a meeting place. You can change it any time.</p>
      {#each BANDS as b}
        <button class="pick" class:on={draft.freqKhz === b.khz} onclick={() => (draft.freqKhz = b.khz)}>
          <span class="dot"></span>
          <span><b>{b.name} <span class="mono faint">{(b.khz / 1000).toFixed(3)} MHz</span></b><span class="s">{b.note}</span></span>
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
        <dt>Frequency</dt><dd class="mono">{(draft.freqKhz / 1000).toFixed(3)} MHz</dd>
        <dt>Map</dt><dd>{draft.location.mode === "off" ? "not shared" : draft.location.mode === "area" ? "a circle" : "exact position"}</dd>
      </dl>
      <div class="row opt"><div class="grow"><h3>Connect by itself</h3><p class="muted small">When you send, KAHF reaches the station, sends it and hangs up.</p></div>
        <label class="switch"><input type="checkbox" bind:checked={draft.autoLink} aria-label="Connect by itself" /><i></i></label></div>
      <div class="row opt"><div class="grow"><h3>Choose the speed by itself</h3><p class="muted small">Faster when the signal is strong, slower but surer when it is weak.</p></div>
        <label class="switch"><input type="checkbox" bind:checked={draft.autoMode} aria-label="Automatic speed" /><i></i></label></div>
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
    height: 100%;
    max-width: 780px;
    margin: 0 auto;
    padding: 18px 28px;
    display: flex;
    flex-direction: column;
  }
  .brand {
    font-weight: 800;
    letter-spacing: 0.3em;
    color: var(--accent);
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
  .opt {
    padding: 8px 0;
    border-bottom: 1px solid var(--line);
  }
</style>
