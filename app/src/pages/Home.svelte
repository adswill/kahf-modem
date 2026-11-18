<script lang="ts">
  import { app } from "../lib/state.svelte";
  import { modeExplained, snrQuality, speedWords, timeAgo } from "../lib/format";
  import Icon from "../components/Icon.svelte";
  import Spectrum from "../components/Spectrum.svelte";

  const p = $derived(app.progress);
  const firstOpen = $derived(app.steps.find((s) => !s.done));
  const quality = $derived(snrQuality(app.metrics?.ownSnr ?? null));
  let tick = $state(Date.now());
  $effect(() => {
    const t = setInterval(() => (tick = Date.now()), 5000);
    return () => clearInterval(t);
  });
</script>

<div class="page">
  <header class="head">
    <div>
      <h1>{app.settings.callsign ? `Hello, ${app.settings.callsign}` : "Welcome to KAHF"}</h1>
      <p class="muted">This is your home screen. It shows what your radio is doing right now.</p>
    </div>
  </header>

  <section class="card hero {app.status.tone}" data-tour="home-status">
    <div class="row">
      <span class="dot-live {app.status.tone}"></span>
      <div class="grow">
        <h2>{app.status.title}</h2>
        <p class="muted">{app.status.detail}</p>
      </div>
      {#if app.link.state === "linked" || app.link.state === "calling"}
        <button class="btn" onclick={() => app.hangUp()}>Hang up</button>
      {:else if !app.hasCall && !app.settings.listenOnly}
        <button class="btn primary" onclick={() => app.go("setup")}>Finish setup</button>
      {:else if !app.settings.listenOnly}
        <button class="btn primary" onclick={() => app.go("messages")}><Icon name="send" /> Write a message</button>
      {:else}
        <button class="btn" onclick={() => app.go("settings")}>Turn on sending</button>
      {/if}
    </div>
  </section>

  {#if p.done < p.total}
    <section class="card" data-tour="checklist">
      <div class="card-title">
        <div>
          <h2>Getting started</h2>
          <p class="muted small">{p.done} of {p.total} done. Follow these steps in order.</p>
        </div>
        <div class="bar" aria-hidden="true"><div style:width={`${(p.done / p.total) * 100}%`}></div></div>
      </div>
      <ol class="steps">
        {#each app.steps as s, i}
          <li class:done={s.done} class:next={firstOpen?.id === s.id}>
            <span class="n">{#if s.done}<Icon name="check" size={15} />{:else}{i + 1}{/if}</span>
            <div class="grow">
              <strong>{s.title}</strong>
              <div class="muted small">{s.why}</div>
            </div>
            {#if !s.done && firstOpen?.id === s.id}
              <button class="btn small primary" onclick={() => app.go(s.go)}>{s.action}</button>
            {/if}
          </li>
        {/each}
      </ol>
    </section>
  {/if}

  <div class="grid-2">
    <section class="card" data-tour="heard">
      <div class="card-title">
        <h2>Who is on the air</h2>
        <button class="btn ghost small" onclick={() => app.go("network")}>See all</button>
      </div>
      {#if app.heard.length === 0}
        <div class="empty">
          <Icon name="wave" size={28} />
          <p><strong>Nobody heard yet</strong></p>
          <p class="muted small">
            Leave KAHF running. Stations that are nearby appear here by themselves, usually within a few minutes.
          </p>
        </div>
      {:else}
        <ul class="list">
          {#each app.heard.slice(0, 5) as h (h.call)}
            {@const q = snrQuality(h.snr)}
            <li>
              <span class="callsign">{h.call}</span>
              <span class="chip {q.tone}">{q.label}</span>
              <span class="faint small grow">{timeAgo(h.at, tick)}</span>
              <button
                class="btn small"
                disabled={app.settings.listenOnly}
                onclick={() => {
                  app.peer = h.call;
                  app.go("messages");
                }}>Message</button
              >
            </li>
          {/each}
        </ul>
      {/if}
    </section>

    <section class="card" data-tour="link">
      <div class="card-title"><h2>Your connection</h2></div>
      {#if app.metrics && app.link.state === "linked"}
        <div class="stack-sm">
          <div class="row">
            <span class="chip {quality.tone}">{quality.label} signal</span>
            <span class="muted small">They hear you at {app.metrics.peerSnr ?? "?"} dB, you hear them at {app.metrics.ownSnr ?? "?"} dB.</span>
          </div>
          <div class="speed">
            <strong>{app.metrics.mode}</strong>
            <span class="muted">{speedWords(app.metrics.netBps)}</span>
          </div>
          <p class="muted small">
            {app.metrics.auto ? "KAHF picked this speed for you." : "You chose this speed."} {modeExplained(app.metrics.mode)}
          </p>
        </div>
      {:else}
        <div class="empty">
          <Icon name="plug" size={28} />
          <p><strong>Not connected</strong></p>
          <p class="muted small">
            {app.settings.autoLink
              ? "You do not need to connect. When you send a message, KAHF contacts the other station by itself."
              : "Connect from the Network page, or turn on automatic connections in Settings."}
          </p>
        </div>
      {/if}
    </section>
  </div>

  <section class="card">
    <div class="card-title">
      <div>
        <h2>What your radio hears right now</h2>
        <p class="muted small">Bright areas are signals. KAHF works in the shaded channel.</p>
      </div>
      <button class="btn ghost small" onclick={() => app.go("radio")}>Open Radio</button>
    </div>
    <Spectrum height={230} compact />
  </section>

  {#if app.log.length}
    <section class="card flat">
      <h3>Latest activity</h3>
      <ul class="log">
        {#each app.log.slice(0, 5) as l}
          <li><span class="faint small mono">{new Date(l.at).toLocaleTimeString()}</span> {l.text}</li>
        {/each}
      </ul>
    </section>
  {/if}
</div>

<style>
  .hero.good {
    border-color: var(--accent);
  }
  .hero.busy {
    border-color: var(--amber);
  }
  .bar {
    width: 140px;
    height: 8px;
    border-radius: 99px;
    background: var(--raised);
    overflow: hidden;
  }
  .bar div {
    height: 100%;
    background: var(--accent);
  }
  .steps {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .steps li {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 12px 14px;
    border-radius: 12px;
    background: var(--panel-2);
  }
  .steps li.next {
    outline: 1.5px solid var(--accent);
    background: var(--accent-soft);
  }
  .steps li.done {
    opacity: 0.6;
  }
  .n {
    width: 28px;
    height: 28px;
    border-radius: 50%;
    background: var(--raised);
    display: grid;
    place-items: center;
    font-weight: 700;
    font-size: 13px;
    flex: none;
  }
  li.done .n {
    background: var(--accent);
    color: var(--accent-ink);
  }
  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    gap: 6px;
    padding: 18px 8px;
    color: var(--faint);
  }
  .empty strong {
    color: var(--text);
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .list li {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 10px;
    border-radius: 10px;
    background: var(--panel-2);
  }
  .speed {
    display: flex;
    flex-direction: column;
  }
  .speed strong {
    font-size: 24px;
  }
  .log {
    list-style: none;
    margin: 8px 0 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 14px;
  }
</style>
