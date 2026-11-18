/**
 * KAHF works on 17 metres only, in 18.111 to 18.120 MHz (IARU Region 1, "all modes, digimodes,
 * automatically controlled data stations"). The band is cut into 12 narrow lanes, 480 Hz wide on
 * a 700 Hz pitch. This list is the same table as crates/kahf-core/src/lanes.rs and docs/MESH-SPEC.md
 * 2.2; lib/bands.test.ts checks every number against that table. The plan is marked unverified in
 * the engine, so nothing can transmit on it yet.
 *
 * `khz` is the radio's USB dial setting (the lane centre minus 1500 Hz of audio), not the lane
 * centre: tuning the dial to the centre would put the signal 1.5 kHz too high.
 */
export type LaneRole = "relay" | "calling" | "spare";

export interface Lane {
  k: number;
  role: LaneRole;
  /** Short label for the dial: R1 to R8 relay, C1 and C2 calling, X1 and X2 spare. */
  name: string;
  lowerHz: number;
  centreHz: number;
  /** USB dial, in kHz (a number with up to two decimals). */
  khz: number;
}

const ROLES: LaneRole[] = ["relay", "relay", "relay", "relay", "spare", "calling", "calling", "spare", "relay", "relay", "relay", "relay"];

function label(k: number): string {
  let r = 0;
  let c = 0;
  let x = 0;
  let out = "";
  for (let i = 0; i <= k; i++) {
    if (ROLES[i] === "relay") out = `R${++r}`;
    else if (ROLES[i] === "calling") out = `C${++c}`;
    else out = `X${++x}`;
  }
  return out;
}

/** The 12 lanes, lowest frequency first (the order they sit on the dial). */
export const BANDS: readonly Lane[] = ROLES.map((role, k) => {
  const lowerHz = 18_111_200 + 700 * k;
  const centreHz = lowerHz + 240;
  return { k, role, name: label(k), lowerHz, centreHz, khz: (centreHz - 1500) / 1000 };
});

/** Where a new station starts: the first calling lane, where operators listen. */
export const DEFAULT_LANE = BANDS[5];

export const BAND_NAME = "17 m";

/** Index of the lane the dial frequency belongs to, or -1 for a frequency set some other way. */
export function bandIndex(khz: number): number {
  return BANDS.findIndex((b) => Math.abs(b.khz - khz) < 0.001);
}

/** A saved frequency from an older version (20, 40 or 80 m) becomes the default lane. */
export function laneKhzOrDefault(khz: number): number {
  return bandIndex(khz) >= 0 ? khz : DEFAULT_LANE.khz;
}

export function roleWords(role: LaneRole): string {
  return role === "relay" ? "relay lane" : role === "calling" ? "calling lane" : "spare lane";
}
