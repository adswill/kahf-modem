//! Colours, text sizes and spacing. Follows the system light/dark setting.
use eframe::egui::{self, Color32, FontId, Stroke, Style, TextStyle, Theme, Visuals};

/// The single accent colour used for selection, links and focus rings.
pub const ACCENT: Color32 = Color32::from_rgb(0x2F, 0x6F, 0xDE);

/// Spacing unit; all gaps and paddings are multiples of this.
pub const UNIT: f32 = 8.0;

pub fn install(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::System);
    ctx.set_style_of(Theme::Light, style(Visuals::light()));
    ctx.set_style_of(Theme::Dark, style(Visuals::dark()));
}

fn style(visuals: Visuals) -> Style {
    let mut style = Style::default();
    style.visuals = visuals;
    style.visuals.selection.bg_fill = ACCENT;
    style.visuals.hyperlink_color = ACCENT;
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
    style.visuals.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);

    style.text_styles = [
        (TextStyle::Heading, FontId::proportional(18.0)),
        (TextStyle::Body, FontId::proportional(14.0)),
        (TextStyle::Button, FontId::proportional(14.0)),
        (TextStyle::Small, FontId::proportional(12.0)),
        (TextStyle::Monospace, FontId::monospace(13.0)),
    ]
    .into();

    style.spacing.item_spacing = egui::vec2(UNIT, UNIT);
    style.spacing.button_padding = egui::vec2(UNIT, UNIT / 2.0);
    style.spacing.interact_size.y = 24.0;
    style
}
