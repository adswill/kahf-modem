//! Amateur band plan check. Advisory only: it reports, it never blocks transmit.
//!
//! The table below is DATA, not law. The values follow the public IARU band
//! plans as understood when this was written. They MUST be checked against the
//! current national regulator's table before release, because allocations change.
//!
//! Edge values are in Hz, lower and upper edge inclusive.

/// IARU region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    Region1,
    Region2,
    Region3,
}

/// One amateur allocation for one region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Allocation {
    pub band: &'static str,
    pub region: Region,
    pub lower_hz: u64,
    pub upper_hz: u64,
}

/// Result of a band plan check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    InBand {
        band: &'static str,
    },
    OutOfBand,
    /// Inside the band, within `NEAR_EDGE_HZ` of a lower or upper edge.
    NearEdge {
        band: &'static str,
        margin_hz: u64,
    },
}

/// Distance from an edge that counts as "near edge".
pub const NEAR_EDGE_HZ: u64 = 5_000;

use Region::{Region1 as R1, Region2 as R2, Region3 as R3};

/// Amateur allocation KAHF works in: 17 m, the same 18.068 to 18.168 MHz in all three regions.
/// Verify against the current national table before release. KAHF knows no other band, so a
/// frequency anywhere else is reported as out of band.
pub static ALLOCATIONS: &[Allocation] = &[
    Allocation {
        band: "17m",
        region: R1,
        lower_hz: 18_068_000,
        upper_hz: 18_168_000,
    },
    Allocation {
        band: "17m",
        region: R2,
        lower_hz: 18_068_000,
        upper_hz: 18_168_000,
    },
    Allocation {
        band: "17m",
        region: R3,
        lower_hz: 18_068_000,
        upper_hz: 18_168_000,
    },
];

/// Check a frequency against the amateur allocations for `region`.
///
/// Never blocks: callers decide what to do with the answer (warn, log, show).
pub fn check(freq_hz: u64, region: Region) -> Check {
    let hit = ALLOCATIONS
        .iter()
        .find(|a| a.region == region && freq_hz >= a.lower_hz && freq_hz <= a.upper_hz);
    match hit {
        None => Check::OutOfBand,
        Some(a) => {
            let margin_hz = (freq_hz - a.lower_hz).min(a.upper_hz - freq_hz);
            if margin_hz <= NEAR_EDGE_HZ {
                Check::NearEdge {
                    band: a.band,
                    margin_hz,
                }
            } else {
                Check::InBand { band: a.band }
            }
        }
    }
}

/// Width of one KAHF channel in Hz (the 2.7 kHz channel grid in select.rs).
pub const KAHF_CHANNEL_WIDTH_HZ: u64 = 2_700;

/// A sub-segment of the band plan where KAHF channels may sit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub name: &'static str,
    pub lower_hz: u64,
    pub upper_hz: u64,
}

/// The segment KAHF uses: 17 m only, 18.111 to 18.120 MHz (IARU Region 1: "all modes, digimodes,
/// automatically controlled data stations", 2700 Hz). Read from the IARU Region 1 HF band plan
/// effective 16 Oct 2020; the engine still refuses to transmit by it until the owner marks the
/// table verified (see `lanes.rs`).
///
/// Left out on purpose: 18.109 to 18.111 MHz (International Beacon Project) and everything below
/// 18.111 MHz (FT8, FT4, WSPR and narrow digimodes). The 12 narrow lanes inside this segment are
/// in `lanes.rs`.
pub static KAHF_SEGMENTS: &[Segment] = &[Segment {
    name: "17 m, 18.111 to 18.120 MHz",
    lower_hz: 18_111_000,
    upper_hz: 18_120_000,
}];

/// Where relays work by default: 17 m, 18.111 to 18.120 MHz (three channels).
pub const RELAY_SEGMENT: Segment = Segment {
    name: "17 m, 18.111 to 18.120 MHz",
    lower_hz: 18_111_000,
    upper_hz: 18_120_000,
};

/// Name of the KAHF segment that holds the whole channel, or `None`.
///
/// The channel is `centre_hz` plus or minus half of `width_hz`. Both edges must lie inside
/// the same segment, inclusive. Region 1 only. Never blocks anything: callers decide.
pub fn allowed_segment(centre_hz: u64, width_hz: u64) -> Option<&'static str> {
    let half = width_hz / 2;
    let lo = centre_hz.checked_sub(half)?;
    let hi = centre_hz.checked_add(width_hz - half)?;
    KAHF_SEGMENTS
        .iter()
        .find(|s| lo >= s.lower_hz && hi <= s.upper_hz)
        .map(|s| s.name)
}

/// Plain English warning for a tuning outside the planned segments, or `None` when it is inside.
///
/// This is a warning only. Transmit is never refused for band or licence reasons.
pub fn tuning_warning(centre_hz: u64, width_hz: u64) -> Option<String> {
    match allowed_segment(centre_hz, width_hz) {
        Some(_) => None,
        None => Some(
            "This tuning is outside the KAHF segment (17 m, 18.111 to 18.120 MHz). \
             Check the band plan before you transmit."
                .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mid_17m_is_in_band() {
        assert_eq!(check(18_115_000, Region::Region1), Check::InBand { band: "17m" });
        assert_eq!(check(18_115_000, Region::Region2), Check::InBand { band: "17m" });
    }

    #[test]
    fn everything_but_17m_is_out_of_band() {
        for hz in [14_100_000, 7_042_000, 3_600_000, 18_067_999, 18_168_001] {
            for r in [Region::Region1, Region::Region2, Region::Region3] {
                assert_eq!(check(hz, r), Check::OutOfBand, "{hz}");
            }
        }
    }

    #[test]
    fn near_edges_are_detected() {
        assert_eq!(
            check(18_069_500, Region::Region1),
            Check::NearEdge { band: "17m", margin_hz: 1_500 }
        );
        assert_eq!(
            check(18_166_000, Region::Region1),
            Check::NearEdge { band: "17m", margin_hz: 2_000 }
        );
        assert_eq!(
            check(18_073_000, Region::Region1),
            Check::NearEdge { band: "17m", margin_hz: 5_000 }
        );
    }

    const N17: &str = "17 m, 18.111 to 18.120 MHz";

    #[test]
    fn segment_17m_lower_edge_inclusive() {
        // Centre 18_112_350 with 2.7 kHz width: lower edge is exactly 18_111_000.
        assert_eq!(allowed_segment(18_112_350, KAHF_CHANNEL_WIDTH_HZ), Some(N17));
        assert_eq!(allowed_segment(18_112_349, KAHF_CHANNEL_WIDTH_HZ), None);
    }

    #[test]
    fn segment_17m_upper_edge_inclusive() {
        // Centre 18_118_650: upper edge is exactly 18_120_000.
        assert_eq!(allowed_segment(18_118_650, KAHF_CHANNEL_WIDTH_HZ), Some(N17));
        assert_eq!(allowed_segment(18_118_651, KAHF_CHANNEL_WIDTH_HZ), None);
    }

    #[test]
    fn old_20m_and_40m_segments_are_gone() {
        assert_eq!(allowed_segment(14_105_050, KAHF_CHANNEL_WIDTH_HZ), None);
        assert_eq!(allowed_segment(7_041_350, KAHF_CHANNEL_WIDTH_HZ), None);
        assert_eq!(KAHF_SEGMENTS.len(), 1);
    }

    #[test]
    fn channel_must_fit_fully_not_just_its_centre() {
        // Centre inside 17 m but the lower half of the channel is below 18.111 MHz.
        assert_eq!(allowed_segment(18_111_500, KAHF_CHANNEL_WIDTH_HZ), None);
        // Centre inside the segment, width larger than the segment is not allowed.
        assert_eq!(allowed_segment(18_115_000, 12_000), None);
    }

    #[test]
    fn beacons_sstv_and_30m_are_not_allowed() {
        assert_eq!(allowed_segment(18_110_000, 480), None); // IBP beacons, 18.109 to 18.111
        assert_eq!(allowed_segment(18_103_000, 480), None); // FT8
        assert_eq!(allowed_segment(18_130_000, 480), None); // SSB QRP, above the data segment
        assert_eq!(allowed_segment(14_230_000, KAHF_CHANNEL_WIDTH_HZ), None); // outside 17 m
        assert_eq!(allowed_segment(10_120_000, KAHF_CHANNEL_WIDTH_HZ), None); // outside 17 m
    }

    #[test]
    fn extreme_inputs_do_not_panic() {
        assert_eq!(allowed_segment(0, KAHF_CHANNEL_WIDTH_HZ), None);
        assert_eq!(allowed_segment(u64::MAX, KAHF_CHANNEL_WIDTH_HZ), None);
    }

    #[test]
    fn tuning_warning_is_only_a_warning() {
        assert_eq!(tuning_warning(18_115_050, KAHF_CHANNEL_WIDTH_HZ), None);
        let w = tuning_warning(14_105_050, KAHF_CHANNEL_WIDTH_HZ).expect("warning expected");
        assert!(w.contains("outside the KAHF segment"));
        assert!(!w.contains("refused"));
    }
}
