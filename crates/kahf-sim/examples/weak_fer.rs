//! Weak-signal mode check: one codeword repeated k times inside one frame behind the long
//! preamble. SNR is in a 2500 Hz bandwidth. Run: cargo run --release -p kahf-sim --example weak_fer -- [frames] [channel]
use kahf_core::phy::{self, Modulation};
use kahf_core::{frame, ldpc};
use kahf_sim::{apply, Profile};
use rayon::prelude::*;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let frames: usize = a.first().and_then(|s| s.parse().ok()).unwrap_or(40);
    let prof = match a.get(1).map(|s| s.as_str()) {
        Some("good") => Profile::WattersonGood,
        Some("moderate") => Profile::WattersonModerate,
        Some("poor") => Profile::WattersonPoor,
        _ => Profile::Awgn,
    };
    println!("snr_2500\trep16\trep32\trep64\trep128");
    for snr10 in (-210..=-90).step_by(30) {
        let snr = snr10 as f32 / 10.0;
        print!("{snr}");
        for k in [16usize, 32, 64, 128] {
            let bad = (0..frames)
                .into_par_iter()
                .filter(|&f| {
                    let data: Vec<u8> = (0..36).map(|i| ((f * 37 + i * 11) % 251) as u8).collect();
                    let coded = ldpc::encode(&frame::build_payload(&data));
                    let rep: Vec<u8> = (0..k).flat_map(|_| coded.iter().copied()).collect();
                    let tx = phy::modulate_weak(&rep, Modulation::Qpsk);
                    let mut padded = vec![0f32; 4000];
                    padded.extend_from_slice(&tx);
                    padded.extend(std::iter::repeat_n(0f32, 4000));
                    let comp = 10.0 * (padded.len() as f32 / tx.len() as f32).log10();
                    let rx = apply(&padded, prof, snr - 2.04 - comp, 5000 + f as u64);
                    let ok = phy::demodulate_weak(&rx, Modulation::Qpsk, rep.len())
                        .map(|l| {
                            let mut sum = vec![0f32; ldpc::N];
                            for c in l.chunks(ldpc::N) {
                                for (s, v) in sum.iter_mut().zip(c) {
                                    *s += v;
                                }
                            }
                            sum
                        })
                        .and_then(|l| ldpc::decode(&l, 50))
                        .and_then(|b| frame::parse_payload(&b))
                        .map(|o| o == data)
                        .unwrap_or(false);
                    !ok
                })
                .count();
            print!("\t{:.2}", bad as f64 / frames as f64);
        }
        println!();
    }
}
