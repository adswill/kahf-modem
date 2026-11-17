//! The six central panels. Each one only edits GUI state, or calls the engine on a user action.
use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText, ScrollArea, TextEdit};
use egui_plot::{Line, Plot, PlotPoints};

use crate::{theme, App, ChatLine, Transfer, NOT_CONNECTED};

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

impl App {
    // ---- Connect ---------------------------------------------------------

    pub fn connect_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Connect");
        ui.add_space(theme::UNIT);
        ui.horizontal(|ui| {
            ui.label("Peer callsign");
            ui.add(TextEdit::singleline(&mut self.peer).desired_width(200.0));
        });
        ui.horizontal(|ui| {
            let can_connect = !self.peer.trim().is_empty();
            if ui.add_enabled(can_connect, egui::Button::new("Connect")).clicked() {
                self.do_connect();
            }
            let connected = self.current_peer.is_some();
            if ui.add_enabled(connected, egui::Button::new("Disconnect")).clicked() {
                self.do_disconnect();
            }
        });
        ui.add_space(theme::UNIT);
        ui.separator();
        ui.label(RichText::new("Event log").strong());
        ScrollArea::vertical()
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for line in &self.log {
                    ui.label(RichText::new(line).monospace());
                }
            });
    }

    fn do_connect(&mut self) {
        let peer = self.peer.trim().to_string();
        match self.engine_call(|e| e.connect(&peer)) {
            Ok(()) => {
                self.status = format!("Connecting to {peer}");
                self.current_peer = Some(peer);
            }
            Err(e) => self.status = e,
        }
    }

    fn do_disconnect(&mut self) {
        match self.engine_call(|e| e.disconnect()) {
            Ok(()) => {
                self.status = "Disconnecting".to_string();
                self.current_peer = None;
            }
            Err(e) => self.status = e,
        }
    }

    // ---- Messages --------------------------------------------------------

    pub fn messages_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Messages");
        ui.add_space(theme::UNIT);

        egui::Panel::left("conversation_list")
            .resizable(false)
            .default_size(200.0)
            .show(ui, |ui| {
                ui.label(RichText::new("Conversations").strong());
                ScrollArea::vertical().show(ui, |ui| {
                    if self.conversations.is_empty() {
                        ui.label(RichText::new("No conversations yet").weak());
                    }
                    for peer in self.conversations.clone() {
                        let selected = self.selected_conv.as_deref() == Some(peer.as_str());
                        if ui.selectable_label(selected, &peer).clicked() {
                            self.selected_conv = Some(peer);
                        }
                    }
                });
            });

        egui::Panel::bottom("chat_input_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                let width = (ui.available_width() - 96.0).max(120.0);
                let response = ui.add(
                    TextEdit::singleline(&mut self.chat_input)
                        .desired_width(width)
                        .hint_text("Type a message"),
                );
                let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let can_send = self.selected_conv.is_some() && !self.chat_input.trim().is_empty();
                if ui.add_enabled(can_send, egui::Button::new("Send")).clicked()
                    || (enter && can_send)
                {
                    self.send_chat();
                }
            });
        });

        egui::CentralPanel::default().show(ui, |ui| {
            match self.selected_conv.clone() {
                None => {
                    ui.label(RichText::new("Select a conversation, or connect to a peer.").weak());
                }
                Some(peer) => {
                    ui.label(RichText::new(&peer).strong());
                    ui.separator();
                    ScrollArea::vertical()
                        .stick_to_bottom(true)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if let Some(lines) = self.chats.get(&peer) {
                                for line in lines {
                                    let who = if line.outgoing { "Me" } else { peer.as_str() };
                                    ui.label(format!("{who}: {}", line.text));
                                }
                            }
                        });
                }
            }
        });
    }

    fn send_chat(&mut self) {
        let Some(to) = self.selected_conv.clone() else {
            return;
        };
        let text = self.chat_input.trim().to_string();
        match self.engine_call(|e| e.send_message(&to, &text)) {
            Ok(()) => {
                self.push_chat(to, ChatLine { outgoing: true, text });
                self.chat_input.clear();
            }
            Err(e) => self.status = e,
        }
    }

    // ---- Mail ------------------------------------------------------------

    pub fn mail_section(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Mail");
            ui.add_space(theme::UNIT);
            ui.selectable_value(&mut self.composing, false, "Reader");
            ui.selectable_value(&mut self.composing, true, "Compose");
        });
        ui.add_space(theme::UNIT);

        egui::Panel::left("inbox")
            .resizable(false)
            .default_size(300.0)
            .show(ui, |ui| {
                ui.label(RichText::new("Inbox").strong());
                ScrollArea::vertical().show(ui, |ui| {
                    if self.mail.is_empty() {
                        ui.label(RichText::new("No mail").weak());
                    }
                    for item in &self.mail {
                        let selected = self.selected_mail == Some(item.id);
                        let title = format!("{}\n{}", item.subject, item.from);
                        if ui.selectable_label(selected, title).clicked() {
                            self.selected_mail = Some(item.id);
                            self.composing = false;
                        }
                    }
                });
            });

        egui::CentralPanel::default().show(ui, |ui| {
            if self.composing {
                self.compose_form(ui);
            } else {
                self.mail_reader(ui);
            }
        });
    }

    fn mail_reader(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.selected_mail else {
            ui.label(RichText::new("Select a message.").weak());
            return;
        };
        let Some(item) = self.mail.iter().find(|m| m.id == id) else {
            ui.label(RichText::new("Message no longer available.").weak());
            return;
        };
        ui.label(RichText::new(&item.subject).strong());
        ui.label(format!("From: {}", item.from));
        ui.separator();
        ui.label(RichText::new("Body not available yet.").weak());
    }

    fn compose_form(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("New message").strong());
        egui::Grid::new("compose_grid")
            .num_columns(2)
            .spacing([theme::UNIT * 2.0, theme::UNIT])
            .show(ui, |ui| {
                ui.label("To");
                ui.add(TextEdit::singleline(&mut self.compose.to).desired_width(360.0));
                ui.end_row();
                ui.label("Subject");
                ui.add(TextEdit::singleline(&mut self.compose.subject).desired_width(360.0));
                ui.end_row();
            });
        ui.label("Body");
        ui.add(
            TextEdit::multiline(&mut self.compose.body)
                .desired_width(f32::INFINITY)
                .desired_rows(8),
        );
        ui.add_space(theme::UNIT);
        ui.label("Attachments");
        ui.horizontal(|ui| {
            ui.add(
                TextEdit::singleline(&mut self.compose.attach_path)
                    .desired_width(360.0)
                    .hint_text("/path/to/file"),
            );
            let can_attach = !self.compose.attach_path.trim().is_empty();
            if ui.add_enabled(can_attach, egui::Button::new("Attach")).clicked() {
                let path = PathBuf::from(self.compose.attach_path.trim());
                self.compose.attachments.push(path);
                self.compose.attach_path.clear();
            }
        });
        let mut remove = None;
        for (i, path) in self.compose.attachments.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(path.display().to_string());
                if ui.small_button("Remove").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            self.compose.attachments.remove(i);
        }
        ui.add_space(theme::UNIT);
        let can_send = !self.compose.to.trim().is_empty();
        if ui.add_enabled(can_send, egui::Button::new("Send")).clicked() {
            self.send_mail_now();
        }
    }

    fn send_mail_now(&mut self) {
        let to = self.compose.to.trim().to_string();
        let subject = self.compose.subject.clone();
        let body = self.compose.body.clone();
        let attachments = self.compose.attachments.clone();
        match self.engine_call(|e| e.send_mail(&to, &subject, &body, &attachments)) {
            Ok(id) => {
                self.status = format!("Mail queued (id {id})");
                self.compose = Default::default();
                self.composing = false;
            }
            Err(e) => self.status = e,
        }
    }

    // ---- Files -----------------------------------------------------------

    pub fn files_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Files");
        ui.add_space(theme::UNIT);
        ui.horizontal(|ui| {
            ui.label("To");
            ui.add(TextEdit::singleline(&mut self.send_file_to).desired_width(140.0));
            ui.label("File");
            ui.add(
                TextEdit::singleline(&mut self.send_file_path)
                    .desired_width(360.0)
                    .hint_text("/path/to/file"),
            );
            let can_send = !self.send_file_to.trim().is_empty() && !self.send_file_path.trim().is_empty();
            if ui.add_enabled(can_send, egui::Button::new("Send file")).clicked() {
                self.send_file_now();
            }
        });
        ui.add_space(theme::UNIT);
        ui.separator();
        if self.transfers.is_empty() {
            ui.label(RichText::new("No transfers").weak());
            return;
        }
        egui::Grid::new("transfer_table")
            .striped(true)
            .num_columns(4)
            .spacing([theme::UNIT * 2.0, theme::UNIT])
            .show(ui, |ui| {
                ui.label(RichText::new("Name").strong());
                ui.label(RichText::new("Size").strong());
                ui.label(RichText::new("Progress").strong());
                ui.label(RichText::new("Direction").strong());
                ui.end_row();
                for t in &self.transfers {
                    transfer_row(ui, t);
                    ui.end_row();
                }
            });
    }

    fn send_file_now(&mut self) {
        let to = self.send_file_to.trim().to_string();
        let path: PathBuf = Path::new(self.send_file_path.trim()).to_path_buf();
        match self.engine_call(|e| e.send_file(&to, &path)) {
            Ok(id) => {
                self.outgoing_ids.insert(id);
                self.status = format!("File queued (id {id})");
            }
            Err(e) => self.status = e,
        }
    }

    // ---- Spectrum --------------------------------------------------------

    pub fn spectrum_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Spectrum");
        ui.add_space(theme::UNIT);
        if self.spectrum.is_empty() {
            ui.label(RichText::new(format!("No spectrum data yet ({NOT_CONNECTED} or idle).")).weak());
        }
        let points: PlotPoints = self
            .spectrum
            .iter()
            .enumerate()
            .map(|(i, v)| [i as f64, *v as f64])
            .collect();
        Plot::new("spectrum_plot")
            .height(ui.available_height().max(200.0))
            .x_axis_label("bin")
            .y_axis_label("level")
            .show(ui, |plot_ui| {
                plot_ui.line(Line::new("spectrum", points));
            });
    }

    // ---- Settings --------------------------------------------------------

    pub fn settings_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Settings");
        ui.add_space(theme::UNIT);
        egui::Grid::new("settings_grid")
            .num_columns(2)
            .spacing([theme::UNIT * 2.0, theme::UNIT])
            .show(ui, |ui| {
                let fields: [(&str, &mut String, &str); 6] = [
                    ("Callsign", &mut self.settings.callsign, "e.g. N0CALL"),
                    ("Audio input", &mut self.settings.audio_in, "device name, blank for default"),
                    ("Audio output", &mut self.settings.audio_out, "device name, blank for default"),
                    ("rigctld address", &mut self.settings.rigctld, "host:port, blank to disable"),
                    ("SDR", &mut self.settings.sdr, "SDR device, blank for none"),
                    ("Data directory", &mut self.settings.data_dir, "folder for mail and files"),
                ];
                for (label, value, hint) in fields {
                    ui.label(label);
                    ui.add(
                        TextEdit::singleline(value)
                            .desired_width(420.0)
                            .hint_text(hint),
                    );
                    ui.end_row();
                }
            });
        ui.add_space(theme::UNIT);
        ui.horizontal(|ui| {
            if ui.button("Apply").clicked() {
                self.apply_settings();
            }
            if ui.button("Revert").clicked() {
                self.settings.callsign = self.config.callsign.clone();
                self.settings.data_dir = self.config.data_dir.display().to_string();
            }
        });
    }
}

fn transfer_row(ui: &mut egui::Ui, t: &Transfer) {
    ui.label(&t.name);
    ui.label(human_size(t.total));
    let fraction = if t.total == 0 {
        0.0
    } else {
        (t.done as f32 / t.total as f32).clamp(0.0, 1.0)
    };
    ui.add(
        egui::ProgressBar::new(fraction)
            .desired_width(200.0)
            .show_percentage(),
    );
    ui.label(if t.outgoing { "Sent" } else { "Received" });
}
