import { beforeEach, describe, expect, it, vi } from "vitest";
import { MockEngine } from "./mock";
import type { EngineEvent } from "./types";

// The demo engine has to behave like the real one: this is what the screens are built against.
beforeEach(() => {
  vi.useFakeTimers();
  const store = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => void store.set(k, v),
  });
  vi.stubGlobal("window", { setTimeout: (f: () => void, ms: number) => setTimeout(f, ms), setInterval: (f: () => void, ms: number) => setInterval(f, ms) });
});

describe("demo engine", () => {
  it("keeps settings between runs", async () => {
    const e = new MockEngine();
    const s = await e.loadSettings();
    await e.saveSettings({ ...s, callsign: "G4ABC" });
    expect((await new MockEngine().loadSettings()).callsign).toBe("G4ABC");
  });

  it("hears stations and emits spectrum frames", async () => {
    const e = new MockEngine();
    const seen: EngineEvent[] = [];
    e.onEvent((x) => seen.push(x));
    await e.start(await e.loadSettings());
    await vi.advanceTimersByTimeAsync(3000);
    expect(seen.some((x) => x.type === "spectrum")).toBe(true);
    expect(seen.some((x) => x.type === "heard" && x.heard.call === "KA1ABC")).toBe(true);
    const frame = seen.find((x) => x.type === "spectrum");
    expect(frame && frame.type === "spectrum" && frame.bins.length).toBe(512);
  });

  it("sends a message to a station it has heard, step by step", async () => {
    const e = new MockEngine();
    const seen: EngineEvent[] = [];
    e.onEvent((x) => seen.push(x));
    await e.start(await e.loadSettings());
    await vi.advanceTimersByTimeAsync(3000);
    await e.send("m1", "KA1ABC", "hello", "chat");
    await vi.advanceTimersByTimeAsync(60000);
    const types = seen.map((x) => x.type);
    expect(types).toContain("tx");
    expect(seen.some((x) => x.type === "sent" && x.id === "m1") || seen.some((x) => x.type === "failed" && x.id === "m1")).toBe(true);
  });

  it("fails a message to a station nobody has heard, with a reason", async () => {
    const e = new MockEngine();
    const seen: EngineEvent[] = [];
    e.onEvent((x) => seen.push(x));
    await e.start(await e.loadSettings());
    await e.send("m2", "ZZ9ZZZ", "anyone?", "chat");
    await vi.advanceTimersByTimeAsync(30000);
    const f = seen.find((x) => x.type === "failed");
    expect(f && f.type === "failed" && f.reason).toMatch(/did not answer/);
    expect(seen.filter((x) => x.type === "link" && x.link.state === "calling").length).toBeGreaterThan(1);
  });

  it("switches between automatic and a fixed speed", async () => {
    const e = new MockEngine();
    await e.setFixedMode("KAMI-64");
    await e.setAutoMode(true);
    expect((await e.modes()).map((m) => m.name)).toContain("KAMI-1d");
  });
});
