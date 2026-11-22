//! Adjacent-lane leakage of the narrow lane mode (MESH-SPEC.md step 4).
//!
//! The victim lane is at 1500 Hz audio. Both neighbours carry a signal 30 dB stronger, at the
//! lane pitch given. For each mode: the SNR (total power in the 8 kHz audio) needed for 10
//! percent frame loss, alone and with the two loud neighbours. The difference is the price of
//! the neighbours. Run: `cargo run --release -p kahf-sim --example lane_leak`

use kahf_core::kami::{self, Shape};
use kahf_core::{ldpc, narrow};

struct Rng(u64);
impl Rng {
    fn u(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
    fn gauss(&mut self) -> f32 {
        let (a, b) = (self.u(), self.u());
        ((-2.0 * a.ln()).sqrt() * (2.0 * std::f64::consts::PI * b).cos()) as f32
    }
}

fn info(seed: u64) -> Vec<u8> {
    let mut r = Rng(seed * 7919 + 1);
    (0..ldpc::K).map(|_| (r.u() > 0.5) as u8).collect()
}

fn rms(x: &[f32]) -> f64 {
    let nz: Vec<&f32> = x.iter().filter(|v| v.abs() > 1e-6).collect();
    (nz.iter().map(|v| (**v as f64).powi(2)).sum::<f64>() / nz.len().max(1) as f64).sqrt()
}

/// One trial: true when the victim frame decodes to what was sent.
fn trial(sh: Shape, pitch: f64, snr_db: f64, neighbours_db: Option<f64>, seed: u64) -> bool {
    let mut rng = Rng(seed);
    let sent = info(seed);
    let victim = narrow::to_lane(&kami::modulate_frame_shape(&ldpc::encode(&sent), sh), 1500.0);
    let level = rms(&victim);
    let n = victim.len() + 8000;
    let mut audio = vec![0f32; n];
    for (i, v) in victim.iter().enumerate() {
        audio[4000 + i] += v;
    }
    if let Some(db) = neighbours_db {
        for centre in [1500.0 - pitch, 1500.0 + pitch] {
            let mut at = (rng.u() * 3000.0) as usize;
            let mut k = 0;
            while at < n {
                let f = kami::modulate_frame_shape(&ldpc::encode(&info(seed * 31 + k + 100)), sh);
                let lane = narrow::to_lane(&f, centre);
                let g = level * 10f64.powf(db / 20.0) / rms(&lane);
                for (i, v) in lane.iter().enumerate() {
                    if at + i < n {
                        audio[at + i] += *v * g as f32;
                    }
                }
                at += lane.len() + 200;
                k += 1;
            }
        }
    }
    let sigma = (level * 10f64.powf(-snr_db / 20.0)) as f32;
    for v in audio.iter_mut() {
        *v += sigma * rng.gauss();
    }
    let rx = narrow::from_lane(&audio, 1500.0);
    let Some((start, hz, tau)) = kami::find_preamble(&rx) else {
        return false;
    };
    let Some((_, llr)) = kami::demodulate_at(&rx, start, hz, tau, ldpc::N, &[sh])
        .into_iter()
        .next()
    else {
        return false;
    };
    ldpc::decode(&llr, 50).as_deref() == Some(sent.as_slice())
}

fn loss(sh: Shape, pitch: f64, snr: f64, nb: Option<f64>, trials: u64) -> f64 {
    let bad = (0..trials).filter(|t| !trial(sh, pitch, snr, nb, *t + 1)).count();
    bad as f64 / trials as f64
}

/// Lowest SNR (1 dB steps) with at most 10 percent loss.
fn need(sh: Shape, pitch: f64, nb: Option<f64>, trials: u64) -> Option<f64> {
    (-20..=30)
        .map(|s| s as f64)
        .find(|s| loss(sh, pitch, *s, nb, trials) <= 0.10)
}

fn main() {
    let trials = 30;
    let modes: [(&str, Shape); 3] = [
        ("KAMI-1c", Shape { wb: 0, lb: 3 }),
        ("KAMI-1a", Shape { wb: 0, lb: 1 }),
        ("KAMI-8", Shape::walsh(3)),
    ];
    println!("SNR (dB, whole 8 kHz audio) for 10 percent frame loss, {trials} frames per point");
    println!(
        "{:<8} {:>6} {:>10} {:>16} {:>8}",
        "mode", "pitch", "alone", "neighbours +30", "price"
    );
    for pitch in [700.0, 800.0] {
        for (name, sh) in modes {
            let alone = need(sh, pitch, None, trials);
            let near = need(sh, pitch, Some(30.0), trials);
            let price = match (alone, near) {
                (Some(a), Some(b)) => format!("{:+.0} dB", b - a),
                _ => "n/a".into(),
            };
            println!(
                "{name:<8} {pitch:>6.0} {:>10} {:>16} {:>8}",
                fmt(alone),
                fmt(near),
                price
            );
        }
    }
}

fn fmt(v: Option<f64>) -> String {
    v.map_or(">30".into(), |v| format!("{v:.0}"))
}
