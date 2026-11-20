//! Simulated rigs. They speak the real wire protocols and never touch hardware or the
//! air, so tests and demos can run the whole rig stack. `SimState` is shared so a test can
//! see what the "radio" did.
use crate::port::Port;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Proto {
    Civ(u8),
    /// ';' text CAT with this many frequency digits.
    Text(usize),
}

#[derive(Debug, Default)]
pub struct SimState {
    pub freq_hz: u64,
    pub transmitting: bool,
    pub rts: bool,
    pub dtr: bool,
    /// Last mode command seen, as the radio's own bytes or text.
    pub last_mode: String,
    /// Every command the radio received.
    pub log: Vec<String>,
}

pub struct SimPort {
    proto: Proto,
    state: Arc<Mutex<SimState>>,
    out: VecDeque<u8>,
    inbuf: Vec<u8>,
}

impl SimPort {
    pub fn new(proto: Proto, start_hz: u64) -> (Self, Arc<Mutex<SimState>>) {
        let state = Arc::new(Mutex::new(SimState {
            freq_hz: start_hz,
            ..Default::default()
        }));
        (
            SimPort {
                proto,
                state: state.clone(),
                out: VecDeque::new(),
                inbuf: Vec::new(),
            },
            state,
        )
    }

    fn civ(&mut self, addr: u8, frame: &[u8]) {
        // frame: to from cmd... (no FE FE, no FD)
        if frame.len() < 3 || frame[0] != addr {
            return;
        }
        let cmd = &frame[2..];
        let mut st = self.state.lock().unwrap();
        st.log.push(format!("{cmd:02X?}"));
        let ack = |out: &mut VecDeque<u8>, code: u8| out.extend([0xFE, 0xFE, 0xE0, addr, code, 0xFD]);
        match cmd {
            [0x05, d @ ..] if d.len() == 5 => {
                st.freq_hz = crate::civ::parse_bcd_freq(d).unwrap_or(st.freq_hz);
                ack(&mut self.out, 0xFB);
            }
            [0x03] => {
                self.out.extend([0xFE, 0xFE, 0xE0, addr, 0x03]);
                self.out.extend(crate::civ::bcd_freq(st.freq_hz));
                self.out.push_back(0xFD);
            }
            [0x06, m, _] => {
                st.last_mode = format!("{m:02X}");
                ack(&mut self.out, 0xFB);
            }
            [0x1A, 0x06, ..] => ack(&mut self.out, 0xFB),
            [0x1C, 0x00, p] => {
                st.transmitting = *p == 1;
                ack(&mut self.out, 0xFB);
            }
            _ => ack(&mut self.out, 0xFA),
        }
    }

    fn text(&mut self, digits: usize, cmd: &str) {
        let mut st = self.state.lock().unwrap();
        st.log.push(cmd.to_string());
        match &cmd[..cmd.len().min(2)] {
            "FA" if cmd.len() == 3 => {
                self.out
                    .extend(format!("FA{:0w$};", st.freq_hz, w = digits).bytes());
            }
            "FA" => {
                if let Ok(f) = cmd[2..cmd.len() - 1].parse() {
                    st.freq_hz = f;
                }
            }
            // Kenwood: TX0 and TX1 key. Yaesu (9 digits): TX1 keys, TX0 releases. QRP: TQ1 keys.
            "TX" => st.transmitting = cmd == "TX1;" || (cmd == "TX0;" && digits == 11),
            "TQ" => st.transmitting = cmd == "TQ1;",
            "RX" => st.transmitting = false,
            "MD" => st.last_mode = cmd.to_string(),
            "DA" => {}
            _ => self.out.extend(b"?;"),
        }
    }
}

impl Port for SimPort {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.inbuf.extend_from_slice(bytes);
        match self.proto {
            Proto::Civ(addr) => {
                while let Some(end) = self.inbuf.iter().position(|b| *b == 0xFD) {
                    let f: Vec<u8> = self.inbuf.drain(..=end).collect();
                    // The bus echoes what the controller sent.
                    self.out.extend(f.iter());
                    if f.len() > 5 {
                        self.civ(addr, &f[2..f.len() - 1]);
                    }
                }
            }
            Proto::Text(d) => {
                while let Some(end) = self.inbuf.iter().position(|b| *b == b';') {
                    let f: Vec<u8> = self.inbuf.drain(..=end).collect();
                    self.text(d, &String::from_utf8_lossy(&f));
                }
            }
        }
        Ok(())
    }

    fn read_until(&mut self, end: u8, _t: Duration) -> Result<Vec<u8>, String> {
        let mut r = Vec::new();
        while let Some(b) = self.out.pop_front() {
            r.push(b);
            if b == end {
                return Ok(r);
            }
        }
        Err("the rig did not answer in time".into())
    }

    fn set_rts(&mut self, on: bool) -> Result<(), String> {
        let mut st = self.state.lock().unwrap();
        st.rts = on;
        st.log.push(format!("RTS {on}"));
        Ok(())
    }

    fn set_dtr(&mut self, on: bool) -> Result<(), String> {
        let mut st = self.state.lock().unwrap();
        st.dtr = on;
        st.log.push(format!("DTR {on}"));
        Ok(())
    }

    fn clear_input(&mut self) {
        self.out.clear();
    }
}
