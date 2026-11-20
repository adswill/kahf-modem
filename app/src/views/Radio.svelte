<script lang="ts">
  import { app } from "../lib/state.svelte";
  import { mhz } from "../lib/format";
  import type { Settings } from "../lib/types";
  import Spectrum from "../components/Spectrum.svelte";
  import AudioPicker from "../components/AudioPicker.svelte";
  import RigPicker from "../components/RigPicker.svelte";
  import Tip from "../components/Tip.svelte";
  import { BANDS } from "../lib/bands";
  import { reachNotice } from "../lib/geo";
  const reach = $derived(reachNotice(app.settings.location.lat, app.settings.location.lon, new Date()));

  // The hardware choices are edited here and applied together, because the radio restarts.
  let kind = $state<"sound" | "sdr" | "none">(app.settings.sdr ? "sdr" : "sound");
  let audioIn = $state(app.settings.audioIn);
  let audioOut = $state(app.settings.audioOut);
  let sdr = $state(app.settings.sdr);
  let rig = $state({ ...app.settings.rig });
  let rigctld = $state(app.settings.rigctld);
  let applying = $state(false);

  const dirty = $derived(
    audioIn !== app.settings.audioIn || audioOut !== app.settings.audioOut || sdr !== app.settings.sdr ||
      rigctld.trim() !== app.settings.rigctld ||
      rig.model !== app.settings.rig.model || rig.port !== app.settings.rig.port || rig.baud !== app.settings.rig.baud,
  );

  async function apply() {
    applying = true;
    const patch: Partial<Settings> = {
      audioIn: kind === "sound" ? audioIn : "",
      audioOut: kind === "sound" ? audioOut : "",
      sdr: kind === "sdr" ? sdr : "",
      rig: { ...rig },
      rigctld: rigctld.trim(),
    };
    await app.saveSettings(patch);
    const ok = await app.startEngine();
    applying = false;
    if (ok) app.toast("Radio restarted with the new settings.", "good");
  }
</script>

<div class="view">
  <div class="main">
    {#if reach}
      <section class="gate {reach.tone === 'good' ? 'off' : 'bench'}" role="status">{reach.text}</section>
    {/if}
    {#if app.txGate.mode === "off" && app.txGate.text}
      <section class="gate off" role="status">{app.txGate.text}</section>
    {/if}
    <section>
      <div class="head row">
        <h1>The channel</h1>
        <span class="faint">{mhz(app.settings.freqKhz)}</span>
        <div class="spacer"></div>
        {#if app.transmitting}<span class="tag bad">TX</span>{:else if app.receiving}<span class="tag good">RX</span>{/if}
      </div>
      <div data-tour="spectrum"><Spectrum height={330} /></div>
    </section>
  </div>

  <aside class="side">
    <section>
      <h2>Channel</h2>
      <div class="row" style="margin-top: 8px">
        <div class="tabs">
          {#each BANDS as b}
            <button class:on={Math.abs(app.settings.freqKhz - b.khz) < 0.001} title="17 m, {b.name}" onclick={() => app.saveSettings({ freqKhz: b.khz })}>{b.name}</button>
          {/each}
        </div>
        <span class="mono">{mhz(app.settings.freqKhz)}</span>
      </div>
      <p class="faint small" style="margin-top: 6px">KAHF uses 17 metres only (18.111 to 18.120 MHz), in three channels, each 2.7 kHz wide. Tune your radio to the frequency shown. A relay works on all three.</p>
    </section>

    <section>
      <h2>How the radio is heard</h2>
      <div style="margin-top: 8px"><AudioPicker bind:kind bind:audioIn bind:audioOut bind:sdr /></div>
    </section>

    <section>
      <h2>Radio control <Tip text="Optional. KAHF can read and set your radio's frequency through Hamlib, so you never touch the dial. Sound still comes through the sound card." /></h2>
      <div style="margin-top: 8px"><RigPicker bind:value={rig} bind:address={rigctld} /></div>
    </section>

    {#if dirty}
      <div class="apply">
        <span class="small">Changes are not applied yet.</span>
        <button class="btn primary" onclick={apply} disabled={applying}>{applying ? "Restarting…" : "Apply and restart the radio"}</button>
      </div>
    {/if}
  </aside>
</div>

<style>
  .gate { padding: 10px 12px; border-radius: 8px; border: 1px solid var(--border, #888); }
  .gate.bench { border-color: #d29922; background: rgba(210, 153, 34, 0.14); }
  .gate.off { border-color: #8b949e; background: rgba(139, 148, 158, 0.12); }
  .view {
    height: 100%;
    display: grid;
    grid-template-columns: 1fr 380px;
    min-height: 0;
  }
  .main,
  .side {
    overflow: auto;
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .side {
    border-left: 1px solid var(--line-strong);
    position: relative;
  }
  .main section,
  .side section {
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 6px;
    padding: 14px 16px;
  }
  /* Side groups carry their caption on the frame, like a group box, instead of a heading inside. */
  .side section {
    position: relative;
    padding-top: 20px;
    margin-top: 4px;
  }
  .side section > h2:first-child {
    position: absolute;
    top: -10px;
    left: 10px;
    padding: 0 7px;
    font-size: var(--fs-md);
    background: var(--bg);
    color: var(--text);
  }
  .head {
    margin-bottom: 10px;
  }
  .apply {
    position: sticky;
    bottom: -20px;
    margin: 0 -24px -20px;
    padding: 12px 24px;
    background: var(--panel-2);
    border-top: 1px solid var(--accent);
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }
</style>
