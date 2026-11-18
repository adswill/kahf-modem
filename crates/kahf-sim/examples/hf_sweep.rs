//! How each KAMI mode copes with real HF: for every mode and every channel, the SNR (dB, per real
//! sample, as `kahf bler`) at which 10 percent of the frames are lost.
//! `cargo run --release -p kahf-sim --example hf_sweep [frames]`
use kahf_core::phy::Modulation as M;
use kahf_sim::{run_point_mod, Profile};

fn main() {
    let frames: usize = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(60);
    let modes = [
        ("KAMI-256", M::Kami256), ("KAMI-64", M::Kami64), ("KAMI-32", M::Kami32), ("KAMI-16", M::Kami16),
        ("KAMI-8", M::Kami8), ("KAMI-4", M::Kami4), ("KAMI-1a", M::Kami1a), ("KAMI-1b", M::Kami1b),
        ("KAMI-1c", M::Kami1c), ("KAMI-1d", M::Kami1d),
    ];
    let profiles = [
        ("AWGN", Profile::Awgn), ("HF good", Profile::HfGood), ("HF moderate", Profile::HfModerate), ("HF poor", Profile::HfPoor),
    ];
    print!("{:10}", "mode");
    for (n, _) in &profiles {
        print!("{n:>13}");
    }
    println!("   (SNR dB for 10% frame loss; > 30 = never)");
    for (name, m) in modes {
        print!("{name:10}");
        for (_, p) in &profiles {
            let mut found = None;
            let mut snr = -16.0f32;
            while snr <= 30.0 {
                let (err, n) = run_point_mod(m, *p, snr, frames, 7);
                if err * 10 <= n {
                    found = Some(snr);
                    break;
                }
                snr += 2.0;
            }
            match found {
                Some(s) => print!("{s:>13.0}"),
                None => print!("{:>13}", ">30"),
            }
        }
        println!();
    }
}
