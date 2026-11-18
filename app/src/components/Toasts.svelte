<script lang="ts">
  import { app } from "../lib/state.svelte";
  import Icon from "./Icon.svelte";
</script>

<div class="toasts" aria-live="polite">
  {#each app.toasts as t (t.id)}
    <div class="toast {t.kind}">
      <Icon name={t.kind === "warn" ? "warn" : t.kind === "good" ? "check" : "info"} />
      <span>{t.text}</span>
    </div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    right: 20px;
    bottom: 20px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    z-index: 90;
    max-width: 420px;
  }
  .toast {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    padding: 12px 16px;
    border-radius: 12px;
    background: var(--raised);
    border: 1px solid var(--line);
    box-shadow: var(--shadow);
    font-size: 14px;
    animation: in 0.2s ease-out;
  }
  .toast.good {
    border-color: var(--accent);
  }
  .toast.warn {
    border-color: var(--amber);
  }
  @keyframes in {
    from {
      transform: translateY(8px);
      opacity: 0;
    }
  }
</style>
