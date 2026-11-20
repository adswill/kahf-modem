//! Keeps this relay registered on the relay server and its schedule fresh.
//!
//! [`Engine::start_server_sync`] starts a background thread that runs
//! [`kahf_net::RelaySync`]. It talks to the server only while relay mode is
//! running, hands each new schedule to the relay (as
//! [`Engine::accept_relay_schedule`] does) and writes what happened to the
//! event log. Dropping the returned [`ServerSync`] stops the thread; so does
//! dropping the engine.

use super::{lock, Engine, Event, Shared};
use kahf_core::relay::Direction;
use kahf_net::{Area, Client, HttpTransport, RelaySync, SlotRequest, Step, SyncConfig, Transport};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Length of each default slot, in seconds.
pub const DEFAULT_SLOT_S: u32 = 60;

/// Receive and transmit pairs asked for by default.
const DEFAULT_PAIRS: usize = 3;

/// Longest single wait, so a stopped engine or relay is noticed quickly.
const MAX_WAIT: Duration = Duration::from_secs(1);

/// Settings for [`Engine::start_server_sync`].
#[derive(Clone, Debug)]
pub struct ServerSyncOptions {
    /// Server root, normally [`kahf_net::DEFAULT_BASE_URL`].
    pub base_url: String,
    /// The one-degree square this relay serves.
    pub area: Area,
    /// Slots to ask for. Empty: receive and transmit pairs of
    /// [`DEFAULT_SLOT_S`] seconds on the relay's channels in turn.
    pub requests: Vec<SlotRequest>,
}

/// What the sync last did, for the app to show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SyncStatus {
    pub registered: bool,
    /// Unix time of the last schedule received.
    pub last_schedule_unix: Option<u64>,
    /// Slots in the last schedule received.
    pub slots: usize,
    /// Text of the last failure; cleared by the next success.
    pub last_error: Option<String>,
    /// Unix time of the next request.
    pub next_attempt_unix: u64,
}

/// Handle to the sync thread. Dropping it stops the thread.
pub struct ServerSync {
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
    status: Arc<Mutex<SyncStatus>>,
}

impl ServerSync {
    pub fn status(&self) -> SyncStatus {
        lock(&self.status).clone()
    }
}

impl Drop for ServerSync {
    fn drop(&mut self) {
        // Closing the channel wakes the thread at once.
        drop(self.stop.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Engine {
    /// Start keeping this relay registered and scheduled on the relay server.
    /// Needs a relay role (see `configure_relay`) and the station key.
    pub fn start_server_sync(&self, opts: ServerSyncOptions) -> Result<ServerSync, String> {
        self.start_server_sync_with(opts, HttpTransport::default())
    }

    fn start_server_sync_with<T: Transport + Send + 'static>(
        &self,
        opts: ServerSyncOptions,
        transport: T,
    ) -> Result<ServerSync, String> {
        if self.shared.station_key.is_none() {
            return Err("The station key could not be loaded, so the relay server cannot be used.".into());
        }
        let client = Client::new(transport, &opts.base_url);
        let requests = if opts.requests.is_empty() {
            let channels = lock(&self.shared.relay)
                .as_ref()
                .map(|r| r.channels_khz().to_vec())
                .ok_or("Set a relay role before connecting to the relay server.")?;
            default_requests(&channels)
        } else {
            opts.requests
        };
        let cfg = SyncConfig::new(&self.config.callsign, opts.area, requests);

        let (stop, stop_rx) = channel::<()>();
        let status = Arc::new(Mutex::new(SyncStatus::default()));
        let shared = Arc::downgrade(&self.shared);
        let thread_status = status.clone();
        let thread = thread::Builder::new()
            .name("kahf-server-sync".into())
            .spawn(move || {
                let mut sync = RelaySync::new(cfg);
                loop {
                    let wait = match run_once(&shared, &client, &mut sync, &thread_status) {
                        Some(wait) => wait,
                        None => return, // the engine is gone
                    };
                    match stop_rx.recv_timeout(wait) {
                        Err(RecvTimeoutError::Timeout) => {}
                        _ => return,
                    }
                }
            })
            .map_err(|e| format!("could not start the relay server thread: {e}"))?;
        Ok(ServerSync {
            stop: Some(stop),
            thread: Some(thread),
            status,
        })
    }
}

/// Run every step that is due, then say how long to wait. None when the engine is gone.
fn run_once<T: Transport>(
    shared: &Weak<Shared>,
    client: &Client<T>,
    sync: &mut RelaySync,
    status: &Mutex<SyncStatus>,
) -> Option<Duration> {
    let shared = shared.upgrade()?;
    if !shared
        .flags
        .relay_running
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        return Some(MAX_WAIT);
    }
    let key = shared.station_key.as_ref()?;
    loop {
        let now = now_unix();
        let step = sync.step(client, key, now);
        let mut st = lock(status);
        st.registered = sync.is_registered();
        st.next_attempt_unix = sync.next_attempt_unix();
        match step {
            Step::Waiting => {
                let secs = sync.next_attempt_unix().saturating_sub(now);
                return Some(Duration::from_secs(secs).min(MAX_WAIT));
            }
            Step::Registered(r) => {
                st.last_error = None;
                log(
                    &shared,
                    format!("Relay server: registered {} in area {}.", r.callsign, r.area),
                );
            }
            Step::Schedule(s) => {
                st.last_error = None;
                st.last_schedule_unix = Some(now);
                st.slots = s.slots.len();
                let accepted = match lock(&shared.relay).as_mut() {
                    Some(relay) => relay.accept_schedule(s.slots),
                    None => Err("no relay role is set".to_string()),
                };
                match accepted {
                    Ok(()) => log(&shared, format!("Relay server: {} slots received.", st.slots)),
                    Err(e) => {
                        st.last_error = Some(e.clone());
                        log(&shared, format!("Relay server: schedule not used: {e}"));
                    }
                }
                for u in &s.unplaced {
                    log(
                        &shared,
                        format!("Relay server: request {} not placed: {}", u.index + 1, u.reason),
                    );
                }
            }
            Step::Failed(e) => {
                let text = e.to_string();
                // Log a failure once, not on every retry.
                if st.last_error.as_deref() != Some(text.as_str()) {
                    log(&shared, format!("{text}. Retrying in the background."));
                }
                st.last_error = Some(text);
            }
        }
    }
}

/// Receive then transmit, on each channel in turn, so a half-duplex radio alternates.
fn default_requests(channels_khz: &[f32]) -> Vec<SlotRequest> {
    let slot = |channel_khz, direction| SlotRequest {
        channel_khz,
        duration_s: DEFAULT_SLOT_S,
        direction,
        urgent: false,
        latency_budget_s: None,
    };
    channels_khz
        .iter()
        .cycle()
        .take(DEFAULT_PAIRS)
        .flat_map(|&c| [slot(c, Direction::Receive), slot(c, Direction::Transmit)])
        .collect()
}

fn log(shared: &Shared, line: String) {
    lock(&shared.events).push(Event::Log(line));
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, RelayAck};
    use kahf_core::relay::{self, Role};
    use kahf_net::{ChannelGrid, Response};
    use std::collections::VecDeque;
    use std::time::Instant;

    /// Answers from a shared queue; an empty queue is "offline".
    #[derive(Clone, Default)]
    struct Fake {
        answers: Arc<Mutex<VecDeque<(u16, String)>>>,
        sent: Arc<Mutex<Vec<String>>>,
    }

    impl Transport for Fake {
        fn post_json(&self, url: &str, _body: &str) -> Result<Response, String> {
            self.sent
                .lock()
                .unwrap()
                .push(url.rsplit('/').next().unwrap().to_string());
            match self.answers.lock().unwrap().pop_front() {
                Some((status, body)) => Ok(Response { status, body }),
                None => Err("offline".into()),
            }
        }
    }

    fn engine(tag: &str) -> Engine {
        let dir = std::env::temp_dir().join(format!("kahf-sync-test-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Engine::new(Config {
            callsign: "LY1ABC".into(),
            data_dir: dir,
            ..Default::default()
        })
        .unwrap()
    }

    fn opts() -> ServerSyncOptions {
        ServerSyncOptions {
            base_url: "https://kahf.test".into(),
            area: Area::parse("N54E25").unwrap(),
            requests: Vec::new(),
        }
    }

    fn wait_for(what: &str, cond: impl Fn() -> bool) {
        let end = Instant::now() + Duration::from_secs(5);
        while !cond() {
            assert!(Instant::now() < end, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn centres() -> Vec<f32> {
        (0..3)
            .map(|c| ChannelGrid::RELAY_17M.centre_khz(c).unwrap())
            .collect()
    }

    #[test]
    fn needs_a_relay_role() {
        let e = engine("no-role");
        assert!(e.start_server_sync_with(opts(), Fake::default()).is_err());
    }

    #[test]
    fn schedule_from_the_server_reaches_the_relay() {
        let mut e = engine("schedule");
        e.configure_relay(Role::Relay, relay::Radio::HalfDuplex, &centres())
            .unwrap();
        let fake = Fake::default();
        let start = now_unix() + 30;
        fake.answers.lock().unwrap().extend([
            (200, r#"{"ok":true,"callsign":"LY1ABC","area":"N54E25","expires_after_s":172800}"#.to_string()),
            (
                200,
                format!(
                    r#"{{"relay":"LY1ABC","slots":[{{"channel":0,"start":{start},"duration":60,"direction":"rx"}},{{"channel":0,"start":{},"duration":60,"direction":"tx"}}],"unplaced":[]}}"#,
                    start + 60
                ),
            ),
        ]);
        let sync = e.start_server_sync_with(opts(), fake.clone()).unwrap();

        thread::sleep(Duration::from_millis(100));
        assert!(
            fake.sent.lock().unwrap().is_empty(),
            "nothing is sent while relay mode is off"
        );

        e.start_relay(RelayAck {
            licence_held: true,
            terms_accepted: true,
        })
        .unwrap();
        wait_for("the schedule", || sync.status().slots == 2);
        let slots = lock(&e.shared.relay).as_ref().unwrap().slots().to_vec();
        assert_eq!(slots.len(), 2);
        assert_eq!(slots[0].start_unix, start);
        assert_eq!(*fake.sent.lock().unwrap(), ["register", "schedule"]);
        let st = sync.status();
        assert!(st.registered && st.last_error.is_none());
        let logs: Vec<_> = e
            .poll_events()
            .into_iter()
            .filter_map(|ev| match ev {
                Event::Log(l) => Some(l),
                _ => None,
            })
            .collect();
        assert!(logs.iter().any(|l| l.contains("2 slots received")), "{logs:?}");
    }

    #[test]
    fn offline_is_reported_once_and_dropping_stops_the_thread() {
        let mut e = engine("offline");
        e.configure_relay(Role::Relay, relay::Radio::HalfDuplex, &centres())
            .unwrap();
        e.start_relay(RelayAck {
            licence_held: true,
            terms_accepted: true,
        })
        .unwrap();
        let fake = Fake::default();
        let sync = e.start_server_sync_with(opts(), fake.clone()).unwrap();
        wait_for("a failure", || sync.status().last_error.is_some());
        assert!(!sync.status().registered);
        assert!(
            sync.status().next_attempt_unix > now_unix(),
            "backs off before the next try"
        );

        let started = Instant::now();
        drop(sync);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "drop stops the thread at once"
        );
        assert_eq!(
            fake.sent.lock().unwrap().len(),
            1,
            "no retry before the back-off ends"
        );
        let offline_logs = e
            .poll_events()
            .into_iter()
            .filter(|ev| matches!(ev, Event::Log(l) if l.contains("not reachable")))
            .count();
        assert_eq!(offline_logs, 1);
    }

    #[test]
    fn default_requests_alternate_receive_and_transmit() {
        let reqs = default_requests(&[1.0, 2.0]);
        let got: Vec<_> = reqs.iter().map(|r| (r.channel_khz, r.direction)).collect();
        use Direction::{Receive as R, Transmit as T};
        assert_eq!(got, [(1.0, R), (1.0, T), (2.0, R), (2.0, T), (1.0, R), (1.0, T)]);
    }
}
