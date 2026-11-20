import { MockEngine } from "./mock";
import { TauriEngine } from "./tauri";
import type { Engine } from "./types";

/** Inside the desktop app the real engine; in a plain browser the demo engine. */
export function makeEngine(): Engine {
  const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
  return inTauri ? new TauriEngine() : new MockEngine();
}
