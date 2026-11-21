//! KAHF works on 17 metres only, in 18.111 to 18.120 MHz (IARU Region 1, "all modes, digimodes,
//! automatically controlled data stations"). The band holds three KAHF channels, each 2.7 kHz
//! wide, the same plan as the engine's default plan (`kahf-core`, `select.rs`).

/// One KAHF channel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Channel {
    /// 0 to 2, lowest frequency first.
    pub index: usize,
    /// Short label for the dial: "Ch 1" to "Ch 3".
    pub name: &'static str,
    /// Centre in kHz.
    pub khz: f64,
}

const BAND_LOW_HZ: u32 = 18_111_000;
const BAND_HIGH_HZ: u32 = 18_120_000;
const SPACING_HZ: u32 = 2_700;
const NAMES: [&str; 3] = ["Ch 1", "Ch 2", "Ch 3"];

/// The channels, lowest frequency first (the order they sit on the dial).
pub fn channels() -> Vec<Channel> {
    let mut out = Vec::new();
    let mut centre = BAND_LOW_HZ + SPACING_HZ / 2;
    while centre + SPACING_HZ / 2 <= BAND_HIGH_HZ {
        let index = out.len();
        out.push(Channel {
            index,
            name: NAMES[index],
            khz: f64::from(centre) / 1000.0,
        });
        centre += SPACING_HZ;
    }
    out
}

/// Where a new station starts: the first channel.
pub fn default_khz() -> f64 {
    channels()[0].khz
}

/// Index of the channel the frequency belongs to, if it is one of them.
pub fn index_of(khz: f64) -> Option<usize> {
    channels().iter().position(|c| (c.khz - khz).abs() < 0.001)
}

/// A frequency saved by an older version (another band, or a lane of the lane test version)
/// becomes the first channel.
pub fn khz_or_default(khz: f64) -> f64 {
    if index_of(khz).is_some() {
        khz
    } else {
        default_khz()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_band_holds_three_channels_of_the_core_plan() {
        let c = channels();
        assert_eq!(c.len(), 3);
        assert!((c[0].khz - 18_112.35).abs() < 1e-9);
        assert!(c[2].khz > c[1].khz && c[1].khz > c[0].khz);
        // The same centres the engine uses.
        let engine = kahf_app_core::settings::channels_khz();
        assert_eq!(engine.len(), c.len());
        for (a, b) in engine.iter().zip(&c) {
            assert!((a - b.khz).abs() < 0.001);
        }
    }

    #[test]
    fn an_old_frequency_falls_back_to_channel_one() {
        assert_eq!(khz_or_default(14_070.0), default_khz());
        let c2 = channels()[1].khz;
        assert_eq!(khz_or_default(c2), c2);
    }
}
