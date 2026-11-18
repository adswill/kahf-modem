//! The relay's push link to the server (MESH-SPEC.md 4 and step 10).
//!
//! A relay holds one WebSocket to its region's hub. The server pushes plans and deliveries the
//! moment they exist, so a message needs no poll wait. The relay sends a heartbeat every 30 s.
//! If the socket is down the relay falls back to an HTTPS poll every 10 s and keeps trying to
//! reconnect (2 s, doubling to 30 s). The hub holds a pushed item in memory for 60 s at most,
//! for a relay whose socket is down; nothing is written to disk.
//!
//! One live socket per relay key (invariant I5): when a second connection arrives the first is
//! sent `Kicked` and its client stops for good.
//!
//! This is the client logic over two traits. The real WebSocket and HTTPS transports are not
//! written yet, and the server side (`server/src/hub.js`) is not wired into a Durable Object.

use crate::clock::Clock;
use std::time::{Duration, Instant};

pub const HEARTBEAT: Duration = Duration::from_secs(30);
pub const POLL_EVERY: Duration = Duration::from_secs(10);
pub const RECONNECT_MIN: Duration = Duration::from_secs(2);
pub const RECONNECT_MAX: Duration = Duration::from_secs(30);
/// How long the hub keeps an item for a relay with no socket.
pub const HOLD: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    Hello,
    Heartbeat { queue: u32 },
    /// A message to carry to relay `to`: only an id and a size, never the body.
    Deliver { id: u64, to: String, size: u32 },
    Plan { epoch: u64 },
    Kicked,
}

/// The WebSocket.
pub trait Wire {
    fn connect(&mut self) -> Result<(), String>;
    fn send(&mut self, m: Msg) -> Result<(), String>;
    /// The next pushed item, if one has arrived. `Err` means the socket is gone.
    fn recv(&mut self) -> Result<Option<Msg>, String>;
}

/// The HTTPS fallback: everything the hub holds for this relay.
pub trait Poller {
    fn poll(&mut self) -> Result<Vec<Msg>, String>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    Ws,
    Poll,
    /// Told to stop: another instance took over this relay key.
    Stopped,
}

pub struct PushClient<W: Wire, P: Poller> {
    clock: Clock,
    wire: W,
    poller: P,
    pub via: Via,
    last_beat: Instant,
    next_poll: Instant,
    next_connect: Instant,
    backoff: Duration,
    pub queue: u32,
}

impl<W: Wire, P: Poller> PushClient<W, P> {
    /// Starts on the poll path and connects at the first `step`.
    pub fn new(clock: Clock, wire: W, poller: P) -> Self {
        let now = clock.now();
        PushClient {
            clock,
            wire,
            poller,
            via: Via::Poll,
            last_beat: now,
            next_poll: now,
            next_connect: now,
            backoff: RECONNECT_MIN,
            queue: 0,
        }
    }

    /// Send something to the hub. Err when the socket is down: the caller keeps the item and tries again.
    pub fn send(&mut self, m: Msg) -> Result<(), String> {
        if self.via != Via::Ws {
            return Err("no socket".into());
        }
        let r = self.wire.send(m);
        if r.is_err() {
            self.drop_socket();
        }
        r
    }

    fn drop_socket(&mut self) {
        let now = self.clock.now();
        self.via = Via::Poll;
        self.next_connect = now + self.backoff;
        self.next_poll = now;
    }

    /// Call often (every few ms). Returns what arrived.
    pub fn step(&mut self) -> Vec<Msg> {
        let now = self.clock.now();
        let mut out = Vec::new();
        match self.via {
            Via::Stopped => {}
            Via::Ws => {
                loop {
                    match self.wire.recv() {
                        Ok(Some(Msg::Kicked)) => {
                            self.via = Via::Stopped;
                            return out;
                        }
                        Ok(Some(m)) => out.push(m),
                        Ok(None) => break,
                        Err(_) => {
                            self.drop_socket();
                            return out;
                        }
                    }
                }
                if now.saturating_duration_since(self.last_beat) >= HEARTBEAT {
                    self.last_beat = now;
                    let q = self.queue;
                    if self.wire.send(Msg::Heartbeat { queue: q }).is_err() {
                        self.drop_socket();
                    }
                }
            }
            Via::Poll => {
                if now >= self.next_connect {
                    if self.wire.connect().is_ok() && self.wire.send(Msg::Hello).is_ok() {
                        self.via = Via::Ws;
                        self.backoff = RECONNECT_MIN;
                        self.last_beat = now;
                    } else {
                        self.backoff = (self.backoff * 2).min(RECONNECT_MAX);
                        self.next_connect = now + self.backoff;
                    }
                }
                if self.via == Via::Poll && now >= self.next_poll {
                    self.next_poll = now + POLL_EVERY;
                    if let Ok(items) = self.poller.poll() {
                        out.extend(items);
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::{HashMap, VecDeque};
    use std::rc::Rc;

    /// A fake hub with a network delay. Items for a relay with a live socket arrive after the
    /// delay; otherwise they wait in RAM for the poll, at most `HOLD`.
    struct Hub {
        clock: Clock,
        latency: Duration,
        up: bool,
        /// relay -> socket generation currently live
        socket: HashMap<String, u64>,
        gen: u64,
        /// pushes on the way: (arrive_at, relay, msg)
        flight: Vec<(Instant, String, Msg)>,
        inbox: HashMap<String, VecDeque<Msg>>,
        held: HashMap<String, Vec<(Instant, Msg)>>,
        beats: HashMap<String, u32>,
    }

    type Shared = Rc<RefCell<Hub>>;

    fn hub(clock: &Clock, latency_ms: u64) -> Shared {
        Rc::new(RefCell::new(Hub {
            clock: clock.clone(),
            latency: Duration::from_millis(latency_ms),
            up: true,
            socket: HashMap::new(),
            gen: 0,
            flight: vec![],
            inbox: HashMap::new(),
            held: HashMap::new(),
            beats: HashMap::new(),
        }))
    }

    impl Hub {
        fn route(&mut self, to: &str, m: Msg) {
            let now = self.clock.now();
            if self.socket.contains_key(to) {
                self.flight.push((now + self.latency, to.to_string(), m));
            } else {
                self.held.entry(to.to_string()).or_default().push((now, m));
            }
        }
        fn pump(&mut self) {
            let now = self.clock.now();
            let (due, rest): (Vec<_>, Vec<_>) = self.flight.drain(..).partition(|(t, _, _)| *t <= now);
            self.flight = rest;
            for (_, to, m) in due {
                if self.socket.contains_key(&to) {
                    self.inbox.entry(to).or_default().push_back(m);
                } else {
                    self.held.entry(to).or_default().push((now, m));
                }
            }
        }
        fn drop_socket(&mut self, relay: &str) {
            self.socket.remove(relay);
            self.inbox.remove(relay);
        }
        /// The hub dies: every socket closes.
        fn set_up(&mut self, up: bool) {
            self.up = up;
            if !up {
                self.socket.clear();
                self.inbox.clear();
                self.flight.clear();
            }
        }
    }

    struct FakeWire {
        hub: Shared,
        me: String,
        gen: u64,
    }

    impl Wire for FakeWire {
        fn connect(&mut self) -> Result<(), String> {
            let mut h = self.hub.borrow_mut();
            if !h.up {
                return Err("hub down".into());
            }
            h.gen += 1;
            self.gen = h.gen;
            // One socket per relay key: the old one is told to stop.
            if h.socket.contains_key(&self.me) {
                h.inbox.entry(self.me.clone()).or_default().push_back(Msg::Kicked);
            }
            h.socket.insert(self.me.clone(), self.gen);
            Ok(())
        }
        fn send(&mut self, m: Msg) -> Result<(), String> {
            let mut h = self.hub.borrow_mut();
            if !h.up || h.socket.get(&self.me) != Some(&self.gen) {
                return Err("socket gone".into());
            }
            match m {
                Msg::Deliver { ref to, .. } => {
                    let to = to.clone();
                    h.route(&to, m);
                }
                Msg::Heartbeat { queue } => {
                    *h.beats.entry(self.me.clone()).or_default() += 1;
                    let _ = queue;
                }
                _ => {}
            }
            Ok(())
        }
        fn recv(&mut self) -> Result<Option<Msg>, String> {
            let mut h = self.hub.borrow_mut();
            if !h.up {
                return Err("hub down".into());
            }
            h.pump();
            if let Some(m) = h.inbox.get_mut(&self.me).and_then(|q| q.pop_front()) {
                return Ok(Some(m));
            }
            if h.socket.get(&self.me) != Some(&self.gen) {
                return Err("socket closed".into());
            }
            Ok(None)
        }
    }

    struct FakePoll {
        hub: Shared,
        me: String,
    }

    impl Poller for FakePoll {
        fn poll(&mut self) -> Result<Vec<Msg>, String> {
            let mut h = self.hub.borrow_mut();
            if !h.up {
                return Err("hub down".into());
            }
            let now = h.clock.now();
            let held = h.held.remove(&self.me).unwrap_or_default();
            Ok(held.into_iter().filter(|(t, _)| now.saturating_duration_since(*t) <= HOLD).map(|(_, m)| m).collect())
        }
    }

    fn client(clock: &Clock, hub: &Shared, me: &str) -> PushClient<FakeWire, FakePoll> {
        PushClient::new(
            clock.clone(),
            FakeWire { hub: hub.clone(), me: me.into(), gen: 0 },
            FakePoll { hub: hub.clone(), me: me.into() },
        )
    }

    /// Steps every client every 5 ms for `ms`; returns (virtual ms when seen, relay index, msg).
    fn run(clock: &Clock, cs: &mut [&mut PushClient<FakeWire, FakePoll>], ms: u64) -> Vec<(u64, usize, Msg)> {
        let t0 = clock.now();
        let mut got = Vec::new();
        for _ in 0..ms / 5 {
            for (i, c) in cs.iter_mut().enumerate() {
                for m in c.step() {
                    got.push((clock.since(t0).as_millis() as u64, i, m));
                }
            }
            clock.sleep(Duration::from_millis(5));
        }
        got
    }

    fn setup(latency_ms: u64) -> (Clock, Shared) {
        let clock = Clock::virtual_time();
        clock.join_thread();
        let h = hub(&clock, latency_ms);
        (clock, h)
    }

    #[test]
    fn a_pushed_delivery_reaches_the_other_relay_within_100_ms() {
        let (clock, h) = setup(40);
        let (mut a, mut b) = (client(&clock, &h, "RA"), client(&clock, &h, "RB"));
        run(&clock, &mut [&mut a, &mut b], 100);
        assert_eq!((a.via, b.via), (Via::Ws, Via::Ws));
        let t_send = clock.now();
        a.send(Msg::Deliver { id: 7, to: "RB".into(), size: 30 }).unwrap();
        let got = run(&clock, &mut [&mut a, &mut b], 500);
        let (at, who, m) = got.iter().find(|(_, _, m)| matches!(m, Msg::Deliver { .. })).expect("delivered");
        assert_eq!((*who, m.clone()), (1, Msg::Deliver { id: 7, to: "RB".into(), size: 30 }));
        assert!(*at < 100, "took {at} ms");
        let _ = t_send;
    }

    #[test]
    fn a_second_connection_with_the_same_key_stops_the_first() {
        let (clock, h) = setup(10);
        let mut first = client(&clock, &h, "RA");
        run(&clock, &mut [&mut first], 50);
        assert_eq!(first.via, Via::Ws);
        let mut second = client(&clock, &h, "RA");
        run(&clock, &mut [&mut first, &mut second], 100);
        assert_eq!(first.via, Via::Stopped);
        assert_eq!(second.via, Via::Ws);
        assert!(first.step().is_empty(), "a stopped client does nothing");
    }

    #[test]
    fn a_heartbeat_goes_out_every_30_seconds() {
        let (clock, h) = setup(10);
        let mut a = client(&clock, &h, "RA");
        run(&clock, &mut [&mut a], 95_000);
        assert_eq!(h.borrow().beats.get("RA").copied(), Some(3));
    }

    #[test]
    fn with_the_socket_down_the_poll_still_delivers_within_10_seconds_then_the_socket_returns() {
        let (clock, h) = setup(40);
        let (mut a, mut b) = (client(&clock, &h, "RA"), client(&clock, &h, "RB"));
        run(&clock, &mut [&mut a, &mut b], 100);
        // B's socket drops (a bad network); the hub holds what comes for B.
        h.borrow_mut().drop_socket("RB");
        a.send(Msg::Deliver { id: 1, to: "RB".into(), size: 10 }).unwrap();
        let got = run(&clock, &mut [&mut a, &mut b], 12_000);
        let at = got.iter().find(|(_, w, m)| *w == 1 && matches!(m, Msg::Deliver { .. })).map(|(t, _, _)| *t).expect("delivered by poll");
        assert!(at <= 10_100, "took {at} ms");
        // B reconnected by itself.
        assert_eq!(b.via, Via::Ws);
        a.send(Msg::Deliver { id: 2, to: "RB".into(), size: 10 }).unwrap();
        let got = run(&clock, &mut [&mut a, &mut b], 300);
        assert!(got.iter().any(|(t, w, m)| *w == 1 && *t < 100 && matches!(m, Msg::Deliver { id: 2, .. })));
    }

    #[test]
    fn a_dead_hub_for_60_seconds_is_survived_and_old_items_are_dropped() {
        let (clock, h) = setup(40);
        let (mut a, mut b) = (client(&clock, &h, "RA"), client(&clock, &h, "RB"));
        run(&clock, &mut [&mut a, &mut b], 100);
        // An item for B is held while B has no socket ...
        h.borrow_mut().drop_socket("RB");
        a.send(Msg::Deliver { id: 9, to: "RB".into(), size: 10 }).unwrap();
        // ... and the hub is then dead for 70 s: the item is older than 60 s when it comes back.
        h.borrow_mut().set_up(false);
        let got = run(&clock, &mut [&mut a, &mut b], 70_000);
        assert!(got.is_empty());
        assert_eq!((a.via, b.via), (Via::Poll, Via::Poll));
        assert!(a.send(Msg::Deliver { id: 10, to: "RB".into(), size: 1 }).is_err(), "the caller keeps what it could not send");
        h.borrow_mut().set_up(true);
        let got = run(&clock, &mut [&mut a, &mut b], 40_000);
        assert!(got.iter().all(|(_, _, m)| !matches!(m, Msg::Deliver { id: 9, .. })), "a 70 s old item must be dropped");
        assert_eq!((a.via, b.via), (Via::Ws, Via::Ws));
        a.send(Msg::Deliver { id: 11, to: "RB".into(), size: 1 }).unwrap();
        let got = run(&clock, &mut [&mut a, &mut b], 200);
        assert!(got.iter().any(|(_, w, m)| *w == 1 && matches!(m, Msg::Deliver { id: 11, .. })));
    }
}
