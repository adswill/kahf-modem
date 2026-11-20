//! Speed-ladder benchmark: per mode, codeword error rate and goodput against SNR (2500 Hz).
//! Run: cargo run --release -p kahf-sim --example ladder_bench -- [bursts] [channel] [from to step]
//! Output is CSV: channel,mode,snr_2500,cw_fer,goodput_bps
use kahf_core::burst::{self, LADDER};
use kahf_sim::{apply, Profile};
use rayon::prelude::*;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let bursts: usize = a.first().and_then(|s| s.parse().ok()).unwrap_or(30);
    let (cn, prof) = match a.get(1).map(|s| s.as_str()) {
        Some("good") => ("good", Profile::WattersonGood),
        Some("moderate") => ("moderate", Profile::WattersonModerate),
        Some("poor") => ("poor", Profile::WattersonPoor),
        _ => ("awgn", Profile::Awgn),
    };
    let from: f32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(-18.0);
    let to: f32 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(30.0);
    let step: f32 = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(2.0);
    println!("channel,mode,snr_2500,cw_fer,goodput_bps");
    let only = a.get(5).cloned();
    for mode in LADDER
        .iter()
        .filter(|m| only.as_deref().is_none_or(|o| o.split(',').any(|x| x == m.name)))
    {
        let n_cw = if mode.reps > 1 { 1 } else { 32 };
        let mut snr = from;
        while snr <= to + 1e-3 {
            let lost: usize = (0..bursts)
                .into_par_iter()
                .map(|f| {
                    let payload: Vec<u8> = (0..n_cw * burst::CW_BYTES)
                        .map(|i| ((f * 131 + i * 17) % 251) as u8)
                        .collect();
                    let (tx, n) = burst::modulate(&payload, mode);
                    let mut s = vec![0f32; 4000];
                    s.extend_from_slice(&tx);
                    s.extend(std::iter::repeat_n(0f32, 4000));
                    let comp = 10.0 * (s.len() as f32 / tx.len() as f32).log10();
                    let rx = apply(&s, prof, snr - 2.04 - comp, 777 + f as u64);
                    match burst::demodulate(&rx, mode, n) {
                        None => n,
                        Some(cws) => cws
                            .iter()
                            .enumerate()
                            .filter(|(c, d)| {
                                d.as_deref() != Some(&payload[c * burst::CW_BYTES..(c + 1) * burst::CW_BYTES])
                            })
                            .count(),
                    }
                })
                .sum();
            let fer = lost as f64 / (bursts * n_cw) as f64;
            println!(
                "{cn},{},{snr:.1},{fer:.3},{:.0}",
                mode.name,
                mode.rate_bps(n_cw) * (1.0 - fer)
            );
            snr += step;
        }
    }
}
