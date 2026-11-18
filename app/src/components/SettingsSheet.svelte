<script lang="ts">
  import { onMount } from "svelte";
  import { app, type SettingsTab } from "../lib/state.svelte";
  import { callProblem, normaliseCall, validCall } from "../lib/callsign";
  import { validLat, validLon } from "../lib/geo";
  import LocationEditor from "./LocationEditor.svelte";
  import Icon from "./Icon.svelte";

  const TABS: { id: SettingsTab; label: string }[] = [
    { id: "station", label: "Your station" },
    { id: "location", label: "Location" },
    { id: "sending", label: "Sending" },
    { id: "look", label: "Look" },
    { id: "about", label: "About" },
  ];

  let call = $state(app.settings.callsign);
  let loc = $state({ ...app.settings.location });
  let mic = $state(app.settings.micIn);
  let devices = $state<string[]>([]);
  let confirmReset = $state(false);

  onMount(async () => (devices = await app.engine.listAudioDevices()));

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
    app.toast("Location saved.", "good");
    await app.shareOnMap();
  }
  function erase() {
    localStorage.clear();
    location.reload();
  }
  const close = () => (app.settingsOpen = false);
</script>

<div class="scrim" role="presentation" onclick={close}></div>
<div class="sheet" role="dialog" aria-label="Settings">
  <header class="row">
    <h1>Settings</h1>
    <div class="spacer"></div>
    <button class="btn quiet icon" onclick={close} aria-label="Close"><Icon name="close" size={17} /></button>
  </header>
  <div class="body">
    <nav>
      {#each TABS as t}
        <button class:on={app.settingsTab === t.id} onclick={() => (app.settingsTab = t.id)}>{t.label}</button>
      {/each}
    </nav>
    <div class="pane">
      {#if app.settingsTab === "station"}
        <h2>Your station</h2>
        <label class="field" style="max-width: 340px">
          <span class="t">Callsign</span>
          <input type="text" bind:value={call} oninput={() => (call = call.toUpperCase())} placeholder="For example KA1ABC" />
          {#if callMsg}<span class="err">{callMsg}</span>{/if}
          <span class="hint">Your name on the air. Other stations see it on everything you send.</span>
        </label>
        <div><button class="btn primary" disabled={!callOk || normaliseCall(call) === app.settings.callsign} onclick={saveCall}>Save</button></div>
      {:else if app.settingsTab === "location"}
        <h2>Location on the map</h2>
        <LocationEditor bind:value={loc} />
        <div><button class="btn primary" disabled={!locOk} onclick={saveLocation}>Save location</button></div>
      {:else if app.settingsTab === "sending"}
        <h2>Sending</h2>
        {#each [
          { key: "listen", t: "Listen only", d: "KAHF receives but never transmits. Safe while you are learning or have no licence.", on: app.settings.listenOnly, set: (v: boolean) => app.setListenOnly(v) },
          { key: "link", t: "Connect by itself", d: "When you send, KAHF reaches the station, sends it, and hangs up after it goes quiet.", on: app.settings.autoLink, set: (v: boolean) => app.setAutoLink(v) },
          { key: "speed", t: "Choose the speed by itself", d: "Recommended. KAHF uses the fastest speed the signal can carry.", on: app.settings.autoMode, set: (v: boolean) => app.setAutoMode(v) },
        ] as o (o.key)}
          <div class="row opt">
            <div class="grow"><h3>{o.t}</h3><p class="muted small">{o.d}</p></div>
            <label class="switch"><input type="checkbox" checked={o.on} onchange={(e) => o.set(e.currentTarget.checked)} aria-label={o.t} /><i></i></label>
          </div>
        {/each}
        <label class="field" style="max-width: 380px">
          <span class="t">Microphone for voice notes</span>
          <select bind:value={mic} onchange={() => app.saveSettings({ micIn: mic })}>
            <option value="">Computer default</option>
            {#each devices as d}<option value={d}>{d}</option>{/each}
          </select>
          <span class="hint">Pick your headset or computer microphone, not the radio interface.</span>
        </label>
      {:else if app.settingsTab === "look"}
        <h2>Look</h2>
        <div class="tabs">
          <button class:on={app.settings.theme === "dark"} onclick={() => app.saveSettings({ theme: "dark" })}>Dark</button>
          <button class:on={app.settings.theme === "light"} onclick={() => app.saveSettings({ theme: "light" })}>Light</button>
        </div>
      {:else}
        <h2>About</h2>
        <p class="muted">KAHF 0.2. Radio messages without the internet.</p>
        <div class="row wrap">
          <button class="btn" onclick={() => { close(); app.tourOpen = true; }}>Take the tour again</button>
          <button class="btn" onclick={() => { close(); app.go("setup"); }}>Run setup again</button>
        </div>
        <hr class="rule" />
        {#if !confirmReset}
          <div><button class="btn danger sm" onclick={() => (confirmReset = true)}>Erase my settings and messages</button></div>
        {:else}
          <div class="note bad">
            <span>This deletes your settings, chats, mail and station list from this computer.</span>
            <button class="btn danger sm" onclick={erase}>Yes, erase</button>
            <button class="btn sm quiet" onclick={() => (confirmReset = false)}>Cancel</button>
          </div>
        {/if}
      {/if}
    </div>
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    background: var(--scrim);
    z-index: 60;
  }
  .sheet {
    position: fixed;
    z-index: 61;
    left: 50%;
    top: 50%;
    transform: translate(-50%, -50%);
    width: min(760px, 94vw);
    height: min(560px, 90vh);
    background: var(--panel);
    border: 1px solid var(--line-strong);
    border-radius: 8px;
    box-shadow: var(--shadow);
    display: flex;
    flex-direction: column;
  }
  header {
    padding: 12px 18px;
    border-bottom: 1px solid var(--line);
  }
  .body {
    flex: 1;
    display: grid;
    grid-template-columns: 170px 1fr;
    min-height: 0;
  }
  nav {
    border-right: 1px solid var(--line);
    padding: 8px 0;
    display: flex;
    flex-direction: column;
  }
  nav button {
    text-align: left;
    background: none;
    border: 0;
    border-left: 2px solid transparent;
    color: var(--muted);
    font: inherit;
    font-weight: 550;
    padding: 9px 16px;
    cursor: pointer;
  }
  nav button:hover {
    color: var(--text);
    background: var(--hover);
  }
  nav button.on {
    color: var(--text);
    background: var(--accent-soft);
    border-left-color: var(--accent);
  }
  .pane {
    overflow: auto;
    padding: 20px 24px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .opt {
    padding: 10px 0;
    border-bottom: 1px solid var(--line);
  }
</style>
