//! Receive-side radio drivers for KAHF. One module per radio under `drivers/`.
//!
//! Vendor libraries are loaded at run time (see `drivers::common`), so the
//! program starts on a machine without them. A radio that is missing reports
//! a plain message; it never stops the program.

pub mod drivers;

/// One adjustable gain stage of a radio (LNA, VGA, mixer, tuner, IF, ...).
#[derive(Debug, Clone, PartialEq)]
pub struct GainStage {
    /// Stage name as the radio's library names it. Shown as is.
    pub name: String,
    pub min_db: f32,
    pub max_db: f32,
    /// Step of the hardware in dB. 0.0 when continuous.
    pub step_db: f32,
    /// True when the stage can be left to the radio's own automatic control.
    pub auto: bool,
}

/// How a gain stage is set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GainSetting {
    /// Let the radio choose (only for stages with `auto`).
    Auto,
    /// A fixed value in dB; the driver clamps it to the stage range.
    Manual(f32),
}

/// A radio that was found, or a network radio that was named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Driver name, for example "rtlsdr".
    pub driver: &'static str,
    /// Human name for the operator, for example "RTL-SDR Blog V4".
    pub label: String,
    /// Serial number for USB radios, host:port for network radios, empty if none.
    pub id: String,
}

/// Receive side of one radio. Implementations are used from one thread at a time.
pub trait RxRadio: Send {
    /// Human name of the open radio.
    fn label(&self) -> String;
    /// Tuning range in Hz, inclusive.
    fn freq_range_hz(&self) -> (u64, u64);
    fn set_frequency_hz(&mut self, hz: u64) -> Result<(), String>;
    /// Sample rates the radio offers, in ascending order.
    fn sample_rates(&self) -> Vec<u32>;
    /// Sets the rate closest to `hz` that the radio offers. Returns the rate in use.
    fn set_sample_rate(&mut self, hz: u32) -> Result<u32, String>;
    fn gain_stages(&self) -> Vec<GainStage>;
    /// Sets one stage by its `GainStage::name`.
    fn set_gain(&mut self, stage: &str, setting: GainSetting) -> Result<(), String>;
    /// Starts streaming. Samples arrive through `read_iq`.
    fn start(&mut self) -> Result<(), String>;
    /// Fills `out` with complex samples (I, Q as f32 in -1..1). Waits at most about
    /// 200 ms and returns 0 when nothing arrived. Returns an error only when the radio is lost.
    fn read_iq(&mut self, out: &mut [(f32, f32)]) -> Result<usize, String>;
    fn stop(&mut self);
    /// Samples the radio dropped because the reader was late.
    fn dropped_samples(&self) -> u64 {
        0
    }
}

/// What the operator typed: "driver" or "driver:serial-or-host:port".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RadioSpec {
    pub driver: String,
    pub id: Option<String>,
}

pub fn parse_spec(text: &str) -> Result<RadioSpec, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("no radio named".into());
    }
    let (driver, id) = match text.split_once(':') {
        Some((d, rest)) => (d.trim(), Some(rest.trim().to_string()).filter(|s| !s.is_empty())),
        None => (text, None),
    };
    if !drivers::NAMES.contains(&driver) {
        return Err(format!(
            "unknown radio {driver:?}; known: {}",
            drivers::NAMES.join(", ")
        ));
    }
    Ok(RadioSpec {
        driver: driver.to_string(),
        id,
    })
}

/// Finds the radios this computer can use. Each driver's probe must return quickly.
pub fn probe_all() -> Vec<DeviceInfo> {
    drivers::probe_all()
}

/// Opens a radio. Errors say what to do next.
pub fn open(spec: &RadioSpec) -> Result<Box<dyn RxRadio>, String> {
    drivers::open(&spec.driver, spec.id.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_driver_and_id() {
        assert_eq!(
            parse_spec("rtlsdr").unwrap(),
            RadioSpec {
                driver: "rtlsdr".into(),
                id: None
            }
        );
        assert_eq!(
            parse_spec("rtltcp:192.168.1.5:1234").unwrap(),
            RadioSpec {
                driver: "rtltcp".into(),
                id: Some("192.168.1.5:1234".into())
            }
        );
        assert!(parse_spec("").is_err());
        assert!(parse_spec("nosuchradio").is_err());
    }
}
