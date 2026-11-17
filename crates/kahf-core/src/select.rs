//! Transmit channel selection from the three channel pools (plan section 2.2 and 2.3).
//!
//! Pools: Online (reserved for server-connected stations), Open (everyone else),
//! Emergency (never reserved, urgent traffic only). The grid is split into thirds
//! by channel index: lower third Online, middle third Open, top third Emergency.
//! This split is a default; the server may change the plan.

/// Default channel grid, all in Hz: the 20 m amateur allocation 14.000 to 14.350 MHz
/// (bandplan.rs, 20 m, Region 1, 2 and 3 alike), in 2.7 kHz steps.
/// Channel centres run from 14_000_000 Hz to 14_348_300 Hz: 130 channels.
pub const DEFAULT_START_HZ: f32 = 14_000_000.0;
pub const DEFAULT_STOP_HZ: f32 = 14_350_000.0;
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

    /// The default plan (see the constants above).
    pub fn default_plan() -> Plan {
        Plan::grid(DEFAULT_START_HZ, DEFAULT_STOP_HZ, DEFAULT_SPACING_HZ)
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

/// Choose the first free channel of `pool`, starting at a random position in that pool.
///
/// `failed_attempts` is how many earlier attempts already failed. At `MAX_ATTEMPTS`
/// or more, the result is `Err(Refusal::GiveUp)`. If no channel of the pool is free,
/// the result is `Err(Refusal::Wait { seconds })` with a random value in [1, 10).
pub fn choose(
    plan: &Plan,
    pool: Pool,
    busy: &dyn Fn(f32) -> bool,
    rng_seed: u64,
    failed_attempts: u32,
) -> Result<f32, Refusal> {
    if failed_attempts >= MAX_ATTEMPTS {
        return Err(Refusal::GiveUp);
    }
    let mut state = rng_seed;
    let members: Vec<f32> = plan
        .channels
        .iter()
        .filter(|(_, p)| *p == pool)
        .map(|(f, _)| *f)
        .collect();
    if !members.is_empty() {
        let start = (next_u64(&mut state) % members.len() as u64) as usize;
        for k in 0..members.len() {
            let f = members[(start + k) % members.len()];
            if !busy(f) {
                return Ok(f);
            }
        }
    }
    let seconds = 1.0 + unit(&mut state) * 9.0;
    Err(Refusal::Wait { seconds })
}

/// Urgent traffic: try Emergency first, then fall back to Open.
///
/// If neither pool has a free channel, the Emergency wait is returned.
pub fn choose_urgent(
    plan: &Plan,
    busy: &dyn Fn(f32) -> bool,
    rng_seed: u64,
    failed_attempts: u32,
) -> Result<f32, Refusal> {
    match choose(plan, Pool::Emergency, busy, rng_seed, failed_attempts) {
        Ok(f) => Ok(f),
        Err(Refusal::GiveUp) => Err(Refusal::GiveUp),
        Err(wait @ Refusal::Wait { .. }) => {
            match choose(plan, Pool::Open, busy, rng_seed ^ 0xA5A5_A5A5, failed_attempts) {
                Ok(f) => Ok(f),
                Err(_) => Err(wait),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bandplan::{self, Region};

    /// First channel centre of the default grid, in Hz.
    const T0: f32 = DEFAULT_START_HZ;

    fn small_plan() -> Plan {
        // 9 channels: 3 Online, 3 Open, 3 Emergency, from the default grid start.
        Plan::grid(T0, T0 + 8.0 * 2700.0, 2700.0)
    }

    #[test]
    fn default_grid_is_20m_in_hz() {
        let plan = Plan::default_plan();
        assert_eq!(plan.channels.len(), 130);
        assert_eq!(plan.channels[0].0, 14_000_000.0);
        assert_eq!(plan.channels[129].0, 14_000_000.0 + 129.0 * 2700.0);
        assert_eq!(plan.channels[129].0, 14_348_300.0);
        // Every channel lies inside the 20 m allocation.
        for (f, _) in &plan.channels {
            assert!((14_000_000.0..=14_350_000.0).contains(f));
        }
    }

    #[test]
    fn default_grid_matches_bandplan_20m() {
        let a = bandplan::ALLOCATIONS
            .iter()
            .find(|a| a.band == "20m" && a.region == Region::Region1)
            .expect("20m allocation in table");
        assert_eq!(DEFAULT_START_HZ as u64, a.lower_hz);
        assert_eq!(DEFAULT_STOP_HZ as u64, a.upper_hz);
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
}
