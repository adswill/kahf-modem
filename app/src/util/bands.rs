//! KAHF works on 17 metres only, in 18.111 to 18.120 MHz (IARU Region 1, "all modes, digimodes,
//! automatically controlled data stations"). The band holds twelve narrow KAHF lanes, each 480 Hz
//! wide with 700 Hz between centres (`kahf-core`, `lanes.rs`). A lane is chosen by the radio's
//! USB dial: the dial that puts the lane at 1500 Hz audio.

use kahf_core::lanes::{BandTable, Role};

/// One KAHF lane, as the person sees it on the dial.
#[derive(Debug, Clone, PartialEq)]
pub struct Channel {
    /// 0 to 11, lowest frequency first.
    pub index: usize,
    /// Short label: R1 to R8 (relay lanes), C1 and C2 (calling), X1 and X2 (spare).
    pub name: String,
    /// The USB dial in kHz: what to tune the radio to.
    pub khz: f64,
    /// What the lane is for, in plain words.
    pub what: &'static str,
}

/// The lanes, lowest frequency first (the order they sit on the dial).
pub fn channels() -> Vec<Channel> {
    let table = BandTable::builtin_r1();
    let (mut relay, mut calling, mut spare) = (0, 0, 0);
    table.bands[0]
        .lanes
        .iter()
        .map(|l| {
            let (name, what) = match l.role {
                Role::Relay => {
                    relay += 1;
                    (format!("R{relay}"), "relay lane")
                }
                Role::Calling => {
                    calling += 1;
                    (format!("C{calling}"), "calling lane")
                }
                Role::Spare => {
                    spare += 1;
                    (format!("X{spare}"), "spare lane")
                }
            };
            Channel { index: l.k as usize, name, khz: l.dial_hz as f64 / 1000.0, what }
        })
        .collect()
}

/// Where a new station starts: calling lane C1.
pub fn default_khz() -> f64 {
    channels().iter().find(|c| c.name == "C1").map_or(18_113.44, |c| c.khz)
}

/// Index of the lane the dial frequency belongs to, if it is one of them.
pub fn index_of(khz: f64) -> Option<usize> {
    channels().iter().position(|c| (c.khz - khz).abs() < 0.001)
}

/// A frequency saved by an older version (another band, or a channel of the three-channel
/// version) becomes calling lane C1.
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
    fn the_band_holds_twelve_lanes_and_c1_is_the_default() {
        let c = channels();
        assert_eq!(c.len(), 12);
        assert!(c.windows(2).all(|w| w[1].khz > w[0].khz));
        assert_eq!(c[5].name, "C1");
        assert!((default_khz() - 18_113.44).abs() < 1e-9);
        assert_eq!(c.iter().filter(|x| x.name.starts_with('R')).count(), 8);
    }

    #[test]
    fn an_old_frequency_falls_back_to_c1() {
        assert_eq!(khz_or_default(14_070.0), default_khz());
        assert_eq!(khz_or_default(18_112.35), default_khz());
        let r1 = channels()[0].khz;
        assert_eq!(khz_or_default(r1), r1);
    }
}
