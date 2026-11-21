//! Soak test of two stations on real audio devices (both on BlackHole 2ch): connect, exchange
//! messages both ways for a while, and report overlapping transmissions and lost messages.
//! Usage: cargo run -p kahf-daemon --example soak -- [seconds] ["BlackHole 2ch"]
use std::time::{Duration, Instant};

use kahf_daemon::{Config, Engine, Event, TxState};

fn station(call: &str, dev: &str) -> Engine {
    let dir = std::env::temp_dir().join(format!("kahf-soak-{call}"));
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

fn main() {
    let secs: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(120);
    let dev = std::env::args().nth(2).unwrap_or_else(|| "BlackHole 2ch".into());
    let mut st = [station("ZZ5ECHO", &dev), station("ZZ6FOXTROT", &dev)];
    let names = ["A", "B"];
    std::thread::sleep(Duration::from_secs(1));
    let t0 = Instant::now();
    let now = |t: Instant| t.duration_since(t0).as_secs_f32();
    st[0].connect("ZZ6FOXTROT").expect("connect");
    let mut tx_since: [Option<Instant>; 2] = [None, None];
    let mut overlaps = 0;
    let mut sent = [0u32; 2];
    let mut got = [0u32; 2];
    let mut linked = [false, false];
    let mut next_send = [
        Instant::now() + Duration::from_secs(8),
        Instant::now() + Duration::from_secs(13),
    ];
    let end = t0 + Duration::from_secs(secs);
    while Instant::now() < end {
        for i in 0..2 {
            for ev in st[i].poll_events() {
                match ev {
                    Event::Spectrum(_) => {}
                    Event::Tx { state } => {
                        match &state {
                            TxState::Starting => {
                                tx_since[i] = Some(Instant::now());
                                if tx_since[1 - i].is_some() {
                                    overlaps += 1;
                                    println!(
                                        "{:7.1}s OVERLAP: {} starts while {} is transmitting",
                                        now(Instant::now()),
                                        names[i],
                                        names[1 - i]
                                    );
                                }
                            }
                            TxState::Done | TxState::Failed(_) => tx_since[i] = None,
                            TxState::Sending => {}
                        }
                        println!("{:7.1}s {}: tx {:?}", now(Instant::now()), names[i], state);
                    }
                    Event::Message { text, .. } => {
                        got[i] += 1;
                        println!("{:7.1}s {}: GOT {text:?}", now(Instant::now()), names[i]);
                    }
                    Event::Link(l) => {
                        linked[i] = matches!(l, kahf_core::link::LinkState::Connected { .. });
                        println!("{:7.1}s {}: link {l:?}", now(Instant::now()), names[i]);
                    }
                    Event::Log(m) => println!("{:7.1}s {}: log {m}", now(Instant::now()), names[i]),
                    other => println!("{:7.1}s {}: {other:?}", now(Instant::now()), names[i]),
                }
            }
            if linked[i] && Instant::now() >= next_send[i] {
                let to = if i == 0 { "ZZ6FOXTROT" } else { "ZZ5ECHO" };
                let n = sent[i] + 1;
                if st[i]
                    .send_message(to, &format!("{} message {n}", names[i]))
                    .is_ok()
                {
                    sent[i] = n;
                    println!("{:7.1}s {}: SEND message {n}", now(Instant::now()), names[i]);
                }
                next_send[i] = Instant::now() + Duration::from_secs(20 + 7 * i as u64);
            }
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    println!(
        "\nsent A {} B {} | received at B {} at A {} | overlaps {overlaps}",
        sent[0], sent[1], got[1], got[0]
    );
}
