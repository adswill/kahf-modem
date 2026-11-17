//! A utilitarian console: square panels, 1 px mid-grey borders, no shadows, no gradients.
//! Accents mark state only: green receiving, amber selected or waiting, red record or error, blue info.

use iced::widget::{button, container, pick_list, progress_bar, text_editor, text_input, toggler};
use iced::{font, theme::Palette, Background, Border, Color, Font, Shadow, Theme};

pub const RADIUS: f32 = 0.0;
pub const HAIRLINE_WIDTH: f32 = 1.0;

const fn hex(v: u32) -> Color {
    Color {
        r: ((v >> 16) & 0xff) as f32 / 255.0,
        g: ((v >> 8) & 0xff) as f32 / 255.0,
        b: (v & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

/// Level 0: the window behind everything.
pub const BACKGROUND: Color = hex(0x1d_1e21);
/// Level 1: panels, bars, the rail.
pub const PANEL: Color = hex(0x26_282c);
/// Level 2: raised things (buttons, header strips, popovers).
pub const RAISED: Color = hex(0x2e_3035);
pub const HAIRLINE: Color = hex(0x3a_3d43);
pub const HAIRLINE_STRONG: Color = hex(0x4c_5057);
pub const TEXT: Color = hex(0xc9_ccd1);
pub const MUTED: Color = hex(0x7d_8189);
pub const FAINT: Color = hex(0x58_5c63);
pub const ACCENT: Color = hex(0x5f_b86a);
/// Darker strip used for panel headers.
pub const HEADER: Color = hex(0x17_181b);
/// Selected, active or waiting.
pub const AMBER: Color = hex(0xe0_a43a);
pub const WARNING: Color = hex(0xe0_a43a);
pub const DANGER: Color = hex(0xd8_5a50);
pub const INFO: Color = hex(0x5b_9bd5);

/// The UI font. On macOS the system font is a variable face that has no usable bold,
/// so a static family with real weights is named; elsewhere the default sans is used.
#[cfg(target_os = "macos")]
pub const UI_FONT: Font = Font {
    family: font::Family::Name("Helvetica Neue"),
    ..Font::DEFAULT
};
#[cfg(not(target_os = "macos"))]
pub const UI_FONT: Font = Font::DEFAULT;

pub const SEMIBOLD: Font = Font {
    weight: font::Weight::Semibold,
    ..UI_FONT
};

pub fn with_alpha(c: Color, a: f32) -> Color {
    Color { a, ..c }
}

pub fn kahf() -> Theme {
    Theme::custom(
        "KAHF".to_string(),
        Palette {
            background: BACKGROUND,
            text: TEXT,
            primary: ACCENT,
            success: ACCENT,
            warning: WARNING,
            danger: DANGER,
        },
    )
}

fn border(color: Color) -> Border {
    Border {
        color,
        width: HAIRLINE_WIDTH,
        radius: 0.0.into(),
    }
}

fn popover_shadow(_strength: f32) -> Shadow {
    Shadow::default()
}

/// Window background (level 0).
pub fn window(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(TEXT),
        background: Some(Background::Color(BACKGROUND)),
        ..Default::default()
    }
}

/// Panel (level 1): fill and a hairline border.
pub fn panel(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(TEXT),
        background: Some(Background::Color(PANEL)),
        border: border(HAIRLINE),
        ..Default::default()
    }
}

/// Bars (top, status, rail): level 1, square, no border (dividers are separate).
pub fn bar(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(TEXT),
        background: Some(Background::Color(PANEL)),
        ..Default::default()
    }
}

/// Raised area (level 2). Callers add the inner highlight with `view::raised`.
pub fn raised(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(TEXT),
        background: Some(Background::Color(RAISED)),
        border: border(HAIRLINE_STRONG),
        ..Default::default()
    }
}

/// Tooltip: a popover, so it may carry a shadow.
pub fn popover(_theme: &Theme) -> container::Style {
    container::Style {
        shadow: popover_shadow(0.6),
        ..raised(_theme)
    }
}

/// Table header strip.
pub fn header_row(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(MUTED),
        background: Some(Background::Color(HEADER)),
        border: Border {
            color: HAIRLINE,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}

/// Table row fill.
pub fn row_fill(selected: bool) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: selected.then_some(Background::Color(with_alpha(AMBER, 0.12))),
        ..Default::default()
    }
}

/// The 2 px bar on the left of a selected row.
pub fn row_bar(selected: bool) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: selected.then_some(Background::Color(ACCENT)),
        ..Default::default()
    }
}

/// Hairline divider fill.
pub fn hairline(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(HAIRLINE)),
        ..Default::default()
    }
}

/// Dimmed backdrop behind modal panels. `a` runs 0..1 during the fade.
pub fn scrim(a: f32) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(with_alpha(BACKGROUND, 0.78 * a))),
        ..Default::default()
    }
}

/// Modal panel; shadow and fill fade in with `a`.
pub fn modal(a: f32) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        text_color: Some(with_alpha(TEXT, a)),
        background: Some(Background::Color(with_alpha(RAISED, a))),
        border: border(with_alpha(HAIRLINE_STRONG, a)),
        shadow: popover_shadow(a),
        ..Default::default()
    }
}

/// Status chip: neutral raised pill; the colour lives in the dot, not the frame.
pub fn chip(_color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        text_color: Some(TEXT),
        background: Some(Background::Color(RAISED)),
        border: Border {
            color: HAIRLINE,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}

/// Tinted notice strip (info, warning, danger).
pub fn notice(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        text_color: Some(color),
        background: Some(Background::Color(with_alpha(color, 0.08))),
        border: border(with_alpha(color, 0.4)),
        ..Default::default()
    }
}

/// Rail button. The active one gets a raised fill; the accent bar is drawn by the caller.
pub fn nav_button(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let (text_color, background) = if active {
            (AMBER, Some(HEADER))
        } else if hovered {
            (TEXT, Some(with_alpha(RAISED, 0.6)))
        } else {
            (MUTED, None)
        };
        button::Style {
            background: background.map(Background::Color),
            text_color,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: RADIUS.into(),
            },
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Primary,
    Secondary,
    Danger,
    Ghost,
}

/// Standard button: primary, secondary, ghost (and danger). Hover lifts the fill,
/// pressed darkens it and draws a 2 px ring. Disabled is dimmed.
pub fn btn(kind: Kind) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let hovered = matches!(status, button::Status::Hovered);
        let pressed = matches!(status, button::Status::Pressed);
        let disabled = matches!(status, button::Status::Disabled);
        let base = match kind {
            Kind::Primary => ACCENT,
            Kind::Danger => DANGER,
            _ => RAISED,
        };
        let shade = |c: Color, f: f32| Color {
            r: c.r * f,
            g: c.g * f,
            b: c.b * f,
            a: c.a,
        };
        let lift = |c: Color, f: f32| Color {
            r: c.r + (1.0 - c.r) * f,
            g: c.g + (1.0 - c.g) * f,
            b: c.b + (1.0 - c.b) * f,
            a: c.a,
        };
        let (fg, bg, line) = match kind {
            Kind::Primary | Kind::Danger => {
                let bg = if pressed {
                    shade(base, 0.82)
                } else if hovered {
                    lift(base, 0.14)
                } else {
                    base
                };
                (BACKGROUND, bg, shade(base, 0.7))
            }
            Kind::Secondary => {
                let bg = if pressed {
                    shade(RAISED, 0.8)
                } else if hovered {
                    lift(RAISED, 0.06)
                } else {
                    RAISED
                };
                (TEXT, bg, if hovered { HAIRLINE_STRONG } else { HAIRLINE })
            }
            Kind::Ghost => (
                if hovered || pressed { TEXT } else { MUTED },
                if pressed {
                    RAISED
                } else if hovered {
                    with_alpha(RAISED, 0.7)
                } else {
                    Color::TRANSPARENT
                },
                Color::TRANSPARENT,
            ),
        };
        if disabled {
            return button::Style {
                background: Some(Background::Color(match kind {
                    Kind::Ghost => Color::TRANSPARENT,
                    _ => with_alpha(RAISED, 0.5),
                })),
                text_color: with_alpha(MUTED, 0.5),
                border: border(with_alpha(HAIRLINE, 0.6)),
                ..Default::default()
            };
        }
        let ring = pressed.then_some(with_alpha(
            if matches!(kind, Kind::Danger) {
                DANGER
            } else {
                ACCENT
            },
            0.85,
        ));
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: fg,
            border: Border {
                color: ring.unwrap_or(line),
                width: if ring.is_some() { 2.0 } else { 1.0 },
                radius: RADIUS.into(),
            },
            ..Default::default()
        }
    }
}

/// One segment of a segmented control.
pub fn segment(selected: bool, first: bool, last: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let r = 0.0f32;
        let radius = iced::border::Radius {
            top_left: if first { r } else { 0.0 },
            bottom_left: if first { r } else { 0.0 },
            top_right: if last { r } else { 0.0 },
            bottom_right: if last { r } else { 0.0 },
        };
        button::Style {
            background: Some(Background::Color(if selected {
                with_alpha(AMBER, 0.16)
            } else if hovered {
                RAISED
            } else {
                BACKGROUND
            })),
            text_color: if selected {
                AMBER
            } else if hovered {
                TEXT
            } else {
                MUTED
            },
            border: Border {
                color: if selected {
                    with_alpha(AMBER, 0.7)
                } else {
                    HAIRLINE
                },
                width: 1.0,
                radius,
            },
            ..Default::default()
        }
    }
}

/// Plain clickable table row (no chrome).
pub fn row_button(_theme: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: hovered.then_some(Background::Color(with_alpha(RAISED, 0.7))),
        text_color: TEXT,
        border: Border {
            radius: 0.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Toggle switch.
pub fn switch(_theme: &Theme, status: toggler::Status) -> toggler::Style {
    let (on, hovered) = match status {
        toggler::Status::Active { is_toggled } => (is_toggled, false),
        toggler::Status::Hovered { is_toggled } => (is_toggled, true),
        toggler::Status::Disabled { is_toggled } => (is_toggled, false),
    };
    let track = if on { with_alpha(ACCENT, 0.9) } else { RAISED };
    toggler::Style {
        background: Background::Color(track),
        background_border_width: 1.0,
        background_border_color: if on {
            ACCENT
        } else if hovered {
            HAIRLINE_STRONG
        } else {
            HAIRLINE
        },
        foreground: Background::Color(if on { BACKGROUND } else { MUTED }),
        foreground_border_width: 0.0,
        foreground_border_color: Color::TRANSPARENT,
        text_color: Some(TEXT),
        border_radius: None,
        padding_ratio: 0.22,
    }
}

/// Thin progress bar.
pub fn progress(_theme: &Theme) -> progress_bar::Style {
    progress_bar::Style {
        background: Background::Color(BACKGROUND),
        bar: Background::Color(ACCENT),
        border: Border {
            color: HAIRLINE,
            width: 1.0,
            radius: 0.0.into(),
        },
    }
}

pub fn input(_theme: &Theme, status: text_input::Status) -> text_input::Style {
    let (line, width) = match status {
        text_input::Status::Focused { .. } => (AMBER, 1.0),
        text_input::Status::Hovered => (HAIRLINE_STRONG, 1.0),
        _ => (HAIRLINE, 1.0),
    };
    text_input::Style {
        background: Background::Color(BACKGROUND),
        border: Border {
            color: line,
            width,
            radius: RADIUS.into(),
        },
        icon: MUTED,
        placeholder: FAINT,
        value: TEXT,
        selection: with_alpha(ACCENT, 0.35),
    }
}

pub fn editor(_theme: &Theme, status: text_editor::Status) -> text_editor::Style {
    let (line, width) = match status {
        text_editor::Status::Focused { .. } => (AMBER, 1.0),
        text_editor::Status::Hovered => (HAIRLINE_STRONG, 1.0),
        _ => (HAIRLINE, 1.0),
    };
    text_editor::Style {
        background: Background::Color(BACKGROUND),
        border: Border {
            color: line,
            width,
            radius: RADIUS.into(),
        },
        placeholder: FAINT,
        value: TEXT,
        selection: with_alpha(ACCENT, 0.35),
    }
}

pub fn dropdown(_theme: &Theme, status: pick_list::Status) -> pick_list::Style {
    let line = match status {
        pick_list::Status::Active => HAIRLINE,
        _ => HAIRLINE_STRONG,
    };
    pick_list::Style {
        text_color: TEXT,
        placeholder_color: MUTED,
        handle_color: MUTED,
        background: Background::Color(BACKGROUND),
        border: border(line),
    }
}
