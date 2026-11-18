//! Frame loss of each modulation on AWGN. Run: cargo run --release -p kahf-sim --example kami_ladder -- [frames]
use kahf_core::phy::Modulation;
use kahf_sim::{run_point_mod, Profile};

fn main() {
    let frames: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(100);
    for (name, m, snrs) in [
        ("KAMI-32", Modulation::Kami32, vec![-6.0f32, -5.0, -4.0, -2.0, 0.0]),
        ("KAMI-16", Modulation::Kami16, vec![-5.0, -4.0, -3.0, -2.0, -1.0, 0.0]),
        ("KAMI-8", Modulation::Kami8, vec![-4.0, -3.0, -2.0, -1.0, 0.0, 1.0, 2.0]),
        ("QPSK", Modulation::Qpsk, vec![-2.0, -1.0, 0.0, 1.0, 2.0, 3.0, 4.0]),
    ] {
        for snr in snrs {
            let (e, n) = run_point_mod(m, Profile::Awgn, snr, frames, 1);
            println!("{name:8} snr {snr:5.1}  frame loss {:.3} ({e}/{n})", e as f64 / n as f64);
        }
    }
}
