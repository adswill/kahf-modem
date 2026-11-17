//! The three coded versions of the Operator layout, picked at start-up by KAHF_VARIANT (a, b or c).
//! They differ only in the accent colour and the density: row height and gaps. The layout is the same.
use eframe::egui::Color32;

pub struct Version {
    pub name: &'static str,
    /// The one accent colour, used for selection, focus, the current step and the spectrum trace.
    pub accent: Color32,
    /// Gap between widgets, in points. The layout's spacing grid is built from it.
    pub gap: f32,
    /// Height of a button and a table row, in points.
    pub row_h: f32,
}

/// a: muted green, comfortable density.
pub const A: Version = Version {
    name: "a",
    accent: Color32::from_rgb(0x6A, 0xA8, 0x7E),
    gap: 8.0,
    row_h: 26.0,
};

/// b: cool grey, dense.
pub const B: Version = Version {
    name: "b",
    accent: Color32::from_rgb(0x9A, 0xA9, 0xB8),
    gap: 6.0,
    row_h: 22.0,
};

/// c: amber, very dense.
pub const C: Version = Version {
    name: "c",
    accent: Color32::from_rgb(0xD9, 0xA4, 0x41),
    gap: 4.0,
    row_h: 18.0,
};

impl Version {
    /// Reads KAHF_VARIANT. Anything other than b or c gives version a.
    pub fn from_env() -> &'static Version {
        match std::env::var("KAHF_VARIANT")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "b" => &B,
            "c" => &C,
            _ => &A,
        }
    }
}
