//! Frame error rate for each modulation over AWGN, to measure the modulation ladder.
//! Run: cargo run --release -p kahf-sim --example mod_fer -- [frames]
use kahf_core::frame;
use kahf_core::ldpc;
use kahf_core::phy::{self, Modulation};

fn main() {
    let frames: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(200);
    let mods = [
        ("QPSK", Modulation::Qpsk),
        ("16QAM", Modulation::Qam16),
        ("64QAM", Modulation::Qam64),
    ];
    print!("snr_db");
    for (name, _) in mods {
        print!("\t{name}");
    }
    println!();
    for snr in (0..=30).step_by(2) {
        print!("{snr}");
        for (_, m) in mods {
            let mut bad = 0;
            for f in 0..frames {
                let data: Vec<u8> = (0..36).map(|i| ((f * 37 + i * 11) % 251) as u8).collect();
                let info = frame::build_payload(&data);
                let coded = ldpc::encode(&info);
                let tx = phy::modulate_frame(&coded, m);
                let rx = kahf_sim::awgn(&tx, snr as f32, 1000 + f as u64);
                let ok = phy::demodulate_frame(&rx, m, ldpc::N)
                    .and_then(|llr| ldpc::decode(&llr, 50))
                    .and_then(|bits| frame::parse_payload(&bits))
                    .map(|out| out == data)
                    .unwrap_or(false);
                if !ok {
                    bad += 1;
                }
            }
            print!("\t{:.4}", bad as f64 / frames as f64);
        }
        println!();
    }
}
