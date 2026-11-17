//! Frame error rate with repetition: the same QPSK frame sent k times with fresh noise,
//! the soft decisions added together before decoding. Measures how far below 0 dB it reaches.
//! Run: cargo run --release -p kahf-sim --example rep_fer -- [frames]
use kahf_core::frame;
use kahf_core::ldpc;
use kahf_core::phy::{self, Modulation};

fn main() {
    let frames: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(100);
    println!("snr_db\trep1\trep2\trep4\trep8");
    for snr10 in (-120..=20).step_by(20) {
        let snr = snr10 as f32 / 10.0;
        print!("{snr}");
        for k in [1usize, 2, 4, 8] {
            let mut bad = 0;
            for f in 0..frames {
                let data: Vec<u8> = (0..36).map(|i| ((f * 37 + i * 11) % 251) as u8).collect();
                let coded = ldpc::encode(&frame::build_payload(&data));
                let tx = phy::modulate_frame(&coded, Modulation::Qpsk);
                let mut sum = vec![0f32; ldpc::N];
                for r in 0..k {
                    let rx = kahf_sim::awgn(&tx, snr, 7000 + (f * 16 + r) as u64);
                    if let Some(llr) = phy::demodulate_frame(&rx, Modulation::Qpsk, ldpc::N) {
                        for (s, l) in sum.iter_mut().zip(llr.iter()) {
                            *s += l;
                        }
                    }
                }
                let ok = ldpc::decode(&sum, 50)
                    .and_then(|bits| frame::parse_payload(&bits))
                    .map(|out| out == data)
                    .unwrap_or(false);
                if !ok {
                    bad += 1;
                }
            }
            print!("\t{:.3}", bad as f64 / frames as f64);
        }
        println!();
    }
}
