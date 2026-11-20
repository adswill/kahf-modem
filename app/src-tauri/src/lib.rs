//! The desktop app: a window (the web UI in `../ui`) on top of the KAHF engine.
//!
//! The window calls the commands below; the engine's events go back to it on the "engine"
//! channel as plain JSON. Everything the window shows comes from here or from its own settings.

mod applog;
mod convert;
mod diag;
mod directory;
mod live;
mod report;
mod rig;
mod settings;
mod update;
mod voice;

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use kahf_daemon::{Area, Config, Engine, RelayAck, ServerSync, ServerSyncOptions, RELAY_SERVER_URL};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};

use convert::{event_json, Ctx, Pending};
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
    /// The station this one is linked to, from the engine's link events.
    link: Mutex<Option<String>>,
    /// Keeps this relay registered and scheduled on kahf.dev while relay mode runs.
    sync: Mutex<Option<ServerSync>>,
    /// Why the server sync is not running while relay mode is, if it is not.
    sync_note: Mutex<Option<String>>,
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

/// This engine has no mode table to show: the modem picks its own speed.
#[tauri::command]
fn engine_modes() -> Value {
    json!([])
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
                let _ = app.emit(
                    "engine",
                    json!({ "type": "log", "text": format!("Radio control is off: {e}") }),
                );
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
    stop_server_sync(&state);
    let mut engine = Engine::new(cfg)?;
    engine.listen()?;
    // Relays the person named, tried in order when a station cannot be reached directly.
    if let Some(hints) = settings["relayHints"].as_array() {
        let calls: Vec<String> = hints
            .iter()
            .filter_map(|x| x.as_str().map(String::from))
            .collect();
        let refs: Vec<&str> = calls.iter().map(String::as_str).collect();
        if let Err(e) = engine.set_relay_paths(&refs) {
            let _ = app.emit("engine", json!({ "type": "log", "text": e }));
        }
    }
    engine.set_listen_only(s.listen_only);
    *state.link.lock().map_err(|e| e.to_string())? = None;
    *state.engine.lock().map_err(|e| e.to_string())? = Some(engine);
    Ok(())
}

fn stop_server_sync(state: &State<Shared>) {
    if let Ok(mut g) = state.sync.lock() {
        *g = None;
    }
    if let Ok(mut g) = state.sync_note.lock() {
        *g = None;
    }
}

/// This engine does not measure channel use yet.
#[tauri::command]
fn engine_occupancy() -> Value {
    Value::Null
}

/// This engine has no transmit gate besides Listen only, which the window shows itself.
#[tauri::command]
fn engine_tx_gate() -> Value {
    json!({ "mode": "real", "text": "" })
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

fn outbox_ids(e: &Engine) -> Vec<u64> {
    e.outbox_pending().iter().map(|p| p.0).collect()
}

/// Makes sure the link is up to `to` (the engine sends only over a link), then runs `send` and
/// finds the outbox id it queued. None when the item has already left the outbox.
fn send_linked(
    state: &State<Shared>,
    to: &str,
    send: impl FnOnce(&mut Engine) -> Res<Option<u64>>,
) -> Res<Option<u64>> {
    let linked = state
        .link
        .lock()
        .map_err(|e| e.to_string())?
        .as_deref()
        .is_some_and(|l| l.eq_ignore_ascii_case(to.trim()));
    let data_dir = state.data_dir.clone();
    with_engine(state, |e| {
        if !linked {
            e.connect(to)?;
        }
        plan_paths(e, &data_dir, to);
        let before = outbox_ids(e);
        let id = send(e)?;
        Ok(id.or_else(|| outbox_ids(e).into_iter().find(|i| !before.contains(i))))
    })
}

/// Points the engine's relay paths at the best 17 m relays toward `to`, using the grid
/// locators of this station and of the known stations. Does nothing when this station or `to`
/// has no position; the message then waits for `to` directly.
fn plan_paths(e: &Engine, data_dir: &std::path::Path, to: &str) {
    use kahf_prop::route::{Conditions, Node};
    let v = settings::load(data_dir);
    let (Some(lat), Some(lon)) = (v["location"]["lat"].as_f64(), v["location"]["lon"].as_f64()) else {
        return;
    };
    let known: Vec<Node> = live::read_contacts(data_dir)
        .unwrap_or_default()
        .iter()
        .filter_map(|c| live::locator_position(&c.qth).map(|p| Node::new(&c.call, p)))
        .collect();
    let Some(dest) = known.iter().find(|n| n.call.eq_ignore_ascii_case(to.trim())) else {
        return;
    };
    let me = Node::new(&EngineSettings::from_json(&v).callsign(), (lat, lon));
    let (hour, month) = live::utc_hour_month(live::now_unix());
    let _ = e.plan_relay_paths(&me, dest, &known, &Conditions::band_17m(hour, month));
}

#[tauri::command]
fn engine_send(state: State<Shared>, id: String, to: String, text: String, kind: String) -> Res<()> {
    let engine_id = send_linked(&state, &to, |e| {
        match kind.as_str() {
            "private" => e.send_message_private(&to, &text)?,
            "urgent" if !text.starts_with("URGENT:") => e.send_message(&to, &format!("URGENT: {text}"))?,
            "signed" => {
                e.set_mail_signing(true);
                let r = e.send_mail(&to, "Message", &text, &[]);
                e.set_mail_signing(false);
                return r.map(Some);
            }
            _ => e.send_message(&to, &text)?,
        }
        Ok(None)
    })?;
    track_opt(&state, &id, &to, engine_id)
}

#[tauri::command]
fn engine_set_listen_only(state: State<Shared>, on: bool) -> Res<()> {
    with_engine(&state, |e| {
        e.set_listen_only(on);
        Ok(())
    })
}

/// The relay registers itself on kahf.dev when relay mode starts, so there is nothing more to
/// share by hand. Returns a sentence for the person.
#[tauri::command]
fn map_share(state: State<Shared>) -> Res<String> {
    let v = settings::load(&state.data_dir);
    if v["location"]["mode"].as_str().unwrap_or("off") == "off" {
        return Ok("You chose not to share your location, so nothing was sent.".into());
    }
    if v["location"]["lat"].as_f64().is_none() {
        return Err("Enter your position in Settings first.".into());
    }
    if state.sync.lock().map_err(|e| e.to_string())?.is_some() {
        Ok("This relay is registered on kahf.dev and listed on the map.".into())
    } else {
        Ok("The map lists relays. Turn on relay mode to appear on it.".into())
    }
}

/// Starts or stops relay mode. Relay mode works on 17 m, on every KAHF channel there. Both
/// acknowledgements must be true; the engine refuses without them as well.
#[tauri::command]
fn engine_set_relay(
    app: AppHandle,
    state: State<Shared>,
    on: bool,
    licence_held: bool,
    terms_accepted: bool,
) -> Res<()> {
    if !on {
        with_engine(&state, |e| {
            e.stop_relay();
            Ok(())
        })?;
        stop_server_sync(&state);
        return Ok(());
    }
    let channels: Vec<f32> = settings::channels_khz().iter().map(|k| *k as f32).collect();
    with_engine(&state, |e| {
        e.configure_relay(
            kahf_core::relay::Role::Relay,
            kahf_core::relay::Radio::HalfDuplex,
            &channels,
        )?;
        e.start_relay(RelayAck {
            licence_held,
            terms_accepted,
        })
    })?;
    // Register with the server in the background and keep the schedule fresh. Without a shared
    // position, or without internet, the relay still works on the radio and says so.
    let v = settings::load(&state.data_dir);
    let loc = &v["location"];
    let note = |text: &str| {
        if let Ok(mut g) = app.state::<Shared>().sync_note.lock() {
            *g = Some(text.to_string());
        }
    };
    let (Some(lat), Some(lon)) = (loc["lat"].as_f64(), loc["lon"].as_f64()) else {
        note("Relay works on the radio only: enter your position in Settings to join the network.");
        return Ok(());
    };
    if loc["mode"].as_str().unwrap_or("off") == "off" {
        note("Relay works on the radio only: location sharing is off, so kahf.dev cannot list this relay.");
        return Ok(());
    }
    let started = match Area::from_position(lat, lon) {
        Ok(area) => with_engine(&state, |e| {
            e.start_server_sync(ServerSyncOptions {
                base_url: RELAY_SERVER_URL.to_string(),
                area,
                requests: Vec::new(),
            })
        }),
        Err(e) => Err(e),
    };
    match started {
        Ok(sync) => {
            *state.sync.lock().map_err(|e| e.to_string())? = Some(sync);
            *state.sync_note.lock().map_err(|e| e.to_string())? = None;
        }
        Err(why) => {
            note("Relay works on the radio only: kahf.dev could not be reached to register.");
            let _ = app.emit("engine", json!({ "type": "log", "text": why }));
        }
    }
    Ok(())
}

/// Remembers that the window's message `id` is waiting in the engine's outbox as `engine_id`.
fn track(state: &State<Shared>, id: &str, to: &str, engine_id: u64) -> Res<()> {
    state.pending.lock().map_err(|e| e.to_string())?.push(Pending {
        ui_id: id.into(),
        to: to.into(),
        engine_id,
    });
    Ok(())
}

/// Like `track`, for an item that may already have left the outbox (then it counts as sent).
fn track_opt(state: &State<Shared>, id: &str, to: &str, engine_id: Option<u64>) -> Res<()> {
    match engine_id {
        Some(n) => track(state, id, to, n),
        None => Ok(()),
    }
}

#[tauri::command]
fn engine_send_mail(
    state: State<Shared>,
    id: String,
    to: String,
    subject: String,
    body: String,
    paths: Vec<String>,
) -> Res<()> {
    let files: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    let engine_id = send_linked(&state, &to, |e| {
        e.send_mail(&to, &subject, &body, &files).map(Some)
    })?;
    track_opt(&state, &id, &to, engine_id)
}

#[tauri::command]
fn engine_send_file(state: State<Shared>, id: String, to: String, path: String) -> Res<()> {
    let engine_id = send_linked(&state, &to, |e| {
        e.send_file(&to, std::path::Path::new(&path)).map(Some)
    })?;
    track_opt(&state, &id, &to, engine_id)
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
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
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
    let r = std::process::Command::new("explorer")
        .arg(format!("/select,{}", p.display()))
        .spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open")
        .arg(p.parent().unwrap_or(&p))
        .spawn();
    r.map(|_| ()).map_err(|e| format!("Cannot open the folder: {e}"))
}

#[tauri::command]
fn voice_record(app: AppHandle, state: State<Shared>) -> Res<()> {
    let mic = settings::load(&state.data_dir)["micIn"]
        .as_str()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(String::from);
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
    let speaker = settings::load(&state.data_dir)["speakerOut"]
        .as_str()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(String::from);
    state.voice.play(std::path::Path::new(&path), speaker)
}

#[tauri::command]
fn voice_stop_play(state: State<Shared>) {
    state.voice.stop_play();
}

#[tauri::command]
fn voice_send(state: State<Shared>, id: String, to: String, clip_id: String) -> Res<()> {
    let path = voice::Voice::clip_path(&state.data_dir, &clip_id)?;
    let engine_id = send_linked(&state, &to, |e| e.send_file(&to, &path).map(Some))?;
    track_opt(&state, &id, &to, engine_id)
}

#[tauri::command]
fn relay_status(state: State<Shared>) -> Res<Value> {
    let v = settings::load(&state.data_dir);
    let area = match (v["location"]["lat"].as_f64(), v["location"]["lon"].as_f64()) {
        (Some(la), Some(lo)) if v["location"]["mode"].as_str().unwrap_or("off") != "off" => {
            Some(report::area_key(la, lo))
        }
        _ => None,
    };
    let guard = state.engine.lock().map_err(|e| e.to_string())?;
    let engine = guard.as_ref();
    let running = engine.is_some_and(Engine::relay_running);
    let stats = engine
        .map(|e| live::stats_view(&e.relay_stats()))
        .unwrap_or_default();
    let hops: Vec<live::HopView> = engine
        .map(|e| {
            e.recent_hops(live::HOP_ROWS)
                .iter()
                .map(live::HopView::from)
                .collect()
        })
        .unwrap_or_default();
    let slots: Vec<live::SlotView> = engine
        .map(|e| e.relay_schedule().iter().map(live::SlotView::from).collect())
        .unwrap_or_default();
    let outbox: Vec<Value> = engine
        .map(|e| {
            e.outbox_pending()
                .into_iter()
                .map(|(id, what)| json!({ "id": id, "what": what }))
                .collect()
        })
        .unwrap_or_default();
    let server = state
        .sync
        .lock()
        .map_err(|e| e.to_string())?
        .as_ref()
        .map(|s| live::SyncView::from(&s.status()));
    let note = state.sync_note.lock().map_err(|e| e.to_string())?.clone();
    Ok(json!({
        "running": running,
        "forwardedToday": stats.sent_today,
        "forwardedTotal": stats.sent_total,
        "perHour": stats.per_hour,
        "recent": hops.iter().map(|h| json!({
            "at": h.at_unix * 1000, "from": h.relay, "to": "", "kind": if h.heard_by_next { "heard by the next station" } else { "sent on" },
            "channelKhz": h.channel_khz, "messageId": h.message_id
        })).collect::<Vec<_>>(),
        "slots": slots,
        "outbox": outbox,
        "server": server,
        "serverNote": note,
        "area": if running { area } else { None },
        "nowUnix": live::now_unix(),
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

/// Switches this engine does not have. The window hides them; these keep old calls harmless.
#[tauri::command]
fn engine_set_auto_mode(_on: bool) -> Res<()> {
    Ok(())
}

#[tauri::command]
fn engine_set_fixed_mode(_name: String) -> Res<()> {
    Ok(())
}

#[tauri::command]
fn engine_set_auto_link(_on: bool) -> Res<()> {
    Ok(())
}

const NOT_HERE: &str = "This version of the radio engine does not have that yet.";

#[tauri::command]
fn engine_set_tx_level(_pct: u8) -> Res<()> {
    Err(NOT_HERE.into())
}

#[tauri::command]
fn engine_test_tone() -> Res<()> {
    Err(NOT_HERE.into())
}

#[tauri::command]
fn engine_advertise() -> Res<()> {
    Err(NOT_HERE.into())
}

#[tauri::command]
fn engine_set_relay_hints(state: State<Shared>, calls: Vec<String>) -> Res<()> {
    let refs: Vec<&str> = calls.iter().map(String::as_str).collect();
    with_engine(&state, |e| e.set_relay_paths(&refs))
}

/// Stations from the address book (most recently heard first) and the logbook (newest first).
#[tauri::command]
fn logbook(state: State<Shared>) -> Value {
    let (stations, qsos, error) = live::read_files(&state.data_dir);
    json!({ "stations": stations, "qsos": qsos, "error": error })
}

/// Writes the logbook as ADIF next to it and returns the path written.
#[tauri::command]
fn export_adif(state: State<Shared>) -> Res<String> {
    let (_, qsos) = live::read_qsos(&state.data_dir)?;
    if qsos.is_empty() {
        return Err("The logbook is empty, so there is nothing to export.".into());
    }
    live::export_adif(&qsos, &state.data_dir).map(|p| p.display().to_string())
}

#[tauri::command]
async fn test_rig(addr: String) -> Res<String> {
    tauri::async_runtime::spawn_blocking(move || diag::test_rig(&addr))
        .await
        .unwrap_or_else(|_| Err("The test stopped unexpectedly.".into()))
}

#[tauri::command]
async fn test_audio_in(device: String) -> Res<String> {
    let dev = (!device.is_empty()).then_some(device);
    tauri::async_runtime::spawn_blocking(move || diag::test_audio_in(dev.as_deref()))
        .await
        .unwrap_or_else(|_| Err("The test stopped unexpectedly.".into()))
}

#[tauri::command]
async fn test_audio_out(device: String) -> Res<String> {
    let dev = (!device.is_empty()).then_some(device);
    tauri::async_runtime::spawn_blocking(move || diag::test_audio_out(dev.as_deref()))
        .await
        .unwrap_or_else(|_| Err("The test stopped unexpectedly.".into()))
}

/// Asks GitHub for the newest release. Only when the person presses the button.
#[tauri::command]
async fn check_update() -> Res<Value> {
    let found = tauri::async_runtime::spawn_blocking(|| update::check(env!("CARGO_PKG_VERSION")))
        .await
        .map_err(|e| e.to_string())??;
    Ok(match found {
        update::Outcome::UpToDate => {
            json!({ "outcome": "up_to_date", "version": env!("CARGO_PKG_VERSION"), "url": "" })
        }
        update::Outcome::Newer(l) => json!({ "outcome": "newer", "version": l.version, "url": l.url }),
    })
}

/// Asks the public station list on kahf.dev. Sends nothing about this station.
#[tauri::command]
async fn server_status() -> Value {
    tauri::async_runtime::spawn_blocking(|| {
        let started = Instant::now();
        let result = ureq::get(&format!("{RELAY_SERVER_URL}/stations"))
            .set("User-Agent", "kahf-app-status")
            .timeout(Duration::from_secs(8))
            .call()
            .map_err(|e| e.to_string())
            .and_then(|r| r.into_string().map_err(|e| e.to_string()));
        let millis = started.elapsed().as_millis() as u64;
        match result {
            Ok(body) => {
                let n = serde_json::from_str::<Value>(&body)
                    .ok()
                    .and_then(|v| v["stations"].as_array().map(Vec::len))
                    .unwrap_or(0);
                json!({ "reachable": true, "millis": millis, "stationsListed": n, "error": Value::Null })
            }
            Err(e) => json!({ "reachable": false, "millis": millis, "stationsListed": 0, "error": e }),
        }
    })
    .await
    .unwrap_or_else(
        |_| json!({ "reachable": false, "millis": 0, "stationsListed": 0, "error": "The check stopped." }),
    )
}

fn open_dir(dir: &std::path::Path) -> Res<()> {
    let _ = std::fs::create_dir_all(dir);
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(dir).spawn();
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("explorer").arg(dir).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(dir).spawn();
    r.map(|_| ()).map_err(|e| format!("Cannot open the folder: {e}"))
}

#[tauri::command]
fn open_logs() -> Res<()> {
    open_dir(&log_dir())
}

#[tauri::command]
fn open_data_dir(state: State<Shared>) -> Res<()> {
    open_dir(&state.data_dir)
}

/// Where the log files are kept, next to the other KAHF files.
fn log_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .filter(|_| cfg!(windows))
        .map(|a| PathBuf::from(a).join("kahf"))
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|h| PathBuf::from(h).join(".config").join("kahf"))
        });
    base.map(|d| d.join("logs"))
        .unwrap_or_else(|| PathBuf::from("kahf-logs"))
}

/// Opens a page in the browser. Only KAHF's own pages and its releases.
#[tauri::command]
fn open_url(url: String) -> Res<()> {
    let ok = url == "https://kahf.dev"
        || ["https://kahf.dev/", "https://github.com/adswill/kahf-modem/"]
            .iter()
            .any(|p| url.starts_with(p));
    if !ok {
        return Err("That address is not one of KAHF's pages.".into());
    }
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(&url).spawn();
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("explorer").arg(&url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(&url).spawn();
    r.map(|_| ()).map_err(|e| format!("Cannot open the browser: {e}"))
}

#[tauri::command]
fn app_info(state: State<Shared>) -> Value {
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "dataDir": state.data_dir.display().to_string(),
        "logDir": log_dir().display().to_string(),
    })
}

/// Pulls the engine's events about 25 times a second and hands them to the window.
fn poll_loop(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(40));
        let shared = app.state::<Shared>();
        let (events, outbox) = {
            let Ok(mut guard) = shared.engine.lock() else {
                continue;
            };
            match guard.as_mut() {
                Some(e) => (
                    e.poll_events(),
                    e.outbox_pending().iter().map(|p| p.0).collect::<Vec<u64>>(),
                ),
                None => continue,
            }
        };
        let inbox = shared.data_dir.join("inbox");
        for ev in &events {
            if let kahf_daemon::Event::Link(state) = ev {
                if let Ok(mut l) = shared.link.lock() {
                    *l = match state {
                        kahf_core::link::LinkState::Connected { peer } => Some(peer.clone()),
                        _ => None,
                    };
                }
            }
            let pending = shared.pending.lock().map(|p| p.clone()).unwrap_or_default();
            let peer = shared.link.lock().ok().and_then(|l| l.clone());
            let ctx = Ctx {
                pending: &pending,
                inbox: &inbox,
                peer: peer.as_deref(),
            };
            for out in event_json(ev, &ctx) {
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
    applog::init(&log_dir());
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
                link: Mutex::new(None),
                sync: Mutex::new(None),
                sync_note: Mutex::new(None),
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
            engine_tx_gate,
            engine_occupancy,
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
            voice_stop_play,
            voice_send,
            relay_status,
            relay_directory,
            engine_set_relay_hints,
            engine_advertise,
            engine_set_tx_level,
            engine_test_tone,
            logbook,
            export_adif,
            test_rig,
            test_audio_in,
            test_audio_out,
            check_update,
            server_status,
            open_logs,
            open_data_dir,
            open_url,
            app_info,
        ])
        .run(tauri::generate_context!())
        .expect("the KAHF window could not start");
}
