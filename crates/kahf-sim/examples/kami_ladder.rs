//! Frame loss of each modulation against SNR on AWGN, and the lowest SNR with under 5 percent
//! frame loss. Run: cargo run --release -p kahf-sim --example kami_ladder -- [frames] [mode ...]
use kahf_core::phy::Modulation;
use kahf_sim::{run_point_mod, Profile};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let frames: usize = args.first().and_then(|s| s.parse().ok()).unwrap_or(100);
    let only: Vec<&str> = args.iter().skip(1).map(|s| s.as_str()).collect();
    let modes = [
        ("KAMI-256", Modulation::Kami256),
        ("KAMI-128", Modulation::Kami128),
        ("KAMI-64", Modulation::Kami64),
        ("KAMI-32", Modulation::Kami32),
        ("KAMI-16", Modulation::Kami16),
        ("KAMI-8", Modulation::Kami8),
        ("KAMI-4", Modulation::Kami4),
        ("KAMI-2", Modulation::Kami2),
        ("KAMI-8H", Modulation::Kami8H),
        ("KAMI-1a", Modulation::Kami1a),
        ("KAMI-1b", Modulation::Kami1b),
        ("KAMI-1c", Modulation::Kami1c),
        ("KAMI-1d", Modulation::Kami1d),
        ("QPSK", Modulation::Qpsk),
        ("16QAM", Modulation::Qam16),
        ("64QAM", Modulation::Qam64),
    ];
    for (name, m) in modes {
        if !only.is_empty() && !only.contains(&name) {
            continue;
        }
        let mut snr = -16.0f32;
        let (mut prev, mut zeros) = (None::<(f32, f64)>, 0);
        let mut cross = None;
        while snr <= 26.0 && zeros < 2 {
            let (e, n) = run_point_mod(m, Profile::Awgn, snr, frames, 1);
            let loss = e as f64 / n as f64;
            println!("{name:9} snr {snr:5.1}  frame loss {loss:.3} ({e}/{n})");
            if loss < 0.05 && cross.is_none() {
                cross = Some(match prev {
                    Some((ps, pl)) if pl > loss => ps + (snr - ps) * ((pl - 0.05) / (pl - loss)) as f32,
                    _ => snr,
                });
            }
            zeros = if loss == 0.0 { zeros + 1 } else { 0 };
            prev = Some((snr, loss));
            // Coarse steps far from the threshold: 2 dB until loss appears, then 1 dB.
            snr += if loss > 0.999 { 2.0 } else { 1.0 };
        }
        match cross {
            Some(c) => println!("{name:9} RESULT under 5 percent frame loss from {c:.1} dB"),
            None => println!("{name:9} RESULT never under 5 percent up to 26 dB"),
        }
    }
}
