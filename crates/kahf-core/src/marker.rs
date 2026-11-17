//! Start-of-transmission marker for KAHF, and a searcher for it.
//!
//! Construction (KAHF's own):
//! - Chips: 127 chips of the maximal-length sequence of the primitive
//!   polynomial x^7 + x^3 + 1 (recurrence a[n+7] = a[n] XOR a[n+3]),
//!   started from the 7-bit seed 0x69 (the seed with the lowest
//!   aperiodic sidelobe peak of the 127 cyclic shifts). Bit 1 maps to -1, bit 0 to +1.
//! - Each chip lasts 6 samples at 8000 Hz (chip rate 1333.3 Hz) and is shaped
//!   by a half-sine pulse.
//! - The chip train is BPSK on a 1500 Hz carrier, so the marker is a real
//!   signal whose energy sits inside the 300 to 2700 Hz voice band.
//! - Length 762 samples (95.25 ms). Peak amplitude is 0.95.
//!
//! The searcher computes normalised cross-correlation with an FFT
//! overlap-add correlator and keeps one peak per marker length.

use std::cmp::Ordering;
use std::f64::consts::PI;

const FS: f64 = 8000.0;
const CARRIER_HZ: f64 = 1500.0;
const SEED: u8 = 0x69;
const CHIPS: usize = 127;
const SPC: usize = 6;

/// Default detection threshold on normalised correlation (0..1).
/// Pure noise peaks stay near 0.2; a marker at 0 dB SNR sits near 0.7.
pub const DEFAULT_THRESHOLD: f32 = 0.4;

fn chip_sequence() -> [f64; CHIPS] {
    let mut state = SEED;
    let mut out = [0.0; CHIPS];
    for c in out.iter_mut() {
        *c = if state & 1 == 1 { -1.0 } else { 1.0 };
        let fb = (state ^ (state >> 3)) & 1;
        state = (state >> 1) | (fb << 6);
    }
    out
}

/// Chip train synthesised at `fs` with `spc` samples per chip, carrier `fc`.
fn synth(spc: usize, fc: f64, fs: f64) -> Vec<f64> {
    let chips = chip_sequence();
    (0..CHIPS * spc)
        .map(|i| {
            let env = (PI * ((i % spc) as f64 + 0.5) / spc as f64).sin();
            chips[i / spc] * env * (2.0 * PI * fc * i as f64 / fs).cos()
        })
        .collect()
}

fn scaled(m: Vec<f64>, peak: f64) -> Vec<f64> {
    let p = m.iter().fold(0.0f64, |a, &v| a.max(v.abs()));
    m.into_iter().map(|v| v / p * peak).collect()
}

/// The transmitted marker at 8000 Hz (762 samples, peak 0.95).
pub fn marker_samples() -> Vec<f32> {
    scaled(synth(SPC, CARRIER_HZ, FS), 0.95)
        .into_iter()
        .map(|v| v as f32)
        .collect()
}

fn template() -> Vec<f64> {
    marker_samples().into_iter().map(|v| v as f64).collect()
}

/// In-place radix-2 complex FFT. Length must be a power of two.
fn fft(re: &mut [f64], im: &mut [f64], inverse: bool) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let sign = if inverse { 1.0 } else { -1.0 };
    let mut len = 2;
    while len <= n {
        let ang = sign * 2.0 * PI / len as f64;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f64, 0.0f64);
            for k in 0..len / 2 {
                let a = start + k;
                let b = a + len / 2;
                let tr = re[b] * cr - im[b] * ci;
                let ti = re[b] * ci + im[b] * cr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
        }
        len <<= 1;
    }
    if inverse {
        let s = 1.0 / n as f64;
        for v in re.iter_mut() {
            *v *= s;
        }
        for v in im.iter_mut() {
            *v *= s;
        }
    }
}

/// Normalised cross-correlation of `x` with `tmpl`, computed by FFT
/// overlap-add. Output index i is the window starting at x[i]; the length
/// is x.len() - tmpl.len() + 1 (empty if x is shorter than the template).
fn normalised_correlation(x: &[f64], tmpl: &[f64]) -> Vec<f64> {
    let l = tmpl.len();
    if x.len() < l || l == 0 {
        return Vec::new();
    }
    let n_out = x.len() - l + 1;
    let nfft = (2 * l).next_power_of_two();
    let step = nfft - l + 1;

    // Spectrum of the time-reversed template.
    let mut hr = vec![0.0; nfft];
    let mut hi = vec![0.0; nfft];
    for k in 0..l {
        hr[l - 1 - k] = tmpl[k];
    }
    fft(&mut hr, &mut hi, false);

    let mut corr = vec![0.0; n_out];
    let mut br = vec![0.0; nfft];
    let mut bi = vec![0.0; nfft];
    let mut s = 0;
    while s < n_out {
        for i in 0..nfft {
            br[i] = if s + i < x.len() { x[s + i] } else { 0.0 };
            bi[i] = 0.0;
        }
        fft(&mut br, &mut bi, false);
        for i in 0..nfft {
            let (ar, ai) = (br[i], bi[i]);
            let (cr, ci) = (hr[i], hi[i]);
            br[i] = ar * cr - ai * ci;
            bi[i] = ar * ci + ai * cr;
        }
        fft(&mut br, &mut bi, true);
        // Indices l-1..nfft are free of circular wrap-around.
        for (j, &v) in br.iter().enumerate().skip(l - 1) {
            let idx = s + j - (l - 1);
            if idx < n_out {
                corr[idx] = v;
            }
        }
        s += step;
    }

    let mut p2 = vec![0.0; x.len() + 1];
    for i in 0..x.len() {
        p2[i + 1] = p2[i] + x[i] * x[i];
    }
    let et = tmpl.iter().map(|v| v * v).sum::<f64>().sqrt();
    (0..n_out)
        .map(|i| {
            let ex = (p2[i + l] - p2[i]).sqrt();
            if ex < 1e-12 || et < 1e-12 {
                0.0
            } else {
                corr[i] / (ex * et)
            }
        })
        .collect()
}

/// Start samples of every marker in `samples` whose normalised correlation
/// is at least `threshold`. One detection is kept per marker length (the
/// strongest peak wins). Returned indices are ascending.
pub fn find_markers(samples: &[f32], threshold: f32) -> Vec<usize> {
    let tmpl = template();
    let x: Vec<f64> = samples.iter().map(|&v| v as f64).collect();
    let ncc = normalised_correlation(&x, &tmpl);
    let thr = threshold as f64;
    let mut cand: Vec<usize> = (0..ncc.len()).filter(|&i| ncc[i] >= thr).collect();
    cand.sort_by(|&a, &b| ncc[b].partial_cmp(&ncc[a]).unwrap_or(Ordering::Equal));
    let l = tmpl.len();
    let mut accepted: Vec<usize> = Vec::new();
    for c in cand {
        if accepted.iter().all(|&a| a.abs_diff(c) >= l) {
            accepted.push(c);
        }
    }
    accepted.sort_unstable();
    accepted
}

/// Wide-capture search. Splits 0..fs/2 into `bins` sub-bands, mixes each
/// band to baseband with a real cosine at its centre, low-pass filters it
/// (3.5 kHz cut-off) and decimates to 8000 Hz, then correlates with the
/// marker. Returns, per band, the largest |normalised correlation| over time.
///
/// Real mixing only: the marker occupies about 5 kHz of RF, so bands must be
/// at least that wide (bins <= sample_rate / 10000) or neighbouring bands
/// pick up its energy. The sample rate must be an integer multiple of
/// 8000 Hz, otherwise all zeros are returned.
pub fn search_wide(capture: &[f32], sample_rate: f32, bins: usize) -> Vec<f32> {
    let mut best = vec![0.0f32; bins];
    let fs = sample_rate as f64;
    let d = (fs / FS).round();
    if bins == 0 || d < 1.0 || (fs - d * FS).abs() > 1e-6 * fs {
        return best;
    }
    let d = d as usize;
    let half = 16 * d;
    let cutoff = 3500.0 / fs;
    let taps: Vec<f64> = (0..=2 * half)
        .map(|k| {
            let t = k as f64 - half as f64;
            let sinc = if t == 0.0 {
                2.0 * cutoff
            } else {
                (2.0 * PI * cutoff * t).sin() / (PI * t)
            };
            let w = 0.54 - 0.46 * (2.0 * PI * k as f64 / (2 * half) as f64).cos();
            sinc * w
        })
        .collect();

    let x: Vec<f64> = capture.iter().map(|&v| v as f64).collect();
    let tmpl = template();
    for (b, slot) in best.iter_mut().enumerate() {
        let fb = (b as f64 + 0.5) * (fs / 2.0) / bins as f64;
        let w = 2.0 * PI * fb / fs;
        let mixed: Vec<f64> = x
            .iter()
            .enumerate()
            .map(|(n, &v)| v * (w * n as f64).cos())
            .collect();
        let nout = mixed.len() / d;
        let base: Vec<f64> = (0..nout)
            .map(|t| {
                let c = (t * d) as isize - half as isize;
                let mut acc = 0.0;
                for (k, &h) in taps.iter().enumerate() {
                    let idx = c + k as isize;
                    if idx >= 0 && (idx as usize) < mixed.len() {
                        acc += h * mixed[idx as usize];
                    }
                }
                acc
            })
            .collect();
        let ncc = normalised_correlation(&base, &tmpl);
        *slot = ncc.iter().fold(0.0f64, |m, &v| m.max(v.abs())) as f32;
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u64);
    impl Rng {
        fn uniform(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
        fn gauss(&mut self) -> f64 {
            let u1 = self.uniform().max(1e-300);
            let u2 = self.uniform();
            (-2.0 * u1.ln()).sqrt() * (2.0 * PI * u2).cos()
        }
    }

    fn noise(n: usize, sigma: f64, rng: &mut Rng) -> Vec<f64> {
        (0..n).map(|_| sigma * rng.gauss()).collect()
    }

    fn marker_power(m: &[f32]) -> f64 {
        m.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / m.len() as f64
    }

    #[test]
    fn marker_shape_length_peak_and_band() {
        let m = marker_samples();
        assert!(m.len() <= 800, "length {}", m.len());
        let peak = m.iter().fold(0.0f32, |a, &v| a.max(v.abs()));
        assert!((peak - 0.95).abs() < 1e-5, "peak {}", peak);
        // Energy in 300..2700 Hz via a direct DFT.
        let n = m.len();
        let (mut inband, mut total) = (0.0f64, 0.0f64);
        for k in 0..n / 2 {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (i, &v) in m.iter().enumerate() {
                let a = 2.0 * PI * k as f64 * i as f64 / n as f64;
                re += v as f64 * a.cos();
                im -= v as f64 * a.sin();
            }
            let p = re * re + im * im;
            total += p;
            let f = k as f64 * FS / n as f64;
            if (300.0..=2700.0).contains(&f) {
                inband += p;
            }
        }
        let frac = inband / total;
        println!(
            "marker length {} ms, in-band energy {:.3}",
            n as f64 * 1000.0 / FS,
            frac
        );
        assert!(frac > 0.9, "in-band fraction {}", frac);
    }

    #[test]
    fn marker_sidelobe_level() {
        // Full-lag autocorrelation: pad the template by its own length on
        // both sides so every lag appears; zero lag sits at index l - 1 + l.
        let t = template();
        let l = t.len();
        let mut padded = vec![0.0; l - 1];
        padded.extend_from_slice(&t);
        padded.extend(vec![0.0; l - 1]);
        let ncc = normalised_correlation(&padded, &t);
        let zero = l - 1;
        let chip = SPC;
        let side = ncc
            .iter()
            .enumerate()
            .filter(|(i, _)| (*i as isize - zero as isize).unsigned_abs() >= chip)
            .fold(0.0f64, |m, (_, &v)| m.max(v.abs()));
        let peak = ncc[zero];
        assert!(peak > 0.99, "zero-lag peak {}", peak);
        let db = 20.0 * side.log10();
        println!("marker sidelobe peak {:.4} ({:.1} dB)", side, db);
        assert!(db < -15.0, "sidelobe {} dB", db);
    }

    #[test]
    fn marker_detected_at_0db_in_noise() {
        let m = marker_samples();
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let sigma = marker_power(&m).sqrt();
        let n = 10 * FS as usize;
        let mut x = noise(n, sigma, &mut rng);
        let off = 40_000usize;
        for (i, &v) in m.iter().enumerate() {
            x[off + i] += v as f64;
        }
        let xf: Vec<f32> = x.iter().map(|&v| v as f32).collect();
        let d = find_markers(&xf, DEFAULT_THRESHOLD);
        println!("0 dB detections: {:?}", d);
        assert_eq!(d.len(), 1, "detections {:?}", d);
        assert!((d[0] as isize - off as isize).abs() <= 1, "at {}", d[0]);
    }

    #[test]
    fn marker_no_false_alarm_in_noise() {
        let mut rng = Rng(0x1234_5678_9ABC_DEF1);
        let n = 10 * FS as usize;
        let x: Vec<f32> = noise(n, 1.0, &mut rng).iter().map(|&v| v as f32).collect();
        let d = find_markers(&x, DEFAULT_THRESHOLD);
        println!("false alarms in 10 s noise: {}", d.len());
        assert!(d.is_empty(), "false alarms at {:?}", d);
    }

    #[test]
    fn marker_two_markers_one_second_apart() {
        let m = marker_samples();
        let mut rng = Rng(0xDEAD_BEEF_0BAD_F00D);
        let sigma = marker_power(&m).sqrt();
        let n = 3 * FS as usize;
        let mut x = noise(n, sigma, &mut rng);
        let offs = [8_000usize, 16_000usize];
        for &off in &offs {
            for (i, &v) in m.iter().enumerate() {
                x[off + i] += v as f64;
            }
        }
        let xf: Vec<f32> = x.iter().map(|&v| v as f32).collect();
        let d = find_markers(&xf, DEFAULT_THRESHOLD);
        println!("two-marker detections: {:?}", d);
        assert_eq!(d.len(), 2, "detections {:?}", d);
        for (got, want) in d.iter().zip(offs.iter()) {
            assert!((*got as isize - *want as isize).abs() <= 1, "{} vs {}", got, want);
        }
    }

    #[test]
    fn marker_search_wide_finds_subband() {
        // Marker at 48 kHz, RF-modulated onto the centre of sub-band 1 of 4
        // (6 kHz wide bands, centres 3, 9, 15, 21 kHz).
        let fs = 48_000.0f64;
        let fc = 9_000.0f64;
        let m48 = scaled(synth(36, CARRIER_HZ, fs), 0.95);
        let n = 2 * fs as usize;
        let off = 30_000usize;
        let mut cap = vec![0.0f32; n];
        for (i, &v) in m48.iter().enumerate() {
            let t = (off + i) as f64;
            cap[off + i] = (v * (2.0 * PI * fc * t / fs).cos()) as f32;
        }
        let best = search_wide(&cap, fs as f32, 4);
        println!("search_wide per band: {:?}", best);
        let arg = best
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(Ordering::Equal))
            .map(|(i, _)| i)
            .unwrap();
        assert_eq!(arg, 1);
        assert!(best[1] > 0.8, "band 1 {}", best[1]);
        for (b, &v) in best.iter().enumerate() {
            if b != 1 {
                assert!(v < best[1] * 0.6, "band {} {}", b, v);
            }
        }
    }
}
