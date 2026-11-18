<script lang="ts">
  import { app } from "../lib/state.svelte";
  import { callProblem, normaliseCall, validCall } from "../lib/callsign";
  import { validLat, validLon } from "../lib/geo";
  import LocationEditor from "../components/LocationEditor.svelte";
  import Icon from "../components/Icon.svelte";

  let call = $state(app.settings.callsign);
  let loc = $state({ ...app.settings.location });
  let rig = $state(app.settings.rigctld);
  let licence = $state(false);
  let terms = $state(false);
  let confirmReset = $state(false);

  const callMsg = $derived(callProblem(call));
  const callOk = $derived(validCall(normaliseCall(call)));
  const locOk = $derived(loc.mode === "off" || (validLat(loc.lat) && validLon(loc.lon)));

  async function saveCall() {
    if (!callOk) return;
    await app.saveSettings({ callsign: normaliseCall(call), listenOnly: false });
    app.toast("Callsign saved.", "good");
  }
  async function saveLocation() {
    if (!locOk) return;
    await app.saveSettings({ location: { ...loc, lat: loc.mode === "off" ? null : loc.lat, lon: loc.mode === "off" ? null : loc.lon } });
    app.toast(loc.mode === "off" ? "You are not shared on the map." : "Location saved.", "good");
  }
  async function reset() {
    localStorage.clear();
    location.reload();
  }
</script>

<div class="page">
  <header class="head">
    <div>
      <h1>Settings</h1>
      <p class="muted">Everything here has a sensible default. Change only what you need.</p>
    </div>
  </header>

  <section class="card">
    <h2>Your station</h2>
    <div class="row wrap" style="margin-top: 14px">
      <label class="field grow" style="max-width: 360px">
        Callsign
        <input type="text" bind:value={call} oninput={() => (call = call.toUpperCase())} placeholder="For example KA1ABC" />
        {#if callMsg}<span class="field-error">{callMsg}</span>{/if}
      </label>
      <button class="btn primary" disabled={!callOk || normaliseCall(call) === app.settings.callsign} onclick={saveCall}>Save</button>
    </div>
  </section>

  <section class="card">
    <h2>Sending</h2>
    <div class="stack" style="margin-top: 14px">
      <div class="row">
        <div class="grow">
          <h3>Listen only</h3>
          <p class="muted small">KAHF receives but never transmits. Safe while you are learning or have no licence.</p>
        </div>
        <label class="switch">
          <input type="checkbox" checked={app.settings.listenOnly} onchange={(e) => app.setListenOnly(e.currentTarget.checked)} aria-label="Listen only" />
          <span></span>
        </label>
      </div>
      <div class="row">
        <div class="grow">
          <h3>Connect by itself</h3>
          <p class="muted small">When you send a message, KAHF calls the station, sends it, and hangs up after it goes quiet.</p>
        </div>
        <label class="switch">
          <input type="checkbox" checked={app.settings.autoLink} onchange={(e) => app.setAutoLink(e.currentTarget.checked)} aria-label="Connect by itself" />
          <span></span>
        </label>
      </div>
      <div class="row">
        <div class="grow">
          <h3>Choose the speed by itself</h3>
          <p class="muted small">Recommended. KAHF uses the fastest speed the signal can carry.</p>
        </div>
        <label class="switch">
          <input type="checkbox" checked={app.settings.autoMode} onchange={(e) => app.setAutoMode(e.currentTarget.checked)} aria-label="Automatic speed" />
          <span></span>
        </label>
      </div>
    </div>
  </section>

  <section class="card" data-tour="location">
    <h2>Location on the map</h2>
    <p class="muted small" style="margin: 4px 0 14px">Choose what the public map at kahf.dev may show about you.</p>
    <LocationEditor bind:value={loc} />
    <div class="row" style="margin-top: 16px">
      <button class="btn primary" disabled={!locOk} onclick={saveLocation}>Save location</button>
    </div>
  </section>

  <section class="card">
    <h2>Radio control</h2>
    <p class="muted small" style="margin: 4px 0 14px">
      Optional. If you run Hamlib's rigctld, KAHF can set your radio's frequency for you. Sound devices are on the Radio page.
    </p>
    <div class="row wrap">
      <label class="field grow" style="max-width: 360px">
        Address
        <input type="text" placeholder="localhost:4532" bind:value={rig} />
      </label>
      <button class="btn" onclick={() => app.saveSettings({ rigctld: rig })}>Save</button>
    </div>
  </section>

  <section class="card">
    <h2>Help the network: be a relay</h2>
    <p class="muted" style="margin: 4px 0 14px">
      A relay passes messages between stations that cannot hear each other. It helps everyone, but it
      means your station transmits by itself for others, so it must stay on and be attended.
    </p>
    <div class="stack-sm">
      <label class="check"><input type="checkbox" bind:checked={licence} /> My licence allows unattended or automatic operation.</label>
      <label class="check"><input type="checkbox" bind:checked={terms} /> I accept the relay terms: I will keep the station running as agreed.</label>
      <div class="row">
        <div class="grow"><strong>Act as a relay</strong></div>
        <label class="switch">
          <input
            type="checkbox"
            checked={app.settings.relay}
            disabled={!app.settings.relay && !(licence && terms)}
            onchange={(e) => app.setRelay(e.currentTarget.checked, licence, terms)}
            aria-label="Act as a relay"
          />
          <span></span>
        </label>
      </div>
    </div>
  </section>

  <section class="card">
    <h2>Appearance</h2>
    <div class="seg" style="margin-top: 12px">
      <button class:on={app.settings.theme === "dark"} onclick={() => app.saveSettings({ theme: "dark" })}><Icon name="moon" size={15} /> Dark</button>
      <button class:on={app.settings.theme === "light"} onclick={() => app.saveSettings({ theme: "light" })}><Icon name="sun" size={15} /> Light</button>
    </div>
  </section>

  <section class="card">
    <h2>About and help</h2>
    <div class="row wrap" style="margin-top: 12px">
      <button class="btn" onclick={() => { app.tourOpen = true; }}><Icon name="play" size={15} /> Take the tour again</button>
      <button class="btn" onclick={() => app.go("setup")}>Run setup again</button>
      <button class="btn" onclick={() => app.go("welcome")}>Show the welcome screen</button>
    </div>
    <p class="faint small" style="margin-top: 14px">KAHF 0.2. Radio messages without the internet.</p>
    <div style="margin-top: 14px">
      {#if !confirmReset}
        <button class="btn danger small" onclick={() => (confirmReset = true)}>Erase my settings and messages</button>
      {:else}
        <div class="notice bad">
          <Icon name="warn" />
          <span>This deletes your settings, conversations and station list from this computer.</span>
          <button class="btn danger small" onclick={reset}>Yes, erase</button>
          <button class="btn small ghost" onclick={() => (confirmReset = false)}>Cancel</button>
        </div>
      {/if}
    </div>
  </section>
</div>

<style>
  .check {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    font-size: 14px;
  }
  .check input {
    margin-top: 4px;
  }
</style>
