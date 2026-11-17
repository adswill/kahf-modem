//! Look of the window: neutral greys, one accent colour taken from the version, a dark default and a light option.
//! Corners are at most 4 px, strokes are 1 px hairlines, there are no shadows or gradients, and spacing
//! follows an 8 px grid. Inter is the text face and JetBrains Mono the face for frequencies and levels.
use std::sync::Arc;

use crate::version::Version;

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow, Stroke,
    Style, TextStyle, Theme, ThemePreference, Visuals,
};

/// Neutral grey for the idle dot and the listen-only ring.
pub const IDLE: Color32 = Color32::from_rgb(0x8A, 0x8F, 0x98);
/// Red. Used for warnings and for the Transmitting state, and for nothing else.
pub const WARN: Color32 = Color32::from_rgb(0xC0, 0x39, 0x2B);
pub const TRANSMIT: Color32 = WARN;

/// Spacing grid unit. Gaps and paddings are multiples of this.
pub const UNIT: f32 = 8.0;
/// Corner radius, never more than 4 px.
pub const RADIUS: u8 = 4;
/// Type scale in points: 20 heading, 14 body and buttons, 13 labels and monospace numbers.
pub const SIZE_TITLE: f32 = 20.0;
pub const SIZE_BODY: f32 = 14.0;
pub const SIZE_LABEL: f32 = 13.0;
/// Font family name of Inter SemiBold, used for titles.
const SEMIBOLD: &str = "Inter SemiBold";

/// Neutral surfaces and text for one colour scheme.
struct Palette {
    panel: Color32,
    surface: Color32,
    field: Color32,
    hover: Color32,
    active: Color32,
    border: Color32,
    text: Color32,
    weak: Color32,
    dark: bool,
}

const DARK: Palette = Palette {
    panel: Color32::from_rgb(0x0B, 0x0C, 0x0E),
    surface: Color32::from_rgb(0x11, 0x13, 0x16),
    field: Color32::from_rgb(0x07, 0x08, 0x09),
    hover: Color32::from_rgb(0x1B, 0x1E, 0x23),
    active: Color32::from_rgb(0x24, 0x28, 0x2E),
    border: Color32::from_rgb(0x1F, 0x23, 0x29),
    text: Color32::from_rgb(0xD6, 0xDB, 0xE1),
    weak: Color32::from_rgb(0x6F, 0x78, 0x83),
    dark: true,
};

const LIGHT: Palette = Palette {
    panel: Color32::from_rgb(0xF2, 0xF3, 0xF5),
    surface: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    field: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    hover: Color32::from_rgb(0xE6, 0xE9, 0xED),
    active: Color32::from_rgb(0xD9, 0xDE, 0xE4),
    border: Color32::from_rgb(0xD3, 0xD8, 0xDE),
    text: Color32::from_rgb(0x1B, 0x1F, 0x25),
    weak: Color32::from_rgb(0x5E, 0x67, 0x72),
    dark: false,
};

/// Loads the bundled fonts and sets both colour schemes. The window starts dark.
pub fn install(ctx: &egui::Context, v: &Version) {
    let mut fonts = FontDefinitions::default();
    let bundled = [
        ("inter", &include_bytes!("../assets/fonts/Inter-Regular.ttf")[..]),
        (
            "inter-semibold",
            &include_bytes!("../assets/fonts/Inter-SemiBold.ttf")[..],
        ),
        (
            "jetbrains-mono",
            &include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf")[..],
        ),
    ];
    for (name, bytes) in bundled {
        fonts
            .font_data
            .insert(name.to_string(), Arc::new(FontData::from_static(bytes)));
    }
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "inter".to_string());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "jetbrains-mono".to_string());
    fonts.families.insert(
        FontFamily::Name(SEMIBOLD.into()),
        vec!["inter-semibold".to_string(), "inter".to_string()],
    );
    ctx.set_fonts(fonts);

    ctx.set_style_of(Theme::Light, style(Visuals::light(), &LIGHT, v));
    ctx.set_style_of(Theme::Dark, style(Visuals::dark(), &DARK, v));
    apply(ctx, true);
}

/// Switches between the dark and light schemes.
pub fn apply(ctx: &egui::Context, dark: bool) {
    let pref = if dark {
        ThemePreference::Dark
    } else {
        ThemePreference::Light
    };
    ctx.set_theme(pref);
}

/// Section heading: small capitals in the accent colour, with a thin rule under it. Used for every
/// collapsible or titled block so the sidebar and the panels read the same way.
pub fn section_heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(UNIT);
    ui.label(
        egui::RichText::new(text.to_uppercase())
            .size(11.0)
            .family(FontFamily::Name(SEMIBOLD.into()))
            .color(ui.visuals().hyperlink_color),
    );
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().rect_filled(
        rect,
        CornerRadius::ZERO,
        ui.visuals().widgets.noninteractive.bg_stroke.color,
    );
    ui.add_space(UNIT / 2.0);
}

/// Monospace text for frequencies, levels and callsigns, so digits line up.
pub fn number(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text).font(FontId::monospace(SIZE_LABEL))
}

fn style(mut visuals: Visuals, p: &Palette, v: &Version) -> Style {
    let accent = v.accent;
    visuals.dark_mode = p.dark;
    visuals.panel_fill = p.panel;
    visuals.window_fill = p.surface;
    visuals.window_stroke = Stroke::new(1.0, p.border);
    visuals.window_corner_radius = CornerRadius::same(RADIUS);
    visuals.window_shadow = Shadow::NONE;
    visuals.popup_shadow = Shadow::NONE;
    visuals.menu_corner_radius = CornerRadius::same(RADIUS);
    visuals.extreme_bg_color = p.field;
    visuals.faint_bg_color = p.surface;
    visuals.weak_text_color = Some(p.weak);
    visuals.hyperlink_color = accent;
    visuals.selection.bg_fill = accent.gamma_multiply(0.35);
    visuals.selection.stroke = Stroke::new(1.0, accent);
    visuals.warn_fg_color = WARN;
    visuals.error_fg_color = WARN;

    let radius = CornerRadius::same(RADIUS);
    let hairline = Stroke::new(1.0, p.border);
    let text = Stroke::new(1.0, p.text);
    let w = &mut visuals.widgets;
    for (state, fill) in [
        (&mut w.noninteractive, p.panel),
        (&mut w.inactive, p.surface),
        (&mut w.open, p.surface),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = hairline;
        state.fg_stroke = text;
        state.corner_radius = radius;
        state.expansion = 0.0;
    }
    for (state, fill) in [(&mut w.hovered, p.hover), (&mut w.active, p.active)] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::new(1.0, accent);
        state.fg_stroke = text;
        state.corner_radius = radius;
        state.expansion = 0.0;
    }

    let mut style = Style {
        visuals,
        ..Default::default()
    };
    style.text_styles = [
        (
            TextStyle::Heading,
            FontId::new(SIZE_TITLE, FontFamily::Name(SEMIBOLD.into())),
        ),
        (TextStyle::Body, FontId::proportional(SIZE_BODY)),
        (TextStyle::Button, FontId::proportional(SIZE_BODY)),
        (TextStyle::Small, FontId::proportional(SIZE_LABEL)),
        (TextStyle::Monospace, FontId::monospace(SIZE_LABEL)),
    ]
    .into();

    style.spacing.item_spacing = egui::vec2(v.gap, v.gap);
    style.spacing.button_padding = egui::vec2(v.gap, v.gap / 2.0);
    style.spacing.interact_size.y = v.row_h;
    style.spacing.window_margin = Margin::same(UNIT as i8);
    style
}
