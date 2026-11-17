//! KAHF desktop GUI. Only the public API of kahf-daemon (Config, Engine, Event) is used.
//! Engine calls are guarded: a panic or error shows "engine not connected" and the UI keeps running.
mod sections;
mod theme;

use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::time::Duration;

use eframe::egui;
use kahf_daemon::{Config, Engine, Event};

pub const NOT_CONNECTED: &str = "engine not connected";
const LOG_CAP: usize = 500;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    Connect,
    Messages,
    Mail,
    Files,
    Spectrum,
    Settings,
}

impl Section {
    pub const ALL: [Section; 6] = [
        Section::Connect,
        Section::Messages,
        Section::Mail,
        Section::Files,
        Section::Spectrum,
        Section::Settings,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Section::Connect => "Connect",
            Section::Messages => "Messages",
            Section::Mail => "Mail",
            Section::Files => "Files",
            Section::Spectrum => "Spectrum",
            Section::Settings => "Settings",
        }
    }
}

pub struct ChatLine {
    pub outgoing: bool,
    pub text: String,
}

pub struct MailItem {
    pub id: u64,
    pub from: String,
    pub subject: String,
}

pub struct Transfer {
    pub id: u64,
    pub name: String,
    pub done: u64,
    pub total: u64,
    pub outgoing: bool,
}

/// Draft values of the Settings section; applied with the Apply button.
pub struct SettingsForm {
    pub callsign: String,
    pub audio_in: String,
    pub audio_out: String,
    pub rigctld: String,
    pub sdr: String,
    pub data_dir: String,
}

#[derive(Default)]
pub struct Compose {
    pub to: String,
    pub subject: String,
    pub body: String,
    pub attachments: Vec<PathBuf>,
    pub attach_path: String,
}

pub struct App {
    pub engine: Option<Engine>,
    /// True once an engine call has returned Ok; drives the engine-state label.
    pub live: bool,
    /// True after a successful user action; enables polling of engine events.
    pub polling: bool,

    pub section: Section,
    pub status: String,
    pub config: Config,
    pub settings: SettingsForm,

    // Connect
    pub peer: String,
    pub current_peer: Option<String>,
    pub link_state: String,
    pub log: Vec<String>,

    // Messages
    pub conversations: Vec<String>,
    pub chats: HashMap<String, Vec<ChatLine>>,
    pub selected_conv: Option<String>,
    pub chat_input: String,

    // Mail
    pub mail: Vec<MailItem>,
    pub selected_mail: Option<u64>,
    pub composing: bool,
    pub compose: Compose,

    // Files
    pub transfers: Vec<Transfer>,
    pub outgoing_ids: HashSet<u64>,
    pub send_file_to: String,
    pub send_file_path: String,

    // Spectrum
    pub spectrum: Vec<f32>,

    // Status bar placeholders: no engine event carries these yet.
    pub audio_level: Option<f32>,
    pub snr_db: Option<f32>,
}

fn default_data_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".kahf"))
        .unwrap_or_else(|| PathBuf::from(".kahf"))
}

/// Builds the engine without letting a panic escape into the UI.
fn open_engine(config: Config) -> Result<Engine, String> {
    match catch_unwind(AssertUnwindSafe(|| Engine::new(config))) {
        Ok(result) => result,
        Err(_) => Err("engine constructor panicked".to_string()),
    }
}

impl App {
    fn new() -> Self {
        let config = Config {
            data_dir: default_data_dir(),
            ..Default::default()
        };
        let (engine, status) = match open_engine(config.clone()) {
            Ok(e) => (Some(e), format!("{NOT_CONNECTED}: no engine call has succeeded yet")),
            Err(e) => (None, format!("{NOT_CONNECTED}: {e}")),
        };
        let settings = SettingsForm {
            callsign: config.callsign.clone(),
            audio_in: String::new(),
            audio_out: String::new(),
            rigctld: String::new(),
            sdr: String::new(),
            data_dir: config.data_dir.display().to_string(),
        };
        App {
            engine,
            live: false,
            polling: false,
            section: Section::Connect,
            status,
            config,
            settings,
            peer: String::new(),
            current_peer: None,
            link_state: "Idle".to_string(),
            log: Vec::new(),
            conversations: Vec::new(),
            chats: HashMap::new(),
            selected_conv: None,
            chat_input: String::new(),
            mail: Vec::new(),
            selected_mail: None,
            composing: false,
            compose: Compose::default(),
            transfers: Vec::new(),
            outgoing_ids: HashSet::new(),
            send_file_to: String::new(),
            send_file_path: String::new(),
            spectrum: Vec::new(),
            audio_level: None,
            snr_db: None,
        }
    }

    /// Runs one engine method. Panics and missing engines become an error string.
    /// A successful call marks the engine live and turns on event polling.
    pub fn engine_call<T>(
        &mut self,
        f: impl FnOnce(&mut Engine) -> Result<T, String>,
    ) -> Result<T, String> {
        let Some(engine) = self.engine.as_mut() else {
            return Err(NOT_CONNECTED.to_string());
        };
        match catch_unwind(AssertUnwindSafe(|| f(engine))) {
            Ok(Ok(value)) => {
                self.live = true;
                self.polling = true;
                Ok(value)
            }
            Ok(Err(e)) => Err(e),
            Err(_) => {
                self.live = false;
                self.polling = false;
                Err(NOT_CONNECTED.to_string())
            }
        }
    }

    fn poll(&mut self) {
        if !self.polling {
            return;
        }
        if let Ok(events) = self.engine_call(|e| Ok(e.poll_events())) {
            for ev in events {
                self.apply(ev);
            }
        }
    }

    fn apply(&mut self, ev: Event) {
        match ev {
            Event::Link(state) => {
                // LinkState is not named here (only Config, Engine and Event are used),
                // so the state name is taken from its Debug form.
                let name: String = format!("{state:?}")
                    .chars()
                    .take_while(|c| c.is_ascii_alphabetic())
                    .collect();
                if name == "Idle" {
                    self.current_peer = None;
                }
                self.link_state = name;
            }
            Event::Message { from, text } => {
                self.push_chat(from, ChatLine { outgoing: false, text });
            }
            Event::Mail { id, from, subject } => {
                self.mail.insert(0, MailItem { id, from, subject });
            }
            Event::FileProgress { id, name, done, total } => {
                if let Some(t) = self.transfers.iter_mut().find(|t| t.id == id) {
                    t.name = name;
                    t.done = done;
                    t.total = total;
                } else {
                    let outgoing = self.outgoing_ids.contains(&id);
                    self.transfers.push(Transfer { id, name, done, total, outgoing });
                }
            }
            Event::Spectrum(values) => self.spectrum = values,
            Event::Log(line) => {
                self.log.push(line);
                if self.log.len() > LOG_CAP {
                    let excess = self.log.len() - LOG_CAP;
                    self.log.drain(..excess);
                }
            }
        }
    }

    pub fn push_chat(&mut self, peer: String, line: ChatLine) {
        if !self.chats.contains_key(&peer) {
            self.conversations.push(peer.clone());
        }
        if self.selected_conv.is_none() {
            self.selected_conv = Some(peer.clone());
        }
        self.chats.entry(peer).or_default().push(line);
    }

    /// Builds a Config from the Settings draft (empty fields become None).
    pub fn config_from_form(&self) -> Config {
        let opt = |s: &str| {
            let t = s.trim();
            (!t.is_empty()).then(|| t.to_string())
        };
        Config {
            callsign: self.settings.callsign.trim().to_string(),
            audio_in: opt(&self.settings.audio_in),
            audio_out: opt(&self.settings.audio_out),
            rigctld: opt(&self.settings.rigctld),
            sdr: opt(&self.settings.sdr),
            data_dir: PathBuf::from(self.settings.data_dir.trim()),
        }
    }

    /// Recreates the engine from the Settings draft.
    pub fn apply_settings(&mut self) {
        let config = self.config_from_form();
        match open_engine(config.clone()) {
            Ok(engine) => {
                self.engine = Some(engine);
                self.config = config;
                self.live = false;
                self.polling = false;
                self.status = format!("{NOT_CONNECTED}: settings applied");
            }
            Err(e) => {
                self.engine = None;
                self.live = false;
                self.polling = false;
                self.status = format!("{NOT_CONNECTED}: {e}");
            }
        }
    }

    fn engine_state_label(&self) -> &'static str {
        if self.live {
            "engine running"
        } else {
            NOT_CONNECTED
        }
    }

    fn top_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("KAHF");
            ui.separator();
            let callsign = if self.config.callsign.is_empty() {
                "no callsign set"
            } else {
                self.config.callsign.as_str()
            };
            ui.label(callsign);
            ui.separator();
            ui.label(format!("Link: {}", self.link_state));
            ui.separator();
            ui.label(format!("Peer: {}", self.current_peer.as_deref().unwrap_or("none")));
        });
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Audio");
            match self.audio_level {
                Some(v) => {
                    ui.add(egui::ProgressBar::new(v.clamp(0.0, 1.0)).desired_width(120.0));
                }
                None => {
                    ui.add(egui::ProgressBar::new(0.0).desired_width(120.0).text("n/a"));
                }
            }
            ui.separator();
            let snr = self
                .snr_db
                .map_or_else(|| "--".to_string(), |v| format!("{v:.1} dB"));
            ui.label(format!("SNR {snr}"));
            ui.separator();
            ui.label(&self.status);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(self.engine_state_label());
            });
        });
    }

    fn nav_rail(&mut self, ui: &mut egui::Ui) {
        ui.add_space(theme::UNIT);
        for section in Section::ALL {
            ui.selectable_value(&mut self.section, section, section.label());
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
        if self.polling {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("top_bar").show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status_bar").show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("nav_rail")
            .resizable(false)
            .default_size(152.0)
            .show(ui, |ui| self.nav_rail(ui));
        egui::CentralPanel::default().show(ui, |ui| match self.section {
            Section::Connect => self.connect_section(ui),
            Section::Messages => self.messages_section(ui),
            Section::Mail => self.mail_section(ui),
            Section::Files => self.files_section(ui),
            Section::Spectrum => self.spectrum_section(ui),
            Section::Settings => self.settings_section(ui),
        });
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("KAHF")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([960.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "KAHF",
        options,
        Box::new(|cc| {
            theme::install(&cc.egui_ctx);
            Ok(Box::new(App::new()) as Box<dyn eframe::App>)
        }),
    )
}
