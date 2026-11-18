<script lang="ts">
  import { app, type Page } from "../lib/state.svelte";
  import Icon from "./Icon.svelte";

  const items: { page: Page; label: string; icon: string; hint: string }[] = [
    { page: "home", label: "Home", icon: "home", hint: "Start here" },
    { page: "messages", label: "Messages", icon: "message", hint: "Write and read" },
    { page: "network", label: "Network", icon: "network", hint: "Who is on the air" },
    { page: "radio", label: "Radio", icon: "radio", hint: "Signal and speed" },
  ];
  const bottom: { page: Page; label: string; icon: string }[] = [
    { page: "help", label: "Help", icon: "help" },
    { page: "settings", label: "Settings", icon: "settings" },
  ];
</script>

<nav class="side" aria-label="Main">
  <div class="brand">
    <div class="logo" aria-hidden="true">K</div>
    <div>
      <strong>KAHF</strong>
      <div class="faint small">Radio messages</div>
    </div>
  </div>

  <div class="nav">
    {#each items as it}
      <button class="item" class:on={app.page === it.page} onclick={() => app.go(it.page)} data-tour={it.page}>
        <Icon name={it.icon} />
        <span class="label">{it.label}</span>
        {#if it.page === "messages" && app.unread > 0}
          <span class="badge">{app.unread}</span>
        {/if}
      </button>
    {/each}
  </div>

  <div class="spacer"></div>

  <div class="nav">
    {#each bottom as it}
      <button class="item" class:on={app.page === it.page} onclick={() => app.go(it.page)} data-tour={it.page}>
        <Icon name={it.icon} />
        <span class="label">{it.label}</span>
      </button>
    {/each}
  </div>

  <div class="who" data-tour="status">
    <span class="dot-live {app.status.tone}"></span>
    <div class="grow">
      <div class="title">{app.status.title}</div>
      <div class="faint small callsign">{app.settings.callsign || "no callsign"}</div>
    </div>
  </div>
  {#if app.engine.demo}
    <div class="demo small">Demo mode: a pretend radio</div>
  {/if}
</nav>

<style>
  .side {
    width: 244px;
    flex: none;
    background: var(--panel);
    border-right: 1px solid var(--line-soft);
    padding: 20px 14px 14px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 4px 8px 18px;
  }
  .logo {
    width: 38px;
    height: 38px;
    border-radius: 11px;
    background: var(--accent);
    color: var(--accent-ink);
    display: grid;
    place-items: center;
    font-weight: 800;
    font-size: 20px;
  }
  .nav {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .item {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 11px 12px;
    border-radius: 10px;
    border: 0;
    background: none;
    color: var(--muted);
    font: inherit;
    font-weight: 600;
    cursor: pointer;
    text-align: left;
  }
  .item:hover {
    background: var(--raised);
    color: var(--text);
  }
  .item.on {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .label {
    flex: 1;
  }
  .badge {
    background: var(--accent);
    color: var(--accent-ink);
    border-radius: 999px;
    font-size: 12px;
    font-weight: 700;
    padding: 1px 8px;
  }
  .spacer {
    flex: 1;
  }
  .who {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px;
    margin-top: 8px;
    border-radius: 12px;
    background: var(--panel-2);
    border: 1px solid var(--line-soft);
  }
  .title {
    font-weight: 600;
    font-size: 14px;
  }
  .demo {
    text-align: center;
    color: var(--amber);
    background: var(--amber-soft);
    border-radius: 8px;
    padding: 5px;
  }
</style>
