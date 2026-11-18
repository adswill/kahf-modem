// A pretend radio, so the whole app can be tried (and tested) without any hardware.
// It behaves like the real engine: spectrum frames, stations appearing, connections that succeed or
// fail, messages that go out in steps, and link quality that changes.

import { defaultSettings, MODE_TABLE } from "./defaults";
import type {
  Engine, EngineEvent, MessageKind, Metrics, ModeInfo, PickedFile, RelayInfo, RelayStatus, RigModel, Settings, TxGate, Occupancy, VoiceClip,
} from "./types";

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
  private recStart = 0;
  private recTimer = 0;
  private forwarded: RelayStatus = { running: false, forwardedToday: 0, forwardedTotal: 0, recent: [], area: null };
  private relayTimer = 0;

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
    this.later(45000, () =>
      this.emit({
        type: "mail", from: "DL5XYZ", subject: "Field day plans", at: Date.now(),
        body: "Hi,\n\nWe are setting up on the hill on Saturday. Can you bring the long wire antenna?\n\n73, Jonas",
        files: [{ name: "map.png", size: 120_000 }],
      }),
    );
    this.later(30000, () =>
      this.emit({ type: "message", from: "KA1ABC", text: "Welcome to the network! Good to hear you on 17 metres.", at: Date.now() }),
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
  async sendMail(id: string, to: string, _subject: string, _body: string, paths: string[]): Promise<void> {
    this.sendAs(id, to, paths.length ? 6000 : 3000);
  }

  async sendFile(id: string, to: string, _path: string): Promise<void> {
    this.sendAs(id, to, 7000, 48_000);
  }

  /** Calls the station if needed, then sends in steps; reports progress for files. */
  private sendAs(id: string, to: string, ms: number, total = 0) {
    const go = () => {
      this.emit({ type: "tx", state: "starting" });
      this.later(600, () => {
        this.emit({ type: "tx", state: "sending" });
        if (total) {
          for (let i = 1; i <= 5; i++) {
            this.later((ms / 5) * i, () => this.emit({ type: "file", id, name: "file", dir: "out", done: Math.round((total / 5) * i), total }));
          }
        }
        this.later(ms, () => {
          this.emit({ type: "tx", state: "done" });
          this.emit({ type: "sent", id });
        });
      });
    };
    if (this.linkedTo === to) return go();
    this.callPeer(to, 4, 1, (ok) => (ok ? go() : this.emit({ type: "failed", id, reason: `${to} did not answer on this frequency.` })));
  }

  async pickFiles(): Promise<PickedFile[]> {
    // A browser cannot give real paths; the demo offers a pretend file.
    return [{ path: "/demo/Field-report.pdf", name: "Field-report.pdf", size: 48_000 }];
  }

  async openPath(_path: string): Promise<void> {}

  async recordVoice(): Promise<void> {
    this.recStart = Date.now();
    this.recTimer = window.setInterval(() => this.emit({ type: "level", level: 0.25 + Math.random() * 0.6 }), 90);
  }

  async stopVoice(): Promise<VoiceClip | null> {
    clearInterval(this.recTimer);
    const seconds = Math.max(0, Math.round((Date.now() - this.recStart) / 100) / 10);
    if (seconds < 0.5) return null;
    return { id: `clip-${Date.now()}`, seconds, bytes: Math.round(seconds * 88) };
  }

  async cancelVoice(): Promise<void> {
    clearInterval(this.recTimer);
  }

  async playVoice(_path: string): Promise<void> {}

  async stopPlayback(): Promise<void> {}

  async sendVoice(id: string, to: string, clip: VoiceClip): Promise<void> {
    this.sendAs(id, to, 4000, clip.bytes);
  }

  async relayStatus(): Promise<RelayStatus> {
    return { ...this.forwarded };
  }

  async relayDirectory(_refresh: boolean): Promise<RelayInfo[]> {
    const rows: RelayInfo[] = [
      { call: "W6RELAY", lat: 37.5, lon: -122.5, km: 8630, source: "server", lastSeen: Date.now() - 3600_000 },
      { call: "DL0NET", lat: 50.5, lon: 8.5, km: 640, source: "server", lastSeen: Date.now() - 7200_000 },
      { call: "G3HUB", lat: 52.5, lon: -1.5, km: 150, source: "heard", lastSeen: Date.now() - 600_000 },
    ];
    return rows.sort((a, b) => (a.km ?? 0) - (b.km ?? 0));
  }

  async setRelayHints(_calls: string[]): Promise<void> {}

  async setTxLevel(_pct: number): Promise<void> {}

  async testTone(): Promise<void> {
    this.emit({ type: "tx", state: "starting" });
    window.setTimeout(() => this.emit({ type: "tx", state: "done" }), 5000);
  }

  async advertise(): Promise<void> {
    this.emit({ type: "tx", state: "starting" });
    window.setTimeout(() => this.emit({ type: "tx", state: "done" }), 1500);
    // Pretend relays pass it on: stations further away answer a little later.
    [["EA4FAR", 1, 40.4, -3.7], ["JA1XYZ", 2, 35.7, 139.7]].forEach(([call, hops, lat, lon], i) =>
      window.setTimeout(() => this.emit({ type: "heard", heard: { call: String(call), snr: null, at: Date.now(), hops: Number(hops), relay: i === 0, lat: Number(lat), lon: Number(lon) } }), 2500 + i * 1800));
  }

  async rigAvailable(): Promise<boolean> {
    return true;
  }

  async rigModels(): Promise<RigModel[]> {
    const rows: [number, string, string][] = [
      [3073, "Icom", "IC-7300"], [3085, "Icom", "IC-705"], [3070, "Icom", "IC-7100"], [3078, "Icom", "IC-7610"],
      [1035, "Yaesu", "FT-991"], [1036, "Yaesu", "FT-891"], [1042, "Yaesu", "FT-DX10"], [1003, "Yaesu", "FT-817"],
      [2031, "Kenwood", "TS-590S"], [2041, "Kenwood", "TS-890S"], [2029, "Elecraft", "K3"], [2044, "Elecraft", "KX2"],
      [1, "Hamlib", "Dummy"],
    ];
    return rows.map(([id, maker, model]) => ({ id, maker, model, status: "Stable" }));
  }

  async rigPorts(): Promise<string[]> {
    return ["/dev/cu.usbserial-1420", "/dev/cu.SLAB_USBtoUART"];
  }

  async rigConnect(_model: number, port: string, _baud: number): Promise<number> {
    if (!port) throw new Error("Choose the serial port first.");
    return 14_103_000;
  }

  async rigStop(): Promise<void> {}

  async setRelay(on: boolean, _l: boolean, _t: boolean) {
    this.emit({ type: "log", text: on ? "Relay mode is on (demo)." : "Relay mode is off." });
    clearInterval(this.relayTimer);
    this.forwarded.running = on;
    this.forwarded.area = on && this.settings.location.mode !== "off" ? "N51W1" : null;
    if (on) {
      this.relayTimer = window.setInterval(() => {
        const from = NEIGHBOURS[Math.floor(Math.random() * 4)];
        const to = NEIGHBOURS[4 + Math.floor(Math.random() * 4)];
        this.forwarded = {
          ...this.forwarded,
          forwardedToday: this.forwarded.forwardedToday + 1,
          forwardedTotal: this.forwarded.forwardedTotal + 1,
          recent: [{ at: Date.now(), from, to, kind: "message" }, ...this.forwarded.recent].slice(0, 20),
        };
        this.emit({ type: "relay", status: { ...this.forwarded } });
      }, 9000);
    }
    this.emit({ type: "relay", status: { ...this.forwarded } });
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
  async occupancy(): Promise<Occupancy | null> {
    return { at: Math.floor(Date.now() / 1000), lanes: [[4, 0], [5, 2], [6, 14], [7, 0], [8, 0]] };
  }
  async txGate(): Promise<TxGate> {
    return { mode: "bench", text: "Test table: bench radio only, nothing reaches an antenna." };
  }
}
