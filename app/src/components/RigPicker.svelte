<script lang="ts">
  // Choose the radio by name. KAHF starts Hamlib's rigctld for it, so the frequency can be set
  // and read from the computer. The test button reads the radio's frequency straight away.
  import { onMount } from "svelte";
  import { app } from "../lib/state.svelte";
  import type { RigModel } from "../lib/types";
  import Icon from "./Icon.svelte";
  import TestButton from "./TestButton.svelte";

  let {
    value = $bindable(),
    address = $bindable(""),
    spec = $bindable(""),
  }: { value: { model: number | null; name: string; port: string; baud: number }; address?: string; spec?: string } = $props();

  // Direct control: KAHF talks to the radio's serial port itself, without Hamlib.
  const PTTS = [
    { id: "cat", label: "Radio command (needs no wiring)" },
    { id: "rts", label: "RTS line of the serial port" },
    { id: "dtr", label: "DTR line of the serial port" },
    { id: "vox", label: "VOX (the radio keys itself from the sound)" },
  ];
  const parts = spec.split(":");
  const startsWithModel = spec !== "" && !spec.startsWith("rigctld") && !spec.startsWith("flrig");
  let directModels = $state<import("../lib/types").DirectRig[]>([]);
  let dModel = $state(startsWithModel ? parts[0] : "");
  let dPort = $state(startsWithModel ? parts[1] ?? "" : "");
  let dBaud = $state(startsWithModel && /^\d+$/.test(parts[2] ?? "") ? Number(parts[2]) : 0);
  let dPtt = $state(startsWithModel ? (PTTS.find((p) => p.id === parts[parts.length - 1])?.id ?? "cat") : "cat");
  const directSpec = $derived(dModel && dPort ? `${dModel}:${dPort}${dBaud ? `:${dBaud}` : ""}:${dPtt}` : "");
  $effect(() => {
    spec = directSpec;
  });
  function pickDirect(id: string) {
    dModel = id;
    dBaud = directModels.find((m) => m.id === id)?.baud ?? 0;
  }

  let available = $state<boolean | null>(null);
  let models = $state<RigModel[]>([]);
  let ports = $state<string[]>([]);
  let query = $state("");
  let testing = $state(false);
  let result = $state<{ ok: boolean; text: string } | null>(null);
  const BAUDS = [4800, 9600, 19200, 38400, 57600, 115200];

  onMount(async () => {
    directModels = await app.engine.rigDirectModels().catch(() => []);
    if (!ports.length) ports = await app.engine.rigPorts().catch(() => []);
    await load();
  });

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

  <details class="adv" open={dModel !== ""}>
    <summary>Control the radio directly (no Hamlib needed)</summary>
    <p class="faint small">For the radios listed here KAHF talks to the serial port itself, and can key the transmitter by command or by a port line.</p>
    <div class="grid2">
      <label class="field">
        <span class="t">Radio</span>
        <select value={dModel} onchange={(e) => pickDirect(e.currentTarget.value)}>
          <option value="">Not used</option>
          {#each directModels as m (m.id)}<option value={m.id}>{m.label}</option>{/each}
        </select>
      </label>
      <label class="field">
        <span class="t">Serial port</span>
        <select bind:value={dPort}>
          <option value="">Choose…</option>
          {#each ports as p}<option value={p}>{p}</option>{/each}
        </select>
      </label>
      <label class="field">
        <span class="t">Speed (baud)</span>
        <input type="number" min="0" step="1" bind:value={dBaud} />
        <span class="hint">0 uses the radio's usual speed.</span>
      </label>
      <label class="field">
        <span class="t">Keying (PTT)</span>
        <select bind:value={dPtt}>
          {#each PTTS as p (p.id)}<option value={p.id}>{p.label}</option>{/each}
        </select>
      </label>
    </div>
    {#if directSpec}
      <TestButton label="Test the radio" run={() => app.engine.testRigSpec(directSpec)} />
      <p class="faint small">The test reads the radio's frequency. It never transmits.</p>
    {/if}
  </details>

  <details class="adv" open={address !== ""}>
    <summary>Already running a radio control program?</summary>
    <label class="field">
      <span class="t">rigctld address</span>
      <input type="text" placeholder="127.0.0.1:4532" bind:value={address} spellcheck="false" />
      <span class="hint">Host and port of a rigctld you started yourself. Leave empty if KAHF starts it for you.</span>
    </label>
    <TestButton label="Test the address" run={() => app.engine.testRig(address)} />
  </details>
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
  .adv {
    border-top: 1px solid var(--line);
    padding-top: 10px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .adv summary {
    cursor: pointer;
    color: var(--muted);
    margin-bottom: 8px;
  }
  .grid2 {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
  }
</style>
