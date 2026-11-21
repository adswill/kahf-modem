//! Words and numbers for people, not for engineers.

use chrono::{Local, TimeZone};

/// How well a signal-to-noise ratio is heard.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Quality {
    Excellent,
    Good,
    Fair,
    Weak,
    VeryWeak,
    Unknown,
}

impl Quality {
    pub fn of(snr: Option<f64>) -> Self {
        match snr {
            None => Quality::Unknown,
            Some(s) if s >= 10.0 => Quality::Excellent,
            Some(s) if s >= 3.0 => Quality::Good,
            Some(s) if s >= -5.0 => Quality::Fair,
            Some(s) if s >= -12.0 => Quality::Weak,
            Some(_) => Quality::VeryWeak,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Quality::Excellent => "Excellent",
            Quality::Good => "Good",
            Quality::Fair => "Fair",
            Quality::Weak => "Weak",
            Quality::VeryWeak => "Very weak",
            Quality::Unknown => "Unknown",
        }
    }

    /// 0 to 4 bars.
    pub fn level(self) -> u8 {
        match self {
            Quality::Excellent => 4,
            Quality::Good => 3,
            Quality::Fair => 2,
            Quality::Weak | Quality::VeryWeak => 1,
            Quality::Unknown => 0,
        }
    }
}

/// Typing speed in plain terms, from net bits per second (8 bits per character).
pub fn speed_words(net_bps: f64) -> String {
    let cps = net_bps / 8.0;
    if cps < 1.0 {
        "under 1 letter a second".into()
    } else if cps < 20.0 {
        format!("about {} letters a second", cps.round())
    } else if cps < 200.0 {
        format!("about {} letters a second, like fast typing", cps.round())
    } else {
        format!("about {} letters a second, a page in a few seconds", cps.round())
    }
}

fn plural(n: i64, unit: &str) -> String {
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

/// "just now", "5 minutes ago", from unix milliseconds.
pub fn time_ago(ms: i64, now_ms: i64) -> String {
    let s = ((now_ms - ms) / 1000).max(0);
    if s < 10 {
        return "just now".into();
    }
    if s < 60 {
        return format!("{s} seconds ago");
    }
    let m = (s as f64 / 60.0).round() as i64;
    if m < 60 {
        return format!("{} ago", plural(m, "minute"));
    }
    let h = (m as f64 / 60.0).round() as i64;
    if h < 24 {
        return format!("{} ago", plural(h, "hour"));
    }
    format!("{} ago", plural((h as f64 / 24.0).round() as i64, "day"))
}

/// Local time of day, "14:05", from unix milliseconds.
pub fn clock(ms: i64) -> String {
    Local.timestamp_millis_opt(ms).single().map(|t| t.format("%H:%M").to_string()).unwrap_or_default()
}

/// Local date and time, "21 Nov 14:05".
pub fn stamp(ms: i64) -> String {
    Local.timestamp_millis_opt(ms).single().map(|t| t.format("%-d %b %H:%M").to_string()).unwrap_or_default()
}

/// 18112.35 kHz as "18.11235 MHz". 17 m dials have up to two decimals of kHz: all are shown.
pub fn mhz(khz: f64) -> String {
    format!("{:.5} MHz", khz / 1000.0)
}

/// Bytes in plain words.
pub fn size_words(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        let kb = n as f64 / 1024.0;
        format!("{:.*} KB", if n < 10_240 { 1 } else { 0 }, kb)
    } else {
        format!("{:.1} MB", n as f64 / 1024.0 / 1024.0)
    }
}

/// The most one transfer may hold. Keep in step with `MAX_ATTACHMENT_BYTES` in the engine.
pub const FILE_LIMIT_BYTES: u64 = 100 * 1024;

/// Seconds a transfer of `bytes` takes at `net_bps` of useful speed, with room for the
/// acknowledgements and the gaps while the other station answers.
pub fn transfer_seconds(bytes: u64, net_bps: f64) -> f64 {
    (bytes as f64 * 8.0 * 1.3) / net_bps.max(20.0)
}

/// "about 3 minutes", "about 1 hour 20 minutes".
pub fn duration_words(seconds: f64) -> String {
    if seconds < 90.0 {
        return format!("about {} seconds", ((seconds / 5.0).round() as i64 * 5).max(5));
    }
    let min = (seconds / 60.0).round() as i64;
    if min < 60 {
        return format!("about {}", plural(min, "minute"));
    }
    let (h, m) = (min / 60, min % 60);
    if m >= 5 {
        format!("about {} {m} minutes", plural(h, "hour"))
    } else {
        format!("about {}", plural(h, "hour"))
    }
}

/// 18113.5 kHz as "18 113.5 kHz", with a thin space between thousands.
pub fn channel_khz(khz: f64) -> String {
    let whole = khz.trunc() as i64;
    let tenth = (((khz - whole as f64) * 10.0).round() as i64).min(9);
    if whole >= 1000 {
        format!("{}\u{202f}{:03}.{tenth} kHz", whole / 1000, whole % 1000)
    } else {
        format!("{whole}.{tenth} kHz")
    }
}

/// Unix seconds as "HH:MM" UTC.
pub fn utc_hm(unix: i64) -> String {
    let d = unix.rem_euclid(86_400);
    format!("{:02}:{:02}", d / 3600, d / 60 % 60)
}

/// Seconds as "45 s", "15 min" or "1 h 30 min".
pub fn duration(secs: i64) -> String {
    if secs < 60 {
        return format!("{secs} s");
    }
    if secs < 3600 {
        return format!("{} min", (secs as f64 / 60.0).round() as i64);
    }
    let h = secs / 3600;
    let m = ((secs % 3600) as f64 / 60.0).round() as i64;
    if m == 0 {
        format!("{h} h")
    } else {
        format!("{h} h {m} min")
    }
}

/// "in 4 min" or "6 min ago", from a unix time and now (both in seconds).
pub fn relative(unix: i64, now: i64) -> String {
    let d = unix - now;
    let span = duration(d.abs());
    if d >= 0 {
        format!("in {span}")
    } else {
        format!("{span} ago")
    }
}

/// What a mode name means in a sentence.
pub fn mode_explained(name: &str) -> &'static str {
    match name.trim_start_matches("KAMI-") {
        "256" | "128" | "64" => "Very slow but reaches the weakest signals.",
        "32" | "16" => "Slow and sure. A good starting point.",
        "8" | "4" => "Medium speed for decent signals.",
        "1a" => "Fast. Needs a clear signal.",
        "1b" => "Faster. Needs a good signal.",
        "1c" => "Very fast. Needs a strong signal.",
        "1d" => "Fastest. Needs an excellent signal.",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snr_is_graded() {
        assert_eq!(Quality::of(None), Quality::Unknown);
        assert_eq!(Quality::of(Some(12.0)).label(), "Excellent");
        assert_eq!(Quality::of(Some(-20.0)).level(), 1);
        assert_eq!(Quality::of(Some(0.0)), Quality::Fair);
    }

    #[test]
    fn times_are_in_words() {
        assert_eq!(time_ago(0, 5_000), "just now");
        assert_eq!(time_ago(0, 30_000), "30 seconds ago");
        assert_eq!(time_ago(0, 60_000 * 5), "5 minutes ago");
        assert_eq!(time_ago(0, 60_000 * 60), "1 hour ago");
        assert_eq!(time_ago(0, 86_400_000 * 3), "3 days ago");
        assert_eq!(duration(45), "45 s");
        assert_eq!(duration(900), "15 min");
        assert_eq!(duration(5400), "1 h 30 min");
        assert_eq!(relative(100, 40), "in 1 min");
        assert_eq!(relative(40, 100), "1 min ago");
        assert_eq!(utc_hm(3_600 * 13 + 60 * 7), "13:07");
    }

    #[test]
    fn sizes_and_frequencies() {
        assert_eq!(size_words(500), "500 B");
        assert_eq!(size_words(2048), "2.0 KB");
        assert_eq!(size_words(50_000), "49 KB");
        assert_eq!(mhz(18112.35), "18.11235 MHz");
        assert_eq!(channel_khz(18113.5), "18\u{202f}113.5 kHz");
        assert_eq!(duration_words(30.0), "about 30 seconds");
        assert_eq!(duration_words(200.0), "about 3 minutes");
        assert_eq!(duration_words(4800.0), "about 1 hour 20 minutes");
        assert!(transfer_seconds(1000, 284.0) > 30.0);
        assert!(speed_words(284.0).contains("like fast typing"));
    }
}
