// The real engine: the Rust program behind the window. Each call is a Tauri command, and the
// engine's events arrive on one channel.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { defaultSettings } from "./defaults";
import type {
  AppInfo, Engine, Logbook, ServerCheck, UpdateCheck, EngineEvent, MessageKind, ModeInfo, PickedFile, RelayInfo, RelayStatus, RigModel, Settings, TxGate, Occupancy, VoiceClip,
} from "./types";

export class TauriEngine implements Engine {
  readonly demo = false;
  private listeners = new Set<(e: EngineEvent) => void>();
  private started = false;

  async loadSettings(): Promise<Settings> {
    const saved = await invoke<Partial<Settings>>("load_settings");
    const d = defaultSettings();
    return { ...d, ...saved, location: { ...d.location, ...(saved.location ?? {}) } };
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
  shareOnMap() {
    return invoke<string>("map_share");
  }
  sendMail(id: string, to: string, subject: string, body: string, paths: string[]) {
    return invoke<void>("engine_send_mail", { id, to, subject, body, paths });
  }
  sendFile(id: string, to: string, path: string) {
    return invoke<void>("engine_send_file", { id, to, path });
  }
  pickFiles() {
    return invoke<PickedFile[]>("pick_files");
  }
  openPath(path: string) {
    return invoke<void>("open_path", { path });
  }
  recordVoice() {
    return invoke<void>("voice_record");
  }
  stopVoice() {
    return invoke<VoiceClip | null>("voice_stop");
  }
  cancelVoice() {
    return invoke<void>("voice_cancel");
  }
  stopPlayback() {
    return invoke<void>("voice_stop_play");
  }
  playVoice(path: string) {
    return invoke<void>("voice_play", { path });
  }
  sendVoice(id: string, to: string, clip: VoiceClip) {
    return invoke<void>("voice_send", { id, to, clipId: clip.id });
  }
  relayStatus() {
    return invoke<RelayStatus>("relay_status");
  }
  relayDirectory(refresh: boolean) {
    return invoke<RelayInfo[]>("relay_directory", { refresh });
  }
  setTxLevel(pct: number) {
    return invoke<void>("engine_set_tx_level", { pct });
  }
  testTone() {
    return invoke<void>("engine_test_tone");
  }
  advertise() {
    return invoke<void>("engine_advertise");
  }
  setRelayHints(calls: string[]) {
    return invoke<void>("engine_set_relay_hints", { calls });
  }
  rigAvailable() {
    return invoke<boolean>("rig_available");
  }
  rigModels() {
    return invoke<RigModel[]>("rig_models");
  }
  rigPorts() {
    return invoke<string[]>("rig_ports");
  }
  rigConnect(model: number, port: string, baud: number) {
    return invoke<number>("rig_connect", { model, port, baud });
  }
  rigStop() {
    return invoke<void>("rig_stop");
  }
  logbook() {
    return invoke<Logbook>("logbook");
  }
  exportAdif() {
    return invoke<string>("export_adif");
  }
  testRig(addr: string) {
    return invoke<string>("test_rig", { addr });
  }
  testAudioIn(device: string) {
    return invoke<string>("test_audio_in", { device });
  }
  testAudioOut(device: string) {
    return invoke<string>("test_audio_out", { device });
  }
  checkUpdate() {
    return invoke<UpdateCheck>("check_update");
  }
  serverStatus() {
    return invoke<ServerCheck>("server_status");
  }
  openLogs() {
    return invoke<void>("open_logs");
  }
  openDataDir() {
    return invoke<void>("open_data_dir");
  }
  openUrl(url: string) {
    return invoke<void>("open_url", { url });
  }
  appInfo() {
    return invoke<AppInfo>("app_info");
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
  occupancy(): Promise<Occupancy | null> {
    return invoke("engine_occupancy");
  }
  txGate(): Promise<TxGate> {
    return invoke("engine_tx_gate");
  }
}
