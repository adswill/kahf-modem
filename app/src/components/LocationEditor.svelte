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

  // Keep the saved value in step with what is typed.
  $effect(() => {
    value.lat = posOk ? lat : null;
    value.lon = posOk ? lon : null;
  });

  const OPTIONS: { m: LocationMode; t: string; s: string }[] = [
    { m: "off", t: "Don't share my location", s: "Nobody sees where you are. KAHF works exactly the same." },
    { m: "area", t: "Share a circle around me", s: "People see a circle on the map. Your exact position stays private." },
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

<div class="stack">
  {#each OPTIONS as o}
    <button class="choice" class:on={value.mode === o.m} onclick={() => (value.mode = o.m)}>
      <span class="dot"></span>
      <span><strong>{o.t}</strong><span class="sub">{o.s}</span></span>
    </button>
  {/each}

  {#if value.mode !== "off"}
    <div class="card flat stack">
      <h3>Where are you?</h3>
      <div class="grid-2">
        <label class="field">
          Grid square (a short code radio amateurs use)
          <div class="row">
            <input type="text" placeholder="For example JN45rm" bind:value={locatorText} />
            <button class="btn small" disabled={!locatorOk} onclick={applyLocator}>Use</button>
          </div>
        </label>
        <label class="field">
          Or let the computer find you
          <button class="btn" onclick={findMe}><Icon name="pin" /> Find my position</button>
        </label>
      </div>
      <div class="grid-2">
        <label class="field">
          Latitude
          <input type="text" inputmode="decimal" placeholder="For example 51.507" bind:value={latText} />
        </label>
        <label class="field">
          Longitude
          <input type="text" inputmode="decimal" placeholder="For example -0.128" bind:value={lonText} />
        </label>
      </div>
      {#if latText !== "" || lonText !== ""}
        {#if !posOk}
          <span class="field-error">Latitude is -90 to 90 and longitude is -180 to 180.</span>
        {:else}
          <span class="field-ok">Grid square {here}</span>
        {/if}
      {/if}
      {#if value.mode === "area"}
        <label class="field">
          Circle size: {value.radiusKm} km
          <input type="range" min="10" max="100" step="10" bind:value={value.radiusKm} />
        </label>
      {/if}
    </div>
  {/if}
  <div class="notice">
    <Icon name="eye" />
    <span>{sharingSummary(value.mode, lat, lon, value.radiusKm)}</span>
  </div>
  {#if value.mode !== "off"}
    <div class="notice warn">
      <Icon name="info" />
      <span>
        Today the public map lists <strong>relays</strong>, each shown by the one-degree square it is in. Other stations
        appear once they can announce themselves over the air. Your choice is saved now and used as soon as that works.
      </span>
    </div>
  {/if}
</div>
