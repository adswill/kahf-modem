//! Checks the receiver's SNR estimate against the true SNR for each mode.
//! Run: cargo run --release -p kahf-sim --example kami_snr_est
use kahf_core::kami::{demodulate_burst_rx, find_preamble, modulate_burst_shape, Shape};
use kahf_core::{frame, ldpc};

fn main() {
    let modes: [(&str, Shape, &[f32]); 8] = [
        ("KAMI-256", Shape::walsh(8), &[-18.0, -14.0, -10.0]),
        ("KAMI-64", Shape::walsh(6), &[-14.0, -10.0, -6.0]),
        ("KAMI-32", Shape::walsh(5), &[-10.0, -6.0, -2.0, 2.0]),
        ("KAMI-8", Shape::walsh(3), &[-6.0, -2.0, 2.0, 6.0]),
        ("KAMI-1a", Shape { wb: 0, lb: 1 }, &[0.0, 4.0, 8.0, 12.0]),
        ("KAMI-1b", Shape { wb: 0, lb: 2 }, &[6.0, 10.0, 14.0, 20.0]),
        ("KAMI-1c", Shape { wb: 0, lb: 3 }, &[10.0, 14.0, 20.0, 26.0]),
        ("KAMI-1d", Shape { wb: 0, lb: 4 }, &[14.0, 20.0, 26.0, 32.0]),
    ];
    for (name, sh, snrs) in modes {
        for &snr in snrs {
            let cws: Vec<Vec<u8>> = (0..2)
                .map(|j| ldpc::encode(&frame::build_payload_as(&[j as u8; 20], 3)))
                .collect();
            let tx = modulate_burst_shape(&cws, sh);
            let (mut sum, mut n) = (0.0, 0);
            for seed in 0..12u64 {
                let mut x = Vec::new();
                x.extend(&tx);

                let mut rx = kahf_sim::awgn(&x, snr, 100 + seed);
                rx.extend(vec![0f32; 600]);
                if let Some((start, hz, tau)) = find_preamble(&rx) {
                    if let Some(b) = demodulate_burst_rx(&rx, start, hz, tau, ldpc::N, 2, sh) {
                        sum += b.snr_db;
                        n += 1;
                    }
                }
            }
            println!(
                "{name:9} true {snr:6.1} dB   estimated {:6.1} dB  ({n}/12 found)",
                sum / n.max(1) as f64
            );
        }
    }
}
