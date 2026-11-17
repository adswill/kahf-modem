//! Spectrum panel: the latest spectrum row as a line across the plot area, with dB grid lines and a
//! marker on the peak. Uses the same x range as the waterfall (`waterfall::plot_rect`) and draws no
//! frequency labels of its own: the shared axis sits under it. Rows arrive about 10 times a second.
//! The trace is drawn segment by segment from the stored row, so a frame allocates no point buffer.
use std::fmt::Write as _;

use eframe::egui::{self, Align2, FontId, Pos2, Rect, Sense, Stroke, Vec2};

use crate::waterfall::{plot_rect, SPAN_KHZ};

const TOP_PAD: f32 = 8.0;
const BOTTOM_PAD: f32 = 8.0;
const DB_TOP: f32 = 0.0;
const DB_BOTTOM: f32 = -120.0;
const DB_STEP: f32 = 20.0;
/// dB grid labels, from DB_TOP down to DB_BOTTOM in DB_STEP steps. Constant text, so no per-frame strings.
const DB_LABELS: [&str; 7] = ["0", "-20", "-40", "-60", "-80", "-100", "-120"];
/// Vertical grid lines every half kHz.
const FREQ_LINES: usize = 8;
const LABEL_SIZE: f32 = 13.0;

pub struct Spectrum {
    /// Latest row, dBFS per bin. Capacity is kept between rows.
    latest: Vec<f32>,
    /// Text of the peak label, rebuilt in place each frame.
    peak_label: String,
}

impl Default for Spectrum {
    fn default() -> Self {
        Spectrum {
            latest: Vec::new(),
            peak_label: String::with_capacity(48),
        }
    }
}

impl Spectrum {
    /// Replaces the shown row with a new one (dBFS per bin, 0 to 4 kHz). Reuses the buffer.
    pub fn push_row(&mut self, values: &[f32]) {
        self.latest.clear();
        self.latest.extend_from_slice(values);
    }

    /// Draws the spectrum at the given height, over the full width of `ui`.
    pub fn show(&mut self, ui: &mut egui::Ui, note: &str, height: f32) {
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
        let painter = ui.painter_at(rect);
        let visuals = ui.visuals();
        let grid = visuals.widgets.noninteractive.bg_stroke.color;
        let text = visuals.widgets.noninteractive.fg_stroke.color;
        let weak = visuals.weak_text_color();
        let field = visuals.extreme_bg_color;
        let font = FontId::monospace(LABEL_SIZE);
        let accent = visuals.hyperlink_color;

        painter.rect_filled(rect, 0.0, field);
        let full = plot_rect(rect);
        let plot = Rect::from_min_max(
            Pos2::new(full.left(), rect.top() + TOP_PAD),
            Pos2::new(full.right(), rect.bottom() - BOTTOM_PAD),
        );
        let y_of = |db: f32| plot.top() + (DB_TOP - db) / (DB_TOP - DB_BOTTOM) * plot.height();
        let x_of = |frac: f32| plot.left() + frac * plot.width();

        // dB grid and labels, in the left gutter
        let faint = grid.gamma_multiply(0.5);
        let steps = ((DB_TOP - DB_BOTTOM) / DB_STEP) as usize;
        for (i, label) in DB_LABELS.iter().enumerate().take(steps + 1) {
            let y = y_of(DB_TOP - DB_STEP * i as f32);
            painter.line_segment(
                [Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)],
                Stroke::new(1.0, grid),
            );
            painter.text(
                Pos2::new(plot.left() - 6.0, y),
                Align2::RIGHT_CENTER,
                *label,
                font.clone(),
                weak,
            );
        }
        for i in 0..=FREQ_LINES {
            let x = x_of(i as f32 / FREQ_LINES as f32);
            painter.line_segment(
                [Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())],
                Stroke::new(1.0, faint),
            );
        }

        if self.latest.len() < 2 {
            painter.text(
                plot.center(),
                Align2::CENTER_CENTER,
                "No spectrum data yet.",
                FontId::proportional(14.0),
                text,
            );
        } else {
            // Bin k sits at k / (n - 1) across the plot, the same mapping the waterfall uses.
            let last = (self.latest.len() - 1) as f32;
            let stroke = Stroke::new(1.5, accent);
            let mut prev: Option<Pos2> = None;
            for (k, &db) in self.latest.iter().enumerate() {
                let p = Pos2::new(x_of(k as f32 / last), y_of(db.clamp(DB_BOTTOM, DB_TOP)));
                if let Some(q) = prev {
                    painter.line_segment([q, p], stroke);
                }
                prev = Some(p);
            }

            // Peak marker: the bin with the highest level, labelled in kHz and dB.
            let (peak_bin, &peak_db) = self
                .latest
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .unwrap_or((0, &DB_BOTTOM));
            let frac = peak_bin as f32 / last;
            let p = Pos2::new(x_of(frac), y_of(peak_db.clamp(DB_BOTTOM, DB_TOP)));
            painter.line_segment(
                [Pos2::new(p.x, plot.top()), Pos2::new(p.x, plot.bottom())],
                Stroke::new(1.0, faint),
            );
            painter.circle_filled(p, 3.0, accent);
            let (anchor, dx) = if p.x > plot.center().x {
                (Align2::RIGHT_TOP, -6.0)
            } else {
                (Align2::LEFT_TOP, 6.0)
            };
            self.peak_label.clear();
            let _ = write!(
                self.peak_label,
                "Peak {:.3} kHz  {peak_db:.1} dB",
                frac * SPAN_KHZ
            );
            painter.text(
                Pos2::new(p.x + dx, plot.top() + 2.0),
                anchor,
                self.peak_label.as_str(),
                font.clone(),
                text,
            );
        }

        if !note.is_empty() {
            painter.text(
                Pos2::new(plot.left() + 6.0, plot.bottom() - 6.0),
                Align2::LEFT_BOTTOM,
                note,
                font,
                weak,
            );
        }
    }
}
