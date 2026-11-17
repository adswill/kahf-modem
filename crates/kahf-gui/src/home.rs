//! Left sidebar of the main window: four collapsible groups with counts. Connect (callsign field,
//! Connect and Disconnect), Stations (table with last heard), Relay (role, sent counts; relay roles
//! only) and Settings (one dense page). Also the connect card and stations table on their own, for
//! the centre layout.
use eframe::egui::{self, Button, CollapsingHeader, Label, RichText, ScrollArea, TextEdit};
use kahf_core::relay::Role;

use crate::inspector::RelayPanel;
use crate::{store, theme, App};

/// Width of the callsign and last-heard columns of the stations table.
const COL_CALL_W: f32 = 110.0;
const COL_SEEN_W: f32 = 120.0;

/// Group title with its count, for example "Stations (3)".
fn titled(name: &str, count: usize) -> String {
    format!("{name} ({count})")
}

impl App {
    /// The four sidebar groups, top to bottom. Settings opens by itself while something needs attention.
    pub fn sidebar(&mut self, ui: &mut egui::Ui) {
        let relay = self.relay_panel();
        let attention = self.settings_attention();

        CollapsingHeader::new(titled("Connect", usize::from(self.link_active())))
            .default_open(true)
            .show(ui, |ui| self.connect_body(ui));
        ui.add_space(theme::UNIT / 2.0);
        CollapsingHeader::new(titled("Stations", self.recent.len()))
            .default_open(true)
            .show(ui, |ui| {
                let height = ui.available_height().max(120.0);
                self.stations_body(ui, height);
            });
        if relay.is_relay() {
            ui.add_space(theme::UNIT / 2.0);
            CollapsingHeader::new(titled("Relay", relay.stats.sent_today as usize))
                .default_open(true)
                .show(ui, |ui| relay_body(ui, &relay));
        }
        ui.add_space(theme::UNIT / 2.0);
        CollapsingHeader::new(titled("Settings", attention))
            .default_open(attention > 0)
            .show(ui, |ui| self.settings_body(ui));
    }

    /// Connect heading and body, for the centre layout.
    pub fn connect_card(&mut self, ui: &mut egui::Ui) {
        theme::section_heading(ui, "Connect");
        self.connect_body(ui);
    }

    /// Callsign field, Connect and Disconnect, and the link line. Nothing is transmitted from here
    /// until the operator confirms the connect prompt.
    pub fn connect_body(&mut self, ui: &mut egui::Ui) {
        let typed = store::normalise(&self.peer_input);
        let own = self.config.callsign.clone();
        let own_ok = store::valid_callsign(&own);
        let active = self.link_active();
        let hint = if self.peer_input.trim().is_empty() {
            None
        } else if !store::valid_callsign(&typed) {
            Some("Use 3 to 10 letters or digits, with no spaces.")
        } else if typed == own {
            Some("That is your own callsign. Type the station you want to reach.")
        } else {
            None
        };
        let can_connect =
            own_ok && !active && !self.listen_only && hint.is_none() && store::valid_callsign(&typed);

        ui.add(
            TextEdit::singleline(&mut self.peer_input)
                .desired_width(f32::INFINITY)
                .hint_text("Station callsign, e.g. XX1XX"),
        );
        ui.horizontal(|ui| {
            if ui.add_enabled(can_connect, Button::new("Connect")).clicked() {
                self.ask_connect(typed.clone());
            }
            if ui.add_enabled(active, Button::new("Disconnect")).clicked() {
                self.disconnect();
            }
        });
        if self.listen_only {
            ui.label(RichText::new("Listen only is on. Connect is off.").weak());
        } else if !own_ok {
            ui.label(RichText::new("Set your callsign in Settings.").color(theme::WARN));
        } else if let Some(text) = hint {
            ui.label(RichText::new(text).color(theme::WARN));
        }
        ui.label(RichText::new(self.link_line()).strong());
    }

    /// Stations heading and table, for the centre layout.
    pub fn stations_table(&mut self, ui: &mut egui::Ui, height: f32) {
        theme::section_heading(ui, "Stations");
        self.stations_body(ui, height);
    }

    /// Stations table: callsign (click to choose it), last heard, unread count. Rows are read in place.
    pub fn stations_body(&mut self, ui: &mut egui::Ui, height: f32) {
        if self.recent.is_empty() {
            ui.label(RichText::new("Stations you connect to are listed here.").weak());
            return;
        }
        let row_h = self.version.row_h;
        ui.horizontal(|ui| {
            ui.add_sized([COL_CALL_W, row_h], Label::new(RichText::new("Callsign").weak()));
            ui.add_sized(
                [COL_SEEN_W, row_h],
                Label::new(RichText::new("Last heard").weak()),
            );
        });
        let mut clicked: Option<String> = None;
        ScrollArea::vertical()
            .id_salt("stations")
            .max_height(height)
            .auto_shrink([false, false])
            .show_rows(ui, row_h, self.recent.len(), |ui, range| {
                for s in &self.recent[range] {
                    let unread = self.unread.get(&s.call).copied().unwrap_or(0);
                    let selected = self.peer_input == s.call;
                    ui.horizontal(|ui| {
                        if ui
                            .add_sized([COL_CALL_W, row_h], Button::selectable(selected, s.call.as_str()))
                            .clicked()
                        {
                            clicked = Some(s.call.clone());
                        }
                        ui.add_sized(
                            [COL_SEEN_W, row_h],
                            Label::new(theme::number(store::stamp(s.last_seen))),
                        );
                        if unread > 0 {
                            ui.label(RichText::new(format!("{unread} unread")).color(theme::WARN));
                        }
                    });
                }
            });
        if let Some(call) = clicked {
            self.peer_input = call.clone();
            self.open_station(&call);
        }
    }

    /// Outbox count, or a note when the radio has not started.
    pub fn outbox_line(&self, ui: &mut egui::Ui) {
        let outbox = if self.engine.is_some() {
            format!("Outbox: {} waiting", self.outbox)
        } else {
            "Outbox: radio not started".to_string()
        };
        ui.label(theme::number(outbox));
    }
}

/// Relay group body: the role and the send counts.
fn relay_body(ui: &mut egui::Ui, relay: &RelayPanel) {
    let role = match relay.role() {
        Role::Station => "Station",
        Role::Relay => "Relay only",
        Role::RelayAndStation => "Relay and station",
    };
    egui::Grid::new("sidebar_relay_grid")
        .num_columns(2)
        .spacing([theme::UNIT * 2.0, theme::UNIT / 2.0])
        .show(ui, |ui| {
            crate::detail_row(ui, "Role", role);
            crate::detail_row(ui, "Sent today", &relay.stats.sent_today.to_string());
            crate::detail_row(ui, "Sent in total", &relay.stats.sent_total.to_string());
        });
}
