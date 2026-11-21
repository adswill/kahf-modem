//! Position helpers: Maidenhead locators, checking coordinates, and a note about 17 m now.

/// Centre of a Maidenhead locator of 4 or 6 characters, for example "JN45" or "JN45rm".
pub fn locator_to_lat_lon(loc: &str) -> Option<(f64, f64)> {
    let up = loc.trim().to_uppercase();
    let b: Vec<char> = up.chars().collect();
    if !(b.len() == 4 || b.len() == 6) {
        return None;
    }
    let letter = |c: char, max: char| ('A'..=max).contains(&c);
    if !(letter(b[0], 'R') && letter(b[1], 'R') && b[2].is_ascii_digit() && b[3].is_ascii_digit()) {
        return None;
    }
    if b.len() == 6 && !(letter(b[4], 'X') && letter(b[5], 'X')) {
        return None;
    }
    let a = |c: char| f64::from(c as u32 - 'A' as u32);
    let d = |c: char| f64::from(c.to_digit(10).unwrap_or(0));
    let mut lon = a(b[0]) * 20.0 - 180.0 + d(b[2]) * 2.0;
    let mut lat = a(b[1]) * 10.0 - 90.0 + d(b[3]);
    if b.len() == 6 {
        lon += a(b[4]) * (2.0 / 24.0) + 1.0 / 24.0;
        lat += a(b[5]) * (1.0 / 24.0) + 1.0 / 48.0;
    } else {
        lon += 1.0;
        lat += 0.5;
    }
    Some((lat, lon))
}

/// 6-character Maidenhead locator for a position.
pub fn lat_lon_to_locator(lat: f64, lon: f64) -> String {
    let l = lon + 180.0;
    let b = lat + 90.0;
    let up = |n: f64| char::from(b'A' + n as u8);
    let low = |n: f64| char::from(b'a' + n as u8);
    let (lo1, la1) = ((l / 20.0).floor(), (b / 10.0).floor());
    let (lo2, la2) = (((l % 20.0) / 2.0).floor(), (b % 10.0).floor());
    let (lo3, la3) = (
        (((l % 2.0) * 60.0) / 5.0).floor(),
        (((b % 1.0) * 60.0) / 2.5).floor(),
    );
    format!(
        "{}{}{}{}{}{}",
        up(lo1),
        up(la1),
        lo2 as u8,
        la2 as u8,
        low(lo3.min(23.0)),
        low(la3.min(23.0))
    )
}

/// A latitude inside -90 to 90.
pub fn valid_lat(v: Option<f64>) -> bool {
    v.is_some_and(|v| v.is_finite() && (-90.0..=90.0).contains(&v))
}

/// A longitude inside -180 to 180.
pub fn valid_lon(v: Option<f64>) -> bool {
    v.is_some_and(|v| v.is_finite() && (-180.0..=180.0).contains(&v))
}

/// What a person is told others will see, in plain words.
pub fn sharing_summary(mode: &str, lat: Option<f64>, lon: Option<f64>, radius_km: u32) -> String {
    match mode {
        "off" => "Nobody can see where you are. You still appear on the map only if you choose to.".into(),
        _ if !valid_lat(lat) || !valid_lon(lon) => "Enter your position to continue.".into(),
        "precise" => "Your position is shown on the public map to about 200 metres.".into(),
        _ => format!("The public map shows a circle about {radius_km} km wide around you. Your exact position is never sent."),
    }
}

/// Height of the sun above the horizon (degrees) at a place and a time, from the usual
/// low-precision solar formulas. `unix` is seconds since 1970 (UTC).
pub fn sun_elevation(lat: f64, lon: f64, unix: i64) -> f64 {
    let rad = std::f64::consts::PI / 180.0;
    let days = unix as f64 / 86_400.0;
    // Day of the year, counted from 1 January 00:00 UTC.
    let year_start = {
        let (y, _, _) = civil_from_days(days.floor() as i64);
        days_from_civil(y, 1, 1) as f64
    };
    let day = days - year_start;
    let g = (2.0 * std::f64::consts::PI / 365.0) * (day - 0.5);
    let dec = 0.006918 - 0.399912 * g.cos() + 0.070257 * g.sin() - 0.006758 * (2.0 * g).cos()
        + 0.000907 * (2.0 * g).sin()
        - 0.002697 * (3.0 * g).cos()
        + 0.00148 * (3.0 * g).sin();
    let eq = 229.18
        * (0.000075 + 0.001868 * g.cos()
            - 0.032077 * g.sin()
            - 0.014615 * (2.0 * g).cos()
            - 0.040849 * (2.0 * g).sin());
    let utc_min = (unix.rem_euclid(86_400)) as f64 / 60.0;
    let sun_lon = (720.0 - utc_min - eq) / 4.0;
    let h = (lon - sun_lon) * rad;
    let s = (lat * rad).sin() * dec.sin() + (lat * rad).cos() * dec.cos() * h.cos();
    s.clamp(-1.0, 1.0).asin() / rad
}

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm).
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date (year, month, day) of a day count since 1970-01-01.
pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// How the person should read the tone of a reach notice.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tone {
    Good,
    Warn,
}

/// A plain-words note about 17 metres at the person's place right now.
pub fn reach_notice(lat: Option<f64>, lon: Option<f64>, unix: i64) -> Option<(Tone, &'static str)> {
    let (lat, lon) = (lat?, lon?);
    let el = sun_elevation(lat, lon, unix);
    Some(if el > 5.0 {
        (
            Tone::Good,
            "It is daylight at your place. 17 m usually works on paths of about 1000 to 4000 km now.",
        )
    } else if el > -6.0 {
        (
            Tone::Warn,
            "Dawn or dusk at your place. 17 m may open or close on a path at any time.",
        )
    } else {
        (
            Tone::Warn,
            "It is night at your place. 17 m is usually closed now: your messages wait in your outbox and KAHF keeps listening for it to open.",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locators_round_trip() {
        let (lat, lon) = locator_to_lat_lon("JN45rm").unwrap();
        assert!(
            (lat - 45.52).abs() < 0.01 && (lon - 9.46).abs() < 0.01,
            "{lat} {lon}"
        );
        assert_eq!(lat_lon_to_locator(lat, lon).to_uppercase(), "JN45RM");
        let (la, lo) = locator_to_lat_lon("JN45").unwrap();
        assert!((la - 45.5).abs() < 1e-9 && (lo - 9.0).abs() < 1e-9);
        assert_eq!(locator_to_lat_lon("ZZ99"), None);
        assert_eq!(locator_to_lat_lon("JN4"), None);
    }

    #[test]
    fn the_sun_is_up_at_noon_and_down_at_midnight() {
        // 2026-06-21 12:00 UTC on the Greenwich meridian at 51 N.
        let noon = days_from_civil(2026, 6, 21) * 86_400 + 12 * 3600;
        assert!(sun_elevation(51.0, 0.0, noon) > 50.0);
        assert!(sun_elevation(51.0, 0.0, noon + 12 * 3600) < -5.0);
        assert_eq!(reach_notice(None, Some(0.0), noon), None);
        assert_eq!(reach_notice(Some(51.0), Some(0.0), noon).unwrap().0, Tone::Good);
    }

    #[test]
    fn civil_dates_convert_both_ways() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(days_from_civil(2026, 11, 21)), (2026, 11, 21));
    }

    #[test]
    fn sharing_is_described_in_words() {
        assert!(sharing_summary("off", None, None, 50).contains("Nobody"));
        assert!(sharing_summary("area", None, None, 50).contains("Enter your position"));
        assert!(sharing_summary("area", Some(1.0), Some(2.0), 50).contains("50 km"));
        assert!(sharing_summary("precise", Some(1.0), Some(2.0), 50).contains("200 metres"));
    }
}
