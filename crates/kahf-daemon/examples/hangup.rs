//! A calls a relay over BlackHole, waits, hangs up; prints when each side sees what.
//! Usage: cargo run --release -p kahf-daemon --example hangup -- [relay-mode: 1|0]
use std::time::{Duration, Instant};

use kahf_core::relay::{Direction, Radio, Role, Slot};
use kahf_daemon::{Config, Engine, Event, RelayAck};

fn station(call: &str) -> Engine {
    let dir = std::env::temp_dir().join(format!("kahf-hangup-{call}"));
    let _ = std::fs::remove_dir_all(&dir);
    let mut e = Engine::new(Config {
        callsign: call.into(),
        audio_in: Some("BlackHole 2ch".into()),
        audio_out: Some("BlackHole 2ch".into()),
        data_dir: dir,
        ..Config::default()
    })
    .unwrap();
    e.set_listen_only(false);
    e.listen().unwrap();
    e
}

fn main() {
    let relay_mode = std::env::args().nth(1).as_deref() != Some("0");
    let mut a = station("ZZ7GOLF");
    let mut r = station("ZZ8HOTEL");
    if relay_mode {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        r.configure_relay(Role::Relay, Radio::HalfDuplex, &[18_113.0])
            .unwrap();
        r.accept_relay_schedule(vec![Slot {
            channel_khz: 18_113.0,
            start_unix: now - 10,
            duration_s: 3600,
            direction: Direction::Transmit,
        }])
        .unwrap();
        r.start_relay(RelayAck {
            licence_held: true,
            terms_accepted: true,
        })
        .unwrap();
    }
    std::thread::sleep(Duration::from_secs(1));
    a.connect("ZZ8HOTEL").unwrap();
    let t0 = Instant::now();
    let mut hung = false;
    while t0.elapsed() < Duration::from_secs(40) {
        for (n, e) in [("A", &mut a), ("R", &mut r)] {
            for ev in e.poll_events() {
                match ev {
                    Event::Spectrum(_) => {}
                    other => println!("{:5.1}s {n}: {other:?}", t0.elapsed().as_secs_f32()),
                }
            }
        }
        if !hung && t0.elapsed() > Duration::from_secs(8) {
            hung = true;
            println!("{:5.1}s A hangs up", t0.elapsed().as_secs_f32());
            a.disconnect().unwrap();
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}
