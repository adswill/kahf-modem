<script lang="ts">
  import { app, sizeWords } from "../lib/state.svelte";
  import { callProblem, normaliseCall } from "../lib/callsign";
  import { clock, snrQuality, timeAgo } from "../lib/format";
  import type { MessageKind, Msg } from "../lib/types";
  import Icon from "../components/Icon.svelte";
  import Tip from "../components/Tip.svelte";
  import Wave from "../components/Wave.svelte";

  let to = $state(app.peer);
  let text = $state("");
  let kind = $state<MessageKind>("chat");
  let find = $state("");
  let box: HTMLDivElement | undefined = $state();
  let recSeconds = $state(0);
  let recTimer = 0;
  let playing = $state("");

  const peer = $derived(normaliseCall(to));
  const problem = $derived(callProblem(to));
  const thread = $derived(peer && !problem ? app.thread(peer) : []);
  const heardNow = $derived(app.heard.find((h) => h.call === peer) ?? null);
  const quality = $derived(snrQuality(app.link.state === "linked" && app.link.peer === peer ? (app.metrics?.ownSnr ?? null) : (heardNow?.snr ?? null)));
  const conv = $derived(app.conversations.filter((c) => c.peer.includes(find.trim().toUpperCase())));
  const air = $derived(app.onTheAir.filter((h) => h.call.includes(find.trim().toUpperCase())).slice(0, 12));
  const linked = $derived(app.link.state === "linked" && app.link.peer === peer);
  const route = $derived.by(() => {
    if (!peer) return "";
    if (linked) return "Direct link";
    if (heardNow) return "Heard directly. KAHF will contact them when you send.";
    return app.settings.relayHints.length ? `Not heard. KAHF will try a relay (${app.settings.relayHints[0]}).` : "Not heard yet. KAHF will try them directly.";
  });

  $effect(() => {
    app.peer = to;
    app.markChatsRead();
  });
  $effect(() => {
    thread.length;
    queueMicrotask(() => box?.scrollTo({ top: box.scrollHeight }));
  });

  const KINDS: { id: MessageKind; name: string; tip: string }[] = [
    { id: "chat", name: "Normal", tip: "A plain message. Anyone listening to the channel could read it." },
    { id: "private", name: "Private", tip: "Only the station you write to keeps it. Other stations that hear it ignore it. The radio signal itself is still open." },
    { id: "signed", name: "Signed", tip: "Adds your digital signature, so the reader can check it really is from you." },
    { id: "urgent", name: "Urgent", tip: "Goes to the front of the queue. Use it for real emergencies only." },
  ];

  const stateText = (m: Msg): string => {
    switch (m.state) {
      case "queued": return app.settings.autoLink ? "waiting for the channel" : "waiting";
      case "calling": return "reaching them";
      case "sending": return "sending";
      case "sent": return "sent";
      case "failed": return "not delivered";
      default: return "";
    }
  };

  async function submit() {
    if (!peer || problem || !text.trim()) return;
    await app.send(peer, text, kind);
    text = "";
  }

  function keydown(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      submit();
    }
  }

  async function attach() {
    if (!peer || problem) {
      app.toast("Choose who to send it to first.", "warn");
      return;
    }
    const picked = await app.engine.pickFiles();
    if (picked.length) app.askSendFiles(peer, picked);
  }

  async function record() {
    if (!peer || problem) {
      app.toast("Choose who to send the voice note to first.", "warn");
      return;
    }
    await app.startRecording();
    if (!app.recording) return;
    recSeconds = 0;
    recTimer = window.setInterval(async () => {
      recSeconds += 0.1;
      if (recSeconds >= 60) await sendRecording();
    }, 100);
  }

  async function sendRecording() {
    clearInterval(recTimer);
    const clip = await app.stopRecording();
    if (clip && peer) await app.sendVoice(peer, clip);
    else app.toast("That was too short to send.", "info");
  }

  async function cancelRecording() {
    clearInterval(recTimer);
    await app.cancelRecording();
  }

  async function play(m: Msg) {
    if (!m.attachment) return;
    playing = playing === m.id ? "" : m.id;
    if (playing) {
      try {
        await app.engine.playVoice(m.attachment.path ?? "");
      } catch (e) {
        app.toast(`Could not play it: ${e}`, "warn");
      }
      window.setTimeout(() => (playing = playing === m.id ? "" : playing), (m.attachment.seconds ?? 5) * 1000);
    }
  }

  const mmss = (s: number) => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, "0")}`;
</script>

<div class="chats">
  <aside class="people" aria-label="People">
    <div class="find">
      <Icon name="search" size={14} />
      <input type="search" placeholder="Find a callsign" bind:value={find} aria-label="Find a callsign" />
    </div>

    <div class="scroll">
      {#if conv.length}
        <div class="label head">Conversations</div>
        {#each conv as c (c.peer)}
          <button class="person" class:on={peer === c.peer} onclick={() => (to = c.peer)}>
            <div class="row">
              <span class="call grow">{c.peer}</span>
              {#if c.unread}<span class="n mono">{c.unread}</span>{/if}
              <span class="faint small">{clock(c.last.at)}</span>
            </div>
            <div class="muted small clip">
              {c.last.dir === "out" ? "You: " : ""}{c.last.attachment ? (c.last.attachment.kind === "voice" ? "Voice note" : c.last.attachment.name) : c.last.text}
            </div>
          </button>
        {/each}
      {/if}

      {#if air.length}
        <div class="label head">On the air now <Tip text="Stations KAHF has heard on your channel that you have not written to yet." /></div>
        {#each air as h (h.call)}
          {@const q = snrQuality(h.snr)}
          <button class="person" class:on={peer === h.call} onclick={() => (to = h.call)}>
            <div class="row">
              <span class="call grow">{h.call}</span>
              <span class="tag {q.tone}">{q.label}</span>
            </div>
            <div class="faint small">heard {timeAgo(h.at)}</div>
          </button>
        {/each}
      {/if}

      {#if !conv.length && !air.length}
        <p class="faint small pad">Nobody yet. Stations appear here when KAHF hears them.</p>
      {/if}
    </div>
  </aside>

  <section class="thread">
    <header class="top">
      <label class="to">
        <span class="label">To</span>
        <input
          type="text"
          placeholder="Callsign, like KA1ABC"
          autocapitalize="characters"
          spellcheck="false"
          bind:value={to}
          oninput={() => (to = to.toUpperCase())}
          data-tour="to"
        />
      </label>
      {#if peer && !problem}
        <span class="tag {quality.tone}">{quality.label}{#if linked && app.metrics?.ownSnr != null}&nbsp;{app.metrics.ownSnr} dB{/if}</span>
        {#if linked}
          <span class="faint small">{app.metrics?.mode} · {app.metrics?.netBps} bit/s</span>
          <button class="btn sm" onclick={() => app.hangUp()}>Hang up</button>
        {:else}
          <button class="btn sm" onclick={() => app.call(peer)} disabled={app.settings.listenOnly}>Connect now</button>
        {/if}
      {/if}
    </header>

    <div class="bubbles" bind:this={box}>
      {#if !peer || problem}
        <div class="start">
          <h1>Start a conversation</h1>
          <p class="muted">Pick someone on the left, or type their callsign above. Then write below. KAHF handles the radio.</p>
          <ol class="steps">
            {#each app.steps as s}
              <li class:done={s.done}>
                <span class="box mono">{s.done ? "✓" : ""}</span>
                <div><b>{s.title}</b><div class="muted small">{s.why}</div></div>
              </li>
            {/each}
          </ol>
          <div class="row">
            <button class="btn" onclick={() => (app.tourOpen = true)}><Icon name="play" size={14} /> 60-second tour</button>
            <button class="btn quiet" onclick={() => app.openHelp("how")}>How KAHF works</button>
          </div>
        </div>
      {:else if thread.length === 0}
        <div class="start">
          <p class="muted">No messages with <span class="call">{peer}</span> yet.</p>
          <p class="faint small">{route}</p>
        </div>
      {/if}

      {#each thread as m (m.id)}
        <div class="msg {m.dir}" class:fail={m.state === "failed"}>
          {#if m.attachment?.kind === "voice"}
            <div class="voice">
              <button class="btn icon sm" onclick={() => play(m)} aria-label={playing === m.id ? "Pause" : "Play"}>
                <Icon name={playing === m.id ? "pause" : "play"} size={14} />
              </button>
              <Wave seed={m.id} progress={playing === m.id ? 1 : 0} />
              <span class="mono small">{mmss(m.attachment.seconds ?? 0)}</span>
            </div>
          {:else if m.attachment}
            <div class="file">
              <Icon name="file" size={22} />
              <div class="grow">
                <div class="fn">{m.attachment.name}</div>
                <div class="faint small">
                  {sizeWords(m.attachment.size)}
                  {#if m.dir === "out" && m.state !== "sent" && m.state !== "failed" && m.attachment.done}· {Math.round(((m.attachment.done ?? 0) / Math.max(1, m.attachment.size)) * 100)}%{/if}
                </div>
                {#if m.dir === "out" && m.state === "sending"}
                  <div class="prog"><i style:width={`${Math.min(100, ((m.attachment.done ?? 0) / Math.max(1, m.attachment.size)) * 100)}%`}></i></div>
                {/if}
              </div>
              {#if m.dir === "in" && m.attachment.path}
                <button class="btn sm" onclick={() => app.engine.openPath(m.attachment!.path!)}>Open</button>
              {/if}
            </div>
          {/if}
          {#if m.text}<div class="bt">{m.text}</div>{/if}
          <div class="meta small faint mono">
            {#if m.kind !== "chat"}<span class="tag warn">{m.kind}</span>{/if}
            <span>{clock(m.at)}</span>
            {#if m.dir === "out"}<span class:okc={m.state === "sent"} class:badc={m.state === "failed"}>· {stateText(m)}</span>{/if}
          </div>
          {#if m.state === "failed"}
            <div class="why small">
              {m.note ?? "The other station did not answer."}
              <div class="row">
                <button class="btn sm" onclick={() => { app.discard(m.id); if (m.attachment) return; app.send(m.peer, m.text, m.kind); }}>Try again</button>
                <button class="btn sm quiet" onclick={() => app.discard(m.id)}>Delete</button>
              </div>
            </div>
          {/if}
        </div>
      {/each}
    </div>

    <footer class="compose">
      {#if app.settings.listenOnly}
        <div class="note warn">Listen-only is on, so KAHF cannot send. Turn it off in Settings, under Sending.</div>
      {/if}
      {#if app.recording}
        <div class="rec">
          <span class="lamp bad blink"></span>
          <span class="mono">{mmss(recSeconds)}</span>
          <div class="level"><i style:width={`${Math.round(app.recordLevel * 100)}%`}></i></div>
          <span class="faint small">about {Math.round(recSeconds * 88)} bytes so far</span>
          <div class="spacer"></div>
          <button class="btn sm quiet" onclick={cancelRecording}>Cancel</button>
          <button class="btn sm primary" onclick={sendRecording}><Icon name="send" size={13} /> Send voice note</button>
        </div>
      {:else}
        <div class="row">
          <button class="btn icon quiet" onclick={attach} disabled={!peer || !!problem} title="Send a file" aria-label="Send a file"><Icon name="clip" size={18} /></button>
          <textarea
            rows="1"
            placeholder={peer && !problem ? `Message ${peer}` : "Choose who to write to"}
            bind:value={text}
            onkeydown={keydown}
            disabled={!peer || !!problem}
            data-tour="compose"
          ></textarea>
          <button class="btn icon quiet" onclick={record} disabled={!peer || !!problem} title="Record a voice note" aria-label="Record a voice note"><Icon name="mic" size={18} /></button>
          <button class="btn primary" onclick={submit} disabled={!peer || !!problem || !text.trim() || app.settings.listenOnly}><Icon name="send" size={14} /> Send</button>
        </div>
        <div class="row opts small">
          <span class="faint">Send as</span>
          <div class="tabs">
            {#each KINDS as k}
              <button class:on={kind === k.id} onclick={() => (kind = k.id)} title={k.tip}>{k.name}</button>
            {/each}
          </div>
          <Tip text={KINDS.find((k) => k.id === kind)!.tip} />
          <div class="spacer"></div>
          <span class="faint">{route}</span>
        </div>
      {/if}
    </footer>
  </section>
</div>

<style>
  .chats {
    height: 100%;
    display: grid;
    grid-template-columns: 270px 1fr;
    min-height: 0;
  }
  .people {
    border-right: 1px solid var(--line);
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .find {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--line);
    color: var(--faint);
  }
  .find input {
    border: 0;
    background: none;
    padding: 2px 0;
  }
  .scroll {
    overflow: auto;
    flex: 1;
  }
  .head {
    padding: 14px 14px 6px;
  }
  .pad {
    padding: 14px;
  }
  .person {
    width: 100%;
    text-align: left;
    background: none;
    border: 0;
    border-bottom: 1px solid var(--line);
    border-left: 2px solid transparent;
    color: var(--text);
    font: inherit;
    padding: 9px 14px;
    cursor: pointer;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .person:hover {
    background: var(--hover);
  }
  .person.on {
    background: var(--accent-soft);
    border-left-color: var(--accent);
  }
  .clip {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .n {
    background: var(--accent);
    color: var(--accent-ink);
    border-radius: 3px;
    font-size: 11px;
    font-weight: 700;
    padding: 0 5px;
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
    padding: 9px 18px;
    border-bottom: 1px solid var(--line);
    min-height: 51px;
  }
  .to {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .to input {
    width: 200px;
    font-family: ui-monospace, Menlo, monospace;
    font-weight: 600;
  }
  .bubbles {
    flex: 1;
    overflow: auto;
    padding: 18px 22px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .start {
    margin: auto;
    width: min(460px, 100%);
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .steps {
    list-style: none;
    margin: 4px 0;
    padding: 0;
    border-top: 1px solid var(--line);
  }
  .steps li {
    display: flex;
    gap: 12px;
    padding: 11px 0;
    border-bottom: 1px solid var(--line);
  }
  .steps .box {
    width: 20px;
    height: 20px;
    flex: none;
    border: 1px solid var(--line-strong);
    border-radius: 4px;
    display: grid;
    place-items: center;
    color: var(--accent-ink);
    font-size: 13px;
    margin-top: 1px;
  }
  .steps li.done .box {
    background: var(--accent);
    border-color: var(--accent);
  }
  .steps li.done b {
    color: var(--muted);
    text-decoration: line-through;
  }
  .msg {
    max-width: 68%;
    padding: 8px 11px;
    border: 1px solid var(--line);
    border-radius: 8px 8px 8px 2px;
    background: var(--panel);
    align-self: flex-start;
  }
  .msg.out {
    align-self: flex-end;
    border-radius: 8px 8px 2px 8px;
    background: var(--panel-2);
    border-color: var(--line-strong);
  }
  .msg.fail {
    border-color: var(--bad);
  }
  .bt {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .meta {
    display: flex;
    gap: 6px;
    margin-top: 3px;
    align-items: center;
  }
  .okc {
    color: var(--good);
  }
  .badc {
    color: var(--bad);
  }
  .why {
    margin-top: 6px;
    color: var(--muted);
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .voice,
  .file {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 240px;
  }
  .fn {
    font-weight: 600;
    overflow-wrap: anywhere;
  }
  .prog {
    height: 3px;
    background: var(--line);
    border-radius: 2px;
    margin-top: 5px;
    overflow: hidden;
  }
  .prog i {
    display: block;
    height: 100%;
    background: var(--accent);
  }
  .compose {
    padding: 10px 18px 12px;
    border-top: 1px solid var(--line);
    display: flex;
    flex-direction: column;
    gap: 8px;
    background: var(--panel);
  }
  .compose textarea {
    flex: 1;
    min-height: 36px;
    max-height: 120px;
  }
  .opts {
    min-height: 26px;
  }
  .rec {
    display: flex;
    align-items: center;
    gap: 12px;
    min-height: 40px;
  }
  .level {
    width: 140px;
    height: 6px;
    background: var(--line);
    border-radius: 3px;
    overflow: hidden;
  }
  .level i {
    display: block;
    height: 100%;
    background: var(--accent);
    transition: width 0.08s;
  }
</style>
