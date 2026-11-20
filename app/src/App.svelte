<script lang="ts">
  import { onMount } from "svelte";
  import { app } from "./lib/state.svelte";
  import RadioStrip from "./components/RadioStrip.svelte";
  import Nav from "./components/Nav.svelte";
  import Toasts from "./components/Toasts.svelte";
  import SupportNudge from "./components/SupportNudge.svelte";
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
  import Logbook from "./views/Logbook.svelte";

  onMount(() => app.init());

  // The small switch under a main tab: two views of one place.
  const group = $derived(
    app.page === "chats" || app.page === "mail"
      ? { label: "Messages", pages: [{ page: "chats" as const, label: "Chats", badge: app.unreadChats }, { page: "mail" as const, label: "Mail", badge: app.unreadMail }] }
      : app.page === "stations" || app.page === "relay" || app.page === "logbook"
        ? { label: "Stations", pages: [{ page: "stations" as const, label: "Stations and map", badge: 0 }, { page: "relay" as const, label: "Relays", badge: 0 }, { page: "logbook" as const, label: "Logbook", badge: 0 }] }
        : null,
  );
  // Arrow keys, Home and End move between the two tabs, the way a tab strip should.
  function tabKeys(e: KeyboardEvent) {
    if (!group) return;
    const at = group.pages.findIndex((g) => g.page === app.page);
    const to = e.key === "ArrowRight" ? at + 1 : e.key === "ArrowLeft" ? at - 1 : e.key === "Home" ? 0 : e.key === "End" ? group.pages.length - 1 : -1;
    if (to < 0) return;
    e.preventDefault();
    const next = group.pages[(to + group.pages.length) % group.pages.length];
    app.go(next.page);
    setTimeout(() => document.getElementById(`tab-${next.page}`)?.focus(), 0);
  }
  const fullScreen = $derived(app.page === "welcome" || app.page === "setup");

  function keys(e: KeyboardEvent) {
    if (e.key === "Escape") {
      app.settingsOpen = false;
      app.helpOpen = false;
      return;
    }
    if (!(e.metaKey || e.ctrlKey) || fullScreen) return;
    const map: Record<string, typeof app.page> = { "1": "chats", "2": "mail", "3": "stations", "4": "radio", "5": "relay", "6": "logbook" };
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
          {#if group}
            <!-- svelte-ignore a11y_interactive_supports_focus -->
            <div class="subbar" role="tablist" aria-label={group.label} onkeydown={tabKeys}>
              {#each group.pages as g}
                <button
                  role="tab"
                  id="tab-{g.page}"
                  aria-selected={app.page === g.page}
                  aria-controls="panel-main"
                  tabindex={app.page === g.page ? 0 : -1}
                  class:on={app.page === g.page}
                  onclick={() => app.go(g.page)}
                >
                  {g.label}{#if g.badge > 0}<span class="n mono" aria-label="{g.badge} unread">{g.badge}</span>{/if}
                </button>
              {/each}
            </div>
          {/if}
          <div class="page" id="panel-main" role={group ? "tabpanel" : undefined} aria-labelledby={group ? `tab-${app.page}` : undefined}>
          {#if app.page === "chats"}<Chats />
          {:else if app.page === "mail"}<Mail />
          {:else if app.page === "stations"}<Stations />
          {:else if app.page === "radio"}<Radio />
          {:else if app.page === "logbook"}<Logbook />
          {:else}<Relay />{/if}
          </div>
        </main>
      </div>
      <footer class="statusbar">
        <span class="cell grow">{app.status.title}</span>
        <span class="cell">Station <b class="call">{app.settings.callsign || "listening only"}</b></span>
        <span class="cell mono">{(app.settings.freqKhz / 1000).toFixed(5)} MHz</span>
        {#if app.engine.demo}<span class="cell demo">Demo: pretend radio</span>{/if}
      </footer>
    </div>
    {#if app.settingsOpen}<SettingsSheet />{/if}
    {#if app.helpOpen}<HelpPanel />{/if}
  {/if}
  <Toasts />
  <SupportNudge />
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
    display: flex;
    flex-direction: column;
  }
  .page {
    flex: 1;
    min-height: 0;
  }
  .subbar {
    flex: none;
    display: flex;
    gap: 2px;
    padding: 6px 12px 0;
    background: var(--bg);
    border-bottom: 1px solid var(--line-strong);
  }
  .subbar button {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    padding: 4px 18px;
    margin-bottom: -1px;
    font: inherit;
    color: var(--muted);
    background: linear-gradient(var(--bar-top), var(--bar-bot));
    border: 1px solid var(--line);
    border-bottom-color: var(--line-strong);
    border-radius: 5px 5px 0 0;
    cursor: pointer;
  }
  .subbar button:hover {
    color: var(--text);
  }
  .subbar button.on {
    color: var(--text);
    font-weight: 600;
    background: var(--panel);
    border-color: var(--line-strong);
    border-bottom-color: var(--panel);
  }
  .subbar .n {
    background: var(--alert);
    color: #fff;
    border-radius: 9px;
    font-size: 11px;
    font-weight: 700;
    padding: 0 6px;
  }
  .banner {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 18px;
    background: var(--warn-soft);
    border-bottom: 1px solid var(--warn);
    font-size: 13px;
  }
  .statusbar {
    flex: none;
    display: flex;
    gap: 2px;
    padding: 2px 4px;
    background: var(--bg);
    border-top: 1px solid var(--line-strong);
    font-size: 12px;
  }
  .statusbar .cell {
    padding: 1px 10px;
    border: 1px solid var(--line);
    border-radius: 3px;
    background: var(--panel-2);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .statusbar .demo {
    color: var(--warn);
    font-weight: 600;
  }
  .banner span {
    flex: 1;
  }
</style>
