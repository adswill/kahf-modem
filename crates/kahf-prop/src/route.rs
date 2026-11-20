//! Propagation-aware route planning for a radio relay network on one HF band (17 m by
//! default).
//!
//! STATUS: ESTIMATE ONLY, like the rest of this crate. A route is a suggestion of which
//! relays to try, in order. It never keys a radio.
//!
//! Model:
//! - Every station and relay is a node with a position. Two nodes are linked when the band
//!   is predicted usable between them at the given hour: by ground wave within
//!   [`GROUND_WAVE_KM`], otherwise by sky wave inside the LUF..MUF window from [`crate::muf`]
//!   and [`crate::luf`]. On 17 m the MUF of a short path is usually below the band, so the
//!   skip zone (too far for ground wave, too near for sky wave) falls out of the model.
//! - Each link costs one per hop, plus [`MARGINAL_COST`] when it is marginal, plus a small
//!   distance term so that, between equal routes, the shorter one wins.
//! - The planner is Dijkstra over at most [`MAX_HOPS`] hops.

use crate::{great_circle_km, luf, muf, PathInput, Status};

/// Centre of the 17 m band in MHz, the default relay band.
pub const BAND_17M_MHZ: f64 = 18.1;
/// Range in km that ground wave is trusted to cover on 17 m with simple antennas.
pub const GROUND_WAVE_KM: f64 = 30.0;
/// Extra cost of a hop predicted as marginal.
pub const MARGINAL_COST: f64 = 2.0;
/// Most hops in one route (stations included as end points).
pub const MAX_HOPS: usize = 4;
/// Cost per 1000 km, enough to break ties between routes of equal hops.
const DISTANCE_COST_PER_1000KM: f64 = 0.01;

/// A station or relay with its position `(latitude, longitude)` in degrees.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub call: String,
    pub pos: (f64, f64),
}

impl Node {
    pub fn new(call: &str, pos: (f64, f64)) -> Self {
        Node {
            call: call.to_string(),
            pos,
        }
    }
}

/// Band and ionosphere for one plan.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Conditions {
    pub freq_mhz: f64,
    pub utc_hour: u8,
    pub month: u8,
    pub solar_flux: f64,
    pub k_index: f64,
}

impl Conditions {
    /// 17 m with the crate defaults: solar flux 100, K index 2.
    pub fn band_17m(utc_hour: u8, month: u8) -> Self {
        Conditions {
            freq_mhz: BAND_17M_MHZ,
            utc_hour,
            month,
            solar_flux: 100.0,
            k_index: 2.0,
        }
    }

    fn at_hour(self, utc_hour: u8) -> Self {
        Conditions { utc_hour, ..self }
    }
}

/// How usable the band is between two points under `c`.
pub fn link_status(a: (f64, f64), b: (f64, f64), c: &Conditions) -> Status {
    if great_circle_km(a, b) <= GROUND_WAVE_KM {
        return Status::Open;
    }
    let input = PathInput {
        tx: a,
        rx: b,
        utc_hour: c.utc_hour.min(23),
        month: c.month.clamp(1, 12),
        solar_flux: c.solar_flux,
        k_index: c.k_index,
    };
    let (m, l) = (muf(&input), luf(&input));
    let f = c.freq_mhz;
    if f > m || f < l {
        Status::Closed
    } else if f > 0.85 * m || f < 1.2 * l {
        Status::Marginal
    } else {
        Status::Open
    }
}

/// A planned route: every call from the sender to the addressee, in order.
#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    pub calls: Vec<String>,
    pub cost: f64,
    /// True when any hop is only marginal.
    pub marginal: bool,
}

impl Route {
    /// The relays between the end points.
    pub fn relays(&self) -> &[String] {
        &self.calls[1..self.calls.len() - 1]
    }

    /// The first station to send to: the addressee itself or the first relay.
    pub fn next_hop(&self) -> &str {
        &self.calls[1]
    }
}

fn hop_cost(a: &Node, b: &Node, c: &Conditions) -> Option<(f64, bool)> {
    let distance = DISTANCE_COST_PER_1000KM * great_circle_km(a.pos, b.pos) / 1000.0;
    match link_status(a.pos, b.pos, c) {
        Status::Open => Some((1.0 + distance, false)),
        Status::Marginal => Some((1.0 + MARGINAL_COST + distance, true)),
        Status::Closed => None,
    }
}

/// Best route from `from` to `to` through `relays` under `c`, or None when the band does
/// not connect them in at most [`MAX_HOPS`] hops. Relays with the same call as an end point
/// are ignored.
///
/// Bellman-Ford limited to `MAX_HOPS` rounds: `best[k][v]` is the cheapest way to reach `v`
/// in exactly `k` hops, so a cheap route with many hops never hides a dearer one that still
/// fits the hop limit.
pub fn plan(from: &Node, to: &Node, relays: &[Node], c: &Conditions) -> Option<Route> {
    let mut nodes = vec![from.clone(), to.clone()];
    nodes.extend(
        relays
            .iter()
            .filter(|r| !r.call.eq_ignore_ascii_case(&from.call) && !r.call.eq_ignore_ascii_case(&to.call))
            .cloned(),
    );
    let n = nodes.len();
    // Links are worked out once: cost and whether it is marginal, or None when closed.
    let links: Vec<Vec<Option<(f64, bool)>>> = (0..n)
        .map(|u| {
            (0..n)
                .map(|v| {
                    if u == v {
                        None
                    } else {
                        hop_cost(&nodes[u], &nodes[v], c)
                    }
                })
                .collect()
        })
        .collect();
    let mut best = vec![vec![f64::INFINITY; n]; MAX_HOPS + 1];
    let mut prev = vec![vec![None::<usize>; n]; MAX_HOPS + 1];
    let mut weak = vec![vec![false; n]; MAX_HOPS + 1];
    best[0][0] = 0.0;
    for k in 1..=MAX_HOPS {
        for u in 0..n {
            // Paths end at `to`; they never pass through it.
            if !best[k - 1][u].is_finite() || u == 1 {
                continue;
            }
            for v in 1..n {
                let Some((step, marginal)) = links[u][v] else {
                    continue;
                };
                let total = best[k - 1][u] + step;
                if total < best[k][v] {
                    best[k][v] = total;
                    prev[k][v] = Some(u);
                    weak[k][v] = weak[k - 1][u] || marginal;
                }
            }
        }
    }
    let k = (1..=MAX_HOPS).min_by(|&a, &b| best[a][1].total_cmp(&best[b][1]))?;
    if !best[k][1].is_finite() {
        return None;
    }
    let mut calls = vec![nodes[1].call.clone()];
    let (mut at, mut hops) = (1, k);
    while let Some(p) = prev[hops][at] {
        calls.push(nodes[p].call.clone());
        at = p;
        hops -= 1;
    }
    calls.reverse();
    Some(Route {
        calls,
        cost: best[k][1],
        marginal: weak[k][1],
    })
}

/// Relays to try for `to`, best first: the first relay of the best route at each of the
/// next 24 hours, starting at `c.utc_hour`, without repeats. Good input for a station's
/// relay paths, so that a message can still leave when the band moves.
pub fn first_relays(from: &Node, to: &Node, relays: &[Node], c: &Conditions) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for h in 0..24u8 {
        let at = c.at_hour((c.utc_hour % 24 + h) % 24);
        if let Some(route) = plan(from, to, relays, &at) {
            if let Some(first) = route.relays().first() {
                if !out.iter().any(|r| r.eq_ignore_ascii_case(first)) {
                    out.push(first.clone());
                }
            }
        }
    }
    out
}

/// For each UTC hour 0..=23, whether a route exists from `from` to `to`. Lets a relay
/// schedule ask for slots when its paths are open.
pub fn open_hours(from: &Node, to: &Node, relays: &[Node], c: &Conditions) -> [bool; 24] {
    let mut out = [false; 24];
    for (h, slot) in out.iter_mut().enumerate() {
        *slot = plan(from, to, relays, &c.at_hour(h as u8)).is_some();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Rough positions: Helsinki, Tallinn (80 km), Berlin (1100 km), Madrid, New York.
    fn hel() -> Node {
        Node::new("OH2AAA", (60.17, 24.94))
    }
    fn tll() -> Node {
        Node::new("ES1BBB", (59.44, 24.75))
    }
    fn ber() -> Node {
        Node::new("DL1CCC", (52.52, 13.40))
    }
    fn mad() -> Node {
        Node::new("EA4DDD", (40.42, -3.70))
    }
    fn nyc() -> Node {
        Node::new("W2EEE", (40.71, -74.01))
    }

    /// Midday in June with a good sun: 17 m open on long paths.
    fn day() -> Conditions {
        Conditions {
            solar_flux: 150.0,
            ..Conditions::band_17m(12, 6)
        }
    }

    #[test]
    fn ground_wave_neighbours_are_always_linked() {
        let a = Node::new("A1AAA", (60.0, 25.0));
        let b = Node::new("B1BBB", (60.1, 25.0));
        for h in 0..24 {
            assert_eq!(
                link_status(a.pos, b.pos, &Conditions::band_17m(h, 1)),
                Status::Open
            );
        }
    }

    #[test]
    fn short_skywave_path_is_in_the_skip_zone_on_17m() {
        // 80 km: beyond ground wave, far too short for 17 m sky wave.
        assert_eq!(link_status(hel().pos, tll().pos, &day()), Status::Closed);
    }

    #[test]
    fn long_daytime_path_is_usable() {
        assert_ne!(link_status(hel().pos, mad().pos, &day()), Status::Closed);
    }

    #[test]
    fn night_closes_17m() {
        let night = Conditions {
            solar_flux: 70.0,
            ..Conditions::band_17m(1, 12)
        };
        assert_eq!(link_status(hel().pos, mad().pos, &night), Status::Closed);
    }

    #[test]
    fn direct_route_needs_no_relay() {
        let r = plan(&hel(), &mad(), &[ber()], &day()).unwrap();
        if r.calls.len() == 2 {
            assert!(r.relays().is_empty());
            assert_eq!(r.next_hop(), "EA4DDD");
        }
        assert_eq!(r.calls.first().unwrap(), "OH2AAA");
        assert_eq!(r.calls.last().unwrap(), "EA4DDD");
    }

    #[test]
    fn skip_zone_is_bridged_by_a_distant_relay() {
        // Helsinki to Tallinn is in the skip zone; both hear Madrid by sky wave.
        let r = plan(&hel(), &tll(), &[mad()], &day()).expect("route through the relay");
        assert_eq!(r.calls, vec!["OH2AAA", "EA4DDD", "ES1BBB"]);
        assert_eq!(r.next_hop(), "EA4DDD");
        assert!(plan(&hel(), &tll(), &[], &day()).is_none(), "no relay, no route");
    }

    #[test]
    fn ground_wave_relay_chain_beats_nothing() {
        // Three relays 25 km apart carry a message 100 km by ground wave.
        let a = Node::new("A1AAA", (60.0, 25.0));
        let b = Node::new("B1BBB", (60.9, 25.0));
        let relays: Vec<Node> = (1..=3)
            .map(|i| Node::new(&format!("R{i}RRR"), (60.0 + 0.225 * i as f64, 25.0)))
            .collect();
        let night = Conditions::band_17m(0, 12);
        let r = plan(&a, &b, &relays, &night).expect("ground-wave chain");
        assert_eq!(r.calls, vec!["A1AAA", "R1RRR", "R2RRR", "R3RRR", "B1BBB"]);
        assert!(!r.marginal);
        // One more hop than MAX_HOPS allows: no route.
        let far = Node::new("C1CCC", (61.125, 25.0));
        let mut more = relays.clone();
        more.push(Node::new("R4RRR", (60.9, 25.0)));
        assert!(plan(&a, &far, &more, &night).is_none());
    }

    /// A route that needs exactly the hop limit is found, with a spare relay on the side.
    #[test]
    fn route_using_every_allowed_hop_is_found() {
        // Ground-wave line: A - X - Y - Z - B, about 27 km apart, and R1 between A and X.
        let n = |call: &str, lat: f64| Node::new(call, (60.0 + lat, 25.0));
        let c = Conditions::band_17m(0, 12);
        let (a, b) = (n("A1AAA", 0.0), n("B1BBB", 0.96));
        let relays = [
            n("R1RRR", 0.12),
            n("X1XXX", 0.24),
            n("Y1YYY", 0.48),
            n("Z1ZZZ", 0.72),
        ];
        let r = plan(&a, &b, &relays, &c).expect("A-X-Y-Z-B fits in four hops");
        assert_eq!(r.calls, vec!["A1AAA", "X1XXX", "Y1YYY", "Z1ZZZ", "B1BBB"]);
    }

    #[test]
    fn out_of_range_hours_do_not_overflow() {
        let c = Conditions {
            utc_hour: 250,
            ..day()
        };
        let _ = first_relays(&hel(), &tll(), &[mad()], &c);
    }

    #[test]
    fn first_relays_cover_the_day_without_repeats() {
        let relays = [mad(), nyc(), ber()];
        let list = first_relays(&hel(), &tll(), &relays, &day());
        assert!(!list.is_empty());
        let mut sorted = list.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), list.len());
    }

    #[test]
    fn open_hours_follow_the_sun() {
        let hours = open_hours(&hel(), &mad(), &[], &day());
        assert!(hours[12], "midday open");
        assert!(hours.iter().any(|h| !h), "17 m should close for part of the day");
    }
}
