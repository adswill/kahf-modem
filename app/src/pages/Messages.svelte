<script lang="ts">
  import { app } from "../lib/state.svelte";
  import { callProblem, normaliseCall } from "../lib/callsign";
  import { clock, timeAgo } from "../lib/format";
  import type { MessageKind, Msg } from "../lib/types";
  import Icon from "../components/Icon.svelte";

  let to = $state(app.peer);
  let text = $state("");
  let kind = $state<MessageKind>("chat");
  let more = $state(false);
  let box: HTMLDivElement | undefined = $state();

  const peer = $derived(normaliseCall(to));
  const problem = $derived(callProblem(to));
  const thread = $derived(peer ? app.thread(peer) : []);
  const known = $derived(app.conversations.map((c) => c.peer));
  const suggestions = $derived([...new Set([...known, ...app.heard.map((h) => h.call)])].slice(0, 8));

  $effect(() => {
    app.peer = to;
    app.markRead();
  });

  $effect(() => {
    thread.length;
    queueMicrotask(() => box?.scrollTo({ top: box.scrollHeight }));
  });

  const KINDS: { id: MessageKind; name: string; text: string }[] = [
    { id: "chat", name: "Normal", text: "A plain message. Anyone listening may read it." },
    { id: "private", name: "Private", text: "Only the station you write to keeps it. Others who hear it ignore it." },
    { id: "urgent", name: "Urgent", text: "Goes first in the queue. Use it for real emergencies only." },
    { id: "signed", name: "Signed", text: "Adds your digital signature so the reader knows it is really from you." },
  ];

  async function submit() {
    if (!peer || !text.trim() || problem) return;
    await app.send(peer, text, kind);
    text = "";
  }

  function keydown(e: KeyboardEvent) {
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey || !e.shiftKey)) {
      e.preventDefault();
      submit();
    }
  }

  const label = (m: Msg): string => {
    switch (m.state) {
      case "queued": return app.settings.autoLink ? "Waiting for a turn on the air" : "Waiting. Connect to send it";
      case "calling": return "Calling them…";
      case "sending": return "Sending…";
      case "sent": return "Sent over the air";
      case "failed": return "Not delivered";
      default: return "";
    }
  };
</script>

<div class="msgs">
  <aside class="list" aria-label="Conversations">
    <div class="list-head">
      <h2>Messages</h2>
      <button class="btn small" onclick={() => { to = ""; text = ""; }}>New</button>
    </div>
    {#if app.conversations.length === 0}
      <p class="muted small pad">No conversations yet. Choose "New" to write to someone.</p>
    {/if}
    {#each app.conversations as c (c.peer)}
      <button class="conv" class:on={peer === c.peer} onclick={() => (to = c.peer)}>
        <div class="row">
          <span class="callsign grow">{c.peer}</span>
          {#if c.unread}<span class="badge">{c.unread}</span>{/if}
          <span class="faint small">{clock(c.last.at)}</span>
        </div>
        <div class="muted small clip">{c.last.dir === "out" ? "You: " : ""}{c.last.text}</div>
      </button>
    {/each}
  </aside>

  <section class="thread">
    <header class="top">
      <label class="to">
        <span class="muted small">To</span>
        <input
          type="text"
          list="calls"
          placeholder="A callsign, like KA1ABC"
          autocapitalize="characters"
          spellcheck="false"
          bind:value={to}
          oninput={() => (to = to.toUpperCase())}
          data-tour="to"
        />
        <datalist id="calls">{#each suggestions as c}<option value={c}></option>{/each}</datalist>
      </label>
      {#if peer && !problem}
        {#if app.link.state === "linked" && app.link.peer === peer}
          <span class="chip good">Connected</span>
          <button class="btn small" onclick={() => app.hangUp()}>Hang up</button>
        {:else}
          <button class="btn small" onclick={() => app.call(peer)} disabled={app.settings.listenOnly}>Call now</button>
        {/if}
      {/if}
    </header>
    {#if problem}<p class="field-error pad">{problem}</p>{/if}

    <div class="bubbles" bind:this={box}>
      {#if !peer}
        <div class="blank">
          <Icon name="message" size={34} />
          <h2>Write to someone</h2>
          <p class="muted">
            Type a callsign above. Stations you have heard are suggested. You do not need to connect first:
            KAHF calls them for you when you press Send.
          </p>
          {#if suggestions.length}
            <div class="row wrap">
              {#each suggestions.slice(0, 5) as c}
                <button class="btn small" onclick={() => (to = c)}>{c}</button>
              {/each}
            </div>
          {/if}
        </div>
      {:else if thread.length === 0}
        <div class="blank">
          <p class="muted">No messages with <span class="callsign">{peer}</span> yet. Write the first one below.</p>
        </div>
      {/if}
      {#each thread as m (m.id)}
        <div class="bubble {m.dir}" class:fail={m.state === "failed"}>
          <div class="bt">{m.text}</div>
          <div class="meta small">
            {#if m.kind !== "chat"}<span class="chip info">{m.kind}</span>{/if}
            <span>{clock(m.at)}</span>
            {#if m.dir === "out"}
              <span class={m.state === "failed" ? "bad" : m.state === "sent" ? "ok" : ""}>{label(m)}</span>
            {/if}
          </div>
          {#if m.state === "failed"}
            <div class="why small">
              {m.note ?? "The other station did not answer."}
              <div class="row">
                <button class="btn small" onclick={() => { app.discard(m.id); app.send(m.peer, m.text, m.kind); }}>Try again</button>
                <button class="btn small ghost" onclick={() => app.discard(m.id)}>Delete</button>
              </div>
            </div>
          {/if}
        </div>
      {/each}
    </div>

    <footer class="compose">
      {#if app.settings.listenOnly}
        <div class="notice warn">
          <Icon name="warn" />
          <span>Listen-only is on, so KAHF cannot send. Turn it off in Settings when you are ready.</span>
        </div>
      {/if}
      {#if more}
        <div class="kinds">
          {#each KINDS as k}
            <button class="choice" class:on={kind === k.id} onclick={() => (kind = k.id)}>
              <span class="dot"></span>
              <span><strong>{k.name}</strong><span class="sub">{k.text}</span></span>
            </button>
          {/each}
        </div>
      {/if}
      <div class="row">
        <textarea
          rows="2"
          placeholder={peer ? `Write to ${peer}…` : "Choose who to write to first"}
          bind:value={text}
          onkeydown={keydown}
          disabled={!peer || !!problem}
          data-tour="compose"
        ></textarea>
        <div class="stack-sm">
          <button class="btn primary" onclick={submit} disabled={!peer || !!problem || !text.trim() || app.settings.listenOnly}>
            <Icon name="send" /> Send
          </button>
          <button class="btn ghost small" onclick={() => (more = !more)}>{more ? "Fewer options" : "More options"}</button>
        </div>
      </div>
      <div class="faint small">Press Enter to send, Shift+Enter for a new line. Long messages are split into small pieces automatically.</div>
    </footer>
  </section>
</div>

<style>
  .msgs {
    height: 100%;
    display: grid;
    grid-template-columns: 300px 1fr;
  }
  .list {
    border-right: 1px solid var(--line-soft);
    overflow: auto;
    padding: 18px 12px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .list-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0 6px 8px;
  }
  .pad {
    padding: 6px 8px;
  }
  .conv {
    text-align: left;
    background: none;
    border: 0;
    color: var(--text);
    font: inherit;
    padding: 11px 12px;
    border-radius: 11px;
    cursor: pointer;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .conv:hover {
    background: var(--raised);
  }
  .conv.on {
    background: var(--accent-soft);
  }
  .clip {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .badge {
    background: var(--accent);
    color: var(--accent-ink);
    border-radius: 99px;
    font-size: 12px;
    font-weight: 700;
    padding: 0 8px;
  }
  .thread {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }
  .top {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 16px 24px;
    border-bottom: 1px solid var(--line-soft);
  }
  .to {
    display: flex;
    align-items: center;
    gap: 10px;
    flex: 1;
  }
  .to input {
    max-width: 280px;
    font-weight: 600;
  }
  .bubbles {
    flex: 1;
    overflow: auto;
    padding: 20px 24px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .blank {
    margin: auto;
    max-width: 440px;
    text-align: center;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    color: var(--faint);
  }
  .blank h2 {
    color: var(--text);
  }
  .bubble {
    max-width: 66%;
    padding: 10px 14px;
    border-radius: 16px;
    background: var(--panel-2);
    border: 1px solid var(--line-soft);
    align-self: flex-start;
  }
  .bubble.out {
    align-self: flex-end;
    background: var(--accent-soft);
    border-color: transparent;
  }
  .bubble.fail {
    background: var(--red-soft);
  }
  .bt {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .meta {
    display: flex;
    gap: 8px;
    align-items: center;
    margin-top: 4px;
    color: var(--faint);
  }
  .meta .ok {
    color: var(--accent);
  }
  .meta .bad {
    color: var(--red);
  }
  .why {
    margin-top: 8px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    color: var(--muted);
  }
  .compose {
    padding: 14px 24px 18px;
    border-top: 1px solid var(--line-soft);
    display: flex;
    flex-direction: column;
    gap: 10px;
    background: var(--panel);
  }
  .compose textarea {
    flex: 1;
  }
  .kinds {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
    gap: 8px;
  }
  .kinds .choice {
    padding: 10px 12px;
  }
</style>
