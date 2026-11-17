//! First-pass HF propagation estimator.
//!
//! STATUS: ESTIMATE ONLY. None of the predictions in this crate have been
//! validated against measured contacts yet. Use [`validate`] with logged
//! contacts (kahf-log) before trusting any output.
//!
//! Model (simplified, loosely following the structure of ITU-R P.533 and the
//! ITU-R P.1239 foF2 idea; coefficients are NOT the ITU maps or tables):
//!
//! * Path geometry: great circle (haversine, mean Earth radius 6371 km),
//!   split into `ceil(d / 2000 km)` equal hops, midpoint at the half-way point.
//! * Solar zenith at the midpoint from declination (mid-month approximation),
//!   local solar time = UTC hour + longitude / 15. Equation of time ignored.
//! * foF2 (MHz): `(4.0 + 0.04 * SFI) * (0.3 + 0.7 * sqrt(max(cos chi, 0)))`.
//! * MUF (MHz): secant law on a flat-earth hop with 300 km reflection height,
//!   `foF2 * sqrt(1 + (d_hop / 600)^2)`, scaled by `1 - 0.04 * max(K - 2, 0)`
//!   (clamped to 0.5..1.0) as a crude geomagnetic penalty.
//! * LUF (MHz): non-deviative D-layer absorption `A = hops * 60 * a / f^2` dB,
//!   with `a = max(cos chi, 0)^0.75 * (0.5 + 0.5 * SFI / 100) * (1 + 0.15 * max(K - 2, 0))`.
//!   The LUF is the frequency where A equals a fixed 10 dB threshold, floored at 1 MHz.
//! * Band status: Closed outside [LUF, MUF]; Marginal within 15% of MUF or
//!   within 20% above LUF; otherwise Open.
//!
//! Ignored by this model: antenna gain and height, transmitter power, noise
//! and SNR, sporadic E, F1 and F2 layer split, the real seasonal foF2 variation
//! (only the declination moves the sun), geomagnetic latitude effects, the
//! real K-index to disturbance relation, equation of time, terrain and takeoff
//! angle, and the ionospheric height change with time. No `PathInput` field is
//! ignored, but `tx` and `rx` enter only through geometry.

use std::f64::consts::PI;

/// Mean Earth radius in kilometres (haversine model).
const EARTH_RADIUS_KM: f64 = 6371.0;
/// Maximum single-hop ground range assumed by the model, in kilometres.
const HOP_KM: f64 = 2000.0;
/// Virtual reflection height used in the secant law, in kilometres.
const REFLECTION_HEIGHT_KM: f64 = 300.0;
/// Absorption threshold (dB) that defines the LUF.
const LUF_THRESHOLD_DB: f64 = 10.0;
/// Absorption scale constant (dB * MHz^2 per hop at full sun).
const ABSORPTION_C: f64 = 60.0;

/// Amateur HF bands checked by [`predict`]: (name, centre frequency in MHz).
pub const HF_BANDS: &[(&str, f64)] = &[
    ("160m", 1.9),
    ("80m", 3.6),
    ("60m", 5.35),
    ("40m", 7.1),
    ("30m", 10.12),
    ("20m", 14.1),
    ("17m", 18.1),
    ("15m", 21.2),
    ("12m", 24.9),
    ("10m", 28.5),
];

/// Inputs for one path prediction.
///
/// `tx` and `rx` are `(latitude_deg, longitude_deg)`, positive north and east.
/// `utc_hour` is 0..=23 and `month` is 1..=12; out-of-range values are clamped.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PathInput {
    pub tx: (f64, f64),
    pub rx: (f64, f64),
    pub utc_hour: u8,
    pub month: u8,
    /// Solar flux index (SFI). Default 100 via [`PathInput::new`].
    pub solar_flux: f64,
    /// Planetary K index. Default 2 via [`PathInput::new`].
    pub k_index: f64,
}

impl PathInput {
    /// New input with the default SFI of 100 and K index of 2.
    pub fn new(tx: (f64, f64), rx: (f64, f64), utc_hour: u8, month: u8) -> Self {
        Self {
            tx,
            rx,
            utc_hour,
            month,
            solar_flux: 100.0,
            k_index: 2.0,
        }
    }
}

/// Predicted state of a band for a path at a given UTC hour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Status {
    Open,
    Marginal,
    Closed,
}

/// Per-band result from [`predict`].
///
/// `margin_mhz` is the headroom inside the LUF..MUF window:
/// `min(MUF - f, f - LUF)`. Positive means inside the window, negative means
/// outside. This is an estimate and has not been validated against measured contacts yet.
#[derive(Debug, Clone, PartialEq)]
pub struct BandOpen {
    pub band: &'static str,
    pub freq_mhz: f64,
    pub status: Status,
    pub margin_mhz: f64,
}

/// Great-circle distance in kilometres between two `(lat, lon)` points in degrees.
///
/// Haversine formula on a sphere of radius 6371 km. Estimate only; not validated
/// against measured contacts yet.
pub fn great_circle_km(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (lat1, lon1) = (a.0.to_radians(), a.1.to_radians());
    let (lat2, lon2) = (b.0.to_radians(), b.1.to_radians());
    let dlat = lat2 - lat1;
    let dlon = lon2 - lon1;
    let h = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_KM * h.sqrt().min(1.0).asin()
}

/// Midpoint `(lat, lon)` in degrees of the great-circle path between two points.
///
/// Estimate only; not validated against measured contacts yet.
pub fn midpoint(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    let (lat1, lon1) = (a.0.to_radians(), a.1.to_radians());
    let (lat2, lon2) = (b.0.to_radians(), b.1.to_radians());
    let dlon = lon2 - lon1;
    let bx = lat2.cos() * dlon.cos();
    let by = lat2.cos() * dlon.sin();
    let lat = (lat1.sin() + lat2.sin()).atan2(((lat1.cos() + bx).powi(2) + by.powi(2)).sqrt());
    let lon = lon1 + by.atan2(lat1.cos() + bx);
    let lon_deg = (lon.to_degrees() + 540.0).rem_euclid(360.0) - 180.0;
    (lat.to_degrees(), lon_deg)
}

/// Converts a 4- or 6-character Maidenhead locator to the centre `(lat, lon)` in degrees.
///
/// Returns `None` if the locator is malformed. Estimate helper only; not validated
/// against measured contacts yet.
pub fn maidenhead_to_latlon(loc: &str) -> Option<(f64, f64)> {
    let b: Vec<char> = loc.trim().to_ascii_uppercase().chars().collect();
    if b.len() != 4 && b.len() != 6 {
        return None;
    }
    let field = |c: char| -> Option<f64> {
        if ('A'..='R').contains(&c) {
            Some((c as u8 - b'A') as f64)
        } else {
            None
        }
    };
    let digit = |c: char| -> Option<f64> { c.to_digit(10).map(|d| d as f64) };
    let sub = |c: char| -> Option<f64> {
        if ('A'..='X').contains(&c) {
            Some((c as u8 - b'A') as f64)
        } else {
            None
        }
    };
    let mut lon = -180.0 + field(b[0])? * 20.0 + digit(b[2])? * 2.0;
    let mut lat = -90.0 + field(b[1])? * 10.0 + digit(b[3])? * 1.0;
    if b.len() == 6 {
        lon += sub(b[4])? * (2.0 / 24.0) + (1.0 / 24.0);
        lat += sub(b[5])? * (1.0 / 24.0) + (1.0 / 48.0);
    } else {
        lon += 1.0;
        lat += 0.5;
    }
    Some((lat, lon))
}

/// Solar zenith cosine at `(lat, lon)` for a UTC hour and month (mid-month sun).
fn cos_zenith(lat: f64, lon: f64, utc_hour: u8, month: u8) -> f64 {
    let month = month.clamp(1, 12) as f64;
    let doy = (month - 1.0) * 30.44 + 15.0;
    let decl = (-23.44f64).to_radians() * (2.0 * PI * (doy + 10.0) / 365.0).cos();
    let lst = utc_hour.min(23) as f64 + lon / 15.0;
    let hour_angle = (15.0 * (lst - 12.0)).to_radians();
    let lat = lat.to_radians();
    (lat.sin() * decl.sin() + lat.cos() * decl.cos() * hour_angle.cos()).clamp(-1.0, 1.0)
}

/// Estimated F2 critical frequency (MHz) at the path midpoint.
///
/// Estimate only; not validated against measured contacts yet.
fn fof2_mhz(solar_flux: f64, cz: f64) -> f64 {
    let peak = 4.0 + 0.04 * solar_flux;
    peak * (0.3 + 0.7 * cz.max(0.0).sqrt())
}

/// Number of hops and per-hop ground range for the path, in kilometres.
fn hops(distance_km: f64) -> (u32, f64) {
    let n = (distance_km / HOP_KM).ceil().max(1.0) as u32;
    (n, distance_km / n as f64)
}

/// Maximum usable frequency (MHz) for the path.
///
/// Secant-law estimate from the midpoint foF2 and per-hop geometry, with a crude
/// K-index penalty. Estimate only; not validated against measured contacts yet.
pub fn muf(input: &PathInput) -> f64 {
    let d = great_circle_km(input.tx, input.rx);
    let mid = midpoint(input.tx, input.rx);
    let cz = cos_zenith(mid.0, mid.1, input.utc_hour, input.month);
    let f0 = fof2_mhz(input.solar_flux, cz);
    let (_, hop) = hops(d);
    let half = hop / 2.0;
    let sec = (1.0 + (half / REFLECTION_HEIGHT_KM).powi(2)).sqrt();
    let penalty = (1.0 - 0.04 * (input.k_index - 2.0).max(0.0)).clamp(0.5, 1.0);
    f0 * sec * penalty
}

/// Lowest usable frequency (MHz) for the path.
///
/// Non-deviative D-layer absorption with a fixed 10 dB threshold, floored at 1 MHz.
/// Estimate only; not validated against measured contacts yet.
pub fn luf(input: &PathInput) -> f64 {
    let d = great_circle_km(input.tx, input.rx);
    let mid = midpoint(input.tx, input.rx);
    let cz = cos_zenith(mid.0, mid.1, input.utc_hour, input.month);
    let (n, _) = hops(d);
    let a = cz.max(0.0).powf(0.75)
        * (0.5 + 0.5 * input.solar_flux / 100.0)
        * (1.0 + 0.15 * (input.k_index - 2.0).max(0.0));
    let k = n as f64 * ABSORPTION_C * a;
    (k / LUF_THRESHOLD_DB).sqrt().max(1.0)
}

/// Predicts every band in [`HF_BANDS`] for one path and UTC hour.
///
/// ESTIMATE ONLY. This model has not been validated against measured contacts yet.
/// Use [`validate`] with logged contacts before relying on it.
pub fn predict(input: PathInput) -> Vec<BandOpen> {
    let m = muf(&input);
    let l = luf(&input);
    HF_BANDS
        .iter()
        .map(|&(band, f)| {
            let status = if f > m || f < l {
                Status::Closed
            } else if f > 0.85 * m || f < 1.2 * l {
                Status::Marginal
            } else {
                Status::Open
            };
            BandOpen {
                band,
                freq_mhz: f,
                status,
                margin_mhz: (m - f).min(f - l),
            }
        })
        .collect()
}

/// Agreement rate between predictions and observed contacts.
///
/// `observed` holds `(band, was_open)` pairs. A band counts as predicted open only
/// when its status is [`Status::Open`]. Each observed pair whose band appears in
/// `predictions` is compared; observed bands missing from `predictions` are skipped.
/// Returns the fraction that agree, or `f64::NAN` when nothing is comparable.
///
/// Estimate tool only: this has not been run against measured contacts yet.
pub fn validate(predictions: &[BandOpen], observed: &[(&str, bool)]) -> f64 {
    let mut total = 0usize;
    let mut agree = 0usize;
    for &(band, was_open) in observed {
        if let Some(p) = predictions.iter().find(|p| p.band == band) {
            total += 1;
            if (p.status == Status::Open) == was_open {
                agree += 1;
            }
        }
    }
    if total == 0 {
        f64::NAN
    } else {
        agree as f64 / total as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn london_to_new_york_distance() {
        let london = (51.5074, -0.1278);
        let new_york = (40.7128, -74.0060);
        let d = great_circle_km(london, new_york);
        assert!((d - 5570.0).abs() / 5570.0 < 0.01, "got {d} km");
    }

    #[test]
    fn maidenhead_london_is_close() {
        let (lat, lon) = maidenhead_to_latlon("IO91wm").unwrap();
        assert!((lat - 51.5074).abs() < 0.1, "lat {lat}");
        assert!((lon + 0.1278).abs() < 0.1, "lon {lon}");
        assert!(maidenhead_to_latlon("ZZ99").is_none());
    }

    #[test]
    fn high_flux_noon_opens_more_bands_than_low_flux_night() {
        // London to New York; midpoint is near 46N 37W, local noon around 14:30 UTC.
        let tx = (51.5074, -0.1278);
        let rx = (40.7128, -74.0060);
        let day = PathInput {
            tx,
            rx,
            utc_hour: 14,
            month: 3,
            solar_flux: 180.0,
            k_index: 2.0,
        };
        let night = PathInput {
            tx,
            rx,
            utc_hour: 2,
            month: 3,
            solar_flux: 60.0,
            k_index: 2.0,
        };
        let open = |i: PathInput| predict(i).iter().filter(|b| b.status == Status::Open).count();
        let (d, n) = (open(day), open(night));
        assert!(d > n, "day {d} open bands, night {n}");
    }

    #[test]
    fn validate_perfect_match_is_one() {
        let input = PathInput::new((51.5074, -0.1278), (40.7128, -74.0060), 14, 3);
        let preds = predict(input);
        let observed: Vec<(&str, bool)> = preds.iter().map(|p| (p.band, p.status == Status::Open)).collect();
        assert_eq!(validate(&preds, &observed), 1.0);
    }

    #[test]
    fn validate_counts_mismatches() {
        let input = PathInput::new((51.5074, -0.1278), (40.7128, -74.0060), 14, 3);
        let preds = predict(input);
        let open_band = preds.iter().find(|p| p.status == Status::Open).unwrap().band;
        let observed = [(open_band, false)];
        assert_eq!(validate(&preds, &observed), 0.0);
        assert!(validate(&preds, &[("999m", true)]).is_nan());
    }
}
