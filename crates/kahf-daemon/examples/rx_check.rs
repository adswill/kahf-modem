//! Receive-only check of the daemon's SDR receive path through kahf-radio.
//! Opens the named radio, listens for a few seconds and prints the log and spectrum peak.
//! Never transmits.
//!
//! Usage: cargo run -p kahf-daemon --example rx_check -- [radio@freq_hz] [seconds]
//! Default: rtlsdr@145500000 for 4 seconds.

use kahf_daemon::{Config, Engine, Event};
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let sdr = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "rtlsdr@145500000".to_string());
    let seconds: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(4);
    let dir = std::env::temp_dir().join("kahf-rx-check");
    let config = Config {
        callsign: "AA1AAA".into(),
        data_dir: dir,
        sdr: Some(sdr.clone()),
        ..Default::default()
    };
    let mut engine = match Engine::new(config) {
        Ok(e) => e,
        Err(e) => {
            println!("start failed: {e}");
            return;
        }
    };
    println!("engine started with {sdr}");
    if let Err(e) = engine.listen() {
        println!("listen failed: {e}");
        return;
    }
    let until = Instant::now() + Duration::from_secs(seconds);
    let mut spectra = 0usize;
    let mut peak_db = f32::MIN;
    let mut peak_bin = 0usize;
    while Instant::now() < until {
        for ev in engine.poll_events() {
            match ev {
                Event::Log(msg) => println!("log: {msg}"),
                Event::Spectrum(bins) => {
                    spectra += 1;
                    for (i, &b) in bins.iter().enumerate().skip(4) {
                        if b > peak_db {
                            peak_db = b;
                            peak_bin = i;
                        }
                    }
                }
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    println!("spectrum frames: {spectra}");
    if spectra > 0 {
        println!(
            "audio peak: {peak_db:.1} dB at {} Hz",
            peak_bin as f32 * 8000.0 / 2048.0
        );
    }
}
