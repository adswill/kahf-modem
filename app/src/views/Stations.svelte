<script lang="ts">
  import { app } from "../lib/state.svelte";
  import { snrQuality, timeAgo } from "../lib/format";
  import MapView from "../components/MapView.svelte";
  import Tip from "../components/Tip.svelte";

  const me = $derived(
    app.settings.location.mode !== "off" && app.settings.location.lat !== null && app.settings.location.lon !== null
      ? { lat: app.settings.location.lat, lon: app.settings.location.lon }
      : null,
  );
  let tick = $state(Date.now());
  $effect(() => {
    const t = setInterval(() => (tick = Date.now()), 5000);
    return () => clearInterval(t);
  });
</script>

<div class="view">
  <section class="mapbox">
    <div class="head row">
      <h1>Stations</h1>
      <Tip text="A station is one person's KAHF setup, named by its callsign. The map shows relays, and stations that chose to share their location, from kahf.dev. Announce myself tells everyone in range, and through relays everyone beyond, that you exist, so they add you to their contacts." />
      <div class="spacer"></div>
      <button class="btn sm primary" disabled={app.settings.listenOnly} title="Announce your station on the air" onclick={() => app.advertise()}>Announce myself</button>
      <button class="btn sm" onclick={() => app.refreshRelayDirectory(true)}>Refresh from kahf.dev</button>
      <button class="btn sm quiet" onclick={() => window.open("https://kahf.dev/map", "_blank")}>Open the website map</button>
    </div>
    <MapView {me} relays={app.relays} stations={app.heard.flatMap((h) => (h.lat != null && h.lon != null && !app.relays.some((r) => r.call === h.call) ? [{ call: h.call, lat: h.lat, lon: h.lon }] : []))} height={340} />
    {#if !me}
      <p class="faint small">Your own position is not shown because you are not sharing a location. <button class="lnk" onclick={() => app.openSettings("location")}>Change that</button></p>
    {/if}
  </section>

  <section class="heard" data-tour="stations">
    <div class="head row">
      <h2>Heard on the radio</h2>
      <span class="tag">{app.heard.length}</span>
      <Tip text="Stations KAHF has heard directly on your channel. This list does not need the internet." />
    </div>
    {#if app.heard.length === 0}
      <p class="muted">Nobody yet. Stations appear when they transmit, so this can take a few minutes, or longer when the band is quiet. If nothing ever appears, check the picture on the Radio screen.</p>
    {:else}
      <table>
        <thead><tr><th>Station</th><th>Signal</th><th>Heard</th><th></th></tr></thead>
        <tbody>
          {#each app.heard as h (h.call)}
            {@const q = snrQuality(h.snr)}
            <tr>
              <td class="call">{h.call}</td>
              <td>
                {#if h.relay && h.snr === null && h.hops === undefined}
                  <span class="tag good">Relay</span> <span class="faint small">{h.online ? "online, reaches other areas" : "radio only"}</span>
                {:else if h.hops !== undefined && h.snr === null}
                  <span class="tag">Announced</span> <span class="faint small">{h.hops === 0 ? "heard directly" : `through ${h.hops} relay${h.hops === 1 ? "" : "s"}`}</span>
                {:else}
                  <span class="tag {q.tone}">{q.label}</span> {#if h.snr !== null}<span class="faint small mono">{h.snr} dB</span>{/if}
                {/if}
              </td>
              <td class="muted">{timeAgo(h.at, tick)}</td>
              <td class="r">
                <button class="btn sm" disabled={app.settings.listenOnly} onclick={() => { app.peer = h.call; app.go("chats"); }}>Message</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  </section>
</div>

<style>
  .view {
    height: 100%;
    overflow: auto;
    padding: 20px 24px 28px;
    display: flex;
    flex-direction: column;
    gap: 24px;
  }
  .head {
    margin-bottom: 10px;
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
  td {
    padding: 8px;
    border-bottom: 1px solid var(--line);
  }
  td.r {
    text-align: right;
  }
  .lnk {
    background: none;
    border: 0;
    color: var(--accent);
    font: inherit;
    cursor: pointer;
    padding: 0;
    text-decoration: underline;
  }
</style>
