//! Impulse-noise blanker.
//!
//! A sample is an impulse when its magnitude exceeds `THRESHOLD` times the
//! running median absolute value over the last `WINDOW` samples (the current
//! sample included). Each impulse is zeroed together with its neighbours up to
//! `RADIUS` samples away on either side.

const WINDOW: usize = 64;
const THRESHOLD: f32 = 5.0;
const RADIUS: usize = 2;

/// Returns a copy of `samples` with every detected impulse and its
/// neighbours set to zero.
pub fn blank(samples: &[f32]) -> Vec<f32> {
    let mask = impulse_mask(samples);
    samples
        .iter()
        .zip(mask)
        .map(|(&s, blanked)| if blanked { 0.0 } else { s })
        .collect()
}

/// Fraction of samples (0.0 to 1.0) that `blank` would set to zero.
pub fn blanked_fraction(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let mask = impulse_mask(samples);
    mask.iter().filter(|&&m| m).count() as f32 / samples.len() as f32
}

fn impulse_mask(samples: &[f32]) -> Vec<bool> {
    let n = samples.len();
    let mut mask = vec![false; n];
    let mut window: Vec<f32> = Vec::with_capacity(WINDOW);
    for i in 0..n {
        let start = (i + 1).saturating_sub(WINDOW);
        window.clear();
        window.extend(samples[start..=i].iter().map(|s| s.abs()));
        let mid = window.len() / 2;
        let (_, median, _) = window.select_nth_unstable_by(mid, |a, b| a.total_cmp(b));
        let median = *median;
        if samples[i].abs() > THRESHOLD * median {
            let lo = i.saturating_sub(RADIUS);
            let hi = (i + RADIUS).min(n - 1);
            for m in &mut mask[lo..=hi] {
                *m = true;
            }
        }
    }
    mask
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 8000.0;

    fn tone(len: usize) -> Vec<f32> {
        (0..len)
            .map(|k| 0.5 * (2.0 * std::f32::consts::PI * 1000.0 * k as f32 / RATE).sin())
            .collect()
    }

    #[test]
    fn blank_clean_signal_is_unchanged() {
        let clean = tone(8000);
        let out = blank(&clean);
        assert_eq!(out.len(), clean.len());
        let changed = out.iter().zip(&clean).filter(|(a, b)| a != b).count();
        assert!(
            changed as f32 <= 0.001 * clean.len() as f32,
            "changed {changed} of {} samples",
            clean.len()
        );
        assert!(blanked_fraction(&clean) <= 0.001);
    }

    #[test]
    fn blank_removes_click_from_tone() {
        let mut sig = tone(8000);
        let at = 4000;
        sig[at] = 3.0;
        let out = blank(&sig);
        assert!(out[at].abs() < 1e-6, "click not removed: {}", out[at]);
        for (k, v) in out.iter().enumerate().skip(at - 2).take(5) {
            assert_eq!(*v, 0.0, "neighbour {k} not blanked");
        }
        assert!(blanked_fraction(&sig) > 0.0);
    }

    #[test]
    fn blank_empty_input() {
        assert!(blank(&[]).is_empty());
        assert_eq!(blanked_fraction(&[]), 0.0);
    }
}
