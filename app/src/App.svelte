<script lang="ts">
  import { onMount } from "svelte";
  import { app } from "./lib/state.svelte";
  import RadioStrip from "./components/RadioStrip.svelte";
  import Nav from "./components/Nav.svelte";
  import Toasts from "./components/Toasts.svelte";
  import Tour from "./components/Tour.svelte";
  import FileConfirm from "./components/FileConfirm.svelte";
  import SettingsSheet from "./components/SettingsSheet.svelte";
  import HelpPanel from "./components/HelpPanel.svelte";
  import Welcome from "./pages/Welcome.svelte";
  import Setup from "./pages/Setup.svelte";
  import Chats from "./views/Chats.svelte";
  import Mail from "./views/Mail.svelte";
  import Stations from "./views/Stations.svelte";
  import Radio from "./views/Radio.svelte";
  import Relay from "./views/Relay.svelte";

  onMount(() => app.init());

  const fullScreen = $derived(app.page === "welcome" || app.page === "setup");

  function keys(e: KeyboardEvent) {
    if (e.key === "Escape") {
      app.settingsOpen = false;
      app.helpOpen = false;
      return;
    }
    if (!(e.metaKey || e.ctrlKey) || fullScreen) return;
    const map: Record<string, typeof app.page> = { "1": "chats", "2": "mail", "3": "stations", "4": "radio", "5": "relay" };
    if (e.key === ",") {
      e.preventDefault();
      app.openSettings();
    } else if (map[e.key]) {
      e.preventDefault();
      app.go(map[e.key]);
    }
  }
</script>

<svelte:window onkeydown={keys} />

{#if app.ready}
  {#if fullScreen}
    {#if app.page === "welcome"}<Welcome />{:else}<Setup />{/if}
  {:else}
    <div class="shell">
      <RadioStrip />
      {#if app.engineError}
        <div class="banner" role="alert">
          <span>{app.engineError}</span>
          <button class="btn sm" onclick={() => app.startEngine()}>Try again</button>
          <button class="btn sm" onclick={() => app.go("radio")}>Open Radio</button>
        </div>
      {/if}
      <div class="below">
        <Nav />
        <main class="main">
          {#if app.page === "chats"}<Chats />
          {:else if app.page === "mail"}<Mail />
          {:else if app.page === "stations"}<Stations />
          {:else if app.page === "radio"}<Radio />
          {:else}<Relay />{/if}
        </main>
      </div>
    </div>
    {#if app.settingsOpen}<SettingsSheet />{/if}
    {#if app.helpOpen}<HelpPanel />{/if}
  {/if}
  <Toasts />
  <FileConfirm />
  {#if app.tourOpen}<Tour />{/if}
{/if}

<style>
  .shell {
    height: 100%;
    display: flex;
    flex-direction: column;
  }
  .below {
    flex: 1;
    display: flex;
    min-height: 0;
  }
  .main {
    flex: 1;
    min-width: 0;
    min-height: 0;
  }
  .banner {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 18px;
    background: var(--accent-soft);
    border-bottom: 1px solid var(--accent);
    font-size: 13px;
  }
  .banner span {
    flex: 1;
  }
</style>
