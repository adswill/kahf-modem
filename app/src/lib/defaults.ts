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

/** The modes the engine can use, slowest and most robust first. Lane mode (17 m): the speeds of docs/KAMI-MODES.md divided by five (a lane burst is exactly five times longer); the needed SNR is the one the engine measures inside the lane. */
export const MODE_TABLE = [
  { name: "KAMI-256", needSnr: -16.0, rawBps: 25, netBps: 8 },
  { name: "KAMI-128", needSnr: -13.2, rawBps: 44, netBps: 16 },
  { name: "KAMI-64", needSnr: -10.5, rawBps: 75, netBps: 32 },
  { name: "KAMI-32", needSnr: -8.1, rawBps: 125, netBps: 57 },
  { name: "KAMI-16", needSnr: -5.3, rawBps: 200, netBps: 92 },
  { name: "KAMI-8", needSnr: -2.3, rawBps: 300, netBps: 137 },
  { name: "KAMI-4", needSnr: -1.8, rawBps: 400, netBps: 179 },
  { name: "KAMI-1a", needSnr: -0.2, rawBps: 800, netBps: 353 },
  { name: "KAMI-1b", needSnr: 4.8, rawBps: 1600, netBps: 688 },
  { name: "KAMI-1c", needSnr: 8.9, rawBps: 2400, netBps: 986 },
  { name: "KAMI-1d", needSnr: 13.0, rawBps: 3200, netBps: 1207 },
];
