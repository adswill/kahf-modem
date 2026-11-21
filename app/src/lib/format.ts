/** Words and numbers for people, not for engineers. */

export function snrQuality(snr: number | null): { label: string; tone: "good" | "warn" | "bad" | "idle"; level: number } {
  if (snr === null) return { label: "Unknown", tone: "idle", level: 0 };
  if (snr >= 10) return { label: "Excellent", tone: "good", level: 4 };
  if (snr >= 3) return { label: "Good", tone: "good", level: 3 };
  if (snr >= -5) return { label: "Fair", tone: "warn", level: 2 };
  if (snr >= -12) return { label: "Weak", tone: "warn", level: 1 };
  return { label: "Very weak", tone: "bad", level: 1 };
}

/** Typing speed in plain terms, from net bits per second (8 bits per character). */
export function speedWords(netBps: number): string {
  const cps = netBps / 8;
  if (cps < 1) return "under 1 letter a second";
  if (cps < 20) return `about ${Math.round(cps)} letters a second`;
  if (cps < 200) return `about ${Math.round(cps)} letters a second, like fast typing`;
  return `about ${Math.round(cps)} letters a second, a page in a few seconds`;
}

export function timeAgo(ms: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 10) return "just now";
  if (s < 60) return `${s} seconds ago`;
  const m = Math.round(s / 60);
  if (m < 60) return `${m} minute${m === 1 ? "" : "s"} ago`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h} hour${h === 1 ? "" : "s"} ago`;
  const d = Math.round(h / 24);
  return `${d} day${d === 1 ? "" : "s"} ago`;
}

export function clock(ms: number): string {
  return new Date(ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

export function mhz(khz: number): string {
  // 17 m dials have up to two decimals of kHz (10 Hz steps): show them all.
  return `${(khz / 1000).toFixed(5)} MHz`;
}

/** What a mode name means in a sentence. */
export function modeExplained(name: string): string {
  const n = name.replace("KAMI-", "");
  if (n === "256" || n === "128" || n === "64") return "Very slow but reaches the weakest signals.";
  if (n === "32" || n === "16") return "Slow and sure. A good starting point.";
  if (n === "8" || n === "4") return "Medium speed for decent signals.";
  if (n === "1a") return "Fast. Needs a clear signal.";
  if (n === "1b") return "Faster. Needs a good signal.";
  if (n === "1c") return "Very fast. Needs a strong signal.";
  if (n === "1d") return "Fastest. Needs an excellent signal.";
  return "";
}

/** The most one transfer may hold. Keep in step with `MAX_ATTACHMENT_BYTES` in the engine. */
export const FILE_LIMIT_BYTES = 100 * 1024;

/** Seconds a transfer of `bytes` takes at `netBps` of useful speed, with room for the
 * acknowledgements and the gaps while the other station answers. */
export function transferSeconds(bytes: number, netBps: number): number {
  return (bytes * 8 * 1.3) / Math.max(20, netBps);
}

/** "about 3 minutes", "about 1 hour 20 minutes". */
export function durationWords(seconds: number): string {
  if (seconds < 90) return `about ${Math.max(5, Math.round(seconds / 5) * 5)} seconds`;
  const min = Math.round(seconds / 60);
  if (min < 60) return `about ${min} minute${min === 1 ? "" : "s"}`;
  const h = Math.floor(min / 60);
  const m = min % 60;
  return `about ${h} hour${h === 1 ? "" : "s"}${m >= 5 ? ` ${m} minutes` : ""}`;
}

/** 18113.5 kHz as "18 113.5 kHz", with a thin space between thousands. */
export function channelKhz(khz: number): string {
  const whole = Math.trunc(khz);
  const tenth = Math.min(9, Math.round((khz - whole) * 10));
  return whole >= 1000
    ? `${Math.floor(whole / 1000)} ${String(whole % 1000).padStart(3, "0")}.${tenth} kHz`
    : `${whole}.${tenth} kHz`;
}

/** Unix seconds as "HH:MM" UTC. */
export function utcHm(unix: number): string {
  return new Date(unix * 1000).toISOString().slice(11, 16);
}

/** Seconds as "45 s", "15 min" or "1 h 30 min". */
export function duration(secs: number): string {
  if (secs < 60) return `${secs} s`;
  if (secs < 3600) return `${Math.round(secs / 60)} min`;
  const h = Math.floor(secs / 3600);
  const m = Math.round((secs % 3600) / 60);
  return m === 0 ? `${h} h` : `${h} h ${m} min`;
}

/** "in 4 min" or "6 min ago", from a unix time and now (both in seconds). */
export function relative(unix: number, now: number): string {
  const d = unix - now;
  const a = Math.abs(d);
  const span = duration(a);
  return d >= 0 ? `in ${span}` : `${span} ago`;
}
