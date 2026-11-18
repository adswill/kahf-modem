<script lang="ts">
  import { app } from "../lib/state.svelte";
  import { snrQuality, timeAgo } from "../lib/format";
  import Icon from "../components/Icon.svelte";

  let tick = $state(Date.now());
  $effect(() => {
    const t = setInterval(() => (tick = Date.now()), 5000);
    return () => clearInterval(t);
  });

  function openMap() {
    window.open("https://kahf.dev/map", "_blank");
  }
</script>

<div class="page">
  <header class="head">
    <div>
      <h1>Network</h1>
      <p class="muted">
        The stations you can hear right now, and how a message can reach people you cannot hear.
      </p>
    </div>
  </header>

  <section class="card" data-tour="stations">
    <div class="card-title">
      <div>
        <h2>Stations heard</h2>
        <p class="muted small">Heard directly on your frequency. The more you listen, the longer this list gets.</p>
      </div>
      <span class="chip">{app.heard.length}</span>
    </div>
    {#if app.heard.length === 0}
      <div class="blank">
        <Icon name="wave" size={30} />
        <p><strong>Nobody heard yet</strong></p>
        <p class="muted small">
          Stations only appear when they transmit, so this can take a few minutes, or longer when the band is quiet.
          Check the Radio page to see that your radio is picking up sound.
        </p>
        <button class="btn" onclick={() => app.go("radio")}>Check my radio</button>
      </div>
    {:else}
      <table>
        <thead>
          <tr><th>Callsign</th><th>Signal</th><th>Last heard</th><th></th></tr>
        </thead>
        <tbody>
          {#each app.heard as h (h.call)}
            {@const q = snrQuality(h.snr)}
            <tr>
              <td class="callsign">{h.call}</td>
              <td>
                <span class="chip {q.tone}">{q.label}</span>
                {#if h.snr !== null}<span class="faint small">{h.snr} dB</span>{/if}
              </td>
              <td class="muted">{timeAgo(h.at, tick)}</td>
              <td class="right">
                <button
                  class="btn small"
                  disabled={app.settings.listenOnly}
                  onclick={() => {
                    app.peer = h.call;
                    app.go("messages");
                  }}>Message</button
                >
                <button class="btn small ghost" disabled={app.settings.listenOnly} onclick={() => app.call(h.call)}>Call</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  </section>

  <section class="card">
    <h2>Reaching people who are far away</h2>
    <p class="muted">
      Radio waves have limits. If someone is too far or the band is closed, a <strong>relay</strong> can pass your
      message along.
    </p>
    <div class="flow" aria-label="How a relay works">
      <div class="node you"><Icon name="radio" size={22} /><strong>You</strong><span class="small muted">write a message</span></div>
      <div class="link"><Icon name="arrow" /></div>
      <div class="node relay"><Icon name="network" size={22} /><strong>Relay</strong><span class="small muted">a station that stays on the air</span></div>
      <div class="link"><Icon name="arrow" /></div>
      <div class="node them"><Icon name="radio" size={22} /><strong>Them</strong><span class="small muted">receive it</span></div>
    </div>
    <div class="notice good">
      <Icon name="check" />
      <span>
        You do not do anything special. When a station does not answer, KAHF tries a relay by itself and tells you
        what happened. Want to help others? You can turn your own station into a relay in <button class="link" onclick={() => app.go("settings")}>Settings</button>.
      </span>
    </div>
  </section>

  <section class="card">
    <div class="card-title">
      <div>
        <h2>The map</h2>
        <p class="muted small">Stations that choose to share their location appear on the public map at kahf.dev.</p>
      </div>
      <button class="btn" onclick={openMap}>Open the map</button>
    </div>
    <div class="notice">
      <Icon name="eye" />
      <span>
        {app.settings.location.mode === "off"
          ? "You are not on the map. This is the default. You can change it in Settings, under Location."
          : app.settings.location.mode === "area"
            ? "You are on the map as a circle. Your exact position is not shown."
            : "You are on the map with your position shown to about 200 metres."}
      </span>
    </div>
  </section>
</div>

<style>
  table {
    width: 100%;
    border-collapse: collapse;
  }
  th {
    text-align: left;
    font-size: 12.5px;
    color: var(--faint);
    font-weight: 600;
    padding: 6px 10px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  td {
    padding: 11px 10px;
    border-top: 1px solid var(--line-soft);
  }
  td.right {
    text-align: right;
    white-space: nowrap;
  }
  .blank {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    text-align: center;
    padding: 24px;
    color: var(--faint);
  }
  .blank strong {
    color: var(--text);
  }
  .flow {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 14px;
    flex-wrap: wrap;
    padding: 22px 0;
  }
  .node {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    padding: 16px 22px;
    border-radius: 14px;
    background: var(--panel-2);
    border: 1px solid var(--line-soft);
    min-width: 150px;
    text-align: center;
  }
  .node.relay {
    border-color: var(--accent);
    color: var(--accent);
  }
  .node.relay strong {
    color: var(--text);
  }
  .link {
    color: var(--faint);
  }
  button.link {
    background: none;
    border: 0;
    color: var(--accent);
    font: inherit;
    cursor: pointer;
    padding: 0;
    text-decoration: underline;
  }
</style>
