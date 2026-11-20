//! Icom CI-V: IC-7300, IC-705, IC-7610, IC-7100, IC-9700 and any other CI-V radio.
use crate::port::Port;
use crate::{Mode, PttMethod, Rig};
use kahf_core::io::RigControl;
use std::time::Duration;

const CONTROLLER: u8 = 0xE0;
const TIMEOUT: Duration = Duration::from_millis(600);

pub struct CivRig {
    port: Box<dyn Port>,
    addr: u8,
    ptt: PttMethod,
    name: String,
}

pub fn bcd_freq(hz: u64) -> [u8; 5] {
    let mut v = hz % 10_000_000_000;
    let mut out = [0u8; 5];
    for b in out.iter_mut() {
        *b = ((v / 10 % 10) as u8) << 4 | (v % 10) as u8;
        v /= 100;
    }
    out
}

pub fn parse_bcd_freq(d: &[u8]) -> Option<u64> {
    if d.len() != 5 {
        return None;
    }
    let mut hz = 0u64;
    for b in d.iter().rev() {
        let (hi, lo) = (b >> 4, b & 15);
        if hi > 9 || lo > 9 {
            return None;
        }
        hz = hz * 100 + (hi * 10 + lo) as u64;
    }
    Some(hz)
}

impl CivRig {
    pub fn new(port: Box<dyn Port>, addr: u8, ptt: PttMethod, name: &str) -> Self {
        CivRig {
            port,
            addr,
            ptt,
            name: name.into(),
        }
    }

    fn frame(&self, cmd: &[u8]) -> Vec<u8> {
        let mut f = vec![0xFE, 0xFE, self.addr, CONTROLLER];
        f.extend_from_slice(cmd);
        f.push(0xFD);
        f
    }

    /// Sends one command and returns the radio's reply payload (without addresses and 0xFD).
    /// The CI-V bus echoes our own frame, so frames not addressed to us are skipped.
    fn talk(&mut self, cmd: &[u8]) -> Result<Vec<u8>, String> {
        self.port.clear_input();
        let f = self.frame(cmd);
        self.port.write_all(&f)?;
        for _ in 0..4 {
            let r = self.port.read_until(0xFD, TIMEOUT)?;
            let Some(start) = r.windows(2).position(|w| w == [0xFE, 0xFE]) else {
                continue;
            };
            let body = &r[start + 2..r.len() - 1];
            if body.len() >= 3 && body[0] == CONTROLLER && body[1] == self.addr {
                return Ok(body[2..].to_vec());
            }
        }
        Err(format!(
            "{} did not answer (check the CI-V address and baud rate)",
            self.name
        ))
    }

    fn expect_ack(&mut self, cmd: &[u8]) -> Result<(), String> {
        match self.talk(cmd)?.first() {
            Some(0xFB) => Ok(()),
            Some(0xFA) => Err(format!("{} refused the command", self.name)),
            _ => Err(format!("{} sent an unexpected reply", self.name)),
        }
    }
}

impl RigControl for CivRig {
    fn set_ptt(&mut self, on: bool) -> Result<(), String> {
        match self.ptt {
            PttMethod::Cat => self.expect_ack(&[0x1C, 0x00, on as u8]),
            other => crate::line_ptt(self.port.as_mut(), other, on),
        }
    }

    fn set_freq_hz(&mut self, hz: u64) -> Result<(), String> {
        let mut c = vec![0x05];
        c.extend_from_slice(&bcd_freq(hz));
        self.expect_ack(&c)
    }
}

impl Rig for CivRig {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn freq_hz(&mut self) -> Result<u64, String> {
        let r = self.talk(&[0x03])?;
        match r.split_first() {
            Some((0x03, d)) => parse_bcd_freq(d).ok_or_else(|| "bad frequency reply".to_string()),
            _ => Err("unexpected frequency reply".into()),
        }
    }

    fn set_mode(&mut self, mode: Mode) -> Result<(), String> {
        let code = match mode {
            Mode::Lsb | Mode::LsbData => 0x00,
            Mode::Usb | Mode::UsbData => 0x01,
        };
        self.expect_ack(&[0x06, code, 0x01])?;
        // Data mode on, filter 1 (IC-7300, IC-705, IC-7610, IC-9700). Older radios
        // refuse it; plain USB then still works through the USB audio device.
        let data = matches!(mode, Mode::UsbData | Mode::LsbData) as u8;
        let _ = self.expect_ack(&[0x1A, 0x06, data, 0x01]);
        Ok(())
    }
}
