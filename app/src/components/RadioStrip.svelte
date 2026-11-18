<script lang="ts">
  // The front panel: always on screen, so the person always knows what the radio is doing.
  // The dial in the middle is the one place to change channel without leaving the screen.
  import { app } from "../lib/state.svelte";
  import { BANDS, bandIndex, roleWords } from "../lib/bands";
  import Icon from "./Icon.svelte";

  const khz = $derived(app.settings.freqKhz);
  const current = $derived(bandIndex(khz));
  // The dial reads in whole hertz: 18.11344 MHz. Lane dials are multiples of 10 Hz, so five decimals say it all.
  const hz = $derived(Math.round(khz * 1000));
  const whole = $derived(Math.floor(hz / 1_000_000));
  const frac = $derived(String(hz % 1_000_000).padStart(6, "0").slice(0, 5));
  // Digits of the readout, each its own rolling column. The tens place stays blank below 10 MHz.
  const digits = $derived([Math.floor(whole / 10), whole % 10, ...frac.split("").map(Number)]);
  const bars = 16;
  const lit = $derived(Math.round(app.signal * bars));

  // Dragging previews a channel; letting go tunes to it. Never while the radio is sending.
  let preview = $state<number | null>(null);
  let ruler = $state<HTMLElement | null>(null);
  const locked = $derived(app.transmitting);
  const needle = $derived(preview ?? (current >= 0 ? current : 5));

  function indexAt(clientX: number): number {
    if (!ruler) return needle;
    const r = ruler.getBoundingClientRect();
    const f = Math.min(0.999, Math.max(0, (clientX - r.left) / r.width));
    return Math.floor(f * BANDS.length);
  }
  function down(e: PointerEvent) {
    if (locked) return;
    try {
      (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    } catch {
      // A pointer that cannot be captured still drags fine while it stays over the dial.
    }
    preview = indexAt(e.clientX);
  }
  function move(e: PointerEvent) {
    if (preview !== null) preview = indexAt(e.clientX);
  }
  function up() {
    if (preview !== null && preview !== current) tune(preview);
    preview = null;
  }
  function tune(i: number) {
    if (locked || i < 0 || i >= BANDS.length || i === current) return;
    app.saveSettings({ freqKhz: BANDS[i].khz });
  }
  function key(e: KeyboardEvent) {
    const at = current >= 0 ? current : 5;
    if (e.key === "ArrowLeft" || e.key === "ArrowDown") tune(at - 1);
    else if (e.key === "ArrowRight" || e.key === "ArrowUp") tune(at + 1);
    else return;
    e.preventDefault();
  }
  // The wheel tunes only while the dial has keyboard focus, so scrolling past it never retunes.
  function wheel(e: WheelEvent) {
    if (document.activeElement !== e.currentTarget) return;
    e.preventDefault();
    tune((current >= 0 ? current : 5) + (e.deltaY + e.deltaX > 0 ? 1 : -1));
  }
</script>

<header class="strip" data-tour="strip">
  <div class="brand">KAHF</div>

  <div class="dial" class:locked>
    <div class="readout" role="img" aria-label="Frequency {whole}.{frac} megahertz">
      {#each digits as d, i}
        {#if i === 2}<span class="point">.</span>{/if}
        <span class="dg" class:blank={i === 0 && d === 0}><span class="stack" style:--d={d}>{#each Array(10) as _, n}<span>{n}</span>{/each}</span></span>
      {/each}
      <span class="unit">MHz</span>
    </div>
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div
      class="ruler"
      bind:this={ruler}
      role="slider"
      tabindex="0"
      aria-label="17 m lane"
      aria-valuemin={0}
      aria-valuemax={BANDS.length - 1}
      aria-valuenow={needle}
      aria-valuetext="17 m lane {BANDS[needle].name}"
      aria-disabled={locked}
      title={locked ? "The radio is sending. Wait before changing channel." : "Drag, click or use the arrow keys to change channel"}
      onpointerdown={down}
      onpointermove={move}
      onpointerup={up}
      onpointercancel={() => (preview = null)}
      onlostpointercapture={() => (preview = null)}
      onkeydown={key}
      onwheel={wheel}
    >
      {#each BANDS as b, i}
        <span class="tick" class:on={i === needle} title="17 m {roleWords(b.role)} {b.name}"><i></i><b>{b.name}</b></span>
      {/each}
      <span class="needle" style:--at={needle} class:drag={preview !== null}></span>
    </div>
  </div>

  <div class="meter" role="img" aria-label="Signal strength {lit} of {bars}">
    {#each Array(bars) as _, i}
      <span class:on={i < lit} class:hot={i >= bars - 4}></span>
    {/each}
  </div>

  <div class="pills">
    <span class="pill rx" class:on={app.receiving}>RX</span>
    <span class="pill tx" class:on={app.transmitting}>TX</span>
  </div>

  <div class="status" title={app.status.detail}>
    <div class="t">{app.status.title}</div>
    <div class="d faint">{app.status.detail}</div>
  </div>

  <div class="spacer"></div>

  {#if app.relay.running}
    <button class="relay" onclick={() => app.go("relay")} title="Relay mode is on. Open the relay screen.">
      <Icon name="relay" size={14} /> Relay
      <span class="mono">{app.relay.forwardedToday}</span>
    </button>
  {/if}
  <button class="btn quiet icon" data-tour="help" onclick={() => app.openHelp()} title="Help" aria-label="Help"><Icon name="help" size={17} /></button>
  <button class="btn quiet icon" data-tour="settings" onclick={() => app.openSettings()} title="Settings" aria-label="Settings"><Icon name="settings" size={17} /></button>
</header>

<style>
  .strip {
    height: 68px;
    flex: none;
    display: flex;
    align-items: center;
    gap: 22px;
    padding: 0 12px 0 14px;
    background: linear-gradient(var(--bar-top), var(--bar-bot));
    border-bottom: 1px solid var(--line-strong);
  }
  .brand {
    font-weight: 700;
    letter-spacing: 0.1em;
    color: var(--title-ink);
    font-size: 13px;
    padding: 4px 14px;
    border-radius: 5px;
    background: linear-gradient(var(--title-top), var(--title-bot));
    border: 1px solid var(--edge);
  }

  /* ---- the dial ---- */
  .dial {
    display: flex;
    flex-direction: column;
    gap: 3px;
    padding: 4px 14px 5px;
    min-width: 190px;
    background: var(--panel);
    border: 1px solid var(--line-strong);
    border-radius: var(--radius);
    box-shadow: var(--sunk);
  }
  .readout {
    display: flex;
    align-items: baseline;
    font-family: Consolas, "Lucida Console", Menlo, monospace;
    font-size: 24px;
    line-height: 1;
    font-variant-numeric: tabular-nums;
    color: var(--text);
  }
  .dg {
    display: inline-block;
    height: 1em;
    width: 0.6em;
    overflow: hidden;
    line-height: 1;
  }
  .dg.blank {
    opacity: 0;
  }
  .stack {
    display: flex;
    flex-direction: column;
    transform: translateY(calc(var(--d) * -1em));
    transition: transform 0.38s cubic-bezier(0.16, 1, 0.3, 1);
  }
  .stack span {
    height: 1em;
    text-align: center;
  }
  .point {
    width: 0.4em;
    text-align: center;
  }
  .unit {
    margin-left: 8px;
    font-family: "Segoe UI", Tahoma, "Lucida Grande", system-ui, sans-serif;
    font-size: 11.5px;
    color: var(--faint);
  }
  .ruler {
    position: relative;
    display: grid;
    grid-template-columns: repeat(12, 1fr);
    padding-top: 6px;
    border-top: 1px solid var(--line);
    cursor: ew-resize;
    touch-action: none;
    user-select: none;
  }
  .dial.locked .ruler {
    cursor: not-allowed;
    opacity: 0.55;
  }
  .tick {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 1px;
    color: var(--faint);
    font-size: 10px;
    line-height: 1.1;
  }
  .tick i {
    width: 1px;
    height: 5px;
    background: var(--line-strong);
  }
  .tick.on {
    color: var(--text);
    font-weight: 600;
  }
  /* One lane of 12 wide, so it can slide by whole cells; the mark is its centre line. */
  .needle {
    position: absolute;
    top: 0;
    left: 0;
    width: 8.3333%;
    height: 11px;
    display: flex;
    justify-content: center;
    pointer-events: none;
    transform: translateX(calc(var(--at) * 100%));
    transition: transform 0.32s cubic-bezier(0.16, 1, 0.3, 1);
  }
  .needle::after {
    content: "";
    width: 2px;
    height: 100%;
    border-radius: 1px;
    background: var(--accent);
  }
  .needle.drag {
    transition-duration: 0.12s;
  }

  /* ---- signal and lamps ---- */
  .meter {
    display: flex;
    gap: 3px;
    align-items: flex-end;
    height: 30px;
  }
  .meter span {
    width: 6px;
    height: 100%;
    background: var(--line);
    border-radius: 2px;
    transition: background-color 0.12s;
  }
  .meter span:nth-child(-n + 6) {
    height: 55%;
  }
  .meter span.on {
    background: var(--good);
  }
  .meter span.on.hot {
    background: var(--hot);
  }
  .pills {
    display: flex;
    gap: 6px;
  }
  .pill {
    min-width: 38px;
    padding: 3px 10px;
    text-align: center;
    font-size: 12px;
    font-weight: 700;
    letter-spacing: 0.04em;
    border-radius: 4px;
    color: var(--faint);
    background: var(--panel-2);
    border: 1px solid var(--line);
    transition: background-color 0.15s, color 0.15s;
  }
  .pill.rx.on {
    color: #fff;
    background: var(--rx);
    border-color: var(--rx-edge);
  }
  .pill.tx.on {
    color: #fff;
    background: var(--tx);
    border-color: var(--tx-edge);
    animation: blink 1s ease-in-out infinite;
  }
  .status {
    min-width: 0;
  }
  .t {
    font-weight: 600;
    line-height: 1.2;
  }
  .d {
    font-size: 12px;
    max-width: 300px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .relay {
    display: flex;
    align-items: center;
    gap: 7px;
    border: 1px solid var(--warn);
    color: var(--warn);
    background: var(--warn-soft);
    border-radius: 6px;
    padding: 3px 10px;
    font: inherit;
    font-size: 12px;
    font-weight: 700;
    cursor: pointer;
  }
  @keyframes blink {
    50% {
      opacity: 0.55;
    }
  }
</style>
