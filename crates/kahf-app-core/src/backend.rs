//! What the desktop app does besides drawing: the engine, the radio, voice notes, the logbook.
//!
//! The window calls the functions below; the engine's events go back to it through the
//! [`Emitter`] as plain JSON. Everything the window shows comes from here or from its own settings.

use crate::{convert, diag, directory, live, report, rig, settings, update, voice};

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use kahf_daemon::{Config, Engine};

/// The kahf.dev relay mesh: where a running relay links to, and the public station list.
const RELAY_SERVER_URL: &str = "https://kahf.dev";
use serde_json::{json, Value};

use convert::{event_json, Ctx, Pending};
use settings::EngineSettings;

/// Where the engine's events go: the window installs one that wakes it.
pub type Emitter = Arc<dyn Fn(Value) + Send + Sync>;

/// What the app keeps while it runs.
pub struct Shared {
    /// Hands an event to the window.
    pub emit: Emitter,
    engine: Mutex<Option<Engine>>,
    /// Messages the window sent that are still in the engine's outbox.
    pending: Mutex<Vec<Pending>>,
    data_dir: PathBuf,
    /// The `rigctld` KAHF started for the chosen radio, if any.
    rigctld: Mutex<Option<std::process::Child>>,
    voice: voice::Voice,
    /// The station this one is linked to, from the engine's link events.
    link: Mutex<Option<String>>,
    /// Outbox ids this app queued (the last few hundred).
    own_ids: Mutex<Vec<u64>>,
    /// Why the relay is not linked to the kahf.dev mesh while relay mode runs, if it is not.
    sync_note: Mutex<Option<String>>,
}

type Res<T> = Result<T, String>;

/// Runs `f` on its own thread and waits for it, so a panic inside becomes an error.
fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    std::thread::spawn(f)
        .join()
        .map_err(|_| "The task stopped unexpectedly.".to_string())
}

/// Where settings, the logbook and received files live. `KAHF_DATA_DIR` overrides it. The
/// default is the folder earlier versions of the app used, so nobody's settings are lost.
pub fn data_dir() -> PathBuf {
    if let Ok(p) = std::env::var("KAHF_DATA_DIR") {
        return PathBuf::from(p);
    }
    const ID: &str = "dev.kahf.app";
    let home = || {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
    };
    let base = if cfg!(target_os = "macos") {
        home().map(|h| h.join("Library").join("Application Support"))
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| home().map(|h| h.join(".local").join("share")))
    };
    base.map(|b| b.join(ID)).unwrap_or_else(|| PathBuf::from("."))
}

impl Shared {
    /// Where settings, the logbook and received files live.
    pub fn data_dir(&self) -> &std::path::Path {
        &self.data_dir
    }

    /// A fresh app state that reports engine events through `emit`.
    pub fn new(emit: Emitter, data_dir: PathBuf) -> Self {
        Shared {
            emit,
            engine: Mutex::new(None),
            pending: Mutex::new(Vec::new()),
            data_dir,
            rigctld: Mutex::new(None),
            voice: voice::Voice::default(),
            link: Mutex::new(None),
            own_ids: Mutex::new(Vec::new()),
            sync_note: Mutex::new(None),
        }
    }

    fn log(&self, text: impl Into<String>) {
        (self.emit)(json!({ "type": "log", "text": text.into() }));
    }
}

pub fn load_settings(state: &Shared) -> Value {
    settings::load(&state.data_dir)
}

pub fn save_settings(state: &Shared, settings: Value) -> Res<()> {
    settings::save(&state.data_dir, &settings)
}

pub fn list_audio_devices() -> Vec<String> {
    kahf_audio::list_devices()
}

/// The receivers KAHF can talk to, judged by which driver libraries are installed.
pub fn detect_sdr() -> Vec<String> {
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
pub fn rig_available() -> bool {
    rig::find_tool("rigctld").is_some() && rig::find_tool("rigctl").is_some()
}

pub fn rig_models() -> Res<Vec<rig::RigModel>> {
    rig::list_rigs()
}

pub fn rig_ports() -> Vec<String> {
    rig::serial_ports()
}

fn stop_rig(state: &Shared) {
    if let Ok(mut g) = state.rigctld.lock() {
        if let Some(mut c) = g.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// Starts radio control for the chosen radio and reads its frequency as a test.
/// Returns the frequency in Hz.
pub fn rig_connect(state: &Shared, model: u32, port: String, baud: u32) -> Res<u64> {
    stop_rig(state);
    let child = rig::start_rigctld(model, &port, baud)?;
    *state.rigctld.lock().map_err(|e| e.to_string())? = Some(child);
    match rig::read_frequency(rig::local_addr()) {
        Ok(hz) => Ok(hz),
        Err(e) => {
            stop_rig(state);
            Err(e)
        }
    }
}

pub fn rig_stop(state: &Shared) {
    stop_rig(state);
}

/// Starts (or restarts) the engine with the person's settings.
pub fn engine_start(state: &Shared, settings: Value) -> Res<()> {
    let s = EngineSettings::from_json(&settings);
    // The old engine goes first, so it lets go of the sound device, the radio port and the SDR
    // before the new one opens them. Entries for its messages do not belong to the new one.
    let old = state.engine.lock().map_err(|e| e.to_string())?.take();
    drop(old);
    state.pending.lock().map_err(|e| e.to_string())?.clear();
    // A radio chosen by name gets its own radio-control program. If that fails, KAHF still
    // starts with sound only, and says so.
    let mut rigctld = s.rigctld();
    stop_rig(state);
    if let Some((model, port, baud)) = &s.rig_choice {
        match rig::start_rigctld(*model, port, *baud) {
            Ok(child) => {
                if let Ok(mut g) = state.rigctld.lock() {
                    *g = Some(child);
                }
                rigctld = Some(rig::local_addr().to_string());
            }
            Err(e) => {
                state.log(format!("Radio control is off: {e}"));
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
    stop_server_sync(state);
    let engine = match Engine::new(cfg).and_then(|mut e| e.listen().map(|()| e)) {
        Ok(e) => e,
        Err(why) => {
            stop_rig(state);
            return Err(why);
        }
    };
    // Relays the person named, tried in order when a station cannot be reached directly.
    if let Some(hints) = settings["relayHints"].as_array() {
        let calls: Vec<String> = hints
            .iter()
            .filter_map(|x| x.as_str().map(String::from))
            .collect();
        engine.set_relay_hints(&calls);
    }
    engine.set_listen_only(s.listen_only);
    engine.set_dial_hz(s.dial_hz());
    *state.link.lock().map_err(|e| e.to_string())? = None;
    *state.engine.lock().map_err(|e| e.to_string())? = Some(engine);
    Ok(())
}

fn stop_server_sync(state: &Shared) {
    if let Ok(e) = state.engine.lock() {
        if let Some(e) = e.as_ref() {
            e.set_backbone(None);
        }
    }
    if let Ok(mut g) = state.sync_note.lock() {
        *g = None;
    }
}

fn with_engine<T>(state: &Shared, f: impl FnOnce(&mut Engine) -> Res<T>) -> Res<T> {
    let mut guard = state.engine.lock().map_err(|e| e.to_string())?;
    match guard.as_mut() {
        Some(e) => f(e),
        None => Err("The radio has not started yet. Finish setup first.".into()),
    }
}

pub fn engine_connect(state: &Shared, call: String) -> Res<()> {
    with_engine(state, |e| e.connect(&call))
}

pub fn engine_disconnect(state: &Shared) -> Res<()> {
    with_engine(state, |e| e.disconnect())
}

/// Makes sure the link is up to `to` (the engine sends only over a link), then runs `send`.
/// It gives back the outbox id when the engine reports one (mail and files do, chat does not).
fn send_linked(
    state: &Shared,
    to: &str,
    send: impl FnOnce(&mut Engine) -> Res<Option<u64>>,
) -> Res<Option<u64>> {
    let linked = state
        .link
        .lock()
        .map_err(|e| e.to_string())?
        .as_deref()
        .is_some_and(|l| l.eq_ignore_ascii_case(to.trim()));
    with_engine(state, |e| {
        if !linked {
            e.connect(to)?;
        }
        send(e)
    })
}

pub fn engine_send(state: &Shared, id: String, to: String, text: String, kind: String) -> Res<()> {
    let engine_id = send_linked(state, &to, |e| {
        match kind.as_str() {
            "private" => e.send_message_private(&to, &text)?,
            "urgent" if !text.starts_with("URGENT:") => e.send_message(&to, &format!("URGENT: {text}"))?,
            // Mail is signed by default (the engine's own setting), so this is a plain mail.
            "signed" => return e.send_mail(&to, "Message", &text, &[]).map(Some),
            _ => e.send_message(&to, &text)?,
        }
        Ok(None)
    })?;
    track_opt(state, &id, &to, engine_id)
}

pub fn engine_set_listen_only(state: &Shared, on: bool) -> Res<()> {
    with_engine(state, |e| {
        e.set_listen_only(on);
        Ok(())
    })
}

/// The relay registers itself on kahf.dev when relay mode starts, so there is nothing more to
/// share by hand. Returns a sentence for the person.
pub fn map_share(state: &Shared) -> Res<String> {
    let v = settings::load(&state.data_dir);
    if v["location"]["mode"].as_str().unwrap_or("off") == "off" {
        return Ok("You chose not to share your location, so nothing was sent.".into());
    }
    if v["location"]["lat"].as_f64().is_none() {
        return Err("Enter your position in Settings first.".into());
    }
    let running = state.engine.lock().map_err(|e| e.to_string())?.as_ref().is_some_and(Engine::relay_running);
    if running {
        Ok("This relay announces itself on the air and links to the kahf.dev mesh.".into())
    } else {
        Ok("The map lists relays. Turn on relay mode to appear on it.".into())
    }
}

/// Starts or stops relay mode. Relay mode works on 17 m, on every KAHF channel there. Both
/// acknowledgements must be true; the engine refuses without them as well.
pub fn engine_set_relay(state: &Shared, on: bool, licence_held: bool, terms_accepted: bool) -> Res<()> {
    if !on {
        with_engine(state, |e| {
            e.stop_relay();
            Ok(())
        })?;
        stop_server_sync(state);
        return Ok(());
    }
    let Some(channel) = settings::channels_khz().first().map(|k| *k as f32) else {
        return Err("No relay channel is set.".into());
    };
    with_engine(state, |e| e.enable_relay(channel, licence_held, terms_accepted))?;
    // Link to the kahf.dev relay mesh so messages can also travel over the internet. The engine
    // connects in the background; without internet the relay still works on the radio.
    with_engine(state, |e| {
        e.set_backbone(Some(RELAY_SERVER_URL));
        Ok(())
    })?;
    if let Ok(mut g) = state.sync_note.lock() {
        *g = Some("Relay is set to link to the kahf.dev mesh. Without internet it works on the radio only.".into());
    }
    Ok(())
}

/// Remembers that the window's message `id` is waiting in the engine's outbox as `engine_id`.
fn track(state: &Shared, id: &str, to: &str, engine_id: u64) -> Res<()> {
    state.pending.lock().map_err(|e| e.to_string())?.push(Pending {
        ui_id: id.into(),
        to: to.into(),
        engine_id,
        at: Instant::now(),
    });
    if engine_id != 0 {
        remember_own(state, engine_id);
    }
    Ok(())
}

fn remember_own(state: &Shared, engine_id: u64) {
    if let Ok(mut own) = state.own_ids.lock() {
        own.push(engine_id);
        let excess = own.len().saturating_sub(300);
        own.drain(..excess);
    }
}

/// Like `track`. A chat message gets no id from the engine: it waits for the poll loop to find
/// the outbox item that appeared (id 0 means not known yet).
fn track_opt(state: &Shared, id: &str, to: &str, engine_id: Option<u64>) -> Res<()> {
    track(state, id, to, engine_id.unwrap_or(0))
}

pub fn engine_send_mail(
    state: &Shared,
    id: String,
    to: String,
    subject: String,
    body: String,
    paths: Vec<String>,
) -> Res<()> {
    let files: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    let engine_id = send_linked(state, &to, |e| {
        e.send_mail(&to, &subject, &body, &files).map(Some)
    })?;
    track_opt(state, &id, &to, engine_id)
}

pub fn engine_send_file(state: &Shared, id: String, to: String, path: String) -> Res<()> {
    let engine_id = send_linked(state, &to, |e| {
        e.send_file(&to, std::path::Path::new(&path)).map(Some)
    })?;
    track_opt(state, &id, &to, engine_id)
}

/// Describes files the person chose (path, name, size) for the attachment list.
pub fn describe_files(paths: &[PathBuf]) -> Vec<Value> {
    paths
        .iter()
        .map(|p| {
            let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            json!({ "path": p.to_string_lossy(), "name": name, "size": size })
        })
        .collect()
}

/// Shows a received file in the system's file manager.
pub fn open_path(path: String) -> Res<()> {
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

/// The device a voice note records from or plays to, with a log line saying which.
fn voice_device(state: &Shared, input: bool) -> Option<String> {
    let v = settings::load(&state.data_dir);
    let own = v[if input { "micIn" } else { "speakerOut" }].as_str();
    let radio = v[if input { "audioIn" } else { "audioOut" }].as_str();
    let (device, label) = voice::choose_device(
        own,
        kahf_audio::default_device_name(input).as_deref(),
        &kahf_audio::device_names(input),
        radio,
    );
    let what = if input { "recording from" } else { "playing on" };
    state.log(format!("Voice note: {what} {label}."));
    device
}

pub fn voice_record(state: &Shared) -> Res<()> {
    let mic = voice_device(state, true);
    state.voice.record(state.emit.clone(), mic)
}

pub fn voice_stop(state: &Shared) -> Res<Option<Value>> {
    state.voice.stop(&state.data_dir)
}

pub fn voice_cancel(state: &Shared) {
    state.voice.cancel();
}

pub fn voice_play(state: &Shared, path: String) -> Res<()> {
    let speaker = voice_device(state, false);
    state.voice.play(std::path::Path::new(&path), speaker)
}

pub fn voice_stop_play(state: &Shared) {
    state.voice.stop_play();
}

pub fn voice_send(state: &Shared, id: String, to: String, clip_id: String) -> Res<()> {
    let path = voice::Voice::clip_path(&state.data_dir, &clip_id)?;
    let engine_id = send_linked(state, &to, |e| e.send_file(&to, &path).map(Some))?;
    track_opt(state, &id, &to, engine_id)
}

pub fn relay_status(state: &Shared) -> Res<Value> {
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
            e.relay_recent(live::HOP_ROWS)
                .0
                .iter()
                .map(live::HopView::from)
                .collect()
        })
        .unwrap_or_default();
    // The engine plans its own transmit slot; no server schedule exists to show.
    let slots: Vec<live::SlotView> = Vec::new();
    let outbox: Vec<Value> = engine
        .map(|e| {
            e.outbox_pending()
                .into_iter()
                .map(|(id, what)| json!({ "id": id, "what": what }))
                .collect()
        })
        .unwrap_or_default();
    let server: Option<Value> = None;
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
        "referee": engine.map(|e| { let (r, q, g) = e.referee_counts(); json!({ "rounds": r, "requests": q, "grants": g, "heres": e.heres_heard() }) }),
        "area": if running { area } else { None },
        "nowUnix": live::now_unix(),
    }))
}

pub fn relay_directory(state: &Shared, refresh: bool) -> Res<Vec<Value>> {
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
    blocking(move || directory::directory(&dir, here, &manual, refresh)).map_err(|e| e.to_string())?
}

/// The radio's dial changed (the person chose another lane): tell the transmit gate.
pub fn engine_set_dial(state: &Shared, khz: f64) -> Res<()> {
    with_engine(state, |e| {
        e.set_dial_hz(Some((khz * 1000.0).round() as u64));
        Ok(())
    })
}

/// What the transmit gate says, for the Radio screen: `state` is "off", "bench" or "real".
pub fn tx_gate_status(state: &Shared) -> Res<Value> {
    let guard = state.engine.lock().map_err(|e| e.to_string())?;
    Ok(match guard.as_ref() {
        Some(e) => {
            let (st, why) = e.tx_gate();
            json!({ "state": st, "reason": why })
        }
        None => json!({ "state": "off", "reason": "The radio engine is not running." }),
    })
}

pub fn engine_set_relay_hints(state: &Shared, calls: Vec<String>) -> Res<()> {
    with_engine(state, |e| {
        e.set_relay_hints(&calls);
        Ok(())
    })
}

/// Stations from the address book (most recently heard first) and the logbook (newest first).
pub fn logbook(state: &Shared) -> Value {
    let (stations, qsos, error) = live::read_files(&state.data_dir);
    json!({ "stations": stations, "qsos": qsos, "error": error })
}

/// Writes the logbook as ADIF next to it and returns the path written.
pub fn export_adif(state: &Shared) -> Res<String> {
    let (_, qsos) = live::read_qsos(&state.data_dir)?;
    if qsos.is_empty() {
        return Err("The logbook is empty, so there is nothing to export.".into());
    }
    live::export_adif(&qsos, &state.data_dir).map(|p| p.display().to_string())
}

pub fn test_rig(addr: String) -> Res<String> {
    blocking(move || diag::test_rig(&addr)).unwrap_or_else(|_| Err("The test stopped unexpectedly.".into()))
}

pub fn test_audio_in(device: String) -> Res<String> {
    let dev = (!device.is_empty()).then_some(device);
    blocking(move || diag::test_audio_in(dev.as_deref()))
        .unwrap_or_else(|_| Err("The test stopped unexpectedly.".into()))
}

pub fn test_audio_out(device: String) -> Res<String> {
    let dev = (!device.is_empty()).then_some(device);
    blocking(move || diag::test_audio_out(dev.as_deref()))
        .unwrap_or_else(|_| Err("The test stopped unexpectedly.".into()))
}

/// The radios KAHF can control directly (model name, label, default speed).
pub fn rig_direct_models() -> Vec<Value> {
    kahf_rig::MODELS
        .iter()
        .filter(|(name, _, _)| *name != "flrig" && *name != "rigctld")
        .map(|(name, label, baud)| json!({ "id": name, "label": label, "baud": baud }))
        .collect()
}

/// Opens the radio the way the engine would and reads its frequency. Never keys the
/// transmitter.
pub fn rig_test_spec(spec: String) -> Res<String> {
    blocking(move || {
        let mut rig = kahf_rig::open(spec.trim())?;
        let hz = rig.freq_hz()?;
        Ok(format!(
            "{} answered. Dial at {:.3} kHz.",
            rig.name(),
            hz as f64 / 1000.0
        ))
    })
    .unwrap_or_else(|_| Err("The test stopped unexpectedly.".into()))
}

/// Listens to a sound input for three seconds and says whether the level is right.
pub fn audio_level_check(device: String) -> Res<Value> {
    let dev = (!device.is_empty()).then_some(device);
    blocking(move || {
        diag::check_audio_level(dev.as_deref()).map(|(verdict, advice, db)| {
            json!({ "verdict": verdict, "advice": advice, "levelDb": (db * 10.0).round() / 10.0 })
        })
    })
    .unwrap_or_else(|_| Err("The check stopped unexpectedly.".into()))
}

/// Sends the logbook to another service, only when the person presses the button.
/// `service` is "qrz", "clublog", "lotw" or "n1mm"; `cfg` holds that service's fields.
pub fn upload_logbook(state: &Shared, service: String, cfg: Value) -> Res<String> {
    let dir = state.data_dir.clone();
    let settings = settings::load(&dir);
    let call = EngineSettings::from_json(&settings).callsign();
    let grid = match (
        settings["location"]["lat"].as_f64(),
        settings["location"]["lon"].as_f64(),
    ) {
        (Some(la), Some(lo)) => live::grid_of(la, lo),
        _ => String::new(),
    };
    blocking(move || {
        let (_, qsos) = live::read_qsos(&dir)?;
        if qsos.is_empty() {
            return Err("The logbook is empty, so there is nothing to send.".to_string());
        }
        let text = |k: &str| cfg[k].as_str().unwrap_or("").trim().to_string();
        let (mut sent, mut dup) = (0, 0);
        let mut count = |r: Result<kahf_export::Upload, String>| -> Res<()> {
            match r? {
                kahf_export::Upload::Accepted => sent += 1,
                kahf_export::Upload::Duplicate => dup += 1,
            }
            Ok(())
        };
        match service.as_str() {
            "qrz" => {
                let key = text("qrzKey");
                if key.is_empty() {
                    return Err("Enter your QRZ logbook API key first.".into());
                }
                for q in &qsos {
                    count(kahf_export::upload_qrz(
                        kahf_export::QRZ_URL,
                        &key,
                        &kahf_export::adif_record(q, &call, &grid),
                    ))?;
                }
            }
            "clublog" => {
                let (email, pass, app) = (
                    text("clublogEmail"),
                    text("clublogPassword"),
                    text("clublogAppKey"),
                );
                if email.is_empty() || pass.is_empty() || app.is_empty() {
                    return Err("Enter your Club Log email, password and application key first.".into());
                }
                for q in &qsos {
                    count(kahf_export::upload_clublog(
                        kahf_export::CLUBLOG_URL,
                        &email,
                        &pass,
                        &call,
                        &app,
                        &kahf_export::adif_record(q, &call, &grid),
                    ))?;
                }
            }
            "lotw" => {
                let (tqsl, loc) = (text("tqslPath"), text("lotwLocation"));
                if tqsl.is_empty() || loc.is_empty() {
                    return Err("Enter the path of TQSL and the name of your station location first.".into());
                }
                let path = live::export_adif(&qsos, &dir)?;
                count(kahf_export::upload_lotw(&tqsl, &path.display().to_string(), &loc))?;
            }
            "n1mm" => {
                let addr = text("n1mmAddr");
                if addr.is_empty() {
                    return Err("Enter the address of N1MM, for example 127.0.0.1:12060.".into());
                }
                for q in &qsos {
                    kahf_export::send_n1mm(&addr, q, &call, &grid)?;
                    sent += 1;
                }
            }
            _ => return Err("Unknown service.".into()),
        }
        Ok(format!("{sent} sent, {dup} already there."))
    })
    .unwrap_or_else(|_| Err("The upload stopped unexpectedly.".into()))
}

/// Asks GitHub for the newest release. Only when the person presses the button.
pub fn check_update() -> Res<Value> {
    let found = blocking(|| update::check(env!("CARGO_PKG_VERSION"))).map_err(|e| e.to_string())??;
    Ok(match found {
        update::Outcome::UpToDate => {
            json!({ "outcome": "up_to_date", "version": env!("CARGO_PKG_VERSION"), "url": "" })
        }
        update::Outcome::Newer(l) => json!({ "outcome": "newer", "version": l.version, "url": l.url }),
    })
}

/// Asks the public station list on kahf.dev. Sends nothing about this station.
pub fn server_status() -> Value {
    blocking(|| {
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

pub fn open_logs() -> Res<()> {
    open_dir(&log_dir())
}

pub fn open_data_dir(state: &Shared) -> Res<()> {
    open_dir(&state.data_dir)
}

/// Where the log files are kept, next to the other KAHF files.
pub fn log_dir() -> PathBuf {
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
pub fn open_url(url: String) -> Res<()> {
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

pub fn app_info(state: &Shared) -> Value {
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "dataDir": state.data_dir.display().to_string(),
        "logDir": log_dir().display().to_string(),
    })
}

/// Pulls the engine's events about 25 times a second and hands them to the window. Runs on its
/// own thread for as long as the app lives.
pub fn start_poll_loop(shared: Arc<Shared>) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(40));
        let (events, sent) = {
            let Ok(mut guard) = shared.engine.lock() else {
                continue;
            };
            let Some(e) = guard.as_mut() else {
                continue;
            };
            let events = e.poll_events();
            let outbox: Vec<u64> = e.outbox_pending().iter().map(|p| p.0).collect();
            // The outbox and the waiting list are read under the same lock a send holds, so a
            // message queued a moment ago is never judged against an older outbox.
            let sent = settle_pending(&shared, &outbox);
            (events, sent)
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
            let own = shared.own_ids.lock().map(|o| o.clone()).unwrap_or_default();
            let peer = shared.link.lock().ok().and_then(|l| l.clone());
            let ctx = Ctx {
                pending: &pending,
                inbox: &inbox,
                peer: peer.as_deref(),
                own_ids: &own,
            };
            for out in event_json(ev, &ctx) {
                (shared.emit)(out);
            }
        }
        for id in sent {
            (shared.emit)(json!({ "type": "sent", "id": id }));
        }
    });
}

impl Drop for Shared {
    /// The radio control program KAHF started must not outlive it: it would keep the serial port
    /// and its TCP port busy for the next start.
    fn drop(&mut self) {
        if let Ok(mut g) = self.rigctld.lock() {
            if let Some(mut c) = g.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }
}

/// How long a chat message may wait for its outbox item to show up before it counts as gone.
const ADOPT_WAIT: Duration = Duration::from_secs(3);

/// Gives each chat message waiting for an id the first new outbox item, and returns the window
/// ids of messages that have left the outbox (sent over the air).
fn settle_pending(shared: &Shared, outbox: &[u64]) -> Vec<String> {
    let (Ok(mut pending), Ok(mut own)) = (shared.pending.lock(), shared.own_ids.lock()) else {
        return Vec::new();
    };
    let mut free: Vec<u64> = outbox
        .iter()
        .copied()
        .filter(|id| !pending.iter().any(|p| p.engine_id == *id) && !own.contains(id))
        .collect();
    free.sort_unstable();
    let mut free = free.into_iter();
    for p in pending.iter_mut().filter(|p| p.engine_id == 0) {
        if let Some(id) = free.next() {
            p.engine_id = id;
            own.push(id);
        }
    }
    let excess = own.len().saturating_sub(300);
    own.drain(..excess);
    let mut sent: Vec<String> = Vec::new();
    pending.retain(|p| {
        let gone = if p.engine_id == 0 {
            p.at.elapsed() > ADOPT_WAIT
        } else {
            !outbox.contains(&p.engine_id)
        };
        if gone && !sent.contains(&p.ui_id) {
            sent.push(p.ui_id.clone());
        }
        !gone
    });
    sent
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared() -> Shared {
        Shared::new(Arc::new(|_| {}), PathBuf::from("."))
    }

    fn waiting(ui: &str, id: u64) -> Pending {
        Pending {
            ui_id: ui.into(),
            to: "XX1AA".into(),
            engine_id: id,
            at: Instant::now(),
        }
    }

    #[test]
    fn a_chat_message_adopts_the_new_outbox_item_and_is_sent_when_it_leaves() {
        let s = shared();
        s.pending.lock().unwrap().push(waiting("a", 0));
        assert!(settle_pending(&s, &[7]).is_empty());
        assert_eq!(s.pending.lock().unwrap()[0].engine_id, 7);
        assert!(settle_pending(&s, &[7]).is_empty());
        assert_eq!(settle_pending(&s, &[]), vec!["a".to_string()]);
        assert!(s.pending.lock().unwrap().is_empty());
    }

    #[test]
    fn an_id_known_to_another_message_is_not_adopted() {
        let s = shared();
        s.pending.lock().unwrap().push(waiting("mail", 3));
        s.pending.lock().unwrap().push(waiting("chat", 0));
        s.own_ids.lock().unwrap().push(3);
        settle_pending(&s, &[3, 4]);
        let p = s.pending.lock().unwrap();
        assert_eq!((p[0].engine_id, p[1].engine_id), (3, 4));
    }

    #[test]
    fn a_chat_message_that_never_shows_up_counts_as_sent_after_a_while() {
        let s = shared();
        let mut p = waiting("a", 0);
        p.at = Instant::now() - Duration::from_secs(10);
        s.pending.lock().unwrap().push(p);
        assert_eq!(settle_pending(&s, &[]), vec!["a".to_string()]);
    }
}
