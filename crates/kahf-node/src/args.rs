//! Command line for the headless node. No external parser: the surface is small and the
//! messages are written for an operator on a terminal.
use kahf_core::relay::{Radio, Role};
use std::path::PathBuf;

pub const USAGE: &str = "\
kahf-node: run KAHF as a station or an unattended relay on 17 m.

  kahf-node run --call CALL --data-dir DIR [options]
  kahf-node audio-check [--audio-in DEVICE]   find the radio's sound card and check the level
  kahf-node devices                           list sound devices
  kahf-node rigs                              list supported radios

run options:
  --call CALL            your callsign
  --data-dir DIR         where KAHF keeps its files
  --rig SPEC             radio control, e.g. ic7300:/dev/ttyUSB0:115200:cat, flrig, 127.0.0.1:4532
  --audio-in DEVICE      sound input (default: system default)
  --audio-out DEVICE     sound output (default: system default)
  --sdr SPEC             use an SDR instead of a sound card, e.g. rtlsdr@18111000
  --slot N               relay slot 0..2 to tune the radio to before starting (default 0)
  --no-qsy               do not tune the radio
  --relay                run as a relay (also needs both flags below)
  --station-too          also act as a normal station while relaying
  --radio half|full|sdr  half duplex (default), full duplex, or SDR receive assist
  --i-hold-a-licence     confirm you hold an amateur licence for this station
  --accept-relay-terms   accept the terms of relay operation
  --api ADDR             scripting API on a loopback address, e.g. 127.0.0.1:7373 (JSON lines)
  --seconds N            stop after N seconds (default: run until stopped)
";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunArgs {
    pub call: String,
    pub data_dir: PathBuf,
    pub rig: Option<String>,
    pub audio_in: Option<String>,
    pub audio_out: Option<String>,
    pub sdr: Option<String>,
    pub slot: usize,
    pub qsy: bool,
    pub relay: Option<Role>,
    pub radio: Radio,
    pub licence: bool,
    pub terms: bool,
    pub seconds: Option<u64>,
    pub api: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Run(Box<RunArgs>),
    AudioCheck { audio_in: Option<String> },
    Devices,
    Rigs,
    Help,
}

pub fn parse(args: &[String]) -> Result<Command, String> {
    let Some((cmd, rest)) = args.split_first() else {
        return Ok(Command::Help);
    };
    match cmd.as_str() {
        "-h" | "--help" | "help" => Ok(Command::Help),
        "devices" => Ok(Command::Devices),
        "rigs" => Ok(Command::Rigs),
        "audio-check" => {
            let mut audio_in = None;
            let mut it = rest.iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--audio-in" => audio_in = Some(value(&mut it, a)?),
                    o => return Err(format!("unknown option {o:?}")),
                }
            }
            Ok(Command::AudioCheck { audio_in })
        }
        "run" => parse_run(rest).map(|r| Command::Run(Box::new(r))),
        o => Err(format!("unknown command {o:?}")),
    }
}

fn value<'a>(it: &mut impl Iterator<Item = &'a String>, flag: &str) -> Result<String, String> {
    it.next().cloned().ok_or_else(|| format!("{flag} needs a value"))
}

fn parse_run(rest: &[String]) -> Result<RunArgs, String> {
    let mut a = RunArgs {
        call: String::new(),
        data_dir: PathBuf::new(),
        rig: None,
        audio_in: None,
        audio_out: None,
        sdr: None,
        slot: 0,
        qsy: true,
        relay: None,
        radio: Radio::HalfDuplex,
        licence: false,
        terms: false,
        seconds: None,
        api: None,
    };
    let mut station_too = false;
    let mut relay = false;
    let mut it = rest.iter();
    while let Some(f) = it.next() {
        match f.as_str() {
            "--call" => a.call = value(&mut it, f)?.to_ascii_uppercase(),
            "--data-dir" => a.data_dir = value(&mut it, f)?.into(),
            "--rig" => a.rig = Some(value(&mut it, f)?),
            "--audio-in" => a.audio_in = Some(value(&mut it, f)?),
            "--audio-out" => a.audio_out = Some(value(&mut it, f)?),
            "--sdr" => a.sdr = Some(value(&mut it, f)?),
            "--slot" => {
                a.slot = value(&mut it, f)?
                    .parse()
                    .map_err(|_| "--slot wants a number".to_string())?
            }
            "--no-qsy" => a.qsy = false,
            "--relay" => relay = true,
            "--station-too" => station_too = true,
            "--radio" => {
                a.radio = match value(&mut it, f)?.as_str() {
                    "half" => Radio::HalfDuplex,
                    "full" => Radio::FullDuplex,
                    "sdr" => Radio::SdrReceiveAssist,
                    o => return Err(format!("--radio is half, full or sdr, not {o:?}")),
                }
            }
            "--i-hold-a-licence" => a.licence = true,
            "--accept-relay-terms" => a.terms = true,
            "--api" => a.api = Some(value(&mut it, f)?),
            "--seconds" => {
                a.seconds = Some(
                    value(&mut it, f)?
                        .parse()
                        .map_err(|_| "--seconds wants a number".to_string())?,
                )
            }
            o => return Err(format!("unknown option {o:?}")),
        }
    }
    if a.call.is_empty() {
        return Err("--call is required".into());
    }
    if a.data_dir.as_os_str().is_empty() {
        return Err("--data-dir is required".into());
    }
    if relay {
        a.relay = Some(if station_too {
            Role::RelayAndStation
        } else {
            Role::Relay
        });
        if !(a.licence && a.terms) {
            return Err(
                "relay mode needs --i-hold-a-licence and --accept-relay-terms; nothing was started".into(),
            );
        }
    } else if station_too {
        return Err("--station-too only makes sense with --relay".into());
    }
    if a.slot >= kahf_rig::qsy::relay_slot_count() {
        return Err(format!(
            "--slot must be 0 to {}",
            kahf_rig::qsy::relay_slot_count() - 1
        ));
    }
    Ok(a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Result<Command, String> {
        parse(&s.split_whitespace().map(String::from).collect::<Vec<_>>())
    }

    #[test]
    fn relay_needs_both_confirmations() {
        let e = p("run --call oz1abc --data-dir /d --relay --i-hold-a-licence").unwrap_err();
        assert!(e.contains("--accept-relay-terms"));
        let Command::Run(a) = p("run --call oz1abc --data-dir /d --relay --i-hold-a-licence --accept-relay-terms --slot 2 --rig ic7300:/dev/ttyUSB0").unwrap() else {
            panic!()
        };
        assert_eq!(
            (a.call.as_str(), a.slot, a.relay),
            ("OZ1ABC", 2, Some(Role::Relay))
        );
    }

    #[test]
    fn plain_station_and_errors() {
        let Command::Run(a) = p("run --call N0CALL --data-dir /d").unwrap() else {
            panic!()
        };
        assert!(a.relay.is_none() && a.qsy);
        assert!(p("run --data-dir /d").is_err());
        assert!(p("run --call X --data-dir /d --slot 3").is_err());
        assert!(p("run --call X --data-dir /d --station-too").is_err());
        assert!(p("run --call X --data-dir /d --bogus").is_err());
        let Command::Run(a) = p("run --call X --data-dir /d --api 127.0.0.1:7373").unwrap() else {
            panic!()
        };
        assert_eq!(a.api.as_deref(), Some("127.0.0.1:7373"));
        assert!(p("run --call").is_err());
        assert_eq!(p("").unwrap(), Command::Help);
        assert_eq!(p("rigs").unwrap(), Command::Rigs);
    }
}
