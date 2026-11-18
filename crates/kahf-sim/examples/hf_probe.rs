//! Which part of an HF channel breaks a mode? Frame loss at 30 dB SNR for flat fades and
//! echoes of different strength, so the cause is visible. `cargo run --release -p kahf-sim --example hf_probe`
use kahf_core::phy::Modulation as M;
use kahf_sim::{hf_channel, run_point_with};

fn main() {
    let frames = 40;
    println!("{:34} {:>9} {:>9}", "channel at 30 dB SNR", "KAMI-32", "KAMI-1a");
    for (name, delay, spread) in [
        ("flat fade, 0.01 Hz (near static)", 0.0, 0.01),
        ("flat fade, 0.1 Hz", 0.0, 0.1),
        ("flat fade, 1 Hz", 0.0, 1.0),
        ("echo 0.5 ms, 0.01 Hz", 0.0005, 0.01),
        ("echo 1 ms, 0.01 Hz", 0.001, 0.01),
        ("echo 2 ms, 0.01 Hz", 0.002, 0.01),
    ] {
        let mut row = format!("{name:34}");
        for m in [M::Kami32, M::Kami1a] {
            let (e, n) = run_point_with(m, frames, 11, |tx, s| hf_channel(tx, delay, spread, 30.0, s));
            row.push_str(&format!(" {:>8.0}%", 100.0 * e as f64 / n as f64));
        }
        println!("{row}");
    }
}
