// The shapes shared by the window and the engine (real or demo).

export type LinkState =
  | { state: "idle" }
  | { state: "calling"; peer: string; attempt: number; of: number }
  | { state: "linked"; peer: string };

export interface Metrics {
  /** KAMI mode of the last burst we sent, for example "KAMI-32". */
  mode: string;
  /** True when the mode is chosen automatically. */
  auto: boolean;
  /** Net information speed of that mode in a full burst, bit/s. */
  netBps: number;
  /** SNR (dB) at which we hear the other station. */
  ownSnr: number | null;
  /** SNR (dB) at which the other station hears us. */
  peerSnr: number | null;
  /** Share of recent frames lost, percent. */
  ferPct: number;
  sent: number;
  received: number;
  retransmits: number;
}

export interface Heard {
  call: string;
  snr: number | null;
  /** Unix milliseconds. */
  at: number;
}

export type MessageKind = "chat" | "private" | "urgent" | "signed";
export type OutState = "queued" | "calling" | "sending" | "sent" | "failed";

export interface Msg {
  id: string;
  /** The other station. */
  peer: string;
  dir: "in" | "out";
  text: string;
  at: number;
  kind: MessageKind;
  /** Only for outgoing messages. */
  state?: OutState;
  /** Incoming: seen by the person. */
  read?: boolean;
  /** Plain-language reason when a message failed or is waiting. */
  note?: string;
}

export type LocationMode = "off" | "area" | "precise";

export interface Location {
  mode: LocationMode;
  lat: number | null;
  lon: number | null;
  /** Radius of the circle shown for "area", in km. */
  radiusKm: number;
}

export interface Settings {
  callsign: string;
  audioIn: string;
  audioOut: string;
  sdr: string;
  rigctld: string;
  location: Location;
  /** Connect by itself to send queued messages, and hang up afterwards. */
  autoLink: boolean;
  /** Receive only: never transmit. */
  listenOnly: boolean;
  /** Choose the KAMI mode automatically from the other station's SNR report. */
  autoMode: boolean;
  fixedMode: string;
  /** Acts as a relay for other stations. */
  relay: boolean;
  /** Tuned frequency in kHz. */
  freqKhz: number;
  setupDone: boolean;
  tourDone: boolean;
  theme: "dark" | "light";
}

export interface ModeInfo {
  name: string;
  /** SNR (dB) needed to lose under 5 percent of frames. */
  needSnr: number;
  /** Raw bits per second on the air. */
  rawBps: number;
  /** Net information bits per second in a full burst. */
  netBps: number;
}

export type EngineEvent =
  | { type: "link"; link: LinkState }
  | { type: "message"; from: string; text: string; at: number }
  | { type: "heard"; heard: Heard }
  | { type: "metrics"; metrics: Metrics }
  | { type: "spectrum"; bins: number[] }
  | { type: "tx"; state: "starting" | "sending" | "done" | "failed"; reason?: string }
  | { type: "sent"; id: string }
  | { type: "failed"; id: string; reason: string }
  | { type: "log"; text: string };

export interface Engine {
  readonly demo: boolean;
  loadSettings(): Promise<Settings>;
  saveSettings(s: Settings): Promise<void>;
  start(s: Settings): Promise<void>;
  onEvent(cb: (e: EngineEvent) => void): () => void;
  connect(call: string): Promise<void>;
  disconnect(): Promise<void>;
  send(id: string, to: string, text: string, kind: MessageKind): Promise<void>;
  setAutoMode(on: boolean): Promise<void>;
  setFixedMode(name: string): Promise<void>;
  setListenOnly(on: boolean): Promise<void>;
  setAutoLink(on: boolean): Promise<void>;
  /** Pass messages on for other stations (the station must stay on and attended). */
  setRelay(on: boolean, licenceHeld: boolean, termsAccepted: boolean): Promise<void>;
  listAudioDevices(): Promise<string[]>;
  detectSdr(): Promise<string[]>;
  modes(): Promise<ModeInfo[]>;
}
