<script lang="ts">
  import { app, type Page } from "../lib/state.svelte";
  import Icon from "./Icon.svelte";

  // Three places, not five: Chats and Mail live under Messages; Relay lives under Stations.
  const items: { pages: Page[]; label: string; icon: string }[] = [
    { pages: ["chats", "mail"], label: "Messages", icon: "message" },
    { pages: ["stations", "relay"], label: "Stations", icon: "map" },
    { pages: ["radio"], label: "Radio", icon: "radio" },
  ];
  const count = (pages: Page[]) => (pages.includes("chats") ? app.unreadChats + app.unreadMail : 0);
  const here = (pages: Page[]) => pages.includes(app.page);
  const open = (pages: Page[]) => app.go(here(pages) ? app.page : pages[0]);
</script>

<nav class="nav" aria-label="Sections" data-tour="nav">
  {#each items as it}
    <button class="item" class:on={here(it.pages)} onclick={() => open(it.pages)} aria-current={here(it.pages) ? "page" : undefined} data-tour={it.pages[0]}>
      <Icon name={it.icon} size={16} />
      <span class="grow">{it.label}</span>
      {#if count(it.pages) > 0}<span class="n mono" aria-label="{count(it.pages)} unread">{count(it.pages)}</span>{/if}
      {#if it.pages.includes("relay") && app.relay.running}<span class="lamp good"></span>{/if}
    </button>
  {/each}
</nav>

<style>
  .nav {
    width: 150px;
    flex: none;
    border-right: 1px solid var(--line);
    background: var(--bg);
    padding: 10px 8px 10px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .item {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 10px;
    background: none;
    border: 1px solid transparent;
    border-radius: 6px;
    color: var(--text);
    font: inherit;
    cursor: pointer;
    text-align: left;
  }
  .item:hover {
    background: var(--hover);
  }
  .item.on {
    background: var(--sel);
    color: var(--sel-ink);
    border-color: var(--edge);
    font-weight: 600;
  }
  .n {
    background: var(--alert);
    color: #fff;
    border-radius: 9px;
    font-size: 11px;
    font-weight: 700;
    padding: 0 6px;
  }
</style>
