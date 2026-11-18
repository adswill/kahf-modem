<script lang="ts">
  // Choose whether to share a location, and where. Used by setup and by Settings.
  import { app } from "../lib/state.svelte";
  import { latLonToLocator, locatorToLatLon, sharingSummary, validLat, validLon } from "../lib/geo";
  import type { Location, LocationMode } from "../lib/types";
  import Icon from "./Icon.svelte";

  let { value = $bindable() }: { value: Location } = $props();

  let locatorText = $state("");
  let latText = $state(value.lat === null ? "" : String(value.lat));
  let lonText = $state(value.lon === null ? "" : String(value.lon));

  const lat = $derived(latText.trim() === "" ? null : Number(latText));
  const lon = $derived(lonText.trim() === "" ? null : Number(lonText));
  const posOk = $derived(validLat(lat) && validLon(lon));
  const locatorOk = $derived(locatorToLatLon(locatorText) !== null);
  const here = $derived(posOk ? latLonToLocator(lat as number, lon as number) : "");

  $effect(() => {
    value.lat = posOk ? lat : null;
    value.lon = posOk ? lon : null;
  });

  const OPTIONS: { m: LocationMode; t: string; s: string }[] = [
    { m: "off", t: "Do not share my location", s: "Nobody sees where you are. KAHF works the same." },
    { m: "area", t: "Share a circle around me", s: "Others see a circle on the map. Your exact position stays private." },
    { m: "precise", t: "Share my exact position", s: "Shown to about 200 metres. Good for relays that want to be found." },
  ];

  function applyLocator() {
    const p = locatorToLatLon(locatorText);
    if (!p) return;
    latText = p.lat.toFixed(3);
    lonText = p.lon.toFixed(3);
  }

  function findMe() {
    if (!("geolocation" in navigator)) {
      app.toast("This computer cannot find its own position. Type it in instead.", "warn");
      return;
    }
    navigator.geolocation.getCurrentPosition(
      (p) => {
        latText = p.coords.latitude.toFixed(3);
        lonText = p.coords.longitude.toFixed(3);
      },
      () => app.toast("Could not get your position. You can type it in instead.", "warn"),
      { timeout: 8000 },
    );
  }
</script>

<div class="col">
  {#each OPTIONS as o}
    <button class="pick" class:on={value.mode === o.m} onclick={() => (value.mode = o.m)}>
      <span class="dot"></span>
      <span><b>{o.t}</b><span class="s">{o.s}</span></span>
    </button>
  {/each}

  {#if value.mode !== "off"}
    <div class="col box">
      <div class="two">
        <label class="field">
          <span class="t">Grid square</span>
          <div class="row">
            <input type="text" placeholder="For example JN45rm" bind:value={locatorText} />
            <button class="btn sm" disabled={!locatorOk} onclick={applyLocator}>Use</button>
          </div>
          <span class="hint">A short code radio amateurs use for a place.</span>
        </label>
        <div class="field">
          <span class="t">Or</span>
          <button class="btn" onclick={findMe}><Icon name="map" size={14} /> Find my position</button>
        </div>
      </div>
      <div class="two">
        <label class="field">
          <span class="t">Latitude</span>
          <input type="text" inputmode="decimal" placeholder="51.507" bind:value={latText} />
        </label>
        <label class="field">
          <span class="t">Longitude</span>
          <input type="text" inputmode="decimal" placeholder="-0.128" bind:value={lonText} />
        </label>
      </div>
      {#if latText !== "" || lonText !== ""}
        {#if !posOk}<span class="err">Latitude is -90 to 90 and longitude is -180 to 180.</span>
        {:else}<span class="ok">Grid square {here}</span>{/if}
      {/if}
      {#if value.mode === "area"}
        <label class="field">
          <span class="t">Circle size: {value.radiusKm} km</span>
          <input type="range" min="10" max="100" step="10" bind:value={value.radiusKm} />
        </label>
      {/if}
    </div>
  {/if}

  <div class="note"><Icon name="info" size={15} /><span>{sharingSummary(value.mode, lat, lon, value.radiusKm)}</span></div>
  {#if value.mode !== "off"}
    <div class="note warn">
      <Icon name="info" size={15} />
      <span>
        Today the public map lists <b>relays</b>, each by the one-degree square it is in. Other stations appear once they can
        announce themselves over the air. Your choice is saved now and used as soon as that works.
      </span>
    </div>
  {/if}
</div>

<style>
  .box {
    padding: 14px;
    border: 1px solid var(--line);
    border-radius: 6px;
  }
  .two {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
  }
</style>
