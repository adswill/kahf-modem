//! Transmit channel selection from the three channel pools (plan section 2.2 and 2.3).
//!
//! Pools: Online (reserved for server-connected stations), Open (everyone else),
//! Emergency (never reserved, urgent traffic only). The grid is split into thirds
//! by channel index: lower third Online, middle third Open, top third Emergency.
//! This split is a default; the server may change the plan.

use crate::bandplan;

/// Channel spacing in Hz. The default grid uses only channels that lie wholly inside a
/// KAHF segment (bandplan::KAHF_SEGMENTS, IARU Region 1): 40 m 7.040 to 7.045 MHz,
/// 20 m 14.101 to 14.112 MHz and the 17 m relay segment 18.111 to 18.120 MHz. That gives
/// eight channels. 14.070 to 14.100 MHz (beacons),
/// 14.230 MHz (SSTV and FAX) and 30 m are left out on purpose.
pub const DEFAULT_SPACING_HZ: f32 = 2_700.0;

/// Failed attempts allowed before the caller is told to give up.
pub const MAX_ATTEMPTS: u32 = 5;

/// Channel pool a station transmits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pool {
    Online,
    Open,
    Emergency,
}

/// Channel plan: every channel centre frequency in Hz with its pool.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub channels: Vec<(f32, Pool)>,
}

/// Why no channel was chosen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Refusal {
    /// Try again after `seconds` (random, 1 to 10 s).
    Wait { seconds: f32 },
    /// Five attempts failed. Tell the user.
    GiveUp,
}

impl Plan {
    /// Build a grid of channels from `start_hz` to `stop_hz` (inclusive) every `spacing_hz`.
    /// Pools are assigned by index into thirds.
    pub fn grid(start_hz: f32, stop_hz: f32, spacing_hz: f32) -> Plan {
        let mut freqs = Vec::new();
        if spacing_hz > 0.0 && stop_hz >= start_hz {
            let n = ((stop_hz - start_hz) / spacing_hz).floor() as usize + 1;
            for i in 0..n {
                freqs.push(start_hz + spacing_hz * i as f32);
            }
        }
        Plan::from_freqs(freqs)
    }

    /// Build a plan from centre frequencies in list order. Pools are assigned by thirds:
    /// lower third Online, middle third Open, top third Emergency.
    pub fn from_freqs(freqs: Vec<f32>) -> Plan {
        let n = freqs.len();
        let channels = freqs
            .into_iter()
            .enumerate()
            .map(|(i, f)| {
                let pool = if 3 * i < n {
                    Pool::Online
                } else if 3 * i < 2 * n {
                    Pool::Open
                } else {
                    Pool::Emergency
                };
                (f, pool)
            })
            .collect();
        Plan { channels }
    }

    /// The default plan: 2.7 kHz channels, each wholly inside a KAHF segment (see above).
    pub fn default_plan() -> Plan {
        let step = DEFAULT_SPACING_HZ as u64;
        let half = step / 2;
        let mut freqs = Vec::new();
        for seg in bandplan::KAHF_SEGMENTS {
            let mut c = seg.lower_hz + half;
            while c + (step - half) <= seg.upper_hz {
                freqs.push(c as f32);
                c += step;
            }
        }
        Plan::from_freqs(freqs)
    }
}

impl Default for Plan {
    fn default() -> Self {
        Plan::default_plan()
    }
}

/// splitmix64: small deterministic generator, no extra dependency.
fn next_u64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn unit(state: &mut u64) -> f32 {
    // Top 24 bits give a value in [0, 1).
    (next_u64(state) >> 40) as f32 / (1u64 << 24) as f32
}

/// Source of random numbers in [0, 1). Production code uses `SplitMix`; tests pass a fixed list.
pub trait RandomSource {
    fn next_unit(&mut self) -> f32;
}

/// Seeded generator. The same seed always gives the same sequence.
#[derive(Debug, Clone)]
pub struct SplitMix(pub u64);

impl RandomSource for SplitMix {
    fn next_unit(&mut self) -> f32 {
        unit(&mut self.0)
    }
}

/// Random wait in [1, 10) seconds. The upper value is capped so f32 rounding cannot reach 10.
fn random_wait(rng: &mut dyn RandomSource) -> Refusal {
    let seconds = (1.0 + rng.next_unit() * 9.0).min(9.99);
    Refusal::Wait { seconds }
}

/// Choose a transmit channel in `pool`.
///
/// If `preferred` is a channel of this pool, it is tried first. When it is busy, the next
/// free channel of the same pool in plan order is used (wrapping at the end). When there is
/// no preferred channel in the pool, a free channel is picked at random from the free ones.
///
/// `failed_attempts` is how many earlier attempts already failed. At `MAX_ATTEMPTS`
/// or more, the result is `Err(Refusal::GiveUp)`. If no channel of the pool is free,
/// the result is `Err(Refusal::Wait { seconds })` with a random value in [1, 10).
pub fn choose_in_pool(
    plan: &Plan,
    pool: Pool,
    preferred: Option<f32>,
    busy: &dyn Fn(f32) -> bool,
    rng: &mut dyn RandomSource,
    failed_attempts: u32,
) -> Result<f32, Refusal> {
    if failed_attempts >= MAX_ATTEMPTS {
        return Err(Refusal::GiveUp);
    }
    let members: Vec<f32> = plan
        .channels
        .iter()
        .filter(|(_, p)| *p == pool)
        .map(|(f, _)| *f)
        .collect();
    if let Some(p) = preferred {
        if let Some(k) = members.iter().position(|f| *f == p) {
            for step in 0..members.len() {
                let f = members[(k + step) % members.len()];
                if !busy(f) {
                    return Ok(f);
                }
            }
            return Err(random_wait(rng));
        }
    }
    let free: Vec<f32> = members.iter().copied().filter(|f| !busy(*f)).collect();
    if free.is_empty() {
        return Err(random_wait(rng));
    }
    let idx = ((rng.next_unit() * free.len() as f32) as usize).min(free.len() - 1);
    Ok(free[idx])
}

/// Seeded form of `choose_in_pool` with no preferred channel.
pub fn choose(
    plan: &Plan,
    pool: Pool,
    busy: &dyn Fn(f32) -> bool,
    rng_seed: u64,
    failed_attempts: u32,
) -> Result<f32, Refusal> {
    choose_in_pool(plan, pool, None, busy, &mut SplitMix(rng_seed), failed_attempts)
}

/// Urgent traffic ignores the reservations. It tries Emergency first, then Open, then Online.
///
/// If no pool has a free channel, the Emergency wait is returned.
pub fn choose_urgent(
    plan: &Plan,
    busy: &dyn Fn(f32) -> bool,
    rng_seed: u64,
    failed_attempts: u32,
) -> Result<f32, Refusal> {
    if failed_attempts >= MAX_ATTEMPTS {
        return Err(Refusal::GiveUp);
    }
    let mut rng = SplitMix(rng_seed);
    let mut first_wait = None;
    for pool in [Pool::Emergency, Pool::Open, Pool::Online] {
        match choose_in_pool(plan, pool, None, busy, &mut rng, failed_attempts) {
            Ok(f) => return Ok(f),
            Err(Refusal::GiveUp) => return Err(Refusal::GiveUp),
            Err(wait) => {
                first_wait.get_or_insert(wait);
            }
        }
    }
    Err(first_wait.unwrap_or(Refusal::GiveUp))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bandplan::{self, Region};

    /// First channel centre of the test grid, in Hz (inside the 20 m KAHF segment).
    const T0: f32 = 14_102_350.0;

    fn small_plan() -> Plan {
        // 9 channels: 3 Online, 3 Open, 3 Emergency, from the default grid start.
        Plan::grid(T0, T0 + 8.0 * 2700.0, 2700.0)
    }

    #[test]
    fn default_grid_is_the_eight_kahf_channels() {
        let plan = Plan::default_plan();
        let freqs: Vec<f32> = plan.channels.iter().map(|c| c.0).collect();
        // 40 m: one channel (7 041 350 Hz). 20 m: four from 14 102 350 Hz. 17 m: three
        // from 18 112 350 Hz.
        assert_eq!(
            freqs,
            vec![
                7_041_350.0,
                14_102_350.0,
                14_105_050.0,
                14_107_750.0,
                14_110_450.0,
                18_112_350.0,
                18_115_050.0,
                18_117_750.0
            ]
        );
    }

    #[test]
    fn every_default_channel_fits_a_kahf_segment() {
        let plan = Plan::default_plan();
        for (f, _) in &plan.channels {
            let c = *f as u64;
            assert!(
                bandplan::allowed_segment(c, bandplan::KAHF_CHANNEL_WIDTH_HZ).is_some(),
                "channel {} Hz is outside every KAHF segment",
                c
            );
        }
    }

    #[test]
    fn default_grid_avoids_beacons_sstv_and_30m() {
        let plan = Plan::default_plan();
        for (f, _) in &plan.channels {
            let f = *f as u64;
            assert!(!(14_070_000..=14_100_000).contains(&f), "beacon segment used");
            assert!(!(14_225_000..=14_235_000).contains(&f), "SSTV/FAX calling used");
            assert!(!(10_100_000..=10_150_000).contains(&f), "30 m used");
        }
    }

    #[test]
    fn default_segments_lie_inside_region1_allocations() {
        for seg in bandplan::KAHF_SEGMENTS {
            let inside = bandplan::ALLOCATIONS.iter().any(|a| {
                a.region == Region::Region1 && seg.lower_hz >= a.lower_hz && seg.upper_hz <= a.upper_hz
            });
            assert!(inside, "{} is not inside a Region 1 allocation", seg.name);
        }
    }

    fn pool_of(plan: &Plan, f: f32) -> Pool {
        plan.channels.iter().find(|(c, _)| *c == f).unwrap().1
    }

    #[test]
    fn default_split_is_thirds() {
        let plan = Plan::default_plan();
        let n = plan.channels.len();
        assert!(n > 3);
        assert_eq!(plan.channels[0].1, Pool::Online);
        assert_eq!(plan.channels[n - 1].1, Pool::Emergency);
        let online = plan.channels.iter().filter(|c| c.1 == Pool::Online).count();
        let open = plan.channels.iter().filter(|c| c.1 == Pool::Open).count();
        assert!((online as i64 - open as i64).abs() <= 1);
    }

    #[test]
    fn busy_channel_is_skipped() {
        let plan = small_plan();
        let all_but_one = |f: f32| f != T0 + 2.0 * 2700.0;
        for seed in 0..50u64 {
            let got = choose(&plan, Pool::Online, &all_but_one, seed, 0).unwrap();
            assert_eq!(got, T0 + 2.0 * 2700.0);
        }
        let none = |_: f32| false;
        assert!(choose(&plan, Pool::Open, &none, 7, 0).is_ok());
    }

    #[test]
    fn pool_is_respected() {
        let plan = small_plan();
        let free = |_: f32| false;
        for seed in 0..200u64 {
            for pool in [Pool::Online, Pool::Open, Pool::Emergency] {
                let f = choose(&plan, pool, &free, seed, 0).unwrap();
                assert_eq!(pool_of(&plan, f), pool);
            }
        }
    }

    #[test]
    fn wait_is_between_1_and_10_seconds() {
        let plan = small_plan();
        let busy = |_: f32| true;
        for seed in 0..500u64 {
            match choose(&plan, Pool::Open, &busy, seed, 0) {
                Err(Refusal::Wait { seconds }) => assert!((1.0..10.0).contains(&seconds)),
                other => panic!("expected wait, got {:?}", other),
            }
        }
    }

    #[test]
    fn gives_up_after_five_attempts() {
        let plan = small_plan();
        let busy = |_: f32| true;
        assert!(matches!(
            choose(&plan, Pool::Open, &busy, 1, MAX_ATTEMPTS - 1),
            Err(Refusal::Wait { .. })
        ));
        assert_eq!(
            choose(&plan, Pool::Open, &busy, 1, MAX_ATTEMPTS),
            Err(Refusal::GiveUp)
        );
    }

    #[test]
    fn urgent_uses_emergency_then_falls_back_to_open() {
        let plan = small_plan();
        let none = |_: f32| false;
        for seed in 0..100u64 {
            let f = choose_urgent(&plan, &none, seed, 0).unwrap();
            assert_eq!(pool_of(&plan, f), Pool::Emergency);
        }
        // Every Emergency channel busy: fall back to Open.
        let emergency_busy = |f: f32| pool_of(&small_plan(), f) == Pool::Emergency;
        for seed in 0..100u64 {
            let f = choose_urgent(&plan, &emergency_busy, seed, 0).unwrap();
            assert_eq!(pool_of(&plan, f), Pool::Open);
        }
        // Everything busy: wait.
        let all = |_: f32| true;
        assert!(matches!(
            choose_urgent(&plan, &all, 3, 0),
            Err(Refusal::Wait { .. })
        ));
    }

    /// Scripted random values, so tests do not depend on the generator.
    struct Script(Vec<f32>, usize);

    impl RandomSource for Script {
        fn next_unit(&mut self) -> f32 {
            let v = self.0[self.1 % self.0.len()];
            self.1 += 1;
            v
        }
    }

    #[test]
    fn preferred_channel_is_used_when_free() {
        let plan = small_plan();
        let none = |_: f32| false;
        let mut rng = Script(vec![0.0], 0);
        let want = T0 + 4.0 * 2700.0; // an Open channel in small_plan
        assert_eq!(
            choose_in_pool(&plan, Pool::Open, Some(want), &none, &mut rng, 0),
            Ok(want)
        );
    }

    #[test]
    fn busy_preferred_moves_to_next_free_in_same_pool() {
        let plan = small_plan();
        let open: Vec<f32> = plan
            .channels
            .iter()
            .filter(|c| c.1 == Pool::Open)
            .map(|c| c.0)
            .collect();
        // Preferred is the first Open channel and it is busy; the second must be chosen.
        let busy = |f: f32| f == open[0];
        let mut rng = Script(vec![0.0], 0);
        assert_eq!(
            choose_in_pool(&plan, Pool::Open, Some(open[0]), &busy, &mut rng, 0),
            Ok(open[1])
        );
        // Last Open channel busy and preferred: wrap to the first one.
        let busy_last = |f: f32| f == open[2];
        assert_eq!(
            choose_in_pool(&plan, Pool::Open, Some(open[2]), &busy_last, &mut rng, 0),
            Ok(open[0])
        );
    }

    #[test]
    fn preferred_from_another_pool_is_ignored() {
        let plan = small_plan();
        let none = |_: f32| false;
        let mut rng = Script(vec![0.0], 0);
        let online = T0; // Online channel, asked for in the Open pool
        let got = choose_in_pool(&plan, Pool::Open, Some(online), &none, &mut rng, 0).unwrap();
        assert_eq!(pool_of(&plan, got), Pool::Open);
    }

    #[test]
    fn scripted_random_picks_the_expected_free_channel() {
        let plan = small_plan();
        let busy = |f: f32| pool_of(&small_plan(), f) == Pool::Online;
        // Free Open channels: three of them. 0.99 maps to the last one.
        let mut rng = Script(vec![0.99], 0);
        let got = choose_in_pool(&plan, Pool::Open, None, &busy, &mut rng, 0).unwrap();
        let open: Vec<f32> = plan
            .channels
            .iter()
            .filter(|c| c.1 == Pool::Open)
            .map(|c| c.0)
            .collect();
        assert_eq!(got, open[2]);
    }

    #[test]
    fn urgent_ignores_online_reservation_when_others_are_busy() {
        let plan = small_plan();
        // Emergency and Open all busy: urgent must still use an Online channel.
        let only_online_free = |f: f32| pool_of(&small_plan(), f) != Pool::Online;
        let got = choose_urgent(&plan, &only_online_free, 9, 0).unwrap();
        assert_eq!(pool_of(&plan, got), Pool::Online);
    }

    #[test]
    fn wait_is_bounded_over_a_full_retry_sequence() {
        let plan = small_plan();
        let all = |_: f32| true;
        let mut total = 0.0f32;
        let mut waits = 0u32;
        let mut result = None;
        for attempt in 0..=MAX_ATTEMPTS {
            match choose_in_pool(
                &plan,
                Pool::Open,
                None,
                &all,
                &mut SplitMix(attempt as u64 + 1),
                attempt,
            ) {
                Err(Refusal::Wait { seconds }) => {
                    assert!((1.0..10.0).contains(&seconds));
                    total += seconds;
                    waits += 1;
                }
                other => {
                    result = Some(other);
                    break;
                }
            }
        }
        assert_eq!(waits, MAX_ATTEMPTS);
        assert!(total < MAX_ATTEMPTS as f32 * 10.0);
        assert_eq!(result, Some(Err(Refusal::GiveUp)));
    }

    #[test]
    fn emergency_is_never_refused_while_a_channel_is_free() {
        let plan = small_plan();
        // Only one Emergency channel free: urgent always finds it, for many seeds.
        let last_em = plan
            .channels
            .iter()
            .rev()
            .find(|c| c.1 == Pool::Emergency)
            .unwrap()
            .0;
        let busy = |f: f32| f != last_em;
        for seed in 0..200u64 {
            assert_eq!(choose_urgent(&plan, &busy, seed, 0), Ok(last_em));
        }
    }
}
