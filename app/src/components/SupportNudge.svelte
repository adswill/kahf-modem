<script lang="ts">
  import { app, KOFI_URL } from "../lib/state.svelte";
</script>

{#if app.supportNudge}
  <div class="nudge" role="status">
    <p>Enjoying KAHF? If you like it, consider supporting its development. Thank you!</p>
    <div class="row wrap">
      <button class="btn sm" onclick={() => { window.open(KOFI_URL, "_blank"); app.closeNudge(false); }}>Support on Ko-fi</button>
      <button class="btn sm quiet" onclick={() => app.closeNudge(false)}>Not now</button>
      <button class="btn sm quiet" onclick={() => app.closeNudge(true)}>Don't ask again</button>
    </div>
  </div>
{/if}

<style>
  .nudge {
    position: fixed;
    left: 16px;
    bottom: 16px;
    max-width: 360px;
    z-index: 89;
    padding: 12px 14px;
    background: var(--panel-2);
    border: 1px solid var(--info);
    border-radius: 6px;
    box-shadow: var(--shadow);
    font-size: 13px;
  }
  p {
    margin: 0 0 8px;
  }
</style>
