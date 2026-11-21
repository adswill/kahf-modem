//! How long `burst::scan` takes and whether it decodes, per sent mode and per preferred mode.
//! A clean path with an echo of the same burst 0.4 s later (a shared sound device), light noise.
//! Run: cargo run --release -p kahf-sim --example scan_cost
use kahf_core::burst::{self, Scan, LADDER};
use kahf_sim::{apply, Profile};
use std::time::Instant;

fn main() {
    println!("sent,prefer,ok,ms");
    for (si, sent) in LADDER.iter().enumerate() {
        let frames: Vec<Vec<u8>> = (0..burst::long_codewords(sent).min(3))
            .map(|k| (0..30).map(|i| (i * 7 + k * 31) as u8).collect())
            .collect();
        let tx = burst::modulate_frames(&frames, sent, burst::FrameKind::Link);
        let mut s = vec![0f32; 8000];
        s.extend_from_slice(&tx);
        s.extend(std::iter::repeat_n(0f32, 3200));
        s.extend_from_slice(&tx); // the echo
        s.extend(std::iter::repeat_n(0f32, 4000));
        let rx = apply(&s, Profile::Awgn, 25.0, 5);
        // The audio as it stands when the first burst has just ended.
        let upto = (8000 + tx.len() + 1200).min(rx.len());
        for (prefer, pm) in LADDER.iter().enumerate() {
            let t = Instant::now();
            let r = burst::scan(&rx[..upto], prefer);
            let ms = t.elapsed().as_millis();
            let ok = matches!(&r, Scan::Heard(h) if h.mode == si && h.frames.len() == frames.len());
            println!("{},{},{},{}", sent.name, pm.name, ok, ms);
        }
    }
}
