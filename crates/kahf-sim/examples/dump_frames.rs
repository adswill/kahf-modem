//! Writes one random frame of every modulation as raw f32 to the folder given as the first
//! argument, for spectrum measurement. Run: cargo run --release -p kahf-sim --example dump_frames -- /tmp/frames
use kahf_core::phy::{modulate_frame, Modulation};

fn main() {
    let dir = std::env::args().nth(1).expect("folder");
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = 12345u64;
    let mut bits = Vec::new();
    for _ in 0..648 {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        bits.push(((s >> 33) & 1) as u8);
    }
    for (name, m) in [
        ("kami256", Modulation::Kami256),
        ("kami128", Modulation::Kami128),
        ("kami64", Modulation::Kami64),
        ("kami32", Modulation::Kami32),
        ("kami16", Modulation::Kami16),
        ("kami8", Modulation::Kami8),
        ("kami8h", Modulation::Kami8H),
        ("kami4", Modulation::Kami4),
        ("kami2", Modulation::Kami2),
        ("qpsk", Modulation::Qpsk),
        ("qam16", Modulation::Qam16),
        ("qam64", Modulation::Qam64),
    ] {
        let x = modulate_frame(&bits, m);
        let bytes: Vec<u8> = x.iter().flat_map(|v| v.to_le_bytes()).collect();
        std::fs::write(format!("{dir}/{name}.f32"), bytes).unwrap();
    }
}
