//! Hardware-facing traits. Implementations: rigctld TCP (Hamlib), audio via cpal,
//! SDR IQ via OnAir driver layer (later). Keep these as traits only here.
use std::io::{BufRead, BufReader, ErrorKind, Write};
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
pub trait IqSource {
    fn read_iq(&mut self, out: &mut [(f32, f32)]) -> Result<usize, String>;
    fn sample_rate(&self) -> f64;
}

/// Transmit-side IQ sink. The SDR implementations (HackRF, RTL-SDR, Airspy and
/// others, TX and RX) come from OnAir's driver layer through kahf-ffi later.
/// They are not written here.
pub trait IqSink {
    fn write_iq(&mut self, iq: &[(f32, f32)]) -> Result<(), String>;
    fn sample_rate(&self) -> f64;
}

/// Rig state as shown to the operator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RigStatus {
    pub freq_hz: u64,
    pub mode: String,
    pub passband_hz: u32,
    /// Signal strength in dB relative to S9, or None when the rig does not report it.
    pub strength_db: Option<i32>,
    pub ptt: bool,
}

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
        Ok(Self {
            reader: BufReader::new(stream),
        })
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

    fn send_line(&mut self, cmd: &str) -> Result<(), String> {
        self.reader
            .get_mut()
            .write_all(cmd.as_bytes())
            .map_err(|e| format!("rigctld write: {e}"))
    }

    /// Reads one reply line; a read timeout becomes "rig did not answer".
    fn read_reply_line(&mut self) -> Result<String, String> {
        let mut line = String::new();
        let n = self.reader.read_line(&mut line).map_err(|e| match e.kind() {
            ErrorKind::TimedOut | ErrorKind::WouldBlock => "rig did not answer".to_string(),
            _ => format!("rigctld read: {e}"),
        })?;
        if n == 0 {
            return Err("rigctld closed the connection".to_string());
        }
        Ok(line.trim_end().to_string())
    }

    /// Reads the mode name and passband width in Hz (rigctld command `m`).
    pub fn get_mode(&mut self) -> Result<(String, u32), String> {
        self.send_line("m\n")?;
        let mode = self.read_reply_line()?;
        if mode.starts_with("RPRT") {
            return Err(format!("rigctld error for \"m\": {mode}"));
        }
        let passband = self.read_reply_line()?;
        let width = passband
            .parse::<u32>()
            .map_err(|e| format!("rigctld bad passband reply {passband:?}: {e}"))?;
        Ok((mode, width))
    }

    /// Reads signal strength in dB relative to S9 (rigctld `l STRENGTH`).
    pub fn get_strength_db(&mut self) -> Result<i32, String> {
        self.send_line("l STRENGTH\n")?;
        let reply = self.read_reply_line()?;
        if reply.starts_with("RPRT") {
            return Err(format!("rigctld error for \"l STRENGTH\": {reply}"));
        }
        reply
            .parse::<i32>()
            .map_err(|e| format!("rigctld bad strength reply {reply:?}: {e}"))
    }

    /// Reads the PTT state (rigctld command `t`).
    pub fn get_ptt(&mut self) -> Result<bool, String> {
        self.send_line("t\n")?;
        let reply = self.read_reply_line()?;
        if reply.starts_with("RPRT") {
            return Err(format!("rigctld error for \"t\": {reply}"));
        }
        match reply.as_str() {
            "0" => Ok(false),
            "1" => Ok(true),
            other => Err(format!("rigctld bad PTT reply {other:?}")),
        }
    }

    /// Reads frequency, mode, passband, strength and PTT. A rig that does not
    /// support the strength level gives `strength_db: None` instead of an error.
    pub fn status(&mut self) -> Result<RigStatus, String> {
        let freq_hz = self.get_freq_hz()?;
        let (mode, passband_hz) = self.get_mode()?;
        let strength_db = match self.get_strength_db() {
            Ok(v) => Some(v),
            Err(e) if e.starts_with("rigctld error") => None,
            Err(e) => return Err(e),
        };
        let ptt = self.get_ptt()?;
        Ok(RigStatus {
            freq_hz,
            mode,
            passband_hz,
            strength_db,
            ptt,
        })
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
                let reply = if line.trim_end() == "f" {
                    "14074000\n"
                } else {
                    "RPRT 0\n"
                };
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

    /// Fake rigctld on 127.0.0.1 port 0 that answers f, m, l STRENGTH and t.
    fn fake_rigctld(strength_reply: &'static str) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            let mut seen: Vec<String> = Vec::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 {
                    break;
                }
                let reply = match line.trim_end() {
                    "f" => "14074000\n",
                    "m" => "USB\n2400\n",
                    "l STRENGTH" => strength_reply,
                    "t" => "1\n",
                    other => panic!("unexpected rigctld command {other:?}"),
                };
                seen.push(line);
                writer.write_all(reply.as_bytes()).unwrap();
            }
            seen
        });
        (addr, handle)
    }

    #[test]
    fn rig_status_reads_all_fields_from_fake_server() {
        let (addr, server) = fake_rigctld("-12\n");
        let mut client = RigctldClient::connect(&addr).unwrap();
        let status = client.status().unwrap();
        assert_eq!(
            status,
            RigStatus {
                freq_hz: 14_074_000,
                mode: "USB".to_string(),
                passband_hz: 2400,
                strength_db: Some(-12),
                ptt: true,
            }
        );
        drop(client);
        assert_eq!(server.join().unwrap(), vec!["f\n", "m\n", "l STRENGTH\n", "t\n"]);
    }

    #[test]
    fn rig_status_strength_is_none_when_unsupported() {
        let (addr, server) = fake_rigctld("RPRT -11\n");
        let mut client = RigctldClient::connect(&addr).unwrap();
        let status = client.status().unwrap();
        assert_eq!(status.strength_db, None);
        assert_eq!(status.mode, "USB");
        drop(client);
        server.join().unwrap();
    }

    /// Fake rigctld that answers each command with the given reply, in order, and records the commands.
    fn scripted_rigctld(replies: Vec<&'static str>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            let mut seen = Vec::new();
            for reply in replies {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 {
                    break;
                }
                seen.push(line);
                writer.write_all(reply.as_bytes()).unwrap();
            }
            seen
        });
        (addr, handle)
    }

    #[test]
    fn rigctld_set_ptt_error_reply_is_reported_not_ignored() {
        // A rig that refuses PTT must make set_ptt fail, so the daemon can stop the burst.
        let (addr, server) = scripted_rigctld(vec!["RPRT -6\n"]);
        let mut client = RigctldClient::connect(&addr).unwrap();
        let err = client.set_ptt(true).unwrap_err();
        assert!(err.contains("RPRT -6"), "got {err}");
        drop(client);
        assert_eq!(server.join().unwrap(), vec!["T 1\n"]);
    }

    #[test]
    fn rigctld_get_ptt_reads_both_states() {
        let (addr, server) = scripted_rigctld(vec!["0\n", "1\n"]);
        let mut client = RigctldClient::connect(&addr).unwrap();
        assert!(!client.get_ptt().unwrap());
        assert!(client.get_ptt().unwrap());
        drop(client);
        assert_eq!(server.join().unwrap(), vec!["t\n", "t\n"]);
    }

    #[test]
    fn rigctld_get_ptt_rejects_garbage_reply() {
        let (addr, server) = scripted_rigctld(vec!["maybe\n"]);
        let mut client = RigctldClient::connect(&addr).unwrap();
        assert!(client.get_ptt().unwrap_err().contains("bad PTT reply"));
        drop(client);
        server.join().unwrap();
    }

    #[test]
    fn rigctld_set_freq_error_reply_is_reported() {
        let (addr, server) = scripted_rigctld(vec!["RPRT -8\n"]);
        let mut client = RigctldClient::connect(&addr).unwrap();
        assert!(client.set_freq_hz(145_500_000).is_err());
        drop(client);
        assert_eq!(server.join().unwrap(), vec!["F 145500000\n"]);
    }

    #[test]
    fn rigctld_silent_rig_times_out_with_plain_message() {
        // Accepts the connection and never answers. The read timeout is 2 s.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_secs(3));
            drop(stream);
        });
        let mut client = RigctldClient::connect(&addr).unwrap();
        let err = client.get_ptt().unwrap_err();
        assert_eq!(err, "rig did not answer");
        drop(client);
        server.join().unwrap();
    }

    #[test]
    fn rigctld_connect_refused_names_the_address() {
        // Port 1 on loopback is closed; the connect must fail with the address in the message.
        let err = RigctldClient::connect("127.0.0.1:1")
            .err()
            .expect("connect should fail");
        assert!(err.contains("127.0.0.1:1"), "got {err}");
    }
}
