<script lang="ts">
  import { app, type Page } from "../lib/state.svelte";
  import Icon from "./Icon.svelte";

  const items: { page: Page; label: string; icon: string }[] = [
    { page: "chats", label: "Chats", icon: "message" },
    { page: "mail", label: "Mail", icon: "mail" },
    { page: "stations", label: "Stations", icon: "map" },
    { page: "radio", label: "Radio", icon: "radio" },
    { page: "relay", label: "Relay", icon: "relay" },
  ];
  const count = (p: Page) => (p === "chats" ? app.unreadChats : p === "mail" ? app.unreadMail : 0);
</script>

<nav class="nav" aria-label="Sections" data-tour="nav">
  {#each items as it}
    <button class="item" class:on={app.page === it.page} onclick={() => app.go(it.page)} data-tour={it.page}>
      <Icon name={it.icon} size={16} />
      <span class="grow">{it.label}</span>
      {#if count(it.page) > 0}<span class="n mono">{count(it.page)}</span>{/if}
      {#if it.page === "relay" && app.relay.running}<span class="lamp good"></span>{/if}
    </button>
  {/each}
  <div class="spacer"></div>
  <div class="who">
    <div class="label">Station</div>
    <div class="call">{app.settings.callsign || "listening only"}</div>
    {#if app.engine.demo}<div class="demo small">Demo: pretend radio</div>{/if}
  </div>
</nav>

<style>
  .nav {
    width: 156px;
    flex: none;
    border-right: 1px solid var(--line);
    background: var(--panel);
    padding: 10px 8px 12px;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .item {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 10px;
    background: none;
    border: 0;
    border-left: 2px solid transparent;
    border-radius: 0 5px 5px 0;
    color: var(--muted);
    font: inherit;
    font-weight: 550;
    cursor: pointer;
    text-align: left;
  }
  .item:hover {
    color: var(--text);
    background: var(--hover);
  }
  .item.on {
    color: var(--text);
    background: var(--accent-soft);
    border-left-color: var(--accent);
  }
  .n {
    background: var(--accent);
    color: var(--accent-ink);
    border-radius: 3px;
    font-size: 11px;
    font-weight: 700;
    padding: 0 5px;
  }
  .who {
    padding: 10px;
    border-top: 1px solid var(--line);
  }
  .demo {
    margin-top: 6px;
    color: var(--accent);
  }
</style>
