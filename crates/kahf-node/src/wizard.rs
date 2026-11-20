//! First-run audio setup: find the radio's sound card among the devices and judge the
//! input level. The level check listens only; it never keys the radio.

/// Name fragments of sound cards that radios and digital-mode interfaces present.
/// Lower-case. Order does not matter.
const RIG_AUDIO_HINTS: &[&str] = &[
    "usb audio codec", // Icom IC-7300, IC-705, IC-7610, IC-9700 (TI PCM29xx)
    "usb audio",
    "ic-7300",
    "ic-705",
    "ft-991",
    "ft-710",
    "ftdx",
    "ts-590",
    "ts-890",
    "qmx",
    "qdx",
    "digirig",
    "signalink",
    "sound blaster", // sometimes used as interface dongles
    "codec",
];

/// Likely radio sound devices in `devices`, best guess first. Built-in speakers and
/// microphones are never offered.
pub fn detect_rig_audio(devices: &[String]) -> Vec<String> {
    let mut scored: Vec<(usize, &String)> = devices
        .iter()
        .filter_map(|d| {
            let l = d.to_ascii_lowercase();
            if l.contains("built-in")
                || l.contains("speaker")
                || l.contains("microphone")
                || l.contains("hdmi")
            {
                return None;
            }
            RIG_AUDIO_HINTS.iter().position(|h| l.contains(h)).map(|p| (p, d))
        })
        .collect();
    scored.sort_by_key(|(p, _)| *p);
    scored.into_iter().map(|(_, d)| d.clone()).collect()
}

/// How the received level looks, in plain words, and whether it needs a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Level {
    /// Nothing at all: wrong device, muted, or the radio's audio output is off.
    Silent,
    TooLow,
    Good,
    TooHigh,
}

/// Judges the average input level in dBFS measured while the band is open and nothing
/// is being sent. Band noise should sit around -35 to -15 dBFS.
pub fn judge_level(rms_db: f32, peak_abs: f32) -> Level {
    if rms_db <= -80.0 {
        Level::Silent
    } else if peak_abs >= 0.98 || rms_db > -10.0 {
        Level::TooHigh
    } else if rms_db < -45.0 {
        Level::TooLow
    } else {
        Level::Good
    }
}

pub fn advice(l: &Level) -> &'static str {
    match l {
        Level::Silent => "No audio at all. Check that this is the radio's sound card, that the radio is on, and that its USB audio output is enabled.",
        Level::TooLow => "The level is low. Raise the radio's USB audio output level (for example the IC-7300 menu item USB AF Output Level, or the FT-991A DATA OUT level).",
        Level::Good => "The level is good.",
        Level::TooHigh => "The level is too high and may clip. Lower the radio's USB audio output level or the computer's input volume.",
    }
}

/// Transmit drive guidance. Shown with the level check because the two are set together.
pub const TX_ADVICE: &str = "Set transmit drive so the radio's ALC barely moves, and keep power low. A clean signal is better than a loud one.";

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn finds_the_icom_codec_first_and_skips_built_ins() {
        let d = s(&[
            "MacBook Pro Microphone",
            "MacBook Pro Speakers",
            "USB Audio CODEC",
            "HDMI Output",
        ]);
        assert_eq!(detect_rig_audio(&d), s(&["USB Audio CODEC"]));
    }

    #[test]
    fn named_radio_beats_generic() {
        let d = s(&["USB Audio Device", "QMX Transceiver"]);
        assert_eq!(detect_rig_audio(&d)[0], "USB Audio Device");
        assert_eq!(detect_rig_audio(&d).len(), 2);
        assert!(detect_rig_audio(&s(&["Realtek Audio"])).is_empty());
    }

    #[test]
    fn level_judgement() {
        assert_eq!(judge_level(-120.0, 0.0), Level::Silent);
        assert_eq!(judge_level(-60.0, 0.01), Level::TooLow);
        assert_eq!(judge_level(-25.0, 0.3), Level::Good);
        assert_eq!(judge_level(-25.0, 1.0), Level::TooHigh);
        assert_eq!(judge_level(-5.0, 0.9), Level::TooHigh);
        assert!(advice(&Level::Silent).contains("No audio"));
    }
}
