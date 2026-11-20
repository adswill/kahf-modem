//! ';'-terminated text CAT: Kenwood (TS-590, TS-890, TS-480), Yaesu (FT-991A, FT-710,
//! FT-891, FTDX10) and the QRP radios that copy Kenwood (QMX, QDX, (tr)uSDX).
use crate::port::Port;
use crate::{Mode, PttMethod, Rig};
use kahf_core::io::RigControl;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_millis(600);

/// How a family words its commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dialect {
    /// Digits in the frequency field.
    pub freq_digits: usize,
    /// Command that keys the transmitter.
    pub tx_on: &'static str,
    pub tx_off: &'static str,
    /// Mode commands as (USB, USB data, LSB, LSB data). None: the rig has no mode command.
    pub modes: Option<[&'static [&'static str]; 4]>,
}

pub const KENWOOD: Dialect = Dialect {
    freq_digits: 11,
    tx_on: "TX0;",
    tx_off: "RX;",
    modes: Some([&["MD2;"], &["MD2;", "DA1;"], &["MD1;"], &["MD1;", "DA1;"]]),
};
/// TS-590 and TS-890: TX1 sends the USB audio (data) input.
pub const KENWOOD_DATA: Dialect = Dialect {
    tx_on: "TX1;",
    ..KENWOOD
};
/// TS-480 and (tr)uSDX (which copies the TS-480): no data-mode command.
pub const TS480: Dialect = Dialect {
    modes: Some([&["MD2;"], &["MD2;"], &["MD1;"], &["MD1;"]]),
    ..KENWOOD
};
/// QMX and QDX stay on their digital mode; the rig keys with TQ.
pub const QRP: Dialect = Dialect {
    freq_digits: 11,
    tx_on: "TQ1;",
    tx_off: "TQ0;",
    modes: None,
};
pub const YAESU: Dialect = Dialect {
    freq_digits: 9,
    tx_on: "TX1;",
    tx_off: "TX0;",
    modes: Some([&["MD02;"], &["MD0C;"], &["MD01;"], &["MD08;"]]),
};

pub struct TextRig {
    port: Box<dyn Port>,
    dialect: Dialect,
    ptt: PttMethod,
    name: String,
}

impl TextRig {
    pub fn new(port: Box<dyn Port>, dialect: Dialect, ptt: PttMethod, name: &str) -> Self {
        TextRig {
            port,
            dialect,
            ptt,
            name: name.into(),
        }
    }

    fn send(&mut self, cmd: &str) -> Result<(), String> {
        self.port.write_all(cmd.as_bytes())
    }

    fn ask(&mut self, cmd: &str) -> Result<String, String> {
        self.port.clear_input();
        self.send(cmd)?;
        for _ in 0..3 {
            let r = self.port.read_until(b';', TIMEOUT)?;
            let s = String::from_utf8_lossy(&r).into_owned();
            // "?;" is the rig's "I did not understand".
            if s == "?;" {
                return Err(format!("{} refused {cmd}", self.name));
            }
            if s.starts_with(&cmd[..2]) {
                return Ok(s);
            }
        }
        Err(format!("{} did not answer {cmd}", self.name))
    }
}

impl RigControl for TextRig {
    fn set_ptt(&mut self, on: bool) -> Result<(), String> {
        match self.ptt {
            PttMethod::Cat => {
                let c = if on {
                    self.dialect.tx_on
                } else {
                    self.dialect.tx_off
                };
                self.send(c)
            }
            other => crate::line_ptt(self.port.as_mut(), other, on),
        }
    }

    fn set_freq_hz(&mut self, hz: u64) -> Result<(), String> {
        let w = self.dialect.freq_digits;
        if hz >= 10u64.pow(w as u32) {
            return Err(format!("{hz} Hz is out of range"));
        }
        self.send(&format!("FA{hz:0w$};"))
    }
}

impl Rig for TextRig {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn freq_hz(&mut self) -> Result<u64, String> {
        let r = self.ask("FA;")?;
        r.trim_start_matches("FA")
            .trim_end_matches(';')
            .parse()
            .map_err(|_| format!("bad frequency reply {r:?}"))
    }

    fn set_mode(&mut self, mode: Mode) -> Result<(), String> {
        let Some(table) = self.dialect.modes else {
            return Ok(()); // the radio is fixed to its digital mode
        };
        let idx = match mode {
            Mode::Usb => 0,
            Mode::UsbData => 1,
            Mode::Lsb => 2,
            Mode::LsbData => 3,
        };
        for c in table[idx] {
            self.send(c)?;
        }
        Ok(())
    }
}
