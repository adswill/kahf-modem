import { describe, expect, it } from "vitest";
import { BANDS, DEFAULT_CHANNEL, bandIndex, channelKhzOrDefault } from "./bands";

describe("17 m channels", () => {
  it("are the three of the band plan, 2.7 kHz apart, inside 18.111 to 18.120 MHz", () => {
    expect(BANDS.map((b) => b.name)).toEqual(["Ch 1", "Ch 2", "Ch 3"]);
    expect(BANDS.map((b) => b.centreHz)).toEqual([18_112_350, 18_115_050, 18_117_750]);
    for (const b of BANDS) expect(b.lowerHz >= 18_111_000 && b.lowerHz + 2_700 <= 18_120_000).toBe(true);
  });
  it("find a saved frequency, and send other bands to the first channel", () => {
    expect(bandIndex(18115.05)).toBe(1);
    expect(bandIndex(14103)).toBe(-1);
    for (const old of [14103, 7103, 3603, 0, 18113.44]) expect(channelKhzOrDefault(old)).toBe(DEFAULT_CHANNEL.khz);
    expect(channelKhzOrDefault(18117.75)).toBe(18117.75);
    expect(DEFAULT_CHANNEL.name).toBe("Ch 1");
  });
});
