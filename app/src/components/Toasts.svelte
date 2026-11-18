<script lang="ts">
  import { app } from "../lib/state.svelte";
</script>

<div class="toasts" aria-live="polite">
  {#each app.toasts as t (t.id)}
    <div class="toast {t.kind}">{t.text}</div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    right: 16px;
    bottom: 16px;
    display: flex;
    flex-direction: column;
    gap: 6px;
    z-index: 90;
    max-width: 400px;
  }
  .toast {
    padding: 9px 13px;
    background: var(--panel-2);
    border: 1px solid var(--line-strong);
    border-color: var(--info);
    border-radius: 6px;
    box-shadow: var(--shadow);
    font-size: 13px;
  }
  .toast.good {
    border-color: var(--good);
  }
  .toast.warn {
    border-color: var(--accent);
  }
</style>
