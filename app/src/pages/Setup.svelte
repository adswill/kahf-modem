<script lang="ts">
  import { onMount } from "svelte";
  import { app } from "../lib/state.svelte";
  import { callProblem, normaliseCall, validCall } from "../lib/callsign";
  import { validLat, validLon } from "../lib/geo";
  import type { LocationMode } from "../lib/types";
  import Icon from "../components/Icon.svelte";
  import Spectrum from "../components/Spectrum.svelte";
  import LocationEditor from "../components/LocationEditor.svelte";

  // A copy of the settings that only becomes real when the person finishes.
  let draft = $state({ ...app.settings, location: { ...app.settings.location } });
  let step = $state(0);
  let haveLicence = $state(app.settings.callsign !== "" || !app.settings.listenOnly);
  let audioDevices = $state<string[]>([]);
  let sdrDevices = $state<string[]>([]);
  let radioKind = $state<"sound" | "sdr" | "none">(draft.sdr ? "sdr" : "sound");
  let previewing = $state(false);

  const STEPS = ["Callsign", "Radio", "Frequency", "Location", "Ready"];
  const BANDS = [
    { khz: 14103, name: "20 metres", note: "Most active. Best in the daytime, works across continents." },
    { khz: 7103, name: "40 metres", note: "Good in the evening and at night, medium distances." },
    { khz: 3603, name: "80 metres", note: "Night-time and short distances, such as inside one country." },
  ];

  onMount(async () => {
    audioDevices = await app.engine.listAudioDevices();
    sdrDevices = await app.engine.detectSdr();
  });

  const call = $derived(normaliseCall(draft.callsign));
  const callMsg = $derived(callProblem(draft.callsign));
  const callOk = $derived(validCall(call) && !callMsg);

  const posOk = $derived(validLat(draft.location.lat) && validLon(draft.location.lon));

  const canNext = $derived.by(() => {
    if (step === 0) return haveLicence ? callOk : true;
    if (step === 3) return draft.location.mode === "off" || posOk;
    return true;
  });

  async function preview() {
    previewing = true;
    await app.engine.start($state.snapshot({ ...draft, callsign: call }) as typeof draft);
  }

  async function finish() {
    const loc = {
      mode: draft.location.mode as LocationMode,
      lat: draft.location.mode === "off" ? null : draft.location.lat,
      lon: draft.location.mode === "off" ? null : draft.location.lon,
      radiusKm: draft.location.radiusKm,
    };
    await app.finishSetup({
      callsign: haveLicence ? call : "",
      listenOnly: !haveLicence,
      audioIn: radioKind === "sound" ? draft.audioIn : "",
      audioOut: radioKind === "sound" ? draft.audioOut : "",
      sdr: radioKind === "sdr" ? draft.sdr || sdrDevices[0] || "" : "",
      rigctld: draft.rigctld,
      freqKhz: draft.freqKhz,
      location: loc,
      autoLink: draft.autoLink,
      autoMode: draft.autoMode,
    });
    app.tourOpen = !app.settings.tourDone;
  }

  function next() {
    if (step < STEPS.length - 1) step += 1;
    else finish();
  }
</script>

<main class="setup">
  <header class="top">
    <div class="row">
      <div class="logo" aria-hidden="true">K</div>
      <strong>Set up KAHF</strong>
    </div>
    <button class="btn ghost small" onclick={() => app.go(app.settings.setupDone ? "settings" : "welcome")}>
      Cancel
    </button>
  </header>

  <ol class="steps" aria-label="Progress">
    {#each STEPS as s, i}
      <li class:done={i < step} class:now={i === step}>
        <span class="n">{#if i < step}<Icon name="check" size={14} />{:else}{i + 1}{/if}</span>
        <span class="t">{s}</span>
      </li>
    {/each}
  </ol>

  <section class="body">
    {#if step === 0}
      <h1>Who are you on the air?</h1>
      <p class="lead muted">
        Every radio amateur has a callsign, like <span class="callsign">KA1ABC</span>. It is how other
        stations know it is you.
      </p>
      <div class="stack">
        <button class="choice" class:on={haveLicence} onclick={() => (haveLicence = true)}>
          <span class="dot"></span>
          <span>
            <strong>I have an amateur radio licence</strong>
            <span class="sub">I can transmit and I know my callsign.</span>
          </span>
        </button>
        {#if haveLicence}
          <label class="field">
            Your callsign
            <input
              type="text"
              placeholder="For example KA1ABC"
              autocapitalize="characters"
              spellcheck="false"
              bind:value={draft.callsign}
              oninput={() => (draft.callsign = draft.callsign.toUpperCase())}
            />
            {#if callMsg}<span class="field-error">{callMsg}</span>
            {:else if callOk}<span class="field-ok">Looks good.</span>
            {:else}<span class="help">Letters and numbers, with at least one number.</span>{/if}
          </label>
          <div class="notice warn">
            <Icon name="warn" />
            <span>
              Only transmit if your licence allows it on the frequency you use. KAHF never checks for you.
            </span>
          </div>
        {/if}
        <button class="choice" class:on={!haveLicence} onclick={() => (haveLicence = false)}>
          <span class="dot"></span>
          <span>
            <strong>I only want to listen for now</strong>
            <span class="sub">KAHF will not transmit at all. You can add a callsign later in Settings.</span>
          </span>
        </button>
      </div>
    {:else if step === 1}
      <h1>How is your radio connected?</h1>
      <p class="lead muted">KAHF needs to hear the radio. Pick the way yours is connected.</p>
      <div class="stack">
        <button class="choice" class:on={radioKind === "sound"} onclick={() => (radioKind = "sound")}>
          <span class="dot"></span>
          <span>
            <strong>A sound-card cable or interface</strong>
            <span class="sub">The most common way. The radio's audio goes to this computer's sound input.</span>
          </span>
        </button>
        {#if radioKind === "sound"}
          <div class="grid-2">
            <label class="field">
              Sound in (from the radio)
              <select bind:value={draft.audioIn}>
                <option value="">Computer default</option>
                {#each audioDevices as d}<option value={d}>{d}</option>{/each}
              </select>
            </label>
            <label class="field">
              Sound out (to the radio)
              <select bind:value={draft.audioOut}>
                <option value="">Computer default</option>
                {#each audioDevices as d}<option value={d}>{d}</option>{/each}
              </select>
            </label>
          </div>
        {/if}
        <button class="choice" class:on={radioKind === "sdr"} onclick={() => (radioKind = "sdr")}>
          <span class="dot"></span>
          <span>
            <strong>An SDR (HackRF, RTL-SDR and similar)</strong>
            <span class="sub">A USB radio that connects straight to the computer.</span>
          </span>
        </button>
        {#if radioKind === "sdr"}
          <label class="field">
            Detected receivers
            <select bind:value={draft.sdr}>
              {#each sdrDevices as d}<option value={d}>{d}</option>{/each}
              {#if sdrDevices.length === 0}<option value="">None found. Plug it in and reopen this step.</option>{/if}
            </select>
          </label>
        {/if}
        <button class="choice" class:on={radioKind === "none"} onclick={() => (radioKind = "none")}>
          <span class="dot"></span>
          <span>
            <strong>No radio yet</strong>
            <span class="sub">Skip this. Nothing will be heard until you connect one in Settings.</span>
          </span>
        </button>
        {#if radioKind !== "none"}
          <div class="row">
            <button class="btn" onclick={preview}><Icon name="wave" /> Check that I can hear it</button>
            <span class="muted small">Opens the radio and shows the signal below. Moving lines mean it works.</span>
          </div>
          {#if previewing}
            <Spectrum height={220} compact />
          {/if}
        {/if}
      </div>
    {:else if step === 2}
      <h1>Where do you listen?</h1>
      <p class="lead muted">
        KAHF stations gather on one frequency, like a meeting place. Pick a band. You can change it any time.
      </p>
      <div class="stack">
        {#each BANDS as b}
          <button class="choice" class:on={draft.freqKhz === b.khz} onclick={() => (draft.freqKhz = b.khz)}>
            <span class="dot"></span>
            <span>
              <strong>{b.name} <span class="mono faint">{(b.khz / 1000).toFixed(3)} MHz</span></strong>
              <span class="sub">{b.note}</span>
            </span>
          </button>
        {/each}
        <label class="field">
          Radio control address (optional)
          <input type="text" placeholder="localhost:4532" bind:value={draft.rigctld} />
          <span class="help">If you use Hamlib, KAHF can set the radio's frequency for you. Leave empty if unsure.</span>
        </label>
      </div>
    {:else if step === 3}
      <h1>Do you want to be on the map?</h1>
      <p class="lead muted">
        The public map at kahf.dev shows where KAHF stations are, so people can see who is nearby.
        <strong>Sharing is off unless you turn it on.</strong>
      </p>
      <LocationEditor bind:value={draft.location} />
    {:else}
      <h1>You are ready</h1>
      <p class="lead muted">Here is what KAHF will do. You can change all of it later.</p>
      <div class="stack">
        <div class="card flat summary">
          <div><span class="muted">Callsign</span><strong class="callsign">{haveLicence ? call : "listening only"}</strong></div>
          <div>
            <span class="muted">Radio</span>
            <strong>{radioKind === "sound" ? "Sound card" : radioKind === "sdr" ? "SDR" : "none yet"}</strong>
          </div>
          <div><span class="muted">Frequency</span><strong class="mono">{(draft.freqKhz / 1000).toFixed(3)} MHz</strong></div>
          <div>
            <span class="muted">Map</span>
            <strong>{draft.location.mode === "off" ? "not shared" : draft.location.mode === "area" ? "circle" : "exact"}</strong>
          </div>
        </div>
        <div class="card flat stack-sm">
          <div class="row">
            <div class="grow">
              <h3>Connect by itself</h3>
              <p class="muted small">When you write to someone, KAHF contacts them, sends it, and hangs up. You never press Connect.</p>
            </div>
            <label class="switch"><input type="checkbox" bind:checked={draft.autoLink} aria-label="Connect by itself" /><span></span></label>
          </div>
          <div class="row">
            <div class="grow">
              <h3>Choose the speed by itself</h3>
              <p class="muted small">Faster when the signal is strong, slower but surer when it is weak.</p>
            </div>
            <label class="switch"><input type="checkbox" bind:checked={draft.autoMode} aria-label="Automatic speed" /><span></span></label>
          </div>
        </div>
      </div>
    {/if}
  </section>

  <footer class="foot">
    <button class="btn ghost" disabled={step === 0} onclick={() => (step -= 1)}>Back</button>
    <button class="btn primary big" disabled={!canNext} onclick={next}>
      {step === STEPS.length - 1 ? "Start listening" : "Continue"} <Icon name="arrow" />
    </button>
  </footer>
</main>

<style>
  .setup {
    height: 100%;
    display: flex;
    flex-direction: column;
    max-width: 860px;
    margin: 0 auto;
    padding: 20px 28px 20px;
  }
  .top {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 4px 0 18px;
  }
  .logo {
    width: 32px;
    height: 32px;
    border-radius: 9px;
    background: var(--accent);
    color: var(--accent-ink);
    display: grid;
    place-items: center;
    font-weight: 800;
  }
  .steps {
    list-style: none;
    margin: 0 0 22px;
    padding: 0;
    display: flex;
    gap: 8px;
  }
  .steps li {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--faint);
    font-size: 13px;
    font-weight: 600;
    padding-top: 10px;
    border-top: 3px solid var(--line);
  }
  .steps li.now {
    color: var(--text);
    border-color: var(--accent);
  }
  .steps li.done {
    color: var(--accent);
    border-color: var(--accent);
  }
  .n {
    display: grid;
    place-items: center;
    width: 22px;
    height: 22px;
    border-radius: 50%;
    background: var(--raised);
    font-size: 12px;
  }
  .steps li.done .n,
  .steps li.now .n {
    background: var(--accent);
    color: var(--accent-ink);
  }
  @media (max-width: 700px) {
    .steps .t {
      display: none;
    }
  }
  .body {
    flex: 1;
    overflow: auto;
    display: flex;
    flex-direction: column;
    gap: 16px;
    padding: 6px 4px 20px;
  }
  .lead {
    font-size: 16.5px;
    max-width: 640px;
  }
  .foot {
    display: flex;
    justify-content: space-between;
    padding-top: 14px;
    border-top: 1px solid var(--line-soft);
  }
  .summary {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
    gap: 14px 24px;
  }
  .summary div {
    display: flex;
    flex-direction: column;
  }
</style>
