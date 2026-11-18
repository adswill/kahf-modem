import { describe, expect, it } from "vitest";
import { BANDS, DEFAULT_LANE, bandIndex, laneKhzOrDefault } from "./bands";

// docs/MESH-SPEC.md 2.2 and crates/kahf-core/src/lanes.rs: (k, name, lower, centre, dial in Hz).
const TABLE: [number, string, number, number, number][] = [
  [0, "R1", 18_111_200, 18_111_440, 18_109_940],
  [1, "R2", 18_111_900, 18_112_140, 18_110_640],
  [2, "R3", 18_112_600, 18_112_840, 18_111_340],
  [3, "R4", 18_113_300, 18_113_540, 18_112_040],
  [4, "X1", 18_114_000, 18_114_240, 18_112_740],
  [5, "C1", 18_114_700, 18_114_940, 18_113_440],
  [6, "C2", 18_115_400, 18_115_640, 18_114_140],
  [7, "X2", 18_116_100, 18_116_340, 18_114_840],
  [8, "R5", 18_116_800, 18_117_040, 18_115_540],
  [9, "R6", 18_117_500, 18_117_740, 18_116_240],
  [10, "R7", 18_118_200, 18_118_440, 18_116_940],
  [11, "R8", 18_118_900, 18_119_140, 18_117_640],
];

describe("17 m lanes", () => {
  it("are the 12 of the spec table, with the dial and not the centre", () => {
    expect(BANDS).toHaveLength(12);
    for (const [k, name, lower, centre, dial] of TABLE) {
      const b = BANDS[k];
      expect([b.name, b.lowerHz, b.centreHz, Math.round(b.khz * 1000)]).toEqual([name, lower, centre, dial]);
    }
  });
  it("are 8 relay, 2 calling and 2 spare, inside 18.111 to 18.120 MHz", () => {
    expect(BANDS.filter((b) => b.role === "relay")).toHaveLength(8);
    expect(BANDS.filter((b) => b.role === "calling").map((b) => b.name)).toEqual(["C1", "C2"]);
    expect(BANDS.filter((b) => b.role === "spare").map((b) => b.name)).toEqual(["X1", "X2"]);
    for (const b of BANDS) expect(b.lowerHz >= 18_111_000 && b.lowerHz + 480 <= 18_120_000).toBe(true);
  });
  it("find a saved frequency, and send 20, 40 and 80 m settings to the default lane", () => {
    expect(bandIndex(18113.44)).toBe(5);
    expect(bandIndex(14103)).toBe(-1);
    for (const old of [14103, 7103, 3603, 0]) expect(laneKhzOrDefault(old)).toBe(DEFAULT_LANE.khz);
    expect(laneKhzOrDefault(18110.64)).toBe(18110.64);
    expect(DEFAULT_LANE.name).toBe("C1");
  });
});
