/**
 * KAHF works on 17 metres only, in 18.111 to 18.120 MHz (IARU Region 1, "all modes, digimodes,
 * automatically controlled data stations"). The band holds three KAHF channels, each 2.7 kHz wide.
 * This is the same plan as the engine's default plan (crates/kahf-core/src/select.rs); the Rust
 * test `the_17_m_channels_come_from_the_band_plan` and lib/bands.test.ts check the same numbers.
 *
 * `khz` is the channel's centre. Relay mode uses all three channels.
 */
export interface Channel {
  /** 0 to 2, lowest frequency first. */
  k: number;
  /** Short label for the dial: "Ch 1" to "Ch 3". */
  name: string;
  lowerHz: number;
  centreHz: number;
  /** Centre in kHz (a number with up to two decimals). */
  khz: number;
}

const BAND_LOW_HZ = 18_111_000;
const BAND_HIGH_HZ = 18_120_000;
const SPACING_HZ = 2_700;

/** The channels, lowest frequency first (the order they sit on the dial). */
export const BANDS: readonly Channel[] = (() => {
  const out: Channel[] = [];
  for (let c = BAND_LOW_HZ + SPACING_HZ / 2; c + SPACING_HZ / 2 <= BAND_HIGH_HZ; c += SPACING_HZ) {
    const k = out.length;
    out.push({ k, name: `Ch ${k + 1}`, lowerHz: c - SPACING_HZ / 2, centreHz: c, khz: c / 1000 });
  }
  return out;
})();

/** Where a new station starts: the first channel. */
export const DEFAULT_CHANNEL = BANDS[0];

export const BAND_NAME = "17 m";

/** Index of the channel the frequency belongs to, or -1 for a frequency set some other way. */
export function bandIndex(khz: number): number {
  return BANDS.findIndex((b) => Math.abs(b.khz - khz) < 0.001);
}

/** A saved frequency from an older version (20, 40 or 80 m, or a lane of the lane test version) becomes the first channel. */
export function channelKhzOrDefault(khz: number): number {
  return bandIndex(khz) >= 0 ? khz : DEFAULT_CHANNEL.khz;
}
