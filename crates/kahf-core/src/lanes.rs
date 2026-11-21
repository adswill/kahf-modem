//! The band table and the lanes (MESH-SPEC.md sections 2 and 3).
//!
//! All frequencies are whole hertz. Nothing here is a constant of the code: the lanes, the
//! segments and the excluded ranges are data in a `BandTable`. The server signs a table; an
//! app keeps its built-in copy and refuses a table that is unsigned, badly signed, or older.
//!
//! Two kinds of check, kept apart on purpose:
//!
//! - `check_span` is geometry: does this transmitted span fit the table? It says nothing about
//!   whether the table is true.
//! - `permit_tx` also requires the table to be marked verified against the current band plan.
//!   The built-in 17 m table is **not** marked verified (it was read from the 2016 IARU Region 1
//!   plan and has not been compared with the newest one), so nothing may transmit by it yet.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

/// The audio carrier the modem centres a lane on (Hz). Dial = lane centre - this (USB).
pub const AUDIO_CENTRE_HZ: u64 = 1500;
/// Signal width of one lane and the pitch between lane centres (Hz).
pub const LANE_WIDTH_HZ: u64 = 480;
pub const LANE_PITCH_HZ: u64 = 700;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    R1 = 1,
    R2 = 2,
    R3 = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Relay,
    Calling,
    Spare,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lane {
    pub k: u8,
    pub role: Role,
    /// Transmitted lower edge, centre and the USB dial that puts the centre at 1500 Hz audio.
    pub lower_hz: u64,
    pub centre_hz: u64,
    pub dial_hz: u64,
}

impl Lane {
    pub fn upper_hz(&self) -> u64 {
        self.lower_hz + LANE_WIDTH_HZ
    }
}

/// A part of the band with its limits, as the band plan states them (transmitted frequencies).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub lo_hz: u64,
    pub hi_hz: u64,
    pub max_bw_hz: u64,
    pub unattended_ok: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Excluded {
    pub lo_hz: u64,
    pub hi_hz: u64,
    pub why: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rules {
    /// Listen this long before a slot (ms).
    pub sense_ms: u32,
    /// Longest single transmission (s). A software watchdog cuts at this.
    pub max_tx_s: u32,
    pub duty_cycle_pct: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Band {
    pub name: String,
    pub segments: Vec<Segment>,
    pub lanes: Vec<Lane>,
    pub excluded: Vec<Excluded>,
    pub rules: Rules,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BandTable {
    pub version: u32,
    pub region: Region,
    pub valid_from: u64,
    /// True only after the table was compared with the current band plan of its region.
    pub plan_verified: bool,
    pub bands: Vec<Band>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Violation {
    NoSuchBand,
    OutsideLanes,
    TooWide,
    InExcluded(String),
    NotUnattended,
    PlanNotVerified,
}

/// Lane `k` of the 17 m plan: `lower = 18_111_200 + 700 k`, centre = lower + 240, dial = centre - 1500.
pub fn lane_17m(k: u8, role: Role) -> Lane {
    let lower = 18_111_200 + LANE_PITCH_HZ * k as u64;
    let centre = lower + LANE_WIDTH_HZ / 2;
    Lane {
        k,
        role,
        lower_hz: lower,
        centre_hz: centre,
        dial_hz: centre - AUDIO_CENTRE_HZ,
    }
}

/// Roles of lanes 0..12 (MESH-SPEC.md 2.1).
const ROLES_17M: [Role; 12] = [
    Role::Relay,
    Role::Relay,
    Role::Relay,
    Role::Relay,
    Role::Spare,
    Role::Calling,
    Role::Calling,
    Role::Spare,
    Role::Relay,
    Role::Relay,
    Role::Relay,
    Role::Relay,
];

impl BandTable {
    /// The built-in 17 m table for IARU Region 1 (2016 plan). **Not verified** against the
    /// newest plan: see the module note.
    pub fn builtin_r1() -> Self {
        let lanes = ROLES_17M
            .iter()
            .enumerate()
            .map(|(k, r)| lane_17m(k as u8, *r))
            .collect();
        BandTable {
            version: 1,
            region: Region::R1,
            valid_from: 0,
            plan_verified: false,
            bands: vec![Band {
                name: "17m".into(),
                segments: vec![
                    Segment {
                        lo_hz: 18_105_000,
                        hi_hz: 18_109_000,
                        max_bw_hz: 500,
                        unattended_ok: true,
                    },
                    Segment {
                        lo_hz: 18_111_000,
                        hi_hz: 18_120_000,
                        max_bw_hz: 2_700,
                        unattended_ok: true,
                    },
                ],
                lanes,
                excluded: vec![
                    ex(18_109_000, 18_111_000, "IBP beacons only"),
                    ex(18_100_000, 18_103_000, "FT8"),
                    ex(18_104_000, 18_107_000, "FT4"),
                    ex(18_106_000, 18_106_200, "WSPR"),
                ],
                rules: Rules {
                    sense_ms: 300,
                    max_tx_s: 20,
                    duty_cycle_pct: 25,
                },
            }],
        }
    }

    pub fn band(&self, name: &str) -> Option<&Band> {
        self.bands.iter().find(|b| b.name == name)
    }

    /// Canonical bytes: what is signed.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut o = b"KBT1".to_vec();
        o.extend(self.version.to_le_bytes());
        o.push(self.region as u8);
        o.extend(self.valid_from.to_le_bytes());
        o.push(self.plan_verified as u8);
        o.push(self.bands.len() as u8);
        for b in &self.bands {
            put_str(&mut o, &b.name);
            o.push(b.segments.len() as u8);
            for s in &b.segments {
                for v in [s.lo_hz, s.hi_hz, s.max_bw_hz] {
                    o.extend(v.to_le_bytes());
                }
                o.push(s.unattended_ok as u8);
            }
            o.push(b.lanes.len() as u8);
            for l in &b.lanes {
                o.push(l.k);
                o.push(l.role as u8);
                for v in [l.lower_hz, l.centre_hz, l.dial_hz] {
                    o.extend(v.to_le_bytes());
                }
            }
            o.push(b.excluded.len() as u8);
            for e in &b.excluded {
                o.extend(e.lo_hz.to_le_bytes());
                o.extend(e.hi_hz.to_le_bytes());
                put_str(&mut o, &e.why);
            }
            o.extend(b.rules.sense_ms.to_le_bytes());
            o.extend(b.rules.max_tx_s.to_le_bytes());
            o.push(b.rules.duty_cycle_pct);
        }
        o
    }

    /// Sign with the project key. The result is what the server publishes next to the table.
    pub fn sign(&self, project_key: &[u8; 32]) -> [u8; 64] {
        SigningKey::from_bytes(project_key)
            .sign(&self.to_bytes())
            .to_bytes()
    }
}

fn ex(lo_hz: u64, hi_hz: u64, why: &str) -> Excluded {
    Excluded {
        lo_hz,
        hi_hz,
        why: why.into(),
    }
}

fn put_str(o: &mut Vec<u8>, s: &str) {
    o.push(s.len() as u8);
    o.extend(s.as_bytes());
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    Unsigned,
    BadSignature,
    Older { have: u32, got: u32 },
}

/// Accept a table that arrived with a signature. It must verify against the project's public
/// key and must not be older than the one in use (`have`).
pub fn accept(
    table: &BandTable,
    sig: Option<&[u8; 64]>,
    project_public: &[u8; 32],
    have: u32,
) -> Result<(), Refusal> {
    let sig = sig.ok_or(Refusal::Unsigned)?;
    let vk = VerifyingKey::from_bytes(project_public).map_err(|_| Refusal::BadSignature)?;
    vk.verify(&table.to_bytes(), &Signature::from_bytes(sig))
        .map_err(|_| Refusal::BadSignature)?;
    if table.version < have {
        return Err(Refusal::Older {
            have,
            got: table.version,
        });
    }
    Ok(())
}

/// The span a USB signal occupies on the air: the dial plus the audio span (all Hz).
pub fn occupied_span(dial_hz: u64, audio_lo_hz: u64, audio_hi_hz: u64) -> (u64, u64) {
    (dial_hz + audio_lo_hz, dial_hz + audio_hi_hz)
}

impl Band {
    pub fn lane(&self, k: u8) -> Option<&Lane> {
        self.lanes.iter().find(|l| l.k == k)
    }

    pub fn lanes_with(&self, role: Role) -> impl Iterator<Item = &Lane> {
        self.lanes.iter().filter(move |l| l.role == role)
    }

    /// Dial that puts the lowest of `ks` at 1500 Hz audio, for a receiver that listens to
    /// several lanes at once. Every lane must then fall inside `audio_max_hz` of audio.
    pub fn receive_dial(&self, ks: &[u8], audio_max_hz: u64) -> Option<u64> {
        let lanes: Vec<&Lane> = ks.iter().map(|k| self.lane(*k)).collect::<Option<_>>()?;
        let low = lanes.iter().map(|l| l.centre_hz).min()?;
        let dial = low.checked_sub(AUDIO_CENTRE_HZ)?;
        lanes
            .iter()
            .all(|l| l.upper_hz() - dial <= audio_max_hz)
            .then_some(dial)
    }

    /// Geometry: may a signal occupying `lo..hi` (transmitted Hz) go out? It must lie inside
    /// one run of adjacent lanes, inside one unattended segment, within that segment's width
    /// limit, and clear of every excluded range.
    pub fn check_span(&self, lo: u64, hi: u64) -> Result<(), Violation> {
        if hi < lo {
            return Err(Violation::OutsideLanes);
        }
        for e in &self.excluded {
            if lo < e.hi_hz && hi > e.lo_hz {
                return Err(Violation::InExcluded(e.why.clone()));
            }
        }
        let first = self.lanes.iter().find(|l| lo >= l.lower_hz && lo <= l.upper_hz());
        let Some(first) = first else {
            return Err(Violation::OutsideLanes);
        };
        // Walk adjacent lanes until the span is covered, the gaps between lanes count as covered.
        let mut top = first.upper_hz();
        while hi > top {
            let next = self
                .lanes
                .iter()
                .find(|l| l.lower_hz > top && l.lower_hz - top <= LANE_PITCH_HZ - LANE_WIDTH_HZ);
            let Some(next) = next else {
                return Err(Violation::OutsideLanes);
            };
            top = next.upper_hz();
        }
        let seg = self
            .segments
            .iter()
            .find(|s| lo >= s.lo_hz && hi <= s.hi_hz)
            .ok_or(Violation::OutsideLanes)?;
        if !seg.unattended_ok {
            return Err(Violation::NotUnattended);
        }
        if hi - lo > seg.max_bw_hz {
            return Err(Violation::TooWide);
        }
        Ok(())
    }
}

impl BandTable {
    /// The check every transmission must pass. `first_k..=last_k` are the lanes the plan gave
    /// this transmission. The span comes from the dial the radio is really on and the audio span
    /// the modem really fills, so a wrong dial (the dial trap) is refused even when it lands
    /// inside some other valid lane. The table must also be verified.
    pub fn permit_tx(
        &self,
        band: &str,
        first_k: u8,
        last_k: u8,
        dial_hz: u64,
        audio_lo_hz: u64,
        audio_hi_hz: u64,
    ) -> Result<(), Violation> {
        let b = self.band(band).ok_or(Violation::NoSuchBand)?;
        let (lo, hi) = occupied_span(dial_hz, audio_lo_hz, audio_hi_hz);
        let (a, z) = (
            b.lane(first_k).ok_or(Violation::OutsideLanes)?,
            b.lane(last_k).ok_or(Violation::OutsideLanes)?,
        );
        if lo < a.lower_hz || hi > z.upper_hz() {
            return Err(Violation::OutsideLanes);
        }
        b.check_span(lo, hi)?;
        if !self.plan_verified {
            return Err(Violation::PlanNotVerified);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b17() -> Band {
        BandTable::builtin_r1().bands.remove(0)
    }

    /// Every row of the table in MESH-SPEC.md 2.2: (k, lower, centre, upper, dial) in Hz.
    const TABLE: [(u8, u64, u64, u64, u64); 12] = [
        (0, 18_111_200, 18_111_440, 18_111_680, 18_109_940),
        (1, 18_111_900, 18_112_140, 18_112_380, 18_110_640),
        (2, 18_112_600, 18_112_840, 18_113_080, 18_111_340),
        (3, 18_113_300, 18_113_540, 18_113_780, 18_112_040),
        (4, 18_114_000, 18_114_240, 18_114_480, 18_112_740),
        (5, 18_114_700, 18_114_940, 18_115_180, 18_113_440),
        (6, 18_115_400, 18_115_640, 18_115_880, 18_114_140),
        (7, 18_116_100, 18_116_340, 18_116_580, 18_114_840),
        (8, 18_116_800, 18_117_040, 18_117_280, 18_115_540),
        (9, 18_117_500, 18_117_740, 18_117_980, 18_116_240),
        (10, 18_118_200, 18_118_440, 18_118_680, 18_116_940),
        (11, 18_118_900, 18_119_140, 18_119_380, 18_117_640),
    ];

    #[test]
    fn every_lane_matches_the_spec_table() {
        let b = b17();
        assert_eq!(b.lanes.len(), 12);
        for (k, lo, c, hi, dial) in TABLE {
            let l = b.lane(k).unwrap();
            assert_eq!(
                (l.lower_hz, l.centre_hz, l.upper_hz(), l.dial_hz),
                (lo, c, hi, dial),
                "lane {k}"
            );
        }
    }

    #[test]
    fn roles_are_eight_relay_two_calling_two_spare() {
        let b = b17();
        assert_eq!(b.lanes_with(Role::Relay).count(), 8);
        assert_eq!(
            b.lanes_with(Role::Calling).map(|l| l.k).collect::<Vec<_>>(),
            vec![5, 6]
        );
        assert_eq!(
            b.lanes_with(Role::Spare).map(|l| l.k).collect::<Vec<_>>(),
            vec![4, 7]
        );
    }

    #[test]
    fn lanes_stay_inside_the_data_segment_and_clear_of_the_beacons() {
        let b = b17();
        for l in &b.lanes {
            assert!(
                l.lower_hz >= 18_111_200 && l.upper_hz() <= 18_119_380,
                "lane {}",
                l.k
            );
            assert_eq!(b.check_span(l.lower_hz, l.upper_hz()), Ok(()), "lane {}", l.k);
        }
        // 620 Hz clear below 18.120, 1.2 kHz above the beacon segment.
        assert_eq!(18_120_000 - b.lane(11).unwrap().upper_hz(), 620);
        assert_eq!(b.lane(0).unwrap().lower_hz - 18_111_000, 200);
    }

    #[test]
    fn a_signal_on_the_lane_dial_is_inside_its_lane() {
        let t = BandTable::builtin_r1();
        for l in &t.bands[0].lanes {
            // 1500 Hz audio centre, 480 Hz wide: 1260..1740.
            let (lo, hi) = occupied_span(l.dial_hz, 1260, 1740);
            assert_eq!(t.bands[0].check_span(lo, hi), Ok(()), "lane {}", l.k);
        }
    }

    #[test]
    fn the_dial_trap_is_refused() {
        // Tuning the radio's dial to the lane centre puts the signal 1.5 kHz too high.
        let b = b17();
        let l = b.lane(5).unwrap();
        let (lo, hi) = occupied_span(l.centre_hz, 1260, 1740);
        // Geometry alone cannot see this one: lane 5's centre as the dial lands inside lane 7
        // (a spare). It is `permit_tx`, which knows the intended lane, that refuses it.
        assert_eq!(b.check_span(lo, hi), Ok(()));
    }

    #[test]
    fn excluded_ranges_are_refused() {
        let b = b17();
        assert!(matches!(
            b.check_span(18_109_500, 18_109_900),
            Err(Violation::InExcluded(_))
        ));
        assert!(matches!(
            b.check_span(18_100_500, 18_100_900),
            Err(Violation::InExcluded(_))
        ));
        // The narrow-digimode segment below the beacons is not a lane (and holds FT4/WSPR).
        assert!(b.check_span(18_105_000, 18_105_300).is_err());
        assert_eq!(b.check_span(18_120_100, 18_120_500), Err(Violation::OutsideLanes));
    }

    #[test]
    fn bonded_lanes_are_allowed_up_to_the_width_cap() {
        let b = b17();
        let low = b.lane(0).unwrap().lower_hz;
        // Four lanes: 4 x 480 + 3 x 220 = 2580 Hz, within the 2700 Hz cap.
        assert_eq!(b.check_span(low, low + 2580), Ok(()));
        // Five lanes would be 3280 Hz.
        assert_eq!(b.check_span(low, low + 3280), Err(Violation::TooWide));
        // Wider than the lane run is refused as outside.
        assert_eq!(b.check_span(low, low + 9_000), Err(Violation::OutsideLanes));
    }

    #[test]
    fn a_two_lane_receiver_puts_c1_at_1500_and_c2_at_2200() {
        let b = b17();
        let dial = b.receive_dial(&[5, 6], 2_700).unwrap();
        assert_eq!(dial, b.lane(5).unwrap().dial_hz);
        assert_eq!(b.lane(5).unwrap().centre_hz - dial, 1500);
        assert_eq!(b.lane(6).unwrap().centre_hz - dial, 2200);
        // Lanes too far apart for one 2.7 kHz capture.
        assert_eq!(b.receive_dial(&[0, 11], 2_700), None);
    }

    #[test]
    fn the_built_in_table_may_not_transmit_until_it_is_verified() {
        let mut t = BandTable::builtin_r1();
        let d = t.bands[0].lane(2).unwrap().dial_hz;
        assert_eq!(
            t.permit_tx("17m", 2, 2, d, 1260, 1740),
            Err(Violation::PlanNotVerified)
        );
        assert_eq!(
            t.permit_tx("20m", 2, 2, d, 1260, 1740),
            Err(Violation::NoSuchBand)
        );
        t.plan_verified = true;
        assert_eq!(t.permit_tx("17m", 2, 2, d, 1260, 1740), Ok(()));
        // The dial trap: the dial on the centre puts the signal in lane 4, not lane 2.
        assert_eq!(
            t.permit_tx("17m", 2, 2, d + 1500, 1260, 1740),
            Err(Violation::OutsideLanes)
        );
        // The right dial, the wrong lane asked for.
        assert_eq!(
            t.permit_tx("17m", 3, 3, d, 1260, 1740),
            Err(Violation::OutsideLanes)
        );
        // Two bonded lanes: 1180 Hz wide (1500 - 590 .. 1500 + 590 audio) on lanes 2 and 3.
        let d2 = t.bands[0].lane(2).unwrap().lower_hz - 1000;
        assert_eq!(t.permit_tx("17m", 2, 3, d2, 1000, 2180), Ok(()));
        assert_eq!(
            t.permit_tx("17m", 2, 2, d2, 1000, 2180),
            Err(Violation::OutsideLanes)
        );
    }

    #[test]
    fn unsigned_badly_signed_and_older_tables_are_refused() {
        let secret = [7u8; 32];
        let public = SigningKey::from_bytes(&secret).verifying_key().to_bytes();
        let mut t = BandTable::builtin_r1();
        t.version = 5;
        let sig = t.sign(&secret);
        assert_eq!(accept(&t, Some(&sig), &public, 5), Ok(()));
        assert_eq!(accept(&t, Some(&sig), &public, 4), Ok(()));
        assert_eq!(accept(&t, None, &public, 0), Err(Refusal::Unsigned));
        assert_eq!(
            accept(&t, Some(&sig), &public, 6),
            Err(Refusal::Older { have: 6, got: 5 })
        );
        // One changed number breaks the signature.
        let mut forged = t.clone();
        forged.bands[0].segments[1].max_bw_hz = 5_000;
        assert_eq!(
            accept(&forged, Some(&sig), &public, 0),
            Err(Refusal::BadSignature)
        );
        // A signature by another key.
        let other = SigningKey::from_bytes(&[9u8; 32]).verifying_key().to_bytes();
        assert_eq!(accept(&t, Some(&sig), &other, 0), Err(Refusal::BadSignature));
    }
}
