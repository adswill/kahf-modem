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

/// The position this station may put in its announcements, following the person's privacy
/// choice: nothing when off, the middle of its one-degree square for "area", or to the nearest
/// hundredth of a degree (about a kilometre) for "precise".
pub fn shared_position(v: &Value) -> Option<(f64, f64)> {
    let loc = v.get("location")?;
    let (lat, lon) = (loc.get("lat")?.as_f64()?, loc.get("lon")?.as_f64()?);
    match loc.get("mode")?.as_str()? {
        "precise" => Some(((lat * 100.0).round() / 100.0, (lon * 100.0).round() / 100.0)),
        "area" => Some((lat.floor() + 0.5, lon.floor() + 0.5)),
        _ => None,
    }
}

/// Callsign used when the person only listens: the engine needs some valid name.
const LISTEN_ONLY_CALL: &str = "NOCALL1";

pub struct EngineSettings {
    call: String,
    audio_in: String,
    audio_out: String,
    rig: String,
    sdr: String,
    pub freq_khz: u64,
    /// Transmit audio level, percent of full scale.
    pub tx_level: u8,
    pub listen_only: bool,
    pub auto_link: bool,
    pub auto_mode: bool,
    pub fixed_mode: String,
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
            sdr: text("sdr"),
            freq_khz: v.get("freqKhz").and_then(Value::as_u64).unwrap_or(14_103),
            tx_level: v.get("txLevel").and_then(Value::as_u64).unwrap_or(50).clamp(5, 100) as u8,
            listen_only: flag("listenOnly", false),
            auto_link: flag("autoLink", true),
            auto_mode: flag("autoMode", true),
            fixed_mode: {
                let m = text("fixedMode");
                if m.is_empty() { "KAMI-32".to_string() } else { m }
            },
            rig_choice: {
                let r = v.get("rig");
                let model = r.and_then(|r| r.get("model")).and_then(Value::as_u64);
                let port = r.and_then(|r| r.get("port")).and_then(Value::as_str).unwrap_or("").trim().to_string();
                let baud = r.and_then(|r| r.get("baud")).and_then(Value::as_u64).unwrap_or(19_200) as u32;
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

    pub fn rigctld(&self) -> Option<String> {
        (!self.rig.is_empty()).then(|| self.rig.clone())
    }

    /// "hackrf@14103000" or "rtlsdr@14103000" for the receiver the person chose, if any.
    pub fn sdr_spec(&self) -> Option<String> {
        let driver = match self.sdr.to_lowercase().as_str() {
            s if s.starts_with("hackrf") => "hackrf",
            s if s.starts_with("rtl") => "rtlsdr",
            _ => return None,
        };
        Some(format!("{driver}@{}", self.freq_khz * 1000))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_fields_the_engine_needs() {
        let s = EngineSettings::from_json(&json!({
            "callsign": " ka1abc ", "audioIn": "USB Audio", "sdr": "HackRF", "freqKhz": 7103,
            "listenOnly": true, "autoLink": false, "autoMode": false, "fixedMode": "KAMI-64"
        }));
        assert_eq!(s.callsign(), "KA1ABC");
        assert_eq!(s.audio_in().as_deref(), Some("USB Audio"));
        assert_eq!(s.audio_out(), None);
        assert_eq!(s.sdr_spec().as_deref(), Some("hackrf@7103000"));
        assert!(s.listen_only && !s.auto_link && !s.auto_mode);
        assert_eq!(s.fixed_mode, "KAMI-64");
    }

    #[test]
    fn reads_the_chosen_radio_and_ignores_an_incomplete_one() {
        let s = EngineSettings::from_json(&json!({ "rig": { "model": 3073, "port": "/dev/cu.usbserial-1", "baud": 115200 } }));
        assert_eq!(s.rig_choice, Some((3073, "/dev/cu.usbserial-1".to_string(), 115_200)));
        assert_eq!(EngineSettings::from_json(&json!({ "rig": { "model": 3073 } })).rig_choice, None);
        assert_eq!(EngineSettings::from_json(&json!({ "rig": { "model": null, "port": "x" } })).rig_choice, None);
        assert_eq!(EngineSettings::from_json(&json!({ "rig": { "model": 1, "port": "x" } })).rig_choice.unwrap().2, 19_200);
    }

    #[test]
    fn defaults_keep_automation_on_and_listen_only_callsign_valid() {
        let s = EngineSettings::from_json(&json!({}));
        assert_eq!(s.callsign(), "NOCALL1");
        assert!(s.auto_link && s.auto_mode && !s.listen_only);
        assert_eq!(s.freq_khz, 14_103);
        assert_eq!(s.sdr_spec(), None);
    }

    #[test]
    fn settings_survive_a_round_trip_and_a_bad_file_is_ignored() {
        let dir = std::env::temp_dir().join(format!("kahf-app-settings-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(load(&dir), json!({}));
        save(&dir, &json!({ "callsign": "G4ABC", "location": { "mode": "off" } })).unwrap();
        assert_eq!(load(&dir)["callsign"], "G4ABC");
        fs::write(dir.join(FILE), "not json").unwrap();
        assert_eq!(load(&dir), json!({}));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn announcements_follow_the_privacy_choice() {
        let v = |mode: &str| serde_json::json!({ "location": { "mode": mode, "lat": 25.2048, "lon": 55.2708 } });
        assert_eq!(shared_position(&v("off")), None);
        assert_eq!(shared_position(&v("area")), Some((25.5, 55.5)));
        assert_eq!(shared_position(&v("precise")), Some((25.2, 55.27)));
        assert_eq!(shared_position(&serde_json::json!({})), None);
        assert_eq!(shared_position(&serde_json::json!({ "location": { "mode": "precise", "lat": null, "lon": null } })), None);
    }
}
