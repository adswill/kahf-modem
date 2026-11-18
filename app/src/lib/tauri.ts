// The real engine: the Rust program behind the window. Each call is a Tauri command, and the
// engine's events arrive on one channel.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Engine, EngineEvent, MessageKind, ModeInfo, Settings } from "./types";

export class TauriEngine implements Engine {
  readonly demo = false;
  private listeners = new Set<(e: EngineEvent) => void>();
  private started = false;

  loadSettings(): Promise<Settings> {
    return invoke("load_settings");
  }
  saveSettings(s: Settings): Promise<void> {
    return invoke("save_settings", { settings: s });
  }
  async start(s: Settings): Promise<void> {
    if (!this.started) {
      this.started = true;
      await listen<EngineEvent>("engine", (ev) => {
        for (const l of this.listeners) l(ev.payload);
      });
    }
    await invoke("engine_start", { settings: s });
  }
  onEvent(cb: (e: EngineEvent) => void): () => void {
    this.listeners.add(cb);
    return () => this.listeners.delete(cb);
  }
  connect(call: string) {
    return invoke<void>("engine_connect", { call });
  }
  disconnect() {
    return invoke<void>("engine_disconnect");
  }
  send(id: string, to: string, text: string, kind: MessageKind) {
    return invoke<void>("engine_send", { id, to, text, kind });
  }
  setAutoMode(on: boolean) {
    return invoke<void>("engine_set_auto_mode", { on });
  }
  setFixedMode(name: string) {
    return invoke<void>("engine_set_fixed_mode", { name });
  }
  setListenOnly(on: boolean) {
    return invoke<void>("engine_set_listen_only", { on });
  }
  setAutoLink(on: boolean) {
    return invoke<void>("engine_set_auto_link", { on });
  }
  setRelay(on: boolean, licenceHeld: boolean, termsAccepted: boolean) {
    return invoke<void>("engine_set_relay", { on, licenceHeld, termsAccepted });
  }
  listAudioDevices() {
    return invoke<string[]>("list_audio_devices");
  }
  detectSdr() {
    return invoke<string[]>("detect_sdr");
  }
  modes(): Promise<ModeInfo[]> {
    return invoke("engine_modes");
  }
}
