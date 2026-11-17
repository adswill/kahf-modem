//! Settings: one dense page in the left sidebar, every setting in order under section headings, with one
//! Save button.
use std::collections::BTreeMap;
use std::path::PathBuf;

use eframe::egui::{self, Button, ComboBox, RichText, TextEdit};
use kahf_daemon::Config;

use crate::{default_data_dir, store, theme, App};

fn optional(text: &str) -> Option<String> {
    let t = text.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Sets or removes one key of the settings file.
fn put(conf: &mut BTreeMap<String, String>, key: &str, value: &Option<String>) {
    match value {
        Some(v) => {
            conf.insert(key.to_string(), v.clone());
        }
        None => {
            conf.remove(key);
        }
    }
}

/// How much of the operator's position may be shared. Off is the default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocationMode {
    Off,
    Precise,
    Area,
}

impl LocationMode {
    pub const ALL: [LocationMode; 3] = [LocationMode::Off, LocationMode::Precise, LocationMode::Area];

    pub fn label(self) -> &'static str {
        match self {
            LocationMode::Off => "Off",
            LocationMode::Precise => "Precise (about 200 m)",
            LocationMode::Area => "Area",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            LocationMode::Off => "off",
            LocationMode::Precise => "precise",
            LocationMode::Area => "area",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        LocationMode::ALL.into_iter().find(|m| m.key() == key)
    }
}

/// Radius of the area mode, in km. The slider range and the default.
pub const RADIUS_MIN_KM: u32 = 1;
pub const RADIUS_MAX_KM: u32 = 100;
pub const RADIUS_DEFAULT_KM: u32 = 50;

/// Drop-down for an audio device. The first entry is the system default, stored as an empty name.
fn device_picker(ui: &mut egui::Ui, id: &str, value: &mut String, devices: &[String]) {
    let shown = if value.is_empty() {
        "System default".to_string()
    } else {
        value.clone()
    };
    ComboBox::from_id_salt(id)
        .selected_text(shown)
        .width((ui.available_width() - 80.0).max(120.0))
        .show_ui(ui, |ui| {
            ui.selectable_value(value, String::new(), "System default");
            for device in devices {
                ui.selectable_value(value, device.clone(), device);
            }
        });
}

impl App {
    /// Every setting, top to bottom. Shown inside the right column's Settings section.
    pub fn settings_body(&mut self, ui: &mut egui::Ui) {
        if self.config.callsign.is_empty() {
            ui.label(
                RichText::new("Enter your callsign to start. The other settings can stay as they are.")
                    .weak(),
            );
        }
        let inputs = self.audio_inputs.clone();
        let outputs = self.audio_outputs.clone();

        theme::section_heading(ui, "Station");
        ui.label("Callsign");
        ui.add(
            TextEdit::singleline(&mut self.settings.callsign)
                .desired_width(f32::INFINITY)
                .hint_text("e.g. XX1XX"),
        );
        let callsign = store::normalise(&self.settings.callsign);
        if !callsign.is_empty() && !store::valid_callsign(&callsign) {
            ui.label(RichText::new("Use 3 to 10 letters or digits, with no spaces.").color(theme::WARN));
        }
        ui.add_space(theme::UNIT);

        theme::section_heading(ui, "Audio");
        ui.label("Audio input");
        ui.horizontal(|ui| {
            device_picker(ui, "audio_in", &mut self.settings.audio_in, &inputs);
            if ui.button("Refresh").clicked() {
                self.refresh_devices();
            }
        });
        ui.label("Audio output");
        device_picker(ui, "audio_out", &mut self.settings.audio_out, &outputs);
        ui.add_space(theme::UNIT);

        theme::section_heading(ui, "Rig");
        ui.label("Rig control address");
        ui.add(
            TextEdit::singleline(&mut self.settings.rigctld)
                .desired_width(f32::INFINITY)
                .hint_text("127.0.0.1:4532 (optional)"),
        );
        let rig = self.settings.rigctld.trim().to_string();
        if !rig.is_empty() && !store::valid_rigctld(&rig) {
            ui.label(RichText::new("Use host:port, for example 127.0.0.1:4532.").color(theme::WARN));
        }
        ui.add_space(theme::UNIT);

        ui.label("SDR setting");
        ui.add(
            TextEdit::singleline(&mut self.settings.sdr)
                .desired_width(f32::INFINITY)
                .hint_text("Empty uses the sound card"),
        );
        ui.add_space(theme::UNIT);

        theme::section_heading(ui, "Storage");
        ui.label("Data folder");
        ui.add(
            TextEdit::singleline(&mut self.settings.data_dir)
                .desired_width(f32::INFINITY)
                .hint_text("Recent stations and received files"),
        );
        ui.add_space(theme::UNIT);

        theme::section_heading(ui, "Privacy");
        ui.label("Location sharing");
        ui.horizontal(|ui| {
            for choice in LocationMode::ALL {
                ui.radio_value(&mut self.settings.location_mode, choice, choice.label());
            }
        });
        match self.settings.location_mode {
            LocationMode::Off => {
                ui.label(RichText::new("Your position is not shared.").weak());
            }
            LocationMode::Precise => {
                ui.label(
                    RichText::new("Anyone who can see the map can see your position to about 200 m.")
                        .color(theme::WARN),
                );
            }
            LocationMode::Area => {
                ui.add(
                    egui::Slider::new(&mut self.settings.radius_km, RADIUS_MIN_KM..=RADIUS_MAX_KM)
                        .suffix(" km")
                        .text("Radius"),
                );
                ui.label(
                    RichText::new("The map shows a circle this size. Your position is somewhere inside it.")
                        .weak(),
                );
            }
        }
        ui.add_space(theme::UNIT);

        theme::section_heading(ui, "Display");
        ui.label("Theme");
        let mut dark = self.dark;
        ui.horizontal(|ui| {
            ui.radio_value(&mut dark, true, "Dark");
            ui.radio_value(&mut dark, false, "Light");
        });
        if dark != self.dark {
            let ctx = ui.ctx().clone();
            self.set_dark(dark, &ctx);
        }
        ui.add_space(theme::UNIT);

        theme::section_heading(ui, "Transmit");
        let mut listen = self.listen_only;
        if ui.checkbox(&mut listen, "Listen only: never transmit").changed() {
            self.set_listen_only(listen);
        }
        ui.label(RichText::new("Connect, messages, mail and files are turned off.").weak());
        ui.add_space(theme::UNIT);

        if ui.add(Button::new("Save")).clicked() {
            self.save_settings();
        }
    }

    /// Number of settings that need attention: a missing or invalid callsign, a bad rig address.
    pub fn settings_attention(&self) -> usize {
        let mut n = 0;
        if !store::valid_callsign(&store::normalise(&self.settings.callsign)) {
            n += 1;
        }
        let rig = self.settings.rigctld.trim();
        if !rig.is_empty() && !store::valid_rigctld(rig) {
            n += 1;
        }
        n
    }

    /// Asks the audio layer for devices. It cannot list them yet, so the lists stay empty.
    fn refresh_devices(&mut self) {
        self.audio_inputs.clear();
        self.audio_outputs.clear();
        self.status = "No audio devices are listed yet. The system default is used.".to_string();
    }

    fn save_settings(&mut self) {
        let callsign = store::normalise(&self.settings.callsign);
        if !store::valid_callsign(&callsign) {
            self.status = "Your callsign must be 3 to 10 letters or digits.".to_string();
            return;
        }
        let rigctld = optional(&self.settings.rigctld);
        if let Some(addr) = &rigctld {
            if !store::valid_rigctld(addr) {
                self.status =
                    "The radio control address must look like host:port, for example 127.0.0.1:4532."
                        .to_string();
                return;
            }
        }
        let data_dir = self.settings.data_dir.trim().to_string();
        if data_dir.is_empty() {
            self.status = "Choose a data folder.".to_string();
            return;
        }
        let config = Config {
            callsign: callsign.clone(),
            audio_in: optional(&self.settings.audio_in),
            audio_out: optional(&self.settings.audio_out),
            rigctld,
            sdr: optional(&self.settings.sdr),
            data_dir: PathBuf::from(&data_dir),
        };

        let dir = default_data_dir();
        let mut conf = store::read_conf(&dir);
        put(&mut conf, "callsign", &Some(callsign.clone()));
        put(&mut conf, "audio_in", &config.audio_in);
        put(&mut conf, "audio_out", &config.audio_out);
        put(&mut conf, "rigctld", &config.rigctld);
        put(&mut conf, "sdr", &config.sdr);
        put(&mut conf, "data_dir", &Some(data_dir));
        conf.insert(
            "location_mode".to_string(),
            self.settings.location_mode.key().to_string(),
        );
        let radius = self.settings.radius_km.clamp(RADIUS_MIN_KM, RADIUS_MAX_KM);
        self.settings.radius_km = radius;
        conf.insert("location_radius_km".to_string(), radius.to_string());
        if store::write_conf(&dir, &conf).is_err() {
            self.status =
                "Could not save the settings. Check that the KAHF folder can be written.".to_string();
            return;
        }

        let busy = self.link_active();
        self.config = config;
        self.settings.callsign = callsign;
        self.recent = store::load_stations(&self.config.data_dir);
        if busy {
            self.status = "Settings saved. The new settings apply after you disconnect.".to_string();
        } else {
            self.drop_engine();
            self.status = "Settings saved.".to_string();
        }
    }
}
