//! Receive-only check of an RTL-SDR: lists devices, reads a few seconds of IQ at one
//! frequency and reports the level. Never transmits.
//!
//! Usage: cargo run -p kahf-sdr --example rtl_level -- [freq_hz] [seconds]
//! Default: 145500000 Hz (2 m band) for 3 seconds.

use kahf_core::io::IqSource;
use kahf_sdr::rtlsdr::{list_devices, RtlsdrRx};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let freq_hz: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(145_500_000);
    let seconds: f64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(3.0);

    let devices = list_devices();
    println!("devices found: {}", devices.len());
    for (i, name) in devices.iter().enumerate() {
        println!("  {i}: {name}");
    }
    if devices.is_empty() {
        println!("no RTL-SDR found; nothing to test");
        return;
    }

    let mut rx = match RtlsdrRx::open(0, freq_hz, 2_400_000, Some(496), 0) {
        Ok(rx) => rx,
        Err(e) => {
            println!("open failed: {e}");
            return;
        }
    };
    let rate = rx.sample_rate();
    println!("tuned {} Hz, rate {:.0} sps, gain 49.6 dB", freq_hz, rate);

    let want = (rate * seconds) as usize;
    let mut buf = vec![(0.0f32, 0.0f32); 16_384];
    let mut total = 0usize;
    let mut power_sum = 0.0f64;
    let mut peak = 0.0f32;
    let mut sum_i = 0.0f64;
    let mut sum_q = 0.0f64;
    while total < want {
        match rx.read_iq(&mut buf) {
            Ok(0) => continue,
            Ok(n) => {
                for &(i, q) in &buf[..n] {
                    let p = (i * i + q * q) as f64;
                    power_sum += p;
                    sum_i += i as f64;
                    sum_q += q as f64;
                    peak = peak.max(p.sqrt() as f32);
                }
                total += n;
            }
            Err(e) => {
                println!("read failed after {total} samples: {e}");
                return;
            }
        }
    }

    let mean_power = power_sum / total as f64;
    let rms_db = 10.0 * mean_power.max(1e-20).log10();
    let peak_db = 20.0 * (peak as f64).max(1e-10).log10();
    println!("samples read: {total} ({:.2} s)", total as f64 / rate);
    println!("DC offset: I {:.4}, Q {:.4}", sum_i / total as f64, sum_q / total as f64);
    println!("mean power: {rms_db:.1} dBFS, peak: {peak_db:.1} dBFS");
}
