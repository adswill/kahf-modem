<script lang="ts">
  // A Test button with its answer beside it. The tests only listen or look; they never transmit.
  let { label = "Test", run }: { label?: string; run: () => Promise<string> } = $props();

  let state = $state<{ running: boolean; ok: boolean; text: string } | null>(null);

  async function go() {
    state = { running: true, ok: true, text: "" };
    try {
      state = { running: false, ok: true, text: await run() };
    } catch (e) {
      state = { running: false, ok: false, text: String(e).replace(/^Error: /, "") };
    }
  }
</script>

<div class="test">
  <button class="btn sm" onclick={go} disabled={state?.running}>{state?.running ? "Testing…" : label}</button>
  {#if state && !state.running}
    <span class="answer" class:bad={!state.ok} role="status">{state.text}</span>
  {/if}
</div>

<style>
  .test {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    flex-wrap: wrap;
  }
  .answer {
    font-size: 12px;
    line-height: 1.4;
    color: var(--good);
    max-width: 52ch;
    padding-top: 3px;
  }
  .answer.bad {
    color: var(--bad);
  }
</style>
