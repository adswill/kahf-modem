// The one place that holds what the window shows. Pages read from here and call its methods;
// they never talk to the engine directly.

import { callProblem, normaliseCall, validCall } from "./callsign";
import { defaultSettings } from "./defaults";
import { makeEngine } from "./engine";
import type { Engine, EngineEvent, Heard, LinkState, Metrics, MessageKind, ModeInfo, Msg, Settings } from "./types";

export type Page = "welcome" | "setup" | "home" | "messages" | "network" | "radio" | "help" | "settings";

export interface Toast {
  id: number;
  text: string;
  kind: "info" | "good" | "warn";
}

export interface Step {
  id: string;
  title: string;
  why: string;
  done: boolean;
  go: Page;
  action: string;
}

const MSG_STORE = "kahf.messages";
const HEARD_STORE = "kahf.heard";

function load<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key);
    return raw ? (JSON.parse(raw) as T) : fallback;
  } catch {
    return fallback;
  }
}

function save(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    /* ignore */
  }
}

/** Turns what the engine said into advice. */
export function explainStartError(raw: string): string {
  const t = raw.toLowerCase();
  if (t.includes("audio") || t.includes("device") || t.includes("sound")) {
    return "KAHF could not open the sound device. Check that your radio interface is plugged in and not used by another program, then choose it again on the Radio page.";
  }
  if (t.includes("sdr") || t.includes("hackrf") || t.includes("rtl")) {
    return "KAHF could not find the SDR receiver. Plug it in, make sure its driver is installed, and try again.";
  }
  if (t.includes("rig") || t.includes("rigctld") || t.includes("4532")) {
    return "KAHF could not reach the radio control program. Check its address in Settings, or leave it empty.";
  }
  return `The radio could not start. ${raw}`;
}

class AppState {
  engine: Engine = makeEngine();
  ready = $state(false);
  page = $state<Page>("welcome");
  settings = $state<Settings>(defaultSettings());
  link = $state<LinkState>({ state: "idle" });
  heard = $state<Heard[]>([]);
  messages = $state<Msg[]>([]);
  metrics = $state<Metrics | null>(null);
  modes = $state<ModeInfo[]>([]);
  log = $state<{ at: number; text: string }[]>([]);
  spectrum = $state<number[]>([]);
  transmitting = $state(false);
  toasts = $state<Toast[]>([]);
  peer = $state<string>("");
  tourOpen = $state(false);
  /** Why the radio could not start, in plain words. Empty when all is well. */
  engineError = $state("");
  helpTopic = $state<string>("");
  private toastId = 0;

  // ---- derived ---------------------------------------------------------------------------

  get hasCall(): boolean {
    return validCall(this.settings.callsign);
  }

  get unread(): number {
    return this.messages.filter((m) => m.dir === "in" && !m.read).length;
  }

  get steps(): Step[] {
    const s = this.settings;
    return [
      {
        id: "call",
        title: "Enter your callsign",
        why: "It is your name on the air. Everyone you talk to sees it.",
        done: this.hasCall,
        go: "setup",
        action: "Set callsign",
      },
      {
        id: "radio",
        title: "Choose how KAHF hears your radio",
        why: "Pick the sound card or receiver that your radio is connected to.",
        done: s.setupDone && (s.audioIn !== "" || s.sdr !== "" || this.engine.demo),
        go: "setup",
        action: "Choose radio",
      },
      {
        id: "listen",
        title: "Hear your first station",
        why: "Leave KAHF running for a few minutes. Stations nearby appear on their own.",
        done: this.heard.length > 0,
        go: "network",
        action: "See who is on",
      },
      {
        id: "send",
        title: "Send your first message",
        why: "Pick a station you heard, write a line, and press Send. KAHF does the rest.",
        done: this.messages.some((m) => m.dir === "out" && m.state === "sent"),
        go: "messages",
        action: "Write a message",
      },
    ];
  }

  get progress(): { done: number; total: number } {
    const steps = this.steps;
    return { done: steps.filter((x) => x.done).length, total: steps.length };
  }

  /** One sentence for the top of every page: what is happening right now. */
  get status(): { tone: "idle" | "good" | "busy" | "warn"; title: string; detail: string } {
    if (!this.hasCall) {
      return { tone: "warn", title: "Not set up yet", detail: "Enter your callsign to get started." };
    }
    if (this.settings.listenOnly) {
      return { tone: "idle", title: "Listening only", detail: "KAHF will not transmit. Turn this off in Settings when you are ready." };
    }
    if (this.transmitting) {
      return { tone: "busy", title: "Transmitting", detail: "Your signal is on the air. Wait a moment." };
    }
    if (this.link.state === "calling") {
      return {
        tone: "busy",
        title: `Contacting ${this.link.peer}`,
        detail:
          this.link.of > 0
            ? `Try ${this.link.attempt} of ${this.link.of}. KAHF keeps trying by itself.`
            : "KAHF keeps trying by itself. This can take a minute.",
      };
    }
    if (this.link.state === "linked") {
      return { tone: "good", title: `Connected to ${this.link.peer}`, detail: "Messages go straight to them." };
    }
    const n = this.heard.length;
    return {
      tone: "idle",
      title: "Listening",
      detail: n === 0 ? "Nobody has been heard yet. This can take a few minutes." : `${n} station${n === 1 ? "" : "s"} heard so far.`,
    };
  }

  // ---- startup ---------------------------------------------------------------------------

  async init() {
    this.settings = await this.engine.loadSettings();
    this.messages = load<Msg[]>(MSG_STORE, []);
    this.heard = load<Heard[]>(HEARD_STORE, []);
    this.applyTheme();
    this.modes = await this.engine.modes();
    this.engine.onEvent((e) => this.handle(e));
    this.page = this.settings.setupDone && this.hasCall ? "home" : "welcome";
    if (this.settings.setupDone) await this.startEngine();
    this.ready = true;
  }

  applyTheme() {
    document.documentElement.dataset.theme = this.settings.theme;
  }

  go(page: Page, topic = "") {
    this.page = page;
    if (topic) this.helpTopic = topic;
    if (page === "messages") this.markRead();
  }

  async saveSettings(patch: Partial<Settings>) {
    this.settings = { ...this.settings, ...patch };
    await this.engine.saveSettings($state.snapshot(this.settings) as Settings);
    this.applyTheme();
  }

  /** The end of setup: remember it and start the radio. */
  async finishSetup(patch: Partial<Settings>) {
    await this.saveSettings({ ...patch, setupDone: true });
    const ok = await this.startEngine();
    this.page = "home";
    if (ok) this.toast("All set. KAHF is listening for stations.", "good");
  }

  /** Starts the radio. Never throws: a problem becomes a sentence the person can act on. */
  async startEngine(): Promise<boolean> {
    try {
      await this.engine.start($state.snapshot(this.settings) as Settings);
      this.engineError = "";
      return true;
    } catch (e) {
      this.engineError = explainStartError(String(e));
      return false;
    }
  }

  // ---- doing things ----------------------------------------------------------------------

  async call(peer: string) {
    const to = normaliseCall(peer);
    if (callProblem(to) || to === "") {
      this.toast("That does not look like a callsign.", "warn");
      return;
    }
    await this.engine.connect(to);
  }

  async hangUp() {
    await this.engine.disconnect();
  }

  async send(to: string, text: string, kind: MessageKind = "chat") {
    const peer = normaliseCall(to);
    const body = text.trim();
    if (peer === "" || body === "") return;
    if (callProblem(peer)) {
      this.toast(`"${peer}" does not look like a callsign.`, "warn");
      return;
    }
    if (this.settings.listenOnly) {
      this.toast("Listen-only is on, so nothing can be sent. Turn it off in Settings.", "warn");
      return;
    }
    const id = `${Date.now()}-${Math.floor(Math.random() * 1e6)}`;
    const msg: Msg = { id, peer, dir: "out", text: body, at: Date.now(), kind, state: "queued", read: true };
    this.messages = [...this.messages, msg];
    this.persistMessages();
    try {
      await this.engine.send(id, peer, body, kind);
    } catch (e) {
      this.update(id, { state: "failed", note: String(e) });
    }
  }

  async setAutoMode(on: boolean) {
    await this.engine.setAutoMode(on);
    await this.saveSettings({ autoMode: on });
  }

  async setFixedMode(name: string) {
    await this.engine.setFixedMode(name);
    await this.saveSettings({ fixedMode: name, autoMode: false });
  }

  async setListenOnly(on: boolean) {
    await this.engine.setListenOnly(on);
    await this.saveSettings({ listenOnly: on });
  }

  async setAutoLink(on: boolean) {
    await this.engine.setAutoLink(on);
    await this.saveSettings({ autoLink: on });
  }

  discard(id: string) {
    this.messages = this.messages.filter((m) => m.id !== id);
    this.persistMessages();
  }

  async setRelay(on: boolean, licence: boolean, terms: boolean) {
    await this.engine.setRelay(on, licence, terms);
    await this.saveSettings({ relay: on });
    if (on) await this.shareOnMap();
  }

  /** Tell the person what the map now shows about them, in a sentence. */
  async shareOnMap() {
    try {
      this.toast(await this.engine.shareOnMap(), "info");
    } catch (e) {
      this.toast(String(e), "warn");
    }
  }

  markRead() {
    if (this.messages.some((m) => m.dir === "in" && !m.read)) {
      this.messages = this.messages.map((m) => (m.dir === "in" ? { ...m, read: true } : m));
      this.persistMessages();
    }
  }

  toast(text: string, kind: Toast["kind"] = "info") {
    const id = ++this.toastId;
    this.toasts = [...this.toasts, { id, text, kind }];
    window.setTimeout(() => (this.toasts = this.toasts.filter((t) => t.id !== id)), 5200);
  }

  // ---- events from the engine ------------------------------------------------------------

  private update(id: string, patch: Partial<Msg>) {
    this.messages = this.messages.map((m) => (m.id === id ? { ...m, ...patch } : m));
    this.persistMessages();
  }

  private persistMessages() {
    save(MSG_STORE, this.messages.slice(-500));
  }

  handle(e: EngineEvent) {
    switch (e.type) {
      case "link": {
        this.link = e.link;
        if (e.link.state === "calling") {
          for (const m of this.messages) {
            if (m.dir === "out" && m.peer === e.link.peer && (m.state === "queued" || m.state === "calling")) {
              this.update(m.id, { state: "calling" });
            }
          }
        }
        break;
      }
      case "message": {
        this.messages = [
          ...this.messages,
          { id: `in-${e.at}-${this.messages.length}`, peer: e.from, dir: "in", text: e.text, at: e.at, kind: "chat", read: this.page === "messages" },
        ];
        this.persistMessages();
        if (this.page !== "messages") this.toast(`New message from ${e.from}`, "info");
        break;
      }
      case "heard": {
        const rest = this.heard.filter((h) => h.call !== e.heard.call);
        this.heard = [e.heard, ...rest].slice(0, 60);
        save(HEARD_STORE, this.heard);
        break;
      }
      case "metrics":
        this.metrics = e.metrics;
        break;
      case "spectrum":
        this.spectrum = e.bins;
        break;
      case "tx":
        this.transmitting = e.state === "starting" || e.state === "sending";
        if (e.state === "sending") {
          for (const m of this.messages) if (m.dir === "out" && (m.state === "queued" || m.state === "calling")) this.update(m.id, { state: "sending" });
        }
        break;
      case "sent":
        this.update(e.id, { state: "sent", note: undefined });
        break;
      case "failed":
        this.update(e.id, { state: "failed", note: e.reason });
        this.toast(e.reason, "warn");
        break;
      case "log":
        this.log = [{ at: Date.now(), text: e.text }, ...this.log].slice(0, 80);
        break;
    }
  }

  // ---- views over the data ---------------------------------------------------------------

  /** The people you have talked to, most recent first. */
  get conversations(): { peer: string; last: Msg; unread: number }[] {
    const by = new Map<string, Msg[]>();
    for (const m of this.messages) by.set(m.peer, [...(by.get(m.peer) ?? []), m]);
    return [...by.entries()]
      .map(([peer, list]) => ({ peer, last: list[list.length - 1], unread: list.filter((m) => m.dir === "in" && !m.read).length }))
      .sort((a, b) => b.last.at - a.last.at);
  }

  thread(peer: string): Msg[] {
    return this.messages.filter((m) => m.peer === peer);
  }
}

export const app = new AppState();
