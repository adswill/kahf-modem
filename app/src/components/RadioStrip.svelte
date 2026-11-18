<script lang="ts">
  // The front panel: always on screen, so the person always knows what the radio is doing.
  import { app } from "../lib/state.svelte";
  import Icon from "./Icon.svelte";

  const khz = $derived(app.settings.freqKhz);
  const whole = $derived(Math.floor(khz / 1000));
  const frac = $derived(String(khz % 1000).padStart(3, "0"));
  const bars = 16;
  const lit = $derived(Math.round(app.signal * bars));
</script>

<header class="strip" data-tour="strip">
  <div class="brand mono">KAHF</div>

  <button class="freq" onclick={() => app.go("radio")} title="Open the Radio screen" aria-label="Frequency {whole}.{frac} megahertz">
    <span class="mhz mono">{whole}.<b>{frac}</b><i> 000</i></span>
    <span class="unit label">MHz</span>
  </button>

  <div class="meter" aria-label="Signal strength">
    {#each Array(bars) as _, i}
      <span class:on={i < lit} class:hot={i >= bars - 4}></span>
    {/each}
  </div>

  <div class="lamps">
    <span class="lamp-group"><span class="lamp" class:good={app.receiving}></span><span class="label">RX</span></span>
    <span class="lamp-group"><span class="lamp" class:bad={app.transmitting} class:blink={app.transmitting}></span><span class="label">TX</span></span>
  </div>

  <div class="status" title={app.status.detail}>
    <span class="lamp {app.status.tone === 'good' ? 'good' : app.status.tone === 'busy' ? 'warn blink' : app.status.tone === 'warn' ? 'bad' : ''}"></span>
    <div>
      <div class="t">{app.status.title}</div>
      <div class="d faint">{app.status.detail}</div>
    </div>
  </div>

  <div class="spacer"></div>

  {#if app.relay.running}
    <button class="relay" onclick={() => app.go("relay")} title="Relay mode is on. Open the relay screen.">
      <Icon name="relay" size={14} /> RELAY
      <span class="mono">{app.relay.forwardedToday}</span>
    </button>
  {/if}
  <button class="btn quiet icon" data-tour="help" onclick={() => app.openHelp()} title="Help" aria-label="Help"><Icon name="help" size={17} /></button>
  <button class="btn quiet icon" data-tour="settings" onclick={() => app.openSettings()} title="Settings" aria-label="Settings"><Icon name="settings" size={17} /></button>
</header>

<style>
  .strip {
    height: 56px;
    flex: none;
    display: flex;
    align-items: center;
    gap: 22px;
    padding: 0 16px 0 18px;
    background: var(--panel);
    border-bottom: 1px solid var(--line);
  }
  .brand {
    font-weight: 800;
    letter-spacing: 0.24em;
    color: var(--accent);
    font-size: 13px;
  }
  .freq {
    display: flex;
    align-items: baseline;
    gap: 6px;
    background: var(--bg);
    border: 1px solid var(--line);
    border-radius: 6px;
    padding: 5px 12px;
    color: var(--text);
    cursor: pointer;
  }
  .freq:hover {
    border-color: var(--line-strong);
  }
  .mhz {
    font-size: 21px;
    letter-spacing: 0.02em;
  }
  .mhz b {
    font-weight: 400;
  }
  .mhz i {
    font-style: normal;
    color: var(--faint);
    font-size: 15px;
  }
  .meter {
    display: flex;
    gap: 2px;
    align-items: flex-end;
    height: 22px;
  }
  .meter span {
    width: 5px;
    height: 100%;
    background: var(--line);
    border-radius: 1px;
  }
  .meter span:nth-child(-n + 6) {
    height: 55%;
  }
  .meter span.on {
    background: var(--good);
  }
  .meter span.on.hot {
    background: var(--accent);
  }
  .lamps {
    display: flex;
    gap: 12px;
  }
  .lamp-group {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .status {
    display: flex;
    align-items: center;
    gap: 9px;
    min-width: 0;
  }
  .t {
    font-weight: 600;
    line-height: 1.15;
  }
  .d {
    font-size: 12px;
    max-width: 280px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .relay {
    display: flex;
    align-items: center;
    gap: 7px;
    border: 1px solid var(--accent);
    color: var(--accent);
    background: var(--accent-soft);
    border-radius: 6px;
    padding: 4px 10px;
    font: inherit;
    font-size: 12px;
    font-weight: 700;
    letter-spacing: 0.08em;
    cursor: pointer;
  }
</style>
