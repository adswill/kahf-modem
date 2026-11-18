//! The desktop app: a window (the web UI in `../ui`) on top of the KAHF engine.
//!
//! The window calls the commands below; the engine's events go back to it on the "engine"
//! channel as plain JSON. Everything the window shows comes from here or from its own settings.

mod convert;
mod report;
mod rig;
mod settings;

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
    let before: Vec<u64> = with_engine(&state, |e| Ok(e.outbox_pending().iter().map(|p| p.0).collect()))?;
    with_engine(&state, |e| match kind.as_str() {
        "private" => e.send_message_private(&to, &text),
        "urgent" => e.send_message(&to, &format!("URGENT: {text}")),
        "signed" => e.send_mail(&to, "Message", &text, &[]).map(|_| ()),
        _ => e.send_message(&to, &text),
    })?;
    let after: Vec<u64> = with_engine(&state, |e| Ok(e.outbox_pending().iter().map(|p| p.0).collect()))?;
    let mut pending = state.pending.lock().map_err(|e| e.to_string())?;
    for engine_id in after.into_iter().filter(|p| !before.contains(p)) {
        pending.push(Pending { ui_id: id.clone(), to: to.clone(), engine_id });
    }
    Ok(())
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
fn engine_set_relay(state: State<Shared>, on: bool, licence_held: bool, terms_accepted: bool) -> Res<()> {
    // The relay works on the frequency the person listens on.
    let khz = EngineSettings::from_json(&settings::load(&state.data_dir)).freq_khz as f32;
    with_engine(&state, |e| {
        if on {
            e.enable_relay(khz, licence_held, terms_accepted)
        } else {
            e.stop_relay();
            Ok(())
        }
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
        .setup(|app| {
            let handle = app.handle().clone();
            app.manage(Shared {
                engine: Mutex::new(None),
                pending: Mutex::new(Vec::new()),
                data_dir: data_dir(&handle),
                rigctld: Mutex::new(None),
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
        ])
        .run(tauri::generate_context!())
        .expect("the KAHF window could not start");
}
