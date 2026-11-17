//! The outer shell. Top bar: callsign and link chip, the frequency in large monospace, band and mode,
//! RX/TX state, Listen only and the UTC clock. Left: the collapsible sidebar (Connect, Stations, Relay,
//! Settings). Centre: spectrum and waterfall on one shared width, with messages below. Right: the link,
//! transmit and rig column. Bottom: the status bar. Dark, flat and dense, taken from the the reference.
use eframe::egui::{self, vec2, FontId, RichText};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{chip, theme, waterfall, App, Radio};

const TOPBAR_H: f32 = 68.0;
const STATUS_H: f32 = 24.0;
/// Default and minimum widths of the left sidebar, in points.
const SIDEBAR_W: f32 = 260.0;
const SIDEBAR_MIN_W: f32 = 220.0;
/// Width of the right column, in points.
const RIGHT_W: f32 = 300.0;
/// Share of the centre height given to messages. Spectrum and waterfall take the rest.
const BOTTOM_SHARE: f32 = 0.42;
/// Share of the spectrum and waterfall height given to the spectrum. The waterfall takes the rest.
const SPECTRUM_SHARE: f32 = 0.4;

/// Draws the whole frame. Panels are added outside-in: top, bottom, left, right, then the centre.
pub fn frame(app: &mut App, ui: &mut egui::Ui) {
    egui::Panel::top("topbar")
        .exact_size(TOPBAR_H)
        .show(ui, |ui| topbar(app, ui));
    egui::Panel::bottom("status")
        .exact_size(STATUS_H)
        .show(ui, |ui| statusbar(app, ui));
    egui::Panel::left("sidebar")
        .resizable(true)
        .default_size(SIDEBAR_W)
        .min_size(SIDEBAR_MIN_W)
        .show(ui, |ui| app.sidebar(ui));
    egui::Panel::right("right_column")
        .resizable(false)
        .exact_size(RIGHT_W)
        .show(ui, |ui| app.right_column(ui));
    egui::CentralPanel::default().show(ui, |ui| centre(app, ui));
}

/// Top bar: callsign, link chip, frequency, band and mode on the left; RX/TX state, Listen only and the
/// UTC clock on the right.
fn topbar(app: &mut App, ui: &mut egui::Ui) {
    let state = app.radio_state();
    let accent = ui.visuals().hyperlink_color;
    let border = ui.visuals().widgets.noninteractive.bg_stroke.color;
    let weak = ui.visuals().weak_text_color();
    let link_colour = if app.link_active() { accent } else { weak };
    let gap = app.version.gap;
    let freq = app.freq_hz();
    let freq_text = match freq {
        Some(hz) => format!("{:.3}", hz as f64 / 1e6),
        None => "---.---".to_string(),
    };
    let band = match freq {
        Some(hz) => band_name(hz).map_or("No amateur band".to_string(), |b| format!("Band {b}")),
        None => "Band --".to_string(),
    };
    let mode = app
        .metrics
        .rig_mode
        .clone()
        .unwrap_or_else(|| "No mode".to_string());
    let callsign = if app.config.callsign.is_empty() {
        "No callsign set".to_string()
    } else {
        app.config.callsign.clone()
    };

    ui.horizontal_centered(|ui| {
        ui.label(theme::number(callsign));
        ui.add_space(gap);
        chip(ui, &app.link_chip(), border, link_colour);
        ui.add_space(gap * 2.0);
        let freq_colour = if freq.is_some() {
            ui.visuals().text_color()
        } else {
            weak
        };
        ui.label(theme::number("MHz"));
        ui.label(
            RichText::new(freq_text)
                .font(FontId::monospace(34.0))
                .strong()
                .color(freq_colour),
        );
        ui.add_space(gap);
        ui.vertical(|ui| {
            ui.label(theme::number(band));
            ui.label(theme::number(format!("Mode {mode}")));
        });
        ui.add_space(gap * 2.0);
        let (rect, _) = ui.allocate_exact_size(vec2(12.0, 12.0), egui::Sense::hover());
        let painter = ui.painter();
        if state == Radio::ListenOnly {
            painter.circle_stroke(rect.center(), 4.5, egui::Stroke::new(1.0, state.colour(accent)));
        } else {
            painter.circle_filled(rect.center(), 4.5, state.colour(accent));
        }
        ui.label(state.label());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Added first so it sits at the far right.
            ui.label(theme::number(utc_clock()));
            ui.add_space(gap);
            let on = app.listen_only;
            if ui.selectable_label(on, "Listen only").clicked() {
                app.set_listen_only(!on);
            }
        });
    });
}

/// Bottom status bar: state dot and label, mode, CPU, the app's status text and version, then the UTC clock.
fn statusbar(app: &mut App, ui: &mut egui::Ui) {
    let gap = app.version.gap;
    let accent = ui.visuals().hyperlink_color;
    let state = app.radio_state();
    let mode = app
        .metrics
        .rig_mode
        .clone()
        .unwrap_or_else(|| "No mode".to_string());
    ui.horizontal_centered(|ui| {
        state_dot(ui, state, accent);
        ui.label(state.label());
        ui.add_space(gap);
        ui.label(theme::number(format!("Mode {mode}")));
        ui.add_space(gap);
        ui.label(theme::number("CPU --"));
        ui.add_space(gap);
        ui.label(RichText::new(format!("Outbox {}", app.outbox)).weak());
        ui.add_space(gap);
        app.status_bar(ui);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(theme::number(utc_clock()));
            ui.label(RichText::new(format!("v{} ({})", env!("CARGO_PKG_VERSION"), app.version.name)).weak());
        });
    });
}

/// A small filled dot in the state colour.
fn state_dot(ui: &mut egui::Ui, state: Radio, accent: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), egui::Sense::hover());
    if state == Radio::ListenOnly {
        ui.painter()
            .circle_stroke(rect.center(), 4.0, egui::Stroke::new(1.0, state.colour(accent)));
    } else {
        ui.painter()
            .circle_filled(rect.center(), 4.0, state.colour(accent));
    }
}

/// The centre: spectrum and waterfall on top, messages below.
fn centre(app: &mut App, ui: &mut egui::Ui) {
    let gap = app.version.gap;
    let width = ui.available_width();
    let total_h = ui.available_height();
    let bottom_h = (total_h * BOTTOM_SHARE).floor();
    let top_h = total_h - bottom_h - gap - 22.0;
    let note = app.sim_note();
    let accent = ui.visuals().hyperlink_color;
    let state = app.radio_state();
    ui.horizontal(|ui| {
        state_dot(ui, state, accent);
        ui.label(RichText::new(state.label()).strong());
        if !note.is_empty() {
            ui.label(RichText::new(note).weak());
        }
    });

    // One rectangle for the spectrum, the axis and the waterfall: the same left edge and width, so a
    // bin sits in the same pixel column in all three. Stacked vertically, spectrum, axis, waterfall.
    let spec_h = ((top_h - waterfall::AXIS_H - 2.0 * gap) * SPECTRUM_SHARE).floor();
    let wf_h = top_h - spec_h - waterfall::AXIS_H - 2.0 * gap;
    ui.allocate_ui(vec2(width, top_h), |ui| {
        app.spectrum.show(ui, note, spec_h);
        waterfall::axis(ui);
        app.waterfall.show(ui, note, wf_h);
    });

    ui.add_space(gap);
    ui.allocate_ui(vec2(width, bottom_h), |ui| app.messages_panel(ui));
}

/// UTC time of day as HH:MM:SS, from the system clock.
fn utc_clock() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let day = secs % 86_400;
    format!("{:02}:{:02}:{:02} UTC", day / 3600, (day % 3600) / 60, day % 60)
}

/// Amateur band for a frequency in Hz, or None outside the bands listed.
fn band_name(hz: u64) -> Option<&'static str> {
    const BANDS: [(u64, u64, &str); 13] = [
        (1_800_000, 2_000_000, "160m"),
        (3_500_000, 4_000_000, "80m"),
        (5_250_000, 5_450_000, "60m"),
        (7_000_000, 7_300_000, "40m"),
        (10_100_000, 10_150_000, "30m"),
        (14_000_000, 14_350_000, "20m"),
        (18_068_000, 18_168_000, "17m"),
        (21_000_000, 21_450_000, "15m"),
        (24_890_000, 24_990_000, "12m"),
        (28_000_000, 29_700_000, "10m"),
        (50_000_000, 54_000_000, "6m"),
        (144_000_000, 146_000_000, "2m"),
        (430_000_000, 440_000_000, "70cm"),
    ];
    BANDS
        .iter()
        .find(|(lo, hi, _)| (*lo..=*hi).contains(&hz))
        .map(|(_, _, name)| *name)
}
