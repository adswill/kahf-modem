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

/// Amateur allocations. Verify against the current national table before release.
///
/// Region 3 entries for 80 m (3500-3900 kHz) and 40 m (7000-7200 kHz) and
/// Region 2 40 m (7000-7300 kHz) are the least certain; check them first.
pub static ALLOCATIONS: &[Allocation] = &[
    // 2200 m
    Allocation {
        band: "2200m",
        region: R1,
        lower_hz: 135_700,
        upper_hz: 137_800,
    },
    Allocation {
        band: "2200m",
        region: R2,
        lower_hz: 135_700,
        upper_hz: 137_800,
    },
    Allocation {
        band: "2200m",
        region: R3,
        lower_hz: 135_700,
        upper_hz: 137_800,
    },
    // 630 m
    Allocation {
        band: "630m",
        region: R1,
        lower_hz: 472_000,
        upper_hz: 479_000,
    },
    Allocation {
        band: "630m",
        region: R2,
        lower_hz: 472_000,
        upper_hz: 479_000,
    },
    Allocation {
        band: "630m",
        region: R3,
        lower_hz: 472_000,
        upper_hz: 479_000,
    },
    // 160 m
    Allocation {
        band: "160m",
        region: R1,
        lower_hz: 1_810_000,
        upper_hz: 2_000_000,
    },
    Allocation {
        band: "160m",
        region: R2,
        lower_hz: 1_800_000,
        upper_hz: 2_000_000,
    },
    Allocation {
        band: "160m",
        region: R3,
        lower_hz: 1_800_000,
        upper_hz: 2_000_000,
    },
    // 80 m
    Allocation {
        band: "80m",
        region: R1,
        lower_hz: 3_500_000,
        upper_hz: 3_800_000,
    },
    Allocation {
        band: "80m",
        region: R2,
        lower_hz: 3_500_000,
        upper_hz: 4_000_000,
    },
    Allocation {
        band: "80m",
        region: R3,
        lower_hz: 3_500_000,
        upper_hz: 3_900_000,
    },
    // 40 m
    Allocation {
        band: "40m",
        region: R1,
        lower_hz: 7_000_000,
        upper_hz: 7_200_000,
    },
    Allocation {
        band: "40m",
        region: R2,
        lower_hz: 7_000_000,
        upper_hz: 7_300_000,
    },
    Allocation {
        band: "40m",
        region: R3,
        lower_hz: 7_000_000,
        upper_hz: 7_200_000,
    },
    // 30 m
    Allocation {
        band: "30m",
        region: R1,
        lower_hz: 10_100_000,
        upper_hz: 10_150_000,
    },
    Allocation {
        band: "30m",
        region: R2,
        lower_hz: 10_100_000,
        upper_hz: 10_150_000,
    },
    Allocation {
        band: "30m",
        region: R3,
        lower_hz: 10_100_000,
        upper_hz: 10_150_000,
    },
    // 20 m
    Allocation {
        band: "20m",
        region: R1,
        lower_hz: 14_000_000,
        upper_hz: 14_350_000,
    },
    Allocation {
        band: "20m",
        region: R2,
        lower_hz: 14_000_000,
        upper_hz: 14_350_000,
    },
    Allocation {
        band: "20m",
        region: R3,
        lower_hz: 14_000_000,
        upper_hz: 14_350_000,
    },
    // 17 m
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
    // 15 m
    Allocation {
        band: "15m",
        region: R1,
        lower_hz: 21_000_000,
        upper_hz: 21_450_000,
    },
    Allocation {
        band: "15m",
        region: R2,
        lower_hz: 21_000_000,
        upper_hz: 21_450_000,
    },
    Allocation {
        band: "15m",
        region: R3,
        lower_hz: 21_000_000,
        upper_hz: 21_450_000,
    },
    // 12 m
    Allocation {
        band: "12m",
        region: R1,
        lower_hz: 24_890_000,
        upper_hz: 24_990_000,
    },
    Allocation {
        band: "12m",
        region: R2,
        lower_hz: 24_890_000,
        upper_hz: 24_990_000,
    },
    Allocation {
        band: "12m",
        region: R3,
        lower_hz: 24_890_000,
        upper_hz: 24_990_000,
    },
    // 10 m
    Allocation {
        band: "10m",
        region: R1,
        lower_hz: 28_000_000,
        upper_hz: 29_700_000,
    },
    Allocation {
        band: "10m",
        region: R2,
        lower_hz: 28_000_000,
        upper_hz: 29_700_000,
    },
    Allocation {
        band: "10m",
        region: R3,
        lower_hz: 28_000_000,
        upper_hz: 29_700_000,
    },
    // 6 m
    Allocation {
        band: "6m",
        region: R1,
        lower_hz: 50_000_000,
        upper_hz: 52_000_000,
    },
    Allocation {
        band: "6m",
        region: R2,
        lower_hz: 50_000_000,
        upper_hz: 54_000_000,
    },
    Allocation {
        band: "6m",
        region: R3,
        lower_hz: 50_000_000,
        upper_hz: 54_000_000,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fourteen_mhz_is_in_20m() {
        assert_eq!(check(14_100_000, Region::Region1), Check::InBand { band: "20m" });
    }

    #[test]
    fn thirteen_point_nine_mhz_is_out_of_band_region1() {
        assert_eq!(check(13_900_000, Region::Region1), Check::OutOfBand);
    }

    #[test]
    fn near_lower_edge_is_detected() {
        assert_eq!(
            check(14_001_500, Region::Region1),
            Check::NearEdge {
                band: "20m",
                margin_hz: 1_500
            }
        );
    }

    #[test]
    fn near_upper_edge_is_detected() {
        assert_eq!(
            check(14_348_000, Region::Region1),
            Check::NearEdge {
                band: "20m",
                margin_hz: 2_000
            }
        );
    }

    #[test]
    fn exactly_5khz_inside_edge_is_near_edge() {
        assert_eq!(
            check(14_005_000, Region::Region1),
            Check::NearEdge {
                band: "20m",
                margin_hz: 5_000
            }
        );
    }

    #[test]
    fn well_inside_band_is_in_band() {
        assert_eq!(check(14_200_000, Region::Region2), Check::InBand { band: "20m" });
    }

    #[test]
    fn region_specific_edge_80m() {
        // 3.85 MHz: amateur in Region 2 only.
        assert_eq!(check(3_850_000, Region::Region1), Check::OutOfBand);
        assert_eq!(check(3_850_000, Region::Region2), Check::InBand { band: "80m" });
    }

    #[test]
    fn six_metre_region1_stops_at_52_mhz() {
        assert_eq!(check(52_500_000, Region::Region1), Check::OutOfBand);
        assert_eq!(check(52_500_000, Region::Region2), Check::InBand { band: "6m" });
    }
}
