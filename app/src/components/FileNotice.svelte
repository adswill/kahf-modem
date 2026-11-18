<script lang="ts">
  import { app, sizeWords } from "../lib/state.svelte";
  import { FILE_LIMIT_BYTES, durationWords, speedWords, transferSeconds } from "../lib/format";
  import type { PickedFile } from "../lib/types";

  let { files }: { files: PickedFile[] } = $props();

  const total = $derived(files.reduce((n, f) => n + f.size, 0));
  const over = $derived(total > FILE_LIMIT_BYTES);
  const bps = $derived(app.estimateBps);
  const slowest = $derived(transferSeconds(total, 41));
  const now = $derived(transferSeconds(total, bps));
</script>

{#if files.length}
  <div class="note" class:bad={over}>
    <p class="line">
      <b>{sizeWords(total)}</b> to send.
      {#if over}
        That is over the {sizeWords(FILE_LIMIT_BYTES)} limit. Radio is slow, so larger files are not accepted.
      {:else}
        At the current speed ({speedWords(bps)}) it takes <b>{durationWords(now)}</b>.
        On a weak signal it can take up to {durationWords(slowest)}.
      {/if}
    </p>
    <p class="line faint">
      Files go straight to the other station and cannot be passed on through relays, so you must be in direct range of them.
      Keep KAHF open and the radio connected until it finishes.
    </p>
  </div>
{/if}

<style>
  .note {
    border: 1px solid var(--line-strong);
    border-left: 3px solid var(--accent);
    border-radius: 4px;
    padding: 9px 12px;
    font-size: 13px;
    background: var(--panel-2);
  }
  .note.bad {
    border-left-color: var(--bad, #d9534f);
  }
  .line {
    margin: 0 0 4px;
  }
  .line:last-child {
    margin: 0;
  }
</style>
