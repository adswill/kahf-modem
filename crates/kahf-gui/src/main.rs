//! KAHF desktop GUI: one dashboard screen. Only the public API of kahf-daemon (Config, Engine, Event) is used.
//! The radio engine starts on the first Connect or Send, never at launch. Errors are shown in plain words.
//! Nothing transmits without a confirmation, and Listen only turns every transmit path off.
mod home;
mod inspector;
mod layout;
mod settings;
mod spectrum;
mod station;
mod store;
mod theme;
mod version;
mod waterfall;

use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{self, Align2, Color32, CornerRadius, Frame, Key, Margin, RichText, Stroke};
use kahf_daemon::{Config, Engine, Event, TxState};

use inspector::Metrics;
use spectrum::Spectrum;
use store::Station;
use version::Version;
use waterfall::{Simulated, Waterfall};

const LOG_CAP: usize = 500;
/// Simulated spectrum rows are produced at this interval while no radio is running.
const SIM_PERIOD: Duration = Duration::from_millis(100);
/// The engine has no wake-up call, so its event queue is checked at the row rate while it runs.
const POLL_PERIOD: Duration = Duration::from_millis(100);
/// How often the outbox count is asked for. Not asked for while transmitting, so the UI never waits on it then.
const OUTBOX_PERIOD: Duration = Duration::from_secs(2);
/// How long the header shows Transmitting after a message, mail or file is queued or a frame is sent.
const TX_SHOW: Duration = Duration::from_secs(5);
/// How long the header shows Receiving after the last incoming item.
const RX_SHOW: Duration = Duration::from_secs(3);
/// Connection request: attempts, and the gap between them. Matches the engine's handshake timing
/// (1 s retransmit, 5 retries, so 6 attempts in all).
pub const CONNECT_ATTEMPTS: u32 = 6;
const CONNECT_GAP: Duration = Duration::from_secs(1);
const STOPPED_MSG: &str = "The radio engine has stopped. Restart KAHF to use the radio again.";
const UNKNOWN_STATION: &str = "Unknown station";
pub const NO_REPLY_MSG: &str =
    "No reply after 6 attempts. Check the frequency and the audio level, then try again.";
pub const LISTEN_ONLY_MSG: &str = "Listen only is on. Turn it off to transmit.";
const CANCELLED_MSG: &str = "Connection cancelled. Nothing was sent.";

/// One entry in a station's activity list. The list is kept in time order.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Chat {
        outgoing: bool,
        text: String,
        at: u64,
    },
    Mail {
        id: u64,
        outgoing: bool,
        subject: String,
        files: Vec<String>,
        /// Known for mail we sent. The engine does not pass on the text of mail we receive.
        body: Option<String>,
        at: u64,
    },
    File {
        id: u64,
        outgoing: bool,
        name: String,
        done: u64,
        total: u64,
        at: u64,
    },
}

/// What the radio is doing, for the header. Only these four words are shown.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Radio {
    Idle,
    Receiving,
    Transmitting,
    ListenOnly,
}

impl Radio {
    pub fn label(self) -> &'static str {
        match self {
            Radio::Idle => "Idle",
            Radio::Receiving => "Receiving",
            Radio::Transmitting => "Transmitting",
            Radio::ListenOnly => "Listen only",
        }
    }

    fn colour(self, accent: Color32) -> Color32 {
        match self {
            Radio::Idle | Radio::ListenOnly => theme::IDLE,
            Radio::Receiving => accent,
            Radio::Transmitting => theme::TRANSMIT,
        }
    }
}

/// Which file path the path row is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PathFor {
    Attach,
    SendFile,
}

#[derive(Default)]
pub struct Compose {
    pub text: String,
    /// True when the composer is in Mail mode.
    pub mail: bool,
    pub subject: String,
    pub body: String,
    pub attachments: Vec<PathBuf>,
    pub path_for: Option<PathFor>,
    pub path: String,
}

/// Draft values of the Settings panel. Applied with "Save".
pub struct SettingsForm {
    pub callsign: String,
    pub audio_in: String,
    pub audio_out: String,
    pub rigctld: String,
    pub sdr: String,
    pub data_dir: String,
    pub location_mode: settings::LocationMode,
    pub radius_km: u32,
}

impl SettingsForm {
    fn from_config(c: &Config) -> Self {
        SettingsForm {
            callsign: c.callsign.clone(),
            audio_in: c.audio_in.clone().unwrap_or_default(),
            audio_out: c.audio_out.clone().unwrap_or_default(),
            rigctld: c.rigctld.clone().unwrap_or_default(),
            sdr: c.sdr.clone().unwrap_or_default(),
            data_dir: c.data_dir.display().to_string(),
            location_mode: settings::LocationMode::Off,
            radius_km: settings::RADIUS_DEFAULT_KM,
        }
    }
}

pub struct App {
    pub engine: Option<Engine>,
    /// True once an engine call has returned Ok.
    pub live: bool,
    /// True after a successful engine call; enables polling of engine events.
    pub polling: bool,

    pub status: String,
    /// The settings in use.
    pub config: Config,
    pub settings: SettingsForm,
    /// Device names for the drop-downs. Empty until the audio layer can list devices.
    pub audio_inputs: Vec<String>,
    pub audio_outputs: Vec<String>,

    /// When on, no transmit path runs: connect, message, mail and file are all refused.
    pub listen_only: bool,
    /// Dark colour scheme when true. The light option is kept in gui.conf as theme=light.
    pub dark: bool,
    /// Accent and density of this build, chosen by KAHF_VARIANT at start-up.
    pub version: &'static Version,

    // Link
    pub peer_input: String,
    pub current_peer: Option<String>,
    /// "Idle", "Connecting", "Connected" or "Disconnecting".
    pub link_state: String,
    pub confirm_disconnect: bool,
    /// Station waiting for the Transmit and connect confirmation. Nothing is sent until it is accepted.
    pub pending_connect: Option<String>,
    /// When the confirmed connection request was sent. Used for the attempt count.
    pub connect_started: Option<Instant>,
    /// Plain-language reason the last connection gave up. Cleared on the next attempt or connect.
    pub connect_error: Option<String>,
    pub log: Vec<String>,
    pub messages_sent: u64,
    pub messages_received: u64,

    // Stations
    pub recent: Vec<Station>,
    /// The station whose conversation is shown in the messages panel.
    pub selected: Option<String>,
    pub activity: HashMap<String, Vec<Item>>,
    pub unread: HashMap<String, usize>,
    pub transfer_station: HashMap<u64, String>,
    pub outgoing_ids: HashSet<u64>,
    pub open_mail: HashSet<u64>,
    pub compose: Compose,
    /// Items waiting in the engine's outbox, refreshed on a timer.
    pub outbox: usize,
    outbox_checked: Instant,

    // Header activity
    pub rx_until: Option<Instant>,
    pub tx_until: Option<Instant>,
    /// True between the engine's Starting and Done transmit events.
    tx_active: bool,
    /// Unix seconds of the last transmit in this session.
    pub last_tx: Option<u64>,

    // Spectrum and waterfall
    pub spectrum: Spectrum,
    pub waterfall: Waterfall,
    pub sim: Simulated,
    /// Reused buffer for the test-pattern row.
    sim_row: Vec<f32>,
    pub last_sim: Instant,
    /// Rig, transmit and audio values from the engine. Empty until the engine reports them.
    pub metrics: Metrics,
}

/// The default data folder. gui.conf is always kept here.
pub fn default_data_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".kahf"))
        .unwrap_or_else(|| PathBuf::from(".kahf"))
}

/// Builds the engine without letting a panic escape into the UI.
fn open_engine(config: Config) -> Result<Engine, String> {
    match catch_unwind(AssertUnwindSafe(|| Engine::new(config))) {
        Ok(result) => result,
        Err(_) => Err("the radio engine could not start".to_string()),
    }
}

/// Turns an engine error into a sentence a radio operator can act on.
pub fn plain_error(raw: &str) -> String {
    let text = raw.to_lowercase();
    let msg = if text.contains("stopped") {
        STOPPED_MSG
    } else if text.contains("audio") || text.contains("device") {
        "Audio device not found. Choose one in Settings."
    } else if text.contains("rigctld") {
        "Cannot reach the rig control program. Check the rig control address in Settings."
    } else if text.contains("sdr") || text.contains("hackrf") {
        "SDR device not found. Check that it is plugged in, or change the SDR setting."
    } else if text.contains("data_dir") {
        "The data folder cannot be used. Choose another folder in Settings."
    } else if text.contains("destination") {
        "Choose a station first."
    } else if text.contains("empty message") {
        "Type a message first."
    } else if text.contains("could not start") {
        "The radio engine could not start. Check the setup in Settings."
    } else {
        "The radio could not do that. Check the setup in Settings."
    };
    msg.to_string()
}

/// Splits the engine's link-state text, for example `Connected { peer: "XX1XX" }`.
fn parse_link(debug: &str) -> (String, Option<String>) {
    let name: String = debug.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    let peer = debug
        .split("peer: \"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .map(store::normalise);
    (name, peer)
}

/// Frequency for the confirmation panel. Settings has a frequency only inside the SDR setting,
/// "hackrf@freq_hz". Sound-card use has none, so the radio's own frequency applies.
fn frequency_text(sdr: Option<&str>) -> String {
    let Some(spec) = sdr.map(str::trim).filter(|s| !s.is_empty()) else {
        return "Not set in KAHF. Your radio's current frequency is used.".to_string();
    };
    match spec.split_once('@').map(|(_, f)| f.trim().parse::<u64>()) {
        Some(Ok(hz)) => format!("{:.3} MHz (from the SDR setting)", hz as f64 / 1e6),
        Some(Err(_)) => "Frequency in the SDR setting is not a number".to_string(),
        None => "14.100 MHz (SDR default)".to_string(),
    }
}

impl App {
    fn new(version: &'static Version) -> Self {
        let conf_dir = default_data_dir();
        let conf = store::read_conf(&conf_dir);
        let text = |key: &str| conf.get(key).map(|v| v.trim().to_string()).unwrap_or_default();
        let opt = |key: &str| Some(text(key)).filter(|v| !v.is_empty());
        let data_dir = opt("data_dir")
            .map(PathBuf::from)
            .unwrap_or_else(|| conf_dir.clone());
        let config = Config {
            callsign: store::normalise(&text("callsign")),
            audio_in: opt("audio_in"),
            audio_out: opt("audio_out"),
            rigctld: opt("rigctld"),
            sdr: opt("sdr"),
            data_dir: data_dir.clone(),
        };
        let listen_only = conf.get("listen_only").is_some_and(|v| v.trim() == "true");
        let dark = conf.get("theme").is_none_or(|v| v.trim() != "light");
        let recent = store::load_stations(&data_dir);
        let mut settings_form = SettingsForm::from_config(&config);
        settings_form.location_mode = conf
            .get("location_mode")
            .and_then(|k| settings::LocationMode::from_key(k))
            .unwrap_or(settings::LocationMode::Off);
        settings_form.radius_km = conf
            .get("location_radius_km")
            .and_then(|v| v.parse::<u32>().ok())
            .map(|v| v.clamp(settings::RADIUS_MIN_KM, settings::RADIUS_MAX_KM))
            .unwrap_or(settings::RADIUS_DEFAULT_KM);
        let status = if config.callsign.is_empty() {
            "Welcome to KAHF. Enter your callsign in Settings to start."
        } else {
            "Ready."
        };
        App {
            engine: None,
            live: false,
            polling: false,
            status: status.to_string(),
            settings: settings_form,
            audio_inputs: Vec::new(),
            audio_outputs: Vec::new(),
            config,
            listen_only,
            dark,
            version,
            peer_input: String::new(),
            current_peer: None,
            link_state: "Idle".to_string(),
            confirm_disconnect: false,
            pending_connect: None,
            connect_started: None,
            connect_error: None,
            log: Vec::new(),
            messages_sent: 0,
            messages_received: 0,
            recent,
            selected: None,
            activity: HashMap::new(),
            unread: HashMap::new(),
            transfer_station: HashMap::new(),
            outgoing_ids: HashSet::new(),
            open_mail: HashSet::new(),
            compose: Compose::default(),
            outbox: 0,
            outbox_checked: Instant::now(),
            rx_until: None,
            tx_until: None,
            tx_active: false,
            last_tx: None,
            spectrum: Spectrum::default(),
            waterfall: Waterfall::new(),
            sim: Simulated::new(),
            sim_row: Vec::with_capacity(256),
            last_sim: Instant::now(),
            metrics: Metrics::default(),
        }
    }

    /// Starts the engine if it is not running. The error is already in plain words.
    fn ensure_engine(&mut self) -> Result<(), String> {
        if self.engine.is_some() {
            return Ok(());
        }
        if !store::valid_callsign(&self.config.callsign) {
            return Err("Set your callsign in Settings first.".to_string());
        }
        match open_engine(self.config.clone()) {
            Ok(engine) => {
                self.engine = Some(engine);
                Ok(())
            }
            Err(raw) => {
                self.log_line(format!("Error: {raw}"));
                Err(plain_error(&raw))
            }
        }
    }

    /// Runs one engine call, starting the engine first if needed. Errors come back in plain words.
    pub fn radio<T>(&mut self, f: impl FnOnce(&mut Engine) -> Result<T, String>) -> Result<T, String> {
        self.ensure_engine()?;
        let Some(engine) = self.engine.as_mut() else {
            return Err(plain_error("engine missing"));
        };
        match catch_unwind(AssertUnwindSafe(|| f(engine))) {
            Ok(Ok(value)) => {
                self.live = true;
                self.polling = true;
                Ok(value)
            }
            Ok(Err(raw)) => {
                self.log_line(format!("Error: {raw}"));
                if raw.contains("stopped") {
                    self.drop_engine();
                }
                Err(plain_error(&raw))
            }
            Err(_) => {
                self.drop_engine();
                Err(STOPPED_MSG.to_string())
            }
        }
    }

    /// Forgets the engine. The next Connect or Send starts a new one.
    pub fn drop_engine(&mut self) {
        self.engine = None;
        self.live = false;
        self.polling = false;
        self.outbox = 0;
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if !self.polling {
            return;
        }
        if let Ok(events) = self.radio(|e| Ok(e.poll_events())) {
            for ev in events {
                self.apply(ev, ctx);
            }
        }
    }

    /// Feeds the spectrum and waterfall from the test pattern while no radio is running.
    fn feed_simulated(&mut self, ctx: &egui::Context) {
        if self.polling || self.last_sim.elapsed() < SIM_PERIOD {
            return;
        }
        self.last_sim = Instant::now();
        let mut row = std::mem::take(&mut self.sim_row);
        self.sim.next_row(&mut row);
        self.push_spectrum(ctx, &row);
        self.sim_row = row;
    }

    /// One spectrum row, from the engine or the test pattern. Rows arrive about 10 times a second.
    pub fn push_spectrum(&mut self, ctx: &egui::Context, values: &[f32]) {
        self.spectrum.push_row(values);
        self.waterfall.push_row(ctx, values);
    }

    fn apply(&mut self, ev: Event, ctx: &egui::Context) {
        match ev {
            Event::Link(state) => self.on_link(&format!("{state:?}")),
            Event::Message { from, text } => {
                self.messages_received += 1;
                let call = store::normalise(&from);
                self.receive(
                    &call,
                    Item::Chat {
                        outgoing: false,
                        text,
                        at: store::now(),
                    },
                    true,
                );
            }
            Event::Mail { id, from, subject } => {
                let call = store::normalise(&from);
                let item = Item::Mail {
                    id,
                    outgoing: false,
                    subject,
                    files: Vec::new(),
                    body: None,
                    at: store::now(),
                };
                self.receive(&call, item, true);
            }
            Event::FileProgress {
                id,
                name,
                done,
                total,
            } => self.file_progress(id, name, done, total),
            Event::Spectrum(values) => self.push_spectrum(ctx, &values),
            Event::Tx { state } => self.on_tx(state),
            Event::Log(line) => self.log_line(line),
        }
    }

    /// Transmit progress from the engine. Starting and Sending mean Transmitting; Done ends it.
    fn on_tx(&mut self, state: TxState) {
        match state {
            TxState::Starting | TxState::Sending => {
                self.tx_active = true;
                self.last_tx = Some(store::now());
            }
            TxState::Done => {
                self.tx_active = false;
                self.tx_until = None;
            }
            TxState::Failed(reason) => {
                self.tx_active = false;
                self.tx_until = None;
                self.status = plain_error(&reason);
                self.log_line(format!("Transmit failed: {reason}"));
            }
        }
    }

    fn on_link(&mut self, debug: &str) {
        let (name, peer) = parse_link(debug);
        match name.as_str() {
            "Connected" => {
                self.connect_started = None;
                self.connect_error = None;
                if let Some(p) = peer {
                    self.touch_station(&p, true, false);
                    self.status = format!("Connected to {p}.");
                    self.log_line(format!("Connected to {p}"));
                    self.current_peer = Some(p);
                }
                self.link_state = "Connected".to_string();
            }
            "Connecting" => {
                if let Some(p) = peer {
                    self.current_peer = Some(p);
                }
                self.link_state = "Connecting".to_string();
            }
            "Disconnecting" => self.link_state = "Disconnecting".to_string(),
            "Idle" => {
                // Idle while a request was pending means the engine gave up on the handshake.
                let gave_up = self.connect_started.take().is_some();
                let peer = self.current_peer.take();
                if gave_up {
                    self.connect_error = Some(NO_REPLY_MSG.to_string());
                    self.status = NO_REPLY_MSG.to_string();
                    self.log_line("No reply to the connection request");
                } else if let Some(p) = peer {
                    self.status = format!("Disconnected from {p}.");
                    self.log_line(format!("Disconnected from {p}"));
                }
                self.link_state = "Idle".to_string();
            }
            _ => {}
        }
    }

    /// Adds an incoming item to a station's list and counts it as unread unless it is on screen.
    fn receive(&mut self, call: &str, item: Item, is_msg: bool) {
        self.rx_until = Some(Instant::now() + RX_SHOW);
        let viewing = self.selected.as_deref() == Some(call);
        self.push_item(call, item);
        if !viewing {
            *self.unread.entry(call.to_string()).or_insert(0) += 1;
        }
        self.touch_station(call, true, is_msg);
        self.log_line(format!("Received from {call}"));
    }

    fn file_progress(&mut self, id: u64, name: String, done: u64, total: u64) {
        self.rx_until = Some(Instant::now() + RX_SHOW);
        let call = match self.transfer_station.get(&id) {
            Some(call) => call.clone(),
            None => {
                let call = self
                    .current_peer
                    .clone()
                    .unwrap_or_else(|| UNKNOWN_STATION.to_string());
                self.transfer_station.insert(id, call.clone());
                call
            }
        };
        let outgoing = self.outgoing_ids.contains(&id);
        let viewing = self.selected.as_deref() == Some(call.as_str());
        let list = self.activity.entry(call.clone()).or_default();
        let existing = list
            .iter()
            .position(|it| matches!(it, Item::File { id: fid, .. } if *fid == id));
        match existing {
            Some(i) => {
                if let Item::File {
                    name: n,
                    done: d,
                    total: t,
                    ..
                } = &mut list[i]
                {
                    *n = name;
                    *d = done;
                    *t = total;
                }
            }
            None => {
                list.push(Item::File {
                    id,
                    outgoing,
                    name,
                    done,
                    total,
                    at: store::now(),
                });
                if !outgoing && !viewing {
                    *self.unread.entry(call.clone()).or_insert(0) += 1;
                }
                self.touch_station(&call, true, false);
            }
        }
    }

    pub fn push_item(&mut self, call: &str, item: Item) {
        self.activity.entry(call.to_string()).or_default().push(item);
    }

    pub fn log_line(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
        if self.log.len() > LOG_CAP {
            let excess = self.log.len() - LOG_CAP;
            self.log.drain(..excess);
        }
    }

    /// Records a sign of a station in the recent list and saves the file.
    pub fn touch_station(&mut self, call: &str, seen: bool, msg: bool) {
        store::touch(&mut self.recent, call, seen, msg);
        if store::save_stations(&self.config.data_dir, &self.recent).is_err() {
            self.status =
                "Could not save the recent stations list. Check the data folder in Settings.".to_string();
        }
    }

    /// Shows the reason and stops if Listen only is on. Returns true when a transmit must not run.
    pub fn refuse_transmit(&mut self) -> bool {
        if self.listen_only {
            self.status = LISTEN_ONLY_MSG.to_string();
            true
        } else {
            false
        }
    }

    /// Connect was pressed. Nothing is sent here: the confirmation panel asks first.
    pub fn ask_connect(&mut self, call: String) {
        if self.refuse_transmit() {
            return;
        }
        if !store::valid_callsign(&self.config.callsign) {
            self.status = "Set your callsign in Settings before connecting.".to_string();
            return;
        }
        if !store::valid_callsign(&call) {
            self.status = "Type a station callsign first. Use 3 to 10 letters or digits.".to_string();
            return;
        }
        if call == self.config.callsign {
            self.status = "That is your own callsign. Type the station you want to reach.".to_string();
            return;
        }
        self.connect_error = None;
        self.pending_connect = Some(call);
    }

    /// Starts a connection to a station. Called only when the confirmation panel is accepted.
    pub fn connect_to(&mut self, call: String) {
        if self.refuse_transmit() {
            return;
        }
        if !store::valid_callsign(&self.config.callsign) {
            self.status = "Set your callsign in Settings before connecting.".to_string();
            return;
        }
        match self.radio(|e| e.connect(&call)) {
            Ok(()) => {
                self.current_peer = Some(call.clone());
                self.selected = Some(call.clone());
                self.link_state = "Connecting".to_string();
                self.connect_started = Some(Instant::now());
                self.connect_error = None;
                self.touch_station(&call, true, false);
                self.mark_transmit(CONNECT_GAP * CONNECT_ATTEMPTS);
                self.status = format!("Connecting to {call}.");
            }
            Err(e) => {
                self.connect_error = Some(e.clone());
                self.status = e;
            }
        }
    }

    pub fn disconnect(&mut self) {
        self.confirm_disconnect = false;
        self.connect_started = None;
        match self.radio(|e| e.disconnect()) {
            Ok(()) => {
                self.link_state = "Disconnecting".to_string();
                self.mark_sending();
                self.status = "Disconnecting.".to_string();
            }
            Err(e) => self.status = e,
        }
    }

    /// Shows a station's conversation in the messages panel and clears its unread count.
    pub fn open_station(&mut self, call: &str) {
        self.selected = Some(call.to_string());
        self.unread.remove(call);
    }

    /// Marks a queued transmit: the header shows Transmitting for a short time and the time is kept.
    pub fn mark_sending(&mut self) {
        self.mark_transmit(TX_SHOW);
    }

    fn mark_transmit(&mut self, show: Duration) {
        self.tx_until = Some(Instant::now() + show);
        self.last_tx = Some(store::now());
    }

    /// Turns Listen only on or off and keeps the choice in gui.conf.
    pub fn set_listen_only(&mut self, on: bool) {
        self.listen_only = on;
        if on {
            self.pending_connect = None;
        }
        let dir = default_data_dir();
        let mut conf = store::read_conf(&dir);
        conf.insert("listen_only".to_string(), on.to_string());
        self.status = if store::write_conf(&dir, &conf).is_err() {
            "Could not save Listen only. Check that the KAHF folder can be written.".to_string()
        } else if on {
            "Listen only is on. Nothing will be transmitted.".to_string()
        } else {
            "Listen only is off. You can transmit again.".to_string()
        };
    }

    /// Switches the colour scheme and keeps the choice in gui.conf.
    pub fn set_dark(&mut self, dark: bool, ctx: &egui::Context) {
        self.dark = dark;
        theme::apply(ctx, dark);
        let dir = default_data_dir();
        let mut conf = store::read_conf(&dir);
        conf.insert(
            "theme".to_string(),
            if dark { "dark" } else { "light" }.to_string(),
        );
        if store::write_conf(&dir, &conf).is_err() {
            self.status =
                "Could not save the colour choice. Check that the KAHF folder can be written.".to_string();
        }
    }

    /// True while a link is up or being set up.
    pub fn link_active(&self) -> bool {
        matches!(
            self.link_state.as_str(),
            "Connecting" | "Connected" | "Disconnecting"
        )
    }

    /// Attempt the connection request is on, counted from when it was confirmed. 1 when not connecting.
    pub fn connect_attempt(&self) -> u32 {
        let secs = self
            .connect_started
            .map(|t| t.elapsed().as_secs_f32())
            .unwrap_or(0.0);
        ((secs / CONNECT_GAP.as_secs_f32()) as u32 + 1).min(CONNECT_ATTEMPTS)
    }

    /// Plain-language link line for the connect card.
    pub fn link_line(&self) -> String {
        match (self.link_state.as_str(), self.current_peer.as_deref()) {
            ("Connected", Some(p)) => format!("Connected to {p}."),
            ("Connecting", Some(p)) => format!("Connecting to {p}. Waiting for the other station."),
            ("Disconnecting", _) => "Disconnecting.".to_string(),
            _ => "Not connected.".to_string(),
        }
    }

    /// Rig frequency when the engine reports it, else the frequency in the SDR setting.
    pub fn freq_hz(&self) -> Option<u64> {
        self.metrics.rig_freq_hz.or_else(|| {
            let spec = self.config.sdr.as_deref()?;
            spec.split_once('@').and_then(|(_, f)| f.trim().parse().ok())
        })
    }

    /// Short text for the link chip in the top bar.
    fn link_chip(&self) -> String {
        match (self.link_state.as_str(), self.current_peer.as_deref()) {
            ("Connected", Some(p)) => format!("Connected: {p}"),
            ("Connecting", Some(p)) => format!("Connecting: {p}"),
            ("Disconnecting", _) => "Disconnecting".to_string(),
            _ => "Not connected".to_string(),
        }
    }

    /// Header state: Listen only while it is on, otherwise Transmitting or Receiving for a short time
    /// after activity, otherwise Idle.
    fn radio_state(&self) -> Radio {
        if self.listen_only {
            return Radio::ListenOnly;
        }
        let now = Instant::now();
        if self.tx_active || self.tx_until.is_some_and(|t| t > now) {
            return Radio::Transmitting;
        }
        if self.rx_until.is_some_and(|t| t > now) {
            return Radio::Receiving;
        }
        Radio::Idle
    }

    /// Note shown on the spectrum and waterfall while no radio is running.
    fn sim_note(&self) -> &'static str {
        if self.polling {
            ""
        } else {
            "No radio running. Showing a test pattern."
        }
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            ui.label(&self.status);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let radio = if self.live {
                    "Radio: running"
                } else {
                    "Radio: not started"
                };
                ui.label(RichText::new(radio).weak());
            });
        });
    }

    /// Ctrl+1 is gone with the sections. Esc cancels a pending connect, or asks before disconnecting.
    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            if self.pending_connect.is_some() {
                self.pending_connect = None;
                self.status = CANCELLED_MSG.to_string();
            } else if self.link_active() {
                self.confirm_disconnect = !self.confirm_disconnect;
            }
        }
    }

    /// Confirmation before any connection request is sent. Only "Transmit and connect" calls connect_to.
    fn connect_prompt(&mut self, ctx: &egui::Context) {
        let Some(call) = self.pending_connect.clone() else {
            return;
        };
        if self.listen_only || self.link_active() {
            self.pending_connect = None;
            return;
        }
        let own = self.config.callsign.clone();
        let freq = frequency_text(self.config.sdr.as_deref());
        let mut confirm = false;
        let mut cancel = false;
        egui::Window::new("Transmit and connect?")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Grid::new("connect_confirm")
                    .num_columns(2)
                    .spacing([theme::UNIT * 2.0, theme::UNIT])
                    .show(ui, |ui| {
                        detail_row(ui, "Callsign sent", &own);
                        detail_row(ui, "Station", &call);
                        detail_row(ui, "Frequency", &freq);
                        detail_row(ui, "Mode", "QPSK");
                    });
                ui.add_space(theme::UNIT);
                ui.label("Your radio will transmit a connection request now.");
                ui.label("If there is no reply, KAHF gives up after about 6 seconds.");
                ui.add_space(theme::UNIT);
                ui.horizontal(|ui| {
                    if ui.button("Transmit and connect").clicked() {
                        confirm = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if confirm {
            self.pending_connect = None;
            self.connect_to(call);
        } else if cancel {
            self.pending_connect = None;
            self.status = CANCELLED_MSG.to_string();
        }
    }

    fn disconnect_prompt(&mut self, ctx: &egui::Context) {
        if !self.confirm_disconnect {
            return;
        }
        if !self.link_active() {
            self.confirm_disconnect = false;
            return;
        }
        let peer = self
            .current_peer
            .clone()
            .unwrap_or_else(|| "the station".to_string());
        egui::Window::new("Disconnect?")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("Disconnect from {peer} now?"));
                ui.horizontal(|ui| {
                    if ui.button("Disconnect").clicked() {
                        self.disconnect();
                    }
                    if ui.button("Stay connected").clicked() {
                        self.confirm_disconnect = false;
                    }
                });
            });
    }

    /// Asks the engine for its outbox count once every OUTBOX_PERIOD. Skipped while transmitting.
    fn refresh_outbox(&mut self) {
        if self.outbox_checked.elapsed() < OUTBOX_PERIOD || self.radio_state() == Radio::Transmitting {
            return;
        }
        self.outbox_checked = Instant::now();
        if let Some(engine) = &self.engine {
            self.outbox = engine.outbox_pending().len();
        }
    }

    /// Asks for a new frame only when something changes on a timer: the engine queue, the test
    /// pattern, a header state running out, a connection attempt, or the outbox count.
    fn schedule_repaint(&self, ctx: &egui::Context) {
        let now = Instant::now();
        let until = |t: Option<Instant>| t.filter(|t| *t > now).map(|t| t - now);
        let candidates = [
            self.polling.then_some(POLL_PERIOD),
            (!self.polling).then_some(SIM_PERIOD),
            until(self.tx_until),
            until(self.rx_until),
            self.connect_started.map(|_| CONNECT_GAP),
            self.engine.as_ref().map(|_| OUTBOX_PERIOD),
        ];
        let next = candidates.into_iter().flatten().min();
        if let Some(delay) = next {
            ctx.request_repaint_after(delay);
        }
    }
}

/// A label in a hairline frame with 4 px corners, used for the link state in the top bar.
fn chip(ui: &mut egui::Ui, text: &str, border: Color32, colour: Color32) {
    Frame::NONE
        .stroke(Stroke::new(1.0, border))
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(Margin::symmetric(theme::UNIT as i8, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).color(colour));
        });
}

pub fn detail_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(label);
    ui.label(RichText::new(value).strong());
    ui.end_row();
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll(ctx);
        self.feed_simulated(ctx);
        self.refresh_outbox();
        self.schedule_repaint(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_keys(&ctx);
        self.connect_prompt(&ctx);
        self.disconnect_prompt(&ctx);
        layout::frame(self, ui);
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("KAHF")
            .with_inner_size([1512.0, 940.0]),
        ..Default::default()
    };
    eframe::run_native(
        "KAHF",
        options,
        Box::new(|cc| {
            let app = App::new(Version::from_env());
            theme::install(&cc.egui_ctx, app.version);
            theme::apply(&cc.egui_ctx, app.dark);
            Ok(Box::new(app) as Box<dyn eframe::App>)
        }),
    )
}
