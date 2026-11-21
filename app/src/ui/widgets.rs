//! Things Alloy has no widget for, drawn on its canvas: the radio spectrum with its waterfall,
//! the signal meter and a voice note's waveform.

use alloy_ui::prelude::*;
use alloy_ui::{pt, rgb, Color, Rectangle, Size};

use super::kit::*;
use crate::app::Kahf;
use alloy_ui::Path;

const DB_MIN: f32 = -105.0;
const DB_MAX: f32 = -45.0;
const AXIS: f32 = 26.0;
/// The modem uses 312 Hz to 2690 Hz of the 0 to 4 kHz audio.
const SIGNAL_LO: f32 = 312.0;
const SIGNAL_HI: f32 = 2690.0;
const AUDIO_HZ: f32 = 4000.0;

fn rect(x: f32, y: f32, w: f32, h: f32) -> Rectangle {
    Rectangle::new(pt(x, y), Size::new(w, h))
}

/// A blue-to-yellow colour ramp for the waterfall, 0 to 1.
pub fn ramp(t: f32) -> Color {
    const STOPS: [[f32; 3]; 5] = [
        [6.0, 10.0, 40.0],
        [20.0, 60.0, 140.0],
        [20.0, 150.0, 150.0],
        [90.0, 200.0, 110.0],
        [250.0, 220.0, 70.0],
    ];
    let p = t.clamp(0.0, 1.0) * (STOPS.len() - 1) as f32;
    let k = (p.floor() as usize).min(STOPS.len() - 2);
    let f = p - k as f32;
    let c = |i: usize| (STOPS[k][i] + (STOPS[k + 1][i] - STOPS[k][i]) * f) / 255.0;
    Color {
        r: c(0),
        g: c(1),
        b: c(2),
        a: 1.0,
    }
}

fn level(db: f32) -> f32 {
    ((db - DB_MIN) / (DB_MAX - DB_MIN)).clamp(0.0, 1.0)
}

/// The live picture of the radio band: a line graph on top and a scrolling waterfall below.
pub fn spectrum(bins: Vec<f32>, history: Vec<Vec<f32>>, height: f32) -> V {
    canvas::<Kahf>(0.0, height, move |f| {
        let size = f.size();
        let (w, h) = (size.width, size.height);
        let pal = f.palette();
        let plot_h = ((h - AXIS) * 0.42).round();
        let wf_h = h - AXIS - plot_h;
        f.rect(rect(0.0, 0.0, w, h), pal.surface);
        let x = |hz: f32| hz / AUDIO_HZ * w;
        f.rect(
            rect(x(SIGNAL_LO), 0.0, x(SIGNAL_HI) - x(SIGNAL_LO), plot_h + wf_h),
            pal.accent.with_alpha(0.10),
        );
        f.stroke_rect(
            rect(x(SIGNAL_LO), 0.0, x(SIGNAL_HI) - x(SIGNAL_LO), plot_h + wf_h),
            0.0,
            1.0,
            pal.accent.with_alpha(0.6),
        );
        for db in (-100..=-50).step_by(10) {
            let y = plot_h - level(db as f32) * plot_h;
            f.line(pt(0.0, y), pt(w, y), 1.0, pal.border);
        }
        if bins.len() > 1 {
            let pts: Vec<_> = bins
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    pt(
                        i as f32 / (bins.len() - 1) as f32 * w,
                        plot_h - level(*v) * plot_h,
                    )
                })
                .collect();
            f.stroke(&Path::polyline(pts), 1.5, pal.accent);
        }
        // Waterfall: the newest row on top.
        let rows = history.len().max(1);
        let row_h = (wf_h / 90.0).max(1.0);
        let _ = rows;
        f.rect(rect(0.0, plot_h, w, wf_h), rgb(0x060a28));
        for (r, row) in history.iter().enumerate() {
            let y = plot_h + r as f32 * row_h;
            if y >= plot_h + wf_h || row.is_empty() {
                break;
            }
            let cols = row.len().min(w as usize).max(1);
            let cw = w / cols as f32;
            for c in 0..cols {
                let v = row[(c * row.len() / cols).min(row.len() - 1)];
                f.rect(rect(c as f32 * cw, y, cw + 0.6, row_h + 0.6), ramp(level(v)));
            }
        }
        // Frequency axis.
        let step = if w < 560.0 { 1000 } else { 500 };
        for hz in (0..=4000).step_by(step) {
            let px = x(hz as f32);
            let label = if hz == 0 {
                "0".to_string()
            } else {
                format!("{:.1} kHz", hz as f32 / 1000.0)
            };
            let tw = f.text_width(&label, 11.0);
            let tx = if hz == 0 {
                px
            } else if hz == 4000 {
                px - tw
            } else {
                px - tw / 2.0
            };
            f.text(pt(tx, plot_h + wf_h + 6.0), label, 11.0, pal.text_secondary);
        }
        f.text(pt(x(SIGNAL_LO) + 6.0, 6.0), "KAHF signal", 11.0, pal.accent);
    })
    .fill_width()
    .into()
}

/// The signal meter: 16 bars, the last four warm.
pub fn signal_meter(level: f32) -> V {
    canvas::<Kahf>(96.0, 18.0, move |f| {
        let pal = f.palette();
        let n = 16;
        let lit = (level.clamp(0.0, 1.0) * n as f32).round() as usize;
        for i in 0..n {
            let on = i < lit;
            let c = if !on {
                pal.border
            } else if i >= n - 4 {
                pal.warning
            } else {
                pal.success
            };
            f.round_rect(rect(i as f32 * 6.0, 2.0, 4.0, 14.0), 1.0, c);
        }
    })
    .into()
}

/// A level bar, for the microphone while recording.
pub fn level_bar(level: f32, width: f32) -> V {
    canvas::<Kahf>(width, 8.0, move |f| {
        let pal = f.palette();
        let w = f.size().width;
        f.round_rect(rect(0.0, 0.0, w, 8.0), 4.0, pal.border);
        f.round_rect(
            rect(0.0, 0.0, (w * level.clamp(0.0, 1.0)).max(0.0), 8.0),
            4.0,
            pal.accent,
        );
    })
    .into()
}

/// A small waveform for voice notes. Not the real audio: a steady pattern per note, so each one
/// looks its own and playback progress has something to sweep across.
pub fn wave(seed: &str, progress: f32, bars: usize) -> V {
    let mut h: u32 = 7;
    for c in seed.chars() {
        h = h.wrapping_mul(31).wrapping_add(c as u32);
    }
    let heights: Vec<f32> = (0..bars)
        .map(|_| {
            h = h.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            0.18 + (h % 82) as f32 / 100.0
        })
        .collect();
    canvas::<Kahf>(bars as f32 * 5.0, 26.0, move |f| {
        let pal = f.palette();
        for (i, v) in heights.iter().enumerate() {
            let played = (i as f32 / bars as f32) < progress;
            let bh = v * 26.0;
            f.round_rect(
                rect(i as f32 * 5.0, (26.0 - bh) / 2.0, 3.0, bh),
                1.5,
                if played { pal.accent } else { pal.border_strong },
            );
        }
    })
    .into()
}
