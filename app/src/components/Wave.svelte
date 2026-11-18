<script lang="ts">
  // A small waveform for voice notes. Not the real audio: a steady pattern per note, so each
  // one looks its own and playback progress has something to sweep across.
  let { seed, progress = 0, bars = 30 }: { seed: string; progress?: number; bars?: number } = $props();

  const heights = $derived.by(() => {
    let h = 7;
    for (const c of seed) h = (h * 31 + c.charCodeAt(0)) >>> 0;
    return Array.from({ length: bars }, () => {
      h = (h * 1664525 + 1013904223) >>> 0;
      return 18 + (h % 82);
    });
  });
</script>

<div class="wave" aria-hidden="true">
  {#each heights as h, i}
    <span style:height={`${h}%`} class:played={i / bars < progress}></span>
  {/each}
</div>

<style>
  .wave {
    display: flex;
    align-items: center;
    gap: 2px;
    height: 26px;
  }
  span {
    width: 3px;
    background: var(--line-strong);
    border-radius: 2px;
  }
  span.played {
    background: var(--accent);
  }
</style>
