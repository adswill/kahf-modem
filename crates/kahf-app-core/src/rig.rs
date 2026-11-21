//! Radio control through Hamlib.
//!
//! The person picks their radio by name. KAHF reads Hamlib's list of radios (`rigctl -l`), starts
//! `rigctld` for the chosen model and serial port, and talks to it on a local port. A test reads
//! the radio's frequency, so a wrong cable or speed shows up straight away.

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde::Serialize;

/// The local port KAHF's own `rigctld` listens on.
pub const RIGCTLD_PORT: u16 = 4532;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RigModel {
    pub id: u32,
    pub maker: String,
    pub model: String,
    /// Hamlib's own judgement: Stable, Beta, Alpha, Untested.
    pub status: String,
}

/// Parses `rigctl -l`:
/// ` 3073  Icom                   IC-7300                 20250517.14     Stable      RIG_MODEL_IC7300`
pub fn parse_rig_list(text: &str) -> Vec<RigModel> {
    let mut out: Vec<RigModel> = text
        .lines()
        .filter_map(|line| {
            let cols: Vec<&str> = line
                .trim()
                .split("  ")
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .collect();
            if cols.len() < 5 {
                return None;
            }
            let id: u32 = cols[0].parse().ok()?;
            // Version, status and macro are the last three columns; the maker and model are what is between.
            let n = cols.len();
            let (status, _version) = (cols[n - 2], cols[n - 3]);
            let model = cols[2..n - 3].join(" ");
            if model.is_empty() {
                return None;
            }
            Some(RigModel {
                id,
                maker: cols[1].to_string(),
                model,
                status: status.to_string(),
            })
        })
        .collect();
    out.sort_by_key(|m| (m.maker.to_lowercase(), m.model.to_lowercase()));
    out
}

/// Finds a Hamlib program on the PATH or in the usual install folders.
pub fn find_tool(name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    for extra in [
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/usr/bin",
        "/opt/local/bin",
        "C:\\Program Files\\hamlib\\bin",
        "C:\\Program Files (x86)\\hamlib\\bin",
    ] {
        dirs.push(PathBuf::from(extra));
    }
    dirs.into_iter().map(|d| d.join(&exe)).find(|p| p.is_file())
}

pub fn list_rigs() -> Result<Vec<RigModel>, String> {
    let tool = find_tool("rigctl").ok_or("Hamlib is not installed.")?;
    let out = Command::new(tool)
        .arg("-l")
        .output()
        .map_err(|e| format!("Could not run rigctl: {e}"))?;
    let rigs = parse_rig_list(&String::from_utf8_lossy(&out.stdout));
    if rigs.is_empty() {
        return Err("Hamlib gave an empty list of radios.".into());
    }
    Ok(rigs)
}

/// Serial ports that might have a radio on them, most likely first.
pub fn filter_ports(names: &[String]) -> Vec<String> {
    let mut ports: Vec<String> = names
        .iter()
        .filter(|n| {
            let l = n.to_lowercase();
            (l.starts_with("cu.")
                || l.starts_with("ttyusb")
                || l.starts_with("ttyacm")
                || l.starts_with("ttys") && l.len() <= 6)
                && !l.contains("bluetooth")
                && !l.contains("debug-console")
        })
        .map(|n| format!("/dev/{n}"))
        .collect();
    ports.sort_by_key(|p| {
        (
            !(p.contains("usb") || p.contains("USB") || p.contains("ACM")),
            p.clone(),
        )
    });
    ports
}

pub fn serial_ports() -> Vec<String> {
    if cfg!(windows) {
        return (1..=16).map(|n| format!("COM{n}")).collect();
    }
    let names: Vec<String> = std::fs::read_dir("/dev")
        .map(|d| {
            d.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    filter_ports(&names)
}

/// The frequency in Hz that the radio behind `rigctld` at `addr` reports.
pub fn read_frequency(addr: SocketAddr) -> Result<u64, String> {
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(2))
        .map_err(|e| format!("Cannot reach the radio control program: {e}"))?;
    s.set_read_timeout(Some(Duration::from_secs(4))).ok();
    s.write_all(b"f\n").map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(&s)
        .read_line(&mut line)
        .map_err(|e| format!("The radio did not answer: {e}"))?;
    let l = line.trim();
    if l.starts_with("RPRT") {
        return Err(format!(
            "The radio refused: {l}. Check the model, cable and speed."
        ));
    }
    l.parse::<u64>()
        .map_err(|_| format!("The radio said something unexpected: {l:?}"))
}

/// Starts `rigctld` for the chosen radio. The caller keeps the child and stops it later.
pub fn start_rigctld(model: u32, port: &str, baud: u32) -> Result<Child, String> {
    let tool = find_tool("rigctld").ok_or("Hamlib is not installed.")?;
    let mut child = Command::new(tool)
        .args([
            "-m",
            &model.to_string(),
            "-r",
            port,
            "-s",
            &baud.to_string(),
            "-t",
            &RIGCTLD_PORT.to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start the radio control program: {e}"))?;
    // It exits at once when the port or model is wrong.
    std::thread::sleep(Duration::from_millis(700));
    if let Ok(Some(_)) = child.try_wait() {
        let mut why = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = std::io::Read::read_to_string(&mut e, &mut why);
        }
        let why = why
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("it stopped at once")
            .trim()
            .to_string();
        return Err(format!("The radio control program stopped: {why}"));
    }
    Ok(child)
}

pub fn local_addr() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], RIGCTLD_PORT))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    const SAMPLE: &str = " Rig #  Mfg                    Model                   Version         Status      Macro\n     1  Hamlib                 Dummy                   20250101.0      Stable      RIG_MODEL_DUMMY\n  1035  Yaesu                  FT-991                  20241118.18     Stable      RIG_MODEL_FT991\n  3073  Icom                   IC-7300                 20250517.14     Stable      RIG_MODEL_IC7300\n  3094  Icom                   IC-7300MK2              20250517.14     Beta        RIG_MODEL_IC7300MK2\n  2029  Elecraft               K3                      20250515.31     Stable      RIG_MODEL_K3\n   999  Foo                    Weird Model Name X 2    1.0             Alpha       RIG_MODEL_FOO\n";

    #[test]
    fn parses_hamlibs_list() {
        let r = parse_rig_list(SAMPLE);
        assert_eq!(r.len(), 6, "{r:?}");
        let ic = r.iter().find(|m| m.id == 3073).unwrap();
        assert_eq!(
            (ic.maker.as_str(), ic.model.as_str(), ic.status.as_str()),
            ("Icom", "IC-7300", "Stable")
        );
        let w = r.iter().find(|m| m.id == 999).unwrap();
        assert_eq!(w.model, "Weird Model Name X 2");
        assert_eq!(w.status, "Alpha");
        // Sorted by maker then model.
        let makers: Vec<&str> = r.iter().map(|m| m.maker.as_str()).collect();
        assert_eq!(makers, ["Elecraft", "Foo", "Hamlib", "Icom", "Icom", "Yaesu"]);
    }

    #[test]
    fn ignores_the_header_and_junk() {
        assert!(parse_rig_list("Rig # Mfg Model\nnonsense\n\n").is_empty());
    }

    #[test]
    fn only_plausible_serial_ports_are_listed_usb_first() {
        let names: Vec<String> = [
            "cu.Bluetooth-Incoming-Port",
            "cu.debug-console",
            "cu.usbserial-1420",
            "ttyUSB0",
            "tty",
            "ttyS0",
            "disk0",
            "cu.SLAB_USBtoUART",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let p = filter_ports(&names);
        assert!(p.contains(&"/dev/cu.usbserial-1420".to_string()) && p.contains(&"/dev/ttyUSB0".to_string()));
        assert!(!p
            .iter()
            .any(|x| x.contains("Bluetooth") || x.contains("debug") || x.contains("disk")));
        assert!(p[0].to_lowercase().contains("usb"), "{p:?}");
    }

    fn fake_rigctld(reply: &'static str) -> SocketAddr {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let mut buf = [0u8; 8];
                let _ = std::io::Read::read(&mut s, &mut buf);
                let _ = s.write_all(reply.as_bytes());
            }
        });
        addr
    }

    #[test]
    fn reads_the_radios_frequency() {
        assert_eq!(read_frequency(fake_rigctld("14103000\n")).unwrap(), 14_103_000);
    }

    #[test]
    fn a_refusing_or_silent_radio_gives_advice_not_a_crash() {
        let e = read_frequency(fake_rigctld("RPRT -6\n")).unwrap_err();
        assert!(e.contains("refused") && e.contains("cable"), "{e}");
        let e = read_frequency(fake_rigctld("hello\n")).unwrap_err();
        assert!(e.contains("unexpected"), "{e}");
        let closed = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        assert!(read_frequency(closed).unwrap_err().contains("Cannot reach"));
    }

    #[test]
    fn real_hamlib_list_parses_when_installed() {
        if find_tool("rigctl").is_none() {
            return;
        }
        let rigs = list_rigs().unwrap();
        assert!(rigs.len() > 200, "{} radios", rigs.len());
        assert!(rigs.iter().any(|m| m.maker == "Icom" && m.model == "IC-7300"));
        assert!(rigs
            .iter()
            .any(|m| m.maker == "Yaesu" && m.model.starts_with("FT-991")));
    }
}
