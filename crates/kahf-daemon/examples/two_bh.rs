//! Two engines on one BlackHole device: A sends a chat to B. Prints every event.
use kahf_daemon::{Config, Engine};
use std::time::{Duration, Instant};

fn cfg(call: &str, dir: &str) -> Config {
    Config {
        callsign: call.into(),
        audio_in: Some("BlackHole 2ch".into()),
        audio_out: Some("BlackHole 2ch".into()),
        rigctld: None,
        sdr: None,
        data_dir: dir.into(),
    }
}

fn main() {
    let _ = std::fs::remove_dir_all("/tmp/two_bh");
    let mut a = Engine::new(cfg("KA1ALF", "/tmp/two_bh/a")).unwrap();
    let mut b = Engine::new(cfg("KB2BRV", "/tmp/two_bh/b")).unwrap();
    a.listen().unwrap();
    b.listen().unwrap();
    a.set_auto_link(true);
    b.set_auto_link(true);
    b.send_message("KA1ALF", "hello over blackhole").unwrap();
    let t = Instant::now();
    let mut advertised = false;
    while t.elapsed() < Duration::from_secs(30) {
        if !advertised && t.elapsed() > Duration::from_secs(12) {
            advertised = true;
            a.advertise().unwrap();
        }
        for (n, e) in [("A", &mut a), ("B", &mut b)] {
            for ev in e.poll_events() {
                if let kahf_daemon::Event::Spectrum(b) = &ev {
                    let m = b.iter().cloned().fold(f32::MIN, f32::max);
                    if m > -60.0 { println!("[{:5.1}s] {n}: spectrum peak {m:.0} dB", t.elapsed().as_secs_f32()); }
                } else {
                    println!("[{:5.1}s] {n}: {ev:?}", t.elapsed().as_secs_f32());
                }
            }
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}
