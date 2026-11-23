//! Byte transport for CAT rigs: a real serial port, or a simulated rig for tests.
use std::io::{Read, Write};
use std::time::{Duration, Instant};

/// A byte pipe to a rig plus its RTS and DTR lines.
pub trait Port: Send {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), String>;
    /// Reads up to and including `end`. Errors when `timeout` passes first.
    fn read_until(&mut self, end: u8, timeout: Duration) -> Result<Vec<u8>, String>;
    fn set_rts(&mut self, on: bool) -> Result<(), String>;
    fn set_dtr(&mut self, on: bool) -> Result<(), String>;
    /// Drops bytes waiting to be read (stale replies).
    fn clear_input(&mut self);
}

/// Real serial port (8N1, no flow control).
pub struct SerialLink {
    port: Box<dyn serialport::SerialPort>,
}

impl SerialLink {
    /// `rts_high` leaves RTS asserted. Yaesu radios use RTS as flow control on their CAT port
    /// (FT-991A menu "CAT RTS" defaults to ENABLE) and stay silent while it is low.
    pub fn open(path: &str, baud: u32, rts_high: bool) -> Result<Self, String> {
        let mut port = serialport::new(path, baud)
            .timeout(Duration::from_millis(50))
            .open()
            .map_err(|e| format!("open {path}: {e}"))?;
        // Lines start low so opening the port never keys the radio, except RTS where it is
        // the radio's flow control and not PTT.
        port.write_request_to_send(rts_high).ok();
        port.write_data_terminal_ready(false).ok();
        Ok(SerialLink { port })
    }
}

impl Port for SerialLink {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.port
            .write_all(bytes)
            .map_err(|e| format!("serial write: {e}"))?;
        self.port.flush().map_err(|e| format!("serial flush: {e}"))
    }

    fn read_until(&mut self, end: u8, timeout: Duration) -> Result<Vec<u8>, String> {
        let start = Instant::now();
        let mut out = Vec::new();
        let mut b = [0u8; 1];
        while start.elapsed() < timeout {
            match self.port.read(&mut b) {
                Ok(1) => {
                    out.push(b[0]);
                    if b[0] == end {
                        return Ok(out);
                    }
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(e) => return Err(format!("serial read: {e}")),
            }
        }
        Err("the rig did not answer in time".into())
    }

    fn set_rts(&mut self, on: bool) -> Result<(), String> {
        self.port
            .write_request_to_send(on)
            .map_err(|e| format!("RTS: {e}"))
    }

    fn set_dtr(&mut self, on: bool) -> Result<(), String> {
        self.port
            .write_data_terminal_ready(on)
            .map_err(|e| format!("DTR: {e}"))
    }

    fn clear_input(&mut self) {
        let _ = self.port.clear(serialport::ClearBuffer::Input);
    }
}
