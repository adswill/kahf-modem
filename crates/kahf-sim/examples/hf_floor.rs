//! Frame loss at fixed SNRs on the HF channels, for the slow modes (is it a floor or a wall?).
use kahf_core::phy::Modulation as M;
use kahf_sim::{run_point_mod, Profile};
fn main() {
    for (cn, p) in [("HF good", Profile::HfGood), ("HF moderate", Profile::HfModerate)] {
        for (name, m) in [
            ("KAMI-256", M::Kami256),
            ("KAMI-32", M::Kami32),
            ("KAMI-16", M::Kami16),
            ("KAMI-8", M::Kami8),
            ("KAMI-4", M::Kami4),
        ] {
            let mut row = format!("{cn:12}{name:9}");
            for snr in [0.0f32, 10.0, 20.0, 30.0] {
                let (e, n) = run_point_mod(m, p, snr, 100, 21);
                row.push_str(&format!(" {snr:>4.0}dB:{:>3.0}%", 100.0 * e as f64 / n as f64));
            }
            println!("{row}");
        }
    }
}
