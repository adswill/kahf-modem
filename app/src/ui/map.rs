//! A small world map drawn from land outlines built into the app: drag to move, buttons to zoom.
//! It needs no internet, so the screen is never empty. Relays, stations and the person's own
//! position are pinned on it.

use alloy_ui::prelude::*;
use alloy_ui::{pt, rgb, Path, Pointer, Point};

use super::kit::*;
use crate::types::RelayInfo;

const LAND: &str = include_str!("../../assets/land.txt");
const TILE: f64 = 256.0;

/// What the person did to the map.
#[derive(Debug, Clone, PartialEq)]
pub struct MapState {
    pub zoom: f64,
    pub lat: f64,
    pub lon: f64,
    /// A drag in progress: pointer position and the centre when it began.
    pub drag: Option<(Point, f64, f64)>,
    pub centred: bool,
    /// Pixel size of the canvas, learned while painting.
    pub width: f32,
}

impl Default for MapState {
    fn default() -> Self {
        MapState { zoom: 1.0, lat: 30.0, lon: 10.0, drag: None, centred: false, width: 800.0 }
    }
}

fn world(zoom: f64) -> f64 {
    TILE * 2f64.powf(zoom)
}

pub fn px(lon: f64, zoom: f64) -> f64 {
    (lon + 180.0) / 360.0 * world(zoom)
}

pub fn py(lat: f64, zoom: f64) -> f64 {
    let s = (lat.clamp(-85.0, 85.0).to_radians()).sin();
    (0.5 - ((1.0 + s) / (1.0 - s)).ln() / (4.0 * std::f64::consts::PI)) * world(zoom)
}

pub fn to_lon(x: f64, zoom: f64) -> f64 {
    x / world(zoom) * 360.0 - 180.0
}

pub fn to_lat(y: f64, zoom: f64) -> f64 {
    let n = std::f64::consts::PI - 2.0 * std::f64::consts::PI * y / world(zoom);
    (0.5 * (n.exp() - (-n).exp())).atan().to_degrees()
}

fn land() -> Vec<Vec<(f64, f64)>> {
    LAND.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            l.split(' ')
                .filter_map(|p| {
                    let (a, b) = p.split_once(',')?;
                    Some((a.parse().ok()?, b.parse().ok()?))
                })
                .collect()
        })
        .collect()
}

/// A marker on the map.
#[derive(Debug, Clone)]
pub struct Pin {
    pub label: String,
    pub lat: f64,
    pub lon: f64,
    pub kind: PinKind,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PinKind {
    Relay,
    Station,
    Me,
}

impl MapState {
    pub fn zoom_by(&mut self, d: f64) {
        self.zoom = (self.zoom + d).clamp(0.0, 8.0);
    }

    pub fn center_on(&mut self, lat: f64, lon: f64) {
        self.lat = lat;
        self.lon = lon;
    }

    pub fn pointer(&mut self, p: Pointer) {
        match p {
            Pointer::Down(at) => self.drag = Some((at, self.lat, self.lon)),
            Pointer::Move(at) => {
                if let Some((from, lat, lon)) = self.drag {
                    let cx = px(lon, self.zoom) - f64::from(at.x - from.x);
                    let cy = py(lat, self.zoom) - f64::from(at.y - from.y);
                    self.lat = to_lat(cy, self.zoom).clamp(-80.0, 80.0);
                    self.lon = to_lon(cx, self.zoom);
                }
            }
            Pointer::Up(_) => self.drag = None,
        }
    }
}

pub fn view(k: &Kahf, pins: Vec<Pin>, height: f32) -> V {
    let (zoom, clat, clon) = (k.ui.map.zoom, k.ui.map.lat, k.ui.map.lon);
    let me = pins.iter().find(|p| p.kind == PinKind::Me).map(|p| (p.lat, p.lon));
    let canvas = canvas::<Kahf>(0.0, height, move |f| {
        let size = f.size();
        let pal = f.palette();
        f.rect(alloy_ui::Rectangle::new(pt(0.0, 0.0), size), pal.surface);
        let ox = px(clon, zoom) - f64::from(size.width) / 2.0;
        let oy = py(clat, zoom) - f64::from(size.height) / 2.0;
        let w = world(zoom);
        let at = |lat: f64, lon: f64, shift: f64| pt((px(lon, zoom) + shift - ox) as f32, (py(lat, zoom) - oy) as f32);
        // The ocean, then the land, repeated once either side so dragging wraps around.
        f.rect(alloy_ui::Rectangle::new(pt(0.0, 0.0), size), rgb(0x1b3a52).with_alpha(0.35));
        for ring in land() {
            for shift in [-w, 0.0, w] {
                let pts: Vec<Point> = ring.iter().map(|(lo, la)| at(*la, *lo, shift)).collect();
                if pts.iter().all(|p| p.x < -50.0 || p.x > size.width + 50.0) {
                    continue;
                }
                f.fill(&Path::polyline(pts.clone()).close(), pal.surface_raised);
                f.stroke(&Path::polyline(pts), 0.8, pal.border_strong);
            }
        }
        for lon in (-180..=180).step_by(30) {
            for shift in [-w, 0.0, w] {
                f.line(at(80.0, f64::from(lon), shift), at(-80.0, f64::from(lon), shift), 0.5, pal.border);
            }
        }
        for lat in (-60..=60).step_by(30) {
            f.line(at(f64::from(lat), -180.0, -w), at(f64::from(lat), 180.0, w), 0.5, pal.border);
        }
        for pin in &pins {
            let (color, r) = match pin.kind {
                PinKind::Relay => (pal.accent, 6.0),
                PinKind::Station => (pal.success, 4.5),
                PinKind::Me => (pal.warning, 7.0),
            };
            for shift in [-w, 0.0, w] {
                let p = at(pin.lat, pin.lon, shift);
                if p.x < -20.0 || p.x > size.width + 20.0 {
                    continue;
                }
                f.circle(p, r, color);
                f.stroke_circle(p, r, 1.5, pal.surface);
                f.text(pt(p.x + r + 3.0, p.y - 7.0), pin.label.clone(), 11.0, pal.text);
            }
        }
    })
    .fill_width()
    .act(|k: &mut Kahf, p: Pointer| k.ui.map.pointer(p));
    let mut ctl = row![
        button("+").act(|k: &mut Kahf| k.ui.map.zoom_by(1.0)),
        button("−").act(|k: &mut Kahf| k.ui.map.zoom_by(-1.0)),
    ]
    .gap(6.0);
    if let Some((la, lo)) = me {
        ctl = ctl.push(button("My position").act(move |k: &mut Kahf| k.ui.map.center_on(la, lo)));
    }
    ctl = ctl.push(caption("Drag to move. The map is drawn from outlines built into KAHF, so it works without the internet.").dim());
    col![canvas, ctl.align_center()].gap(6.0).into()
}

/// The pins for relays, heard stations that told their place, and the person.
pub fn pins(k: &Kahf) -> Vec<Pin> {
    let mut out: Vec<Pin> = Vec::new();
    let relays: &[RelayInfo] = &k.m.relays;
    for r in relays {
        if let (Some(lat), Some(lon)) = (r.lat, r.lon) {
            out.push(Pin { label: r.call.clone(), lat, lon, kind: PinKind::Relay });
        }
    }
    for h in &k.m.heard {
        if let (Some(lat), Some(lon)) = (h.lat, h.lon) {
            if !relays.iter().any(|r| r.call == h.call) {
                out.push(Pin { label: h.call.clone(), lat, lon, kind: PinKind::Station });
            }
        }
    }
    let l = &k.m.settings.location;
    if l.mode != crate::types::LocationMode::Off {
        if let (Some(lat), Some(lon)) = (l.lat, l.lon) {
            out.push(Pin { label: "YOU".into(), lat, lon, kind: PinKind::Me });
        }
    }
    out
}

use crate::app::Kahf;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_round_trips() {
        for (lat, lon) in [(0.0, 0.0), (51.5, -0.1), (-33.9, 151.2), (60.0, 179.0)] {
            let z = 4.0;
            assert!((to_lat(py(lat, z), z) - lat).abs() < 1e-6);
            assert!((to_lon(px(lon, z), z) - lon).abs() < 1e-6);
        }
        assert!((px(0.0, 0.0) - 128.0).abs() < 1e-9);
    }

    #[test]
    fn the_land_outlines_load() {
        let l = land();
        assert!(l.len() > 100 && l.iter().all(|r| r.len() >= 3));
    }

    #[test]
    fn dragging_moves_the_centre_and_zoom_is_limited() {
        let mut m = MapState::default();
        let (la, lo) = (m.lat, m.lon);
        m.pointer(Pointer::Down(pt(100.0, 100.0)));
        m.pointer(Pointer::Move(pt(60.0, 100.0)));
        assert!(m.lon > lo && (m.lat - la).abs() < 1e-6);
        m.pointer(Pointer::Up(pt(60.0, 100.0)));
        m.pointer(Pointer::Move(pt(0.0, 0.0)));
        assert!(m.drag.is_none());
        m.zoom_by(100.0);
        assert_eq!(m.zoom, 8.0);
        m.zoom_by(-100.0);
        assert_eq!(m.zoom, 0.0);
    }
}
