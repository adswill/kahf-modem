// A pretend radio, so the whole app can be tried (and tested) without any hardware.
// It behaves like the real engine: spectrum frames, stations appearing, connections that succeed or
// fail, messages that go out in steps, and link quality that changes.

import { defaultSettings, MODE_TABLE } from "./defaults";
import type { Engine, EngineEvent, MessageKind, Metrics, ModeInfo, Settings } from "./types";

const STORE = "kahf.settings";
const NEIGHBOURS = ["KA1ABC", "DL5XYZ", "G4HAM", "JA1QRP", "VK3FIX", "W6RELAY", "OH2NET", "PY2LAB"];

export class MockEngine implements Engine {
  readonly demo = true;
  private listeners = new Set<(e: EngineEvent) => void>();
  private timers: number[] = [];
  private settings: Settings = defaultSettings();
  private linkedTo: string | null = null;
  private snr = 4;
  private known = new Set<string>();

  async loadSettings(): Promise<Settings> {
    try {
      const raw = localStorage.getItem(STORE);
      if (raw) return { ...defaultSettings(), ...JSON.parse(raw) };
    } catch {
      /* private mode: fall through */
    }
    return defaultSettings();
  }

  async saveSettings(s: Settings): Promise<void> {
    this.settings = s;
    try {
      localStorage.setItem(STORE, JSON.stringify(s));
    } catch {
      /* ignore */
    }
  }

  onEvent(cb: (e: EngineEvent) => void): () => void {
    this.listeners.add(cb);
    return () => this.listeners.delete(cb);
  }

  private emit(e: EngineEvent) {
    for (const l of this.listeners) l(e);
  }

  private later(ms: number, fn: () => void) {
    this.timers.push(window.setTimeout(fn, ms));
  }

  async start(s: Settings): Promise<void> {
    this.settings = s;
    this.timers.forEach((t) => clearInterval(t));
    this.timers = [];
    this.emit({ type: "log", text: "Demo radio started. Everything you see is simulated." });
    this.timers.push(window.setInterval(() => this.emit({ type: "spectrum", bins: this.spectrum() }), 80));
    this.timers.push(
      window.setInterval(() => {
        const call = NEIGHBOURS[Math.floor(Math.random() * NEIGHBOURS.length)];
        this.known.add(call);
        this.emit({ type: "heard", heard: { call, snr: Math.round(-6 + Math.random() * 22), at: Date.now() } });
      }, 6000),
    );
    this.timers.push(
      window.setInterval(() => {
        this.snr = Math.max(-8, Math.min(18, this.snr + (Math.random() - 0.5) * 3));
        if (this.linkedTo) this.emit({ type: "metrics", metrics: this.metrics() });
      }, 2000),
    );
    this.later(2500, () => {
      this.known.add("KA1ABC");
      this.emit({ type: "heard", heard: { call: "KA1ABC", snr: 9, at: Date.now() } });
    });
    this.later(30000, () =>
      this.emit({ type: "message", from: "KA1ABC", text: "Welcome to the network! Good to hear you on 20 metres.", at: Date.now() }),
    );
  }

  private metrics(): Metrics {
    const mode = this.pickMode(this.snr);
    const info = MODE_TABLE.find((m) => m.name === mode.name) ?? MODE_TABLE[3];
    const chosen = this.settings.autoMode ? info : (MODE_TABLE.find((m) => m.name === this.settings.fixedMode) ?? info);
    return {
      mode: chosen.name,
      auto: this.settings.autoMode,
      netBps: chosen.netBps,
      ownSnr: Math.round(this.snr * 10) / 10,
      peerSnr: Math.round((this.snr - 1.5) * 10) / 10,
      ferPct: this.snr < chosen.needSnr ? 14 : 0,
      sent: 0,
      received: 0,
      retransmits: 0,
    };
  }

  private pickMode(snr: number): ModeInfo {
    const ok = MODE_TABLE.filter((m) => m.needSnr + 1 <= snr);
    return ok.length ? ok[ok.length - 1] : MODE_TABLE[0];
  }

  /** Audio 0 to 4 kHz in dB: a noise floor and a few drifting signals. */
  private spectrum(): number[] {
    const n = 512;
    const t = performance.now() / 1000;
    const out: number[] = [];
    for (let i = 0; i < n; i++) {
      let db = -92 + Math.random() * 5;
      const f = (i / n) * 4000;
      // The channel we are tuned to, busy now and then.
      const burst = Math.sin(t / 3) > 0.2 ? 1 : 0;
      if (f > 300 && f < 2700) db += burst * (30 + Math.random() * 3);
      // A narrow signal that drifts.
      const drift = 3300 + Math.sin(t / 4) * 120;
      db += 28 * Math.exp(-(((f - drift) / 14) ** 2));
      out.push(db);
    }
    return out;
  }

  async connect(call: string): Promise<void> {
    this.emit({ type: "log", text: `Contacting ${call}.` });
    this.callPeer(call, 6);
  }

  private callPeer(call: string, of: number, attempt = 1, done?: (ok: boolean) => void) {
    this.emit({ type: "link", link: { state: "calling", peer: call, attempt, of } });
    this.later(1600, () => {
      const answers = this.known.has(call) && Math.random() < 0.8;
      if (answers) {
        this.linkedTo = call;
        this.emit({ type: "link", link: { state: "linked", peer: call } });
        this.emit({ type: "metrics", metrics: this.metrics() });
        done?.(true);
      } else if (attempt < of) {
        this.callPeer(call, of, attempt + 1, done);
      } else {
        this.emit({ type: "link", link: { state: "idle" } });
        this.emit({ type: "log", text: `${call} did not answer.` });
        done?.(false);
      }
    });
  }

  async disconnect(): Promise<void> {
    this.linkedTo = null;
    this.emit({ type: "link", link: { state: "idle" } });
  }

  async send(id: string, to: string, _text: string, _kind: MessageKind): Promise<void> {
    const go = () => {
      this.emit({ type: "tx", state: "starting" });
      this.later(700, () => {
        this.emit({ type: "tx", state: "sending" });
        this.later(1500, () => {
          this.emit({ type: "tx", state: "done" });
          this.emit({ type: "sent", id });
          if (this.settings.autoLink && this.linkedTo === to) {
            this.later(8000, () => this.disconnect());
          }
        });
      });
    };
    if (this.linkedTo === to) return go();
    this.callPeer(to, 4, 1, (ok) => {
      if (ok) go();
      else this.emit({ type: "failed", id, reason: `${to} did not answer on this frequency.` });
    });
  }

  async setAutoMode(on: boolean) {
    this.settings.autoMode = on;
  }
  async setFixedMode(name: string) {
    this.settings.fixedMode = name;
    this.settings.autoMode = false;
  }
  async setListenOnly(on: boolean) {
    this.settings.listenOnly = on;
  }
  async setAutoLink(on: boolean) {
    this.settings.autoLink = on;
  }
  async setRelay(on: boolean, _l: boolean, _t: boolean) {
    this.emit({ type: "log", text: on ? "Relay mode is on (demo)." : "Relay mode is off." });
  }
  async shareOnMap(): Promise<string> {
    const loc = this.settings.location;
    if (loc.mode === "off") return "You chose not to share your location, so nothing was sent.";
    if (!this.settings.relay) return "The map lists relays for now. Turn on relay mode to appear on it.";
    return "Demo: your relay would be on the map now.";
  }
  async listAudioDevices(): Promise<string[]> {
    return ["MacBook Pro Microphone", "BlackHole 2ch", "USB Audio CODEC (radio interface)"];
  }
  async detectSdr(): Promise<string[]> {
    return ["HackRF One (demo)"];
  }
  async modes(): Promise<ModeInfo[]> {
    return MODE_TABLE;
  }
}
