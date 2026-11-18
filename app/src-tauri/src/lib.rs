//! The desktop app: a window (the web UI in `../ui`) on top of the KAHF engine.
//!
//! The window calls the commands below; the engine's events go back to it on the "engine"
//! channel as plain JSON. Everything the window shows comes from here or from its own settings.

mod convert;
mod directory;
mod report;
mod rig;
mod settings;
mod voice;

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use kahf_daemon::{Config, Engine};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};

use convert::{event_json, Pending};
use settings::EngineSettings;

/// What the app keeps while it runs.
struct Shared {
    engine: Mutex<Option<Engine>>,
    /// Messages the window sent that are still in the engine's outbox.
    pending: Mutex<Vec<Pending>>,
    data_dir: PathBuf,
    /// The `rigctld` KAHF started for the chosen radio, if any.
    rigctld: Mutex<Option<std::process::Child>>,
    voice: voice::Voice,
}

type Res<T> = Result<T, String>;

fn data_dir(app: &AppHandle) -> PathBuf {
    if let Ok(p) = std::env::var("KAHF_DATA_DIR") {
        return PathBuf::from(p);
    }
    app.path().app_data_dir().unwrap_or_else(|_| PathBuf::from("."))
}

#[tauri::command]
fn load_settings(state: State<Shared>) -> Value {
    settings::load(&state.data_dir)
}

#[tauri::command]
fn save_settings(state: State<Shared>, settings: Value) -> Res<()> {
    settings::save(&state.data_dir, &settings)
}

#[tauri::command]
fn list_audio_devices() -> Vec<String> {
    kahf_audio::list_devices()
}

/// The receivers KAHF can talk to, judged by which driver libraries are installed.
#[tauri::command]
fn detect_sdr() -> Vec<String> {
    let (hackrf, rtl) = kahf_sdr::libraries_present();
    let mut found = Vec::new();
    if hackrf {
        found.push("HackRF".to_string());
    }
    if rtl {
        found.push("RTL-SDR".to_string());
    }
    found
}

/// Is Hamlib installed, so radios can be chosen by name?
#[tauri::command]
fn rig_available() -> bool {
    rig::find_tool("rigctld").is_some() && rig::find_tool("rigctl").is_some()
}

#[tauri::command]
fn rig_models() -> Res<Vec<rig::RigModel>> {
    rig::list_rigs()
}

#[tauri::command]
fn rig_ports() -> Vec<String> {
    rig::serial_ports()
}

fn stop_rig(state: &State<Shared>) {
    if let Ok(mut g) = state.rigctld.lock() {
        if let Some(mut c) = g.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// Starts radio control for the chosen radio and reads its frequency as a test.
/// Returns the frequency in Hz.
#[tauri::command]
fn rig_connect(state: State<Shared>, model: u32, port: String, baud: u32) -> Res<u64> {
    stop_rig(&state);
    let child = rig::start_rigctld(model, &port, baud)?;
    *state.rigctld.lock().map_err(|e| e.to_string())? = Some(child);
    match rig::read_frequency(rig::local_addr()) {
        Ok(hz) => Ok(hz),
        Err(e) => {
            stop_rig(&state);
            Err(e)
        }
    }
}

#[tauri::command]
fn rig_stop(state: State<Shared>) {
    stop_rig(&state);
}

#[tauri::command]
fn engine_modes() -> Value {
    json!(kahf_core::kami::mode_table()
        .iter()
        .map(|m| json!({ "name": m.name, "needSnr": m.need_snr, "rawBps": m.raw_bps, "netBps": m.net_bps }))
        .collect::<Vec<_>>())
}

/// Starts (or restarts) the engine with the person's settings.
#[tauri::command]
fn engine_start(app: AppHandle, state: State<Shared>, settings: Value) -> Res<()> {
    let s = EngineSettings::from_json(&settings);
    // A radio chosen by name gets its own radio-control program. If that fails, KAHF still
    // starts with sound only, and says so.
    let mut rigctld = s.rigctld();
    stop_rig(&state);
    if let Some((model, port, baud)) = &s.rig_choice {
        match rig::start_rigctld(*model, port, *baud) {
            Ok(child) => {
                if let Ok(mut g) = state.rigctld.lock() {
                    *g = Some(child);
                }
                rigctld = Some(rig::local_addr().to_string());
            }
            Err(e) => {
                let _ = app.emit("engine", json!({ "type": "log", "text": format!("Radio control is off: {e}") }));
            }
        }
    }
    let cfg = Config {
        callsign: s.callsign(),
        audio_in: s.audio_in(),
        audio_out: s.audio_out(),
        rigctld,
        sdr: s.sdr_spec(),
        data_dir: state.data_dir.clone(),
    };
    let mut engine = Engine::new(cfg)?;
    engine.listen()?;
    if let Some(hints) = settings["relayHints"].as_array() {
        let calls: Vec<String> = hints.iter().filter_map(|x| x.as_str().map(String::from)).collect();
        engine.set_relay_hints(&calls);
    }
    engine.set_tx_level(s.tx_level);
    engine.set_location(settings::shared_position(&settings));
    engine.set_listen_only(s.listen_only);
    engine.set_auto_link(s.auto_link);
    if s.auto_mode {
        engine.set_auto_mode(true);
    } else {
        engine.set_fixed_mode(&s.fixed_mode)?;
    }
    *state.engine.lock().map_err(|e| e.to_string())? = Some(engine);
    Ok(())
}

fn with_engine<T>(state: &State<Shared>, f: impl FnOnce(&mut Engine) -> Res<T>) -> Res<T> {
    let mut guard = state.engine.lock().map_err(|e| e.to_string())?;
    match guard.as_mut() {
        Some(e) => f(e),
        None => Err("The radio has not started yet. Finish setup first.".into()),
    }
}

#[tauri::command]
fn engine_connect(state: State<Shared>, call: String) -> Res<()> {
    with_engine(&state, |e| e.connect(&call))
}

#[tauri::command]
fn engine_disconnect(state: State<Shared>) -> Res<()> {
    with_engine(&state, |e| e.disconnect())
}

#[tauri::command]
fn engine_send(state: State<Shared>, id: String, to: String, text: String, kind: String) -> Res<()> {
    let engine_id = with_engine(&state, |e| match kind.as_str() {
        "private" => e.send_chat(&to, &text, true),
        "urgent" => e.send_chat(&to, &format!("URGENT: {text}"), false),
        "signed" => e.send_mail(&to, "Message", &text, &[]),
        _ => e.send_chat(&to, &text, false),
    })?;
    track(&state, &id, &to, engine_id)
}

#[tauri::command]
fn engine_set_auto_mode(state: State<Shared>, on: bool) -> Res<()> {
    with_engine(&state, |e| {
        e.set_auto_mode(on);
        Ok(())
    })
}

#[tauri::command]
fn engine_set_fixed_mode(state: State<Shared>, name: String) -> Res<()> {
    with_engine(&state, |e| e.set_fixed_mode(&name))
}

#[tauri::command]
fn engine_set_listen_only(state: State<Shared>, on: bool) -> Res<()> {
    with_engine(&state, |e| {
        e.set_listen_only(on);
        Ok(())
    })
}

#[tauri::command]
fn engine_set_auto_link(state: State<Shared>, on: bool) -> Res<()> {
    with_engine(&state, |e| {
        e.set_auto_link(on);
        Ok(())
    })
}

/// Puts the relay on the public map, in its one-degree square, if the person chose to share a
/// location. Returns a sentence for the person.
#[tauri::command]
fn map_share(state: State<Shared>) -> Res<String> {
    let v = settings::load(&state.data_dir);
    let s = EngineSettings::from_json(&v);
    let loc = &v["location"];
    if loc["mode"].as_str().unwrap_or("off") == "off" {
        return Ok("You chose not to share your location, so nothing was sent.".into());
    }
    let (Some(lat), Some(lon)) = (loc["lat"].as_f64(), loc["lon"].as_f64()) else {
        return Err("Enter your position in Settings first.".into());
    };
    if !v["relay"].as_bool().unwrap_or(false) {
        return Ok("The map lists relays for now. Turn on relay mode to appear on it.".into());
    }
    let key = kahf_core::auth::load_or_create(&state.data_dir).map_err(|e| format!("Cannot open your station key: {e}"))?;
    report::register_relay(&key, &s.callsign(), lat, lon)
}

#[tauri::command]
fn engine_set_relay(app: AppHandle, state: State<Shared>, on: bool, licence_held: bool, terms_accepted: bool) -> Res<()> {
    // The relay works on the frequency the person listens on.
    let khz = EngineSettings::from_json(&settings::load(&state.data_dir)).freq_khz as f32;
    with_engine(&state, |e| {
        if on {
            e.enable_relay(khz, licence_held, terms_accepted)
        } else {
            e.stop_relay();
            e.set_backbone(None);
            Ok(())
        }
    })?;
    if on {
        // Join the network over the internet in the background: register with the server, then
        // let the engine carry messages through it. Without a shared area or without internet the
        // relay still works on the radio side, and says so.
        std::thread::spawn(move || {
            let shared = app.state::<Shared>();
            let v = settings::load(&shared.data_dir);
            let note = |text: String| {
                let _ = app.emit("engine", json!({ "type": "log", "text": text }));
            };
            let loc = &v["location"];
            let (Some(lat), Some(lon)) = (loc["lat"].as_f64(), loc["lon"].as_f64()) else {
                return note("Relay works on the radio only: enter your position in Settings to join the network.".into());
            };
            if loc["mode"].as_str().unwrap_or("off") == "off" {
                return note("Relay works on the radio only: location sharing is off, so the network cannot list this relay.".into());
            }
            let s = EngineSettings::from_json(&v);
            let Ok(key) = kahf_core::auth::load_or_create(&shared.data_dir) else {
                return note("Relay works on the radio only: the station key could not be opened.".into());
            };
            match report::register_relay(&key, &s.callsign(), lat, lon) {
                Ok(msg) => {
                    if let Ok(mut g) = shared.engine.lock() {
                        if let Some(e) = g.as_mut() {
                            e.set_backbone(Some(report::SERVER));
                        }
                    }
                    note(format!("{msg} Messages for stations out of range now travel through the network."));
                }
                Err(e) => note(format!("Relay works on the radio only: {e}")),
            }
        });
    }
    Ok(())
}

/// Remembers that the window's message `id` is waiting in the engine's outbox as `engine_id`.
fn track(state: &State<Shared>, id: &str, to: &str, engine_id: u64) -> Res<()> {
    state.pending.lock().map_err(|e| e.to_string())?.push(Pending { ui_id: id.into(), to: to.into(), engine_id });
    Ok(())
}

#[tauri::command]
fn engine_send_mail(state: State<Shared>, id: String, to: String, subject: String, body: String, paths: Vec<String>) -> Res<()> {
    let files: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    let engine_id = with_engine(&state, |e| e.send_mail(&to, &subject, &body, &files))?;
    track(&state, &id, &to, engine_id)
}

#[tauri::command]
fn engine_send_file(state: State<Shared>, id: String, to: String, path: String) -> Res<()> {
    let engine_id = with_engine(&state, |e| e.send_file(&to, std::path::Path::new(&path)))?;
    track(&state, &id, &to, engine_id)
}

/// Opens the system's file chooser (several files allowed). Empty when the person cancels.
#[tauri::command]
async fn pick_files(app: AppHandle) -> Res<Vec<Value>> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog().file().pick_files(move |picked| {
        let _ = tx.send(picked);
    });
    let picked = rx.recv().map_err(|e| e.to_string())?.unwrap_or_default();
    Ok(picked
        .into_iter()
        .filter_map(|f| f.into_path().ok())
        .map(|p| {
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            json!({ "path": p.to_string_lossy(), "name": name, "size": size })
        })
        .collect())
}

/// Shows a received file in the system's file manager.
#[tauri::command]
fn open_path(path: String) -> Res<()> {
    let p = PathBuf::from(&path);
    if !p.exists() {
        return Err("That file is not there any more.".into());
    }
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg("-R").arg(&p).spawn();
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("explorer").arg(format!("/select,{}", p.display())).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(p.parent().unwrap_or(&p)).spawn();
    r.map(|_| ()).map_err(|e| format!("Cannot open the folder: {e}"))
}

#[tauri::command]
fn voice_record(app: AppHandle, state: State<Shared>) -> Res<()> {
    let mic = settings::load(&state.data_dir)["micIn"].as_str().map(str::trim).filter(|m| !m.is_empty()).map(String::from);
    state.voice.record(app, mic)
}

#[tauri::command]
fn voice_stop(state: State<Shared>) -> Res<Option<Value>> {
    state.voice.stop(&state.data_dir)
}

#[tauri::command]
fn voice_cancel(state: State<Shared>) {
    state.voice.cancel();
}

#[tauri::command]
fn voice_play(state: State<Shared>, path: String) -> Res<()> {
    state.voice.play(std::path::Path::new(&path), None)
}

#[tauri::command]
fn voice_send(state: State<Shared>, id: String, to: String, clip_id: String) -> Res<()> {
    let path = voice::Voice::clip_path(&state.data_dir, &clip_id)?;
    let engine_id = with_engine(&state, |e| e.send_file(&to, &path))?;
    track(&state, &id, &to, engine_id)
}

#[tauri::command]
fn relay_status(state: State<Shared>) -> Res<Value> {
    let v = settings::load(&state.data_dir);
    let area = match (v["location"]["lat"].as_f64(), v["location"]["lon"].as_f64()) {
        (Some(la), Some(lo)) if v["location"]["mode"].as_str().unwrap_or("off") != "off" => Some(report::area_key(la, lo)),
        _ => None,
    };
    let running = v["relay"].as_bool().unwrap_or(false);
    let (hops, total) = match state.engine.lock().map_err(|e| e.to_string())?.as_ref() {
        Some(e) => e.relay_recent(25),
        None => (Vec::new(), 0),
    };
    let day_ago = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().saturating_sub(86_400))
        .unwrap_or(0);
    let today = hops.iter().filter(|h| h.at_unix >= day_ago).count();
    Ok(json!({
        "running": running,
        "forwardedToday": today,
        "forwardedTotal": total,
        "recent": hops.iter().map(|h| json!({
            "at": h.at_unix * 1000, "from": h.relay, "to": "", "kind": if h.heard_by_next { "forwarded" } else { "sent on" }
        })).collect::<Vec<_>>(),
        "area": if running { area } else { None },
    }))
}

#[tauri::command]
async fn relay_directory(state: State<'_, Shared>, refresh: bool) -> Res<Vec<Value>> {
    let v = settings::load(&state.data_dir);
    let here = match (v["location"]["lat"].as_f64(), v["location"]["lon"].as_f64()) {
        (Some(la), Some(lo)) => Some((la, lo)),
        _ => None,
    };
    let manual: Vec<String> = v["relayHints"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let dir = state.data_dir.clone();
    tauri::async_runtime::spawn_blocking(move || directory::directory(&dir, here, &manual, refresh))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
fn engine_set_tx_level(state: State<Shared>, pct: u8) -> Res<()> {
    with_engine(&state, |e| {
        e.set_tx_level(pct);
        Ok(())
    })
}

/// A steady tone for five seconds, to set the radio's drive by its ALC meter. This transmits.
#[tauri::command]
fn engine_test_tone(state: State<Shared>) -> Res<()> {
    with_engine(&state, |e| e.test_tone())
}

#[tauri::command]
fn engine_advertise(state: State<Shared>) -> Res<()> {
    let pos = settings::shared_position(&settings::load(&state.data_dir));
    with_engine(&state, |e| {
        e.set_location(pos);
        e.advertise()
    })
}

#[tauri::command]
fn engine_set_relay_hints(state: State<Shared>, calls: Vec<String>) -> Res<()> {
    with_engine(&state, |e| {
        e.set_relay_hints(&calls);
        Ok(())
    })
}

/// Pulls the engine's events about 25 times a second and hands them to the window.
fn poll_loop(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(40));
        let shared = app.state::<Shared>();
        let (events, outbox) = {
            let Ok(mut guard) = shared.engine.lock() else { continue };
            match guard.as_mut() {
                Some(e) => (e.poll_events(), e.outbox_pending().iter().map(|p| p.0).collect::<Vec<u64>>()),
                None => continue,
            }
        };
        for ev in &events {
            let pending = shared.pending.lock().map(|p| p.clone()).unwrap_or_default();
            for out in event_json(ev, &pending) {
                let _ = app.emit("engine", out);
            }
        }
        // A message that left the outbox has been sent over the air.
        let mut sent: Vec<String> = Vec::new();
        if let Ok(mut pending) = shared.pending.lock() {
            pending.retain(|p| {
                let gone = !outbox.contains(&p.engine_id);
                if gone && !sent.contains(&p.ui_id) {
                    sent.push(p.ui_id.clone());
                }
                !gone
            });
        }
        for id in sent {
            let _ = app.emit("engine", json!({ "type": "sent", "id": id }));
        }
    });
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let handle = app.handle().clone();
            app.manage(Shared {
                engine: Mutex::new(None),
                pending: Mutex::new(Vec::new()),
                data_dir: data_dir(&handle),
                rigctld: Mutex::new(None),
                voice: voice::Voice::default(),
            });
            poll_loop(handle);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            load_settings,
            save_settings,
            list_audio_devices,
            detect_sdr,
            engine_modes,
            rig_available,
            rig_models,
            rig_ports,
            rig_connect,
            rig_stop,
            engine_start,
            engine_connect,
            engine_disconnect,
            engine_send,
            engine_set_auto_mode,
            engine_set_fixed_mode,
            engine_set_listen_only,
            engine_set_auto_link,
            engine_set_relay,
            map_share,
            engine_send_mail,
            engine_send_file,
            pick_files,
            open_path,
            voice_record,
            voice_stop,
            voice_cancel,
            voice_play,
            voice_send,
            relay_status,
            relay_directory,
            engine_set_relay_hints,
            engine_advertise,
            engine_set_tx_level,
            engine_test_tone,
        ])
        .run(tauri::generate_context!())
        .expect("the KAHF window could not start");
}
