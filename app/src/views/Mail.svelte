<script lang="ts">
  import FileNotice from "../components/FileNotice.svelte";
  import { app, sizeWords } from "../lib/state.svelte";
  import { callProblem, normaliseCall } from "../lib/callsign";
  import { FILE_LIMIT_BYTES, clock, timeAgo } from "../lib/format";
  import type { MailItem, PickedFile } from "../lib/types";
  import Icon from "../components/Icon.svelte";
  import Tip from "../components/Tip.svelte";

  type Folder = "inbox" | "sent";
  let folder = $state<Folder>("inbox");
  let open = $state<string>("");
  let composing = $state(false);

  let to = $state("");
  let subject = $state("");
  let body = $state("");
  let files = $state<PickedFile[]>([]);

  const list = $derived(
    app.mail
      .filter((m) => (folder === "inbox" ? m.dir === "in" : m.dir === "out"))
      .sort((a, b) => b.at - a.at),
  );
  const current = $derived<MailItem | undefined>(app.mail.find((m) => m.id === open));
  const problem = $derived(callProblem(to));
  const ready = $derived(normaliseCall(to) !== "" && !problem && (body.trim() !== "" || files.length > 0) && files.reduce((n, f) => n + f.size, 0) <= FILE_LIMIT_BYTES && !app.settings.listenOnly);

  $effect(() => {
    if (current && current.dir === "in" && !current.read) app.markMailRead();
  });

  function newMail(reply?: MailItem) {
    composing = true;
    open = "";
    to = reply?.peer ?? "";
    subject = reply ? (reply.subject.startsWith("Re: ") ? reply.subject : `Re: ${reply.subject}`) : "";
    body = reply ? `\n\n— ${reply.peer} wrote:\n${reply.body.split("\n").map((l) => `> ${l}`).join("\n")}` : "";
    files = [];
  }

  async function attach() {
    files = [...files, ...(await app.engine.pickFiles())];
  }

  async function send() {
    if (!ready) return;
    if (await app.sendMail(to, subject, body, files)) {
      composing = false;
      folder = "sent";
      to = subject = body = "";
      files = [];
    }
  }

  const stateText = (s?: string) =>
    ({ queued: "waiting for the channel", calling: "reaching them", sending: "sending", atrelay: "a relay has it", delivered: "delivered", waiting: "no answer yet, trying again", sent: "sent", failed: "not delivered" })[s ?? ""] ?? "";
</script>

<div class="mail">
  <aside class="folders">
    <div class="pad"><button class="btn primary" style="width:100%" onclick={() => newMail()}><Icon name="mail" size={14} /> Write mail</button></div>
    <button class="folder" class:on={folder === "inbox" && !composing} onclick={() => { folder = "inbox"; composing = false; }}>
      Inbox {#if app.unreadMail}<span class="n mono">{app.unreadMail}</span>{/if}
    </button>
    <button class="folder" class:on={folder === "sent" && !composing} onclick={() => { folder = "sent"; composing = false; }}>Sent</button>
    <div class="spacer"></div>
    <p class="faint small pad">
      Mail is for longer messages with a subject and attachments. For quick lines, use Chats.
    </p>
  </aside>

  <section class="listing">
    <div class="label head">{folder === "inbox" ? "Inbox" : "Sent"}</div>
    {#each list as m (m.id)}
      <button class="item" class:on={open === m.id} class:unread={m.dir === "in" && !m.read} onclick={() => { open = m.id; composing = false; }}>
        <div class="row">
          <span class="call grow">{m.peer}</span>
          <span class="faint small">{clock(m.at)}</span>
        </div>
        <div class="subj clip">{m.subject}</div>
        <div class="faint small clip">{m.body.replace(/\s+/g, " ").slice(0, 70)}</div>
      </button>
    {:else}
      <p class="faint small pad">
        {folder === "inbox" ? "No mail yet. Mail from other stations arrives here by itself." : "Nothing sent yet."}
      </p>
    {/each}
  </section>

  <section class="reader">
    {#if composing}
      <div class="form">
        <h1>New mail</h1>
        <label class="field">
          <span class="t">To</span>
          <input type="text" placeholder="Callsign, like KA1ABC" bind:value={to} oninput={() => (to = to.toUpperCase())} />
          {#if problem}<span class="err">{problem}</span>{/if}
        </label>
        <label class="field">
          <span class="t">Subject</span>
          <input type="text" bind:value={subject} />
        </label>
        <label class="field grow">
          <span class="t">Message</span>
          <textarea rows="10" bind:value={body}></textarea>
        </label>
        <div class="col tight">
          <div class="row">
            <button class="btn sm" onclick={attach}><Icon name="clip" size={14} /> Attach a file</button>
            <Tip text="Attached files are sent after the mail. Larger files take longer: KAHF shows the progress in the conversation." />
          </div>
          {#each files as f, i}
            <div class="att row">
              <Icon name="file" size={16} /> <span class="grow">{f.name}</span> <span class="faint small">{sizeWords(f.size)}</span>
              <button class="btn quiet sm" onclick={() => (files = files.filter((_, j) => j !== i))}>Remove</button>
            </div>
          {/each}
          <FileNotice {files} />
        </div>
        <div class="row">
          <button class="btn primary" onclick={send} disabled={!ready}><Icon name="send" size={14} /> Send mail</button>
          <button class="btn quiet" onclick={() => (composing = false)}>Discard</button>
          {#if app.settings.listenOnly}<span class="err">Listen-only is on. Turn it off in Settings to send.</span>{/if}
        </div>
      </div>
    {:else if current}
      <article class="read">
        <header>
          <h1>{current.subject}</h1>
          <div class="row small muted">
            <span>{current.dir === "in" ? "From" : "To"} <span class="call">{current.peer}</span></span>
            <span>·</span><span>{timeAgo(current.at)}</span>
            {#if current.dir === "out"}<span class="tag {current.state === 'sent' || current.state === 'delivered' || current.state === 'atrelay' ? 'good' : current.state === 'failed' ? 'bad' : 'warn'}">{stateText(current.state)}</span>{/if}
          </div>
        </header>
        <hr class="rule" />
        <pre class="bodytext">{current.body}</pre>
        {#if current.files.length}
          <div class="col tight">
            <div class="label">Attachments</div>
            {#each current.files as f}
              <div class="att row">
                <Icon name="file" size={16} /> <span class="grow">{f.name}</span> <span class="faint small">{sizeWords(f.size)}</span>
                {#if f.path}<button class="btn sm" onclick={() => app.engine.openPath(f.path!)}>Open</button>{/if}
              </div>
            {/each}
          </div>
        {/if}
        {#if current.state === "failed"}<div class="note bad">{current.note ?? "The other station did not answer."}</div>{/if}
        {#if current.dir === "in"}
          <div class="row"><button class="btn" onclick={() => newMail(current)}>Reply</button></div>
        {/if}
      </article>
    {:else}
      <p class="blank muted">No message selected. Pick one on the left, or write a new one.</p>
    {/if}
  </section>
</div>

<style>
  .mail {
    height: 100%;
    display: grid;
    grid-template-columns: 150px 290px 1fr;
    min-height: 0;
  }
  .folders {
    border-right: 1px solid var(--line);
    display: flex;
    flex-direction: column;
  }
  .pad {
    padding: 12px;
  }
  .folder {
    background: none;
    border: 0;
    color: var(--muted);
    font: inherit;
    font-weight: 550;
    text-align: left;
    padding: 8px 14px;
    cursor: pointer;
    display: flex;
    justify-content: space-between;
  }
  .folder:hover {
    background: var(--hover);
    color: var(--text);
  }
  .folder.on {
    color: var(--text);
    background: var(--hover);
    box-shadow: inset 0 0 0 1px var(--accent);
  }
  .n {
    background: var(--accent);
    color: var(--accent-ink);
    border-radius: 3px;
    font-size: 11px;
    font-weight: 700;
    padding: 0 5px;
  }
  .listing {
    border-right: 1px solid var(--line);
    overflow: auto;
  }
  .head {
    padding: 14px 14px 8px;
  }
  .item {
    width: 100%;
    text-align: left;
    background: none;
    border: 0;
    border-bottom: 1px solid var(--line);
    color: var(--text);
    font: inherit;
    padding: 9px 14px;
    cursor: pointer;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .item:hover {
    background: var(--hover);
  }
  .item.on {
    background: var(--hover);
    box-shadow: inset 0 0 0 1px var(--accent);
  }
  .item.unread .subj {
    font-weight: 700;
  }
  .clip {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .reader {
    overflow: auto;
    min-width: 0;
  }
  .form,
  .read {
    padding: 22px 28px;
    max-width: 760px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .bodytext {
    font: inherit;
    white-space: pre-wrap;
    margin: 0;
    line-height: 1.55;
  }
  .att {
    padding: 6px 9px;
    border: 1px solid var(--line);
    border-radius: 6px;
  }
  .blank {
    margin: 0;
    padding: 14px 18px;
  }
</style>
