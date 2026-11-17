//! Right column of the main window: link steps, transmit state and queue, channel in use, rig, audio
//! level, hops for the last message, and for relay roles the schedule. A collapsed event log sits at
//! the bottom. Settings and the relay statistics live in the left sidebar (see home.rs).
use eframe::egui::{self, CollapsingHeader, RichText, ScrollArea, Sense, Stroke, Vec2};
use kahf_core::relay::{self, Role};

use crate::{detail_row, store, theme, App, CONNECT_ATTEMPTS};

/// Values the engine reports about the transmit path and the rig. Each stays None until reported.
// TODO: Event::Tx carries only the transmit progress. SNR, speed, frame error rate, rig and audio
// level need the engine to report them before these fields can be filled.
#[derive(Default)]
pub struct Metrics {
    pub snr_db: Option<f32>,
    pub speed_bps: Option<f32>,
    /// Fraction of frames with errors, 0 to 1.
    pub frame_error_rate: Option<f32>,
    pub rig_freq_hz: Option<u64>,
    pub rig_mode: Option<String>,
    pub ptt: Option<bool>,
    /// Audio level, 0 to 1.
    pub audio_level: Option<f32>,
}

/// Relay state for the relay blocks. Each field stays at its default until the app reports it.
// TODO: the app does not hold relay state yet. Until it does, `App::relay_panel` returns the default,
// so the role is Station and the relay blocks stay hidden.
#[derive(Default)]
pub struct RelayPanel {
    /// None until the role is set. None reads as a plain station.
    pub role: Option<Role>,
    /// None until the radio class is known.
    pub radio: Option<relay::Radio>,
    /// Slot length in seconds. Zero means no schedule is set.
    pub slot_s: u32,
    pub stats: relay::Stats,
    pub hops: relay::HopLog,
    /// Message id of the last message sent or received, if any.
    pub last_message: Option<u64>,
}

impl RelayPanel {
    pub fn role(&self) -> Role {
        self.role.unwrap_or(Role::Station)
    }

    /// True for a relay, and for a station that also relays. Only these roles show relay blocks.
    pub fn is_relay(&self) -> bool {
        matches!(self.role(), Role::Relay | Role::RelayAndStation)
    }
}

const NOT_REPORTED: &str = "Not reported yet";

/// A reported value as text, or "Not reported yet".
fn reported(value: Option<String>) -> String {
    value.unwrap_or_else(|| NOT_REPORTED.to_string())
}

impl App {
    /// Relay state of the app. See the TODO on `RelayPanel`.
    pub fn relay_panel(&self) -> RelayPanel {
        RelayPanel::default()
    }

    /// The whole right column. Settings are not here; they are in the sidebar.
    pub fn right_column(&mut self, ui: &mut egui::Ui) {
        let relay = self.relay_panel();
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            self.link_section(ui);
            self.transmit_section(ui);
            self.channel_section(ui);
            self.rig_section(ui);
            self.audio_section(ui);
            self.hops_section(ui, &relay);
            if relay.is_relay() {
                schedule_section(ui, &relay);
            }

            ui.add_space(theme::UNIT);
            CollapsingHeader::new("Event log")
                .default_open(false)
                .show(ui, |ui| self.event_log(ui));
        });
    }

    /// Link heading, the station and message counts, and the numbered connection steps.
    pub fn link_section(&mut self, ui: &mut egui::Ui) {
        theme::section_heading(ui, "Link");
        egui::Grid::new("link_grid")
            .num_columns(2)
            .spacing([theme::UNIT * 2.0, theme::UNIT / 2.0])
            .show(ui, |ui| {
                detail_row(ui, "Station", self.current_peer.as_deref().unwrap_or("None"));
                detail_row(ui, "Sent", &self.messages_sent.to_string());
                detail_row(ui, "Received", &self.messages_received.to_string());
            });
        ui.add_space(theme::UNIT / 2.0);
        self.connect_steps(ui);
    }

    /// Transmit heading: state, the queue of messages waiting, last transmit time, and the figures
    /// the engine reports.
    pub fn transmit_section(&self, ui: &mut egui::Ui) {
        theme::section_heading(ui, "Transmit");
        let last = self
            .last_tx
            .map(store::stamp)
            .unwrap_or_else(|| "None this session".to_string());
        let queue = if self.engine.is_some() {
            format!("{} waiting", self.outbox)
        } else {
            "Radio not started".to_string()
        };
        let m = &self.metrics;
        egui::Grid::new("tx_grid")
            .num_columns(2)
            .spacing([theme::UNIT * 2.0, theme::UNIT / 2.0])
            .show(ui, |ui| {
                detail_row(ui, "State", self.radio_state().label());
                detail_row(ui, "Queue", &queue);
                detail_row(ui, "Last transmit", &last);
                detail_row(ui, "SNR", &reported(m.snr_db.map(|v| format!("{v:.1} dB"))));
                detail_row(
                    ui,
                    "Speed",
                    &reported(m.speed_bps.map(|v| format!("{v:.0} bit/s"))),
                );
                detail_row(
                    ui,
                    "Frame error rate",
                    &reported(m.frame_error_rate.map(|v| format!("{:.1} %", v * 100.0))),
                );
            });
    }

    /// Channel in use: the frequency and where the signal comes from.
    pub fn channel_section(&self, ui: &mut egui::Ui) {
        theme::section_heading(ui, "Channel");
        let frequency = match self.freq_hz() {
            Some(hz) => format!("{:.3} MHz", hz as f64 / 1e6),
            None => NOT_REPORTED.to_string(),
        };
        let source = self
            .config
            .sdr
            .clone()
            .map(|s| format!("SDR: {s}"))
            .unwrap_or_else(|| "Sound card".to_string());
        egui::Grid::new("channel_grid")
            .num_columns(2)
            .spacing([theme::UNIT * 2.0, theme::UNIT / 2.0])
            .show(ui, |ui| {
                detail_row(ui, "Frequency", &frequency);
                detail_row(ui, "Source", &source);
            });
    }

    /// Rig heading: frequency, mode and PTT as the engine reports them.
    pub fn rig_section(&self, ui: &mut egui::Ui) {
        theme::section_heading(ui, "Rig");
        let m = &self.metrics;
        egui::Grid::new("rig_grid")
            .num_columns(2)
            .spacing([theme::UNIT * 2.0, theme::UNIT / 2.0])
            .show(ui, |ui| {
                detail_row(
                    ui,
                    "Frequency",
                    &reported(m.rig_freq_hz.map(|hz| format!("{:.3} MHz", hz as f64 / 1e6))),
                );
                detail_row(ui, "Mode", &reported(m.rig_mode.clone()));
                detail_row(
                    ui,
                    "PTT",
                    &reported(m.ptt.map(|on| if on { "On" } else { "Off" }.to_string())),
                );
            });
    }

    /// Audio heading and the level meter.
    pub fn audio_section(&self, ui: &mut egui::Ui) {
        theme::section_heading(ui, "Audio");
        ui.label("Level");
        level_meter(ui, self.metrics.audio_level);
    }

    /// Hops the last message took through relays, in order.
    fn hops_section(&self, ui: &mut egui::Ui, relay: &RelayPanel) {
        theme::section_heading(ui, "Hops for last message");
        let Some(id) = relay.last_message else {
            ui.label(RichText::new("No message sent or received yet.").weak());
            return;
        };
        let chain = relay.hops.chain_for(id);
        if chain.is_empty() {
            ui.label(RichText::new("No relay hops for this message.").weak());
            return;
        }
        for (i, hop) in chain.iter().enumerate() {
            let heard = if hop.heard_by_next {
                "heard by next"
            } else {
                "not heard by next"
            };
            ui.label(theme::number(format!(
                "{}. {} at {}, {:.1} kHz, {heard}",
                i + 1,
                hop.relay,
                store::clock(hop.at_unix),
                hop.channel_khz
            )));
        }
    }

    /// Numbered steps of a connection. The current step is bold and in the accent colour.
    /// After a failed attempt the reason and one next action are shown instead.
    fn connect_steps(&mut self, ui: &mut egui::Ui) {
        if let Some(error) = self.connect_error.clone() {
            ui.label(RichText::new(error).color(theme::WARN));
            if ui.button("Try again").clicked() {
                let call = store::normalise(&self.peer_input);
                self.ask_connect(call);
            }
            return;
        }
        let current = match self.link_state.as_str() {
            "Connected" => 3,
            "Connecting" => 2,
            _ => 0,
        };
        let step2 = if current == 2 {
            format!(
                "2. Wait for reply (attempt {} of {}).",
                self.connect_attempt(),
                CONNECT_ATTEMPTS
            )
        } else {
            "2. Wait for reply.".to_string()
        };
        let steps = [
            "1. Send request.".to_string(),
            step2,
            "3. Connected: messages and files are available.".to_string(),
        ];
        for (i, step) in steps.iter().enumerate() {
            let text = if i + 1 == current {
                RichText::new(step).strong().color(ui.visuals().hyperlink_color)
            } else {
                RichText::new(step).weak()
            };
            ui.label(text);
        }
    }

    /// Event log inside a collapsed section. Only the visible rows are drawn.
    fn event_log(&self, ui: &mut egui::Ui) {
        if self.log.is_empty() {
            ui.label(RichText::new("Events will appear here.").weak());
            return;
        }
        ScrollArea::vertical()
            .max_height(160.0)
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show_rows(ui, 16.0, self.log.len(), |ui, range| {
                for line in &self.log[range] {
                    ui.label(theme::number(line.as_str()));
                }
            });
    }
}

/// Half-duplex schedule for a relay: the first slots, receive and transmit in turn.
fn schedule_section(ui: &mut egui::Ui, relay: &RelayPanel) {
    theme::section_heading(ui, "Relay schedule");
    let duplex = match relay.radio {
        Some(r) if relay::can_receive_while_transmitting(r) => "Receives while transmitting.",
        Some(_) => "Half duplex: receives and transmits in turns.",
        None => "Radio type not reported yet.",
    };
    ui.label(RichText::new(duplex).weak());
    if relay.slot_s == 0 {
        ui.label(RichText::new("No slot length set.").weak());
        return;
    }
    for (start, transmit) in relay::alternating_slots(relay.slot_s, 4) {
        let what = if transmit { "Transmit" } else { "Receive" };
        ui.label(theme::number(format!("{start} s  {what}")));
    }
}

/// Audio level as a bar, 0 to 1. An empty track when the level is not reported.
fn level_meter(ui: &mut egui::Ui, level: Option<f32>) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 12.0), Sense::hover());
    let painter = ui.painter_at(rect);
    let visuals = ui.visuals();
    painter.rect_filled(rect, 0.0, visuals.extreme_bg_color);
    if let Some(v) = level {
        let fill = egui::Rect::from_min_size(
            rect.min,
            Vec2::new(rect.width() * v.clamp(0.0, 1.0), rect.height()),
        );
        painter.rect_filled(fill, 0.0, visuals.hyperlink_color);
    }
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color),
    );
    if level.is_none() {
        ui.label(RichText::new(NOT_REPORTED).weak());
    }
}
