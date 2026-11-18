<script lang="ts">
  import { onMount } from "svelte";
  import { app } from "../lib/state.svelte";
  import { mhz, modeExplained, snrQuality, speedWords } from "../lib/format";
  import Icon from "../components/Icon.svelte";
  import Spectrum from "../components/Spectrum.svelte";

  let audioDevices = $state<string[]>([]);
  const BANDS = [
    { khz: 14103, name: "20 m" },
    { khz: 7103, name: "40 m" },
    { khz: 3603, name: "80 m" },
  ];

  onMount(async () => (audioDevices = await app.engine.listAudioDevices()));

  const q = $derived(snrQuality(app.metrics?.ownSnr ?? null));
  const current = $derived(app.metrics?.mode ?? (app.settings.autoMode ? "Automatic" : app.settings.fixedMode));
  const linkedNeed = $derived(app.modes.find((m) => m.name === app.metrics?.mode)?.needSnr ?? null);
</script>

<div class="page">
  <header class="head">
    <div>
      <h1>Radio</h1>
      <p class="muted">The signal your radio hears, the speed KAHF is using, and the radio settings. Most people never need to change anything here.</p>
    </div>
  </header>

  <section class="card" data-tour="spectrum">
    <div class="card-title">
      <div>
        <h2>The band right now</h2>
        <p class="muted small">Tuned to <strong class="mono">{mhz(app.settings.freqKhz)}</strong>. Lines that move mean your radio is working.</p>
      </div>
      {#if app.transmitting}<span class="chip bad">Transmitting</span>{/if}
    </div>
    <Spectrum height={360} />
    <div class="notice" style="margin-top: 14px">
      <Icon name="info" />
      <span>
        <strong>One message uses one channel.</strong> A KAHF transmission is about 2.4 kHz wide, the shaded band, and
        always the same width whatever the speed. That keeps the rest of the band free for other stations.
      </span>
    </div>
  </section>

  <section class="card" data-tour="speed">
    <div class="card-title">
      <div>
        <h2>Speed</h2>
        <p class="muted small">
          KAHF picks the fastest speed your signal can carry. Weak signals need slow, sure speeds.
        </p>
      </div>
      <div class="row">
        <span class="muted">Automatic</span>
        <label class="switch">
          <input type="checkbox" checked={app.settings.autoMode} onchange={(e) => app.setAutoMode(e.currentTarget.checked)} aria-label="Automatic speed" />
          <span></span>
        </label>
      </div>
    </div>

    <div class="now">
      <div>
        <div class="faint small">Using now</div>
        <strong class="big">{current}</strong>
        {#if app.metrics}<div class="muted small">{speedWords(app.metrics.netBps)}</div>{/if}
      </div>
      <div>
        <div class="faint small">Signal from the other station</div>
        <span class="chip {q.tone}">{q.label}</span>
        {#if app.metrics?.ownSnr != null}<span class="muted small"> {app.metrics.ownSnr} dB</span>{/if}
      </div>
    </div>

    {#if !app.settings.autoMode}
      <div class="notice warn">
        <Icon name="warn" />
        <span>You chose the speed yourself. If a signal gets weaker, messages will stop arriving until you pick a slower one or turn Automatic back on.</span>
      </div>
    {/if}

    <details class="modes" open={!app.settings.autoMode}>
      <summary>All speeds explained</summary>
      <table>
        <thead>
          <tr><th>Mode</th><th>Needs a signal of at least</th><th>Speed</th><th></th></tr>
        </thead>
        <tbody>
          {#each app.modes as m}
            <tr class:on={!app.settings.autoMode && app.settings.fixedMode === m.name}>
              <td class="mono"><strong>{m.name}</strong></td>
              <td>{m.needSnr} dB <span class="faint small">{modeExplained(m.name)}</span></td>
              <td class="muted">{m.netBps} bit/s</td>
              <td class="right">
                <button class="btn small" onclick={() => app.setFixedMode(m.name)}>Use this</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
      <p class="faint small">
        The slow modes can be read when the signal is weaker than the noise. The fast modes need a clean, strong signal.
        Speeds are about the information that arrives, measured in a full burst of 8 messages.
      </p>
    </details>
  </section>

  <div class="grid-2">
    <section class="card">
      <h2>Frequency</h2>
      <p class="muted small" style="margin-bottom: 12px">All KAHF stations meet on the same frequency. Change it only if your group agreed on another.</p>
      <div class="seg">
        {#each BANDS as b}
          <button class:on={app.settings.freqKhz === b.khz} onclick={() => app.saveSettings({ freqKhz: b.khz })}>{b.name}</button>
        {/each}
      </div>
      <p class="mono" style="margin-top: 12px; font-size: 22px">{mhz(app.settings.freqKhz)}</p>
    </section>

    <section class="card">
      <h2>Sound</h2>
      <div class="stack-sm" style="margin-top: 10px">
        <label class="field">
          From the radio
          <select value={app.settings.audioIn} onchange={(e) => app.saveSettings({ audioIn: e.currentTarget.value })}>
            <option value="">Computer default</option>
            {#each audioDevices as d}<option value={d}>{d}</option>{/each}
          </select>
        </label>
        <label class="field">
          To the radio
          <select value={app.settings.audioOut} onchange={(e) => app.saveSettings({ audioOut: e.currentTarget.value })}>
            <option value="">Computer default</option>
            {#each audioDevices as d}<option value={d}>{d}</option>{/each}
          </select>
        </label>
        <p class="faint small">Changes apply the next time KAHF starts.</p>
      </div>
    </section>
  </div>
</div>

<style>
  .now {
    display: flex;
    gap: 40px;
    flex-wrap: wrap;
    padding: 6px 0 14px;
  }
  .big {
    font-size: 26px;
    display: block;
  }
  .modes {
    margin-top: 14px;
  }
  summary {
    cursor: pointer;
    font-weight: 600;
    color: var(--accent);
    padding: 6px 0;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    margin-top: 8px;
  }
  th {
    text-align: left;
    font-size: 12.5px;
    color: var(--faint);
    padding: 6px 10px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  td {
    padding: 9px 10px;
    border-top: 1px solid var(--line-soft);
  }
  td.right {
    text-align: right;
  }
  tr.on td {
    background: var(--accent-soft);
  }
</style>
