<script lang="ts">
  import { app } from "../lib/state.svelte";
  import { mhz, snrQuality, speedWords } from "../lib/format";
  import type { Settings } from "../lib/types";
  import Spectrum from "../components/Spectrum.svelte";
  import AudioPicker from "../components/AudioPicker.svelte";
  import RigPicker from "../components/RigPicker.svelte";
  import Tip from "../components/Tip.svelte";
  import { BANDS, roleWords } from "../lib/bands";
  import { reachNotice } from "../lib/geo";
  const reach = $derived(reachNotice(app.settings.location.lat, app.settings.location.lon, new Date()));

  // 17 m only: the 12 lanes, lowest frequency first.
  const BAND_TABS = BANDS;

  // The hardware choices are edited here and applied together, because the radio restarts.
  let kind = $state<"sound" | "sdr" | "none">(app.settings.sdr ? "sdr" : app.settings.audioIn || app.settings.audioOut ? "sound" : "sound");
  let audioIn = $state(app.settings.audioIn);
  let audioOut = $state(app.settings.audioOut);
  let sdr = $state(app.settings.sdr);
  let rig = $state({ ...app.settings.rig });
  let applying = $state(false);

  const dirty = $derived(
    audioIn !== app.settings.audioIn || audioOut !== app.settings.audioOut || sdr !== app.settings.sdr ||
      rig.model !== app.settings.rig.model || rig.port !== app.settings.rig.port || rig.baud !== app.settings.rig.baud,
  );

  async function apply() {
    applying = true;
    const patch: Partial<Settings> = {
      audioIn: kind === "sound" ? audioIn : "",
      audioOut: kind === "sound" ? audioOut : "",
      sdr: kind === "sdr" ? sdr : "",
      rig: { ...rig },
    };
    await app.saveSettings(patch);
    const ok = await app.startEngine();
    applying = false;
    if (ok) app.toast("Radio restarted with the new settings.", "good");
  }

  const q = $derived(snrQuality(app.metrics?.ownSnr ?? null));
  const current = $derived(app.metrics?.mode ?? (app.settings.autoMode ? "Automatic" : app.settings.fixedMode));
</script>

<div class="view">
  <div class="main">
    {#if reach}
      <section class="gate {reach.tone === 'good' ? 'off' : 'bench'}" role="status">{reach.text}</section>
    {/if}
    {#if app.txGate.mode === "bench"}
      <section class="gate bench" role="status">
        <b>TEST TABLE ON.</b> This is a bench radio: nothing reaches an antenna. A real radio never uses the test table.
      </section>
    {:else if app.txGate.mode === "off" && app.txGate.text}
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

    <section data-tour="speed">
      <div class="head row">
        <h2>Speed</h2>
        <Tip text="KAHF picks the fastest speed your signal can carry. Slow speeds work in weak signals; fast ones need a clean, strong signal. Automatic is almost always best." />
        <div class="spacer"></div>
        <span class="muted">Automatic</span>
        <label class="switch">
          <input type="checkbox" checked={app.settings.autoMode} onchange={(e) => app.setAutoMode(e.currentTarget.checked)} aria-label="Automatic speed" />
          <i></i>
        </label>
      </div>
      <div class="now row">
        <div>
          <div class="label">Using</div>
          <div class="big">{current}</div>
          {#if app.metrics}<div class="faint small">{speedWords(app.metrics.netBps)}</div>{/if}
        </div>
        <div>
          <div class="label">Signal from them</div>
          <span class="tag {q.tone}">{q.label}</span>
          {#if app.metrics?.ownSnr != null}<span class="faint small mono"> {app.metrics.ownSnr} dB</span>{/if}
        </div>
        <div>
          <div class="label">They hear you</div>
          <span class="mono">{app.metrics?.peerSnr ?? "-"}</span><span class="faint small"> dB</span>
        </div>
      </div>
      {#if !app.settings.autoMode}
        <div class="note warn">You chose the speed. If the signal gets weaker, messages will stop arriving until you pick a slower speed or turn Automatic back on.</div>
      {/if}
      <table>
        <thead><tr><th>Mode</th><th>Needs at least</th><th>Speed</th><th></th></tr></thead>
        <tbody>
          {#each app.modes as m}
            <tr class:on={!app.settings.autoMode && app.settings.fixedMode === m.name}>
              <td class="mono">{m.name}</td>
              <td class="mono">{m.needSnr} dB</td>
              <td class="muted">{m.netBps} bit/s</td>
              <td class="r"><button class="btn sm quiet" onclick={() => app.setFixedMode(m.name)}>Use</button></td>
            </tr>
          {/each}
        </tbody>
      </table>
    </section>
  </div>

  <aside class="side">
    <section>
      <h2>Frequency</h2>
      <div class="row" style="margin-top: 8px">
        <div class="tabs">
          {#each BAND_TABS as b}
            <button class:on={Math.abs(app.settings.freqKhz - b.khz) < 0.001} title="{roleWords(b.role)} {b.name}" onclick={() => app.saveSettings({ freqKhz: b.khz })}>{b.name}</button>
          {/each}
        </div>
        <span class="mono">{mhz(app.settings.freqKhz)}</span>
      </div>
      <p class="faint small" style="margin-top: 6px">KAHF uses 17 metres only (18.111 to 18.120 MHz), in 12 narrow lanes. C1 and C2 are where operators call. R1 to R8 are the relay lanes. X1 and X2 are spare. The frequency shown is the radio's USB dial setting, not the lane centre. Stay on C1 unless your group agreed on another lane.</p>
    </section>

    <section>
      <h2>Transmit level <Tip text="How loud KAHF sends into your radio. Too loud and the radio spreads the signal onto the channels next to it, which spoils reception for everyone on a shared band. Lower is always safer." /></h2>
      <div class="row" style="margin-top: 8px">
        <input type="range" min="10" max="100" step="5" value={app.settings.txLevel}
          oninput={(e) => app.setTxLevel(Number((e.currentTarget as HTMLInputElement).value))} aria-label="Transmit level" />
        <span class="mono">{app.settings.txLevel}%</span>
      </div>
      <p class="faint small" style="margin-top: 6px">
        Press the button, then watch your radio's ALC meter. It should barely move. If it moves a lot, lower the level here (or the
        sound-card volume) until it does not.
      </p>
      <button class="btn sm" style="margin-top: 6px" disabled={app.settings.listenOnly} onclick={() => app.testTone()}>Send a 5 second test tone</button>
      <p class="faint small" style="margin-top: 4px">This transmits. Use a dummy load or make sure the channel is free.</p>
    </section>

    <section>
      <h2>How the radio is heard</h2>
      <div style="margin-top: 8px"><AudioPicker bind:kind bind:audioIn bind:audioOut bind:sdr /></div>
    </section>

    <section>
      <h2>Radio control <Tip text="Optional. KAHF can read and set your radio's frequency through Hamlib, so you never touch the dial. Sound still comes through the sound card." /></h2>
      <div style="margin-top: 8px"><RigPicker bind:value={rig} /></div>
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
  .now {
    gap: 40px;
    padding: 10px 0 14px;
  }
  .big {
    font-size: 24px;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    margin-top: 10px;
  }
  th {
    text-align: left;
    font-size: 12px;
    color: var(--muted);
    font-weight: 600;
    padding: 4px 8px;
    background: linear-gradient(var(--bar-top), var(--bar-bot));
    border: 1px solid var(--line);
    border-width: 1px 1px 1px 0;
  }
  td {
    padding: 6px 8px;
    border-bottom: 1px solid var(--line);
  }
  td.r {
    text-align: right;
  }
  tr.on td {
    background: var(--accent-soft);
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
