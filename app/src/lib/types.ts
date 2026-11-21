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
  /** Set when the station announced itself: how many relays its announcement passed through. */
  hops?: number;
  /** The announcing station runs a relay. */
  relay?: boolean;
  /** A relay that has its internet connection, so it can reach other areas. */
  online?: boolean;
  /** Where the station said it is, when it chose to say. */
  lat?: number | null;
  lon?: number | null;
}

export type MessageKind = "chat" | "private" | "urgent" | "signed";

/** A file or a voice note that travels with a message. */
export interface Attachment {
  kind: "file" | "voice";
  name: string;
  size: number;
  /** Where it is on this computer (sent from here, or saved to the inbox). */
  path?: string;
  /** Voice notes: length in seconds. */
  seconds?: number;
  /** Transfer progress in bytes, while it is on the air. */
  done?: number;
}

export interface MailItem {
  id: string;
  peer: string;
  dir: "in" | "out";
  subject: string;
  body: string;
  at: number;
  files: { name: string; size: number; path?: string }[];
  state?: OutState;
  note?: string;
  read?: boolean;
}

export interface RigModel {
  id: number;
  maker: string;
  model: string;
  status: string;
}

export interface PickedFile {
  path: string;
  name: string;
  size: number;
}

/** A recorded voice clip, ready to send. */
export interface VoiceClip {
  id: string;
  seconds: number;
  /** Size once compressed, in bytes. */
  bytes: number;
  /** Where the note is saved on this computer, so it can be played back. */
  path?: string;
}

export interface RelayHop {
  at: number;
  from: string;
  to: string;
  kind: string;
  /** The channel the message went out on, in kHz, when known. */
  channelKhz?: number;
}

/** One slot of the relay's accepted schedule, from kahf.dev. */
export interface RelaySlot {
  channel_khz: number;
  /** Unix seconds. */
  start_unix: number;
  duration_s: number;
  direction: "receive" | "transmit";
}

/** What the relay's link to kahf.dev last did. Null while it is not running. */
export interface ServerSync {
  registered: boolean;
  last_schedule_unix: number | null;
  slots: number;
  last_error: string | null;
  next_attempt_unix: number;
}

export interface RelayStatus {
  running: boolean;
  forwardedToday: number;
  forwardedTotal: number;
  /** Messages sent per hour, as [hour start in unix seconds, count], oldest first. */
  perHour?: [number, number][];
  recent: RelayHop[];
  /** The accepted schedule, in time order. */
  slots?: RelaySlot[];
  /** What is still waiting to go out. */
  outbox?: { id: number; what: string }[];
  server?: ServerSync | null;
  /** Why the link to kahf.dev is not running while relay mode is. */
  serverNote?: string | null;
  nowUnix?: number;
  /** The one-degree square the relay is listed in, if it is on the map. */
  area: string | null;
}

/** A station from the address book the engine keeps. */
export interface Contact {
  call: string;
  name: string;
  qth: string;
  notes: string;
  /** YYYY-MM-DD, or empty when never heard. */
  last_heard: string;
}

/** One logged contact. */
export interface Qso {
  call: string;
  date: string;
  time: string;
  band: string;
  mode: string;
  freq_mhz: number | null;
  rst_sent: string;
  rst_rcvd: string;
  name: string;
  comment: string;
}

export interface Logbook {
  stations: Contact[];
  qsos: Qso[];
  /** Why a file could not be read, when one could not. */
  error: string | null;
}

export interface UpdateCheck {
  outcome: "up_to_date" | "newer";
  version: string;
  url: string;
}

/** How kahf.dev answered a plain request for its public station list. */
export interface ServerCheck {
  reachable: boolean;
  millis: number;
  stationsListed: number;
  error: string | null;
}

export interface AppInfo {
  version: string;
  dataDir: string;
  logDir: string;
}

export interface RelayInfo {
  call: string;
  /** Centre of its area, when known. */
  lat: number | null;
  lon: number | null;
  /** Kilometres from this station, when both positions are known. */
  km: number | null;
  source: "server" | "heard" | "manual";
  lastSeen: number | null;
}
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
  /** A file or voice note, with or without text. */
  attachment?: Attachment;
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

export interface ExportServices {
  qrzKey: string;
  clublogEmail: string;
  clublogPassword: string;
  clublogAppKey: string;
  tqslPath: string;
  lotwLocation: string;
  n1mmAddr: string;
}

export interface DirectRig {
  id: string;
  label: string;
  baud: number;
}

export interface LevelCheck {
  verdict: "silent" | "low" | "good" | "high";
  advice: string;
  levelDb: number;
}

export interface Settings {
  callsign: string;
  audioIn: string;
  audioOut: string;
  sdr: string;
  rigctld: string;
  /** A radio controlled directly by name, such as `ic7300:/dev/cu.usbserial-1:115200:cat`. */
  rigSpec: string;
  /** Fields for sending the logbook to other services. Kept on this computer only. */
  exportServices: ExportServices;
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
  /** Relays to try, in order, when a station cannot be reached directly. */
  relayHints: string[];
  /** A radio chosen by name, for frequency control through Hamlib. */
  rig: { model: number | null; name: string; port: string; baud: number };
  /** Where voice notes are recorded: a sound device name, empty for the computer's microphone. */
  micIn: string;
  /** Speakers for playing voice notes. Empty: the system's default output. */
  speakerOut: string;
  /** Tuned frequency in kHz. */
  freqKhz: number;
  /** Transmit audio level, percent of full scale. Lower is safer for everyone on the channel. */
  txLevel: number;
  setupDone: boolean;
  tourDone: boolean;
  theme: "dark" | "light";
}

export interface Occupancy {
  at: number;
  /** [lane number 0 to 11, dB over the lane's noise floor] */
  lanes: [number, number][];
}

export interface TxGate {
  mode: "real" | "bench" | "off";
  text: string;
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
  | { type: "file"; name: string; done: number; total: number; dir: "in" | "out"; id: string; from?: string; path?: string }
  | { type: "mail"; from: string; subject: string; body: string; at: number; files: { name: string; size: number; path?: string }[] }
  | { type: "level"; level: number }
  | { type: "relay"; status: RelayStatus }
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
  /** Put the station on the public map as far as the person allowed. Returns a sentence to show. */
  shareOnMap(): Promise<string>;
  sendMail(id: string, to: string, subject: string, body: string, paths: string[]): Promise<void>;
  sendFile(id: string, to: string, path: string): Promise<void>;
  pickFiles(): Promise<PickedFile[]>;
  openPath(path: string): Promise<void>;
  recordVoice(): Promise<void>;
  /** Stops recording and returns the clip, or null when nothing was recorded. */
  stopVoice(): Promise<VoiceClip | null>;
  cancelVoice(): Promise<void>;
  playVoice(path: string): Promise<void>;
  stopPlayback(): Promise<void>;
  sendVoice(id: string, to: string, clip: VoiceClip): Promise<void>;
  relayStatus(): Promise<RelayStatus>;
  /** Relays this station knows, nearest first. `refresh` asks the server (needs the internet). */
  relayDirectory(refresh: boolean): Promise<RelayInfo[]>;
  setRelayHints(calls: string[]): Promise<void>;
  /** Tells everyone in range, and through relays everyone beyond, that this station exists. */
  advertise(): Promise<void>;
  setTxLevel(pct: number): Promise<void>;
  /** Sends a steady tone for five seconds so the radio's drive can be set by its ALC meter. */
  testTone(): Promise<void>;
  rigAvailable(): Promise<boolean>;
  rigModels(): Promise<RigModel[]>;
  rigPorts(): Promise<string[]>;
  /** Starts radio control and returns the radio's frequency in Hz. */
  rigConnect(model: number, port: string, baud: number): Promise<number>;
  rigStop(): Promise<void>;
  /** Radios that can be controlled directly, without Hamlib. */
  rigDirectModels(): Promise<DirectRig[]>;
  /** Opens a radio by its spec and reads its frequency. Never transmits. */
  testRigSpec(spec: string): Promise<string>;
  /** Listens to an input for three seconds and judges the level. */
  audioLevelCheck(device: string): Promise<LevelCheck>;
  /** Sends the logbook to another service. Only when the person asks. */
  uploadLogbook(service: "qrz" | "clublog" | "lotw" | "n1mm", cfg: ExportServices): Promise<string>;
  /** The address book and the logbook, read from the data folder. */
  logbook(): Promise<Logbook>;
  /** Writes the logbook as ADIF next to it and returns where. */
  exportAdif(): Promise<string>;
  /** Asks a rigctld at host:port for the radio's frequency. Never transmits. */
  testRig(addr: string): Promise<string>;
  /** Listens to a sound input briefly. Never transmits. */
  testAudioIn(device: string): Promise<string>;
  /** Opens a sound output and plays silence. Never transmits. */
  testAudioOut(device: string): Promise<string>;
  /** Asks GitHub for the newest release. Only when the person asks. */
  checkUpdate(): Promise<UpdateCheck>;
  /** Asks kahf.dev for its public station list, to show whether it can be reached. */
  serverStatus(): Promise<ServerCheck>;
  openLogs(): Promise<void>;
  openDataDir(): Promise<void>;
  /** Opens a KAHF page in the browser. */
  openUrl(url: string): Promise<void>;
  appInfo(): Promise<AppInfo>;
  listAudioDevices(): Promise<string[]>;
  detectSdr(): Promise<string[]>;
  modes(): Promise<ModeInfo[]>;
  /** Whether this station may transmit: real, bench (test table, no antenna) or off, with the reason. */
  txGate(): Promise<TxGate>;
  /** Lane activity over the last minute, or null before the first report. */
  occupancy(): Promise<Occupancy | null>;
}
