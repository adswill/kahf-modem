/** The channels KAHF knows, lowest frequency first (the order they sit on the dial). */
export const BANDS = [
  { khz: 3603, name: "80 m" },
  { khz: 7103, name: "40 m" },
  { khz: 14103, name: "20 m" },
] as const;

/** Index of the band the frequency belongs to, or -1 for a frequency set some other way. */
export function bandIndex(khz: number): number {
  return BANDS.findIndex((b) => b.khz === khz);
}
