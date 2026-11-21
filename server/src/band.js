// The 17 m lane plan: the one table the whole server derives its channels from.
// It mirrors crates/kahf-core/src/lanes.rs (BandTable::builtin_r1) and the pool rule of
// crates/kahf-core/src/select.rs (Plan::default_plan); test/band.test.mjs checks the figures.
// A "channel" in schedules, busy zones and pools is a lane number 0..11.

/** The one band segment KAHF uses, in kHz. Lower edge included, upper edge not. */
export const BAND_SEGMENT_KHZ = Object.freeze({ low: 18_111, high: 18_120 });

/** Width of one lane's signal and the pitch between lane lower edges, in Hz. */
export const LANE_WIDTH_HZ = 480;
export const LANE_PITCH_HZ = 700;
/** Audio centre of a lane's signal above the USB dial, in Hz. */
export const AUDIO_CENTRE_HZ = 1500;
/** Lower edge of lane 0 in Hz. */
export const FIRST_LOWER_HZ = 18_111_200;

/** Role of each lane, in lane order: R1-R4, X1, C1, C2, X2, R5-R8. */
const ROLES = ["R1", "R2", "R3", "R4", "X1", "C1", "C2", "X2", "R5", "R6", "R7", "R8"];

/** Pool of a role: relay lanes are the online pool, calling lanes open, spare lanes emergency. */
const POOL_OF = { R: "online", C: "open", X: "emergency" };

/**
 * The twelve lanes. lowerHz is the lower edge of the signal, centreHz its centre and dialHz
 * the USB dial setting (centre minus the 1500 Hz audio centre).
 * @type {ReadonlyArray<Readonly<{ k: number, role: string, pool: "online" | "open" | "emergency", lowerHz: number, centreHz: number, dialHz: number }>>}
 */
export const LANES = Object.freeze(
  ROLES.map((role, k) => {
    const lowerHz = FIRST_LOWER_HZ + LANE_PITCH_HZ * k;
    const centreHz = lowerHz + LANE_WIDTH_HZ / 2;
    return Object.freeze({ k, role, pool: POOL_OF[role[0]], lowerHz, centreHz, dialHz: centreHz - AUDIO_CENTRE_HZ });
  }),
);

/** Number of lanes (channels). */
export const BAND_CHANNELS = LANES.length;

/** Lane numbers of each pool, ascending. */
export const POOLS = Object.freeze({
  online: Object.freeze(LANES.filter((l) => l.pool === "online").map((l) => l.k)),
  open: Object.freeze(LANES.filter((l) => l.pool === "open").map((l) => l.k)),
  emergency: Object.freeze(LANES.filter((l) => l.pool === "emergency").map((l) => l.k)),
});

/**
 * Lane for a frequency in kHz: the lane whose 700 Hz cell (lower edge up to the next lane's
 * lower edge) holds it, or null outside the lanes. Integer Hz arithmetic keeps edges exact.
 * Reports carry whole kHz, so only some lanes can be named that way (see reportKhz).
 * @param {number} frequencyKhz
 * @returns {number | null}
 */
export function channelOf(frequencyKhz) {
  if (typeof frequencyKhz !== "number" || !Number.isFinite(frequencyKhz)) return null;
  const hz = Math.round(frequencyKhz * 1000);
  const offset = hz - FIRST_LOWER_HZ;
  if (offset < 0) return null;
  const k = Math.floor(offset / LANE_PITCH_HZ);
  return k < BAND_CHANNELS ? k : null;
}

/**
 * A whole-kHz frequency that maps to lane k, or null when no whole kHz falls in that lane's
 * cell (lanes 0, 3, 7 and 10). Used by tools that must file a report for a lane.
 * @param {number} k
 * @returns {number | null}
 */
export function reportKhz(k) {
  const lane = LANES[k];
  if (!lane) return null;
  const first = Math.ceil(lane.lowerHz / 1000);
  return first * 1000 < lane.lowerHz + LANE_PITCH_HZ ? first : null;
}
