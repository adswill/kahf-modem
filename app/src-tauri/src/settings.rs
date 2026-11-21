//! The settings file and the part of it the engine needs.
//!
//! The window owns the shape of the settings (see `ui/src/lib/types.ts`); this file stores them
//! as JSON and reads out the few fields the engine uses.

use std::fs;
use std::path::Path;

use serde_json::Value;

const FILE: &str = "app-settings.json";

/// The saved settings, or an empty object the first time.
pub fn load(dir: &Path) -> Value {
    fs::read_to_string(dir.join(FILE))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Default::default()))
}

pub fn save(dir: &Path, settings: &Value) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("Cannot create the settings folder: {e}"))?;
    let tmp = dir.join(format!("{FILE}.tmp"));
    let text = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    fs::write(&tmp, text).map_err(|e| format!("Cannot save the settings: {e}"))?;
    fs::rename(&tmp, dir.join(FILE)).map_err(|e| format!("Cannot save the settings: {e}"))
}

/// Callsign used when the person only listens: the engine needs some valid name.
const LISTEN_ONLY_CALL: &str = "NOCALL1";

/// The channels KAHF uses on 17 m (centres in kHz), from the core band plan.
pub fn channels_khz() -> Vec<f64> {
    let seg = kahf_core::bandplan::RELAY_SEGMENT;
    kahf_core::select::Plan::default_plan()
        .channels
        .iter()
        .map(|(hz, _)| f64::from(*hz))
        .filter(|hz| *hz >= seg.lower_hz as f64 && *hz <= seg.upper_hz as f64)
        .map(|hz| hz / 1000.0)
        .collect()
}

pub struct EngineSettings {
    call: String,
    audio_in: String,
    audio_out: String,
    rig: String,
    /// A radio controlled directly, such as `ic7300:/dev/cu.usbserial-1:115200:cat`.
    rig_spec: String,
    sdr: String,
    pub listen_only: bool,
    /// The radio chosen by name: Hamlib model number, serial port, speed.
    pub rig_choice: Option<(u32, String, u32)>,
}

impl EngineSettings {
    pub fn from_json(v: &Value) -> Self {
        let text = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
        let flag = |k: &str, d: bool| v.get(k).and_then(Value::as_bool).unwrap_or(d);
        EngineSettings {
            call: text("callsign").to_uppercase(),
            audio_in: text("audioIn"),
            audio_out: text("audioOut"),
            rig: text("rigctld"),
            rig_spec: text("rigSpec"),
            sdr: text("sdr"),
            listen_only: flag("listenOnly", false),
            rig_choice: {
                let r = v.get("rig");
                let model = r.and_then(|r| r.get("model")).and_then(Value::as_u64);
                let port = r
                    .and_then(|r| r.get("port"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let baud = r
                    .and_then(|r| r.get("baud"))
                    .and_then(Value::as_u64)
                    .unwrap_or(19_200) as u32;
                match model {
                    Some(m) if m > 0 && !port.is_empty() => Some((m as u32, port, baud)),
                    _ => None,
                }
            },
        }
    }

    pub fn callsign(&self) -> String {
        if self.call.is_empty() {
            LISTEN_ONLY_CALL.to_string()
        } else {
            self.call.clone()
        }
    }

    pub fn audio_in(&self) -> Option<String> {
        (!self.audio_in.is_empty()).then(|| self.audio_in.clone())
    }

    pub fn audio_out(&self) -> Option<String> {
        (!self.audio_out.is_empty()).then(|| self.audio_out.clone())
    }

    /// What the engine opens for radio control: a direct radio spec if one was chosen, else the
    /// rigctld address.
    pub fn rigctld(&self) -> Option<String> {
        if !self.rig_spec.is_empty() {
            return Some(self.rig_spec.clone());
        }
        (!self.rig.is_empty()).then(|| self.rig.clone())
    }

    /// "hackrf" when the person chose a HackRF; the engine opens the first one it finds.
    pub fn sdr_spec(&self) -> Option<String> {
        self.sdr
            .to_lowercase()
            .starts_with("hackrf")
            .then(|| "hackrf".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_fields_the_engine_needs() {
        let s = EngineSettings::from_json(&json!({
            "callsign": " ka1abc ", "audioIn": "USB Audio", "sdr": "HackRF", "freqKhz": 18115.05, "listenOnly": true
        }));
        assert_eq!(s.callsign(), "KA1ABC");
        assert_eq!(s.audio_in().as_deref(), Some("USB Audio"));
        assert_eq!(s.audio_out(), None);
        assert_eq!(s.sdr_spec().as_deref(), Some("hackrf"));
        assert!(s.listen_only);
    }

    #[test]
    fn a_direct_radio_wins_over_a_rigctld_address() {
        let s = EngineSettings::from_json(
            &json!({ "rigctld": "127.0.0.1:4532", "rigSpec": "ic7300:/dev/x:115200:cat" }),
        );
        assert_eq!(s.rigctld().as_deref(), Some("ic7300:/dev/x:115200:cat"));
        let s = EngineSettings::from_json(&json!({ "rigctld": "127.0.0.1:4532" }));
        assert_eq!(s.rigctld().as_deref(), Some("127.0.0.1:4532"));
    }

    #[test]
    fn reads_the_chosen_radio_and_ignores_an_incomplete_one() {
        let s = EngineSettings::from_json(
            &json!({ "rig": { "model": 3073, "port": "/dev/cu.usbserial-1", "baud": 115200 } }),
        );
        assert_eq!(
            s.rig_choice,
            Some((3073, "/dev/cu.usbserial-1".to_string(), 115_200))
        );
        assert_eq!(
            EngineSettings::from_json(&json!({ "rig": { "model": 3073 } })).rig_choice,
            None
        );
        assert_eq!(
            EngineSettings::from_json(&json!({ "rig": { "model": 1, "port": "x" } }))
                .rig_choice
                .unwrap()
                .2,
            19_200
        );
    }

    #[test]
    fn defaults_keep_the_listen_only_callsign_valid() {
        let s = EngineSettings::from_json(&json!({}));
        assert_eq!(s.callsign(), "NOCALL1");
        assert!(!s.listen_only);
        assert_eq!(s.sdr_spec(), None);
    }

    #[test]
    fn settings_survive_a_round_trip_and_a_bad_file_is_ignored() {
        let dir = std::env::temp_dir().join(format!("kahf-app-settings-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(load(&dir), json!({}));
        save(
            &dir,
            &json!({ "callsign": "G4ABC", "location": { "mode": "off" } }),
        )
        .unwrap();
        assert_eq!(load(&dir)["callsign"], "G4ABC");
        fs::write(dir.join(FILE), "not json").unwrap();
        assert_eq!(load(&dir), json!({}));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_17_m_channels_come_from_the_band_plan() {
        let all = channels_khz();
        assert_eq!(all.len(), 3);
        assert!(all.windows(2).all(|w| w[1] > w[0]));
        assert!(all[0] > 18_111.0 && all[2] < 18_120.0);
    }
}
