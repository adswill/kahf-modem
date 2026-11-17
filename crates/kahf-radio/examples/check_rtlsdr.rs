//! Receive-only check of the RTL-SDR driver: open the first radio, tune 145.500 MHz, read 3 seconds, print the mean level.
//! Nothing is transmitted. Run with: cargo run -p kahf-radio --example check_rtlsdr

use std::process::ExitCode;
use std::time::{Duration, Instant};

use kahf_radio::drivers::rtlsdr;
use kahf_radio::GainSetting;

const FREQ_HZ: u64 = 145_500_000;
const RATE_HZ: u32 = 2_048_000;
const READ_FOR: Duration = Duration::from_secs(3);

fn main() -> ExitCode {
    let mut radio = match rtlsdr::open(None) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("open failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("radio: {}", radio.label());
    if let Err(e) = radio.set_frequency_hz(FREQ_HZ) {
        eprintln!("tune failed: {e}");
        return ExitCode::FAILURE;
    }
    let rate = match radio.set_sample_rate(RATE_HZ) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("sample rate failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = radio.set_gain("tuner", GainSetting::Manual(30.0)) {
        eprintln!("gain failed: {e}");
        return ExitCode::FAILURE;
    }
    if let Err(e) = radio.start() {
        eprintln!("start failed: {e}");
        return ExitCode::FAILURE;
    }

    let mut buf = vec![(0.0f32, 0.0f32); 16_384];
    let mut power_sum = 0.0f64;
    let mut count = 0u64;
    let started = Instant::now();
    while started.elapsed() < READ_FOR {
        match radio.read_iq(&mut buf) {
            Ok(n) => {
                for &(i, q) in &buf[..n] {
                    power_sum += (i * i + q * q) as f64;
                }
                count += n as u64;
            }
            Err(e) => {
                eprintln!("read failed: {e}");
                radio.stop();
                return ExitCode::FAILURE;
            }
        }
    }
    radio.stop();

    if count == 0 {
        eprintln!(
            "no samples arrived in 3 seconds. Check the antenna and that no other program holds the radio."
        );
        return ExitCode::FAILURE;
    }
    let mean_db = 10.0 * (power_sum / count as f64).max(1e-12).log10();
    println!("sample rate: {rate} Hz, tuned: {} Hz", FREQ_HZ);
    println!("samples: {count}, dropped: {}", radio.dropped_samples());
    println!("mean level at 145.500 MHz: {mean_db:.1} dB (full scale = 0 dB)");
    ExitCode::SUCCESS
}
