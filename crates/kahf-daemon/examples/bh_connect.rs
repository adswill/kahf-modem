//! Two stations on real audio devices (for example both on BlackHole 2ch), to see whether a
//! connection comes up. Usage: cargo run -p kahf-daemon --example bh_connect -- "BlackHole 2ch"
use std::time::{Duration, Instant};

use kahf_daemon::{Config, Engine, Event};

fn station(call: &str, dev: &str) -> Engine {
    let dir = std::env::temp_dir().join(format!("kahf-bh-connect-{call}"));
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
    let dev = std::env::args().nth(1).unwrap_or_else(|| "BlackHole 2ch".into());
    let mut a = station("ZZ3CHARLIE", &dev);
    let mut b = station("ZZ4DELTA", &dev);
    std::thread::sleep(Duration::from_secs(1));
    println!("A connects to ZZ4DELTA");
    a.connect("ZZ4DELTA").expect("connect");
    let end = Instant::now() + Duration::from_secs(15);
    let t0 = Instant::now();
    while Instant::now() < end {
        for (who, e) in [("A", &mut a), ("B", &mut b)] {
            for ev in e.poll_events() {
                match ev {
                    Event::Spectrum(bins) => {
                        let peak = bins.iter().cloned().fold(f32::MIN, f32::max);
                        if peak > -80.0 {
                            println!(
                                "{:6.1}s {who}: spectrum peak {peak:.1} dB",
                                t0.elapsed().as_secs_f32()
                            );
                        }
                    }
                    other => println!("{:6.1}s {who}: {other:?}", t0.elapsed().as_secs_f32()),
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
