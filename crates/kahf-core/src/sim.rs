//! A small network simulator: many stations on a shared HF channel, in virtual time.
//!
//! It runs the real `Link` (ARQ) and `Mac` (who may send when) of every station against a
//! medium that does what radio does: a station that sends is deaf, two bursts that overlap at
//! a receiver destroy each other (unless one is much stronger), two stations may not hear each
//! other but both reach a third (hidden stations), and a burst can be lost to fading.
//!
//! It is the measuring stick for NETWORK-SCENARIOS.md: a rule is real when a scenario passes here.

use std::collections::{HashMap, VecDeque};

use crate::link::{Action, Link, LinkState};
use crate::mac::{Class, Mac};

/// Virtual time step.
const STEP_MS: u64 = 25;
/// Air time of a burst of `n` frames (KAMI-32 like: a preamble and 0.55 s per codeword).
pub fn burst_ms(n: usize) -> u64 {
    1_200 + 550 * n as u64
}
/// A stronger burst survives an overlap when it is this many dB above the other.
const CAPTURE_DB: f32 = 6.0;
const BURST_MAX: usize = 8;

struct Tx {
    from: usize,
    start: u64,
    end: u64,
    frames: Vec<Vec<u8>>,
}

pub struct Outgoing {
    class: Class,
    payload: Vec<u8>,
}

/// One job: send `chunks` chunks to `dest`.
#[derive(Clone)]
struct Job {
    id: u16,
    dest: usize,
    chunks: u8,
}

pub struct Station {
    pub call: String,
    link: Link,
    pub mac: Mac,
    pub out: VecDeque<Outgoing>,
    txing: Option<u64>,
    jobs: VecDeque<Job>,
    sent_job: bool,
    /// (sender index, job id) -> chunks received.
    got: HashMap<(usize, u16), u8>,
    pub delivered: Vec<(usize, u16, u64)>,
    pub tx_ms: u64,
    pub lost_overlap: u64,
    pub lost_fade: u64,
}

pub struct Net {
    pub now: u64,
    pub st: Vec<Station>,
    /// `hear[a][b]`: SNR in dB at which b hears a, or None.
    hear: Vec<Vec<Option<f32>>>,
    active: Vec<Tx>,
    /// Recently finished transmissions, kept to judge overlaps.
    recent: VecDeque<Tx>,
    /// Probability that a burst is lost to fading even without a collision.
    pub fade_loss: f32,
    rng: u64,
    /// Total transmissions that were destroyed at some receiver by an overlap.
    pub collisions: u64,
    pub bursts: u64,
    /// Test switch: every station sends as soon as it has something, ignoring the MAC.
    pub no_mac: bool,
    pub trace: bool,
}

impl Net {
    /// `n` stations that all hear each other at `snr_db`.
    pub fn full(n: usize, snr_db: f32, seed: u64) -> Net {
        let hear = (0..n)
            .map(|a| (0..n).map(|b| (a != b).then_some(snr_db)).collect())
            .collect();
        Net::with(n, hear, seed)
    }

    /// Stations with an explicit hearing table.
    pub fn with(n: usize, hear: Vec<Vec<Option<f32>>>, seed: u64) -> Net {
        let st = (0..n)
            .map(|i| {
                let call = format!(
                    "K{}{}",
                    (b'A' + (i / 26) as u8) as char,
                    (b'A' + (i % 26) as u8) as char
                ) + "1AA";
                let mut link = Link::new(&call);
                link.set_air_ms(burst_ms(1));
                Station {
                    call,
                    link,
                    mac: Mac::new(seed.wrapping_mul(2654435761).wrapping_add(i as u64 * 7919 + 1)),
                    out: VecDeque::new(),
                    txing: None,
                    jobs: VecDeque::new(),
                    sent_job: false,
                    got: HashMap::new(),
                    delivered: Vec::new(),
                    tx_ms: 0,
                    lost_overlap: 0,
                    lost_fade: 0,
                }
            })
            .collect();
        Net {
            now: 0,
            st,
            hear,
            active: Vec::new(),
            recent: VecDeque::new(),
            fade_loss: 0.0,
            rng: seed | 1,
            collisions: 0,
            bursts: 0,
            no_mac: false,
            trace: false,
        }
    }

    fn rand01(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        ((x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32) / (1u64 << 24) as f32
    }

    /// Station `from` wants to send a message of `chunks` chunks to `dest` (job `id`).
    pub fn send(&mut self, from: usize, dest: usize, id: u16, chunks: u8) {
        self.st[from].jobs.push_back(Job { id, dest, chunks });
    }

    fn queue(&mut self, i: usize, actions: Vec<Action>, from_tick: bool) {
        for a in actions {
            if let Action::Transmit(p) = a {
                let answer = matches!(p.first(), Some(0x11) | Some(0x02) | Some(0x06));
                // The same frame already waiting to go out says nothing new.
                if self.st[i].out.iter().any(|o| o.payload == p) {
                    continue;
                }
                if from_tick && !answer {
                    // A retransmission: the last attempt got no answer.
                    self.st[i].mac.failure();
                }
                let class = if answer { Class::Answer } else { Class::Data };
                self.st[i].out.push_back(Outgoing { class, payload: p });
            }
        }
    }

    fn receive(&mut self, j: usize, frames: &[Vec<u8>]) {
        for f in frames {
            let before = self.st[j].link.describe();
            let acts = self.st[j].link.on_receive(f);
            if self.trace {
                println!(
                    "      st{j} rx tag {} : {} -> {}",
                    f[0],
                    before,
                    self.st[j].link.describe()
                );
            }
            // Only an answer to our own traffic shows the channel worked for us.
            if matches!(f.first(), Some(0x11) | Some(0x02)) {
                self.st[j].mac.success();
            }
            for a in &acts {
                if let Action::Deliver(chunk) = a {
                    if chunk.len() >= 4 {
                        let from_idx = chunk[0] as usize;
                        let id = u16::from_be_bytes([chunk[1], chunk[2]]);
                        let total = chunk[3];
                        let n = self.st[j].got.entry((from_idx, id)).or_insert(0);
                        *n += 1;
                        if *n == total {
                            let now = self.now;
                            self.st[j].delivered.push((from_idx, id, now));
                        }
                    }
                }
            }
            self.queue(j, acts, false);
        }
    }

    fn finish_tx(&mut self, tx: Tx) {
        let n = self.st.len();
        for j in 0..n {
            let Some(snr) = self.hear[tx.from][j] else {
                continue;
            };
            // Deaf while sending?
            let deaf = self.st[j].txing.is_some_and(|e| e > tx.start) || self.was_txing(j, tx.start, tx.end);
            // Overlapping audible bursts at j, and capture.
            let mut destroyed = deaf;
            for other in self.active.iter().chain(self.recent.iter()) {
                if other.from == tx.from || other.from == j {
                    continue;
                }
                if other.start < tx.end && other.end > tx.start {
                    if let Some(osnr) = self.hear[other.from][j] {
                        if snr < osnr + CAPTURE_DB {
                            destroyed = true;
                        }
                    }
                }
            }
            if destroyed && !deaf {
                self.st[j].lost_overlap += 1;
                self.collisions += 1;
            }
            if self.trace {
                println!(
                    "  {:7} tx st{} tags {:?} -> st{} destroyed={} deaf={}",
                    self.now,
                    tx.from,
                    tx.frames.iter().map(|f| f[0]).collect::<Vec<_>>(),
                    j,
                    destroyed,
                    deaf
                );
            }
            self.st[j].mac.on_rx_end(self.now);
            if destroyed {
                continue;
            }
            if self.fade_loss > 0.0 && self.rand01() < self.fade_loss {
                if self.trace {
                    println!("      faded at st{j}");
                }
                self.st[j].lost_fade += 1;
                continue;
            }
            self.receive(j, &tx.frames);
        }
    }

    fn was_txing(&self, j: usize, s: u64, e: u64) -> bool {
        self.recent
            .iter()
            .any(|t| t.from == j && t.start < e && t.end > s)
            || self
                .active
                .iter()
                .any(|t| t.from == j && t.start < e && t.end > s)
    }

    /// Run until virtual time `until_ms`.
    pub fn run(&mut self, until_ms: u64) {
        while self.now < until_ms {
            self.step();
        }
    }

    fn step(&mut self) {
        self.now += STEP_MS;
        let now = self.now;
        let n = self.st.len();
        // Finished transmissions.
        let (done, still): (Vec<Tx>, Vec<Tx>) = std::mem::take(&mut self.active)
            .into_iter()
            .partition(|t| t.end <= now);
        self.active = still;
        for tx in done {
            self.st[tx.from].txing = None;
            self.st[tx.from].mac.on_tx_end(now);
            self.st[tx.from].link.tx_finished(tx.end);
            let (from, start, end) = (tx.from, tx.start, tx.end);
            self.finish_tx(tx);
            self.recent.push_back(Tx {
                from,
                start,
                end,
                frames: Vec::new(),
            });
        }
        while self.recent.front().is_some_and(|t| t.end + 30_000 < now) {
            self.recent.pop_front();
        }
        for i in 0..n {
            // Ticks and job driving.
            let acts = self.st[i].link.tick(now);
            self.queue(i, acts, true);
            self.drive_jobs(i);
            self.try_transmit(i);
        }
    }

    fn drive_jobs(&mut self, i: usize) {
        let st = &mut self.st[i];
        let Some(job) = st.jobs.front().cloned() else {
            return;
        };
        let peer = self.st_call(job.dest);
        let state = self.st[i].link.metrics().state;
        match state {
            LinkState::Idle => {
                self.st[i].sent_job = false;
                let acts = self.st[i].link.connect(&peer);
                self.queue(i, acts, false);
            }
            LinkState::Connected { peer: p } if p == peer && !self.st[i].sent_job => {
                self.st[i].sent_job = true;
                for k in 0..job.chunks {
                    let mut chunk = vec![i as u8];
                    chunk.extend_from_slice(&job.id.to_be_bytes());
                    chunk.push(job.chunks);
                    chunk.push(k);
                    let acts = self.st[i].link.send(&chunk);
                    self.queue(i, acts, false);
                }
            }
            _ => {}
        }
        // The receiver has everything: finish the job and drop the link.
        let done = self.st[job.dest].got.get(&(i, job.id)).copied() == Some(job.chunks);
        if done {
            self.st[i].jobs.pop_front();
            self.st[i].sent_job = false;
            let acts = self.st[i].link.disconnect();
            self.queue(i, acts, false);
        }
    }

    fn st_call(&self, i: usize) -> String {
        self.st[i].call.clone()
    }

    fn try_transmit(&mut self, i: usize) {
        if self.st[i].txing.is_some() || self.st[i].out.is_empty() {
            return;
        }
        // Answers go ahead of everything else: the other station is waiting for them.
        let class = if self.st[i].out.iter().any(|o| o.class == Class::Answer) {
            Class::Answer
        } else {
            Class::Data
        };
        let now = self.now;
        if !self.no_mac && self.st[i].mac.check(now, class).is_some() {
            return;
        }
        // One burst for everything of the same class that is waiting.
        let mut frames = Vec::new();
        let mut keep = VecDeque::new();
        while let Some(o) = self.st[i].out.pop_front() {
            if o.class == class && frames.len() < BURST_MAX {
                frames.push(o.payload);
            } else {
                keep.push_back(o);
            }
        }
        self.st[i].out = keep;
        let end = now + burst_ms(frames.len());
        self.st[i].txing = Some(end);
        self.st[i].tx_ms += end - now;
        self.st[i].mac.on_tx_start(now, end);
        self.bursts += 1;
        // Everyone who hears it notices the start.
        for j in 0..self.st.len() {
            if self.hear[i][j].is_some() && self.st[j].txing.is_none() {
                self.st[j].mac.on_rx_start(now);
            }
        }
        self.active.push(Tx {
            from: i,
            start: now,
            end,
            frames,
        });
    }

    /// Messages delivered to `dest` so far.
    pub fn delivered_to(&self, dest: usize) -> usize {
        self.st[dest].delivered.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Everything arrives in this much virtual time (ten minutes).
    const LIMIT: u64 = 600_000;

    #[test]
    fn s8_two_stations_start_at_the_same_instant() {
        let mut failures = Vec::new();
        for seed in 1..=40u64 {
            let mut net = Net::full(3, 15.0, seed);
            // 0 and 1 both send to 2 at time zero.
            net.send(0, 2, 1, 3);
            net.send(1, 2, 2, 3);
            net.run(LIMIT);
            if net.delivered_to(2) != 2 {
                failures.push((seed, net.delivered_to(2), net.collisions));
            }
        }
        assert!(
            failures.is_empty(),
            "not delivered (seed, delivered, collisions): {failures:?}"
        );
    }

    /// Proves the simulator can see a collision: with the MAC off, the same start fails.
    #[test]
    fn the_simulator_sees_collisions_when_the_mac_is_off() {
        let mut lost = 0;
        for seed in 1..=20u64 {
            let mut net = Net::full(3, 15.0, seed);
            net.no_mac = true;
            net.send(0, 2, 1, 3);
            net.send(1, 2, 2, 3);
            net.run(LIMIT);
            lost += (net.delivered_to(2) != 2) as u32 + (net.collisions > 0) as u32;
        }
        assert!(lost > 0, "no collision was ever seen without the MAC");
    }

    #[test]
    fn s10_hidden_stations_both_reach_the_relay() {
        // 0 and 1 hear only 2; 2 hears both.
        let mut hear = vec![vec![None; 3]; 3];
        for a in [0usize, 1] {
            hear[a][2] = Some(15.0);
            hear[2][a] = Some(15.0);
        }
        let mut failures = Vec::new();
        for seed in 1..=40u64 {
            let mut net = Net::with(3, hear.clone(), seed);
            net.send(0, 2, 1, 3);
            net.send(1, 2, 2, 3);
            net.run(LIMIT);
            if net.delivered_to(2) != 2 {
                failures.push((seed, net.delivered_to(2), net.collisions));
            }
        }
        // Hidden stations cannot hear each other, so listening cannot stop their calls from
        // landing on the hub's reception. The BUSY reply and the back-off get nearly all of
        // them through; the rest need the relay to grant turns (NETWORK-DESIGN L2).
        assert!(
            failures.len() <= 1,
            "hidden stations (seed, delivered, collisions): {failures:?}"
        );
    }

    #[test]
    fn s15_ten_stations_send_to_one_hub() {
        let mut ok = 0;
        let runs = 10;
        for seed in 1..=runs {
            let mut net = Net::full(11, 15.0, seed);
            for i in 1..11usize {
                net.send(i, 0, i as u16, 3);
            }
            net.run(30 * 60_000);
            ok += net.delivered_to(0);
        }
        let total = runs as usize * 10;
        assert!(ok * 100 >= total * 95, "delivered {ok} of {total}");
    }

    #[test]
    fn s39_fading_loses_bursts_but_messages_still_arrive() {
        let mut ok = 0;
        for seed in 1..=20u64 {
            let mut net = Net::full(3, 15.0, seed);
            net.fade_loss = 0.3;
            net.send(0, 2, 1, 3);
            net.send(1, 2, 2, 3);
            net.run(20 * 60_000);
            ok += net.delivered_to(2);
        }
        assert!(ok >= 38, "delivered {ok} of 40 with 30 percent fading");
    }

    #[test]
    fn a_single_message_goes_through_on_a_quiet_channel() {
        let mut net = Net::full(2, 15.0, 7);
        net.send(0, 1, 1, 4);
        net.run(120_000);
        assert_eq!(net.delivered_to(1), 1);
        assert_eq!(net.collisions, 0);
    }
}
