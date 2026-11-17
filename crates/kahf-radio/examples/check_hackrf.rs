//! Receive-only check of an attached HackRF: open, tune 145.500 MHz at 10 Msps, read 3 seconds,
//! print the mean level. Nothing is transmitted.
//!
//! Run with: cargo run -p kahf-radio --example check_hackrf

use std::time::{Duration, Instant};

use kahf_radio::{open, parse_spec};

const FREQ_HZ: u64 = 145_500_000;
const RATE_HZ: u32 = 10_000_000;
const RUN: Duration = Duration::from_secs(3);

fn main() {
    if let Err(e) = run() {
        eprintln!("check_hackrf: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let spec = parse_spec("hackrf")?;
    let mut radio = open(&spec)?;
    println!("radio: {}", radio.label());

    let rate = radio.set_sample_rate(RATE_HZ)?;
    radio.set_frequency_hz(FREQ_HZ)?;
    println!("tuned {} MHz at {} Msps", FREQ_HZ as f64 / 1e6, rate as f64 / 1e6);

    radio.start()?;
    let started = Instant::now();
    let mut buf = vec![(0.0f32, 0.0f32); 16_384];
    let mut power_sum = 0.0f64;
    let mut count = 0u64;
    while started.elapsed() < RUN {
        let n = radio.read_iq(&mut buf)?;
        for &(i, q) in &buf[..n] {
            power_sum += f64::from(i) * f64::from(i) + f64::from(q) * f64::from(q);
        }
        count += n as u64;
    }
    let dropped = radio.dropped_samples();
    radio.stop();

    if count == 0 {
        return Err("no samples arrived in 3 seconds".into());
    }
    let mean_power = power_sum / count as f64;
    let level_db = 10.0 * mean_power.max(1e-30).log10();
    println!("samples: {count}, dropped: {dropped}");
    println!("mean level: {level_db:.1} dB (full scale = 0 dB)");
    Ok(())
}
