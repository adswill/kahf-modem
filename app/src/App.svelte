<script lang="ts">
  import { onMount } from "svelte";
  import { app } from "./lib/state.svelte";
  import Sidebar from "./components/Sidebar.svelte";
  import Toasts from "./components/Toasts.svelte";
  import Tour from "./components/Tour.svelte";
  import Welcome from "./pages/Welcome.svelte";
  import Setup from "./pages/Setup.svelte";
  import Home from "./pages/Home.svelte";
  import Messages from "./pages/Messages.svelte";
  import Network from "./pages/Network.svelte";
  import Radio from "./pages/Radio.svelte";
  import Help from "./pages/Help.svelte";
  import Settings from "./pages/Settings.svelte";

  onMount(() => app.init());

  const fullScreen = $derived(app.page === "welcome" || app.page === "setup");

  function keys(e: KeyboardEvent) {
    if (!(e.metaKey || e.ctrlKey) || fullScreen) return;
    const map: Record<string, typeof app.page> = { "1": "home", "2": "messages", "3": "network", "4": "radio", "5": "help", "6": "settings" };
    const p = map[e.key];
    if (p) {
      e.preventDefault();
      app.go(p);
    }
  }
</script>

<svelte:window onkeydown={keys} />

{#if app.ready}
  {#if fullScreen}
    {#if app.page === "welcome"}<Welcome />{:else}<Setup />{/if}
  {:else}
    <div class="shell">
      <Sidebar />
      <main class="main">
        {#if app.page === "home"}<Home />
        {:else if app.page === "messages"}<Messages />
        {:else if app.page === "network"}<Network />
        {:else if app.page === "radio"}<Radio />
        {:else if app.page === "help"}<Help />
        {:else}<Settings />{/if}
      </main>
    </div>
  {/if}
  <Toasts />
  {#if app.tourOpen}<Tour />{/if}
{/if}

<style>
  .shell {
    height: 100%;
    display: flex;
  }
  .main {
    flex: 1;
    min-width: 0;
    height: 100%;
    overflow: hidden;
  }
</style>
