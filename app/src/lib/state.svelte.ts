// The one place that holds what the window shows. Views read from here and call its methods;
// they never talk to the engine directly.

import { callProblem, normaliseCall, validCall } from "./callsign";
import { defaultSettings } from "./defaults";
import { FILE_LIMIT_BYTES } from "./format";
import { makeEngine } from "./engine";
import type {
  Attachment, Engine, EngineEvent, Heard, LinkState, MailItem, MessageKind, Metrics, ModeInfo, Msg, PickedFile,
  RelayInfo, RelayStatus, Settings, VoiceClip,
} from "./types";

export type Page = "welcome" | "setup" | "chats" | "mail" | "stations" | "radio" | "relay";
export type SettingsTab = "station" | "radio" | "location" | "sending" | "relay" | "look" | "about";

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
}

const MSG_STORE = "kahf.messages";
const MAIL_STORE = "kahf.mail";
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
    return "KAHF could not reach the radio control program. Check the radio on the Radio page, or leave radio control off.";
  }
  return `The radio could not start. ${raw}`;
}

/** Bytes in plain words. */
export function sizeWords(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(n < 10240 ? 1 : 0)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

class AppState {
  engine: Engine = makeEngine();
  ready = $state(false);
  page = $state<Page>("welcome");
  settings = $state<Settings>(defaultSettings());
  link = $state<LinkState>({ state: "idle" });
  heard = $state<Heard[]>([]);
  messages = $state<Msg[]>([]);
  mail = $state<MailItem[]>([]);
  metrics = $state<Metrics | null>(null);
  modes = $state<ModeInfo[]>([]);
  log = $state<{ at: number; text: string }[]>([]);
  spectrum = $state<number[]>([]);
  transmitting = $state(false);
  receiving = $state(false);
  toasts = $state<Toast[]>([]);
  peer = $state<string>("");
  tourOpen = $state(false);
  settingsOpen = $state(false);
  settingsTab = $state<SettingsTab>("station");
  helpOpen = $state(false);
  helpTopic = $state<string>("");
  /** Why the radio could not start, in plain words. Empty when all is well. */
  engineError = $state("");
  recording = $state(false);
  recordLevel = $state(0);
  relay = $state<RelayStatus>({ running: false, forwardedToday: 0, forwardedTotal: 0, recent: [], area: null });
  relays = $state<RelayInfo[]>([]);
  private toastId = 0;
  private lastSignal = 0;
  /** How strong the signal in the KAHF channel is, 0 to 1, smoothed. Drives the meter. */
  signal = $state(0);

  // ---- derived ---------------------------------------------------------------------------

  get hasCall(): boolean {
    return validCall(this.settings.callsign);
  }

  get unreadChats(): number {
    return this.messages.filter((m) => m.dir === "in" && !m.read).length;
  }

  get unreadMail(): number {
    return this.mail.filter((m) => m.dir === "in" && !m.read).length;
  }

  get steps(): Step[] {
    const s = this.settings;
    return [
      { id: "call", title: "Your callsign is set", why: "It is your name on the air.", done: this.hasCall },
      {
        id: "radio",
        title: "KAHF can hear your radio",
        why: "A sound card or receiver is chosen.",
        done: s.setupDone && (s.audioIn !== "" || s.sdr !== "" || this.engine.demo),
      },
      { id: "listen", title: "You have heard a station", why: "Leave KAHF running. Stations appear on their own.", done: this.heard.length > 0 },
      {
        id: "send",
        title: "You have sent a message",
        why: "Pick a station, write a line, press Send.",
        done: this.messages.some((m) => m.dir === "out" && m.state === "sent"),
      },
    ];
  }

  /** What the radio is doing, in one short line: shown in the strip on every screen. */
  get status(): { tone: "idle" | "good" | "busy" | "warn"; title: string; detail: string } {
    if (!this.hasCall && !this.settings.listenOnly) {
      return { tone: "warn", title: "Not set up", detail: "Enter your callsign to start." };
    }
    if (this.engineError) return { tone: "warn", title: "Radio not running", detail: this.engineError };
    if (this.settings.listenOnly) {
      return { tone: "idle", title: "Listening only", detail: "KAHF will not transmit." };
    }
    if (this.transmitting) return { tone: "busy", title: "Sending", detail: "Your signal is on the air." };
    if (this.link.state === "calling") {
      return {
        tone: "busy",
        title: `Reaching ${this.link.peer}`,
        detail: this.link.of > 0 ? `Try ${this.link.attempt} of ${this.link.of}` : "KAHF keeps trying by itself",
      };
    }
    if (this.link.state === "linked") return { tone: "good", title: `Linked to ${this.link.peer}`, detail: "Messages go straight to them." };
    if (this.receiving) return { tone: "good", title: "Receiving", detail: "A signal is on the channel." };
    const n = this.heard.length;
    return { tone: "idle", title: "Listening", detail: n === 0 ? "Nobody heard yet." : `${n} heard` };
  }

  // ---- startup ---------------------------------------------------------------------------

  async init() {
    this.settings = await this.engine.loadSettings();
    this.messages = load<Msg[]>(MSG_STORE, []);
    this.mail = load<MailItem[]>(MAIL_STORE, []);
    this.heard = load<Heard[]>(HEARD_STORE, []);
    this.applyTheme();
    this.modes = await this.engine.modes();
    this.engine.onEvent((e) => this.handle(e));
    this.page = this.settings.setupDone && (this.hasCall || this.settings.listenOnly) ? "chats" : "welcome";
    if (this.settings.setupDone) {
      await this.startEngine();
      this.engine.setRelayHints(this.settings.relayHints).catch(() => {});
      this.refreshRelayDirectory(false);
    }
    this.ready = true;
  }

  applyTheme() {
    document.documentElement.dataset.theme = this.settings.theme;
  }

  go(page: Page) {
    this.page = page;
    if (page === "chats") this.markChatsRead();
    if (page === "mail") this.markMailRead();
  }

  openSettings(tab: SettingsTab = "station") {
    this.settingsTab = tab;
    this.settingsOpen = true;
  }

  openHelp(topic = "") {
    if (topic) this.helpTopic = topic;
    this.helpOpen = true;
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
    this.page = "chats";
    if (ok) this.toast("All set. KAHF is listening for stations.", "good");
    this.refreshRelayDirectory(false);
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

  // ---- text, files and voice ---------------------------------------------------------------

  private newId() {
    return `${Date.now()}-${Math.floor(Math.random() * 1e6)}`;
  }

  private canSend(): boolean {
    if (this.settings.listenOnly) {
      this.toast("Listen-only is on, so nothing can be sent. Turn it off in Settings, under Sending.", "warn");
      return false;
    }
    return true;
  }

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
    if (peer === "" || body === "" || !this.canSend()) return;
    if (callProblem(peer)) {
      this.toast(`"${peer}" does not look like a callsign.`, "warn");
      return;
    }
    const id = this.newId();
    this.addOut({ id, peer, dir: "out", text: body, at: Date.now(), kind, state: "queued", read: true });
    try {
      await this.engine.send(id, peer, body, kind);
    } catch (e) {
      this.update(id, { state: "failed", note: String(e) });
    }
  }

  /** Files waiting for the person to confirm, after the warning about time and hops. */
  pendingFiles = $state<{ to: string; files: PickedFile[] } | null>(null);

  /** The speed to base estimates on: the live link when there is one, else the starting speed. */
  get estimateBps(): number {
    return this.metrics?.netBps ?? 284;
  }

  /** Checks the limit, then asks the person to confirm. */
  askSendFiles(to: string, files: PickedFile[]) {
    const ok = files.filter((f) => f.size <= FILE_LIMIT_BYTES);
    if (ok.length < files.length) {
      this.toast(`Files over ${sizeWords(FILE_LIMIT_BYTES)} cannot be sent: radio is too slow for them. Zip or shrink the file, or send it another way.`, "warn");
    }
    if (ok.length) this.pendingFiles = { to, files: ok };
  }

  async confirmFiles() {
    const p = this.pendingFiles;
    this.pendingFiles = null;
    if (p) for (const f of p.files) await this.sendFile(p.to, f);
  }

  async sendFile(to: string, file: PickedFile) {
    const peer = normaliseCall(to);
    if (!peer || callProblem(peer) || !this.canSend()) return;
    const id = this.newId();
    const attachment: Attachment = { kind: "file", name: file.name, size: file.size, path: file.path, done: 0 };
    this.addOut({ id, peer, dir: "out", text: "", at: Date.now(), kind: "chat", state: "queued", read: true, attachment });
    try {
      await this.engine.sendFile(id, peer, file.path);
    } catch (e) {
      this.update(id, { state: "failed", note: String(e) });
    }
  }

  async sendVoice(to: string, clip: VoiceClip) {
    const peer = normaliseCall(to);
    if (!peer || callProblem(peer) || !this.canSend()) return;
    const id = this.newId();
    const attachment: Attachment = { kind: "voice", name: "Voice note", size: clip.bytes, seconds: clip.seconds, done: 0 };
    this.addOut({ id, peer, dir: "out", text: "", at: Date.now(), kind: "chat", state: "queued", read: true, attachment });
    try {
      await this.engine.sendVoice(id, peer, clip);
    } catch (e) {
      this.update(id, { state: "failed", note: String(e) });
    }
  }

  async sendMail(to: string, subject: string, body: string, files: PickedFile[]): Promise<boolean> {
    const peer = normaliseCall(to);
    if (!peer || callProblem(peer)) {
      this.toast("Enter the callsign of the station you are writing to.", "warn");
      return false;
    }
    if (!this.canSend()) return false;
    const id = this.newId();
    const item: MailItem = {
      id, peer, dir: "out", subject: subject.trim() || "(no subject)", body, at: Date.now(),
      files: files.map((f) => ({ name: f.name, size: f.size, path: f.path })), state: "queued", read: true,
    };
    this.mail = [...this.mail, item];
    save(MAIL_STORE, this.mail.slice(-300));
    try {
      await this.engine.sendMail(id, peer, item.subject, body, files.map((f) => f.path));
    } catch (e) {
      this.updateMail(id, { state: "failed", note: String(e) });
    }
    return true;
  }

  async startRecording() {
    if (this.recording) return;
    try {
      await this.engine.recordVoice();
      this.recording = true;
    } catch (e) {
      this.toast(`Could not record: ${e}. Check the microphone in Settings, under Radio.`, "warn");
    }
  }

  async stopRecording(): Promise<VoiceClip | null> {
    this.recording = false;
    this.recordLevel = 0;
    return this.engine.stopVoice();
  }

  async cancelRecording() {
    this.recording = false;
    this.recordLevel = 0;
    await this.engine.cancelVoice();
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

  async setRelay(on: boolean, licence: boolean, terms: boolean) {
    await this.engine.setRelay(on, licence, terms);
    await this.saveSettings({ relay: on });
    this.relay = await this.engine.relayStatus();
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

  async refreshRelayDirectory(refresh: boolean) {
    try {
      this.relays = await this.engine.relayDirectory(refresh);
      if (refresh) this.toast(`${this.relays.length} relay${this.relays.length === 1 ? "" : "s"} known.`, "info");
    } catch (e) {
      if (refresh) this.toast(`Could not reach the server: ${e}. KAHF still works without it.`, "warn");
    }
  }

  /** Announce this station on the air; relays pass it on. */
  async advertise() {
    try {
      await this.engine.advertise();
      this.toast("Announcement sent. Relays pass it on, and every station that hears it adds you to its contacts.", "info");
    } catch (e) {
      this.toast(String(e), "warn");
    }
  }

  async setTxLevel(pct: number) {
    await this.saveSettings({ txLevel: pct });
    await this.engine.setTxLevel(pct).catch(() => {});
  }

  async testTone() {
    try {
      await this.engine.testTone();
      this.toast("Sending a test tone for 5 seconds. Watch the ALC meter on your radio.", "info");
    } catch (e) {
      this.toast(String(e), "warn");
    }
  }

  async setRelayHints(calls: string[]) {
    await this.saveSettings({ relayHints: calls });
    await this.engine.setRelayHints(calls);
  }

  discard(id: string) {
    this.messages = this.messages.filter((m) => m.id !== id);
    this.persistMessages();
  }

  markChatsRead() {
    if (this.messages.some((m) => m.dir === "in" && !m.read)) {
      this.messages = this.messages.map((m) => (m.dir === "in" ? { ...m, read: true } : m));
      this.persistMessages();
    }
  }

  markMailRead() {
    if (this.mail.some((m) => m.dir === "in" && !m.read)) {
      this.mail = this.mail.map((m) => (m.dir === "in" ? { ...m, read: true } : m));
      save(MAIL_STORE, this.mail);
    }
  }

  toast(text: string, kind: Toast["kind"] = "info") {
    const id = ++this.toastId;
    this.toasts = [...this.toasts, { id, text, kind }];
    window.setTimeout(() => (this.toasts = this.toasts.filter((t) => t.id !== id)), 5200);
  }

  // ---- events from the engine ------------------------------------------------------------

  private addOut(m: Msg) {
    this.messages = [...this.messages, m];
    this.persistMessages();
  }

  private update(id: string, patch: Partial<Msg>) {
    this.messages = this.messages.map((m) => (m.id === id ? { ...m, ...patch } : m));
    this.persistMessages();
  }

  private updateMail(id: string, patch: Partial<MailItem>) {
    this.mail = this.mail.map((m) => (m.id === id ? { ...m, ...patch } : m));
    save(MAIL_STORE, this.mail);
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
          { id: `in-${e.at}-${this.messages.length}`, peer: e.from, dir: "in", text: e.text, at: e.at, kind: "chat", read: this.page === "chats" },
        ];
        this.persistMessages();
        if (this.page !== "chats") this.toast(`New message from ${e.from}`, "info");
        break;
      }
      case "mail": {
        this.mail = [
          ...this.mail,
          { id: `in-${e.at}-${this.mail.length}`, peer: e.from, dir: "in", subject: e.subject, body: e.body, at: e.at, files: e.files, read: this.page === "mail" },
        ];
        save(MAIL_STORE, this.mail.slice(-300));
        if (this.page !== "mail") this.toast(`New mail from ${e.from}: ${e.subject}`, "info");
        break;
      }
      case "file": {
        if (e.dir === "out") {
          this.update(e.id, { attachment: { ...(this.messages.find((m) => m.id === e.id)?.attachment as Attachment), done: e.done } });
          break;
        }
        // A file or voice note that arrived: shown in the conversation with the sender.
        if (e.done >= e.total && e.from) {
          const voice = /\.kvm$/i.test(e.name);
          this.messages = [
            ...this.messages,
            {
              id: `in-file-${Date.now()}-${this.messages.length}`, peer: e.from, dir: "in", text: "", at: Date.now(), kind: "chat",
              read: this.page === "chats",
              attachment: { kind: voice ? "voice" : "file", name: voice ? "Voice note" : e.name, size: e.total, path: e.path, done: e.total },
            },
          ];
          this.persistMessages();
          if (this.page !== "chats") this.toast(`${voice ? "Voice note" : "File"} from ${e.from}`, "info");
        }
        break;
      }
      case "heard": {
        const old = this.heard.find((h) => h.call === e.heard.call);
        const rest = this.heard.filter((h) => h.call !== e.heard.call);
        // A plain "heard" must not wipe what an announcement told us about the station.
        const merged = { ...e.heard, lat: e.heard.lat ?? old?.lat, lon: e.heard.lon ?? old?.lon, relay: e.heard.relay ?? old?.relay, online: e.heard.online ?? old?.online };
        this.heard = [merged, ...rest].slice(0, 80);
        save(HEARD_STORE, this.heard);
        break;
      }
      case "metrics":
        this.metrics = e.metrics;
        break;
      case "spectrum": {
        this.spectrum = e.bins;
        // A signal inside the KAHF channel (0.3 to 2.7 kHz) well above the floor.
        const n = e.bins.length;
        if (n) {
          const lo = Math.floor((300 / 4000) * n);
          const hi = Math.floor((2700 / 4000) * n);
          const band = e.bins.slice(lo, hi);
          const mean = band.reduce((a, b) => a + b, 0) / Math.max(1, band.length);
          const floor = [...e.bins].sort((a, b) => a - b)[Math.floor(n * 0.2)];
          const above = mean - floor;
          const loud = above > 12;
          if (loud) this.lastSignal = Date.now();
          this.receiving = Date.now() - this.lastSignal < 1500;
          const target = Math.max(0, Math.min(1, above / 40));
          this.signal = this.signal * 0.7 + target * 0.3;
        }
        break;
      }
      case "level":
        this.recordLevel = e.level;
        break;
      case "relay":
        this.relay = e.status;
        break;
      case "tx":
        this.transmitting = e.state === "starting" || e.state === "sending";
        if (e.state === "sending") {
          for (const m of this.messages) if (m.dir === "out" && (m.state === "queued" || m.state === "calling")) this.update(m.id, { state: "sending" });
          for (const m of this.mail) if (m.dir === "out" && (m.state === "queued" || m.state === "calling")) this.updateMail(m.id, { state: "sending" });
        }
        break;
      case "sent":
        if (this.mail.some((m) => m.id === e.id)) this.updateMail(e.id, { state: "sent", note: undefined });
        else this.update(e.id, { state: "sent", note: undefined });
        break;
      case "failed":
        if (this.mail.some((m) => m.id === e.id)) this.updateMail(e.id, { state: "failed", note: e.reason });
        else this.update(e.id, { state: "failed", note: e.reason });
        this.toast(e.reason, "warn");
        break;
      case "log":
        this.log = [{ at: Date.now(), text: e.text }, ...this.log].slice(0, 100);
        break;
    }
  }

  // ---- views over the data ---------------------------------------------------------------

  /** The people you have written to or heard from in conversations, most recent first. */
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

  /** Heard stations you have not talked to yet. */
  get onTheAir(): Heard[] {
    const talked = new Set(this.conversations.map((c) => c.peer));
    return this.heard.filter((h) => !talked.has(h.call));
  }

  get inboxFiles(): { name: string; size: number; path?: string; from: string; at: number }[] {
    return this.messages
      .filter((m) => m.dir === "in" && m.attachment?.kind === "file")
      .map((m) => ({ name: m.attachment!.name, size: m.attachment!.size, path: m.attachment!.path, from: m.peer, at: m.at }));
  }
}

export const app = new AppState();
