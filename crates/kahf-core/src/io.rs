//! Hardware-facing traits. Implementations: rigctld TCP (Hamlib), audio via cpal,
//! SDR IQ via OnAir driver layer (later). Keep these as traits only here.
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

/// Default address of a local Hamlib rigctld daemon.
pub const RIGCTLD_DEFAULT_ADDR: &str = "127.0.0.1:4532";

const RIGCTLD_TIMEOUT: Duration = Duration::from_secs(2);

pub trait RigControl {
    fn set_ptt(&mut self, on: bool) -> Result<(), String>;
    fn set_freq_hz(&mut self, hz: u64) -> Result<(), String>;
}

/// Receive-side IQ source. The SDR implementations (HackRF, RTL-SDR, Airspy and
/// others, TX and RX) come from OnAir's driver layer through kahf-ffi later.
/// They are not written here.
pub trait IqSource { fn read_iq(&mut self, out: &mut [(f32, f32)]) -> Result<usize, String>; fn sample_rate(&self) -> f64; }

/// Transmit-side IQ sink. The SDR implementations (HackRF, RTL-SDR, Airspy and
/// others, TX and RX) come from OnAir's driver layer through kahf-ffi later.
/// They are not written here.
pub trait IqSink { fn write_iq(&mut self, iq: &[(f32, f32)]) -> Result<(), String>; fn sample_rate(&self) -> f64; }

/// RigControl over a TCP connection to Hamlib rigctld (line-based protocol).
pub struct RigctldClient {
    reader: BufReader<TcpStream>,
}

impl RigctldClient {
    pub fn connect(addr: &str) -> Result<Self, String> {
        let stream = TcpStream::connect(addr).map_err(|e| format!("rigctld connect {addr}: {e}"))?;
        stream
            .set_read_timeout(Some(RIGCTLD_TIMEOUT))
            .map_err(|e| format!("rigctld set read timeout: {e}"))?;
        stream
            .set_write_timeout(Some(RIGCTLD_TIMEOUT))
            .map_err(|e| format!("rigctld set write timeout: {e}"))?;
        Ok(Self { reader: BufReader::new(stream) })
    }

    /// Sends one command line (including the trailing newline) and returns the reply line.
    fn command(&mut self, cmd: &str) -> Result<String, String> {
        self.reader
            .get_mut()
            .write_all(cmd.as_bytes())
            .map_err(|e| format!("rigctld write: {e}"))?;
        let mut line = String::new();
        let n = self
            .reader
            .read_line(&mut line)
            .map_err(|e| format!("rigctld read: {e}"))?;
        if n == 0 {
            return Err("rigctld closed the connection".to_string());
        }
        Ok(line.trim_end().to_string())
    }

    fn expect_ok(&mut self, cmd: &str) -> Result<(), String> {
        let reply = self.command(cmd)?;
        if reply == "RPRT 0" {
            Ok(())
        } else {
            Err(format!("rigctld error for {:?}: {reply}", cmd.trim_end()))
        }
    }

    /// Reads the current frequency in Hz (rigctld command `f`).
    pub fn get_freq_hz(&mut self) -> Result<u64, String> {
        let reply = self.command("f\n")?;
        if reply.starts_with("RPRT") {
            return Err(format!("rigctld error for \"f\": {reply}"));
        }
        reply
            .parse::<u64>()
            .map_err(|e| format!("rigctld bad frequency reply {reply:?}: {e}"))
    }
}

impl RigControl for RigctldClient {
    fn set_ptt(&mut self, on: bool) -> Result<(), String> {
        self.expect_ok(if on { "T 1\n" } else { "T 0\n" })
    }

    fn set_freq_hz(&mut self, hz: u64) -> Result<(), String> {
        self.expect_ok(&format!("F {hz}\n"))
    }
}

/// In-memory RigControl for tests. Every `set_ptt` call is appended to `ptt`;
/// `freq_hz` holds the last frequency set.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RecordingRig {
    pub ptt: Vec<bool>,
    pub freq_hz: Option<u64>,
}

impl RigControl for RecordingRig {
    fn set_ptt(&mut self, on: bool) -> Result<(), String> {
        self.ptt.push(on);
        Ok(())
    }

    fn set_freq_hz(&mut self, hz: u64) -> Result<(), String> {
        self.freq_hz = Some(hz);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn recording_rig_records_ptt_and_frequency() {
        let mut rig = RecordingRig::default();
        assert_eq!(rig.freq_hz, None);
        rig.set_ptt(true).unwrap();
        rig.set_ptt(false).unwrap();
        rig.set_freq_hz(7_074_000).unwrap();
        rig.set_freq_hz(14_074_000).unwrap();
        assert_eq!(rig.ptt, vec![true, false]);
        assert_eq!(rig.freq_hz, Some(14_074_000));
    }

    #[test]
    fn rigctld_client_against_fake_server() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        // Fake rigctld: replies "RPRT 0" to every command, except "f" which gets a frequency.
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            let mut seen: Vec<String> = Vec::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 {
                    break;
                }
                let reply = if line.trim_end() == "f" { "14074000\n" } else { "RPRT 0\n" };
                seen.push(line);
                writer.write_all(reply.as_bytes()).unwrap();
            }
            seen
        });

        let mut client = RigctldClient::connect(&addr.to_string()).unwrap();
        client.set_ptt(true).unwrap();
        client.set_ptt(false).unwrap();
        client.set_freq_hz(14_074_000).unwrap();
        assert_eq!(client.get_freq_hz().unwrap(), 14_074_000);
        drop(client);

        let seen = server.join().unwrap();
        assert_eq!(seen, vec!["T 1\n", "T 0\n", "F 14074000\n", "f\n"]);
    }
}
