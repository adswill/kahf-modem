//! Narrow lane mode, first version (MESH-SPEC.md 2.4): the ordinary KAMI waveforms at one fifth
//! of the chip rate, so the signal is 480 Hz wide instead of 2.4 kHz and five times slower.
//!
//! No new modem. A KAMI audio stream is read as if it were sampled at 1600 Hz instead of
//! 8000 Hz: every length and rate scales by five. `to_lane` turns it into 8 kHz audio centred on
//! a chosen audio frequency; `from_lane` takes the lane back out of 8 kHz audio (rejecting
//! the neighbouring lanes) as a stream the ordinary KAMI demodulator reads.
//!
//! Estimated speed (est.): the KAMI-MODES.md net rates divided by five.

use std::f64::consts::PI;

/// Slow-down factor: 2000 chips/s to 400 chips/s.
pub const SLOW: usize = 5;
const FS: f64 = 8000.0;
/// Carrier of a KAMI stream (Hz) and where it sits when the stream is read at 1/5 the rate.
const KAMI_CARRIER: f64 = 1500.0;
const SLOW_CARRIER: f64 = KAMI_CARRIER / SLOW as f64;
/// Edge of the lane passband (Hz): half the signal width (240) plus margin; a neighbour lane at
/// 700 Hz pitch starts 460 Hz from the centre.
const CUTOFF_HZ: f64 = 300.0;

#[derive(Clone, Copy)]
struct C(f64, f64);

/// Windowed-sinc low-pass (Blackman), unit gain at DC.
fn lowpass(cut_hz: f64, fs: f64, taps: usize) -> Vec<f64> {
    let m = (taps - 1) as f64 / 2.0;
    let fc = cut_hz / fs;
    let h: Vec<f64> = (0..taps)
        .map(|i| {
            let x = i as f64 - m;
            let sinc = if x == 0.0 { 2.0 * fc } else { (2.0 * PI * fc * x).sin() / (PI * x) };
            let w = 0.42 - 0.5 * (2.0 * PI * i as f64 / (taps - 1) as f64).cos()
                + 0.08 * (4.0 * PI * i as f64 / (taps - 1) as f64).cos();
            sinc * w
        })
        .collect();
    let sum: f64 = h.iter().sum();
    h.iter().map(|v| v / sum).collect()
}

/// Same-length convolution of a complex stream, centred (no delay).
fn filt(x: &[C], h: &[f64]) -> Vec<C> {
    let half = h.len() / 2;
    (0..x.len())
        .map(|n| {
            let (mut re, mut im) = (0.0, 0.0);
            for (k, hk) in h.iter().enumerate() {
                let i = n as isize + half as isize - k as isize;
                if i >= 0 && (i as usize) < x.len() {
                    re += hk * x[i as usize].0;
                    im += hk * x[i as usize].1;
                }
            }
            C(re, im)
        })
        .collect()
}

fn mix(x: &[C], hz: f64, fs: f64) -> Vec<C> {
    x.iter()
        .enumerate()
        .map(|(n, c)| {
            let p = 2.0 * PI * hz * n as f64 / fs;
            let (s, co) = p.sin_cos();
            C(c.0 * co - c.1 * s, c.0 * s + c.1 * co)
        })
        .collect()
}

/// A KAMI stream (8 kHz, carrier 1500 Hz, as the modulator makes it) as lane audio at 8 kHz,
/// centred on `centre_hz`, five times longer.
pub fn to_lane(kami: &[f32], centre_hz: f64) -> Vec<f32> {
    // Read at 1600 Hz: the carrier is 300 Hz. Quadrature mix to baseband and low-pass.
    let x: Vec<C> = kami.iter().map(|v| C(*v as f64 * 2.0, 0.0)).collect();
    let bb = filt(&mix(&x, -SLOW_CARRIER, FS / SLOW as f64), &lowpass(CUTOFF_HZ, FS / SLOW as f64, 301));
    // Up to 8 kHz: zero-stuff by five and low-pass (gain five restores the level).
    let mut up = vec![C(0.0, 0.0); bb.len() * SLOW];
    for (i, c) in bb.iter().enumerate() {
        up[i * SLOW] = C(c.0 * SLOW as f64, c.1 * SLOW as f64);
    }
    let up = filt(&up, &lowpass(CUTOFF_HZ, FS, 601));
    mix(&up, centre_hz, FS).iter().map(|c| c.0 as f32).collect()
}

/// The lane centred on `centre_hz` taken out of 8 kHz audio as a KAMI stream the ordinary
/// demodulator reads (one fifth as many samples, carrier at 1500 Hz in its own time).
pub fn from_lane(audio: &[f32], centre_hz: f64) -> Vec<f32> {
    let x: Vec<C> = audio.iter().map(|v| C(*v as f64, 0.0)).collect();
    let bb = filt(&mix(&x, -centre_hz, FS), &lowpass(CUTOFF_HZ, FS, 801));
    let low: Vec<C> = bb.iter().step_by(SLOW).copied().collect();
    mix(&low, SLOW_CARRIER, FS / SLOW as f64).iter().map(|c| c.0 as f32).collect()
}

/// Power spectrum (dB, relative to the largest bin) of `x` at the given frequencies, by direct
/// Hann-windowed DFT. Small inputs only.
pub fn psd_db(x: &[f32], freqs: &[f64]) -> Vec<f64> {
    let n = x.len();
    let p: Vec<f64> = freqs
        .iter()
        .map(|f| {
            let (mut re, mut im) = (0.0, 0.0);
            for (i, v) in x.iter().enumerate() {
                let w = 0.5 - 0.5 * (2.0 * PI * i as f64 / (n - 1) as f64).cos();
                let ph = 2.0 * PI * f * i as f64 / FS;
                re += *v as f64 * w * ph.cos();
                im -= *v as f64 * w * ph.sin();
            }
            re * re + im * im
        })
        .collect();
    let top = p.iter().cloned().fold(1e-30, f64::max);
    p.iter().map(|v| 10.0 * (v / top + 1e-30).log10()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kami::{self, Shape};
    use crate::ldpc;

    fn info(seed: u8) -> Vec<u8> {
        (0..ldpc::K).map(|i| ((i as u32 * 7 + seed as u32 * 13 + (i as u32 >> 3)) & 1) as u8).collect()
    }

    /// Send `info` through the lane at `centre`, decode it again.
    fn round_trip(sh: Shape, centre: f64, info: &[u8]) -> Option<Vec<u8>> {
        let air = kami::modulate_frame_shape(&ldpc::encode(info), sh);
        let mut audio = vec![0f32; 4000];
        audio.extend(to_lane(&air, centre));
        audio.extend(vec![0f32; 4000]);
        let rx = from_lane(&audio, centre);
        let (start, hz, tau) = kami::find_preamble(&rx)?;
        let (_, llr) = kami::demodulate_at(&rx, start, hz, tau, ldpc::N, &[sh]).into_iter().next()?;
        ldpc::decode(&llr, 50)
    }

    #[test]
    fn the_lane_is_five_times_longer() {
        let air = kami::modulate_frame_shape(&ldpc::encode(&info(1)), Shape { wb: 0, lb: 3 });
        let lane = to_lane(&air, 1500.0);
        assert_eq!(lane.len(), air.len() * SLOW);
        assert_eq!(from_lane(&lane, 1500.0).len(), air.len());
    }

    #[test]
    fn a_frame_survives_the_lane_at_several_centres_and_modes() {
        for centre in [1500.0, 2200.0, 800.0] {
            for sh in [Shape { wb: 0, lb: 3 }, Shape::walsh(5)] {
                let i = info(3);
                assert_eq!(round_trip(sh, centre, &i), Some(i), "centre {centre} wb {}", sh.wb);
            }
        }
    }

    #[test]
    fn the_signal_stays_inside_480_hz_at_minus_26_db() {
        let air = kami::modulate_frame_shape(&ldpc::encode(&info(5)), Shape::walsh(5));
        let lane = to_lane(&air, 1500.0);
        // The middle of the signal, preamble and data, avoiding the ends.
        let mid = &lane[lane.len() / 4..lane.len() * 3 / 4];
        let freqs: Vec<f64> = (0..=240).map(|k| 1500.0 - 1200.0 + 10.0 * k as f64).collect();
        let db = psd_db(&mid[..mid.len().min(16000)], &freqs);
        let inside: Vec<f64> =
            freqs.iter().zip(&db).filter(|(f, _)| (**f - 1500.0).abs() > 240.0 && **f > 0.0).map(|(_, d)| *d).collect();
        let worst = inside.iter().cloned().fold(f64::MIN, f64::max);
        assert!(worst < -26.0, "outside +-240 Hz the level reaches {worst:.1} dB");
    }
}
