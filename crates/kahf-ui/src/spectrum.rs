//! Spectrum canvas, waterfall ring buffer and the simulated input.
//!
//! The spectrum and the waterfall share one bin-to-x mapping: bin i of
//! `BINS` sits at x = i / BINS of the width. All buffers are allocated once.

use iced::mouse;
use iced::widget::canvas::{self, Action, Geometry, Path, Stroke};
use iced::widget::image;
use iced::{Event, Font, Point, Rectangle, Renderer, Size, Theme};

use crate::app::Message;
use crate::theme;

pub const BINS: usize = 1024;
pub const WF_COLS: usize = 400;
pub const WF_ROWS: usize = 300;
pub const DB_MIN: f32 = -115.0;
pub const DB_MAX: f32 = -45.0;
/// Waterfall colour window: wide enough that weak and strong signals use the whole ramp.
const WF_DB_MIN: f32 = -111.0;
const WF_DB_MAX: f32 = -62.0;
pub const SPAN_HZ: f64 = 12_000.0;
pub const CENTRE_HZ: f64 = 7_078_000.0;
pub const BANDWIDTH_HZ: f64 = 2_400.0;
pub const FRAMES_PER_SECOND: u64 = 25;

const MONO: Font = Font::MONOSPACE;

/// Frequency in Hz at a fraction 0..1 of the span.
pub fn hz_at(frac: f32) -> f64 {
    CENTRE_HZ - SPAN_HZ / 2.0 + f64::from(frac) * SPAN_HZ
}

/// "7 078.000" style: kHz with a space grouping the thousands.
pub fn khz_text(hz: f64) -> String {
    let total = (hz.max(0.0) + 0.5) as u64; // rounded to whole Hz
    let (khz, rest) = (total / 1000, total % 1000);
    if khz >= 1000 {
        format!("{} {:03}.{:03}", khz / 1000, khz % 1000, rest)
    } else {
        format!("{khz}.{rest:03}")
    }
}

// ---------------------------------------------------------------- simulation

pub struct Sim {
    rng: u64,
    t: f32,
    pub peak_bin: usize,
    pub peak_db: f32,
}

impl Sim {
    pub fn new() -> Self {
        Self { rng: 0x9E37_79B9_7F4A_7C15, t: 0.0, peak_bin: 0, peak_db: DB_MIN }
    }

    fn noise(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Writes the next simulated frame into `out` (length `BINS`), no allocation.
    pub fn step(&mut self, out: &mut [f32]) {
        self.t += 1.0 / FRAMES_PER_SECOND as f32;
        let burst_on = (self.t * 0.7).sin() > -0.35;
        let drift = 0.5 + 0.12 * (self.t * 0.21).sin();
        let mut peak = (0usize, DB_MIN);
        for (i, o) in out.iter_mut().enumerate() {
            let x = i as f32 / BINS as f32;
            let mut db = -106.0 + (self.noise() + self.noise() - 1.0) * 5.0;
            // Slow band-limited noise bump from the receiver filter shape.
            db += 4.0 * (1.0 - (2.0 * (x - 0.5)).powi(2));
            // Three narrow carriers.
            for (pos, lvl) in [(0.17f32, -80.0f32), (0.83, -88.0), (0.69, -92.0)] {
                let d = (x - pos) / 0.0035;
                db = db.max(lvl - 14.0 * d * d + self.noise() * 2.0);
            }
            // A data burst about 2.4 kHz wide that drifts a little.
            if burst_on {
                let half = (BANDWIDTH_HZ / SPAN_HZ) as f32 / 2.0;
                let edge = ((x - (drift - half)).min((drift + half) - x) / 0.006).clamp(0.0, 1.0);
                db = db.max(-98.0 + 20.0 * edge + (self.noise() - 0.5) * 7.0 * edge - 20.0 * (1.0 - edge));
            }
            // Smooth against the previous frame so the trace is calm.
            *o = *o * 0.55 + db * 0.45;
            if *o > peak.1 {
                peak = (i, *o);
            }
        }
        self.peak_bin = peak.0;
        self.peak_db = peak.1;
    }
}

// ------------------------------------------------------------------ waterfall

/// Ring buffer of 300 colour-mapped rows. Pushing a row writes into place;
/// no row is ever moved.
pub struct Waterfall {
    px: Vec<u8>,
    head: usize,
    lut: Vec<[u8; 4]>,
}

impl Waterfall {
    pub fn new() -> Self {
        // Dark blue, teal, yellow, white: lightness rises evenly along the ramp.
        const STOPS: [(f32, [f32; 3]); 5] = [
            (0.0, [7.0, 10.0, 36.0]),
            (0.28, [20.0, 48.0, 112.0]),
            (0.52, [26.0, 140.0, 148.0]),
            (0.78, [236.0, 204.0, 72.0]),
            (1.0, [255.0, 253.0, 240.0]),
        ];
        let lut = (0..256)
            .map(|i| {
                let v = i as f32 / 255.0;
                let k = STOPS.iter().rposition(|s| s.0 <= v).unwrap_or(0).min(STOPS.len() - 2);
                let (a, b) = (STOPS[k], STOPS[k + 1]);
                let f = (v - a.0) / (b.0 - a.0);
                let c = |j: usize| (a.1[j] + (b.1[j] - a.1[j]) * f) as u8;
                [c(0), c(1), c(2), 255]
            })
            .collect();
        let mut wf = Self { px: vec![0; WF_ROWS * WF_COLS * 4], head: 0, lut };
        for p in wf.px.as_chunks_mut::<4>().0 {
            *p = [7, 10, 36, 255];
        }
        wf
    }

    pub fn push(&mut self, bins: &[f32]) {
        self.head = (self.head + WF_ROWS - 1) % WF_ROWS;
        let row = &mut self.px[self.head * WF_COLS * 4..(self.head + 1) * WF_COLS * 4];
        for (c, out) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let lo = c * BINS / WF_COLS;
            let hi = ((c + 1) * BINS / WF_COLS).max(lo + 1);
            let db = bins[lo..hi].iter().copied().fold(DB_MIN, f32::max);
            let v = ((db - WF_DB_MIN) / (WF_DB_MAX - WF_DB_MIN)).clamp(0.0, 1.0);
            *out = self.lut[(v * 255.0) as usize];
        }
    }

    /// One copy of the ring, newest row first, for the GPU image.
    pub fn handle(&self) -> image::Handle {
        let split = self.head * WF_COLS * 4;
        let mut v = Vec::with_capacity(self.px.len());
        v.extend_from_slice(&self.px[split..]);
        v.extend_from_slice(&self.px[..split]);
        image::Handle::from_rgba(WF_COLS as u32, WF_ROWS as u32, v)
    }
}

// --------------------------------------------------------------------- canvas

fn text(content: String, at: Point, size: f32, color: iced::Color) -> canvas::Text {
    canvas::Text {
        content,
        position: at,
        color,
        size: size.into(),
        font: MONO,
        ..Default::default()
    }
}

fn click_to_tune(event: &Event, bounds: Rectangle, cursor: mouse::Cursor) -> Option<Action<Message>> {
    if let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event {
        let p = cursor.position_in(bounds)?;
        let f = (p.x / bounds.width).clamp(0.0, 1.0);
        return Some(Action::publish(Message::Tune(f)).and_capture());
    }
    None
}

/// Channel markers shown as dashed lines across both plots.
pub const MARKS: [(f64, &str); 2] = [(7_074_000.0, "Ch 1"), (7_081_000.0, "Ch 2")];

fn band_markers(frame: &mut canvas::Frame, size: Size, labels: bool) {
    for (hz, name) in MARKS {
        let x = ((hz - (CENTRE_HZ - SPAN_HZ / 2.0)) / SPAN_HZ) as f32 * size.width;
        let dash = canvas::LineDash { segments: &[3.0, 4.0], offset: 0 };
        frame.stroke(
            &Path::line(Point::new(x, 0.0), Point::new(x, size.height)),
            Stroke { line_dash: dash, ..Stroke::default().with_color(theme::with_alpha(theme::WARNING, 0.55)).with_width(1.0) },
        );
        if labels {
            let mut t = text(name.to_string(), Point::new(x + 4.0, 4.0), 10.0, theme::WARNING);
            t.align_x = iced::alignment::Horizontal::Left.into();
            frame.fill_text(t);
        }
    }
}

/// Time scale on the waterfall's left edge: newest row on top, one tick every 2 s.
fn time_scale(frame: &mut canvas::Frame, size: Size) {
    let total = WF_ROWS as f32 / FRAMES_PER_SECOND as f32;
    let mut s = 0.0f32;
    while s <= total + 0.01 {
        let y = (s / total * size.height).min(size.height - 1.0);
        frame.fill_rectangle(Point::new(0.0, y), Size::new(6.0, 1.0), theme::with_alpha(theme::TEXT, 0.6));
        let label = if s == 0.0 { "now".to_string() } else { format!("-{s:.0}s") };
        let ty = if s == 0.0 { y + 2.0 } else { (y - 12.0).max(1.0) };
        frame.fill_rectangle(Point::new(0.0, ty - 1.0), Size::new(34.0, 13.0), theme::with_alpha(theme::BACKGROUND, 0.6));
        frame.fill_text(text(label, Point::new(8.0, ty), 10.0, theme::TEXT));
        s += 2.0;
    }
}

fn passband(frame: &mut canvas::Frame, size: Size, tune: f32) {
    let half = (BANDWIDTH_HZ / SPAN_HZ) as f32 / 2.0;
    let x0 = ((tune - half) * size.width).max(0.0);
    let x1 = ((tune + half) * size.width).min(size.width);
    frame.fill_rectangle(
        Point::new(x0, 0.0),
        Size::new((x1 - x0).max(0.0), size.height),
        theme::with_alpha(theme::INFO, 0.12),
    );
    let x = tune * size.width;
    frame.stroke(
        &Path::line(Point::new(x, 0.0), Point::new(x, size.height)),
        Stroke::default().with_color(theme::TEXT).with_width(1.0),
    );
}

fn cursor_line(frame: &mut canvas::Frame, size: Size, cursor: mouse::Cursor, bounds: Rectangle, label: bool) {
    if let Some(p) = cursor.position_in(bounds) {
        frame.stroke(
            &Path::line(Point::new(p.x, 0.0), Point::new(p.x, size.height)),
            Stroke::default().with_color(theme::with_alpha(theme::TEXT, 0.35)).with_width(1.0),
        );
        if label {
            let hz = hz_at(p.x / size.width);
            let x = if p.x > size.width - 110.0 { p.x - 106.0 } else { p.x + 6.0 };
            frame.fill_text(text(format!("{} kHz", khz_text(hz)), Point::new(x, 20.0), 11.0, theme::TEXT));
        }
    }
}

/// Spectrum trace, grid, tuning marker, peak marker.
pub struct SpectrumPlot<'a> {
    pub data: &'a [f32],
    pub peak_bin: usize,
    pub tune: f32,
    pub grid: &'a canvas::Cache,
}

impl canvas::Program<Message> for SpectrumPlot<'_> {
    type State = ();

    fn update(&self, _: &mut (), event: &Event, bounds: Rectangle, cursor: mouse::Cursor) -> Option<Action<Message>> {
        click_to_tune(event, bounds, cursor)
    }

    fn mouse_interaction(&self, _: &(), bounds: Rectangle, cursor: mouse::Cursor) -> mouse::Interaction {
        if cursor.is_over(bounds) { mouse::Interaction::Crosshair } else { mouse::Interaction::default() }
    }

    fn draw(&self, _: &(), renderer: &Renderer, _: &Theme, bounds: Rectangle, cursor: mouse::Cursor) -> Vec<Geometry> {
        let size = bounds.size();
        let (w, h) = (size.width, size.height);
        let plot_h = h - 16.0; // bottom strip holds the frequency labels
        let y_of = |db: f32| plot_h * (1.0 - ((db - DB_MIN) / (DB_MAX - DB_MIN)).clamp(0.0, 1.0));

        let grid = self.grid.draw(renderer, size, |frame| {
            frame.fill_rectangle(Point::ORIGIN, size, theme::PANEL);
            let minor = Stroke::default().with_color(theme::with_alpha(theme::HAIRLINE, 0.55)).with_width(1.0);
            let major = Stroke::default().with_color(theme::HAIRLINE).with_width(1.0);
            let mut db = DB_MIN + 5.0;
            while db < DB_MAX {
                let y = y_of(db);
                let ten = ((db - DB_MIN) as i32) % 10 == 0;
                frame.stroke(&Path::line(Point::new(0.0, y), Point::new(w, y)), if ten { major } else { minor });
                if ((db - DB_MIN) as i32) % 20 == 0 {
                    frame.fill_text(text(format!("{db:.0}"), Point::new(4.0, y - 12.0), 10.0, theme::MUTED));
                }
                db += 5.0;
            }
            let kmax = (SPAN_HZ / 1000.0) as i32;
            for k in 0..=kmax {
                let frac = k as f32 / kmax as f32;
                let x = frac * w;
                frame.stroke(&Path::line(Point::new(x, 0.0), Point::new(x, plot_h)), major);
                let khz = hz_at(frac) / 1000.0;
                let (px, align) = if k == 0 {
                    (x + 3.0, iced::alignment::Horizontal::Left.into())
                } else if k == kmax {
                    (x - 3.0, iced::alignment::Horizontal::Right.into())
                } else {
                    (x, iced::alignment::Horizontal::Center.into())
                };
                let mut t = text(format!("{khz:.0}"), Point::new(px, plot_h + 3.0), 10.0, theme::MUTED);
                t.align_x = align;
                frame.fill_text(t);
            }
            frame.stroke(
                &Path::line(Point::new(0.0, plot_h), Point::new(w, plot_h)),
                Stroke::default().with_color(theme::MUTED).with_width(1.0),
            );
            band_markers(frame, Size::new(w, plot_h), true);
        });

        let mut frame = canvas::Frame::new(renderer, size);
        passband(&mut frame, Size::new(w, plot_h), self.tune);
        {
            let label = khz_text(hz_at(self.tune));
            let tx = self.tune * w;
            let tag_w = 70.0;
            let x0 = if tx + tag_w + 6.0 > w { tx - tag_w - 4.0 } else { tx + 4.0 };
            frame.fill_rectangle(Point::new(x0, 3.0), Size::new(tag_w, 15.0), theme::with_alpha(theme::RAISED, 0.92));
            frame.fill_text(text(label, Point::new(x0 + 5.0, 5.0), 11.0, theme::TEXT));
        }

        // Trace: one point per pixel column (max of the bins under it).
        let cols = (w as usize).clamp(2, 1200);
        let trace = Path::new(|b| {
            b.move_to(Point::new(0.0, plot_h));
            for c in 0..cols {
                let lo = c * BINS / cols;
                let hi = ((c + 1) * BINS / cols).max(lo + 1).min(self.data.len());
                let db = self.data[lo..hi].iter().copied().fold(DB_MIN, f32::max);
                b.line_to(Point::new(c as f32 / (cols - 1) as f32 * w, y_of(db)));
            }
            b.line_to(Point::new(w, plot_h));
            b.close();
        });
        frame.fill(&trace, theme::with_alpha(theme::ACCENT, 0.15));
        frame.stroke(&trace, Stroke::default().with_color(theme::ACCENT).with_width(1.0));

        // Peak marker: small triangle above the highest bin.
        let px = (self.peak_bin as f32 + 0.5) / BINS as f32 * w;
        let py = (y_of(self.data[self.peak_bin.min(self.data.len() - 1)]) - 4.0).max(6.0);
        let tri = Path::new(|b| {
            b.move_to(Point::new(px, py));
            b.line_to(Point::new(px - 4.0, py - 6.0));
            b.line_to(Point::new(px + 4.0, py - 6.0));
            b.close();
        });
        frame.fill(&tri, theme::WARNING);

        cursor_line(&mut frame, Size::new(w, plot_h), cursor, bounds, true);
        vec![grid, frame.into_geometry()]
    }
}

/// Tuning marker drawn over the waterfall image, same x mapping as the spectrum.
pub struct WaterfallMarker {
    pub tune: f32,
    pub scale: bool,
}

impl canvas::Program<Message> for WaterfallMarker {
    type State = ();

    fn update(&self, _: &mut (), event: &Event, bounds: Rectangle, cursor: mouse::Cursor) -> Option<Action<Message>> {
        click_to_tune(event, bounds, cursor)
    }

    fn mouse_interaction(&self, _: &(), bounds: Rectangle, cursor: mouse::Cursor) -> mouse::Interaction {
        if cursor.is_over(bounds) { mouse::Interaction::Crosshair } else { mouse::Interaction::default() }
    }

    fn draw(&self, _: &(), renderer: &Renderer, _: &Theme, bounds: Rectangle, cursor: mouse::Cursor) -> Vec<Geometry> {
        let size = bounds.size();
        let mut frame = canvas::Frame::new(renderer, size);
        band_markers(&mut frame, size, false);
        passband(&mut frame, size, self.tune);
        if self.scale {
            time_scale(&mut frame, size);
        }
        cursor_line(&mut frame, size, cursor, bounds, false);
        vec![frame.into_geometry()]
    }
}
