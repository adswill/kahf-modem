<script lang="ts">
  // Choose the radio by name. KAHF starts Hamlib's rigctld for it, so the frequency can be set
  // and read from the computer. The test button reads the radio's frequency straight away.
  import { onMount } from "svelte";
  import { app } from "../lib/state.svelte";
  import type { RigModel } from "../lib/types";
  import Icon from "./Icon.svelte";

  let { value = $bindable() }: { value: { model: number | null; name: string; port: string; baud: number } } = $props();

  let available = $state<boolean | null>(null);
  let models = $state<RigModel[]>([]);
  let ports = $state<string[]>([]);
  let query = $state("");
  let testing = $state(false);
  let result = $state<{ ok: boolean; text: string } | null>(null);
  const BAUDS = [4800, 9600, 19200, 38400, 57600, 115200];

  onMount(load);

  async function load() {
    available = await app.engine.rigAvailable();
    if (available) {
      models = await app.engine.rigModels().catch(() => []);
      ports = await app.engine.rigPorts();
    }
  }

  const matches = $derived.by(() => {
    const q = query.trim().toLowerCase();
    if (!q) return [];
    const words = q.split(/\s+/);
    return models.filter((m) => words.every((w) => `${m.maker} ${m.model}`.toLowerCase().includes(w))).slice(0, 8);
  });

  function choose(m: RigModel) {
    value.model = m.id;
    value.name = `${m.maker} ${m.model}`;
    query = "";
    result = null;
  }

  function clear() {
    value.model = null;
    value.name = "";
    result = null;
    app.engine.rigStop().catch(() => {});
  }

  async function test() {
    if (value.model === null) return;
    testing = true;
    result = null;
    try {
      const hz = await app.engine.rigConnect(value.model, value.port, value.baud);
      result = { ok: true, text: `Connected. Your radio is on ${(hz / 1e6).toFixed(6)} MHz.` };
    } catch (e) {
      result = { ok: false, text: String(e).replace(/^Error: /, "") };
    }
    testing = false;
  }
</script>

<div class="col">
  {#if available === false}
    <div class="note warn">
      <Icon name="info" size={16} />
      <div class="col tight">
        <div><b>Radio control needs Hamlib.</b> It is a free program that talks to more than 300 radios.</div>
        <div class="mono small">Mac: brew install hamlib<br />Linux: sudo apt install libhamlib-utils<br />Windows: hamlib.github.io</div>
        <div><button class="btn sm" onclick={load}><Icon name="refresh" size={13} /> I installed it, check again</button></div>
      </div>
    </div>
  {:else if available}
    {#if value.model === null}
      <label class="field">
        <span class="t">Search for your radio</span>
        <input type="search" placeholder="For example IC-7300, FT-991, K3" bind:value={query} />
        <span class="hint">{models.length} radios known. Type part of the name.</span>
      </label>
      {#if matches.length}
        <div class="results">
          {#each matches as m (m.id)}
            <button class="pick" onclick={() => choose(m)}>
              <span class="grow"><b>{m.maker} {m.model}</b></span>
              {#if m.status !== "Stable"}<span class="tag warn">{m.status}</span>{/if}
            </button>
          {/each}
        </div>
      {:else if query.trim()}
        <p class="faint small">No radio matches "{query}". Try fewer letters.</p>
      {/if}
    {:else}
      <div class="chosen row">
        <Icon name="radio" size={18} />
        <div class="grow"><b>{value.name}</b></div>
        <button class="btn sm quiet" onclick={clear}>Change</button>
      </div>
      <div class="grid2">
        <label class="field">
          <span class="t">Serial port</span>
          <select bind:value={value.port}>
            <option value="">Choose…</option>
            {#each ports as p}<option value={p}>{p}</option>{/each}
          </select>
          <span class="hint">The USB cable from the radio to the computer.</span>
        </label>
        <label class="field">
          <span class="t">Speed (baud)</span>
          <select bind:value={value.baud}>
            {#each BAUDS as b}<option value={b}>{b}</option>{/each}
          </select>
          <span class="hint">Must match the radio's menu setting.</span>
        </label>
      </div>
      <div class="row">
        <button class="btn" onclick={test} disabled={testing || !value.port}>{testing ? "Testing…" : "Test the connection"}</button>
        {#if result}<span class={result.ok ? "ok" : "err"}>{result.text}</span>{/if}
      </div>
      {#if result && !result.ok}
        <p class="faint small">Check that the radio is on, the cable is in, and its USB or CAT speed matches. On an Icom the CI-V address must match too.</p>
      {/if}
    {/if}
  {:else}
    <p class="faint small">Looking for Hamlib…</p>
  {/if}
</div>

<style>
  .results {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .chosen {
    padding: 9px 12px;
    border: 1px solid var(--accent);
    background: var(--accent-soft);
    border-radius: 6px;
  }
  .grid2 {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
  }
</style>
