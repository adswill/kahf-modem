<script lang="ts">
  import { app } from "../lib/state.svelte";
  import Tip from "../components/Tip.svelte";

  let query = $state("");
  let ex = $state({ ...app.settings.exportServices });
  let busy = $state("");
  let answer = $state<{ service: string; ok: boolean; text: string } | null>(null);

  // Sends the logbook to one service, only when its button is pressed.
  async function send(service: "qrz" | "clublog" | "lotw" | "n1mm") {
    busy = service;
    answer = null;
    try {
      await app.saveSettings({ exportServices: { ...ex } });
      answer = { service, ok: true, text: await app.engine.uploadLogbook(service, { ...ex }) };
    } catch (e) {
      answer = { service, ok: false, text: String(e).replace(/^Error: /, "") };
    }
    busy = "";
  }
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

  <details class="send">
    <summary>Send the logbook to other services</summary>
    <p class="faint small">Nothing is sent until you press a button. The keys and passwords below are kept on this computer, in the settings file, as plain text.</p>
    <div class="svc">
      <h3>QRZ Logbook</h3>
      <label class="field"><span class="t">Logbook API key</span><input type="password" bind:value={ex.qrzKey} autocomplete="off" /></label>
      <button class="btn sm" disabled={busy !== "" || !ex.qrzKey || app.book.qsos.length === 0} onclick={() => send("qrz")}>{busy === "qrz" ? "Sending…" : "Send to QRZ"}</button>
    </div>
    <div class="svc">
      <h3>Club Log</h3>
      <div class="grid3">
        <label class="field"><span class="t">Email</span><input type="text" bind:value={ex.clublogEmail} autocomplete="off" /></label>
        <label class="field"><span class="t">Password</span><input type="password" bind:value={ex.clublogPassword} autocomplete="off" /></label>
        <label class="field"><span class="t">Application key</span><input type="password" bind:value={ex.clublogAppKey} autocomplete="off" /></label>
      </div>
      <button class="btn sm" disabled={busy !== "" || !ex.clublogEmail || !ex.clublogPassword || !ex.clublogAppKey || app.book.qsos.length === 0} onclick={() => send("clublog")}>{busy === "clublog" ? "Sending…" : "Send to Club Log"}</button>
    </div>
    <div class="svc">
      <h3>Logbook of the World (needs ARRL TQSL)</h3>
      <div class="grid3">
        <label class="field"><span class="t">Path of TQSL</span><input type="text" bind:value={ex.tqslPath} placeholder="/Applications/TQSL.app/Contents/MacOS/tqsl" /></label>
        <label class="field"><span class="t">Station location name</span><input type="text" bind:value={ex.lotwLocation} /></label>
      </div>
      <button class="btn sm" disabled={busy !== "" || !ex.tqslPath || !ex.lotwLocation || app.book.qsos.length === 0} onclick={() => send("lotw")}>{busy === "lotw" ? "Sending…" : "Send with TQSL"}</button>
    </div>
    <div class="svc">
      <h3>N1MM Logger+ (on this network)</h3>
      <label class="field"><span class="t">Address</span><input type="text" bind:value={ex.n1mmAddr} placeholder="127.0.0.1:12060" /></label>
      <button class="btn sm" disabled={busy !== "" || !ex.n1mmAddr || app.book.qsos.length === 0} onclick={() => send("n1mm")}>{busy === "n1mm" ? "Sending…" : "Send to N1MM"}</button>
    </div>
    {#if answer}<p class={answer.ok ? "ok" : "err"} role="status">{answer.text}</p>{/if}
  </details>
</div>

<style>
  .send { border-top: 1px solid var(--line); padding-top: 12px; }
  .send summary { cursor: pointer; font-weight: 600; margin-bottom: 8px; }
  .svc { display: flex; flex-direction: column; gap: 8px; align-items: flex-start; padding: 12px 0; border-bottom: 1px solid var(--line); max-width: 760px; }
  .grid3 { display: grid; grid-template-columns: repeat(3, 1fr); gap: 12px; width: 100%; }
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
