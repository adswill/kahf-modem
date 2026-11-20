//! Airtime planning for files and images: how many chunks and seconds a payload needs, and the
//! largest payload that fits a time budget, so the UI can show "about 4 min" before sending and
//! the sender can pick image quality to match the channel.
use crate::session::CHUNK_DATA;

/// Fixed per-message cost on top of the body: meta byte and a signed block (callsign + key + sig).
const OVERHEAD_BYTES: usize = 1 + 96 + 12;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    pub body_bytes: usize,
    pub chunks: usize,
    /// Estimated seconds on air at `payload_bps`, including a per-chunk link overhead factor.
    pub seconds: f64,
}

/// Plans a payload of `body_bytes` (already compressed if it will be) at an effective
/// application throughput of `payload_bps` bits per second. `link_factor` multiplies the time
/// for framing, acknowledgements and retries (1.0 is lossless, 1.5 to 2.5 typical on HF).
pub fn plan(body_bytes: usize, payload_bps: f64, link_factor: f64) -> Plan {
    let total = body_bytes.saturating_add(OVERHEAD_BYTES);
    let chunks = total.div_ceil(CHUNK_DATA).max(1);
    let seconds = if payload_bps > 0.0 {
        (chunks * CHUNK_DATA) as f64 * 8.0 / payload_bps * link_factor.max(1.0)
    } else {
        f64::INFINITY
    };
    Plan {
        body_bytes,
        chunks,
        seconds,
    }
}

/// Largest body (bytes) that fits `budget_s` seconds, the inverse of [`plan`]. Capped at the
/// session limit.
pub fn max_body_for(budget_s: f64, payload_bps: f64, link_factor: f64) -> usize {
    if !(budget_s > 0.0 && payload_bps > 0.0) {
        return 0;
    }
    let bytes = (budget_s * payload_bps / 8.0 / link_factor.max(1.0)) as usize;
    bytes
        .saturating_sub(OVERHEAD_BYTES)
        .min(crate::session::MAX_MESSAGE - OVERHEAD_BYTES)
}

/// Chooses an image encoding for a time budget: the best quality whose estimated airtime fits.
pub fn encode_image_for(
    rgb: &[u8],
    width: u32,
    height: u32,
    budget_s: f64,
    payload_bps: f64,
    link_factor: f64,
) -> Result<(Vec<u8>, Plan), String> {
    let max = max_body_for(budget_s, payload_bps, link_factor);
    if max == 0 {
        return Err("no time budget".into());
    }
    let data = crate::image::encode_image(rgb, width, height, max)?;
    let p = plan(data.len(), payload_bps, link_factor);
    Ok((data, p))
}

/// "4 min 20 s" style label for the UI.
pub fn label(seconds: f64) -> String {
    if !seconds.is_finite() {
        return "unknown".into();
    }
    let s = seconds.round() as u64;
    match (s / 3600, (s % 3600) / 60, s % 60) {
        (0, 0, s) => format!("{s} s"),
        (0, m, s) => format!("{m} min {s} s"),
        (h, m, _) => format!("{h} h {m} min"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_scales_and_inverts() {
        let p = plan(1000, 100.0, 2.0);
        assert!(p.chunks > 1000 / CHUNK_DATA);
        let back = max_body_for(p.seconds, 100.0, 2.0);
        assert!(back >= 1000 - CHUNK_DATA && back <= 1000 + CHUNK_DATA, "{back}");
        assert!(plan(0, 100.0, 1.0).chunks >= 1);
        assert!(plan(10, 0.0, 1.0).seconds.is_infinite());
        assert_eq!(max_body_for(0.0, 100.0, 1.0), 0);
    }

    #[test]
    fn image_fits_budget() {
        let (w, h) = (64u32, 48u32);
        let rgb: Vec<u8> = (0..w * h * 3).map(|i| (i * 7 % 251) as u8).collect();
        let (data, p) = encode_image_for(&rgb, w, h, 300.0, 200.0, 2.0).unwrap();
        assert!(p.seconds <= 300.0 + 5.0, "{}", p.seconds);
        assert!(crate::image::decode_image(&data).is_ok());
        assert!(encode_image_for(&rgb, w, h, 0.5, 200.0, 2.0).is_err());
    }

    #[test]
    fn labels() {
        assert_eq!(label(45.0), "45 s");
        assert_eq!(label(260.0), "4 min 20 s");
        assert_eq!(label(7260.0), "2 h 1 min");
    }
}
