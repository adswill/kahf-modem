import type { Settings } from "./types";

export const defaultSettings = (): Settings => ({
  callsign: "",
  audioIn: "",
  audioOut: "",
  sdr: "",
  rigctld: "",
  // Privacy first: nothing is shared until the person chooses to.
  location: { mode: "off", lat: null, lon: null, radiusKm: 50 },
  autoLink: true,
  listenOnly: false,
  autoMode: true,
  fixedMode: "KAMI-32",
  relay: false,
  relayHints: [],
  rig: { model: null, name: "", port: "", baud: 19200 },
  micIn: "",
  speakerOut: "",
  freqKhz: 18113.44, // 17 m, lane C1 (the first calling lane)
  txLevel: 50,
  setupDone: false,
  tourDone: false,
  theme: "dark",
});

/** The modes the engine can use, slowest and most robust first. Numbers from docs/KAMI-MODES.md. */
export const MODE_TABLE = [
  { name: "KAMI-256", needSnr: -16.0, rawBps: 125, netBps: 41 },
  { name: "KAMI-128", needSnr: -13.2, rawBps: 219, netBps: 80 },
  { name: "KAMI-64", needSnr: -10.5, rawBps: 375, netBps: 161 },
  { name: "KAMI-32", needSnr: -8.1, rawBps: 625, netBps: 284 },
  { name: "KAMI-16", needSnr: -5.3, rawBps: 1000, netBps: 462 },
  { name: "KAMI-8", needSnr: -2.3, rawBps: 1500, netBps: 687 },
  { name: "KAMI-4", needSnr: -1.8, rawBps: 2000, netBps: 895 },
  { name: "KAMI-1a", needSnr: -0.2, rawBps: 4000, netBps: 1764 },
  { name: "KAMI-1b", needSnr: 4.8, rawBps: 8000, netBps: 3439 },
  { name: "KAMI-1c", needSnr: 8.9, rawBps: 12000, netBps: 4931 },
  { name: "KAMI-1d", needSnr: 13.0, rawBps: 16000, netBps: 6033 },
];
