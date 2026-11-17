//! Messages panel under the centre column: the conversation with the selected station, a composer
//! with Message or Mail mode, and Attach (mail) or Send file (message). Ctrl+Enter sends.
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use eframe::egui::{self, Button, Key, Modifiers, ProgressBar, RichText, ScrollArea, TextEdit};

use crate::{store, theme, App, Item, PathFor};

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

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

impl App {
    pub fn messages_panel(&mut self, ui: &mut egui::Ui) {
        // Ctrl+Enter is read before any text field in this panel is drawn, so it never adds a newline.
        let send_key = ui.input_mut(|i| {
            i.consume_key(Modifiers::CTRL, Key::Enter) | i.consume_key(Modifiers::COMMAND, Key::Enter)
        });
        let Some(call) = self.selected.clone() else {
            theme::section_heading(ui, "Messages");
            ui.label(RichText::new("Choose a station on the left to read and write to it.").weak());
            return;
        };
        self.unread.remove(&call);

        let line = if self.current_peer.as_deref() == Some(call.as_str()) {
            self.link_line()
        } else {
            format!("Not linked to {call}. Press Connect on the left to link.")
        };
        ui.horizontal(|ui| {
            theme::section_heading(ui, &format!("Conversation with {call}"));
            ui.label(RichText::new(line).weak());
        });

        egui::Panel::bottom("composer").show(ui, |ui| self.composer(ui, &call));
        if send_key {
            if self.compose.mail {
                self.send_mail();
            } else {
                self.send_chat();
            }
        }

        // Read in place: the history is never cloned. The panel's fields are separate from the map.
        let activity = &self.activity;
        let open_mail = &mut self.open_mail;
        ScrollArea::vertical()
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| match activity.get(&call) {
                Some(items) if !items.is_empty() => {
                    for item in items {
                        item_view(ui, &call, item, open_mail);
                        ui.add_space(theme::UNIT / 2.0);
                    }
                }
                _ => {
                    ui.label(
                        RichText::new(format!(
                            "No messages, mail or files with {call} yet. Type below to send one."
                        ))
                        .weak(),
                    );
                }
            });
    }

    /// Mode buttons, the optional path row, and the message or mail fields.
    fn composer(&mut self, ui: &mut egui::Ui, call: &str) {
        ui.horizontal(|ui| {
            if ui.selectable_label(!self.compose.mail, "Message").clicked() {
                self.compose.mail = false;
            }
            if ui.selectable_label(self.compose.mail, "Mail").clicked() {
                self.compose.mail = true;
            }
            // In Mail mode the button attaches a file to the mail; in Message mode it sends a file.
            let (label, purpose) = if self.compose.mail {
                ("Attach", PathFor::Attach)
            } else {
                ("Send file", PathFor::SendFile)
            };
            if ui.button(label).clicked() {
                self.toggle_path(purpose);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new("Ctrl+Enter sends").weak());
            });
        });
        if let Some(purpose) = self.compose.path_for {
            self.path_row(ui, call, purpose);
        }
        if self.compose.mail {
            self.mail_fields(ui, call);
        } else {
            self.message_field(ui, call);
        }
    }

    fn toggle_path(&mut self, purpose: PathFor) {
        self.compose.path_for = if self.compose.path_for == Some(purpose) {
            None
        } else {
            Some(purpose)
        };
    }

    fn path_row(&mut self, ui: &mut egui::Ui, call: &str, purpose: PathFor) {
        let hint = match purpose {
            PathFor::Attach => "Full path of the file to attach to the mail".to_string(),
            PathFor::SendFile => format!("Full path of the file to send to {call}"),
        };
        ui.horizontal(|ui| {
            ui.add(
                TextEdit::singleline(&mut self.compose.path)
                    .desired_width((ui.available_width() - 180.0).max(120.0))
                    .hint_text(hint),
            );
            match purpose {
                PathFor::Attach => {
                    if ui.button("Add to mail").clicked() {
                        self.add_attachment();
                    }
                }
                PathFor::SendFile => {
                    if ui
                        .add_enabled(!self.listen_only, Button::new("Send file"))
                        .clicked()
                    {
                        self.send_file_now();
                    }
                }
            }
            if ui.button("Close").clicked() {
                self.compose.path_for = None;
            }
        });
    }

    fn message_field(&mut self, ui: &mut egui::Ui, call: &str) {
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 80.0).max(120.0);
            ui.add(
                TextEdit::multiline(&mut self.compose.text)
                    .desired_rows(2)
                    .desired_width(width)
                    .hint_text(format!("Message to {call}")),
            );
            let can_send = !self.compose.text.trim().is_empty() && !self.listen_only;
            if ui.add_enabled(can_send, Button::new("Send")).clicked() {
                self.send_chat();
            }
        });
    }

    fn mail_fields(&mut self, ui: &mut egui::Ui, call: &str) {
        ui.horizontal(|ui| {
            ui.label("Subject");
            ui.add(
                TextEdit::singleline(&mut self.compose.subject)
                    .desired_width(f32::INFINITY)
                    .hint_text(format!("Subject of the mail to {call}")),
            );
        });
        ui.add(
            TextEdit::multiline(&mut self.compose.body)
                .desired_rows(2)
                .desired_width(f32::INFINITY)
                .hint_text("Mail text"),
        );
        let mut remove = None;
        for (i, path) in self.compose.attachments.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(format!("Attached: {}", file_name(path)));
                if ui.small_button("Remove").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            self.compose.attachments.remove(i);
        }
        ui.horizontal(|ui| {
            let can_send = !self.compose.subject.trim().is_empty() && !self.listen_only;
            if ui.add_enabled(can_send, Button::new("Send mail")).clicked() {
                self.send_mail();
            }
        });
    }

    pub fn send_chat(&mut self) {
        if self.refuse_transmit() {
            return;
        }
        let Some(to) = self.selected.clone() else {
            return;
        };
        let text = self.compose.text.trim().to_string();
        if text.is_empty() {
            self.status = "Type a message first.".to_string();
            return;
        }
        match self.radio(|e| e.send_message(&to, &text)) {
            Ok(()) => {
                self.compose.text.clear();
                self.messages_sent += 1;
                self.push_item(
                    &to,
                    Item::Chat {
                        outgoing: true,
                        text,
                        at: store::now(),
                    },
                );
                self.touch_station(&to, false, true);
                self.mark_sending();
                self.status = format!("Message queued for {to}.");
            }
            Err(e) => self.status = e,
        }
    }

    pub fn send_mail(&mut self) {
        if self.refuse_transmit() {
            return;
        }
        let Some(to) = self.selected.clone() else {
            return;
        };
        let subject = self.compose.subject.trim().to_string();
        if subject.is_empty() {
            self.status = "Add a subject for the mail.".to_string();
            return;
        }
        let body = self.compose.body.clone();
        let paths = self.compose.attachments.clone();
        match self.radio(|e| e.send_mail(&to, &subject, &body, &paths)) {
            Ok(id) => {
                let files: Vec<String> = paths.iter().map(|p| file_name(p)).collect();
                self.compose.subject.clear();
                self.compose.body.clear();
                self.compose.attachments.clear();
                self.push_item(
                    &to,
                    Item::Mail {
                        id,
                        outgoing: true,
                        subject,
                        files,
                        body: Some(body),
                        at: store::now(),
                    },
                );
                self.touch_station(&to, false, true);
                self.mark_sending();
                self.status = format!("Mail queued for {to}.");
            }
            Err(e) => self.status = e,
        }
    }

    pub fn send_file_now(&mut self) {
        if self.refuse_transmit() {
            return;
        }
        let Some(to) = self.selected.clone() else {
            return;
        };
        let path = PathBuf::from(self.compose.path.trim());
        if path.as_os_str().is_empty() {
            self.status = "Type the full path of the file to send.".to_string();
            return;
        }
        if !path.is_file() {
            self.status = "No file at that path. Check the path and try again.".to_string();
            return;
        }
        let name = file_name(&path);
        match self.radio(|e| e.send_file(&to, &path)) {
            Ok(id) => {
                self.transfer_station.insert(id, to.clone());
                self.outgoing_ids.insert(id);
                self.push_item(
                    &to,
                    Item::File {
                        id,
                        outgoing: true,
                        name: name.clone(),
                        done: 0,
                        total: 0,
                        at: store::now(),
                    },
                );
                self.compose.path.clear();
                self.compose.path_for = None;
                self.touch_station(&to, false, true);
                self.mark_sending();
                self.status = format!("File {name} queued for {to}.");
            }
            Err(e) => self.status = e,
        }
    }

    pub fn add_attachment(&mut self) {
        let path = PathBuf::from(self.compose.path.trim());
        if !path.is_file() {
            self.status = "No file at that path. Check the path and try again.".to_string();
            return;
        }
        self.status = format!("Attached {} to the mail.", file_name(&path));
        self.compose.attachments.push(path);
        self.compose.path.clear();
        self.compose.path_for = None;
    }
}

/// One entry of a conversation. Takes the mail-open set directly so the history is only borrowed.
fn item_view(ui: &mut egui::Ui, call: &str, item: &Item, open_mail: &mut HashSet<u64>) {
    ui.group(|ui| match item {
        Item::Chat { outgoing, text, at } => {
            let who = if *outgoing {
                "You".to_string()
            } else {
                call.to_string()
            };
            ui.horizontal(|ui| {
                ui.strong(who);
                ui.label(RichText::new(store::clock(*at)).weak());
            });
            ui.label(text);
        }
        Item::Mail {
            id,
            outgoing,
            subject,
            files,
            body,
            at,
        } => {
            let direction = if *outgoing { "Mail to" } else { "Mail from" };
            ui.horizontal(|ui| {
                ui.strong(format!("{direction} {call}"));
                ui.label(RichText::new(store::clock(*at)).weak());
            });
            ui.label(format!("Subject: {subject}"));
            if !files.is_empty() {
                ui.label(format!("Attached: {}", files.join(", ")));
            }
            let open = open_mail.contains(id);
            if ui.button(if open { "Hide text" } else { "Show text" }).clicked() {
                if open {
                    open_mail.remove(id);
                } else {
                    open_mail.insert(*id);
                }
            }
            if open {
                match body {
                    Some(text) => {
                        ui.label(text);
                    }
                    None => {
                        ui.label(
                            RichText::new(
                                "The text of this mail is not available in this version of KAHF yet.",
                            )
                            .weak(),
                        );
                    }
                }
            }
        }
        Item::File {
            outgoing,
            name,
            done,
            total,
            at,
            ..
        } => {
            ui.horizontal(|ui| {
                ui.strong(format!("File {name}"));
                ui.label(RichText::new(store::clock(*at)).weak());
            });
            let direction = if *outgoing {
                format!("Sending to {call}")
            } else {
                format!("Receiving from {call}")
            };
            ui.label(direction);
            if *total == 0 {
                ui.label(RichText::new("Waiting to start").weak());
            } else {
                let frac = (*done as f32 / *total as f32).clamp(0.0, 1.0);
                ui.add(ProgressBar::new(frac).text(format!(
                    "{} of {}",
                    human_size(*done),
                    human_size(*total)
                )));
                if done >= total {
                    ui.label("Complete.");
                }
            }
        }
    });
}
