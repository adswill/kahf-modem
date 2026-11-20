//! Five stations on one real audio device (BlackHole 2ch), as a small mesh: A sends to D by
//! way of the relays R1 and R2 (set as relay paths, so every message takes two hops even though
//! all of them could hear each other), and E only listens. Reports delivery and the time each
//! message took end to end.
//! Usage: cargo run --release -p kahf-daemon --example mesh -- [messages] ["BlackHole 2ch"]
use std::time::{Duration, Instant};

use kahf_core::relay::{Direction, Radio, Role, Slot};
use kahf_daemon::{Config, Engine, Event, RelayAck};

fn station(call: &str, dev: &str) -> Engine {
    let dir = std::env::temp_dir().join(format!("kahf-mesh-{call}"));
    let _ = std::fs::remove_dir_all(&dir);
    let mut e = Engine::new(Config {
        callsign: call.into(),
        audio_in: Some(dev.into()),
        audio_out: Some(dev.into()),
        data_dir: dir,
        ..Config::default()
    })
    .expect("engine");
    e.set_listen_only(false);
    e.listen().expect("listen");
    e
}

fn relay(e: &Engine) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    e.configure_relay(Role::Relay, Radio::HalfDuplex, &[18_113.0])
        .unwrap();
    e.accept_relay_schedule(vec![Slot {
        channel_khz: 18_113.0,
        start_unix: now - 10,
        duration_s: 6 * 3600,
        direction: Direction::Transmit,
    }])
    .unwrap();
    e.start_relay(RelayAck {
        licence_held: true,
        terms_accepted: true,
    })
    .unwrap();
}

fn main() {
    let n: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(5);
    let dev = std::env::args().nth(2).unwrap_or_else(|| "BlackHole 2ch".into());
    let mut a = station("ZZ1ALPHA", &dev);
    let mut r1 = station("ZZ2RELAY", &dev);
    let mut r2 = station("ZZ3RELAY", &dev);
    let mut d = station("ZZ4DELTA", &dev);
    let mut e = station("ZZ5ECHO", &dev);
    relay(&r1);
    relay(&r2);
    r1.set_relay_paths(&["ZZ3RELAY"]).unwrap();
    a.set_relay_paths(&["ZZ2RELAY"]).unwrap();
    // How long A tries D directly before taking the relay path.
    a.set_relay_timeout_ms(8_000);
    std::thread::sleep(Duration::from_secs(1));

    let mut delivered = Vec::new();
    for i in 1..=n {
        let text = format!("mesh message {i}");
        let t0 = Instant::now();
        a.send_message("ZZ4DELTA", &text).unwrap();
        // A first tries D directly; when D cannot be reached the engine moves the message to the
        // relay path, and only then is the relay called.
        let mut routed = false;
        while !routed && t0.elapsed() < Duration::from_secs(120) {
            for ev in a.poll_events() {
                if let Event::Log(s) = &ev {
                    if s.contains("radio relay through") {
                        routed = true;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        println!(
            "{:6.1}s A: {}",
            t0.elapsed().as_secs_f32(),
            if routed {
                "message moved to the relay path"
            } else {
                "NO relay path chosen"
            }
        );
        a.connect("ZZ2RELAY").ok();
        let mut got = None;
        let mut a_idle = false;
        let mut held = false;
        while t0.elapsed() < Duration::from_secs(300) && got.is_none() {
            for ev in d.poll_events() {
                if let Event::Message { from, text: t } = &ev {
                    println!("{:6.1}s D got {t:?} from {from}", t0.elapsed().as_secs_f32());
                    if *t == text {
                        got = Some(t0.elapsed());
                    }
                }
            }
            for (n, eng) in [("A", &mut a), ("R1", &mut r1), ("R2", &mut r2), ("E", &mut e)] {
                for ev in eng.poll_events() {
                    match ev {
                        Event::Spectrum(_) | Event::Tx { .. } => {}
                        Event::Log(s) if n == "R1" && s.starts_with("Holding a message") => held = true,
                        Event::Relay { from, to, step } => println!(
                            "{:6.1}s {n}: relay {from}->{to} {step:?}",
                            t0.elapsed().as_secs_f32()
                        ),
                        Event::Link(l) => println!("{:6.1}s {n}: link {l:?}", t0.elapsed().as_secs_f32()),
                        Event::Message { text, from } => println!(
                            "{:6.1}s {n} (bystander?) heard message {text:?} from {from}",
                            t0.elapsed().as_secs_f32()
                        ),
                        _ => {}
                    }
                }
            }
            if !a_idle && held {
                a_idle = true;
                a.disconnect().ok();
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        match got {
            Some(t) => {
                println!("message {i}: delivered in {:.1} s", t.as_secs_f32());
                delivered.push(t);
            }
            None => println!("message {i}: NOT delivered in 300 s"),
        }
        std::thread::sleep(Duration::from_secs(5));
    }
    let avg = if delivered.is_empty() {
        0.0
    } else {
        delivered.iter().map(|t| t.as_secs_f32()).sum::<f32>() / delivered.len() as f32
    };
    println!("\ndelivered {}/{n}, average {avg:.1} s", delivered.len());
}
