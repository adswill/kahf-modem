<script lang="ts">
  import { app, sizeWords } from "../lib/state.svelte";
  import FileNotice from "./FileNotice.svelte";
  import Icon from "./Icon.svelte";
</script>

{#if app.pendingFiles}
  {@const p = app.pendingFiles}
  <div class="scrim" role="presentation" onclick={() => (app.pendingFiles = null)}></div>
  <div class="dlg" role="dialog" aria-label="Send files">
    <h2>Send to {p.to}?</h2>
    <ul>
      {#each p.files as f}
        <li><Icon name="file" size={15} /> <span class="grow">{f.name}</span> <span class="faint small">{sizeWords(f.size)}</span></li>
      {/each}
    </ul>
    <FileNotice files={p.files} />
    <div class="row end">
      <button class="btn quiet" onclick={() => (app.pendingFiles = null)}>Cancel</button>
      <button class="btn primary" onclick={() => app.confirmFiles()}>Send</button>
    </div>
  </div>
{/if}

<style>
  .scrim {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    z-index: 80;
  }
  .dlg {
    position: fixed;
    z-index: 81;
    left: 50%;
    top: 50%;
    transform: translate(-50%, -50%);
    width: min(480px, calc(100vw - 32px));
    background: var(--panel);
    border: 1px solid var(--line-strong);
    border-radius: 6px;
    padding: 18px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  h2 {
    margin: 0;
    font-size: 16px;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  li {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .end {
    justify-content: flex-end;
    gap: 8px;
  }
</style>
