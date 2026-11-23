//! Radio control for KAHF: CAT over serial for the popular HF radios, flrig, and PTT by
//! CAT, RTS, DTR or VOX. Every driver implements `kahf_core::io::RigControl`, so the
//! daemon uses them like rigctld. Simulated rigs in `sim` let tests run the whole stack
//! without hardware and without transmitting.
pub mod civ;
pub mod flrig;
pub mod port;
pub mod qsy;
pub mod sim;
pub mod text;

use kahf_core::io::{RigControl, RigctldClient};
use port::{Port, SerialLink};

/// Sideband and data flag. KAHF runs upper sideband data; the radio's DATA mode routes
/// audio from the USB sound device on most radios.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Usb,
    UsbData,
    Lsb,
    LsbData,
}

/// How the transmitter is keyed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PttMethod {
    /// A CAT command. Needs no extra wiring.
    Cat,
    /// RTS serial line.
    Rts,
    /// DTR serial line.
    Dtr,
    /// The radio keys itself from the audio. KAHF sends nothing.
    Vox,
}

/// A controllable radio.
pub trait Rig: RigControl {
    fn name(&self) -> String;
    fn freq_hz(&mut self) -> Result<u64, String>;
    fn set_mode(&mut self, mode: Mode) -> Result<(), String>;
}

/// Drives PTT with a serial line; VOX does nothing.
pub(crate) fn line_ptt(port: &mut dyn Port, m: PttMethod, on: bool) -> Result<(), String> {
    match m {
        PttMethod::Rts => port.set_rts(on),
        PttMethod::Dtr => port.set_dtr(on),
        PttMethod::Vox | PttMethod::Cat => Ok(()),
    }
}

/// Radios KAHF knows by name: (spec name, label, default baud).
pub const MODELS: &[(&str, &str, u32)] = &[
    ("ic7300", "Icom IC-7300", 115_200),
    ("ic705", "Icom IC-705", 115_200),
    ("ic7610", "Icom IC-7610", 115_200),
    ("ic7100", "Icom IC-7100", 19_200),
    ("ic9700", "Icom IC-9700", 115_200),
    ("ft991a", "Yaesu FT-991A", 38_400),
    ("ft710", "Yaesu FT-710", 38_400),
    ("ft891", "Yaesu FT-891", 38_400),
    ("ftdx10", "Yaesu FTDX10", 38_400),
    ("ts590", "Kenwood TS-590S/SG", 115_200),
    ("ts890", "Kenwood TS-890S", 115_200),
    ("ts480", "Kenwood TS-480", 57_600),
    ("qmx", "QRP Labs QMX", 115_200),
    ("qdx", "QRP Labs QDX", 115_200),
    ("trusdx", "(tr)uSDX", 115_200),
    ("flrig", "any radio through flrig", 0),
    ("rigctld", "any radio through Hamlib rigctld", 0),
];

/// What the operator typed: `model:device[:baud][:ptt]`, or `flrig[:host:port]`,
/// or `rigctld:host:port`. Example: `ic7300:/dev/ttyUSB0:115200:cat`, `ft991a:COM3:rts`.
/// `icom-94:` style names a CI-V radio by hex address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RigSpec {
    pub model: String,
    pub device: String,
    pub baud: Option<u32>,
    pub ptt: PttMethod,
}

pub fn parse_ptt(s: &str) -> Result<PttMethod, String> {
    match s.to_ascii_lowercase().as_str() {
        "cat" => Ok(PttMethod::Cat),
        "rts" => Ok(PttMethod::Rts),
        "dtr" => Ok(PttMethod::Dtr),
        "vox" | "none" => Ok(PttMethod::Vox),
        o => Err(format!("unknown PTT method {o:?} (use cat, rts, dtr or vox)")),
    }
}

pub fn parse_spec(text: &str) -> Result<RigSpec, String> {
    let mut parts: Vec<&str> = text.trim().split(':').collect();
    let model = parts.remove(0).to_ascii_lowercase();
    let net = model == "flrig" || model == "rigctld";
    let mut ptt = PttMethod::Cat;
    if let Some(last) = parts.last() {
        if let Ok(p) = parse_ptt(last) {
            ptt = p;
            parts.pop();
        }
    }
    let mut baud = None;
    if !net {
        if let Some(b) = parts.last().and_then(|p| p.parse::<u32>().ok()) {
            if parts.len() >= 2 {
                baud = Some(b);
                parts.pop();
            }
        }
    }
    let device = parts.join(":");
    if model.is_empty() {
        return Err("no radio named".into());
    }
    if !net && model_info(&model).is_none() {
        return Err(format!("unknown radio {model:?}; known: {}", known_models()));
    }
    if device.is_empty() && !net {
        return Err("no serial device given (for example /dev/ttyUSB0 or COM3)".into());
    }
    Ok(RigSpec {
        model,
        device,
        baud,
        ptt,
    })
}

pub fn known_models() -> String {
    MODELS.iter().map(|m| m.0).collect::<Vec<_>>().join(", ")
}

fn civ_addr(model: &str) -> Option<u8> {
    match model {
        "ic7300" => Some(0x94),
        "ic705" => Some(0xA4),
        "ic7610" => Some(0x98),
        "ic7100" => Some(0x88),
        "ic9700" => Some(0xA2),
        m => u8::from_str_radix(m.strip_prefix("icom-")?, 16).ok(),
    }
}

fn dialect(model: &str) -> Option<text::Dialect> {
    Some(match model {
        "ft991a" | "ft710" | "ft891" | "ftdx10" => text::YAESU,
        "ts590" | "ts890" => text::KENWOOD_DATA,
        "ts480" | "trusdx" => text::TS480,
        "qmx" | "qdx" => text::QRP,
        _ => return None,
    })
}

fn model_info(model: &str) -> Option<(&'static str, u32)> {
    if let Some(m) = MODELS.iter().find(|m| m.0 == model) {
        return Some((m.1, m.2));
    }
    civ_addr(model).map(|_| ("Icom CI-V radio", 19_200))
}

/// Builds a rig on an already open port. Used by `open` and by tests with `sim::SimPort`.
pub fn with_port(spec: &RigSpec, port: Box<dyn Port>) -> Result<Box<dyn Rig>, String> {
    let (label, _) = model_info(&spec.model).ok_or_else(|| format!("unknown radio {:?}", spec.model))?;
    if let Some(a) = civ_addr(&spec.model) {
        return Ok(Box::new(civ::CivRig::new(port, a, spec.ptt, label)));
    }
    let d = dialect(&spec.model).ok_or_else(|| format!("{} needs no serial port", spec.model))?;
    Ok(Box::new(text::TextRig::new(port, d, spec.ptt, label)))
}

/// Opens the rig the operator described.
pub fn open(text: &str) -> Result<Box<dyn Rig>, String> {
    let spec = parse_spec(text)?;
    match spec.model.as_str() {
        "flrig" => {
            let addr = if spec.device.is_empty() {
                flrig::DEFAULT_ADDR
            } else {
                &spec.device
            };
            Ok(Box::new(flrig::FlrigClient::connect(addr, spec.ptt)?))
        }
        "rigctld" => {
            let addr = if spec.device.is_empty() {
                kahf_core::io::RIGCTLD_DEFAULT_ADDR
            } else {
                &spec.device
            };
            Ok(Box::new(RigctldRig(RigctldClient::connect(addr)?)))
        }
        m => {
            let baud = spec.baud.or_else(|| model_info(m).map(|i| i.1)).unwrap_or(9600);
            // Yaesu CAT ports use RTS as flow control; keep it asserted unless RTS is the PTT line.
            let rts_high = rts_high_on_open(m, spec.ptt);
            with_port(&spec, Box::new(SerialLink::open(&spec.device, baud, rts_high)?))
        }
    }
}

/// Hamlib rigctld as a `Rig`.
struct RigctldRig(RigctldClient);

impl RigControl for RigctldRig {
    fn set_ptt(&mut self, on: bool) -> Result<(), String> {
        self.0.set_ptt(on)
    }
    fn set_freq_hz(&mut self, hz: u64) -> Result<(), String> {
        self.0.set_freq_hz(hz)
    }
}

impl Rig for RigctldRig {
    fn name(&self) -> String {
        "rigctld".into()
    }
    fn freq_hz(&mut self) -> Result<u64, String> {
        self.0.get_freq_hz()
    }
    fn set_mode(&mut self, _mode: Mode) -> Result<(), String> {
        Ok(()) // rigctld's set-mode needs a radio-specific passband; leave the mode as is.
    }
}

/// Whether RTS is asserted when the serial port opens. Yaesu CAT ports use RTS as flow
/// control and stay silent while it is low; everywhere else, and whenever RTS is the PTT line,
/// it starts low so that opening the port never keys the radio.
fn rts_high_on_open(model: &str, ptt: PttMethod) -> bool {
    dialect(model) == Some(text::YAESU) && ptt != PttMethod::Rts
}

#[cfg(test)]
mod rts_tests {
    use super::*;

    #[test]
    fn yaesu_cat_ports_open_with_rts_high_unless_rts_is_the_ptt_line() {
        assert!(rts_high_on_open("ft991a", PttMethod::Cat));
        assert!(!rts_high_on_open("ft991a", PttMethod::Rts));
        assert!(!rts_high_on_open("ic7300", PttMethod::Cat));
    }
}

#[cfg(test)]
mod tests;
