<script lang="ts">
  import { app } from "../lib/state.svelte";
  import { callProblem, normaliseCall } from "../lib/callsign";
  import { channelKhz, clock, duration, relative, timeAgo, utcHm } from "../lib/format";
  import Icon from "../components/Icon.svelte";
  import Tip from "../components/Tip.svelte";

  let tab = $state<"use" | "run">(app.relay.running ? "run" : "use");
  let licence = $state(false);
  let terms = $state(false);
  let manual = $state("");
  let busy = $state(false);
  let checking = $state(false);
  let serverCheck = $state<import("../lib/types").ServerCheck | null>(null);
  const hourMax = $derived(Math.max(1, ...(app.relay.perHour ?? []).map(([, n]) => n)));

  async function checkServer() {
    checking = true;
    try {
      serverCheck = await app.engine.serverStatus();
    } finally {
      checking = false;
    }
  }

  const hints = $derived(app.settings.relayHints);
  const manualProblem = $derived(callProblem(manual));
  const sharing = $derived(app.settings.location.mode !== "off" && app.settings.location.lat !== null);

  async function toggleUse(call: string) {
    const next = hints.includes(call) ? hints.filter((c) => c !== call) : [...hints, call];
    await app.setRelayHints(next);
  }

  async function addManual() {
    const c = normaliseCall(manual);
    if (!c || manualProblem) return;
    if (!hints.includes(c)) await app.setRelayHints([...hints, c]);
    manual = "";
  }

  async function startRelay() {
    busy = true;
    await app.setRelay(true, licence, terms);
    busy = false;
  }

  async function stopRelay() {
    busy = true;
    await app.setRelay(false, licence, terms);
    busy = false;
  }
</script>

<div class="view">
  <div class="head row">
    <h1>Relay</h1>
    <div class="tabs">
      <button class:on={tab === "use"} onclick={() => (tab = "use")}>Use relays</button>
      <button class:on={tab === "run"} onclick={() => (tab = "run")}>
        Run a relay {#if app.relay.running}<span class="lamp good"></span>{/if}
      </button>
    </div>
  </div>

  {#if tab === "use"}
    <section class="col">
      <p class="muted lead">
        A relay is a station that stays on the air and passes messages along, so people who cannot hear each other can still talk.
        When a station does not answer, KAHF tries the relays you chose here, in order.
      </p>
      <div class="flow" aria-label="How a relay passes a message on">
        <div><b>You</b><span class="faint small">write</span></div>
        <Icon name="arrow" /> <div class="rl"><b>Relay</b><span class="faint small">passes it on</span></div>
        <Icon name="arrow" /> <div><b>Them</b><span class="faint small">receive</span></div>
      </div>

      <div class="row">
        <h2>Relays I know</h2>
        <Tip text="This list comes from kahf.dev when you are online, and is remembered for when you are not. KAHF works without the internet: relays you add by callsign are used the same way." />
        <div class="spacer"></div>
        <button class="btn sm" onclick={() => app.refreshRelayDirectory(true)}><Icon name="refresh" size={13} /> Update from kahf.dev</button>
      </div>

      {#if app.relays.length === 0 && hints.length === 0}
        <div class="note">No relays known yet. Update from kahf.dev when you are online, or add one by callsign below.</div>
      {/if}

      <table>
        <thead><tr><th>Relay</th><th>Distance</th><th>Source</th><th>Seen</th><th class="r">Use it</th></tr></thead>
        <tbody>
          {#each app.relays as r (r.call)}
            <tr>
              <td class="call">{r.call}</td>
              <td class="mono">{r.km !== null ? `${Math.round(r.km)} km` : "-"}</td>
              <td><span class="tag">{r.source === "server" ? "kahf.dev" : r.source === "heard" ? "heard" : "added"}</span></td>
              <td class="muted">{r.lastSeen ? timeAgo(r.lastSeen) : "-"}</td>
              <td class="r">
                <label class="switch" style="display:inline-block">
                  <input type="checkbox" checked={hints.includes(r.call)} onchange={() => toggleUse(r.call)} aria-label="Use {r.call}" />
                  <i></i>
                </label>
              </td>
            </tr>
          {/each}
          {#each hints.filter((h) => !app.relays.some((r) => r.call === h)) as h}
            <tr>
              <td class="call">{h}</td><td>-</td><td><span class="tag">added</span></td><td>-</td>
              <td class="r"><button class="btn sm quiet" onclick={() => toggleUse(h)}>Remove</button></td>
            </tr>
          {/each}
        </tbody>
      </table>

      <div class="row">
        <input type="text" placeholder="Add a relay by callsign" bind:value={manual} oninput={() => (manual = manual.toUpperCase())} style="max-width: 260px" />
        <button class="btn" onclick={addManual} disabled={!manual || !!manualProblem}>Add</button>
        {#if manualProblem}<span class="err">{manualProblem}</span>{/if}
      </div>
      <p class="faint small">
        {hints.length ? `Order of use: ${hints.join(", ")}.` : "No relay is chosen, so messages go directly or not at all."}
      </p>
    </section>
  {:else}
    <section class="col">
      {#if !app.relay.running}
        <p class="muted lead">
          Running a relay means your station passes messages between others, by itself, while your radio stays on. It helps the whole
          network, and it is a responsibility.
        </p>
        <div class="req">
          <h3>Before you start</h3>
          <ul>
            <li>Your radio and computer stay on and attended.</li>
            <li>Your licence allows automatic or unattended operation.</li>
            <li>You share a location, so the relay is listed on the map. {sharing ? "" : "Not set yet: do it in Settings."}</li>
          </ul>
          <label class="chk"><input type="checkbox" bind:checked={licence} /> My licence allows unattended or automatic operation.</label>
          <label class="chk"><input type="checkbox" bind:checked={terms} /> I accept the relay terms: I will keep the station running as agreed.</label>
          <div class="row">
            <button class="btn primary lg" disabled={!licence || !terms || busy || app.settings.listenOnly} onclick={startRelay}>Start relay mode</button>
            {#if app.settings.listenOnly}<span class="err">Listen-only is on, so a relay cannot transmit.</span>{/if}
          </div>
        </div>
      {:else}
        <div class="live">
          <div class="row">
            <span class="lamp good"></span>
            <h2>Relay mode is on</h2>
            <div class="spacer"></div>
            <button class="btn danger" onclick={stopRelay} disabled={busy}>Stop relay mode</button>
          </div>
          <div class="stats">
            <div><div class="big mono">{app.relay.forwardedToday}</div><div class="label">Sent today</div></div>
            <div><div class="big mono">{app.relay.forwardedTotal}</div><div class="label">Sent in total</div></div>
            <div><div class="big mono">{app.relay.outbox?.length ?? 0}</div><div class="label">Waiting to go out</div></div>
          </div>

          <h3>kahf.dev</h3>
          <dl class="ledger">
            <dt>Link</dt>
            <dd>
              {#if app.relay.server}
                <span class="tag {app.relay.server.registered ? 'good' : 'warn'}">{app.relay.server.registered ? "Registered" : "Not registered yet"}</span>
              {:else}
                <span class="tag warn">Not connected</span>
              {/if}
            </dd>
            {#if app.relay.server}
              <dt>Schedule</dt>
              <dd>
                {#if app.relay.server.last_schedule_unix}
                  {app.relay.server.slots} slot{app.relay.server.slots === 1 ? "" : "s"}, received at <span class="mono">{utcHm(app.relay.server.last_schedule_unix)}</span> UTC
                {:else}
                  <span class="muted">None received yet</span>
                {/if}
              </dd>
              <dt>Next ask</dt>
              <dd><span class="mono">{utcHm(app.relay.server.next_attempt_unix)}</span> UTC</dd>
              {#if app.relay.server.last_error}
                <dt>Last problem</dt><dd class="err">{app.relay.server.last_error}</dd>
              {/if}
            {:else if app.relay.serverNote}
              <dt>Why</dt><dd class="muted">{app.relay.serverNote}</dd>
            {/if}
            <dt>On the map</dt>
            <dd>
              <span class="mono">{app.relay.area ?? "-"}</span>
              {#if !app.relay.area}<span class="faint small"> Not listed: share a location in Settings to appear on the map.</span>{/if}
            </dd>
            <dt>Reachable</dt>
            <dd>
              {#if serverCheck}
                <span class="tag {serverCheck.reachable ? 'good' : 'bad'}">{serverCheck.reachable ? "Yes" : "No"}</span>
                <span class="faint small mono"> {serverCheck.reachable ? `${serverCheck.millis} ms, ${serverCheck.stationsListed} stations listed` : (serverCheck.error ?? "")}</span>
              {/if}
              <button class="btn sm" onclick={checkServer} disabled={checking}>{checking ? "Checking…" : "Check kahf.dev"}</button>
            </dd>
          </dl>

          <h3>Schedule</h3>
          {#if !app.relay.slots || app.relay.slots.length === 0}
            <p class="faint">No slots yet. The relay works on the radio anyway; kahf.dev adds a schedule when it can be reached.</p>
          {:else}
            <table>
              <thead><tr><th>Starts (UTC)</th><th>In</th><th>Channel</th><th>Length</th><th>Does</th></tr></thead>
              <tbody>
                {#each app.relay.slots as s}
                  <tr>
                    <td class="mono">{utcHm(s.start_unix)}</td>
                    <td class="muted">{relative(s.start_unix, app.relay.nowUnix ?? Math.floor(Date.now() / 1000))}</td>
                    <td class="mono">{channelKhz(s.channel_khz)}</td>
                    <td class="mono">{duration(s.duration_s)}</td>
                    <td>{s.direction === "transmit" ? "Sends" : "Listens"}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          {/if}

          {#if app.relay.outbox && app.relay.outbox.length}
            <h3>Outbox</h3>
            <table>
              <thead><tr><th>#</th><th>Waiting</th></tr></thead>
              <tbody>
                {#each app.relay.outbox as o (o.id)}<tr><td class="mono">{o.id}</td><td>{o.what}</td></tr>{/each}
              </tbody>
            </table>
          {/if}

          {#if app.relay.perHour && app.relay.perHour.length}
            <h3>Sent per hour</h3>
            <div class="hours" role="img" aria-label="Messages sent in each of the last hours">
              {#each app.relay.perHour as [h, n] (h)}
                <div class="hr"><div class="bar" style="height: {Math.max(6, (n / hourMax) * 100)}%"></div><span class="mono">{n}</span><span class="faint small mono">{utcHm(h)}</span></div>
              {/each}
            </div>
          {/if}

          <h3>What it has done</h3>
          {#if app.relay.recent.length === 0}
            <p class="faint">Nothing forwarded yet. Quiet is normal: relays are used when stations cannot reach each other.</p>
          {:else}
            <table>
              <thead><tr><th>Time</th><th>Relay</th><th>Channel</th><th>Result</th></tr></thead>
              <tbody>
                {#each app.relay.recent as h}
                  <tr>
                    <td class="mono">{clock(h.at)}</td><td class="call">{h.from}</td>
                    <td class="mono">{h.channelKhz ? channelKhz(h.channelKhz) : "-"}</td><td class="muted">{h.kind}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          {/if}
        </div>
      {/if}
    </section>
  {/if}
</div>

<style>
  .view {
    height: 100%;
    overflow: auto;
    padding: 20px 24px 32px;
    display: flex;
    flex-direction: column;
    gap: 18px;
    max-width: 880px;
  }
  .lead {
    max-width: 62ch;
    line-height: 1.55;
  }
  .flow {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 14px 0;
    color: var(--faint);
  }
  .flow div {
    display: flex;
    flex-direction: column;
    padding: 8px 16px;
    border: 1px solid var(--line-strong);
    border-radius: 6px;
    color: var(--text);
  }
  .flow .rl {
    border-color: var(--accent);
  }
  table {
    width: 100%;
    border-collapse: collapse;
  }
  th {
    text-align: left;
    font-size: 12px;
    color: var(--muted);
    font-weight: 600;
    padding: 4px 8px;
    background: linear-gradient(var(--bar-top), var(--bar-bot));
    border: 1px solid var(--line);
    border-width: 1px 1px 1px 0;
  }
  th.r,
  td.r {
    text-align: right;
  }
  td {
    padding: 8px;
    border-bottom: 1px solid var(--line);
  }
  .req {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 16px 18px;
    border: 1px solid var(--line-strong);
    border-radius: 6px;
  }
  .req ul {
    margin: 0;
    padding-left: 18px;
    color: var(--muted);
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .chk {
    display: flex;
    gap: 9px;
    align-items: flex-start;
  }
  .live {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .stats {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 0;
    border: 1px solid var(--line);
    border-radius: 6px;
  }
  .stats > div {
    padding: 14px 18px;
    border-right: 1px solid var(--line);
  }
  .stats > div:last-child {
    border-right: 0;
  }
  .big {
    font-size: 26px;
  }
  .ledger {
    display: grid;
    grid-template-columns: 9rem 1fr;
    margin: 0;
    border-top: 1px solid var(--line);
  }
  .ledger dt,
  .ledger dd {
    margin: 0;
    padding: 7px 4px;
    border-bottom: 1px solid var(--line);
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .ledger dt {
    color: var(--muted);
  }
  .hours {
    display: flex;
    align-items: flex-end;
    gap: 10px;
    height: 120px;
    padding-top: 6px;
  }
  .hr {
    flex: 0 0 44px;
    height: 100%;
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
    align-items: center;
    gap: 2px;
  }
  .hr .bar {
    width: 100%;
    background: var(--accent);
    min-height: 4px;
  }
</style>
