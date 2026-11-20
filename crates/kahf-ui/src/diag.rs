//! Connection tests for the rig and the sound devices. Each runs only when the
//! operator presses a Test button, and never transmits: the output test opens the
//! device and plays silence.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

const RIG_TIMEOUT: Duration = Duration::from_secs(3);
const LISTEN: Duration = Duration::from_millis(600);
/// Below this the input is silent for practical purposes.
const SILENT_DB: f32 = -90.0;

/// Asks rigctld for the dial frequency. Blocking: call it off the UI thread.
pub fn test_rig(addr: &str) -> Result<String, String> {
    let addr = addr.trim();
    if addr.is_empty() {
        return Err("No rigctld address is set.".into());
    }
    let socks: Vec<_> = addr
        .to_socket_addrs()
        .map_err(|_| format!("\"{addr}\" is not a host:port address."))?
        .collect();
    let mut last = None;
    let mut stream = None;
    for sock in &socks {
        match TcpStream::connect_timeout(sock, RIG_TIMEOUT) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => last = Some(e),
        }
    }
    let mut stream = stream.ok_or_else(|| match last {
        Some(e) => format!("Cannot reach rigctld at {addr}: {e}. Is it running?"),
        None => format!("\"{addr}\" did not resolve."),
    })?;
    let _ = stream.set_read_timeout(Some(RIG_TIMEOUT));
    let _ = stream.set_write_timeout(Some(RIG_TIMEOUT));
    stream
        .write_all(b"f\n")
        .map_err(|e| format!("rigctld did not take the request: {e}"))?;
    let mut line = String::new();
    BufReader::new(&stream)
        .read_line(&mut line)
        .map_err(|e| format!("rigctld did not answer: {e}"))?;
    judge_freq_reply(line.trim())
}

/// Turns rigctld's answer to `f` into a message.
pub fn judge_freq_reply(reply: &str) -> Result<String, String> {
    if reply.is_empty() {
        return Err("rigctld closed the connection without an answer.".into());
    }
    if reply.starts_with("RPRT") {
        return Err(format!(
            "rigctld is running but the radio did not answer ({reply}). Check the cable and the rig model."
        ));
    }
    let hz: u64 = reply
        .parse()
        .map_err(|_| format!("Unexpected answer from rigctld: {reply}"))?;
    Ok(format!("Radio answered. Dial at {:.3} kHz.", hz as f64 / 1000.0))
}

/// Opens the input device and listens briefly. Blocking.
pub fn test_audio_in(device: Option<&str>) -> Result<String, String> {
    let mut input = kahf_audio::AudioIn::open(device).map_err(|e| format!("Cannot open the input: {e}"))?;
    std::thread::sleep(LISTEN);
    if let Some(e) = input.error() {
        return Err(format!("The input stopped: {e}"));
    }
    let mut buf = vec![0.0f32; 16_000];
    let n = input.read(&mut buf);
    judge_input(input.name(), &buf[..n])
}

/// Turns what the input delivered into a message.
pub fn judge_input(name: &str, samples: &[f32]) -> Result<String, String> {
    if samples.is_empty() {
        return Err(format!(
            "{name} opened but no audio arrived. Check that it is not in use elsewhere."
        ));
    }
    let db = kahf_audio::level_db(samples);
    if db < SILENT_DB {
        Ok(format!(
            "{name} works, but it is silent. Check the cable and the volume."
        ))
    } else {
        Ok(format!("{name} works. Level {db:.0} dB."))
    }
}

/// Opens the output device and sends a moment of silence. Blocking.
pub fn test_audio_out(device: Option<&str>) -> Result<String, String> {
    let mut out = kahf_audio::AudioOut::open(device).map_err(|e| format!("Cannot open the output: {e}"))?;
    out.write(&[0.0; 1600])
        .map_err(|e| format!("The output failed: {e}"))?;
    std::thread::sleep(Duration::from_millis(250));
    Ok("The output opened. Nothing was sent to the radio.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn fake_rigctld(answer: &'static str) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let mut line = String::new();
                let _ = BufReader::new(s.try_clone().unwrap()).read_line(&mut line);
                let _ = s.write_all(answer.as_bytes());
            }
        });
        addr
    }

    #[test]
    fn rig_reports_frequency() {
        let addr = fake_rigctld("18110000\n");
        assert_eq!(test_rig(&addr).unwrap(), "Radio answered. Dial at 18110.000 kHz.");
    }

    #[test]
    fn rig_error_reply_blames_the_radio() {
        let addr = fake_rigctld("RPRT -5\n");
        assert!(test_rig(&addr).unwrap_err().contains("radio did not answer"));
    }

    #[test]
    fn rig_closed_connection_is_reported() {
        assert!(test_rig(&fake_rigctld(""))
            .unwrap_err()
            .contains("without an answer"));
    }

    #[test]
    fn rig_refused_says_not_running() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        drop(l);
        assert!(test_rig(&addr).unwrap_err().contains("Is it running"));
    }

    #[test]
    fn rig_bad_address() {
        assert!(test_rig("").is_err());
        assert!(test_rig("not an address").unwrap_err().contains("host:port"));
    }

    #[test]
    fn input_judgement() {
        assert!(judge_input("Mic", &[]).unwrap_err().contains("no audio"));
        assert!(judge_input("Mic", &[0.0; 100]).unwrap().contains("silent"));
        assert!(judge_input("Mic", &[0.1, -0.1, 0.1, -0.1])
            .unwrap()
            .contains("-20 dB"));
    }
}
