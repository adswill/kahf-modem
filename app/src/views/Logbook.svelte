<script lang="ts">
  import { app } from "../lib/state.svelte";
  import Tip from "../components/Tip.svelte";

  let query = $state("");
  const q = $derived(query.trim().toLowerCase());
  const rows = $derived(
    app.book.qsos.filter((r) => !q || [r.call, r.name, r.comment, r.band, r.date].some((v) => v.toLowerCase().includes(q))),
  );
</script>

<div class="view">
  <div class="head row">
    <h1>Logbook</h1>
    <Tip text="Every contact KAHF logs is kept in a plain text file in your data folder. Export writes the same list as an ADIF file (logbook.adi) that other logging programs and the online logbooks can read." />
    <span class="tag">{app.book.qsos.length}</span>
    <div class="spacer"></div>
    <input type="search" placeholder="Find a callsign or note" bind:value={query} aria-label="Search the logbook" style="max-width: 240px" />
    <button class="btn sm" onclick={() => app.engine.openDataDir()}>Open the data folder</button>
    <button class="btn sm primary" disabled={app.book.qsos.length === 0} onclick={() => app.exportLogbook()}>Export as ADIF</button>
  </div>

  {#if app.book.error}
    <div class="note warn" role="alert">A data file could not be read: {app.book.error}</div>
  {/if}

  {#if app.book.qsos.length === 0}
    <p class="muted">Nothing logged yet. Contacts appear here, newest first, as soon as KAHF logs them.</p>
  {:else if rows.length === 0}
    <p class="muted">No contact matches "{query}".</p>
  {:else}
    <table>
      <thead>
        <tr><th>Date</th><th>Time (UTC)</th><th>Station</th><th>Name</th><th>Band</th><th>Frequency</th><th>Sent</th><th>Received</th><th>Note</th></tr>
      </thead>
      <tbody>
        {#each rows as r, i (r.date + r.time + r.call + i)}
          <tr>
            <td class="mono">{r.date}</td>
            <td class="mono">{r.time}</td>
            <td class="call">{r.call}</td>
            <td>{r.name}</td>
            <td class="mono">{r.band}</td>
            <td class="mono">{r.freq_mhz !== null ? `${r.freq_mhz.toFixed(5)} MHz` : "-"}</td>
            <td class="mono">{r.rst_sent || "-"}</td>
            <td class="mono">{r.rst_rcvd || "-"}</td>
            <td class="muted">{r.comment}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</div>

<style>
  .view {
    height: 100%;
    overflow: auto;
    padding: 20px 24px 28px;
    display: flex;
    flex-direction: column;
    gap: 16px;
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
    white-space: nowrap;
  }
  td {
    padding: 7px 8px;
    border-bottom: 1px solid var(--line);
  }
</style>
