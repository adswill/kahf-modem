<script lang="ts">
  // How the radio's sound reaches the computer: a sound card, an SDR receiver, or nothing yet.
  import { onMount } from "svelte";
  import { app } from "../lib/state.svelte";

  type Kind = "sound" | "sdr" | "none";
  let {
    kind = $bindable(),
    audioIn = $bindable(),
    audioOut = $bindable(),
    sdr = $bindable(),
  }: { kind: Kind; audioIn: string; audioOut: string; sdr: string } = $props();

  let devices = $state<string[]>([]);
  let sdrs = $state<string[]>([]);
  onMount(async () => {
    devices = await app.engine.listAudioDevices();
    sdrs = await app.engine.detectSdr();
  });
</script>

<div class="col">
  <button class="pick" class:on={kind === "sound"} onclick={() => (kind = "sound")}>
    <span class="dot"></span>
    <span><b>Sound card or radio interface</b><span class="s">The radio's audio goes into this computer's sound input. The usual way.</span></span>
  </button>
  {#if kind === "sound"}
    <div class="two">
      <label class="field">
        <span class="t">From the radio</span>
        <select bind:value={audioIn}>
          <option value="">Computer default</option>
          {#each devices as d}<option value={d}>{d}</option>{/each}
        </select>
      </label>
      <label class="field">
        <span class="t">To the radio</span>
        <select bind:value={audioOut}>
          <option value="">Computer default</option>
          {#each devices as d}<option value={d}>{d}</option>{/each}
        </select>
      </label>
    </div>
  {/if}

  <button class="pick" class:on={kind === "sdr"} onclick={() => (kind = "sdr")}>
    <span class="dot"></span>
    <span><b>An SDR receiver</b><span class="s">HackRF, RTL-SDR and similar USB radios that connect straight to the computer.</span></span>
  </button>
  {#if kind === "sdr"}
    <label class="field">
      <span class="t">Receiver</span>
      <select bind:value={sdr}>
        {#each sdrs as d}<option value={d}>{d}</option>{/each}
        {#if sdrs.length === 0}<option value="">None found. Plug it in, then reopen this.</option>{/if}
      </select>
    </label>
  {/if}

  <button class="pick" class:on={kind === "none"} onclick={() => (kind = "none")}>
    <span class="dot"></span>
    <span><b>No radio yet</b><span class="s">Skip this. Nothing is heard until you connect one on the Radio screen.</span></span>
  </button>
</div>

<style>
  .two {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
  }
</style>
