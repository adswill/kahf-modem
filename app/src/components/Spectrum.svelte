<script lang="ts">
  // The live picture of the radio band: a line graph on top and a scrolling waterfall below.
  // Audio from the radio covers 0 to 4 kHz; KAHF uses the middle part, 0.3 to 2.7 kHz.
  import { app } from "../lib/state.svelte";

  let { height = 340, compact = false }: { height?: number; compact?: boolean } = $props();

  let canvas: HTMLCanvasElement;
  let wf: HTMLCanvasElement | null = null;

  const DB_MIN = -105;
  const DB_MAX = -45;
  const AXIS = 26;

  // A blue-to-yellow colour ramp for the waterfall.
  const ramp: [number, number, number][] = [];
  for (let i = 0; i < 256; i++) {
    const t = i / 255;
    const stops = [
      [6, 10, 40],
      [20, 60, 140],
      [20, 150, 150],
      [90, 200, 110],
      [250, 220, 70],
    ];
    const p = t * (stops.length - 1);
    const k = Math.min(stops.length - 2, Math.floor(p));
    const f = p - k;
    ramp.push([0, 1, 2].map((c) => Math.round(stops[k][c] + (stops[k + 1][c] - stops[k][c]) * f)) as [number, number, number]);
  }

  function css(name: string): string {
    return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  }

  function draw() {
    if (!canvas) return;
    const dpr = window.devicePixelRatio || 1;
    const w = canvas.clientWidth;
    const h = canvas.clientHeight;
    if (canvas.width !== Math.round(w * dpr) || canvas.height !== Math.round(h * dpr)) {
      canvas.width = Math.round(w * dpr);
      canvas.height = Math.round(h * dpr);
      wf = null;
    }
    const ctx = canvas.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    const bins = app.spectrum;
    const plotH = Math.round((h - AXIS) * 0.42);
    const wfH = h - AXIS - plotH;

    ctx.fillStyle = css("--panel-2");
    ctx.fillRect(0, 0, w, h);

    // The KAHF channel: the part of the audio that KAHF listens to and sends in.
    const x = (hz: number) => (hz / 4000) * w;
    ctx.fillStyle = css("--accent-soft");
    ctx.fillRect(x(300), 0, x(2700) - x(300), plotH + wfH);
    ctx.strokeStyle = css("--accent");
    ctx.globalAlpha = 0.5;
    ctx.setLineDash([4, 4]);
    ctx.beginPath();
    ctx.moveTo(x(300), 0);
    ctx.lineTo(x(300), plotH + wfH);
    ctx.moveTo(x(2700), 0);
    ctx.lineTo(x(2700), plotH + wfH);
    ctx.stroke();
    ctx.setLineDash([]);
    ctx.globalAlpha = 1;

    // Grid.
    ctx.strokeStyle = css("--line");
    ctx.lineWidth = 1;
    for (let db = -100; db <= -50; db += 10) {
      const y = plotH - ((db - DB_MIN) / (DB_MAX - DB_MIN)) * plotH;
      ctx.beginPath();
      ctx.moveTo(0, y);
      ctx.lineTo(w, y);
      ctx.stroke();
    }

    if (bins.length) {
      // Line graph.
      ctx.beginPath();
      for (let i = 0; i < bins.length; i++) {
        const px = (i / (bins.length - 1)) * w;
        const py = plotH - Math.max(0, Math.min(1, (bins[i] - DB_MIN) / (DB_MAX - DB_MIN))) * plotH;
        if (i === 0) ctx.moveTo(px, py);
        else ctx.lineTo(px, py);
      }
      ctx.strokeStyle = css("--accent");
      ctx.lineWidth = 1.5;
      ctx.stroke();

      // Waterfall: scroll down one row, paint the new one on top.
      if (!wf || wf.width !== canvas.width || wf.height !== Math.round(wfH * dpr)) {
        wf = document.createElement("canvas");
        wf.width = canvas.width;
        wf.height = Math.max(1, Math.round(wfH * dpr));
        const c = wf.getContext("2d")!;
        c.fillStyle = css("--bg");
        c.fillRect(0, 0, wf.width, wf.height);
      }
      const wc = wf.getContext("2d")!;
      // Each frame adds about 2 screen pixels, so a few seconds of history are easy to read.
      const step = Math.max(1, Math.round(2 * dpr));
      wc.drawImage(wf, 0, step);
      const row = wc.createImageData(wf.width, step);
      for (let px = 0; px < wf.width; px++) {
        const v = bins[Math.min(bins.length - 1, Math.floor((px / wf.width) * bins.length))];
        const t = Math.max(0, Math.min(1, (v - DB_MIN) / (DB_MAX - DB_MIN)));
        const [r, g, b] = ramp[Math.round(t * 255)];
        for (let y = 0; y < step; y++) {
          const o = (y * wf.width + px) * 4;
          row.data[o] = r;
          row.data[o + 1] = g;
          row.data[o + 2] = b;
          row.data[o + 3] = 255;
        }
      }
      wc.putImageData(row, 0, 0);
      ctx.drawImage(wf, 0, plotH, w, wfH);
    }

    // Frequency axis.
    ctx.fillStyle = css("--faint");
    ctx.font = "11px Consolas, Menlo, monospace";
    ctx.textBaseline = "top";
    const tuned = app.settings.freqKhz;
    const labelStep = w < 560 ? 1000 : 500;
    for (let hz = 0; hz <= 4000; hz += labelStep) {
      const px = x(hz);
      ctx.textAlign = hz === 0 ? "left" : hz === 4000 ? "right" : "center";
      ctx.fillText((tuned + hz / 1000).toFixed(1), px, plotH + wfH + 6);
    }
    ctx.textAlign = "left";
    ctx.fillStyle = css("--accent");
    ctx.fillText("KAHF channel", x(300) + 8, 6);
  }

  $effect(() => {
    app.spectrum;
    app.settings.theme;
    requestAnimationFrame(draw);
  });
</script>

<div class="spectrum" style:height={`${height}px`}>
  <canvas bind:this={canvas} aria-label="Live radio spectrum and waterfall"></canvas>
  {#if !compact}
    <div class="legend small faint">
      Brighter is stronger. The shaded band is the one 2.4 kHz channel KAHF uses, whatever the speed.
    </div>
  {/if}
</div>

<style>
  .spectrum {
    position: relative;
    border-radius: 6px;
    overflow: hidden;
    border: 1px solid var(--line);
    display: flex;
    flex-direction: column;
  }
  canvas {
    width: 100%;
    flex: 1;
    display: block;
    min-height: 0;
  }
  .legend {
    padding: 8px 12px;
    border-top: 1px solid var(--line);
    background: var(--panel);
  }
</style>
