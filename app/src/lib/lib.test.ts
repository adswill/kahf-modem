import { describe, expect, it } from "vitest";
import { callProblem, normaliseCall, validCall } from "./callsign";
import { latLonToLocator, locatorToLatLon, sharingSummary, validLat, validLon } from "./geo";
import { snrQuality, speedWords, timeAgo, mhz, modeExplained } from "./format";
import { defaultSettings, MODE_TABLE } from "./defaults";

describe("callsigns", () => {
  it("accepts ordinary callsigns and normalises case", () => {
    expect(normaliseCall(" ka1abc ")).toBe("KA1ABC");
    expect(validCall("KA1ABC")).toBe(true);
    expect(validCall("G4ABC")).toBe(true);
    expect(validCall("DL5XYZ")).toBe(true);
  });
  it("rejects what cannot be a callsign", () => {
    expect(validCall("AB")).toBe(false);
    expect(validCall("ABCDEF")).toBe(false); // no number
    expect(validCall("123456")).toBe(false); // no letter
    expect(validCall("KA1ABC!!")).toBe(false);
    expect(validCall("A".repeat(11) + "1")).toBe(false);
  });
  it("explains problems in plain words", () => {
    expect(callProblem("")).toBeNull();
    expect(callProblem("KA")).toMatch(/at least 3/);
    expect(callProblem("KAXYZ")).toMatch(/number/);
    expect(callProblem("KA 1")).toMatch(/letters and numbers/);
    expect(callProblem("KA1ABC")).toBeNull();
  });
});

describe("positions", () => {
  it("turns a locator into the centre of its square", () => {
    const p = locatorToLatLon("IO91wm")!;
    expect(p.lat).toBeCloseTo(51.52, 1);
    expect(p.lon).toBeCloseTo(-0.125, 1);
    const q = locatorToLatLon("JN45")!;
    expect(q.lat).toBeCloseTo(45.5, 1);
    expect(q.lon).toBeCloseTo(9, 1);
  });
  it("rejects bad locators", () => {
    for (const bad of ["", "ZZ99", "JN4", "JN45r", "12AB", "JN45rmx"]) expect(locatorToLatLon(bad)).toBeNull();
  });
  it("round-trips a position to its locator", () => {
    expect(latLonToLocator(51.5074, -0.1278)).toBe("IO91wm");
    expect(latLonToLocator(40.7128, -74.006)).toBe("FN20xr");
    expect(latLonToLocator(-33.8688, 151.2093)).toBe("QF56od");
    const p = locatorToLatLon(latLonToLocator(48.8566, 2.3522))!;
    expect(latLonToLocator(p.lat, p.lon)).toBe(latLonToLocator(48.8566, 2.3522));
  });
  it("validates coordinates", () => {
    expect(validLat(90)).toBe(true);
    expect(validLat(-90.1)).toBe(false);
    expect(validLat(null)).toBe(false);
    expect(validLon(180)).toBe(true);
    expect(validLon(181)).toBe(false);
    expect(validLon(NaN)).toBe(false);
  });
  it("says what others will see", () => {
    expect(sharingSummary("off", null, null, 50)).toMatch(/Nobody/);
    expect(sharingSummary("area", null, null, 50)).toMatch(/Enter your position/);
    expect(sharingSummary("area", 1, 2, 80)).toMatch(/80 km/);
    expect(sharingSummary("precise", 1, 2, 80)).toMatch(/200 metres/);
  });
});

describe("words for people", () => {
  it("names signal quality", () => {
    expect(snrQuality(null).label).toBe("Unknown");
    expect(snrQuality(15).label).toBe("Excellent");
    expect(snrQuality(5).label).toBe("Good");
    expect(snrQuality(-2).label).toBe("Fair");
    expect(snrQuality(-8).label).toBe("Weak");
    expect(snrQuality(-20).label).toBe("Very weak");
    expect(snrQuality(-20).tone).toBe("bad");
  });
  it("describes speed in letters", () => {
    expect(speedWords(5)).toMatch(/under 1 letter/);
    expect(speedWords(284)).toMatch(/36 letters/);
    expect(speedWords(6033)).toMatch(/page/);
  });
  it("tells time and frequency", () => {
    expect(timeAgo(1000, 2000)).toBe("just now");
    expect(timeAgo(0, 30_000)).toBe("30 seconds ago");
    expect(timeAgo(0, 90_000)).toBe("2 minutes ago");
    expect(timeAgo(0, 3_600_000)).toBe("1 hour ago");
    expect(mhz(18113.44)).toBe("18.11344 MHz");
    expect(modeExplained("KAMI-1d")).toMatch(/Fastest/);
  });
});

describe("defaults", () => {
  it("keep privacy first and automation on", () => {
    const s = defaultSettings();
    expect(s.location.mode).toBe("off");
    expect(s.autoLink).toBe(true);
    expect(s.autoMode).toBe(true);
    expect(s.listenOnly).toBe(false);
    expect(s.setupDone).toBe(false);
  });
  it("list the modes slowest first with rising speed and SNR", () => {
    for (let i = 1; i < MODE_TABLE.length; i++) {
      expect(MODE_TABLE[i].needSnr).toBeGreaterThan(MODE_TABLE[i - 1].needSnr);
      expect(MODE_TABLE[i].netBps).toBeGreaterThan(MODE_TABLE[i - 1].netBps);
    }
    expect(MODE_TABLE.at(-1)!.netBps).toBeGreaterThan(1000);
  });
});

import { explainStartError } from "./state.svelte";
describe("start errors", () => {
  it("turn engine messages into advice", () => {
    expect(explainStartError("cannot open radio: audio device missing")).toMatch(/sound device/);
    expect(explainStartError("no HackRF found")).toMatch(/SDR/);
    expect(explainStartError("cannot connect to rigctld at localhost:4532")).toMatch(/radio control/);
    expect(explainStartError("something odd")).toMatch(/could not start/);
  });
});

import { FILE_LIMIT_BYTES, durationWords, transferSeconds } from "./format";

describe("file transfer estimates", () => {
  it("scales with size and speed", () => {
    expect(transferSeconds(10_000, 1000)).toBeCloseTo(104, 0);
    expect(transferSeconds(10_000, 10)).toBe(transferSeconds(10_000, 20));
  });
  it("says it in plain words", () => {
    expect(durationWords(40)).toBe("about 40 seconds");
    expect(durationWords(300)).toBe("about 5 minutes");
    expect(durationWords(4800)).toBe("about 1 hour 20 minutes");
    expect(FILE_LIMIT_BYTES).toBe(102400);
  });
});
