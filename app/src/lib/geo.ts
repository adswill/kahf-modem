// Position helpers: Maidenhead locators and checking coordinates.

/** Centre of a Maidenhead locator of 4 or 6 characters, for example "JN45" or "JN45rm". */
export function locatorToLatLon(loc: string): { lat: number; lon: number } | null {
  const s = loc.trim();
  if (!/^[A-Ra-r]{2}[0-9]{2}([A-Xa-x]{2})?$/.test(s)) return null;
  const up = s.toUpperCase();
  const a = (c: string) => c.charCodeAt(0) - 65;
  let lon = a(up[0]) * 20 - 180 + Number(up[2]) * 2;
  let lat = a(up[1]) * 10 - 90 + Number(up[3]);
  if (up.length === 6) {
    lon += a(up[4]) * (2 / 24) + 1 / 24;
    lat += a(up[5]) * (1 / 24) + 1 / 48;
  } else {
    lon += 1;
    lat += 0.5;
  }
  return { lat, lon };
}

/** 6-character Maidenhead locator for a position. */
export function latLonToLocator(lat: number, lon: number): string {
  const L = lon + 180;
  const B = lat + 90;
  const f = (n: number) => String.fromCharCode(65 + n);
  const g = (n: number) => String.fromCharCode(97 + n);
  const lo1 = Math.floor(L / 20);
  const la1 = Math.floor(B / 10);
  const lo2 = Math.floor((L % 20) / 2);
  const la2 = Math.floor(B % 10);
  const lo3 = Math.floor(((L % 2) * 60) / 5);
  const la3 = Math.floor(((B % 1) * 60) / 2.5);
  return `${f(lo1)}${f(la1)}${lo2}${la2}${g(lo3)}${g(la3)}`;
}

export function validLat(v: number | null): boolean {
  return v !== null && Number.isFinite(v) && v >= -90 && v <= 90;
}

export function validLon(v: number | null): boolean {
  return v !== null && Number.isFinite(v) && v >= -180 && v <= 180;
}

/** What a person is told others will see, in plain words. */
export function sharingSummary(mode: string, lat: number | null, lon: number | null, radiusKm: number): string {
  if (mode === "off") return "Nobody can see where you are. You still appear on the map only if you choose to.";
  if (!validLat(lat) || !validLon(lon)) return "Enter your position to continue.";
  if (mode === "precise") return "Your position is shown on the public map to about 200 metres.";
  return `The public map shows a circle about ${radiusKm} km wide around you. Your exact position is never sent.`;
}
