//! Waterfall: one GPU texture, BINS wide and ROWS tall, used as a ring. A new row is uploaded alone
//! (one row of pixels) and the image is drawn in two pieces so the newest row is always at the top.
//! The texture is never rebuilt. The spectrum and the waterfall share `plot_rect`, so a bin sits at
//! the same x in both. Also holds the simulated source used when no engine is running.
use std::fmt::Write as _;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align2, Color32, ColorImage, FontId, Pos2, Rect, Sense, Stroke, TextureHandle, TextureOptions, Vec2,
};

/// Shortest gap between two repaints requested by new data: 25 frames per second.
const MIN_REPAINT_GAP: Duration = Duration::from_millis(40);
/// Colour scale tick labels, from DB_LOW to DB_HIGH in 20 dB steps. Constant text.
const SCALE_LABELS: [(f32, &str); 5] = [
    (-100.0, "-100"),
    (-80.0, "-80"),
    (-60.0, "-60"),
    (-40.0, "-40"),
    (-20.0, "-20"),
];
/// Axis labels, one per kHz of the 0 to 4 kHz span.
const AXIS_LABELS: [&str; 5] = ["0 kHz", "1 kHz", "2 kHz", "3 kHz", "4 kHz"];

/// Rows kept. One texture row each.
pub const ROWS: usize = 280;
/// Columns in the texture. Engine rows are resampled to this width.
pub const BINS: usize = 512;
/// Bins per simulated row, the same width the engine sends.
const SIM_BINS: usize = 256;
/// Audio spans 0 to 4 kHz (see the spectrum note in kahf-daemon).
pub const SPAN_KHZ: f32 = 4.0;
/// Height of the shared frequency axis between the spectrum and the waterfall.
pub const AXIS_H: f32 = 24.0;
/// Left gutter and right padding of the plot area. Used by the spectrum, the axis and the waterfall alike.
pub const GUTTER_W: f32 = 56.0;
pub const RIGHT_PAD: f32 = 8.0;
/// Colour scale window in dBFS.
pub const DB_LOW: f32 = -100.0;
pub const DB_HIGH: f32 = -20.0;
/// Entries in the colour look-up table.
const LUT_LEN: usize = 256;
const SCALE_STEPS: usize = 40;
const BACKGROUND: Color32 = Color32::from_rgb(8, 10, 14);
const TAG_TEXT: Color32 = Color32::from_rgb(220, 222, 232);

/// Colour stops of the map, position 0 (quiet) to 1 (loud): dark blue, blue, cyan, yellow, white.
const STOPS: [(f32, [u8; 3]); 5] = [
    (0.0, [4, 8, 40]),
    (0.3, [20, 60, 160]),
    (0.5, [0, 190, 220]),
    (0.75, [250, 220, 60]),
    (1.0, [255, 255, 255]),
];

/// The plot area inside a row that is `area` wide. Same x range for the spectrum and the waterfall.
pub fn plot_rect(area: Rect) -> Rect {
    Rect::from_min_max(
        Pos2::new(area.left() + GUTTER_W, area.top()),
        Pos2::new(area.right() - RIGHT_PAD, area.bottom()),
    )
}

fn colour_at(t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    for pair in STOPS.windows(2) {
        let (t0, c0) = pair[0];
        let (t1, c1) = pair[1];
        if t <= t1 {
            let k = (t - t0) / (t1 - t0);
            let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * k).round() as u8;
            return Color32::from_rgb(mix(c0[0], c1[0]), mix(c0[1], c1[1]), mix(c0[2], c1[2]));
        }
    }
    let [r, g, b] = STOPS[STOPS.len() - 1].1;
    Color32::from_rgb(r, g, b)
}

pub struct Waterfall {
    texture: Option<TextureHandle>,
    /// Texture row of the newest spectrum row. Older rows follow it downwards, wrapping at ROWS.
    base: usize,
    /// Rows received so far, up to ROWS.
    filled: usize,
    lut: Vec<Color32>,
    /// Resampled row, reused for every push.
    scratch: Vec<f32>,
    /// Click position as a fraction of the 0 to 4 kHz span.
    marker: Option<f32>,
    /// Text of the click readout, rebuilt in place.
    marker_label: String,
    /// When the last data-driven repaint was requested, for the 25 fps cap.
    last_request: Option<Instant>,
}

impl Default for Waterfall {
    fn default() -> Self {
        Self::new()
    }
}

impl Waterfall {
    pub fn new() -> Self {
        let lut = (0..LUT_LEN)
            .map(|i| colour_at(i as f32 / (LUT_LEN - 1) as f32))
            .collect();
        Waterfall {
            texture: None,
            base: 0,
            filled: 0,
            lut,
            scratch: Vec::with_capacity(BINS),
            marker: None,
            marker_label: String::with_capacity(16),
            last_request: None,
        }
    }

    fn level_colour(&self, db: f32) -> Color32 {
        let t = ((db - DB_LOW) / (DB_HIGH - DB_LOW)).clamp(0.0, 1.0);
        self.lut[(t * (LUT_LEN - 1) as f32).round() as usize]
    }

    /// Writes one spectrum row (dBFS per bin) as the newest row and uploads only that row.
    /// Any width is resampled to BINS.
    pub fn push_row(&mut self, ctx: &egui::Context, values: &[f32]) {
        self.resample(values);
        let row: Vec<Color32> = self.scratch.iter().map(|&db| self.level_colour(db)).collect();
        let base = (self.base + ROWS - 1) % ROWS;
        self.base = base;
        self.filled = (self.filled + 1).min(ROWS);
        let texture = self.texture.get_or_insert_with(|| {
            let blank = ColorImage::new([BINS, ROWS], vec![BACKGROUND; BINS * ROWS]);
            ctx.load_texture("waterfall", blank, TextureOptions::LINEAR)
        });
        texture.set_partial([0, base], ColorImage::new([BINS, 1], row), TextureOptions::LINEAR);
        self.request_repaint(ctx);
    }

    /// Asks for a repaint for new data, at most 25 times a second. A faster feed is held to that cap.
    fn request_repaint(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        match self.last_request {
            Some(at) if now.duration_since(at) < MIN_REPAINT_GAP => {
                ctx.request_repaint_after(MIN_REPAINT_GAP - now.duration_since(at));
            }
            _ => {
                self.last_request = Some(now);
                ctx.request_repaint();
            }
        }
    }

    /// Linear resampling of one row into `scratch`. An empty row is the floor level.
    fn resample(&mut self, values: &[f32]) {
        self.scratch.clear();
        if values.is_empty() {
            self.scratch.resize(BINS, DB_LOW);
            return;
        }
        let last = values.len() - 1;
        for i in 0..BINS {
            let pos = i as f32 * last as f32 / (BINS - 1) as f32;
            let lo = pos.floor() as usize;
            let hi = (lo + 1).min(last);
            let frac = pos - lo as f32;
            self.scratch.push(values[lo] + (values[hi] - values[lo]) * frac);
        }
    }

    /// Draws the waterfall with its colour scale in the left gutter. `note` is a short tag such as
    /// "No radio running"; an empty note draws nothing.
    /// Draws the waterfall at the given height, over the full width of `ui`. The rows are scaled to fit.
    pub fn show(&mut self, ui: &mut egui::Ui, note: &str, height: f32) {
        let width = ui.available_width();
        let (area, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
        let view = plot_rect(area);
        if response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                if view.contains(pos) {
                    self.marker = Some(((pos.x - view.left()) / view.width()).clamp(0.0, 1.0));
                }
            }
        }

        let painter = ui.painter_at(area);
        painter.rect_filled(area, 0.0, BACKGROUND);
        if let Some(tex) = &self.texture {
            // Display row i shows texture row (base + i) mod ROWS. Draw the rows from base to the
            // end first, then the rows from 0 to base.
            let id = tex.id();
            let h = view.height();
            let base = self.base;
            let cut = view.top() + h * (ROWS - base) as f32 / ROWS as f32;
            let uv = |v0: f32, v1: f32| Rect::from_min_max(Pos2::new(0.0, v0), Pos2::new(1.0, v1));
            let part = |top: f32, bottom: f32| {
                Rect::from_min_max(Pos2::new(view.left(), top), Pos2::new(view.right(), bottom))
            };
            let split = base as f32 / ROWS as f32;
            painter.image(id, part(view.top(), cut), uv(split, 1.0), Color32::WHITE);
            if base > 0 {
                painter.image(id, part(cut, view.bottom()), uv(0.0, split), Color32::WHITE);
            }
        }
        if self.filled == 0 {
            painter.text(
                view.center(),
                Align2::CENTER_CENTER,
                "No signal data yet.",
                FontId::proportional(14.0),
                TAG_TEXT,
            );
        }

        // Colour scale: low at the bottom, high at the top. Ticks and labels every 20 dB.
        let bar_left = area.left() + 4.0;
        let bar_right = area.left() + 12.0;
        let bar_top = area.top() + 8.0;
        let bar_bottom = area.bottom() - 8.0;
        let bar_h = bar_bottom - bar_top;
        let slice = bar_h / SCALE_STEPS as f32;
        for i in 0..SCALE_STEPS {
            let t = i as f32 / (SCALE_STEPS - 1) as f32;
            let bottom = bar_bottom - slice * i as f32;
            let cell = Rect::from_min_max(
                Pos2::new(bar_left, bottom - slice - 0.5),
                Pos2::new(bar_right, bottom),
            );
            painter.rect_filled(cell, 0.0, colour_at(t));
        }
        let font = FontId::monospace(13.0);
        let weak = ui.visuals().weak_text_color();
        for (db, label) in SCALE_LABELS {
            let t = (db - DB_LOW) / (DB_HIGH - DB_LOW);
            let y = bar_bottom - t * bar_h;
            painter.line_segment(
                [Pos2::new(bar_left - 2.0, y), Pos2::new(bar_left, y)],
                Stroke::new(1.0, weak),
            );
            painter.text(
                Pos2::new(bar_right + 4.0, y),
                Align2::LEFT_CENTER,
                label,
                font.clone(),
                weak,
            );
        }

        if !note.is_empty() {
            let tag = Rect::from_min_size(view.left_top() + Vec2::new(8.0, 8.0), Vec2::new(236.0, 20.0));
            painter.rect_filled(tag, 2.0, BACKGROUND);
            painter.text(
                tag.left_center() + Vec2::new(4.0, 0.0),
                Align2::LEFT_CENTER,
                note,
                FontId::proportional(13.0),
                TAG_TEXT,
            );
        }
        if let Some(frac) = self.marker {
            let x = view.left() + frac * view.width();
            painter.line_segment(
                [Pos2::new(x, view.top()), Pos2::new(x, view.bottom())],
                Stroke::new(1.0, Color32::WHITE),
            );
            let left = if x + 110.0 > view.right() {
                x - 102.0
            } else {
                x + 6.0
            };
            let tag = Rect::from_min_size(Pos2::new(left, view.top() + 8.0), Vec2::new(96.0, 20.0));
            painter.rect_filled(tag, 2.0, BACKGROUND);
            self.marker_label.clear();
            let _ = write!(self.marker_label, "{:.3} kHz", frac * SPAN_KHZ);
            painter.text(
                tag.left_center() + Vec2::new(4.0, 0.0),
                Align2::LEFT_CENTER,
                self.marker_label.as_str(),
                font,
                TAG_TEXT,
            );
        }
    }
}

/// The frequency axis, drawn once between the spectrum and the waterfall. A tick every half kHz,
/// a label every kHz, on the same x range as the plots.
pub fn axis(ui: &mut egui::Ui) {
    let width = ui.available_width();
    let (area, _) = ui.allocate_exact_size(Vec2::new(width, AXIS_H), Sense::hover());
    let painter = ui.painter_at(area);
    let plot = plot_rect(area);
    let weak = ui.visuals().weak_text_color();
    let tick = ui.visuals().widgets.noninteractive.bg_stroke.color;
    let font = FontId::monospace(13.0);
    let whole = SPAN_KHZ as usize;
    for half in 0..=(2 * whole) {
        let x = plot.left() + plot.width() * half as f32 / (2 * whole) as f32;
        let len = if half % 2 == 0 { 6.0 } else { 3.0 };
        painter.line_segment(
            [Pos2::new(x, area.top()), Pos2::new(x, area.top() + len)],
            Stroke::new(1.0, tick),
        );
        if half % 2 == 0 {
            let k = half / 2;
            let anchor = match k {
                0 => Align2::LEFT_TOP,
                _ if k == whole => Align2::RIGHT_TOP,
                _ => Align2::CENTER_TOP,
            };
            painter.text(
                Pos2::new(x, area.top() + 8.0),
                anchor,
                AXIS_LABELS[k],
                font.clone(),
                weak,
            );
        }
    }
}

/// Fake spectrum source used when no engine is running: a noise floor with two drifting carriers.
pub struct Simulated {
    seq: u64,
    state: u64,
}

impl Default for Simulated {
    fn default() -> Self {
        Self::new()
    }
}

impl Simulated {
    pub fn new() -> Self {
        Simulated {
            seq: 0,
            state: 0x9E37_79B9_7F4A_7C15,
        }
    }

    /// Uniform value in [0, 1) from a xorshift generator.
    fn uniform(&mut self) -> f32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        (self.state >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Writes the next spectrum row in dBFS, SIM_BINS wide, into `out` (reused by the caller).
    pub fn next_row(&mut self, out: &mut Vec<f32>) {
        self.seq += 1;
        let t = self.seq as f32;
        let carrier_a = 150.0 + 70.0 * (t * 0.03).sin();
        let carrier_b = 60.0 + 25.0 * (t * 0.017 + 1.0).sin();
        out.clear();
        for k in 0..SIM_BINS {
            let n: f32 = (0..4).map(|_| self.uniform()).sum::<f32>() - 2.0;
            let floor_db = -88.0 + 5.0 * n;
            let linear = 10f32.powf(floor_db / 10.0)
                + 10f32.powf(carrier_db(k, carrier_a, -38.0) / 10.0)
                + 10f32.powf(carrier_db(k, carrier_b, -52.0) / 10.0);
            out.push(10.0 * linear.log10());
        }
    }
}

/// Level of a carrier at one bin: a peak that falls off quickly away from its centre.
fn carrier_db(bin: usize, centre: f32, peak: f32) -> f32 {
    let d = (bin as f32 - centre) / 1.5;
    peak - 4.0 * d * d
}
