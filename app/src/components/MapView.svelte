<script lang="ts">
  // A small slippy map: drag to move, wheel or buttons to zoom. Tiles come from the KAHF server
  // (which fetches them from OpenStreetMap for us). With no internet it still shows a grid and
  // the markers, so the screen is never empty.
  import type { RelayInfo } from "../lib/types";

  let {
    me = null,
    relays = [],
    stations = [],
    height = 380,
  }: { me?: { lat: number; lon: number } | null; relays?: RelayInfo[]; stations?: { call: string; lat: number; lon: number }[]; height?: number } = $props();

  const TILE = 256;
  const SERVER = "https://kahf.dev/tiles";

  let box: HTMLDivElement | undefined = $state();
  let w = $state(600);
  let zoom = $state(3);
  let center = $state({ lat: 30, lon: 10 });
  let centred = false;
  // Start on the person's own position once it is known.
  $effect(() => {
    if (me && !centred) {
      centred = true;
      center = { lat: me.lat, lon: me.lon };
    }
  });
  let offline = $state(false);
  let drag: { x: number; y: number; lat: number; lon: number } | null = null;

  const world = $derived(TILE * 2 ** zoom);
  const px = (lon: number) => ((lon + 180) / 360) * world;
  const py = (lat: number) => {
    const s = Math.sin((Math.max(-85, Math.min(85, lat)) * Math.PI) / 180);
    return (0.5 - Math.log((1 + s) / (1 - s)) / (4 * Math.PI)) * world;
  };
  const toLon = (x: number) => (x / world) * 360 - 180;
  const toLat = (y: number) => {
    const n = Math.PI - (2 * Math.PI * y) / world;
    return (180 / Math.PI) * Math.atan(0.5 * (Math.exp(n) - Math.exp(-n)));
  };

  const originX = $derived(px(center.lon) - w / 2);
  const originY = $derived(py(center.lat) - height / 2);

  const tiles = $derived.by(() => {
    const n = 2 ** zoom;
    const out: { x: number; y: number; left: number; top: number; key: string }[] = [];
    for (let tx = Math.floor(originX / TILE); tx <= Math.floor((originX + w) / TILE); tx++) {
      for (let ty = Math.floor(originY / TILE); ty <= Math.floor((originY + height) / TILE); ty++) {
        if (ty < 0 || ty >= n) continue;
        const wrapped = ((tx % n) + n) % n;
        out.push({ x: wrapped, y: ty, left: tx * TILE - originX, top: ty * TILE - originY, key: `${zoom}/${wrapped}/${ty}/${tx}` });
      }
    }
    return out;
  });

  const at = (lat: number, lon: number) => ({ x: px(lon) - originX, y: py(lat) - originY });

  function down(e: PointerEvent) {
    drag = { x: e.clientX, y: e.clientY, lat: center.lat, lon: center.lon };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }
  function move(e: PointerEvent) {
    if (!drag) return;
    const dx = e.clientX - drag.x;
    const dy = e.clientY - drag.y;
    const cx = px(drag.lon) - dx;
    const cy = py(drag.lat) - dy;
    center = { lat: Math.max(-80, Math.min(80, toLat(cy))), lon: toLon(cx) };
  }
  function up() {
    drag = null;
  }
  function wheel(e: WheelEvent) {
    e.preventDefault();
    zoomBy(e.deltaY < 0 ? 1 : -1);
  }
  function zoomBy(d: number) {
    zoom = Math.max(1, Math.min(8, zoom + d));
  }
  function here() {
    if (me) center = { lat: me.lat, lon: me.lon };
  }
</script>

<div class="map" bind:this={box} bind:clientWidth={w} style:height={`${height}px`}>
  <div class="grid"></div>
  <div class="surface" role="application" aria-label="Map" onpointerdown={down} onpointermove={move} onpointerup={up} onwheel={wheel}>
    {#each tiles as t (t.key)}
      <img
        src={`${SERVER}/${zoom}/${t.x}/${t.y}.png`}
        alt=""
        draggable="false"
        style:left={`${t.left}px`}
        style:top={`${t.top}px`}
        onerror={() => (offline = true)}
        onload={() => (offline = false)}
      />
    {/each}
    {#each relays.filter((r) => r.lat !== null && r.lon !== null) as r (r.call)}
      {@const p = at(r.lat as number, r.lon as number)}
      <div class="pin relay" style:left={`${p.x}px`} style:top={`${p.y}px`} title="{r.call}{r.km !== null ? ` · ${Math.round(r.km)} km` : ''}">
        <span class="mono">{r.call}</span>
      </div>
    {/each}
    {#each stations as s (s.call)}
      {@const p = at(s.lat, s.lon)}
      <div class="pin station" style:left={`${p.x}px`} style:top={`${p.y}px`} title={s.call}>
        <span class="mono">{s.call}</span>
      </div>
    {/each}
    {#if me}
      {@const p = at(me.lat, me.lon)}
      <div class="pin me" style:left={`${p.x}px`} style:top={`${p.y}px`}><span class="mono">YOU</span></div>
    {/if}
  </div>
  <div class="ctl">
    <button class="btn sm icon" onclick={() => zoomBy(1)} aria-label="Zoom in">+</button>
    <button class="btn sm icon" onclick={() => zoomBy(-1)} aria-label="Zoom out">−</button>
    {#if me}<button class="btn sm" onclick={here}>My position</button>{/if}
  </div>
  {#if offline}
    <div class="off small">Map pictures need the internet. Relays are still placed correctly.</div>
  {/if}
</div>

<style>
  .map {
    position: relative;
    overflow: hidden;
    border: 1px solid var(--line);
    border-radius: 6px;
    background: var(--bg);
    touch-action: none;
  }
  .grid {
    position: absolute;
    inset: 0;
    background-image: linear-gradient(var(--line) 1px, transparent 1px), linear-gradient(90deg, var(--line) 1px, transparent 1px);
    background-size: 48px 48px;
    opacity: 0.5;
  }
  .surface {
    position: absolute;
    inset: 0;
    cursor: grab;
  }
  .surface:active {
    cursor: grabbing;
  }
  img {
    position: absolute;
    width: 256px;
    height: 256px;
    user-select: none;
    filter: grayscale(0.85) invert(0.9) hue-rotate(180deg) contrast(0.85);
  }
  :global(:root[data-theme="light"]) img {
    filter: grayscale(0.5) contrast(0.95);
  }
  .pin {
    position: absolute;
    transform: translate(-50%, -100%);
    display: flex;
    flex-direction: column;
    align-items: center;
    font-size: 11px;
    font-weight: 700;
    pointer-events: auto;
  }
  .pin span {
    padding: 1px 5px;
    border-radius: 3px;
    background: var(--panel);
    border: 1px solid var(--line-strong);
  }
  .pin::after {
    content: "";
    width: 9px;
    height: 9px;
    border-radius: 50%;
    margin-top: 2px;
    background: var(--good);
    border: 2px solid var(--bg);
  }
  .pin.station::after {
    background: var(--info);
  }
  .pin.me span {
    color: var(--accent-ink);
    background: var(--accent);
    border-color: var(--accent);
  }
  .pin.me::after {
    background: var(--accent);
  }
  .ctl {
    position: absolute;
    right: 8px;
    top: 8px;
    display: flex;
    gap: 6px;
  }
  .ctl .btn {
    background: var(--panel);
  }
  .off {
    position: absolute;
    left: 8px;
    bottom: 8px;
    padding: 4px 8px;
    border: 1px solid var(--line-strong);
    border-radius: 4px;
    background: var(--panel);
    color: var(--muted);
  }
</style>
