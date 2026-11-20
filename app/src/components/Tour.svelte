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
    { page: "chats", target: "strip", title: "The radio strip", text: "Always on screen. The frequency, a signal meter, RX and TX lamps, and what the radio is doing right now in words." },
    { page: "chats", target: "nav", title: "Three places", text: "Messages holds Chats for quick messages, voice notes and files, and Mail for longer ones. Stations shows who is on the air, the map and the relays. Radio is for the signal and your equipment." },
    { page: "chats", target: "to", title: "Who to write to", text: "Pick someone from the list, or type a callsign here. Stations KAHF hears appear on the left by themselves." },
    { page: "chats", target: "compose", title: "Just press Enter", text: "No connecting, no settings. KAHF reaches the station and tells you what happened. The paperclip sends a file and the microphone records a voice note." },
    { page: "radio", target: "spectrum", title: "The channel, live", text: "The shaded band is where KAHF listens. Moving lines mean your radio is being heard." },
    { page: "relay", target: "nav", title: "Relays", text: "Under Stations, Relays: a relay passes messages on for stations that cannot hear each other. Choose the ones KAHF may use, or run one yourself." },
    { page: "chats", target: "settings", title: "Settings", text: "Your callsign, the location you share with the map, and whether KAHF may transmit at all." },
    { page: "chats", target: "help", title: "Help is always here", text: "Searchable guides, troubleshooting and a glossary. You can take this tour again from there." },
  ];

  let i = $state(0);
  let rect = $state<{ x: number; y: number; w: number; h: number } | null>(null);
  const stop = $derived(stops[i]);

  async function place() {
    app.page = stop.page;
    for (let tries = 0; tries < 20; tries++) {
      await new Promise((r) => requestAnimationFrame(() => r(null)));
      const el = document.querySelector(`[data-tour="${stop.target}"]`) as HTMLElement | null;
      if (el) {
        const b = el.getBoundingClientRect();
        rect = { x: b.left - 6, y: b.top - 6, w: b.width + 12, h: b.height + 12 };
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
    app.page = "chats";
  }

  const cardStyle = $derived.by(() => {
    if (!rect) return "left: 50%; top: 40%; transform: translateX(-50%)";
    const below = window.innerHeight - (rect.y + rect.h);
    const right = window.innerWidth - (rect.x + rect.w);
    if (rect.w < 260 && right > 420) return `left: ${rect.x + rect.w + 14}px; top: ${Math.max(16, Math.min(rect.y, window.innerHeight - 230))}px`;
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
  <div class="pop" style={cardStyle}>
    <div class="label">Step {i + 1} of {stops.length}</div>
    <h2>{stop.title}</h2>
    <p class="muted">{stop.text}</p>
    <div class="row">
      <button class="btn sm quiet" onclick={done}>Skip</button>
      <div class="spacer"></div>
      {#if i > 0}<button class="btn sm" onclick={() => (i -= 1)}>Back</button>{/if}
      {#if i < stops.length - 1}
        <button class="btn sm primary" onclick={() => (i += 1)}>Next</button>
      {:else}
        <button class="btn sm primary" onclick={done}>Finish</button>
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
    border-radius: 6px;
    box-shadow: 0 0 0 9999px var(--scrim);
    outline: 2px solid var(--accent);
    transition: all 0.18s ease;
    pointer-events: none;
  }
  .pop {
    position: absolute;
    width: 360px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 14px 16px;
    background: var(--panel);
    border: 1px solid var(--line-strong);
    border-radius: 8px;
    box-shadow: var(--shadow);
  }
</style>
