//! Pure simulation of a half-duplex relay network with a central planner.
//!
//! Model:
//! - Relays sit at random positions on a `SIDE` x `SIDE` plane. Two relays
//!   hear each other when they are within `radius`.
//! - Time runs in 1 s slots. Each channel offers one transmit opportunity
//!   per slot. `cycle_s` is the channel's repeating cycle and sets the
//!   latency budget (`BUDGET_CYCLES` cycles per message).
//! - Relays are half-duplex: in a slot a relay transmits or receives, never
//!   both, on any channel.
//! - Two transmitters that hear each other never share a slot on one channel.
//!   A hop is also refused when its receiver hears another transmitter active
//!   on that channel in the same slot.
//! - Messages arrive as a Poisson process. Each gets a shortest hop path
//!   (BFS). The planner books its hops in order at arrival, each hop in the
//!   earliest feasible slot. A message that cannot finish inside its budget is
//!   dropped and its partial bookings are released.
//!
//! No network or radio code: this only counts slots.

use std::collections::VecDeque;

/// Side length of the square plane the relays are placed on.
const SIDE: f64 = 100.0;
/// Latency budget per message, in channel cycles.
const BUDGET_CYCLES: u64 = 4;

#[derive(Debug, Clone)]
pub struct SimConfig {
    pub relays: usize,
    pub channels: usize,
    pub cycle_s: u64,
    pub radius: f64,
    pub arrival_per_min: f64,
    pub duration_s: u64,
    pub seed: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SimResult {
    pub delivered: usize,
    pub dropped: usize,
    pub mean_latency_s: f64,
    pub p95_latency_s: f64,
    pub mean_hops: f64,
    /// Channel candidates the planner rejected to keep interfering relays apart.
    pub collisions_avoided: usize,
}

/// Simulation entry point. Deterministic for a given config.
pub fn run(cfg: &SimConfig) -> SimResult {
    simulate(cfg).result
}

/// Xorshift64 generator.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed })
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniform in [0, 1).
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

/// Reservations, indexed by absolute second.
struct Schedule {
    channels: usize,
    /// (relay, channel) pairs transmitting in each second.
    tx: Vec<Vec<(usize, usize)>>,
    /// Relays receiving in each second.
    rx: Vec<Vec<usize>>,
}

impl Schedule {
    fn new(horizon: usize, channels: usize) -> Self {
        Schedule {
            channels,
            tx: vec![Vec::new(); horizon],
            rx: vec![Vec::new(); horizon],
        }
    }

    /// First channel on which hop `a -> b` can transmit in second `t`.
    /// `rejections` counts channels refused only because an interfering
    /// transmitter already holds them.
    fn free_channel(
        &self,
        adj: &[bool],
        n: usize,
        t: usize,
        a: usize,
        b: usize,
        rejections: &mut usize,
    ) -> Option<usize> {
        let txs = &self.tx[t];
        let rxs = &self.rx[t];
        let busy = |r: usize| rxs.contains(&r) || txs.iter().any(|&(x, _)| x == r);
        if busy(a) || busy(b) {
            return None;
        }
        for c in 0..self.channels {
            let clash = txs
                .iter()
                .any(|&(x, ch)| ch == c && (adj[a * n + x] || adj[b * n + x]));
            if !clash {
                return Some(c);
            }
            *rejections += 1;
        }
        None
    }
}

/// Breadth-first shortest hop path from `src` to `dst`, endpoints included.
fn shortest_path(adj: &[bool], n: usize, src: usize, dst: usize) -> Option<Vec<usize>> {
    let mut prev = vec![usize::MAX; n];
    let mut seen = vec![false; n];
    let mut queue = VecDeque::new();
    seen[src] = true;
    queue.push_back(src);
    while let Some(u) = queue.pop_front() {
        if u == dst {
            break;
        }
        for v in 0..n {
            if adj[u * n + v] && !seen[v] {
                seen[v] = true;
                prev[v] = u;
                queue.push_back(v);
            }
        }
    }
    if !seen[dst] {
        return None;
    }
    let mut path = vec![dst];
    while *path.last().unwrap() != src {
        let p = prev[*path.last().unwrap()];
        path.push(p);
    }
    path.reverse();
    Some(path)
}

/// Book every hop of `path` in order. Returns the completion second, or
/// `None` (with all partial bookings released) if the deadline cannot be met.
fn place(
    sched: &mut Schedule,
    adj: &[bool],
    n: usize,
    path: &[usize],
    arrival: u64,
    deadline: u64,
    rejections: &mut usize,
) -> Option<u64> {
    let mut ready = arrival;
    // (second, channel, transmitter, receiver)
    let mut booked: Vec<(usize, usize, usize, usize)> = Vec::new();
    for hop in path.windows(2) {
        let (a, b) = (hop[0], hop[1]);
        let mut found = None;
        for t in ready..deadline {
            if let Some(c) = sched.free_channel(adj, n, t as usize, a, b, rejections) {
                found = Some((t as usize, c));
                break;
            }
        }
        match found {
            Some((t, c)) => {
                sched.tx[t].push((a, c));
                sched.rx[t].push(b);
                booked.push((t, c, a, b));
                ready = t as u64 + 1;
            }
            None => {
                for (t, c, ta, tb) in booked {
                    sched.tx[t].retain(|&e| e != (ta, c));
                    sched.rx[t].retain(|&r| r != tb);
                }
                return None;
            }
        }
    }
    Some(ready)
}

struct Outcome {
    result: SimResult,
    // Read only by the schedule test, so dead in the non-test build.
    #[cfg_attr(not(test), allow(dead_code))]
    sched: Schedule,
    #[cfg_attr(not(test), allow(dead_code))]
    adj: Vec<bool>,
}

fn simulate(cfg: &SimConfig) -> Outcome {
    let n = cfg.relays;
    let budget = BUDGET_CYCLES * cfg.cycle_s;
    let mut rng = Rng::new(cfg.seed);

    let pos: Vec<(f64, f64)> = (0..n).map(|_| (rng.unit() * SIDE, rng.unit() * SIDE)).collect();
    let mut adj = vec![false; n * n];
    for i in 0..n {
        for j in 0..n {
            if i != j {
                let dx = pos[i].0 - pos[j].0;
                let dy = pos[i].1 - pos[j].1;
                adj[i * n + j] = (dx * dx + dy * dy).sqrt() <= cfg.radius;
            }
        }
    }

    // Poisson arrival times in whole seconds.
    let rate = cfg.arrival_per_min / 60.0;
    let mut arrivals = Vec::new();
    if rate > 0.0 && n >= 2 {
        let mut t = 0.0f64;
        loop {
            t += -(1.0 - rng.unit()).ln() / rate;
            if t >= cfg.duration_s as f64 {
                break;
            }
            arrivals.push(t as u64);
        }
    }

    let horizon = (cfg.duration_s + budget + 2) as usize;
    let mut sched = Schedule::new(horizon, cfg.channels);
    let mut latencies = Vec::new();
    let mut hop_total = 0usize;
    let mut dropped = 0usize;
    let mut avoided = 0usize;

    for &arr in &arrivals {
        let src = rng.below(n);
        let mut dst = rng.below(n - 1);
        if dst >= src {
            dst += 1;
        }
        let path = match shortest_path(&adj, n, src, dst) {
            Some(p) => p,
            None => {
                dropped += 1;
                continue;
            }
        };
        match place(&mut sched, &adj, n, &path, arr, arr + budget, &mut avoided) {
            Some(done) => {
                latencies.push((done - arr) as f64);
                hop_total += path.len() - 1;
            }
            None => dropped += 1,
        }
    }

    latencies.sort_by(f64::total_cmp);
    let delivered = latencies.len();
    let (mean_latency_s, p95_latency_s, mean_hops) = if delivered == 0 {
        (0.0, 0.0, 0.0)
    } else {
        let mean = latencies.iter().sum::<f64>() / delivered as f64;
        let rank = ((0.95 * delivered as f64).ceil() as usize).max(1);
        (mean, latencies[rank - 1], hop_total as f64 / delivered as f64)
    };

    Outcome {
        result: SimResult {
            delivered,
            dropped,
            mean_latency_s,
            p95_latency_s,
            mean_hops,
            collisions_avoided: avoided,
        },
        sched,
        adj,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(relays: usize, rate: f64, radius: f64, seed: u64) -> SimConfig {
        SimConfig {
            relays,
            channels: 4,
            cycle_s: 10,
            radius,
            arrival_per_min: rate,
            duration_s: 3600,
            seed,
        }
    }

    #[test]
    fn relay_sim_zero_arrivals_delivers_nothing() {
        let r = run(&cfg(50, 0.0, 25.0, 1));
        assert_eq!(r.delivered, 0);
        assert_eq!(r.dropped, 0);
        assert_eq!(r.mean_latency_s, 0.0);
    }

    #[test]
    fn relay_sim_light_load_delivers_everything() {
        let r = run(&cfg(20, 1.0, 45.0, 7));
        assert!(r.delivered > 0, "{r:?}");
        assert_eq!(r.dropped, 0, "{r:?}");
    }

    #[test]
    fn relay_sim_heavy_load_raises_p95() {
        let light = run(&cfg(50, 1.0, 25.0, 42));
        let heavy = run(&cfg(50, 60.0, 25.0, 42));
        assert!(
            heavy.p95_latency_s > light.p95_latency_s,
            "light {light:?} heavy {heavy:?}"
        );
    }

    #[test]
    fn relay_sim_schedule_keeps_interferers_apart() {
        let c = cfg(50, 60.0, 25.0, 3);
        let out = simulate(&c);
        assert!(out.result.delivered > 0, "{:?}", out.result);
        let n = c.relays;
        for (t, txs) in out.sched.tx.iter().enumerate() {
            for (i, &(a, ca)) in txs.iter().enumerate() {
                assert!(
                    !out.sched.rx[t].contains(&a),
                    "relay {a} transmits and receives in second {t}"
                );
                for &(b, cb) in &txs[i + 1..] {
                    assert_ne!(a, b, "relay {a} transmits twice in second {t}");
                    if ca == cb {
                        assert!(
                            !out.adj[a * n + b],
                            "interfering relays {a} and {b} share channel {ca} in second {t}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    #[ignore]
    fn relay_sim_sweep_arrival_rates() {
        println!("rate/min  delivered  dropped  mean_lat_s  p95_lat_s  mean_hops  collisions_avoided");
        for rate in [1.0, 5.0, 20.0, 60.0] {
            let r = run(&SimConfig {
                relays: 50,
                channels: 4,
                cycle_s: 10,
                radius: 25.0,
                arrival_per_min: rate,
                duration_s: 3600,
                seed: 42,
            });
            println!(
                "{:>8}  {:>9}  {:>7}  {:>10.1}  {:>9.1}  {:>9.2}  {:>18}",
                rate,
                r.delivered,
                r.dropped,
                r.mean_latency_s,
                r.p95_latency_s,
                r.mean_hops,
                r.collisions_avoided
            );
        }
    }
}
