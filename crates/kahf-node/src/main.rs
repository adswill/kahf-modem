use kahf_audio::{level_db, AudioIn};
use kahf_core::relay::Role;
use kahf_daemon::{Config, Engine, Event, RelayAck};
use kahf_node::api::{self, Station};
use kahf_node::args::{self, Command, RunArgs, USAGE};
use kahf_node::wizard;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let code = match args::parse(&argv).and_then(execute) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("kahf-node: {e}");
            2
        }
    };
    std::process::exit(code);
}

fn execute(cmd: Command) -> Result<(), String> {
    match cmd {
        Command::Help => {
            print!("{USAGE}");
            Ok(())
        }
        Command::Devices => {
            for d in kahf_audio::list_devices() {
                println!("{d}");
            }
            Ok(())
        }
        Command::Rigs => {
            for (name, label, _) in kahf_rig::MODELS {
                println!("{name:10} {label}");
            }
            Ok(())
        }
        Command::AudioCheck { audio_in } => audio_check(audio_in),
        Command::Run(a) => run(&a),
    }
}

fn audio_check(audio_in: Option<String>) -> Result<(), String> {
    let devices = kahf_audio::list_devices();
    let found = wizard::detect_rig_audio(&devices);
    match found.first() {
        Some(d) => println!("Likely radio sound card: {d}"),
        None => println!(
            "No radio sound card found among {} devices. Is the radio on and plugged in?",
            devices.len()
        ),
    }
    let dev = audio_in.or_else(|| found.first().cloned());
    let mut input = AudioIn::open(dev.as_deref())?;
    println!(
        "Listening on {} for 4 seconds. Nothing is transmitted.",
        input.name()
    );
    let end = Instant::now() + Duration::from_secs(4);
    let mut all = Vec::new();
    let mut buf = [0f32; 4096];
    while Instant::now() < end {
        let n = input.read(&mut buf);
        all.extend_from_slice(&buf[..n]);
        std::thread::sleep(Duration::from_millis(20));
    }
    if let Some(e) = input.error() {
        return Err(e);
    }
    let peak = all.iter().fold(0f32, |m, v| m.max(v.abs()));
    let rms = level_db(&all);
    let lvl = wizard::judge_level(rms, peak);
    println!("Level {rms:.1} dBFS, peak {peak:.2}. {}", wizard::advice(&lvl));
    println!("{}", wizard::TX_ADVICE);
    Ok(())
}

fn run(a: &RunArgs) -> Result<(), String> {
    if a.qsy {
        if let Some(spec) = a.rig.as_deref().filter(|s| kahf_rig::parse_spec(s).is_ok()) {
            // Tune with a short-lived connection; the engine opens its own for PTT.
            let mut rig = kahf_rig::open(spec)?;
            let dial = kahf_rig::qsy::qsy_to_relay_slot(rig.as_mut(), a.slot)?;
            println!(
                "Tuned {} to {} Hz (17 m relay slot {}).",
                rig.name(),
                dial,
                a.slot
            );
        } else if a.rig.is_some() {
            println!("The rig is a rigctld address; tune it to 17 m yourself (use --no-qsy to hide this).");
        }
    }
    let mut engine = Engine::new(Config {
        callsign: a.call.clone(),
        audio_in: a.audio_in.clone(),
        audio_out: a.audio_out.clone(),
        rigctld: a.rig.clone(),
        sdr: a.sdr.clone(),
        data_dir: a.data_dir.clone(),
    })?;
    if let Some(role) = a.relay {
        let centre = kahf_rig::qsy::relay_slot_centre_hz(a.slot).ok_or("bad slot")? as f32 / 1000.0;
        engine.configure_relay(role, a.radio, &[centre])?;
        engine.start_relay(RelayAck {
            licence_held: a.licence,
            terms_accepted: a.terms,
        })?;
        println!(
            "Relay running as {} ({}).",
            a.call,
            if role == Role::Relay {
                "relay"
            } else {
                "relay and station"
            }
        );
    }
    engine.listen()?;
    println!("Listening. Press Ctrl+C to stop.");
    let stop = Arc::new(AtomicBool::new(false));
    let s2 = stop.clone();
    ctrlc::set_handler(move || s2.store(true, Ordering::SeqCst)).map_err(|e| e.to_string())?;
    let api_rx = match &a.api {
        Some(addr) => {
            let (rx, bound) = api::serve(addr)?;
            println!("Scripting API on {bound}.");
            Some(rx)
        }
        None => None,
    };
    let mut node = Node {
        engine,
        args: a.clone(),
        log: Vec::new(),
        seq: 0,
    };
    let until = a.seconds.map(|s| Instant::now() + Duration::from_secs(s));
    while !stop.load(Ordering::SeqCst) && until.is_none_or(|u| Instant::now() < u) {
        node.pump();
        if let Some(rx) = &api_rx {
            while let Ok((req, reply)) = rx.try_recv() {
                let _ = reply.send(api::handle(&mut node, req));
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    node.engine.stop_relay();
    println!("Stopped.");
    Ok(())
}

/// The running station, as the scripting API sees it.
struct Node {
    engine: Engine,
    args: RunArgs,
    /// Recent events as text, newest last, each with a sequence number.
    log: Vec<(u64, String)>,
    seq: u64,
}

impl Node {
    fn note(&mut self, line: String) {
        println!("{line}");
        self.seq += 1;
        self.log.push((self.seq, line));
        if self.log.len() > 200 {
            self.log.remove(0);
        }
    }

    fn pump(&mut self) {
        for ev in self.engine.poll_events() {
            match ev {
                Event::Log(m) => self.note(m),
                Event::Message { from, text } => self.note(format!("message from {from}: {text}")),
                Event::Relay { call, online, .. } => self.note(format!(
                    "relay {call} {}",
                    if online { "online" } else { "offline" }
                )),
                Event::Tx { state } => self.note(format!("tx: {state:?}")),
                _ => {}
            }
        }
    }
}

impl Station for Node {
    fn status(&mut self) -> Value {
        json!({
            "call": self.args.call,
            "relay_running": self.engine.relay_running(),
            "rig": self.args.rig,
        })
    }

    fn send(&mut self, to: &str, text: &str) -> Result<(), String> {
        self.engine.send_message(to, text)
    }

    fn events(&mut self, since: u64) -> Value {
        let events: Vec<Value> = self
            .log
            .iter()
            .filter(|(n, _)| *n > since)
            .map(|(n, t)| json!({"n": n, "text": t}))
            .collect();
        json!({ "next": self.seq, "events": events })
    }

    fn qsy(&mut self, slot: usize) -> Result<u64, String> {
        let spec = self.args.rig.as_deref().ok_or("no rig configured")?;
        let parsed = kahf_rig::parse_spec(spec).map_err(|_| "this rig setting cannot be tuned by KAHF")?;
        if parsed.model != "flrig" {
            return Err("the engine holds the serial port while running; restart with --slot instead".into());
        }
        let mut rig = kahf_rig::open(spec)?;
        kahf_rig::qsy::qsy_to_relay_slot(rig.as_mut(), slot)
    }
}
