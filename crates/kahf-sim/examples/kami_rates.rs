//! Net information rate of every KAMI mode for bursts of 1, 4, 8 and 16 codewords (one
//! preamble per burst). Run: cargo run --release -p kahf-sim --example kami_rates
use kahf_core::kami::{burst_len_shape, Shape};

fn main() {
    let modes: [(&str, Shape); 13] = [
        ("KAMI-256", Shape::walsh(8)),
        ("KAMI-128", Shape::walsh(7)),
        ("KAMI-64", Shape::walsh(6)),
        ("KAMI-32", Shape::walsh(5)),
        ("KAMI-16", Shape::walsh(4)),
        ("KAMI-8", Shape::walsh(3)),
        ("KAMI-4", Shape::walsh(2)),
        ("KAMI-2", Shape::walsh(1)),
        ("KAMI-8H", Shape { wb: 3, lb: 2 }),
        ("KAMI-1a", Shape { wb: 0, lb: 1 }),
        ("KAMI-1b", Shape { wb: 0, lb: 2 }),
        ("KAMI-1c", Shape { wb: 0, lb: 3 }),
        ("KAMI-1d", Shape { wb: 0, lb: 4 }),
    ];
    println!("mode      raw bit/s   net info bit/s for 1, 4, 8, 16 codewords per burst");
    for (name, sh) in modes {
        let net = |m: usize| (m * 324) as f64 / (burst_len_shape(648, m, sh) as f64 / 8000.0);
        println!(
            "{name:9} {:9.0}   {:6.0} {:6.0} {:6.0} {:6.0}",
            sh.raw_bps(),
            net(1),
            net(4),
            net(8),
            net(16)
        );
    }
}
