<script lang="ts">
  // A guided tour: dims the screen, lights up one part at a time and says what it is for.
  import { app, type Page } from "../lib/state.svelte";

  interface Stop {
    page: Page;
    target: string;
    title: string;
    text: string;
  }

  const stops: Stop[] = [
    { page: "home", target: "home-status", title: "What is happening", text: "This card always says what your radio is doing right now, in plain words. Start here." },
    { page: "home", target: "checklist", title: "Your first steps", text: "A short list for new users. The highlighted step is the next thing to do." },
    { page: "home", target: "heard", title: "Who is on the air", text: "Stations that KAHF hears appear here by themselves. You do not need to search." },
    { page: "messages", target: "to", title: "Write to someone", text: "Type a callsign. KAHF suggests stations you have heard." },
    { page: "messages", target: "compose", title: "Just press Send", text: "No connecting, no settings. KAHF reaches the station, sends your message and tells you what happened." },
    { page: "radio", target: "spectrum", title: "The band, live", text: "Your radio's signal. The shaded band is the one channel KAHF uses. Moving lines mean it is working." },
    { page: "radio", target: "speed", title: "Speed", text: "Automatic picks the fastest speed your signal can carry. You can leave it alone." },
    { page: "settings", target: "location", title: "Your privacy", text: "Your location is off by default. Share a circle or your position only if you want to be on the public map." },
    { page: "help", target: "help", title: "Help is always here", text: "Short guides, troubleshooting and a glossary. You can take this tour again from here." },
  ];

  let i = $state(0);
  let rect = $state<{ x: number; y: number; w: number; h: number } | null>(null);
  const stop = $derived(stops[i]);

  async function place() {
    app.page = stop.page;
    // Wait for the page to draw, then find the element.
    for (let tries = 0; tries < 20; tries++) {
      await new Promise((r) => requestAnimationFrame(() => r(null)));
      const el = document.querySelector(`[data-tour="${stop.target}"]`) as HTMLElement | null;
      if (el) {
        el.scrollIntoView({ block: "center", behavior: "instant" as ScrollBehavior });
        const b = el.getBoundingClientRect();
        rect = { x: b.left - 8, y: b.top - 8, w: b.width + 16, h: b.height + 16 };
        return;
      }
    }
    rect = null;
  }

  $effect(() => {
    i;
    place();
  });

  function done() {
    app.tourOpen = false;
    app.saveSettings({ tourDone: true });
    app.page = "home";
  }

  // Put the card on the side of the highlighted part that has the most room.
  const cardStyle = $derived.by(() => {
    if (!rect) return "left: 50%; top: 40%; transform: translateX(-50%)";
    const below = window.innerHeight - (rect.y + rect.h);
    const top = below > 220 ? rect.y + rect.h + 14 : Math.max(16, rect.y - 210);
    const left = Math.min(Math.max(16, rect.x), window.innerWidth - 400);
    return `left: ${left}px; top: ${top}px`;
  });
</script>

<div class="tour" role="dialog" aria-label="Guided tour">
  {#if rect}
    <div class="hole" style:left={`${rect.x}px`} style:top={`${rect.y}px`} style:width={`${rect.w}px`} style:height={`${rect.h}px`}></div>
  {:else}
    <div class="dim"></div>
  {/if}
  <div class="card pop" style={cardStyle}>
    <div class="faint small">Step {i + 1} of {stops.length}</div>
    <h2>{stop.title}</h2>
    <p class="muted">{stop.text}</p>
    <div class="row">
      <button class="btn ghost small" onclick={done}>Skip</button>
      <div class="grow"></div>
      {#if i > 0}<button class="btn small" onclick={() => (i -= 1)}>Back</button>{/if}
      {#if i < stops.length - 1}
        <button class="btn primary small" onclick={() => (i += 1)}>Next</button>
      {:else}
        <button class="btn primary small" onclick={done}>Finish</button>
      {/if}
    </div>
  </div>
</div>

<style>
  .tour {
    position: fixed;
    inset: 0;
    z-index: 100;
  }
  .dim {
    position: absolute;
    inset: 0;
    background: var(--scrim);
  }
  .hole {
    position: absolute;
    border-radius: 16px;
    box-shadow: 0 0 0 9999px var(--scrim);
    outline: 2px solid var(--accent);
    transition: all 0.2s ease;
    pointer-events: none;
  }
  .pop {
    position: absolute;
    width: 380px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    box-shadow: var(--shadow);
    border-color: var(--line);
  }
</style>
