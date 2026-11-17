//! Icons and small gauges, all drawn as vector paths on an iced canvas.
//! Icons live on a 24 x 24 grid and are stroked with round caps.

use std::collections::VecDeque;

use iced::widget::canvas::{self, path::Arc, stroke, Geometry, Path, Stroke};
use iced::{mouse, Color, Length, Point, Radians, Rectangle, Renderer, Size, Theme};

use crate::app::Message;
use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Operate,
    Stations,
    Messages,
    Mail,
    Files,
    Relay,
    Logbook,
    Map,
    Settings,
    Clock,
    Paperclip,
    Send,
    Antenna,
    Wave,
}

struct IconProgram {
    icon: Icon,
    color: Color,
}

pub fn icon<'a>(icon: Icon, size: f32, color: Color) -> iced::Element<'a, Message> {
    canvas::Canvas::new(IconProgram { icon, color })
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .into()
}

impl canvas::Program<Message> for IconProgram {
    type State = ();

    fn draw(&self, _: &(), renderer: &Renderer, _: &Theme, bounds: Rectangle, _: mouse::Cursor) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let k = bounds.width.min(bounds.height) / 24.0;
        draw_icon(&mut frame, self.icon, k, self.color);
        vec![frame.into_geometry()]
    }
}

fn pen(color: Color, k: f32) -> Stroke<'static> {
    Stroke {
        style: stroke::Style::Solid(color),
        width: (1.7 * k).max(1.2),
        line_cap: stroke::LineCap::Round,
        line_join: stroke::LineJoin::Round,
        ..Stroke::default()
    }
}

fn arc(b: &mut canvas::path::Builder, c: (f32, f32), r: f32, a0: f32, a1: f32, k: f32) {
    b.arc(Arc {
        center: Point::new(c.0 * k, c.1 * k),
        radius: r * k,
        start_angle: Radians(a0.to_radians()),
        end_angle: Radians(a1.to_radians()),
    });
}

fn draw_icon(f: &mut canvas::Frame, icon: Icon, k: f32, color: Color) {
    let p = |x: f32, y: f32| Point::new(x * k, y * k);
    let line = |f: &mut canvas::Frame, a: (f32, f32), b: (f32, f32)| {
        f.stroke(&Path::line(p(a.0, a.1), p(b.0, b.1)), pen(color, k));
    };
    let poly = |f: &mut canvas::Frame, pts: &[(f32, f32)], close: bool| {
        let path = Path::new(|b| {
            b.move_to(p(pts[0].0, pts[0].1));
            for q in &pts[1..] {
                b.line_to(p(q.0, q.1));
            }
            if close {
                b.close();
            }
        });
        f.stroke(&path, pen(color, k));
    };
    let ring = |f: &mut canvas::Frame, c: (f32, f32), r: f32| {
        f.stroke(&Path::circle(p(c.0, c.1), r * k), pen(color, k));
    };
    let dot = |f: &mut canvas::Frame, c: (f32, f32), r: f32| {
        f.fill(&Path::circle(p(c.0, c.1), r * k), color);
    };
    let rrect = |f: &mut canvas::Frame, x: f32, y: f32, w: f32, h: f32, r: f32| {
        f.stroke(&Path::rounded_rectangle(p(x, y), Size::new(w * k, h * k), (r * k).into()), pen(color, k));
    };
    let arc_stroke = |f: &mut canvas::Frame, c: (f32, f32), r: f32, a0: f32, a1: f32| {
        let path = Path::new(|b| arc(b, c, r, a0, a1, k));
        f.stroke(&path, pen(color, k));
    };

    match icon {
        Icon::Operate => {
            // waveform bars
            for (x, h) in [(4.0, 3.0), (8.0, 8.0), (12.0, 13.0), (16.0, 7.0), (20.0, 3.0)] {
                line(f, (x, 12.0 - h / 2.0), (x, 12.0 + h / 2.0));
            }
        }
        Icon::Stations => {
            // radio mast with waves
            line(f, (12.0, 11.0), (12.0, 21.0));
            poly(f, &[(8.0, 21.0), (12.0, 13.0), (16.0, 21.0)], false);
            dot(f, (12.0, 9.0), 1.6);
            arc_stroke(f, (12.0, 9.0), 4.5, 150.0, 210.0);
            arc_stroke(f, (12.0, 9.0), 4.5, -30.0, 30.0);
            arc_stroke(f, (12.0, 9.0), 8.0, 150.0, 210.0);
            arc_stroke(f, (12.0, 9.0), 8.0, -30.0, 30.0);
        }
        Icon::Messages => {
            let path = Path::new(|b| {
                b.move_to(p(5.0, 5.0));
                b.line_to(p(19.0, 5.0));
                b.quadratic_curve_to(p(21.0, 5.0), p(21.0, 7.0));
                b.line_to(p(21.0, 14.0));
                b.quadratic_curve_to(p(21.0, 16.0), p(19.0, 16.0));
                b.line_to(p(11.0, 16.0));
                b.line_to(p(6.5, 20.0));
                b.line_to(p(6.5, 16.0));
                b.line_to(p(5.0, 16.0));
                b.quadratic_curve_to(p(3.0, 16.0), p(3.0, 14.0));
                b.line_to(p(3.0, 7.0));
                b.quadratic_curve_to(p(3.0, 5.0), p(5.0, 5.0));
                b.close();
            });
            f.stroke(&path, pen(color, k));
            line(f, (7.5, 9.5), (16.5, 9.5));
            line(f, (7.5, 12.5), (13.0, 12.5));
        }
        Icon::Mail => {
            rrect(f, 3.0, 5.5, 18.0, 13.0, 2.0);
            poly(f, &[(3.5, 7.0), (12.0, 13.5), (20.5, 7.0)], false);
        }
        Icon::Files => {
            poly(f, &[(6.0, 3.0), (14.0, 3.0), (19.0, 8.0), (19.0, 21.0), (6.0, 21.0)], true);
            poly(f, &[(14.0, 3.0), (14.0, 8.0), (19.0, 8.0)], false);
            line(f, (9.0, 13.0), (16.0, 13.0));
            line(f, (9.0, 16.5), (16.0, 16.5));
        }
        Icon::Relay => {
            ring(f, (5.0, 12.0), 2.6);
            ring(f, (19.0, 12.0), 2.6);
            line(f, (8.0, 12.0), (16.0, 12.0));
            poly(f, &[(13.0, 9.0), (16.0, 12.0), (13.0, 15.0)], false);
            arc_stroke(f, (12.0, 12.0), 8.0, 215.0, 325.0);
        }
        Icon::Logbook => {
            rrect(f, 5.0, 3.5, 14.0, 17.0, 2.0);
            line(f, (9.0, 3.5), (9.0, 20.5));
            line(f, (12.0, 8.0), (16.0, 8.0));
            line(f, (12.0, 11.5), (16.0, 11.5));
        }
        Icon::Map => {
            let pin = Path::new(|b| {
                b.move_to(p(12.0, 21.0));
                b.bezier_curve_to(p(6.0, 15.0), p(5.0, 12.0), p(5.0, 9.5));
                b.bezier_curve_to(p(5.0, 5.5), p(8.0, 3.0), p(12.0, 3.0));
                b.bezier_curve_to(p(16.0, 3.0), p(19.0, 5.5), p(19.0, 9.5));
                b.bezier_curve_to(p(19.0, 12.0), p(18.0, 15.0), p(12.0, 21.0));
                b.close();
            });
            f.stroke(&pin, pen(color, k));
            ring(f, (12.0, 9.5), 2.6);
        }
        Icon::Settings => {
            line(f, (4.0, 7.0), (20.0, 7.0));
            line(f, (4.0, 12.0), (20.0, 12.0));
            line(f, (4.0, 17.0), (20.0, 17.0));
            for (x, y) in [(9.0, 7.0), (15.0, 12.0), (8.0, 17.0)] {
                f.fill(&Path::circle(p(x, y), 2.6 * k), theme::PANEL);
                ring(f, (x, y), 2.6);
            }
        }
        Icon::Clock => {
            ring(f, (12.0, 12.0), 8.5);
            poly(f, &[(12.0, 7.0), (12.0, 12.0), (15.5, 14.0)], false);
        }
        Icon::Paperclip => {
            let clip = Path::new(|b| {
                b.move_to(p(16.5, 8.0));
                b.line_to(p(9.0, 15.5));
                b.bezier_curve_to(p(7.5, 17.0), p(9.5, 19.5), p(11.5, 17.5));
                b.line_to(p(18.5, 10.5));
                b.bezier_curve_to(p(21.0, 8.0), p(17.5, 4.5), p(15.0, 7.0));
                b.line_to(p(7.5, 14.5));
                b.bezier_curve_to(p(4.0, 18.0), p(8.0, 22.0), p(11.5, 18.5));
            });
            f.stroke(&clip, pen(color, k));
        }
        Icon::Send => {
            poly(f, &[(3.5, 4.5), (21.0, 12.0), (3.5, 19.5), (7.0, 12.0)], true);
            line(f, (7.0, 12.0), (14.0, 12.0));
        }
        Icon::Antenna => {
            line(f, (12.0, 10.0), (12.0, 21.0));
            poly(f, &[(7.5, 21.0), (12.0, 12.0), (16.5, 21.0)], false);
            line(f, (9.2, 17.0), (14.8, 17.0));
            ring(f, (12.0, 8.0), 1.8);
            for r in [5.0, 8.5] {
                arc_stroke(f, (12.0, 8.0), r, 140.0, 220.0);
                arc_stroke(f, (12.0, 8.0), r, -40.0, 40.0);
            }
        }
        Icon::Wave => {
            let w = Path::new(|b| {
                b.move_to(p(2.0, 12.0));
                b.bezier_curve_to(p(5.0, 3.0), p(8.0, 3.0), p(11.0, 12.0));
                b.bezier_curve_to(p(14.0, 21.0), p(17.0, 21.0), p(22.0, 12.0));
            });
            f.stroke(&w, pen(color, k));
            f.stroke(
                &Path::line(p(2.0, 21.5), p(22.0, 21.5)),
                Stroke { style: stroke::Style::Solid(theme::with_alpha(color, 0.45)), width: 1.0, ..Stroke::default() },
            );
        }
    }
}

// ------------------------------------------------------------------ status dot

/// A status dot. With `pulse` (0..1) a soft ring breathes around it.
pub struct Dot {
    pub color: Color,
    pub pulse: Option<f32>,
}

pub fn dot<'a>(color: Color, pulse: Option<f32>) -> iced::Element<'a, Message> {
    canvas::Canvas::new(Dot { color, pulse }).width(Length::Fixed(14.0)).height(Length::Fixed(14.0)).into()
}

impl canvas::Program<Message> for Dot {
    type State = ();

    fn draw(&self, _: &(), renderer: &Renderer, _: &Theme, bounds: Rectangle, _: mouse::Cursor) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let c = Point::new(bounds.width / 2.0, bounds.height / 2.0);
        if let Some(p) = self.pulse {
            frame.fill(&Path::circle(c, 3.0 + 4.0 * p), theme::with_alpha(self.color, 0.30 * (1.0 - p)));
        }
        frame.fill(&Path::circle(c, 3.2), self.color);
        vec![frame.into_geometry()]
    }
}

// ----------------------------------------------------------------------- gauges

/// Audio level bar with a peak-hold tick.
pub struct LevelBar {
    pub level: f32,
    pub peak: f32,
}

impl canvas::Program<Message> for LevelBar {
    type State = ();

    fn draw(&self, _: &(), renderer: &Renderer, _: &Theme, bounds: Rectangle, _: mouse::Cursor) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let (w, h) = (bounds.width, bounds.height);
        frame.fill_rectangle(Point::ORIGIN, Size::new(w, h), theme::BACKGROUND);
        let lvl = self.level.clamp(0.0, 1.0);
        let color = if lvl > 0.9 { theme::DANGER } else if lvl > 0.75 { theme::WARNING } else { theme::ACCENT };
        frame.fill_rectangle(Point::new(0.0, 0.0), Size::new(w * lvl, h), color);
        let px = (w * self.peak.clamp(0.0, 1.0)).clamp(1.0, w - 1.0);
        frame.fill_rectangle(Point::new(px - 1.0, 0.0), Size::new(2.0, h), theme::TEXT);
        frame.stroke(
            &Path::rectangle(Point::new(0.5, 0.5), Size::new(w - 1.0, h - 1.0)),
            Stroke::default().with_color(theme::HAIRLINE).with_width(1.0),
        );
        vec![frame.into_geometry()]
    }
}

/// SNR sparkline over the last 60 seconds (one sample per second).
pub struct Sparkline<'a> {
    pub data: &'a VecDeque<f32>,
}

impl canvas::Program<Message> for Sparkline<'_> {
    type State = ();

    fn draw(&self, _: &(), renderer: &Renderer, _: &Theme, bounds: Rectangle, _: mouse::Cursor) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let (w, h) = (bounds.width, bounds.height);
        frame.fill_rectangle(Point::ORIGIN, Size::new(w, h), theme::BACKGROUND);
        for frac in [0.25f32, 0.5, 0.75] {
            frame.stroke(
                &Path::line(Point::new(0.0, h * frac), Point::new(w, h * frac)),
                Stroke::default().with_color(theme::with_alpha(theme::HAIRLINE, 0.7)).with_width(1.0),
            );
        }
        let n = self.data.len();
        if n >= 2 {
            // Newest sample at the right edge; the 60 s window slides in from the left.
            let step = w / 59.0;
            let pt = |i: usize| {
                let x = w - (n - 1 - i) as f32 * step;
                let v = (self.data[i] / 36.0).clamp(0.0, 1.0);
                Point::new(x, h - 3.0 - v * (h - 6.0))
            };
            let area = Path::new(|b| {
                b.move_to(Point::new(pt(0).x, h));
                for i in 0..n {
                    b.line_to(pt(i));
                }
                b.line_to(Point::new(pt(n - 1).x, h));
                b.close();
            });
            frame.fill(&area, theme::with_alpha(theme::INFO, 0.15));
            let line = Path::new(|b| {
                b.move_to(pt(0));
                for i in 1..n {
                    b.line_to(pt(i));
                }
            });
            frame.stroke(&line, Stroke::default().with_color(theme::INFO).with_width(1.5));
            frame.fill(&Path::circle(pt(n - 1), 2.5), theme::INFO);
        }
        frame.stroke(
            &Path::rectangle(Point::new(0.5, 0.5), Size::new(w - 1.0, h - 1.0)),
            Stroke::default().with_color(theme::HAIRLINE).with_width(1.0),
        );
        vec![frame.into_geometry()]
    }
}
