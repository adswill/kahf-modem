<script lang="ts">
  import { app } from "../lib/state.svelte";
  import { callProblem, normaliseCall } from "../lib/callsign";
  import { clock, timeAgo } from "../lib/format";
  import Icon from "../components/Icon.svelte";
  import Tip from "../components/Tip.svelte";

  let tab = $state<"use" | "run">(app.relay.running ? "run" : "use");
  let licence = $state(false);
  let terms = $state(false);
  let manual = $state("");
  let busy = $state(false);

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
        <div><b class="call">You</b><span class="faint small">write</span></div>
        <Icon name="arrow" /> <div class="rl"><b class="call">Relay</b><span class="faint small">passes it on</span></div>
        <Icon name="arrow" /> <div><b class="call">Them</b><span class="faint small">receive</span></div>
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
            <div><div class="big mono">{app.relay.forwardedToday}</div><div class="label">Forwarded today</div></div>
            <div><div class="big mono">{app.relay.forwardedTotal}</div><div class="label">Forwarded in total</div></div>
            <div>
              <div class="big mono">{app.relay.area ?? "-"}</div>
              <div class="label">On the map {#if !app.relay.area}<Tip text="Not listed: share a location in Settings to appear on the map." />{/if}</div>
            </div>
          </div>
          <p class="muted">
            Waiting for messages for other stations. When one arrives, your station forwards it in your transmit window and
            logs it below. You do not need to do anything.
          </p>
          <h3>What it has done</h3>
          {#if app.relay.recent.length === 0}
            <p class="faint">Nothing forwarded yet. Quiet is normal: relays are used when stations cannot reach each other.</p>
          {:else}
            <table>
              <thead><tr><th>Time</th><th>From</th><th></th><th>To</th><th>Kind</th></tr></thead>
              <tbody>
                {#each app.relay.recent as h}
                  <tr><td class="mono">{clock(h.at)}</td><td class="call">{h.from}</td><td class="faint">→</td><td class="call">{h.to}</td><td class="muted">{h.kind}</td></tr>
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
    font-size: 11px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--faint);
    font-weight: 600;
    padding: 5px 8px;
    border-bottom: 1px solid var(--line);
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
</style>
