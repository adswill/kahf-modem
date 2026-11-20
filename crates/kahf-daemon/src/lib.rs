//! KAHF engine: the one API that the GUI, the CLI and the JSON server all use.
//! Owner: daemon agent (implements); GUI agent consumes this signature list as is.
//!
//! `Engine` is a handle. A worker thread owns the link state, the radio (sound
//! card or SDR) and the TX/RX pipeline. Calls from the caller's thread send a
//! command to the worker and wait only for the quick part (validation, opening
//! the radio on first use, queueing). Events come back through a channel and
//! are drained with `poll_events`.
use std::collections::{HashMap, HashSet, VecDeque};
use std::f64::consts::PI;
use std::fs;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use kahf_audio::{AudioIn, AudioOut, Resampler, RATE};
use kahf_core::access::{self, CarrierSense};
use kahf_core::auth::{self, StationKey};
use kahf_core::frame;
use kahf_core::io::{IqSink, RigControl, RigctldClient};
use kahf_core::ldpc;
use kahf_core::link::{Action, Link, LinkState};
use kahf_core::outbox::{OutItem, Outbox, Priority};
use kahf_core::phy::{self, Modulation};
use kahf_core::relay::{self, Hop, HopLog, Relay, Role, Slot, Stats};
use kahf_core::select::{self, Refusal};
use kahf_core::sense::Sense;
use kahf_core::session::{self, Message};
use kahf_core::spectrum::Analyser;
use kahf_log::contacts::{self, Contact};
use kahf_log::{valid_call, Qso};
use kahf_prop::route;
use kahf_radio::{GainSetting, RadioSpec, RxRadio};
use kahf_sdr::hackrf::HackrfTx;
use sha2::{Digest, Sha256};

mod server_sync;
pub use kahf_net::{Area, SlotRequest, DEFAULT_BASE_URL as RELAY_SERVER_URL};
pub use server_sync::{ServerSync, ServerSyncOptions, SyncStatus, DEFAULT_SLOT_S};

/// What the operator confirms before relay mode starts. Both must be true.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RelayAck {
    /// The operator holds an amateur licence that covers this station.
    pub licence_held: bool,
    /// The operator accepts the terms of relay operation.
    pub terms_accepted: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub callsign: String,
    pub audio_in: Option<String>,
    pub audio_out: Option<String>,
    pub rigctld: Option<String>,
    pub sdr: Option<String>,
    pub data_dir: PathBuf,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Link(kahf_core::link::LinkState),
    Message {
        from: String,
        text: String,
    },
    Mail {
        id: u64,
        from: String,
        subject: String,
    },
    FileProgress {
        id: u64,
        name: String,
        done: u64,
        total: u64,
    },
    Spectrum(Vec<f32>),
    Log(String),
    /// Transmit progress: Starting (PTT on), Sending (audio handed to the radio),
    /// Done (PTT off) or Failed (with the reason).
    Tx {
        state: TxState,
    },
    /// Radio relay of another station's message by this station (plan 4).
    Relay {
        from: String,
        to: String,
        step: RelayStep,
    },
}

/// What happened to a message this station relays.
#[derive(Clone, Debug, PartialEq)]
pub enum RelayStep {
    /// Heard and held, waiting for a transmit slot and its station.
    Held,
    /// Handed on to the next station.
    Sent,
    /// Not held, with the reason (a copy already relayed, or the relay queue is full).
    Dropped(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum TxState {
    Starting,
    Sending,
    Done,
    Failed(String),
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MODULATION: Modulation = Modulation::Qpsk;
const LDPC_ITER: usize = 50;
/// Largest payload one link frame carries (see frame::build_payload).
const MAX_CHUNK: usize = 36;
/// Preamble symbol length in audio samples (FFT 256 + cyclic prefix 32).
const PREAMBLE_LEN: usize = 288;
const PREAMBLE_THRESHOLD: f64 = 0.5;
const LEAD_SILENCE: usize = RATE as usize / 5; // lets PTT and VOX settle
const GAP_SILENCE: usize = RATE as usize / 10;
const TAIL_SILENCE: usize = RATE as usize / 10;
/// Upper bound on buffered session chunks before a partial message is dropped.
const MAX_CHUNKS: usize = 4096;
/// Run the frame scanner every 0.1 s of new audio.
const SCAN_HOP: usize = RATE as usize / 10;
/// Spectrum: 2048-point FFT, 1024 bins (0 to 4 kHz at 8 kHz audio).
const SPEC_FFT: usize = 2048;
const SPEC_BINS: usize = SPEC_FFT / 2;
/// New spectrum frame at most every 40 ms (25 per second), on the worker clock.
const SPEC_PERIOD: Duration = Duration::from_millis(40);
/// Event queue: Log and FileProgress events beyond this many are dropped, oldest first.
const EVENT_CAP: usize = 1024;
/// TX is written in chunks of this many audio samples (0.1 s).
const TX_CHUNK: usize = RATE as usize / 10;
/// Audio is handed to the device at most this far ahead of real time.
const TX_AHEAD_S: f64 = 1.0;
/// PTT stays on this long after the last sample's nominal end.
const TX_TAIL_S: f64 = 0.1;
/// Wait after a failed RX read before trying the radio again.
const RX_RETRY: Duration = Duration::from_millis(200);

/// HackRF sample rate (2.4 Msps = 300 x 8 kHz). Transmit is single sideband: I is the audio, Q its Hilbert transform.
const SDR_RATE_HZ: u32 = 2_400_000;
const SDR_OVERSAMPLE: usize = (SDR_RATE_HZ / RATE) as usize;
/// Half-length of the interpolation filter, in audio samples.
const SDR_HALF: usize = 8;
/// Used when the sdr setting has no "@frequency" part: 18.111 MHz USB, the low edge of the
/// 17 m relay segment, so its first channel sits at 1.35 kHz audio.
const DEFAULT_SDR_FREQ_HZ: u64 = 18_111_000;
const SDR_RX_LNA_DB: u32 = 16;
const SDR_RX_VGA_DB: u32 = 20;
const SDR_TX_VGA_DB: u32 = 20;

const POLL: Duration = Duration::from_millis(5);
const STOPPED: &str = "engine stopped";

/// Plan 6.3: how long a message waits for its station to be reached on air.
const RELAY_TIMEOUT_MS: u64 = 120_000;
/// Most messages a relay holds for other stations at once (plan 4).
const MAX_HELD_FORWARDS: usize = 64;
/// How many relayed messages a relay remembers, so it passes each one on only once.
const RELAYED_MEMORY: usize = 1024;
/// A relay calls the same station again at most this often.
const DIAL_INTERVAL: Duration = Duration::from_secs(30);
/// Outbox kind of a message held for another station.
const KIND_RELAY: &str = "relay";
const LISTEN_ONLY_TEXT: &str = "Listen only is on. Turn it off in Settings before you transmit.";
const UPDATE_LOCK_TEXT: &str = "A required update is pending. Install it before you transmit.";

/// Text for a message whose station was not reached and no relay path is on (plan 6.3).
fn unreached_text(call: &str) -> String {
    format!(
        "Could not reach {call} on air. The station did not respond on any relay path. Try again later, or enable internet relay in Settings."
    )
}

// ---------------------------------------------------------------------------
// Public handle
// ---------------------------------------------------------------------------

type Factory = Box<dyn Fn() -> Result<Box<dyn Backend>, String> + Send>;

/// State shared by the worker and the handle. Locks are held only for a push or a copy.
struct Shared {
    events: Mutex<EventQueue>,
    /// Outbox snapshot for `outbox_pending`, written by the worker.
    pending: Mutex<Vec<(u64, String)>>,
    flags: Flags,
    /// Station key from the data folder, loaded at start-up. None when it could not be loaded.
    station_key: Option<StationKey>,
    /// Why the station key could not be loaded; logged once by the worker.
    key_error: Mutex<Option<String>>,
    /// Relay role and schedule (plan 4). None until a role is configured.
    relay: Mutex<Option<Relay>>,
    /// Forwards sent by this station as a relay (plan 4).
    relay_stats: Mutex<Stats>,
    /// One entry per forward, keyed by the public message id.
    hops: Mutex<HopLog>,
    /// Relays this station may send through when a station is not reached directly (plan 6.3).
    relay_paths: Mutex<Vec<String>>,
}

/// Settings the handle sets and the worker reads. All off by default.
struct Flags {
    /// Outgoing mail is signed with the station key (plan 4). On by default.
    sign_mail: AtomicBool,
    /// Relay mode is running. Set by `start_relay`, cleared by `stop_relay`. Never saved.
    relay_running: AtomicBool,
    /// The operator started a connection and has not disconnected. Only then do outbox
    /// sends go out without relay mode.
    operator_link: AtomicBool,
    listen_only: AtomicBool,
    /// Weak mode: every frame is sent WEAK_COPIES times for stations that cannot reply.
    weak_mode: AtomicBool,
    update_locked: AtomicBool,
    internet_relay: AtomicBool,
    /// Relay other stations' messages (setting B). On by default; used only in relay mode.
    third_party_relay: AtomicBool,
    relay_timeout_ms: AtomicU64,
}

impl Flags {
    fn new() -> Self {
        Flags {
            sign_mail: AtomicBool::new(true),
            relay_running: AtomicBool::new(false),
            operator_link: AtomicBool::new(false),
            listen_only: AtomicBool::new(false),
            weak_mode: AtomicBool::new(false),
            update_locked: AtomicBool::new(false),
            // Setting D: on by default. The gateway is a stub, so nothing leaves the station
            // until the gateway exists and the operator sends that way.
            internet_relay: AtomicBool::new(true),
            third_party_relay: AtomicBool::new(true),
            relay_timeout_ms: AtomicU64::new(RELAY_TIMEOUT_MS),
        }
    }
}

/// Why nothing may transmit right now, if anything. A required update wins over Listen only.
fn tx_refusal(f: &Flags) -> Option<&'static str> {
    if f.update_locked.load(Ordering::SeqCst) {
        Some(UPDATE_LOCK_TEXT)
    } else if f.listen_only.load(Ordering::SeqCst) {
        Some(LISTEN_ONLY_TEXT)
    } else {
        None
    }
}

/// Plan 6.2: route a message through the server gateway. Stub until the server exists.
fn relay_via_gateway(_dest: &str) -> Result<(), String> {
    // Parked: radio relay comes first (owner's decision, 2026-11-20). When the kahf.dev
    // gateway exists (plan 5 and 6.2), send there and return Ok once it accepts.
    Err("the internet gateway is not available yet".into())
}

#[derive(Default)]
struct EventQueue {
    items: VecDeque<Event>,
    /// Only the latest spectrum frame is kept; older ones are dropped.
    spectrum: Option<Vec<f32>>,
}

impl EventQueue {
    fn push(&mut self, ev: Event) {
        if self.items.len() >= EVENT_CAP {
            // Drop a Log line or a FileProgress first. Message, Mail, Link and Tx are never dropped.
            let victim = self
                .items
                .iter()
                .position(|e| matches!(e, Event::Log(_)))
                .or_else(|| {
                    self.items
                        .iter()
                        .position(|e| matches!(e, Event::FileProgress { .. }))
                });
            if let Some(i) = victim {
                self.items.remove(i);
            }
        }
        self.items.push_back(ev);
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

pub struct Engine {
    pub config: Config,
    cmd: Sender<Cmd>,
    shared: Arc<Shared>,
    ids: Arc<AtomicU64>,
    worker: Option<JoinHandle<()>>,
}

impl Engine {
    /// Validates the config. No device is opened here: the radio opens on the
    /// first connect or send, so the GUI runs without hardware.
    pub fn new(config: Config) -> Result<Self, String> {
        validate(&config)?;
        let radio_cfg = config.clone();
        Self::spawn(config, Box::new(move || open_radio(&radio_cfg)))
    }

    /// Returns at once. A failure to reach the peer or open the radio shows up as an Event::Log.
    pub fn connect(&mut self, peer: &str) -> Result<(), String> {
        let peer = peer.trim().to_string();
        if peer.is_empty() || peer == self.config.callsign {
            return Err(format!("bad peer callsign {peer:?}"));
        }
        if let Some(why) = tx_refusal(&self.shared.flags) {
            return Err(why.into());
        }
        self.shared.flags.operator_link.store(true, Ordering::SeqCst);
        self.send_cmd(Cmd::Connect { peer })
    }

    pub fn disconnect(&mut self) -> Result<(), String> {
        self.shared.flags.operator_link.store(false, Ordering::SeqCst);
        self.send_cmd(Cmd::Disconnect)
    }

    /// Listen only (plan 0): while on, no transmit path runs. Receive keeps working.
    pub fn set_listen_only(&self, on: bool) {
        self.shared.flags.listen_only.store(on, Ordering::SeqCst);
    }

    /// Weak mode: sends every frame WEAK_COPIES times in QPSK, so it decodes at about -4 dB.
    /// Much slower, and the receiver gets no acknowledgements in time to repeat anything.
    pub fn set_weak_mode(&self, on: bool) {
        self.shared.flags.weak_mode.store(on, Ordering::SeqCst);
    }

    /// Set while a required update is pending (plan 0 and 8.5). While set, no transmit path runs.
    pub fn set_update_locked(&self, on: bool) {
        self.shared.flags.update_locked.store(on, Ordering::SeqCst);
    }

    /// Mail signing (plan 4). On by default; the station key signs each outgoing mail body.
    pub fn set_mail_signing(&self, on: bool) {
        self.shared.flags.sign_mail.store(on, Ordering::SeqCst);
    }

    /// Relay role (plan 4): sets the role, the radio class and the assigned channels in kHz.
    /// Replaces any earlier role and schedule.
    pub fn configure_relay(
        &self,
        role: Role,
        radio: relay::Radio,
        channels_khz: &[f32],
    ) -> Result<(), String> {
        let r = Relay::new(&self.config.callsign, role, radio, channels_khz)?;
        *lock(&self.shared.relay) = Some(r);
        Ok(())
    }

    /// Starts relay mode for this run only (plan 4). Refused unless a relay role is set and
    /// both acknowledgements are true. Nothing is saved: relay stops on `stop_relay` or when
    /// the app quits.
    pub fn start_relay(&self, ack: RelayAck) -> Result<(), String> {
        if !(ack.licence_held && ack.terms_accepted) {
            return Err(
                "Relay mode did not start. Confirm that you hold a licence and accept the terms, then try again."
                    .to_string(),
            );
        }
        if lock(&self.shared.relay).is_none() {
            return Err("Relay mode did not start. Set a relay role first.".to_string());
        }
        self.shared.flags.relay_running.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Stops relay mode (plan 4). Unattended sends stop with it.
    pub fn stop_relay(&self) {
        self.shared.flags.relay_running.store(false, Ordering::SeqCst);
    }

    /// True while relay mode is running (plan 4).
    pub fn relay_running(&self) -> bool {
        self.shared.flags.relay_running.load(Ordering::SeqCst)
    }

    /// Relay schedule from the server (plan 4). A rejected schedule leaves the old one in place.
    pub fn accept_relay_schedule(&self, slots: Vec<Slot>) -> Result<(), String> {
        match lock(&self.shared.relay).as_mut() {
            Some(r) => r.accept_schedule(slots),
            None => Err("no relay role is set; configure one first".to_string()),
        }
    }

    /// Forward counters for this station as a relay (plan 4).
    pub fn relay_stats(&self) -> Stats {
        lock(&self.shared.relay_stats).clone()
    }

    /// Forwards recorded for one message, in order (plan 4). `message_id` is the public id.
    pub fn relay_hops(&self, message_id: u64) -> Vec<Hop> {
        lock(&self.shared.hops).chain_for(message_id)
    }

    /// Relays this station may send through by radio when a station is not reached directly
    /// (plan 6.3). A message waits `RELAY_TIMEOUT_MS` for its station; after that it goes to
    /// whichever of these relays the station is connected to. An empty list turns it off.
    pub fn set_relay_paths(&self, calls: &[&str]) -> Result<(), String> {
        let mut paths: Vec<String> = Vec::new();
        for call in calls {
            let call = call.trim();
            if !valid_call(call) {
                return Err(format!("bad relay callsign {call:?}"));
            }
            if call.eq_ignore_ascii_case(&self.config.callsign) {
                return Err("a station cannot relay through itself".to_string());
            }
            if !paths.iter().any(|p| p.eq_ignore_ascii_case(call)) {
                paths.push(call.to_string());
            }
        }
        *lock(&self.shared.relay_paths) = paths;
        Ok(())
    }

    /// Sets the relay paths from the 17 m route planner: for each of the next 24 hours, the
    /// first relay of the best predicted route from `me` to `to` (kahf_prop::route). Returns
    /// the paths set, best first; empty when no route is predicted at any hour.
    pub fn plan_relay_paths(
        &self,
        me: &route::Node,
        to: &route::Node,
        relays: &[route::Node],
        conditions: &route::Conditions,
    ) -> Result<Vec<String>, String> {
        let paths = route::first_relays(me, to, relays, conditions);
        let calls: Vec<&str> = paths.iter().map(String::as_str).collect();
        self.set_relay_paths(&calls)?;
        Ok(self.relay_paths())
    }

    /// The relays set with `set_relay_paths`.
    pub fn relay_paths(&self) -> Vec<String> {
        lock(&self.shared.relay_paths).clone()
    }

    /// The last `limit` forwards of any message, newest first (plan 4).
    pub fn recent_hops(&self, limit: usize) -> Vec<Hop> {
        lock(&self.shared.hops).recent(limit)
    }

    /// The accepted relay schedule in time order. Empty until a role and a schedule are set.
    pub fn relay_schedule(&self) -> Vec<Slot> {
        lock(&self.shared.relay)
            .as_ref()
            .map(|r| r.slots().to_vec())
            .unwrap_or_default()
    }

    /// Internet relay (plan 6.1). On by default (setting D). The gateway is still a stub:
    /// nothing goes over the internet until it exists and the operator sends a message that way.
    /// When off, no traffic leaves over the internet.
    pub fn set_internet_relay(&self, on: bool) {
        self.shared.flags.internet_relay.store(on, Ordering::SeqCst);
    }

    /// Label of setting B, shown next to the switch.
    pub const THIRD_PARTY_RELAY_LABEL: &'static str = "Relay other stations' messages";

    /// Third-party relaying (setting B). On by default. While it is on and relay mode runs, a
    /// station with a relay role holds messages addressed to other stations and passes them
    /// on unchanged in its transmit slots. The operator confirms the licence each start.
    pub fn set_third_party_relay(&self, on: bool) {
        self.shared.flags.third_party_relay.store(on, Ordering::SeqCst);
    }

    /// True when setting B is on.
    pub fn third_party_relay(&self) -> bool {
        self.shared.flags.third_party_relay.load(Ordering::SeqCst)
    }

    /// Test only: how long a message waits for its station before the relay step.
    #[cfg(test)]
    fn set_relay_timeout_ms(&self, ms: u64) {
        self.shared.flags.relay_timeout_ms.store(ms, Ordering::SeqCst);
    }

    /// Opens the radio if it is not open yet and starts receiving, without connecting.
    /// Safe to call more than once. Returns at once; an open failure is an Event::Log.
    pub fn listen(&mut self) -> Result<(), String> {
        self.send_cmd(Cmd::Listen)
    }

    /// Queues a chat message. Text starting with "URGENT:" goes out first.
    pub fn send_message(&mut self, to: &str, text: &str) -> Result<(), String> {
        check_to(to)?;
        if text.is_empty() {
            return Err("empty message".into());
        }
        let msg = Message::Chat {
            from: self.config.callsign.clone(),
            to: to.trim().to_string(),
            text: text.to_string(),
        };
        self.queue(vec![msg]).map(|_| ())
    }

    /// Queues a chat message marked private: on air it carries the session private bit, and
    /// only the addressee's station keeps it. Not encryption; the text is readable on air.
    pub fn send_message_private(&mut self, to: &str, text: &str) -> Result<(), String> {
        check_to(to)?;
        if text.is_empty() {
            return Err("empty message".into());
        }
        let msg = Message::Chat {
            from: self.config.callsign.clone(),
            to: to.trim().to_string(),
            text: text.to_string(),
        };
        self.queue_with(vec![msg], true).map(|_| ())
    }

    /// Queues a mail. Each attachment is also queued as a file right after it.
    /// Returns the outbox id of the mail.
    pub fn send_mail(
        &mut self,
        to: &str,
        subject: &str,
        body: &str,
        attachments: &[PathBuf],
    ) -> Result<u64, String> {
        check_to(to)?;
        let files = attachments
            .iter()
            .map(|p| read_attachment(p))
            .collect::<Result<Vec<_>, String>>()?;
        let names = files.iter().map(|(n, _)| n.clone()).collect();
        let to = to.trim().to_string();
        let from = self.config.callsign.clone();
        let body = match (
            &self.shared.station_key,
            self.shared.flags.sign_mail.load(Ordering::SeqCst),
        ) {
            (Some(key), true) => sign_body(key, &from, body),
            _ => body.to_string(),
        };
        let mut msgs = vec![Message::Mail {
            from: from.clone(),
            to: to.clone(),
            subject: subject.to_string(),
            body,
            attachments: names,
        }];
        msgs.extend(files.into_iter().map(|(name, data)| Message::File {
            from: from.clone(),
            to: to.clone(),
            name,
            data,
        }));
        let ids = self.queue(msgs)?;
        ids.first().copied().ok_or_else(|| STOPPED.to_string())
    }

    /// Queues one file. Returns the outbox id, also used in FileProgress events.
    pub fn send_file(&mut self, to: &str, path: &Path) -> Result<u64, String> {
        check_to(to)?;
        let (name, data) = read_attachment(path)?;
        let msg = Message::File {
            from: self.config.callsign.clone(),
            to: to.trim().to_string(),
            name,
            data,
        };
        let ids = self.queue(vec![msg])?;
        ids.first().copied().ok_or_else(|| STOPPED.to_string())
    }

    /// Events since the last call, without waiting. Spectrum comes as the latest frame only.
    pub fn poll_events(&mut self) -> Vec<Event> {
        let mut q = lock(&self.shared.events);
        let mut out: Vec<Event> = q.items.drain(..).collect();
        if let Some(bins) = q.spectrum.take() {
            out.push(Event::Spectrum(bins));
        }
        out
    }

    /// Items still waiting in the outbox as (id, short description), in the order
    /// they will go out (urgent first). Items not yet sent are kept across restarts.
    /// Reads the worker's last snapshot, so it may lag a queued command by a moment.
    pub fn outbox_pending(&self) -> Vec<(u64, String)> {
        lock(&self.shared.pending).clone()
    }

    fn spawn(config: Config, factory: Factory) -> Result<Self, String> {
        let (cmd, cmd_rx) = channel();
        // Plan 4: the station key is loaded at start-up. A failure is not fatal: mail goes unsigned.
        let (station_key, key_error) = match auth::load_or_create(&config.data_dir) {
            Ok(k) => (Some(k), None),
            Err(e) => (
                None,
                Some(format!("station key not loaded, mail will not be signed: {e}")),
            ),
        };
        let shared = Arc::new(Shared {
            events: Mutex::new(EventQueue::default()),
            pending: Mutex::new(Vec::new()),
            flags: Flags::new(),
            station_key,
            key_error: Mutex::new(key_error),
            relay: Mutex::new(None),
            relay_stats: Mutex::new(Stats::default()),
            hops: Mutex::new(HopLog::new()),
            relay_paths: Mutex::new(Vec::new()),
        });
        let ids = Arc::new(AtomicU64::new(1));
        let engine_config = config.clone();
        let (worker_shared, worker_ids) = (shared.clone(), ids.clone());
        let worker = thread::Builder::new()
            .name("kahf-engine".into())
            .spawn(move || Worker::new(config, cmd_rx, worker_shared, factory, worker_ids).run())
            .map_err(|e| format!("cannot start engine thread: {e}"))?;
        Ok(Engine {
            config: engine_config,
            cmd,
            shared,
            ids,
            worker: Some(worker),
        })
    }

    /// Hands messages to the worker, which puts them in the outbox in order. Returns
    /// their ids at once: the ids come from a shared counter, not from the worker.
    fn queue(&self, msgs: Vec<Message>) -> Result<Vec<u64>, String> {
        self.queue_with(msgs, false)
    }

    /// As `queue`, with the session private bit set on every message when `private` is true.
    fn queue_with(&self, msgs: Vec<Message>, private: bool) -> Result<Vec<u64>, String> {
        if let Some(why) = tx_refusal(&self.shared.flags) {
            return Err(why.into());
        }
        let ids: Vec<u64> = msgs
            .iter()
            .map(|_| self.ids.fetch_add(1, Ordering::Relaxed))
            .collect();
        self.cmd
            .send(Cmd::Queue {
                msgs,
                ids: ids.clone(),
                private,
            })
            .map_err(|_| STOPPED.to_string())?;
        Ok(ids)
    }

    /// Enqueues a command for the worker and returns without waiting for it.
    fn send_cmd(&self, cmd: Cmd) -> Result<(), String> {
        self.cmd.send(cmd).map_err(|_| STOPPED.to_string())
    }

    /// Test only: hands the worker audio to transmit as if the link had sent it.
    #[cfg(test)]
    fn test_burst(&self, samples: Vec<f32>) -> Result<(), String> {
        self.send_cmd(Cmd::TestBurst(samples))
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.cmd.send(Cmd::Shutdown);
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

fn check_to(to: &str) -> Result<(), String> {
    if to.trim().is_empty() {
        Err("no destination callsign".into())
    } else {
        Ok(())
    }
}

fn read_attachment(path: &Path) -> Result<(String, Vec<u8>), String> {
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .ok_or_else(|| format!("{} has no file name", path.display()))?;
    let data = fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    Ok((name, data))
}

fn validate(c: &Config) -> Result<(), String> {
    let cs = c.callsign.trim();
    if cs.is_empty() {
        return Err("callsign is empty".into());
    }
    if cs != c.callsign || cs.chars().any(char::is_whitespace) || cs.chars().count() > 32 {
        return Err(format!("bad callsign {:?}", c.callsign));
    }
    for (name, dev) in [("audio_in", &c.audio_in), ("audio_out", &c.audio_out)] {
        if let Some(d) = dev {
            if d.trim().is_empty() {
                return Err(format!("{name} is an empty device name"));
            }
        }
    }
    if let Some(addr) = &c.rigctld {
        // Either a rig spec ("ic7300:/dev/ttyUSB0:115200:cat", "flrig") or a rigctld host:port.
        if kahf_rig::parse_spec(addr).is_err() {
            match addr.rsplit_once(':') {
                Some((host, port)) if !host.is_empty() && port.parse::<u16>().is_ok() => {}
                _ => {
                    return Err(format!(
                        "bad rig setting {addr:?} (want host:port for rigctld, or model:device[:baud][:ptt]; models: {})",
                        kahf_rig::known_models()
                    ))
                }
            }
        }
    }
    if let Some(s) = &c.sdr {
        parse_sdr(s)?;
    }
    if c.data_dir.as_os_str().is_empty() {
        return Err("data_dir is empty".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Commands and queued work
// ---------------------------------------------------------------------------

enum Cmd {
    Connect {
        peer: String,
    },
    Disconnect,
    Listen,
    /// `ids[i]` is the public id of `msgs[i]`, allocated by the caller.
    /// `private`: the messages go on air with the session private bit.
    Queue {
        msgs: Vec<Message>,
        ids: Vec<u64>,
        private: bool,
    },
    Shutdown,
    #[cfg(test)]
    TestBurst(Vec<f32>),
}

// ---------------------------------------------------------------------------
// Radio backends
// ---------------------------------------------------------------------------

/// The worker's view of the radio. Samples are real, 8 kHz, mono.
/// Transmit is split into steps so the worker never sits inside a long device call:
/// `tx_start`, then `tx_write` as the device has room, then `tx_stop`.
trait Backend {
    /// Append the received audio that is available now. May block briefly.
    fn read_audio(&mut self, out: &mut Vec<f32>) -> Result<(), String>;
    /// Start a transmission: PTT on, or open the transmitter. No audio yet.
    fn tx_start(&mut self) -> Result<(), String>;
    /// Hand over `samples[start..start + n]`. Called only while the device has room.
    fn tx_write(&mut self, samples: &[f32], start: usize, end: usize) -> Result<(), String>;
    /// End the transmission: PTT off, transmitter closed. Called after every `tx_start`.
    fn tx_stop(&mut self) -> Result<(), String>;
}

fn open_radio(cfg: &Config) -> Result<Box<dyn Backend>, String> {
    if let Some(sdr) = &cfg.sdr {
        return Ok(Box::new(SdrBackend::new(parse_sdr(sdr)?)));
    }
    let input = AudioIn::open(cfg.audio_in.as_deref())?;
    let output = AudioOut::open(cfg.audio_out.as_deref())?;
    let rig: Option<Box<dyn RigControl>> = match &cfg.rigctld {
        Some(text) if kahf_rig::parse_spec(text).is_ok() => Some(kahf_rig::open(text)?),
        Some(addr) => Some(Box::new(RigctldClient::connect(addr)?)),
        None => None,
    };
    Ok(Box::new(AudioBackend {
        input: Some(input),
        output: Some(output),
        rig,
    }))
}

fn set_ptt(rig: &mut Option<Box<dyn RigControl>>, on: bool) -> Result<(), String> {
    match rig {
        Some(r) => r.set_ptt(on),
        None => Ok(()),
    }
}

/// Sound card in and out, with optional rig PTT (rigctld, flrig or direct CAT; see kahf-rig). No PTT unless rigctld is set (VOX).
struct AudioBackend {
    input: Option<AudioIn>,
    output: Option<AudioOut>,
    rig: Option<Box<dyn RigControl>>,
}

impl Backend for AudioBackend {
    fn read_audio(&mut self, out: &mut Vec<f32>) -> Result<(), String> {
        let Some(input) = self.input.as_mut() else {
            return Ok(());
        };
        if let Some(e) = input.error() {
            return Err(e);
        }
        let mut buf = [0f32; 4096];
        loop {
            let n = input.read(&mut buf);
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        Ok(())
    }

    fn tx_start(&mut self) -> Result<(), String> {
        if self.output.is_none() {
            return Err("no audio output device configured".into());
        }
        set_ptt(&mut self.rig, true)
    }

    fn tx_write(&mut self, samples: &[f32], start: usize, end: usize) -> Result<(), String> {
        match self.output.as_mut() {
            Some(o) => o.write(&samples[start..end]),
            None => Err("no audio output device configured".into()),
        }
    }

    fn tx_stop(&mut self) -> Result<(), String> {
        set_ptt(&mut self.rig, false)
    }
}

/// SDR receive through kahf-radio (any driver the operator names); transmit stays on the
/// HackRF path for now. Transmit is single sideband: I is the audio, Q its Hilbert transform.
/// Receive takes I and decimates to 8 kHz.
/// The HackRF cannot receive and transmit at once, so RX is closed while sending.
struct SdrBackend {
    spec: SdrSpec,
    rx: Option<Box<dyn RxRadio>>,
    tx: Option<HackrfTx>,
    rx_resampler: Resampler,
    iq: Vec<(f32, f32)>,
}

impl SdrBackend {
    fn new(spec: SdrSpec) -> Self {
        SdrBackend {
            spec,
            rx: None,
            tx: None,
            rx_resampler: Resampler::new(SDR_RATE_HZ, RATE),
            iq: vec![(0.0, 0.0); 16_384],
        }
    }

    /// Stops and drops the receiver, if one is open.
    fn close_rx(&mut self) {
        if let Some(mut rx) = self.rx.take() {
            rx.stop();
        }
    }
}

/// Sets the gains the receive path wants: LNA and VGA by name where the driver has them,
/// automatic for any other stage the driver can leave to itself.
fn set_rx_gains(rx: &mut dyn RxRadio) -> Result<(), String> {
    for stage in rx.gain_stages() {
        let name = stage.name.to_ascii_lowercase();
        let want = if name.contains("lna") {
            Some(SDR_RX_LNA_DB as f32)
        } else if name.contains("vga") {
            Some(SDR_RX_VGA_DB as f32)
        } else {
            None
        };
        match want {
            Some(db) => rx.set_gain(&stage.name, GainSetting::Manual(db))?,
            None if stage.auto => rx.set_gain(&stage.name, GainSetting::Auto)?,
            None => {}
        }
    }
    Ok(())
}

impl Backend for SdrBackend {
    fn read_audio(&mut self, out: &mut Vec<f32>) -> Result<(), String> {
        if self.rx.is_none() {
            let mut rx = kahf_radio::open(&self.spec.radio)?;
            rx.set_frequency_hz(self.spec.freq_hz)?;
            let rate = rx.set_sample_rate(SDR_RATE_HZ)?;
            set_rx_gains(rx.as_mut())?;
            rx.start()?;
            self.rx_resampler = Resampler::new(rate, RATE);
            self.rx = Some(rx);
        }
        let Some(rx) = self.rx.as_mut() else {
            return Ok(());
        };
        let n = match rx.read_iq(&mut self.iq) {
            Ok(n) => n,
            Err(e) => {
                self.close_rx();
                return Err(e);
            }
        };
        let mono: Vec<f32> = self.iq[..n].iter().map(|s| s.0).collect();
        self.rx_resampler.process(&mono, out);
        Ok(())
    }

    fn tx_start(&mut self) -> Result<(), String> {
        if self.spec.radio.driver != "hackrf" {
            return Err(format!(
                "transmit needs a HackRF; {} is receive only",
                self.spec.radio.driver
            ));
        }
        self.close_rx();
        let tx = HackrfTx::open(
            self.spec.radio.id.as_deref(),
            self.spec.freq_hz,
            SDR_RATE_HZ as f64,
            SDR_TX_VGA_DB,
            false,
        )?;
        self.tx = Some(tx);
        Ok(())
    }

    fn tx_write(&mut self, samples: &[f32], start: usize, end: usize) -> Result<(), String> {
        let tx = self.tx.as_mut().ok_or("transmitter not open")?;
        // Interpolation needs the audio around each chunk, so IQ comes from the whole burst.
        for s in (start..end).step_by(256) {
            let e = (s + 256).min(end);
            tx.write_iq(&sdr_iq_range(samples, s, e))?;
        }
        Ok(())
    }

    fn tx_stop(&mut self) -> Result<(), String> {
        // Dropping the transmitter stops it. The worker waits for playback to end first.
        self.tx = None;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
struct SdrSpec {
    radio: RadioSpec,
    freq_hz: u64,
}

/// Parses "driver[:id][@freq_hz]", for example "hackrf:0000abc@14074000" or "rtlsdr@145500000".
fn parse_sdr(s: &str) -> Result<SdrSpec, String> {
    let (dev, freq) = match s.split_once('@') {
        Some((d, f)) => (d, Some(f)),
        None => (s, None),
    };
    let radio = kahf_radio::parse_spec(dev)?;
    let freq_hz = match freq {
        Some(f) => f
            .trim()
            .parse::<u64>()
            .ok()
            .filter(|&hz| hz > 0)
            .ok_or_else(|| format!("bad SDR frequency {f:?}"))?,
        None => DEFAULT_SDR_FREQ_HZ,
    };
    Ok(SdrSpec { radio, freq_hz })
}

/// Windowed-sinc polyphase kernel: one row per fractional phase of the
/// 8 kHz -> 2.4 MHz step. Row p has 2 * SDR_HALF taps; tap j reads input
/// offset j - (SDR_HALF - 1). Each row is normalised to unit DC gain.
fn sdr_kernel() -> &'static [Vec<f32>] {
    static KERNEL: OnceLock<Vec<Vec<f32>>> = OnceLock::new();
    KERNEL.get_or_init(|| {
        let half = SDR_HALF as f64;
        let cutoff = 0.45; // cycles per audio sample
        (0..SDR_OVERSAMPLE)
            .map(|p| {
                let frac = p as f64 / SDR_OVERSAMPLE as f64;
                let taps: Vec<f64> = (0..2 * SDR_HALF)
                    .map(|j| {
                        let k = j as f64 - (half - 1.0);
                        let d = frac - k;
                        if d.abs() >= half {
                            return 0.0;
                        }
                        let win = 0.5 * (1.0 + (PI * d / half).cos());
                        let sinc = if d.abs() < 1e-12 {
                            2.0 * cutoff
                        } else {
                            (2.0 * PI * cutoff * d).sin() / (PI * d)
                        };
                        win * sinc
                    })
                    .collect();
                let sum: f64 = taps.iter().sum();
                taps.iter().map(|t| (t / sum) as f32).collect()
            })
            .collect()
    })
}

/// The whole audio buffer (8 kHz) as the HackRF transmits it: 2.4 Msps single-sideband IQ.
pub fn audio_to_iq(samples: &[f32]) -> Vec<(f32, f32)> {
    sdr_iq_range(samples, 0, samples.len())
}

/// Half-length of the Hilbert filter in audio samples (only odd taps are non-zero).
const SDR_HILBERT_HALF: usize = 127;

/// Hann-windowed ideal Hilbert taps, index k = 0..=SDR_HILBERT_HALF. Zero for even k.
fn hilbert_taps() -> &'static [f32] {
    static TAPS: OnceLock<Vec<f32>> = OnceLock::new();
    TAPS.get_or_init(|| {
        (0..=SDR_HILBERT_HALF)
            .map(|k| {
                if k == 0 || k % 2 == 0 {
                    return 0.0;
                }
                let kf = k as f64;
                let win = 0.5 * (1.0 + (PI * kf / (SDR_HILBERT_HALF as f64 + 1.0)).cos());
                ((2.0 / (PI * kf)) * win) as f32
            })
            .collect()
    })
}

/// Hilbert transform of audio at index idx (zero outside the buffer).
fn hilbert_at(x: &[f32], idx: isize) -> f32 {
    let at = |i: isize| {
        usize::try_from(i)
            .ok()
            .and_then(|u| x.get(u))
            .copied()
            .unwrap_or(0.0)
    };
    let taps = hilbert_taps();
    let mut acc = 0f32;
    for k in (1..=SDR_HILBERT_HALF).step_by(2) {
        acc += taps[k] * (at(idx - k as isize) - at(idx + k as isize));
    }
    acc
}

/// IQ samples for audio input samples m0..m1 (taps reach outside the range).
/// The output is single sideband: I is the audio and Q is its Hilbert transform,
/// so the carrier has only the upper sideband and no mirror image.
fn sdr_iq_range(x: &[f32], m0: usize, m1: usize) -> Vec<(f32, f32)> {
    let kernel = sdr_kernel();
    let mut out = Vec::with_capacity((m1 - m0) * SDR_OVERSAMPLE);
    let mut quad = [0f32; 2 * SDR_HALF];
    for m in m0..m1 {
        let base = m as isize - (SDR_HALF as isize - 1);
        for (j, q) in quad.iter_mut().enumerate() {
            *q = hilbert_at(x, base + j as isize);
        }
        for taps in kernel {
            let mut acc_i = 0f32;
            let mut acc_q = 0f32;
            for (j, &t) in taps.iter().enumerate() {
                let idx = base + j as isize;
                if let Some(&v) = usize::try_from(idx).ok().and_then(|i| x.get(i)) {
                    acc_i += t * v;
                }
                acc_q += t * quad[j];
            }
            out.push((acc_i, acc_q));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Frame layer helpers (PHY + LDPC + frame), shared by TX and RX
// ---------------------------------------------------------------------------

/// The modulations a frame can use, in the order the receiver tries them.
/// The id in the frame (0, 1, 2) is the index in this list.
const MODULATIONS: [Modulation; 3] = [Modulation::Qpsk, Modulation::Qam16, Modulation::Qam64];

fn mod_id(m: Modulation) -> u8 {
    MODULATIONS.iter().position(|&x| x == m).unwrap_or(0) as u8
}

/// Samples of one zero-payload frame at the transmit modulation. Used to find the preamble.
fn frame_template() -> &'static [f32] {
    static FRAME: OnceLock<Vec<f32>> = OnceLock::new();
    FRAME.get_or_init(|| phy::modulate_frame(&vec![0u8; ldpc::N], MODULATION))
}

/// Samples of one zero-payload frame in the given modulation.
fn frame_len_for(m: Modulation) -> usize {
    static LENS: OnceLock<Vec<usize>> = OnceLock::new();
    let lens = LENS.get_or_init(|| {
        MODULATIONS
            .iter()
            .map(|&x| phy::modulate_frame(&vec![0u8; ldpc::N], x).len())
            .collect()
    });
    lens[mod_id(m) as usize]
}

/// Samples of one frame at the transmit modulation.
fn frame_len() -> usize {
    frame_len_for(MODULATION)
}

/// One frame, sent with the given modulation. The frame carries the modulation id.
fn frame_samples_as(payload: &[u8], m: Modulation) -> Result<Vec<f32>, String> {
    if payload.len() > MAX_CHUNK {
        return Err(format!(
            "frame payload of {} bytes is over {MAX_CHUNK}",
            payload.len()
        ));
    }
    let coded = ldpc::encode(&frame::build_payload_as(payload, mod_id(m)));
    Ok(phy::modulate_frame(&coded, m))
}

/// Copies of each frame in weak mode. Eight copies, soft-combined, decode at about -4 dB.
const WEAK_COPIES: usize = 8;

/// A weak-mode burst: every frame sent WEAK_COPIES times in QPSK, with gaps. No acknowledgements.
fn weak_burst(frames: &[Vec<u8>]) -> Result<Vec<f32>, String> {
    let mut out = vec![0f32; LEAD_SILENCE];
    let mut first = true;
    for payload in frames {
        for _ in 0..WEAK_COPIES {
            if !first {
                out.extend(std::iter::repeat_n(0f32, GAP_SILENCE));
            }
            first = false;
            out.extend(frame_samples_as(payload, Modulation::Qpsk)?);
        }
    }
    out.extend(std::iter::repeat_n(0f32, TAIL_SILENCE));
    Ok(out)
}

/// Adds the log-likelihoods of the weak-mode copies that start at `samples[0]` and then every stride.
/// None unless the sum decodes to a valid QPSK frame.
fn decode_combined(samples: &[f32]) -> Option<Vec<u8>> {
    let flen = frame_len_for(Modulation::Qpsk);
    let stride = flen + GAP_SILENCE;
    let mut sum = vec![0f32; ldpc::N];
    for i in 0..WEAK_COPIES {
        let seg = samples.get(i * stride..i * stride + flen)?;
        let llr = phy::demodulate_frame(seg, Modulation::Qpsk, ldpc::N)?;
        for (s, l) in sum.iter_mut().zip(llr.iter()) {
            *s += l;
        }
    }
    let info = ldpc::decode(&sum, LDPC_ITER)?;
    let (data, id) = frame::parse_payload_id(&info)?;
    (id == mod_id(Modulation::Qpsk)).then_some(data)
}

/// One transmit burst: lead-in silence, the frames with gaps, tail silence.
fn burst(frames: &[Vec<u8>], m: Modulation) -> Result<Vec<f32>, String> {
    let mut out = vec![0f32; LEAD_SILENCE];
    for (i, payload) in frames.iter().enumerate() {
        if i > 0 {
            out.extend(std::iter::repeat_n(0f32, GAP_SILENCE));
        }
        out.extend(frame_samples_as(payload, m)?);
    }
    out.extend(std::iter::repeat_n(0f32, TAIL_SILENCE));
    Ok(out)
}

/// Normalised correlation against the preamble. Returns the start of the first
/// preamble above the threshold, refined to the strongest nearby offset.
fn find_preamble(x: &[f32]) -> Option<usize> {
    if x.len() < PREAMBLE_LEN {
        return None;
    }
    let tpl = &frame_template()[..PREAMBLE_LEN];
    let tpl_e: f64 = tpl.iter().map(|&v| (v as f64) * (v as f64)).sum();
    let last = x.len() - PREAMBLE_LEN;
    let corr = |t: usize| -> f64 {
        let w = &x[t..t + PREAMBLE_LEN];
        let mut dot = 0f64;
        let mut e = 0f64;
        for (a, b) in w.iter().zip(tpl) {
            let a = *a as f64;
            dot += a * *b as f64;
            e += a * a;
        }
        dot / (tpl_e * e).sqrt().max(1e-30)
    };
    let t0 = (0..=last).find(|&t| corr(t) >= PREAMBLE_THRESHOLD)?;
    let mut best = (t0, corr(t0));
    for t in (t0 + 1)..=(t0 + PREAMBLE_LEN).min(last) {
        let r = corr(t);
        if r > best.1 {
            best = (t, r);
        }
    }
    Some(best.0)
}

/// Decode exactly one frame that starts at the preamble, sent with modulation `m`.
/// None on a CRC or LDPC failure, or when the frame says it was sent another way.
fn decode_at(samples: &[f32], m: Modulation) -> Option<Vec<u8>> {
    let llr = phy::demodulate_frame(samples, m, ldpc::N)?;
    let info = ldpc::decode(&llr, LDPC_ITER)?;
    let (data, id) = frame::parse_payload_id(&info)?;
    (id == mod_id(m)).then_some(data)
}

// ---------------------------------------------------------------------------
// Inbox
// ---------------------------------------------------------------------------

/// Keeps the last path part and replaces anything unsafe. Remote names are not trusted.
fn safe_name(name: &str) -> String {
    let base = Path::new(name)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let clean: String = base
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " ._-()".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cut: String = clean.trim_start_matches('.').trim().chars().take(120).collect();
    if cut.is_empty() {
        "file".to_string()
    } else {
        cut
    }
}

fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let p = Path::new(name);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_string());
    let ext = p
        .extension()
        .map(|s| format!(".{}", s.to_string_lossy()))
        .unwrap_or_default();
    (2..)
        .map(|i| dir.join(format!("{stem} ({i}){ext}")))
        .find(|c| !c.exists())
        .unwrap_or(first)
}

fn save_file(inbox: &Path, name: &str, data: &[u8]) -> Result<PathBuf, String> {
    fs::create_dir_all(inbox).map_err(|e| format!("cannot create {}: {e}", inbox.display()))?;
    let path = unique_path(inbox, &safe_name(name));
    fs::write(&path, data).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// Outbox and logbook helpers
// ---------------------------------------------------------------------------

/// Session chunks for one message as they go on air. The private bit is set when `private`.
fn air_chunks(msg: &Message, private: bool) -> Result<Vec<Vec<u8>>, String> {
    guard("session encode", || session::encode_signed(msg, None, private))
}

fn kind_of(msg: &Message) -> &'static str {
    match msg {
        Message::Chat { .. } => "message",
        Message::Mail { .. } => "mail",
        Message::File { .. } => "file",
    }
}

/// Text starting with "URGENT:" (a chat text or a mail body) goes out first.
fn priority_of(msg: &Message) -> Priority {
    let text = match msg {
        Message::Chat { text, .. } => text,
        Message::Mail { body, .. } => body,
        Message::File { .. } => return Priority::Normal,
    };
    if text.starts_with("URGENT:") {
        Priority::Urgent
    } else {
        Priority::Normal
    }
}

fn dest_of(msg: &Message) -> &str {
    match msg {
        Message::Chat { to, .. } | Message::Mail { to, .. } | Message::File { to, .. } => to,
    }
}

fn describe_msg(msg: &Message) -> String {
    match msg {
        Message::Chat { to, .. } => format!("message to {to}"),
        Message::Mail { to, subject, .. } => format!("mail to {to}: {subject}"),
        Message::File { to, name, .. } => format!("file {name} to {to}"),
    }
}

/// Label for an item read back from the outbox file. Decodes the payload, so use it only at load time.
fn describe(item: &OutItem) -> String {
    match decode_payload(&item.payload) {
        Some(Payload::Msg { msg, .. }) => describe_msg(&msg),
        Some(Payload::Forward { from, to, .. }) => describe_forward(&from, &to),
        None => format!("{} (unreadable)", item.kind),
    }
}

fn describe_forward(from: &str, to: &str) -> String {
    format!("relay of a message from {from} to {to}")
}

fn sender_of(msg: &Message) -> &str {
    match msg {
        Message::Chat { from, .. } | Message::Mail { from, .. } | Message::File { from, .. } => from,
    }
}

/// What one outbox item sends: a message of this station, or another station's message
/// that this relay holds and passes on.
#[derive(Debug, PartialEq)]
enum Payload {
    Msg {
        msg: Message,
        private: bool,
    },
    /// Session chunks exactly as heard, so the signature and private bit survive the hop.
    Forward {
        from: String,
        to: String,
        /// The original message was urgent, so it keeps its place and the urgent pool.
        urgent: bool,
        chunks: Vec<Vec<u8>>,
    },
}

impl Payload {
    fn urgent(&self) -> bool {
        match self {
            Payload::Msg { msg, .. } => priority_of(msg) == Priority::Urgent,
            Payload::Forward { urgent, .. } => *urgent,
        }
    }

    fn chunks(&self) -> Result<Vec<Vec<u8>>, String> {
        match self {
            Payload::Msg { msg, private } => air_chunks(msg, *private),
            Payload::Forward { chunks, .. } => Ok(chunks.clone()),
        }
    }
}

/// Outbox tag of a held forward. Message kinds use 0 to 2.
const TAG_FORWARD: u8 = 3;

/// Outbox payload of a held forward: the tag, the sender and addressee, an urgent byte,
/// then the chunks.
fn encode_forward(from: &str, to: &str, urgent: bool, chunks: &[Vec<u8>]) -> Vec<u8> {
    let mut out = vec![TAG_FORWARD];
    put_field(&mut out, from.as_bytes());
    put_field(&mut out, to.as_bytes());
    out.push(u8::from(urgent));
    out.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
    for c in chunks {
        put_field(&mut out, c);
    }
    out
}

/// Reads back any outbox payload. None for anything malformed.
fn decode_payload(bytes: &[u8]) -> Option<Payload> {
    if bytes.first() != Some(&TAG_FORWARD) {
        return decode_item(bytes).map(|(msg, private)| Payload::Msg { msg, private });
    }
    let mut r = Fields(&bytes[1..]);
    let from = r.text()?;
    let to = r.text()?;
    let urgent = match r.take(1)?[0] {
        0 => false,
        1 => true,
        _ => return None,
    };
    let n = r.u32()? as usize;
    let chunks = (0..n)
        .map(|_| r.bytes().map(<[u8]>::to_vec))
        .collect::<Option<Vec<_>>>()?;
    r.0.is_empty().then_some(Payload::Forward {
        from,
        to,
        urgent,
        chunks,
    })
}

/// Identity of a relayed message: SHA-256 over its chunks as heard.
fn relay_digest(chunks: &[Vec<u8>]) -> [u8; 32] {
    let mut h = Sha256::new();
    for c in chunks {
        h.update(c);
    }
    h.finalize().into()
}

/// Sender and addressee of any outbox payload, read without decoding the rest. Every
/// layout starts with the tag, then the sender, then the addressee.
fn peek_route(bytes: &[u8]) -> Option<(String, String)> {
    let mut r = Fields(bytes);
    r.take(1)?;
    Some((r.text()?, r.text()?))
}

/// Outbox tag bit: the message is private (session private bit on air).
const TAG_PRIVATE: u8 = 0x80;

/// Outbox payload: a tag byte (kind in the low bits, `TAG_PRIVATE` on top), then
/// length-prefixed fields (u32 little endian).
fn encode_message(msg: &Message, private: bool) -> Vec<u8> {
    let mut out = Vec::new();
    let tag = |kind: u8| kind | if private { TAG_PRIVATE } else { 0 };
    match msg {
        Message::Chat { from, to, text } => {
            out.push(tag(0));
            for f in [from, to, text] {
                put_field(&mut out, f.as_bytes());
            }
        }
        Message::Mail {
            from,
            to,
            subject,
            body,
            attachments,
        } => {
            out.push(tag(1));
            for f in [from, to, subject, body] {
                put_field(&mut out, f.as_bytes());
            }
            out.extend_from_slice(&(attachments.len() as u32).to_le_bytes());
            for a in attachments {
                put_field(&mut out, a.as_bytes());
            }
        }
        Message::File { from, to, name, data } => {
            out.push(tag(2));
            put_field(&mut out, from.as_bytes());
            put_field(&mut out, to.as_bytes());
            put_field(&mut out, name.as_bytes());
            put_field(&mut out, data);
        }
    }
    out
}

/// Reads back what `encode_message` wrote, with the private bit. None for anything malformed.
fn decode_item(bytes: &[u8]) -> Option<(Message, bool)> {
    let mut r = Fields(bytes);
    let tag = r.take(1)?[0];
    let private = tag & TAG_PRIVATE != 0;
    let msg = match tag & !TAG_PRIVATE {
        0 => Message::Chat {
            from: r.text()?,
            to: r.text()?,
            text: r.text()?,
        },
        1 => {
            let from = r.text()?;
            let to = r.text()?;
            let subject = r.text()?;
            let body = r.text()?;
            let n = r.u32()? as usize;
            let attachments = (0..n).map(|_| r.text()).collect::<Option<Vec<String>>>()?;
            Message::Mail {
                from,
                to,
                subject,
                body,
                attachments,
            }
        }
        2 => Message::File {
            from: r.text()?,
            to: r.text()?,
            name: r.text()?,
            data: r.bytes()?.to_vec(),
        },
        _ => return None,
    };
    r.0.is_empty().then_some((msg, private))
}

fn put_field(out: &mut Vec<u8>, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
}

struct Fields<'a>(&'a [u8]);

impl<'a> Fields<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let all: &'a [u8] = self.0;
        if all.len() < n {
            return None;
        }
        let (head, tail) = all.split_at(n);
        self.0 = tail;
        Some(head)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }

    fn text(&mut self) -> Option<String> {
        String::from_utf8(self.bytes()?.to_vec()).ok()
    }
}

/// HF and VHF/UHF amateur bands in ADIF names, by frequency in MHz.
const BANDS: &[(f64, f64, &str)] = &[
    (1.8, 2.0, "160M"),
    (3.5, 4.0, "80M"),
    (5.3, 5.41, "60M"),
    (7.0, 7.3, "40M"),
    (10.1, 10.15, "30M"),
    (14.0, 14.35, "20M"),
    (18.068, 18.168, "17M"),
    (21.0, 21.45, "15M"),
    (24.89, 24.99, "12M"),
    (28.0, 29.7, "10M"),
    (50.0, 54.0, "6M"),
    (144.0, 148.0, "2M"),
    (430.0, 440.0, "70CM"),
];

/// Band name for a frequency in MHz, or empty when it is outside the amateur bands.
fn band_name(mhz: f64) -> String {
    BANDS
        .iter()
        .find(|(lo, hi, _)| (*lo..=*hi).contains(&mhz))
        .map(|(_, _, name)| name.to_string())
        .unwrap_or_default()
}

fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// (YYYYMMDD, HHMMSS) in UTC for a Unix time in seconds.
fn utc_stamp(secs: i64) -> (String, String) {
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    // Civil date from day count (proleptic Gregorian, Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (
        format!("{y:04}{m:02}{d:02}"),
        format!("{:02}{:02}{:02}", sod / 3_600, sod % 3_600 / 60, sod % 60),
    )
}

/// Writes the file via a temp file and rename, so a crash never leaves half a queue.
fn write_atomic(path: &Path, data: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("bin.tmp");
    fs::write(&tmp, data).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("cannot replace {}: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// Worker thread
// ---------------------------------------------------------------------------

/// Runs a closure and turns a panic into an error. The link and session layers
/// may still be unimplemented; this keeps the worker alive when they are.
fn guard<T>(what: &str, f: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|p| {
        let why = p
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| p.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_string());
        format!("{what}: {why}")
    })
}

/// A conversation in progress. It is written to the logbook when the link leaves Connected.
struct OpenQso {
    peer: String,
    date: String,
    time: String,
}

/// One burst on its way to the radio. Audio is handed over at most `TX_AHEAD_S` ahead of real time.
struct TxJob {
    samples: Vec<f32>,
    /// Next sample to hand to the radio.
    pos: usize,
    /// When PTT went on (`tx_start` succeeded). Playback time is measured from here.
    started: Option<Instant>,
    sending: bool,
}

/// What one step of a transmit did.
enum TxStep {
    /// Nothing to do yet: the device is ahead of real time, or PTT is still settling.
    Wait,
    Progress,
    Finished,
}

struct Worker {
    cmds: Receiver<Cmd>,
    shared: Arc<Shared>,
    factory: Factory,
    ids: Arc<AtomicU64>,
    inbox: PathBuf,
    data_dir: PathBuf,
    /// Frequency from the sdr setting, if one is set (logbook band and freq).
    freq_mhz: Option<f64>,
    /// Tuned frequency in Hz; receive audio is centred on it (0 Hz offset).
    tuned_hz: Option<u64>,
    /// Carrier sense over the receive audio (plan 2.1).
    sense: Sense,
    /// Channel plan for transmit selection (plan 2.2).
    plan: select::Plan,
    /// Energy carrier sense per plan channel (plan 2.1), fed from the spectrum. Keyed by Hz.
    carrier: HashMap<u64, CarrierSense>,
    /// When the carrier sense was last fed (one frame per FRAME_S).
    carrier_fed: Option<Instant>,
    /// Failed channel checks for the next transmit (plan 2.3).
    access_failures: u32,
    /// No transmit starts before this time after a busy channel asked us to wait.
    access_retry_at: Option<Instant>,
    /// Our callsign. Private messages are kept only when addressed to it.
    callsign: String,
    backend: Option<Box<dyn Backend>>,
    link: Link,
    /// Index into MODULATIONS for the next transmission. Moves one step at a time with the peer's reports.
    tx_level: u8,
    link_state: LinkState,
    qso: Option<OpenQso>,
    outbox: Outbox,
    outbox_path: PathBuf,
    /// Public id and label per outbox entry, keyed by the outbox's own id.
    tags: HashMap<u64, (u64, String)>,
    bursts: VecDeque<Vec<f32>>,
    /// Link control frames (CONNECT, ACK, ...) waiting for the channel check.
    control: VecDeque<Vec<Vec<u8>>>,
    tx: Option<TxJob>,
    rx_buf: Vec<f32>,
    spec: Analyser,
    spec_due: Instant,
    since_scan: usize,
    rx_chunks: Vec<Vec<u8>>,
    rx_error: Option<String>,
    /// After a failed RX read, the radio is not read again before this time.
    rx_retry_at: Option<Instant>,
    tick_error: Option<String>,
    started: Instant,
    /// Queued outbox items: destination and the time (worker clock) they were queued.
    queued: HashMap<u64, (String, u64)>,
    /// Outbox items whose station was not reached in time; each is reported once.
    unreached_noted: HashSet<u64>,
    /// The transmit refusal last logged, so a held transmit is reported once.
    refusal_logged: Option<&'static str>,
    /// Link clock in ms: worker time minus the time spent airing our own frames.
    link_ms: u64,
    /// When `link_ms` was last advanced.
    link_mark: Instant,
    /// Forwards handed to the link and not yet acknowledged: outbox id, public id, from, to.
    handed: Vec<(u64, u64, String, String)>,
    /// Digests of messages already held for relay, oldest first, so each is passed on once.
    relayed: VecDeque<[u8; 32]>,
    /// The station this relay called itself to pass messages on, while that call lasts.
    dialed: Option<String>,
    /// No new relay call to a station before this time.
    dial_after: HashMap<String, Instant>,
}

/// Line that separates a mail body from its signature block.
const SIG_MARK: &str = "\n-----KAHF SIGNATURE-----\n";

/// The body with its signature block appended: the mark, then the auth block as hex.
fn sign_body(key: &StationKey, callsign: &str, body: &str) -> String {
    let block = auth::sign(key, callsign, body.as_bytes());
    let hex: String = block.iter().map(|b| format!("{b:02x}")).collect();
    format!("{body}{SIG_MARK}{hex}\n")
}

/// Splits a received mail body into the text and a label for the signature.
/// The label says who signed it, or why the signature does not count.
fn split_signature(body: &str, sender: &str) -> (String, String) {
    let Some(at) = body.rfind(SIG_MARK) else {
        return (body.to_string(), "Not signed".to_string());
    };
    let text = &body[..at];
    let hex = body[at + SIG_MARK.len()..].trim();
    let block: Option<Vec<u8>> = (hex.len().is_multiple_of(2) && hex.is_ascii())
        .then(|| {
            (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
                .collect::<Option<Vec<u8>>>()
        })
        .flatten();
    let label = match block.map(|b| auth::verify(&b, text.as_bytes())) {
        Some(Ok((call, public))) if call.eq_ignore_ascii_case(sender) => {
            format!("Signed by {call}, key {}", auth::fingerprint(&public))
        }
        Some(Ok((call, _))) => format!("Signed by {call}, which is not the sender {sender}"),
        Some(Err(e)) => format!("Signature not valid: {e}"),
        None => "Signature not valid: not hex".to_string(),
    };
    (text.to_string(), label)
}

/// Key for a channel in the carrier-sense map: its frequency in whole Hz.
fn carrier_key(channel_hz: f32) -> u64 {
    channel_hz.round().max(0.0) as u64
}

/// Result of one channel check (plan 2.3).
enum Pick {
    Go,
    Wait,
    GaveUp,
}

impl Worker {
    fn new(
        config: Config,
        cmds: Receiver<Cmd>,
        shared: Arc<Shared>,
        factory: Factory,
        ids: Arc<AtomicU64>,
    ) -> Self {
        let outbox_path = config.data_dir.join("outbox.bin");
        let outbox = fs::read(&outbox_path)
            .ok()
            .and_then(|bytes| Outbox::from_bytes(&bytes))
            .unwrap_or_default();
        let tuned_hz = config
            .sdr
            .as_deref()
            .and_then(|s| parse_sdr(s).ok())
            .map(|s| s.freq_hz);
        let freq_mhz = tuned_hz.map(|f| f as f64 / 1_000_000.0);
        // Items saved by an earlier run get new public ids, in the order they go out.
        let mut tags = HashMap::new();
        let mut queued = HashMap::new();
        let mut relayed = VecDeque::new();
        let mut copy = outbox.clone();
        while let Some((oid, item)) = copy.next(u64::MAX) {
            copy.cancel(oid);
            tags.insert(oid, (ids.fetch_add(1, Ordering::Relaxed), describe(&item)));
            // Items from an earlier run wait for the relay timeout from start-up.
            if let Some((_, to)) = peek_route(&item.payload) {
                queued.insert(oid, (to, 0));
            }
            // Held forwards keep their digest, so a copy heard after a restart is not held twice.
            if let Some(Payload::Forward { chunks, .. }) = decode_payload(&item.payload) {
                relayed.push_back(relay_digest(&chunks));
            }
        }
        let w = Worker {
            cmds,
            shared,
            factory,
            ids,
            inbox: config.data_dir.join("inbox"),
            data_dir: config.data_dir.clone(),
            freq_mhz,
            tuned_hz,
            sense: Sense::new(RATE as f32),
            plan: select::Plan::default_plan(),
            carrier: HashMap::new(),
            carrier_fed: None,
            access_failures: 0,
            access_retry_at: None,
            callsign: config.callsign.clone(),
            backend: None,
            link: Link::new(&config.callsign),
            tx_level: 0,
            link_state: LinkState::Idle,
            qso: None,
            outbox,
            outbox_path,
            tags,
            bursts: VecDeque::new(),
            control: VecDeque::new(),
            tx: None,
            rx_buf: Vec::new(),
            spec: Analyser::new(SPEC_FFT, RATE as f32),
            spec_due: Instant::now(),
            since_scan: 0,
            rx_chunks: Vec::new(),
            rx_error: None,
            rx_retry_at: None,
            tick_error: None,
            started: Instant::now(),
            queued,
            unreached_noted: HashSet::new(),
            refusal_logged: None,
            link_ms: 0,
            link_mark: Instant::now(),
            handed: Vec::new(),
            relayed,
            dialed: None,
            dial_after: HashMap::new(),
        };
        w.publish_pending();
        w
    }

    /// One pass per loop: link timers, commands, then at most one unit of work.
    /// Transmit runs in small steps here, so commands and receive keep moving.
    fn run(mut self) {
        self.log("engine started");
        let key_error = lock(&self.shared.key_error).take();
        if let Some(e) = key_error {
            self.log(e);
        }
        'main: loop {
            self.tick_link();
            loop {
                match self.cmds.try_recv() {
                    Ok(Cmd::Shutdown) | Err(TryRecvError::Disconnected) => break 'main,
                    Ok(cmd) => self.command(cmd),
                    Err(TryRecvError::Empty) => break,
                }
            }
            match tx_refusal(&self.shared.flags) {
                Some(why) => self.hold_tx(why),
                None => self.refusal_logged = None,
            }
            self.check_relays();
            self.settle_forwards();
            self.dial_relays();
            if self.tx.is_some() {
                // Half duplex: no receive while a burst is on the air.
                if !self.tx_step() {
                    thread::sleep(POLL);
                }
                continue;
            }
            // Control frames (ACK, CONNECT, ...) go before data; each passes the channel check.
            self.flush_control();
            // Queued items go first (only while linked), one unit per pass.
            if let Some((id, payload)) = self.next_due() {
                if self.clear_channel(id, payload.urgent()) {
                    self.send_item(id, payload);
                }
                continue;
            }
            if let Some(samples) = self.bursts.pop_front() {
                self.tx = Some(TxJob {
                    samples,
                    pos: 0,
                    started: None,
                    sending: false,
                });
                continue;
            }
            if self.rx_step() == 0 {
                thread::sleep(POLL);
            }
        }
        self.abort_tx();
    }

    fn emit(&self, ev: Event) {
        lock(&self.shared.events).push(ev);
    }

    fn log(&self, msg: impl Into<String>) {
        self.emit(Event::Log(msg.into()));
    }

    fn next_id(&self) -> u64 {
        self.ids.fetch_add(1, Ordering::Relaxed)
    }

    fn command(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Connect { peer } => {
                if let Err(e) = self.connect(&peer) {
                    self.log(e);
                }
            }
            Cmd::Disconnect => {
                if let Err(e) = self.disconnect() {
                    self.log(e);
                }
            }
            Cmd::Listen => {
                if let Err(e) = self.ensure_radio() {
                    self.log(e);
                }
            }
            Cmd::Queue { msgs, ids, private } => self.enqueue(msgs, ids, private),
            Cmd::Shutdown => {}
            #[cfg(test)]
            Cmd::TestBurst(samples) => self.bursts.push_back(samples),
        }
    }

    /// Opens the radio on first use. Errors go back to the caller.
    fn ensure_radio(&mut self) -> Result<(), String> {
        if self.backend.is_some() {
            return Ok(());
        }
        let backend = (self.factory)().map_err(|e| format!("cannot open radio: {e}"))?;
        self.backend = Some(backend);
        self.log("radio open");
        Ok(())
    }

    fn connect(&mut self, peer: &str) -> Result<(), String> {
        self.ensure_radio()?;
        let actions = guard("link connect", || self.link.connect(peer))?;
        self.log(format!("calling {peer}"));
        self.apply(actions);
        Ok(())
    }

    fn disconnect(&mut self) -> Result<(), String> {
        self.ensure_radio()?;
        let actions = guard("link disconnect", || self.link.disconnect())?;
        self.apply(actions);
        Ok(())
    }

    fn now_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// Adds messages to the outbox in order and saves the queue. `ids` are the public ids the caller returned.
    fn enqueue(&mut self, msgs: Vec<Message>, ids: Vec<u64>, private: bool) {
        for (m, pid) in msgs.iter().zip(ids) {
            let item = OutItem {
                kind: kind_of(m).to_string(),
                payload: encode_message(m, private),
            };
            let oid = self.outbox.push(item, priority_of(m));
            self.tags.insert(oid, (pid, describe_msg(m)));
            let at = self.now_ms();
            self.queued.insert(oid, (dest_of(m).to_string(), at));
        }
        self.persist_outbox();
    }

    /// Plan 6.3: a message whose station is not reached on air within the timeout goes to
    /// the relay step once. With internet relay on, the gateway takes it (plan 6.2). Otherwise
    /// the user is told. The item stays in the outbox, so it still goes if the station appears.
    fn check_relays(&mut self) {
        let now = self.now_ms();
        let timeout = self.shared.flags.relay_timeout_ms.load(Ordering::SeqCst);
        let connected = match &self.link_state {
            LinkState::Connected { peer } => Some(peer.clone()),
            _ => None,
        };
        let mut due: Vec<(u64, String)> = self
            .queued
            .iter()
            .filter(|(oid, (dest, at))| {
                let on_air = connected.as_deref().is_some_and(|p| p.eq_ignore_ascii_case(dest));
                !on_air && now.saturating_sub(*at) >= timeout && !self.unreached_noted.contains(oid)
            })
            .map(|(oid, (dest, _))| (*oid, dest.clone()))
            .collect();
        if due.is_empty() {
            return;
        }
        due.sort();
        let paths = lock(&self.shared.relay_paths).clone();
        for (oid, dest) in due {
            self.unreached_noted.insert(oid);
            let from = self
                .outbox
                .due(u64::MAX)
                .iter()
                .find(|(id, _)| *id == oid)
                .and_then(|(_, item)| peek_route(&item.payload))
                .map(|(from, _)| from)
                .unwrap_or_default();
            // The relays this message may use: never its sender or its addressee.
            let usable = self.next_stations(oid, &from, &dest, &paths).split_off(1);
            if !usable.is_empty() {
                self.log(format!(
                    "Could not reach {dest} on air. It goes by radio relay through {} once connected.",
                    usable.join(" or ")
                ));
                continue;
            }
            if !self.shared.flags.internet_relay.load(Ordering::SeqCst) {
                self.log(unreached_text(&dest));
                continue;
            }
            match relay_via_gateway(&dest) {
                Ok(()) => {
                    self.outbox.done(oid);
                    self.log(format!("Sent to {dest} by internet relay."));
                    self.persist_outbox();
                }
                Err(e) => self.log(format!(
                    "Could not reach {dest} on air, and internet relay failed: {e}. The message stays in the outbox."
                )),
            }
        }
    }

    /// Plan 4: a station with a relay role sends only inside one of its transmit slots.
    /// A station without a relay role, or with no schedule yet, is never held.
    fn relay_slot_open(&self) -> bool {
        match lock(&self.shared.relay).as_ref() {
            None => true,
            Some(r) if r.role == Role::Station => true,
            // A relay role is held unless relay mode runs and a transmit slot covers now.
            // With no schedule there is no slot, so it is held.
            Some(r) => {
                self.shared.flags.relay_running.load(Ordering::SeqCst)
                    && !r.slots().is_empty()
                    && r.may_relay_transmit_at(now_unix_secs().max(0) as u64)
            }
        }
    }

    /// Plan: only relay mode may transmit unattended. An outbox or control send goes out
    /// when relay mode runs, or when the operator started a connection and has not
    /// disconnected. Listen only is checked separately and always wins.
    fn transmit_allowed(&self) -> bool {
        self.shared.flags.relay_running.load(Ordering::SeqCst)
            || self.shared.flags.operator_link.load(Ordering::SeqCst)
    }

    /// Counts one forward as a relay and records its hop. Only for a station with a relay role.
    fn record_forward(&self, public_id: u64) {
        let guard = lock(&self.shared.relay);
        let Some(r) = guard.as_ref().filter(|r| r.role != Role::Station) else {
            return;
        };
        let now = now_unix_secs().max(0) as u64;
        let day_start = now - now % 86_400;
        lock(&self.shared.relay_stats).count_sent(now, day_start);
        lock(&self.shared.hops).record(Hop {
            message_id: public_id,
            relay: r.callsign.clone(),
            at_unix: now,
            channel_khz: self.tuned_hz.map_or(0.0, |hz| hz as f32 / 1000.0),
            heard_by_next: false,
        });
    }

    /// Stops any transmit in progress and drops queued bursts while a refusal holds.
    /// Logs the refusal once, not on every pass.
    fn hold_tx(&mut self, why: &'static str) {
        self.bursts.clear();
        self.control.clear();
        if self.tx.is_some() {
            self.abort_tx();
            self.emit_tx(TxState::Failed(why.into()));
        }
        if self.refusal_logged != Some(why) {
            self.log(why);
            self.refusal_logged = Some(why);
        }
    }

    /// The next outbox item that may go out now. Nothing is sent unless the link is
    /// Connected, and only to the station it is connected to. The queue is in order,
    /// so an item for another station at the front holds the rest until that station is called.
    /// The next outbox item that may go to the connected station: one addressed to it, or,
    /// when it is one of our relays, one whose station was not reached directly (plan 6.3).
    /// Items for other stations are skipped, so they never block the ones that can go.
    fn next_due(&mut self) -> Option<(u64, Payload)> {
        if tx_refusal(&self.shared.flags).is_some() {
            return None;
        }
        if !self.transmit_allowed() || !self.relay_slot_open() {
            return None;
        }
        if self.access_retry_at.is_some_and(|t| Instant::now() < t) {
            return None;
        }
        let peer = match &self.link_state {
            LinkState::Connected { peer } => peer.clone(),
            _ => return None,
        };
        let paths = lock(&self.shared.relay_paths).clone();
        let now = self.now_ms();
        let mut unreadable = Vec::new();
        let mut pick = None;
        for (id, item) in self.outbox.due(now) {
            let Some((from, to)) = peek_route(&item.payload) else {
                unreadable.push(id);
                continue;
            };
            if self.handed.iter().any(|h| h.0 == id) {
                continue;
            }
            let next = self.next_stations(id, &from, &to, &paths);
            if next.iter().any(|c| c.eq_ignore_ascii_case(&peer)) {
                match decode_payload(&item.payload) {
                    Some(p) => {
                        pick = Some((id, p));
                        break;
                    }
                    None => unreadable.push(id),
                }
            }
        }
        if !unreadable.is_empty() {
            for id in unreadable {
                self.outbox.cancel(id);
                self.log("dropped an unreadable outbox item");
            }
            self.persist_outbox();
        }
        pick
    }

    /// Audio offset (Hz from the tuned frequency) of a plan channel, if the receive audio can hear it.
    fn audible_offset_hz(&self, channel_hz: f32) -> Option<f32> {
        let tuned = self.tuned_hz? as f64;
        let off = channel_hz as f64 - tuned;
        (0.0..RATE as f64 / 2.0).contains(&off).then_some(off as f32)
    }

    /// Carrier sense for one plan channel (plan 2.1). A channel outside the receive audio
    /// cannot be measured, so it is not called busy.
    fn channel_busy(&self, channel_hz: f32) -> bool {
        let Some(off) = self.audible_offset_hz(channel_hz) else {
            return false;
        };
        let marker = self
            .sense
            .channel_busy(off / 1000.0, select::DEFAULT_SPACING_HZ / 1000.0);
        match self.carrier.get(&carrier_key(channel_hz)) {
            Some(cs) => cs.is_busy(marker),
            None => marker,
        }
    }

    /// Feeds one power frame per plan channel from the latest spectrum (plan 2.1). A frame
    /// is taken at most once per FRAME_S. Power is summed over the channel width, so a
    /// station anywhere in the channel raises the level.
    fn feed_carrier(&mut self, bins_db: &[f32]) {
        let now = Instant::now();
        if self
            .carrier_fed
            .is_some_and(|t| now.duration_since(t).as_secs_f32() < access::FRAME_S)
        {
            return;
        }
        self.carrier_fed = Some(now);
        let bin_hz = RATE as f32 / SPEC_FFT as f32;
        let half_hz = select::DEFAULT_SPACING_HZ / 2.0;
        let channels: Vec<f32> = self.plan.channels.iter().map(|(f, _)| *f).collect();
        for ch in channels {
            let Some(off) = self.audible_offset_hz(ch) else {
                continue;
            };
            let lo = (((off - half_hz) / bin_hz).floor().max(0.0)) as usize;
            let hi = (((off + half_hz) / bin_hz).ceil() as usize).min(bins_db.len().saturating_sub(1));
            if lo > hi || bins_db.is_empty() {
                continue;
            }
            let power: f32 = bins_db[lo..=hi].iter().map(|db| 10f32.powf(db / 10.0)).sum();
            self.carrier.entry(carrier_key(ch)).or_default().push(power);
        }
    }

    /// Plan 2.3: picks a free channel before a transmit. Urgent messages try the Emergency
    /// pool first, others the Open pool. Returns true when the transmit may go now. A busy
    /// plan holds the item for the returned seconds; the last try gives up and the item stays
    /// in the outbox for a later retry.
    fn clear_channel(&mut self, id: u64, urgent: bool) -> bool {
        matches!(self.pick_channel(Some(id), urgent), Pick::Go)
    }

    /// The channel check shared by data and control bursts. `id` is the outbox item, if any;
    /// a control burst has none and is dropped on the last try.
    fn pick_channel(&mut self, id: Option<u64>, urgent: bool) -> Pick {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
            ^ id.unwrap_or(0);
        let pool = if urgent {
            select::Pool::Emergency
        } else {
            select::Pool::Open
        };
        let pick = {
            let busy = |f: f32| self.channel_busy(f);
            select::choose_in_pool(
                &self.plan,
                pool,
                None,
                &busy,
                &mut select::SplitMix(seed),
                self.access_failures,
            )
        };
        match pick {
            Ok(_) => {
                self.access_failures = 0;
                self.access_retry_at = None;
                Pick::Go
            }
            Err(Refusal::Wait { seconds }) => {
                self.access_failures += 1;
                self.access_retry_at = Some(Instant::now() + Duration::from_secs_f32(seconds));
                self.log(format!("Channel busy. Trying again in {seconds:.0} s."));
                Pick::Wait
            }
            Err(Refusal::GiveUp) => {
                self.access_failures = 0;
                self.access_retry_at = None;
                match id {
                    Some(id) => {
                        self.log("No free channel after 5 tries. The message stays in the outbox and will be tried again later.");
                        self.fail_item(id, "channel busy".to_string());
                    }
                    None => self.log("No free channel after 5 tries. A link control burst was dropped."),
                }
                Pick::GaveUp
            }
        }
    }

    /// Control bursts (CONNECT, ACK, DISCONNECT, timers) get the same channel check as data.
    /// A busy channel holds the oldest frame until the retry time; the plan's other channels
    /// are tried by the same pick, so a free one takes it. The last try drops it.
    fn flush_control(&mut self) {
        if self.control.is_empty() {
            return;
        }
        if tx_refusal(&self.shared.flags).is_some() {
            self.control.clear();
            return;
        }
        // Link replies (ACK and the like) answer the other station's call, so they are not
        // gated by transmit_allowed. Only the relay slot and Listen only apply to them.
        if !self.relay_slot_open() {
            return;
        }
        if self.access_retry_at.is_some_and(|t| Instant::now() < t) {
            return;
        }
        match self.pick_channel(None, false) {
            Pick::Go => {
                if let Some(frames) = self.control.pop_front() {
                    self.push_burst(&frames);
                }
            }
            Pick::Wait => {}
            Pick::GaveUp => {
                self.control.pop_front();
            }
        }
    }

    /// TX: session encode -> link send -> one burst per link call. The item is done once
    /// all its chunks are handed to the link; on error it is retried with backoff.
    fn send_item(&mut self, id: u64, payload: Payload) {
        let chunks = match payload.chunks() {
            Ok(c) => c,
            Err(e) => return self.fail_item(id, e),
        };
        // Progress is shown for our own files only, not for files relayed for others.
        let (name, total) = match &payload {
            Payload::Msg {
                msg: Message::File { name, data, .. },
                ..
            } => (Some(name.clone()), data.len() as u64),
            _ => (None, 0),
        };
        let public = self.tags.get(&id).map(|t| t.0).unwrap_or(id);
        let count = chunks.len().max(1) as u64;
        for (i, chunk) in chunks.iter().enumerate() {
            match guard("link send", || self.link.send(chunk)) {
                Ok(actions) => self.apply_link(actions, true),
                Err(e) => return self.fail_item(id, e),
            }
            if !matches!(self.link_state, LinkState::Connected { .. }) {
                return self.fail_item(id, "link dropped during send".to_string());
            }
            if let Some(name) = &name {
                self.emit(Event::FileProgress {
                    id: public,
                    name: name.clone(),
                    done: total * (i as u64 + 1) / count,
                    total,
                });
            }
        }
        if let Payload::Forward { from, to, .. } = payload {
            // A relayed message is done only once the next station has acknowledged it
            // (see `settle_forwards`); until then it stays held.
            self.handed.push((id, public, from, to));
            return;
        }
        self.outbox.done(id);
        self.persist_outbox();
    }

    /// Forwards handed to the link finish here: done once the peer acknowledged every chunk,
    /// back to the queue with a retry wait when the link dropped first.
    fn settle_forwards(&mut self) {
        if self.handed.is_empty() {
            return;
        }
        let connected = matches!(self.link_state, LinkState::Connected { .. });
        if connected && !self.link.is_flushed() {
            return;
        }
        for (id, public, from, to) in std::mem::take(&mut self.handed) {
            if connected {
                self.outbox.done(id);
                self.record_forward(public);
                self.log(format!("Relayed a message from {from} to {to}."));
                self.emit_relay(&from, &to, RelayStep::Sent);
                self.persist_outbox();
            } else {
                self.fail_item(
                    id,
                    format!("link to {to} dropped before the relay was acknowledged"),
                );
            }
        }
    }

    fn fail_item(&mut self, id: u64, why: String) {
        let was_queued = self.outbox.pending().iter().any(|(pid, _, _)| *pid == id);
        let now = self.now_ms();
        self.outbox.failed(id, now);
        let still_queued = self.outbox.pending().iter().any(|(pid, _, _)| *pid == id);
        if was_queued && !still_queued {
            self.log(format!("gave up on an outbox item: {why}"));
        } else {
            self.log(format!("send failed, will retry: {why}"));
        }
        self.persist_outbox();
    }

    /// Saves the whole outbox and refreshes the snapshot the handle reads. Called only after the queue changed.
    fn persist_outbox(&mut self) {
        let live: HashSet<u64> = self.outbox.pending().iter().map(|(id, _, _)| *id).collect();
        self.tags.retain(|oid, _| live.contains(oid));
        self.queued.retain(|oid, _| live.contains(oid));
        self.unreached_noted.retain(|oid| live.contains(oid));
        if let Err(e) = write_atomic(&self.outbox_path, &self.outbox.to_bytes()) {
            self.log(format!("cannot save outbox: {e}"));
        }
        self.publish_pending();
    }

    /// Copies the outbox listing (urgent first, then order of queueing) into the shared snapshot.
    fn publish_pending(&self) {
        let mut list = self.outbox.pending();
        list.sort_by_key(|(id, prio, _)| (*prio, *id));
        let rows: Vec<(u64, String)> = list
            .iter()
            .filter_map(|(oid, _, _)| self.tags.get(oid).cloned())
            .collect();
        *lock(&self.shared.pending) = rows;
    }

    /// Link state changed. A conversation starts when the link comes up with a station
    /// and is logged when the link leaves Connected.
    fn on_link_state(&mut self, state: &LinkState) {
        let current = self.qso.as_ref().map(|q| q.peer.clone());
        let next = match state {
            LinkState::Connected { peer } => Some(peer.clone()),
            _ => None,
        };
        if current != next {
            if current.is_some() {
                self.finish_qso();
            }
            if let Some(peer) = next {
                self.start_qso(peer);
            }
        }
        self.link_state = state.clone();
    }

    fn start_qso(&mut self, peer: String) {
        let (date, time) = utc_stamp(now_unix_secs());
        self.note_contact(&peer, &date);
        self.qso = Some(OpenQso { peer, date, time });
    }

    fn finish_qso(&mut self) {
        let Some(q) = self.qso.take() else {
            return;
        };
        let qso = Qso {
            call: q.peer.clone(),
            date: q.date,
            time: q.time,
            band: self.freq_mhz.map(band_name).unwrap_or_default(),
            mode: "KAHF".to_string(),
            freq_mhz: self.freq_mhz,
            rst_sent: String::new(),
            rst_rcvd: String::new(),
            name: String::new(),
            comment: String::new(),
        };
        if let Err(e) = fs::create_dir_all(&self.data_dir) {
            self.log(format!("cannot create {}: {e}", self.data_dir.display()));
        }
        match kahf_log::add(&self.data_dir.join("logbook.txt"), &qso) {
            Ok(()) => self.log(format!("logged contact with {}", q.peer)),
            Err(e) => self.log(format!("cannot write logbook: {e}")),
        }
    }

    /// Sets last_heard for the peer in contacts.txt, adding the contact if it is new.
    fn note_contact(&mut self, peer: &str, date: &str) {
        if !valid_call(peer) {
            return;
        }
        if let Err(e) = fs::create_dir_all(&self.data_dir) {
            self.log(format!("cannot create {}: {e}", self.data_dir.display()));
            return;
        }
        let path = self.data_dir.join("contacts.txt");
        let mut list = match contacts::load(&path) {
            Ok(l) => l,
            Err(e) => {
                self.log(e);
                return;
            }
        };
        if !list.iter().any(|c| c.call.eq_ignore_ascii_case(peer)) {
            contacts::upsert(
                &mut list,
                Contact {
                    call: peer.to_string(),
                    name: String::new(),
                    qth: String::new(),
                    notes: String::new(),
                    last_heard: None,
                },
            );
        }
        contacts::touch(&mut list, peer, date);
        if let Err(e) = contacts::save(&path, &list) {
            self.log(e);
        }
    }

    /// Runs link timers (handshake retries, DATA retransmits) on the monotonic clock.
    /// True while a burst of ours is on the air or built and about to go. Frames held back
    /// by the channel check or a closed slot do not count, so a dead link still times out.
    fn airing(&self) -> bool {
        self.tx.is_some() || !self.bursts.is_empty()
    }

    /// The link's clock stands still while our own burst is on the air, so its
    /// retransmit timers count only time in which the other station could have answered.
    /// Without this a timer runs out during our own long burst and the retransmit then
    /// covers the peer's reply, again and again.
    fn tick_link(&mut self) {
        let now = Instant::now();
        if !self.airing() {
            self.link_ms += now.duration_since(self.link_mark).as_millis() as u64;
        }
        self.link_mark = now;
        let now_ms = self.link_ms;
        match guard("link tick", || self.link.tick(now_ms)) {
            Ok(actions) => {
                self.tick_error = None;
                self.apply(actions);
            }
            Err(e) => {
                // Runs every pass, so log an error only once until it changes.
                if self.tick_error.as_deref() != Some(e.as_str()) {
                    self.log(e.clone());
                    self.tick_error = Some(e);
                }
            }
        }
    }

    /// Handles link actions from control paths (CONNECT, DISCONNECT, ACK, timers). Transmit
    /// payloads wait for the channel check in `flush_control`.
    fn apply(&mut self, actions: Vec<Action>) {
        self.apply_link(actions, false);
    }

    /// Handles link actions: transmit payloads become one burst, deliveries go to the
    /// session layer, state changes become events. `cleared` means the channel was already
    /// checked for these payloads (data sent after `clear_channel`).
    fn apply_link(&mut self, actions: Vec<Action>, cleared: bool) {
        let mut frames = Vec::new();
        for action in actions {
            match action {
                Action::Transmit(payload) => frames.push(payload),
                Action::Deliver(chunk) => self.deliver(chunk),
                Action::StateChanged(state) => {
                    self.on_link_state(&state);
                    self.emit(Event::Link(state));
                }
                // Link quality snapshots have no event yet.
                Action::Metrics(_) => {}
            }
        }
        if frames.is_empty() {
            return;
        }
        // Every on-air burst passes here, so this is the one place a refusal must hold.
        if let Some(why) = tx_refusal(&self.shared.flags) {
            self.hold_tx(why);
            return;
        }
        if frames.iter().any(|f| f.len() > MAX_CHUNK) {
            self.log("link produced an over-long frame; dropped");
            return;
        }
        if cleared {
            self.push_burst(&frames);
        } else {
            self.control.push_back(frames);
        }
    }

    /// Builds the audio for one burst and queues it for transmit.
    fn push_burst(&mut self, frames: &[Vec<u8>]) {
        // Step the modulation one level at a time: up after a clean report, down after a poor one.
        match self.link.peer_frame_error_pct() {
            Some(f) if f < 2 && (self.tx_level as usize) + 1 < MODULATIONS.len() => self.tx_level += 1,
            Some(f) if f >= 8 && self.tx_level > 0 => self.tx_level -= 1,
            _ => {}
        }
        let m = MODULATIONS[self.tx_level as usize];
        let built = if self.shared.flags.weak_mode.load(Ordering::SeqCst) {
            weak_burst(frames)
        } else {
            burst(frames, m)
        };
        match built {
            Ok(samples) => self.bursts.push_back(samples),
            Err(e) => self.log(format!("cannot build transmission: {e}")),
        }
    }

    fn emit_tx(&self, state: TxState) {
        self.emit(Event::Tx { state });
    }

    fn radio(&mut self) -> Result<&mut Box<dyn Backend>, String> {
        self.backend.as_mut().ok_or_else(|| "no radio open".to_string())
    }

    /// Moves the current burst along by one step. Never waits for the device:
    /// audio is written only while the device is less than `TX_AHEAD_S` ahead.
    fn tx_advance(&mut self, job: &mut TxJob) -> Result<TxStep, String> {
        let now = Instant::now();
        let Some(started) = job.started else {
            self.radio()?.tx_start()?;
            job.started = Some(now);
            self.emit_tx(TxState::Starting);
            return Ok(TxStep::Progress);
        };
        let elapsed = now.duration_since(started).as_secs_f64();
        if job.pos < job.samples.len() {
            let played_or_queued = job.pos as f64 / RATE as f64;
            if played_or_queued - elapsed >= TX_AHEAD_S {
                return Ok(TxStep::Wait);
            }
            let end = (job.pos + TX_CHUNK).min(job.samples.len());
            self.radio()?.tx_write(&job.samples, job.pos, end)?;
            job.pos = end;
            if !job.sending {
                job.sending = true;
                self.emit_tx(TxState::Sending);
            }
            return Ok(TxStep::Progress);
        }
        let nominal_end = job.samples.len() as f64 / RATE as f64 + TX_TAIL_S;
        if elapsed < nominal_end {
            return Ok(TxStep::Wait);
        }
        Ok(TxStep::Finished)
    }

    /// One transmit step. Returns true if it did something, false if the worker may sleep.
    fn tx_step(&mut self) -> bool {
        let Some(mut job) = self.tx.take() else {
            return false;
        };
        match self.tx_advance(&mut job) {
            Ok(TxStep::Wait) => {
                self.tx = Some(job);
                false
            }
            Ok(TxStep::Progress) => {
                self.tx = Some(job);
                true
            }
            Ok(TxStep::Finished) => {
                let secs = job.samples.len() as f64 / RATE as f64;
                self.stop_radio_tx();
                self.end_rx_gap();
                self.emit_tx(TxState::Done);
                self.log(format!("sent {secs:.1} s of audio"));
                true
            }
            Err(e) => {
                self.stop_radio_tx();
                self.end_rx_gap();
                self.emit_tx(TxState::Failed(e.clone()));
                self.log(format!("transmit failed: {e}"));
                true
            }
        }
    }

    /// Ends the burst on the radio (PTT off). Always runs, even after an error.
    fn stop_radio_tx(&mut self) {
        if let Some(be) = self.backend.as_mut() {
            let _ = be.tx_stop();
        }
    }

    /// Stops the transmit in progress, if any, on shutdown.
    fn abort_tx(&mut self) {
        if self.tx.take().is_some() {
            self.stop_radio_tx();
        }
    }

    /// After a burst, the receive side starts clean: our own signal is not decoded.
    fn end_rx_gap(&mut self) {
        self.rx_buf.clear();
        self.since_scan = 0;
        self.spec = Analyser::new(SPEC_FFT, RATE as f32);
    }

    /// One RX pass: read what the radio has, update the spectrum, scan for frames.
    /// Returns the number of new samples.
    fn rx_step(&mut self) -> usize {
        if self.rx_retry_at.is_some_and(|t| Instant::now() < t) {
            return 0;
        }
        let mut fresh = Vec::new();
        let read = match self.backend.as_mut() {
            Some(be) => be.read_audio(&mut fresh),
            None => return 0,
        };
        if let Err(e) = read {
            self.report_rx(e);
            return 0;
        }
        self.rx_error = None;
        if fresh.is_empty() {
            return 0;
        }
        let n = fresh.len();
        self.rx_buf.extend_from_slice(&fresh);

        self.sense.push(&fresh);
        self.spec.push(&fresh);
        let now = Instant::now();
        if now >= self.spec_due {
            if let Some(bins) = self.spec.spectrum_db() {
                debug_assert_eq!(bins.len(), SPEC_BINS);
                self.feed_carrier(&bins);
                // Latest frame only: if the GUI is slow, older frames are replaced, not queued.
                lock(&self.shared.events).spectrum = Some(bins);
                self.spec_due = (self.spec_due + SPEC_PERIOD).max(now);
            }
        }

        self.since_scan += n;
        if self.since_scan >= SCAN_HOP {
            self.since_scan = 0;
            self.decode_frames();
        }
        n
    }

    fn report_rx(&mut self, err: String) {
        if self.rx_error.as_deref() != Some(err.as_str()) {
            self.log(format!("receive: {err}"));
            self.rx_error = Some(err);
        }
        self.rx_retry_at = Some(Instant::now() + RX_RETRY);
    }

    /// Finds frames in the RX buffer. Consumed frames leave the buffer. An
    /// incomplete frame stays until more audio arrives.
    fn decode_frames(&mut self) {
        let longest = MODULATIONS.iter().map(|&m| frame_len_for(m)).max().unwrap_or(0);
        loop {
            let Some(start) = find_preamble(&self.rx_buf) else {
                let keep = self.rx_buf.len().saturating_sub(PREAMBLE_LEN);
                self.rx_buf.drain(..keep);
                return;
            };
            // Wait until the longest possible frame is in the buffer: the modulation is not known yet.
            if start + longest > self.rx_buf.len() {
                self.rx_buf.drain(..start);
                return;
            }
            let mut found = None;
            for &m in &MODULATIONS {
                let len = frame_len_for(m);
                if let Some(payload) = decode_at(&self.rx_buf[start..start + len], m) {
                    found = Some((payload, len));
                    break;
                }
            }
            if found.is_none() {
                // Weak mode: the copies may still be arriving. Wait for all of them, then combine.
                let stride = frame_len_for(Modulation::Qpsk) + GAP_SILENCE;
                let span = (WEAK_COPIES - 1) * stride + frame_len_for(Modulation::Qpsk);
                if start + span > self.rx_buf.len() {
                    self.rx_buf.drain(..start);
                    return;
                }
                if let Some(payload) = decode_combined(&self.rx_buf[start..start + span]) {
                    self.rx_buf.drain(..start + span);
                    self.on_frame(payload);
                    continue;
                }
            }
            let (decoded, used) = match found {
                Some((payload, len)) => (Some(payload), len),
                None => (None, frame_len()),
            };
            self.rx_buf.drain(..start + used);
            if let Some(payload) = decoded {
                self.on_frame(payload);
            }
        }
    }

    fn on_frame(&mut self, payload: Vec<u8>) {
        match guard("link receive", || self.link.on_receive(&payload)) {
            Ok(actions) => self.apply(actions),
            Err(e) => self.log(e),
        }
    }

    fn deliver(&mut self, chunk: Vec<u8>) {
        self.rx_chunks.push(chunk);
        if self.rx_chunks.len() > MAX_CHUNKS {
            self.rx_chunks.clear();
            self.log("dropped a partial message: too many chunks");
            return;
        }
        match guard("session decode", || session::decode_for_relay(&self.rx_chunks)) {
            Ok(Some((signed, chunks))) => {
                self.rx_chunks.clear();
                if !dest_of(&signed.message).eq_ignore_ascii_case(&self.callsign) && self.relaying() {
                    self.hold_for_relay(&signed.message, chunks);
                    return;
                }
                // A private message is for its addressee's inbox only; other stations drop it.
                if signed.private && !dest_of(&signed.message).eq_ignore_ascii_case(&self.callsign) {
                    return;
                }
                self.on_message(signed.message);
            }
            Ok(None) => {}
            Err(e) => {
                self.rx_chunks.clear();
                self.log(e);
            }
        }
    }

    /// True while this station passes other stations' messages on: a relay role, relay mode
    /// running and setting B on (plan 4).
    fn relaying(&self) -> bool {
        let role = lock(&self.shared.relay).as_ref().map(|r| r.role);
        matches!(role, Some(Role::Relay | Role::RelayAndStation))
            && self.shared.flags.relay_running.load(Ordering::SeqCst)
            && self.shared.flags.third_party_relay.load(Ordering::SeqCst)
    }

    /// Holds another station's message to pass on unchanged. A message is held once: copies
    /// heard again (directly or round a loop of relays) are dropped, and so are our own.
    fn hold_for_relay(&mut self, msg: &Message, chunks: Vec<Vec<u8>>) {
        let (from, to) = (sender_of(msg).to_string(), dest_of(msg).to_string());
        if from.eq_ignore_ascii_case(&self.callsign) {
            return;
        }
        let digest = relay_digest(&chunks);
        if self.relayed.contains(&digest) {
            self.log(format!(
                "Already relayed the message from {from} to {to}; dropped the copy."
            ));
            self.emit_relay(&from, &to, RelayStep::Dropped("already relayed".into()));
            return;
        }
        let held = self
            .outbox
            .due(u64::MAX)
            .iter()
            .filter(|(_, item)| item.kind == KIND_RELAY)
            .count();
        if held >= MAX_HELD_FORWARDS {
            self.log(format!(
                "Relay queue is full; dropped the message from {from} to {to}."
            ));
            self.emit_relay(&from, &to, RelayStep::Dropped("relay queue is full".into()));
            return;
        }
        if self.relayed.len() >= RELAYED_MEMORY {
            self.relayed.pop_front();
        }
        self.relayed.push_back(digest);
        let item = OutItem {
            kind: KIND_RELAY.to_string(),
            payload: encode_forward(&from, &to, priority_of(msg) == Priority::Urgent, &chunks),
        };
        let oid = self.outbox.push(item, priority_of(msg));
        let pid = self.next_id();
        self.tags.insert(oid, (pid, describe_forward(&from, &to)));
        let at = self.now_ms();
        self.queued.insert(oid, (to.clone(), at));
        self.persist_outbox();
        self.log(format!("Holding a message from {from} for {to} to relay."));
        self.emit_relay(&from, &to, RelayStep::Held);
    }

    fn emit_relay(&self, from: &str, to: &str, step: RelayStep) {
        self.emit(Event::Relay {
            from: from.to_string(),
            to: to.to_string(),
            step,
        });
    }

    /// Stations an outbox item may go to next, best first: its addressee, and, once the
    /// addressee was not reached in time, the relays in `paths`. Never back to its sender.
    fn next_stations(&self, id: u64, from: &str, to: &str, paths: &[String]) -> Vec<String> {
        let mut out = vec![to.to_string()];
        if self.unreached_noted.contains(&id) {
            out.extend(
                paths
                    .iter()
                    .filter(|p| !p.eq_ignore_ascii_case(from) && !p.eq_ignore_ascii_case(to))
                    .cloned(),
            );
        }
        out
    }

    /// True when any due item, our own or held, may go to `peer` now.
    fn has_due_for(&self, peer: &str) -> bool {
        let paths = lock(&self.shared.relay_paths).clone();
        self.outbox.due(self.now_ms()).iter().any(|(id, item)| {
            peek_route(&item.payload).is_some_and(|(from, to)| {
                self.next_stations(*id, &from, &to, &paths)
                    .iter()
                    .any(|c| c.eq_ignore_ascii_case(peer))
            })
        })
    }

    /// Stations this relay should call for the messages it holds, in sending order, without
    /// repeats. A held message whose station was not reached also lists the relay paths, so
    /// it can go on over more than one hop.
    fn dial_targets(&self) -> Vec<String> {
        let paths = lock(&self.shared.relay_paths).clone();
        let mut targets: Vec<String> = Vec::new();
        for (id, item) in self.outbox.due(self.now_ms()) {
            if item.kind != KIND_RELAY {
                continue;
            }
            let Some((from, to)) = peek_route(&item.payload) else {
                continue;
            };
            for call in self.next_stations(id, &from, &to, &paths) {
                if !targets.iter().any(|t| t.eq_ignore_ascii_case(&call)) {
                    targets.push(call);
                }
            }
        }
        targets
    }

    /// Plan 4: in relay mode a relay calls the station a held message is for, inside a
    /// transmit slot, and hangs up once everything for it is acknowledged. A station that
    /// does not answer is called again after `DIAL_INTERVAL`.
    fn dial_relays(&mut self) {
        if !self.shared.flags.relay_running.load(Ordering::SeqCst) || tx_refusal(&self.shared.flags).is_some()
        {
            return;
        }
        match self.link_state.clone() {
            LinkState::Connected { peer } => {
                let ours = self
                    .dialed
                    .as_deref()
                    .is_some_and(|d| d.eq_ignore_ascii_case(&peer));
                if ours && !self.has_due_for(&peer) && self.handed.is_empty() && self.link.is_flushed() {
                    self.dialed = None;
                    self.log(format!("Relayed everything held for {peer}; hanging up."));
                    if let Err(e) = self.disconnect() {
                        self.log(e);
                    }
                }
            }
            LinkState::Idle => {
                self.dialed = None;
                if self.tx.is_some() || !self.control.is_empty() || !self.relay_slot_open() {
                    return;
                }
                let now = Instant::now();
                let next = self
                    .dial_targets()
                    .into_iter()
                    .find(|d| self.dial_after.get(d).is_none_or(|t| now >= *t));
                let Some(dest) = next else {
                    return;
                };
                self.dial_after.insert(dest.clone(), now + DIAL_INTERVAL);
                self.dialed = Some(dest.clone());
                if let Err(e) = self.connect(&dest) {
                    self.log(e);
                }
            }
            _ => {}
        }
    }

    fn on_message(&mut self, msg: Message) {
        match msg {
            Message::Chat { from, text, .. } => self.emit(Event::Message { from, text }),
            Message::Mail {
                from,
                to,
                subject,
                body,
                attachments,
            } => {
                let id = self.next_id();
                let (body, label) = split_signature(&body, &from);
                self.log(format!("mail {id} from {from}: {label}"));
                let mut text =
                    format!("From: {from}\nTo: {to}\nSubject: {subject}\nSignature: {label}\n\n{body}\n");
                if !attachments.is_empty() {
                    text.push_str("\nAttachments:\n");
                    for a in &attachments {
                        text.push_str(&format!("  {a}\n"));
                    }
                }
                match save_file(&self.inbox, &format!("mail-{id}.txt"), text.as_bytes()) {
                    Ok(p) => self.log(format!("saved mail to {}", p.display())),
                    Err(e) => self.log(e),
                }
                self.emit(Event::Mail { id, from, subject });
            }
            Message::File { name, data, .. } => {
                let id = self.next_id();
                let total = data.len() as u64;
                match save_file(&self.inbox, &name, &data) {
                    Ok(p) => self.log(format!("saved file to {}", p.display())),
                    Err(e) => self.log(e),
                }
                self.emit(Event::FileProgress {
                    id,
                    name,
                    done: total,
                    total,
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests (no real devices are opened)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use kahf_core::relay::Direction;
    use std::sync::atomic::AtomicBool;
    use std::sync::Mutex;
    use std::time::Instant;

    type Pipe = Arc<Mutex<VecDeque<f32>>>;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("kahf-daemon-test-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn config(callsign: &str, dir: PathBuf) -> Config {
        Config {
            callsign: callsign.into(),
            data_dir: dir,
            ..Default::default()
        }
    }

    #[test]
    fn rig_setting_takes_a_spec_or_a_rigctld_address() {
        let ok = |r: &str| {
            let mut c = config("N0CALL", PathBuf::from("/tmp/x"));
            c.rigctld = Some(r.into());
            validate(&c)
        };
        assert!(ok("127.0.0.1:4532").is_ok());
        assert!(ok("ic7300:/dev/ttyUSB0:115200:cat").is_ok());
        assert!(ok("ft991a:COM3:rts").is_ok());
        assert!(ok("flrig").is_ok());
        assert!(ok("ic7300").is_err());
        assert!(ok("not a rig").is_err());
    }

    /// In-memory radio: transmits into `tx`, receives from `rx`.
    struct FakeRadio {
        rx: Pipe,
        tx: Pipe,
    }

    impl Backend for FakeRadio {
        fn read_audio(&mut self, out: &mut Vec<f32>) -> Result<(), String> {
            out.extend(self.rx.lock().unwrap().drain(..));
            Ok(())
        }

        fn tx_start(&mut self) -> Result<(), String> {
            Ok(())
        }

        fn tx_write(&mut self, samples: &[f32], start: usize, end: usize) -> Result<(), String> {
            self.tx
                .lock()
                .unwrap()
                .extend(samples[start..end].iter().copied());
            Ok(())
        }

        fn tx_stop(&mut self) -> Result<(), String> {
            Ok(())
        }
    }

    /// Output that takes 30 ms to accept each write, like a busy sound card.
    struct SlowOutputRadio {
        written: Arc<AtomicU64>,
    }

    impl Backend for SlowOutputRadio {
        fn read_audio(&mut self, _out: &mut Vec<f32>) -> Result<(), String> {
            Ok(())
        }

        fn tx_start(&mut self) -> Result<(), String> {
            Ok(())
        }

        fn tx_write(&mut self, _samples: &[f32], start: usize, end: usize) -> Result<(), String> {
            thread::sleep(Duration::from_millis(30));
            self.written.fetch_add((end - start) as u64, Ordering::SeqCst);
            Ok(())
        }

        fn tx_stop(&mut self) -> Result<(), String> {
            Ok(())
        }
    }

    /// Receiver that makes 8 kHz audio in real time: 1 kHz tone, as the sound card would deliver it.
    struct ClockRadio {
        start: Instant,
        given: usize,
    }

    impl Backend for ClockRadio {
        fn read_audio(&mut self, out: &mut Vec<f32>) -> Result<(), String> {
            let due = (self.start.elapsed().as_secs_f64() * RATE as f64) as usize;
            for n in self.given..due {
                let t = n as f64 / RATE as f64;
                out.push(0.5 * (2.0 * PI * 1000.0 * t).sin() as f32);
            }
            self.given = due.max(self.given);
            Ok(())
        }

        fn tx_start(&mut self) -> Result<(), String> {
            Ok(())
        }

        fn tx_write(&mut self, _samples: &[f32], _start: usize, _end: usize) -> Result<(), String> {
            Ok(())
        }

        fn tx_stop(&mut self) -> Result<(), String> {
            Ok(())
        }
    }

    /// Polls until `want` holds or the time runs out. Used where a reply used to be waited for.
    fn wait_until(secs: u64, want: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            if want() {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        want()
    }

    fn fake_factory(rx: Pipe, tx: Pipe) -> Factory {
        Box::new(move || {
            Ok(Box::new(FakeRadio {
                rx: rx.clone(),
                tx: tx.clone(),
            }) as Box<dyn Backend>)
        })
    }

    fn wait_for(engine: &mut Engine, want: impl Fn(&Event) -> bool, secs: u64) -> Option<Event> {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            if let Some(ev) = engine.poll_events().into_iter().find(|e| want(e)) {
                return Some(ev);
            }
            thread::sleep(Duration::from_millis(20));
        }
        None
    }

    #[test]
    fn utc_stamp_matches_known_times() {
        assert_eq!(utc_stamp(0), ("19700101".to_string(), "000000".to_string()));
        assert_eq!(
            utc_stamp(1_700_000_000),
            ("20231114".to_string(), "221320".to_string())
        );
        assert_eq!(
            utc_stamp(1_709_164_800),
            ("20240229".to_string(), "000000".to_string())
        );
    }

    #[test]
    fn band_names_from_frequency() {
        assert_eq!(band_name(14.1), "20M");
        assert_eq!(band_name(7.074), "40M");
        assert_eq!(band_name(10.0), "");
    }

    #[test]
    fn queued_item_round_trips_through_bytes() {
        let mail = Message::Mail {
            from: "AA1AAA".into(),
            to: "BB1BBB".into(),
            subject: "s".into(),
            body: "URGENT: b\n".into(),
            attachments: vec!["x.bin".into()],
        };
        let file = Message::File {
            from: "AA1AAA".into(),
            to: "BB1BBB".into(),
            name: "f".into(),
            data: vec![0, 255],
        };
        for m in [mail, file] {
            assert_eq!(decode_item(&encode_message(&m, false)), Some((m, false)));
        }
        assert_eq!(
            priority_of(&Message::Chat {
                from: String::new(),
                to: String::new(),
                text: "URGENT: now".into()
            }),
            Priority::Urgent
        );
    }

    #[test]
    fn rejects_empty_callsign() {
        assert!(Engine::new(config("", temp_dir("empty"))).is_err());
    }

    #[test]
    fn rejects_bad_sdr_setting() {
        let mut c = config("T1AA", temp_dir("sdr"));
        c.sdr = Some("nosuchradio".into());
        assert!(Engine::new(c).is_err());
    }

    #[test]
    fn new_does_not_open_the_radio() {
        let opened = Arc::new(AtomicBool::new(false));
        let flag = opened.clone();
        let factory: Factory = Box::new(move || {
            flag.store(true, Ordering::SeqCst);
            Err("no radio in this test".to_string())
        });
        let engine = Engine::spawn(config("T1AA", temp_dir("lazy")), factory).unwrap();
        thread::sleep(Duration::from_millis(50));
        assert!(!opened.load(Ordering::SeqCst));
        drop(engine);
    }

    #[test]
    fn engine_starts_with_no_sdr_and_with_hackrf_listed() {
        // Start-up never opens a radio: no SDR configured, or a HackRF named as the input.
        let mut none = config("AA1AAA", temp_dir("start-no-sdr"));
        none.sdr = None;
        assert!(Engine::new(none).is_ok());

        let mut listed = config("AA1AAA", temp_dir("start-hackrf"));
        listed.sdr = Some("hackrf@145500000".into());
        match Engine::new(listed) {
            Ok(engine) => drop(engine),
            Err(e) => panic!("HackRF listed should validate: {e}"),
        }

        let mut bad = config("AA1AAA", temp_dir("start-bad-sdr"));
        bad.sdr = Some("nosuchradio@145500000".into());
        assert!(Engine::new(bad).is_err(), "unknown radio should be refused");
    }

    #[test]
    fn parses_sdr_settings() {
        let s = parse_sdr("hackrf:0000abc@14074000").unwrap();
        assert_eq!(s.radio.driver, "hackrf");
        assert_eq!(s.radio.id.as_deref(), Some("0000abc"));
        assert_eq!(s.freq_hz, 14_074_000);
        let r = parse_sdr("rtlsdr@145500000").unwrap();
        assert_eq!(r.radio.driver, "rtlsdr");
        assert_eq!(r.radio.id, None);
        assert_eq!(r.freq_hz, 145_500_000);
        assert_eq!(parse_sdr("hackrf").unwrap().freq_hz, DEFAULT_SDR_FREQ_HZ);
        assert!(parse_sdr("hackrf@0").is_err());
        assert!(parse_sdr("nosuchradio").is_err());
    }

    #[test]
    fn spectrum_peaks_at_tone_bin() {
        let x: Vec<f32> = (0..SPEC_FFT * 2)
            .map(|n| 0.5 * (2.0 * PI * 1000.0 * n as f64 / RATE as f64).sin() as f32)
            .collect();
        let mut an = Analyser::new(SPEC_FFT, RATE as f32);
        an.push(&x);
        let s = an.spectrum_db().expect("one frame is ready");
        assert_eq!(s.len(), SPEC_BINS);
        let peak = s
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;
        // 1000 Hz / (8000 Hz / 2048) = bin 256
        assert_eq!(peak, 256);
    }

    #[test]
    fn event_queue_keeps_latest_spectrum_and_important_events() {
        let mut q = EventQueue::default();
        q.push(Event::Message {
            from: "AA1AAA".into(),
            text: "keep".into(),
        });
        for i in 0..(EVENT_CAP + 10) {
            q.push(Event::Log(format!("log {i}")));
        }
        assert_eq!(q.items.len(), EVENT_CAP);
        assert!(q
            .items
            .iter()
            .any(|e| matches!(e, Event::Message { text, .. } if text == "keep")));
        assert!(!q.items.iter().any(|e| matches!(e, Event::Log(s) if s == "log 0")));
    }

    #[test]
    fn poll_events_returns_quickly_during_transmit() {
        let written = Arc::new(AtomicU64::new(0));
        let w = written.clone();
        let factory: Factory =
            Box::new(move || Ok(Box::new(SlowOutputRadio { written: w.clone() }) as Box<dyn Backend>));
        let dir = temp_dir("tx-poll");
        let mut e = Engine::spawn(config("AA1AAA", dir.clone()), factory).unwrap();
        e.listen().unwrap();
        // One second of audio: 10 writes of 30 ms each in the worker.
        e.test_burst(vec![0.1; RATE as usize]).unwrap();

        let started = Instant::now();
        let mut states = Vec::new();
        let mut slowest = Duration::ZERO;
        while written.load(Ordering::SeqCst) < RATE as u64 && started.elapsed() < Duration::from_secs(10) {
            let t = Instant::now();
            let got = e.poll_events();
            slowest = slowest.max(t.elapsed());
            states.extend(got.into_iter().filter_map(|ev| match ev {
                Event::Tx { state } => Some(state),
                _ => None,
            }));
            // The handle must also answer while the worker is sending.
            let t = Instant::now();
            let _ = e.outbox_pending();
            slowest = slowest.max(t.elapsed());
            thread::sleep(Duration::from_millis(1));
        }
        assert!(slowest < Duration::from_millis(5), "slowest call {slowest:?}");
        assert_eq!(states.first(), Some(&TxState::Starting), "{states:?}");
        assert!(states.contains(&TxState::Sending), "{states:?}");
        let done = wait_for(&mut e, |ev| matches!(ev, Event::Tx { state: TxState::Done }), 10);
        assert!(done.is_some(), "transmit did not finish");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn spectrum_arrives_at_least_20_times_a_second() {
        let factory: Factory = Box::new(|| {
            Ok(Box::new(ClockRadio {
                start: Instant::now(),
                given: 0,
            }) as Box<dyn Backend>)
        });
        let dir = temp_dir("spectrum-rate");
        let mut e = Engine::spawn(config("AA1AAA", dir.clone()), factory).unwrap();
        e.listen().unwrap();
        let started = Instant::now();
        let mut when = Vec::new();
        while started.elapsed() < Duration::from_millis(1500) {
            for ev in e.poll_events() {
                if let Event::Spectrum(bins) = ev {
                    assert_eq!(bins.len(), SPEC_BINS);
                    when.push(Instant::now());
                }
            }
            thread::sleep(Duration::from_millis(1));
        }
        let _ = fs::remove_dir_all(&dir);
        // Count the frames in the first second that has frames (the first FFT needs 256 ms of audio).
        let first = *when.first().expect("spectrum frames arrived");
        let in_one_second = when
            .iter()
            .filter(|t| t.duration_since(first) < Duration::from_secs(1))
            .count();
        assert!(in_one_second >= 20, "only {in_one_second} frames in one second");
    }

    #[test]
    fn frame_survives_air_and_is_found_by_preamble_search() {
        assert_eq!(frame_template()[..PREAMBLE_LEN].len(), PREAMBLE_LEN);
        let payload = b"hello kahf".to_vec();
        let mut air = vec![0f32; 1234];
        air.extend(burst(&[payload.clone()], MODULATION).unwrap());
        air.extend(vec![0f32; 500]);
        let start = find_preamble(&air).expect("preamble found");
        let expected = 1234 + LEAD_SILENCE;
        assert!(
            start.abs_diff(expected) <= 1,
            "start {start}, expected {expected}"
        );
        let got = decode_at(&air[start..start + frame_len()], MODULATION).expect("frame decodes");
        assert_eq!(got, payload);
    }

    #[test]
    fn back_to_back_frames_decode_in_order() {
        let payloads: Vec<Vec<u8>> = vec![b"one".to_vec(), b"two".to_vec(), b"three".to_vec()];
        let air = burst(&payloads, MODULATION).unwrap();
        let flen = frame_len();
        let mut pos = 0;
        let mut got = Vec::new();
        while let Some(rel) = find_preamble(&air[pos..]) {
            let start = pos + rel;
            if start + flen > air.len() {
                break;
            }
            got.push(decode_at(&air[start..start + flen], MODULATION).expect("frame decodes"));
            pos = start + flen;
        }
        assert_eq!(got, payloads);
    }

    #[test]
    fn weak_mode_decodes_where_one_copy_does_not() {
        let payload = b"weak mode".to_vec();
        let clean = weak_burst(&[payload.clone()]).expect("weak burst builds");
        // The first copy starts after the lead-in silence.
        let stride = frame_len_for(Modulation::Qpsk) + GAP_SILENCE;
        let copy_at = |i: usize| LEAD_SILENCE + i * stride;
        let noisy = kahf_sim::awgn(&clean, -4.0, 4242);
        let one = &noisy[copy_at(0)..copy_at(0) + frame_len_for(Modulation::Qpsk)];
        assert_eq!(
            decode_at(one, Modulation::Qpsk),
            None,
            "a single copy should fail at -4 dB"
        );
        let combined = decode_combined(&noisy[copy_at(0)..]);
        assert_eq!(combined.as_deref(), Some(payload.as_slice()));
    }

    #[test]
    fn each_modulation_decodes_only_as_itself() {
        let payload = b"KAHF modulation test".to_vec();
        for &m in &MODULATIONS {
            let air = frame_samples_as(&payload, m).expect("frame builds");
            assert_eq!(decode_at(&air, m).as_deref(), Some(payload.as_slice()));
            for &other in &MODULATIONS {
                if other != m {
                    assert_eq!(decode_at(&air, other), None, "{m:?} frame read as {other:?}");
                }
            }
        }
    }

    #[test]
    fn sdr_interpolation_keeps_dc() {
        // The Hilbert filter reaches SDR_HILBERT_HALF samples, so only the middle is checked.
        let x = vec![0.5f32; 1024];
        let iq = sdr_iq_range(&x, 0, 1024);
        assert_eq!(iq.len(), 1024 * SDR_OVERSAMPLE);
        for &(i, q) in &iq[SDR_OVERSAMPLE * 200..SDR_OVERSAMPLE * 824] {
            assert!((i - 0.5).abs() < 1e-3 && q.abs() < 1e-3);
        }
    }

    #[test]
    fn sdr_transmit_has_no_image() {
        // 1 kHz tone at audio rate; the transmitted spectrum must sit only at +1 kHz.
        let rate = RATE as f64;
        let x: Vec<f32> = (0..RATE as usize * 2)
            .map(|n| (2.0 * PI * 1000.0 * n as f64 / rate).cos() as f32)
            .collect();
        let iq = audio_to_iq(&x);
        let fs = SDR_RATE_HZ as f64;
        let (a, b) = (iq.len() / 4, iq.len() * 3 / 4);
        let level = |f: f64| {
            let (mut re, mut im) = (0f64, 0f64);
            for (n, &(i, q)) in iq[a..b].iter().enumerate() {
                let t = 2.0 * PI * f * (n + a) as f64 / fs;
                re += i as f64 * t.cos() + q as f64 * t.sin();
                im += q as f64 * t.cos() - i as f64 * t.sin();
            }
            (re * re + im * im).sqrt()
        };
        let wanted = level(1000.0);
        let image = level(-1000.0);
        assert!(wanted > 0.0);
        assert!(
            image / wanted < 0.01,
            "image is {:.1} dB down",
            20.0 * (image / wanted).log10()
        );
    }

    #[test]
    fn inbox_names_are_safe_and_unique() {
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_name("a b/c.txt"), "c.txt");
        assert_eq!(safe_name(".."), "file");
        let dir = temp_dir("inbox");
        let a = save_file(&dir.join("inbox"), "note.txt", b"1").unwrap();
        let b = save_file(&dir.join("inbox"), "note.txt", b"2").unwrap();
        assert_ne!(a, b);
        assert_eq!(fs::read(&b).unwrap(), b"2");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Two engines on crossed fake pipes: A talks to B and B talks to A, no echo.
    #[test]
    fn queued_while_offline_is_sent_after_link_up() {
        let a_to_b: Pipe = Arc::new(Mutex::new(VecDeque::new()));
        let b_to_a: Pipe = Arc::new(Mutex::new(VecDeque::new()));
        let mut a = Engine::spawn(
            config("AA1AAA", temp_dir("offline-a")),
            fake_factory(b_to_a.clone(), a_to_b.clone()),
        )
        .unwrap();
        let mut b = Engine::spawn(
            config("BB1BBB", temp_dir("offline-b")),
            fake_factory(a_to_b.clone(), b_to_a.clone()),
        )
        .unwrap();

        a.send_message("BB1BBB", "hi there").unwrap();
        assert!(wait_until(5, || a.outbox_pending().len() == 1));
        thread::sleep(Duration::from_millis(200));
        assert!(
            a_to_b.lock().unwrap().is_empty(),
            "nothing may go out before the link is up"
        );

        b.listen().unwrap();
        a.connect("BB1BBB").unwrap();
        assert!(wait_for(
            &mut a,
            |e| matches!(e, Event::Link(kahf_core::link::LinkState::Connected { .. })),
            60
        )
        .is_some());
        let got = wait_for(
            &mut b,
            |e| matches!(e, Event::Message { text, .. } if text == "hi there"),
            60,
        );
        assert!(got.is_some(), "queued message did not arrive");

        let deadline = Instant::now() + Duration::from_secs(30);
        while !a.outbox_pending().is_empty() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(a.outbox_pending().is_empty(), "sent item stayed in the outbox");
    }

    #[test]
    fn outbox_survives_restart() {
        let dir = temp_dir("restart");
        let a_to_b: Pipe = Arc::new(Mutex::new(VecDeque::new()));
        let b_to_a: Pipe = Arc::new(Mutex::new(VecDeque::new()));
        {
            let mut a = Engine::spawn(
                config("AA1AAA", dir.clone()),
                fake_factory(b_to_a.clone(), a_to_b.clone()),
            )
            .unwrap();
            a.send_message("BB1BBB", "URGENT: keep me").unwrap();
            a.send_message("BB1BBB", "second").unwrap();
        }
        assert!(dir.join("outbox.bin").exists());

        let mut a = Engine::spawn(config("AA1AAA", dir.clone()), fake_factory(b_to_a, a_to_b)).unwrap();
        assert!(wait_until(5, || a.outbox_pending().len() == 2));
        let pending = a.outbox_pending();
        assert_eq!(pending.len(), 2, "{pending:?}");
        assert_eq!(pending[0].0, 1, "urgent item stays first after reload");
        assert!(
            pending.iter().all(|(_, d)| d == "message to BB1BBB"),
            "{pending:?}"
        );
        a.send_message("BB1BBB", "third").unwrap();
        assert!(wait_until(5, || a.outbox_pending().len() == 3));
    }

    #[test]
    fn logbook_line_written_on_disconnect() {
        let a_to_b: Pipe = Arc::new(Mutex::new(VecDeque::new()));
        let b_to_a: Pipe = Arc::new(Mutex::new(VecDeque::new()));
        let dir = temp_dir("logbook-a");
        let mut a = Engine::spawn(
            config("AA1AAA", dir.clone()),
            fake_factory(b_to_a.clone(), a_to_b.clone()),
        )
        .unwrap();
        let mut b = Engine::spawn(
            config("BB1BBB", temp_dir("logbook-b")),
            fake_factory(a_to_b.clone(), b_to_a.clone()),
        )
        .unwrap();

        b.listen().unwrap();
        a.connect("BB1BBB").unwrap();
        assert!(wait_for(
            &mut a,
            |e| matches!(e, Event::Link(kahf_core::link::LinkState::Connected { .. })),
            60
        )
        .is_some());
        let contacts = fs::read_to_string(dir.join("contacts.txt")).unwrap();
        assert!(contacts.starts_with("BB1BBB\t\t\t\t"), "{contacts}");

        a.disconnect().unwrap();
        let log = dir.join("logbook.txt");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !log.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        let text = fs::read_to_string(&log).unwrap();
        assert!(
            text.lines()
                .any(|l| l.starts_with("BB1BBB,") && l.contains(",KAHF,")),
            "{text}"
        );
    }

    #[test]
    fn loopback_message_reaches_other_engine() {
        let a_to_b: Pipe = Arc::new(Mutex::new(VecDeque::new()));
        let b_to_a: Pipe = Arc::new(Mutex::new(VecDeque::new()));
        let mut a = Engine::spawn(
            config("AA1AAA", temp_dir("loop-a")),
            fake_factory(b_to_a.clone(), a_to_b.clone()),
        )
        .unwrap();
        let mut b = Engine::spawn(
            config("BB1BBB", temp_dir("loop-b")),
            fake_factory(a_to_b.clone(), b_to_a.clone()),
        )
        .unwrap();

        b.listen().unwrap();
        a.connect("BB1BBB").unwrap();
        let linked = wait_for(
            &mut a,
            |e| matches!(e, Event::Link(kahf_core::link::LinkState::Connected { .. })),
            60,
        );
        assert!(linked.is_some(), "link did not come up");

        a.send_message("BB1BBB", "hi there").unwrap();
        let got = wait_for(
            &mut b,
            |e| matches!(e, Event::Message { text, .. } if text == "hi there"),
            60,
        );
        assert!(got.is_some(), "message did not arrive");
    }

    /// One station with a fake radio whose transmit output is `a_to_b`.
    fn lone_station(tag: &str) -> (Engine, Pipe) {
        let a_to_b: Pipe = Arc::new(Mutex::new(VecDeque::new()));
        let b_to_a: Pipe = Arc::new(Mutex::new(VecDeque::new()));
        let mut a = Engine::spawn(
            config("AA1AAA", temp_dir(tag)),
            fake_factory(b_to_a, a_to_b.clone()),
        )
        .unwrap();
        a.listen().unwrap();
        (a, a_to_b)
    }

    #[test]
    fn listen_only_refuses_every_transmit_path() {
        let (mut a, a_to_b) = lone_station("listen-only");
        a.set_listen_only(true);
        let err = a.send_message("BB1BBB", "hi").unwrap_err();
        assert_eq!(err, LISTEN_ONLY_TEXT);
        assert_eq!(a.connect("BB1BBB").unwrap_err(), LISTEN_ONLY_TEXT);
        // Audio already built for transmit is dropped too.
        a.test_burst(vec![0.5; 4000]).unwrap();
        thread::sleep(Duration::from_millis(500));
        assert!(a_to_b.lock().unwrap().is_empty(), "listen only let audio out");
        // Switching it off restores transmit.
        a.set_listen_only(false);
        assert!(a.send_message("BB1BBB", "hi").is_ok());
        a.test_burst(vec![0.5; 4000]).unwrap();
        assert!(wait_until(5, || !a_to_b.lock().unwrap().is_empty()));
    }

    /// Radio that counts how often PTT would be keyed (tx_start calls).
    struct PttCountingRadio {
        keyed: Arc<AtomicU64>,
    }

    impl Backend for PttCountingRadio {
        fn read_audio(&mut self, _out: &mut Vec<f32>) -> Result<(), String> {
            Ok(())
        }

        fn tx_start(&mut self) -> Result<(), String> {
            self.keyed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn tx_write(&mut self, _samples: &[f32], _start: usize, _end: usize) -> Result<(), String> {
            Ok(())
        }

        fn tx_stop(&mut self) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn listen_only_never_keys_ptt() {
        let keyed = Arc::new(AtomicU64::new(0));
        let k = keyed.clone();
        let factory: Factory =
            Box::new(move || Ok(Box::new(PttCountingRadio { keyed: k.clone() }) as Box<dyn Backend>));
        let mut a = Engine::spawn(config("AA1AAA", temp_dir("ptt-listen-only")), factory).unwrap();
        a.listen().unwrap();
        a.set_listen_only(true);
        assert_eq!(a.send_message("BB1BBB", "hi").unwrap_err(), LISTEN_ONLY_TEXT);
        a.test_burst(vec![0.5; 4000]).unwrap();
        thread::sleep(Duration::from_millis(300));
        assert_eq!(
            keyed.load(Ordering::SeqCst),
            0,
            "PTT keyed while Listen only was on"
        );
        // Control: the same burst keys PTT once Listen only is off.
        a.set_listen_only(false);
        a.test_burst(vec![0.5; 4000]).unwrap();
        assert!(wait_until(5, || keyed.load(Ordering::SeqCst) >= 1));
    }

    #[test]
    fn required_update_lock_refuses_every_transmit_path() {
        let (mut a, a_to_b) = lone_station("update-lock");
        a.set_update_locked(true);
        assert_eq!(a.send_message("BB1BBB", "hi").unwrap_err(), UPDATE_LOCK_TEXT);
        // The update lock wins when Listen only is also on.
        a.set_listen_only(true);
        assert_eq!(a.connect("BB1BBB").unwrap_err(), UPDATE_LOCK_TEXT);
        a.set_listen_only(false);
        a.test_burst(vec![0.5; 4000]).unwrap();
        thread::sleep(Duration::from_millis(500));
        assert!(a_to_b.lock().unwrap().is_empty(), "update lock let audio out");
        a.set_update_locked(false);
        assert!(a.send_message("BB1BBB", "hi").is_ok());
    }

    #[test]
    fn unreached_station_gets_plain_sentence_once_when_relay_is_off() {
        let (mut a, _) = lone_station("relay-off");
        a.set_relay_timeout_ms(200);
        // Internet relay is on by default now, so this test turns it off to test the plain sentence.
        a.set_internet_relay(false);
        a.send_message("CC1CCC", "hello?").unwrap();
        let want = "Could not reach CC1CCC on air. The station did not respond on any relay path. Try again later, or enable internet relay in Settings.";
        let got = wait_for(&mut a, |e| *e == Event::Log(want.to_string()), 10);
        assert!(got.is_some(), "plain sentence was not logged");
        thread::sleep(Duration::from_millis(500));
        let again = a
            .poll_events()
            .iter()
            .filter(|e| matches!(e, Event::Log(s) if s.starts_with("Could not reach")))
            .count();
        assert_eq!(again, 0, "the sentence repeated");
        assert_eq!(a.outbox_pending().len(), 1, "message must stay queued");
    }

    #[test]
    fn internet_relay_and_third_party_settings_default_on() {
        let dir = temp_dir("owner-defaults");
        let e = Engine::new(config("AA1AAA", dir)).unwrap();
        assert!(
            e.shared.flags.internet_relay.load(Ordering::SeqCst),
            "setting D should default on"
        );
        assert!(e.third_party_relay(), "setting B should default on");
        e.set_third_party_relay(false);
        assert!(!e.third_party_relay());
        assert_eq!(Engine::THIRD_PARTY_RELAY_LABEL, "Relay other stations' messages");
    }

    #[test]
    fn default_internet_relay_sends_nothing_while_gateway_is_a_stub() {
        // Internet relay is on by default. The gateway stub refuses, so the message stays
        // queued and nothing goes over the internet.
        let (mut a, _) = lone_station("relay-default-on");
        a.set_relay_timeout_ms(200);
        a.send_message("CC1CCC", "hello?").unwrap();
        let got = wait_for(
            &mut a,
            |e| matches!(e, Event::Log(s) if s.contains("internet gateway is not available yet")),
            10,
        );
        assert!(got.is_some(), "stub was not reached");
        assert_eq!(a.outbox_pending().len(), 1, "stub failure must keep the message");
    }

    #[test]
    fn unreached_station_goes_to_relay_when_relay_is_on() {
        let (mut a, _) = lone_station("relay-on");
        a.set_internet_relay(true);
        a.set_relay_timeout_ms(200);
        a.send_message("CC1CCC", "hello?").unwrap();
        let got = wait_for(
            &mut a,
            |e| matches!(e, Event::Log(s) if s.starts_with("Could not reach CC1CCC on air, and internet relay failed")),
            10,
        );
        assert!(got.is_some(), "relay path was not tried");
        assert_eq!(
            a.outbox_pending().len(),
            1,
            "gateway failure must keep the message"
        );
    }

    /// A worker without a running thread, for checks on one step at a time.
    fn bare_worker(tag: &str, callsign: &str) -> Worker {
        bare_worker_in(temp_dir(tag), callsign)
    }

    /// As `bare_worker`, on a given data folder, so a test can start it twice.
    fn bare_worker_in(dir: PathBuf, callsign: &str) -> Worker {
        let (_cmd, rx) = channel();
        let shared = Arc::new(Shared {
            events: Mutex::new(EventQueue::default()),
            pending: Mutex::new(Vec::new()),
            flags: Flags::new(),
            station_key: None,
            key_error: Mutex::new(None),
            relay: Mutex::new(None),
            relay_stats: Mutex::new(Stats::default()),
            hops: Mutex::new(HopLog::new()),
            relay_paths: Mutex::new(Vec::new()),
        });
        let factory: Factory = Box::new(|| Err("no radio in this test".into()));
        let mut cfg = config(callsign, dir);
        cfg.sdr = None;
        Worker::new(cfg, rx, shared, factory, Arc::new(AtomicU64::new(1)))
    }

    fn chat(from: &str, to: &str, text: &str) -> Message {
        Message::Chat {
            from: from.into(),
            to: to.into(),
            text: text.into(),
        }
    }

    fn logged(w: &Worker) -> Vec<String> {
        lock(&w.shared.events)
            .items
            .iter()
            .filter_map(|e| match e {
                Event::Log(s) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    fn messages_seen(w: &Worker) -> Vec<String> {
        lock(&w.shared.events)
            .items
            .iter()
            .filter_map(|e| match e {
                Event::Message { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn private_send_sets_the_session_bit_and_survives_the_outbox() {
        let mut w = bare_worker("private-send", "AA1AAA");
        // The operator started this connection, so the outbox may send.
        w.shared.flags.operator_link.store(true, Ordering::SeqCst);
        w.enqueue(vec![chat("AA1AAA", "BB1BBB", "for you")], vec![7], true);
        w.link_state = LinkState::Connected {
            peer: "BB1BBB".into(),
        };
        let (_, payload) = w.next_due().expect("item is due while linked");
        assert!(
            matches!(payload, Payload::Msg { private: true, .. }),
            "outbox lost the private bit"
        );
        let air = payload.chunks().unwrap();
        assert!(session::decode_signed(&air).unwrap().private, "air bit not set");

        // The same path without the mark sends a plain message.
        let mut plain = bare_worker("plain-send", "AA1AAA");
        plain.shared.flags.operator_link.store(true, Ordering::SeqCst);
        plain.enqueue(vec![chat("AA1AAA", "BB1BBB", "for all")], vec![8], false);
        plain.link_state = LinkState::Connected {
            peer: "BB1BBB".into(),
        };
        let (_, payload) = plain.next_due().unwrap();
        assert!(matches!(payload, Payload::Msg { private: false, .. }));
        assert!(
            !session::decode_signed(&payload.chunks().unwrap())
                .unwrap()
                .private
        );
    }

    #[test]
    fn private_message_is_kept_only_by_its_addressee() {
        let private_air = session::encode_signed(&chat("AA1AAA", "BB1BBB", "secret"), None, true);
        let public_air = session::encode(&chat("AA1AAA", "BB1BBB", "open"));

        let mut addressee = bare_worker("private-rx-to", "BB1BBB");
        for c in &private_air {
            addressee.deliver(c.clone());
        }
        assert_eq!(messages_seen(&addressee), vec!["secret".to_string()]);

        let mut other = bare_worker("private-rx-other", "CC1CCC");
        for c in &private_air {
            other.deliver(c.clone());
        }
        assert!(
            messages_seen(&other).is_empty(),
            "other station kept a private message"
        );

        // The flag only narrows delivery: a public message still reaches any station.
        for c in &public_air {
            other.deliver(c.clone());
        }
        assert_eq!(messages_seen(&other), vec!["open".to_string()]);
    }

    /// Feeds the worker's carrier sense a quiet floor followed by a 1 kHz tone, so the
    /// channel at 1 kHz audio offset reads busy.
    fn make_busy(w: &mut Worker) {
        w.tuned_hz = Some(14_000_000);
        let mut seed: u32 = 12345;
        let quiet: Vec<f32> = (0..32_000)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.002
            })
            .collect();
        let tone: Vec<f32> = (0..8_000)
            .map(|n| (2.0 * PI * 1000.0 * n as f64 / RATE as f64).sin() as f32 * 0.5)
            .collect();
        w.sense.push(&quiet);
        w.sense.push(&tone);
    }

    #[test]
    fn control_burst_waits_for_a_clear_channel() {
        let mut w = bare_worker("control-wait", "BB1BBB");
        w.plan = select::Plan {
            channels: vec![(14_001_000.0, select::Pool::Open)],
        };
        make_busy(&mut w);
        assert!(
            w.channel_busy(14_001_000.0),
            "test setup: channel should read busy"
        );

        w.apply(vec![Action::Transmit(vec![1, 2, 3])]);
        assert_eq!(w.control.len(), 1, "ACK/CONNECT must queue for the channel check");
        assert!(w.bursts.is_empty(), "control burst went out on a busy channel");

        w.flush_control();
        assert_eq!(w.control.len(), 1, "busy channel must hold the frame");
        assert!(w.bursts.is_empty());
        assert!(w.access_retry_at.is_some(), "a wait must set the retry time");

        // The channel clears and the retry time passes: the frame goes out.
        w.sense = Sense::new(RATE as f32);
        w.access_retry_at = None;
        w.flush_control();
        assert!(w.control.is_empty());
        assert_eq!(w.bursts.len(), 1, "control burst not sent after the wait");
    }

    #[test]
    fn control_burst_moves_to_a_free_channel() {
        let mut w = bare_worker("control-move", "BB1BBB");
        w.plan = select::Plan {
            channels: vec![
                (14_001_000.0, select::Pool::Open),
                (14_003_000.0, select::Pool::Open),
            ],
        };
        make_busy(&mut w);
        w.apply(vec![Action::Transmit(vec![4, 5, 6])]);
        w.flush_control();
        assert!(w.control.is_empty(), "free second channel should take the frame");
        assert_eq!(w.bursts.len(), 1);
    }

    #[test]
    fn control_burst_is_dropped_after_the_last_try() {
        let mut w = bare_worker("control-giveup", "BB1BBB");
        w.apply(vec![Action::Transmit(vec![7])]);
        w.access_failures = select::MAX_ATTEMPTS;
        w.flush_control();
        assert!(w.control.is_empty());
        assert!(w.bursts.is_empty());
        assert!(logged(&w).iter().any(|l| l.contains("control burst was dropped")));
    }

    #[test]
    fn update_lock_refuses_control_bursts_too() {
        let mut w = bare_worker("control-lock", "BB1BBB");
        w.shared.flags.update_locked.store(true, Ordering::SeqCst);
        // A CONNECT or ACK produced while locked never reaches the air.
        w.apply(vec![Action::Transmit(vec![9, 9])]);
        w.flush_control();
        assert!(w.control.is_empty());
        assert!(w.bursts.is_empty(), "update lock let a control burst out");

        // A frame queued before the lock is dropped at the send point.
        w.shared.flags.update_locked.store(false, Ordering::SeqCst);
        w.apply(vec![Action::Transmit(vec![8])]);
        assert_eq!(w.control.len(), 1);
        w.shared.flags.update_locked.store(true, Ordering::SeqCst);
        w.flush_control();
        assert!(w.control.is_empty());
        assert!(w.bursts.is_empty(), "queued control burst survived the lock");
    }

    // -----------------------------------------------------------------------
    // Plan plumbing: channel access (2.1 to 2.3), signing (4), relay (4)
    // -----------------------------------------------------------------------

    /// Feeds a channel with a quiet history and then a loud one, so it reads busy.
    fn load_channel(w: &mut Worker, hz: u64) {
        let cs = w.carrier.entry(hz).or_default();
        for _ in 0..60 {
            cs.push(1e-8);
        }
        for _ in 0..20 {
            cs.push(1e-3);
        }
    }

    #[test]
    fn carrier_sense_marks_loud_channel_busy_and_quiet_one_free() {
        let mut w = bare_worker("carrier-busy", "AA1AAA");
        w.tuned_hz = Some(14_000_000);
        load_channel(&mut w, 14_000_000);
        assert!(w.channel_busy(14_000_000.0), "loud channel should read busy");
        // A channel with no energy history is free unless the start marker is seen.
        assert!(
            !w.channel_busy(14_000_000.0 + 2_700.0),
            "unmeasured channel read busy"
        );
    }

    #[test]
    fn carrier_sense_ignores_channels_outside_the_audio() {
        let mut w = bare_worker("carrier-outside", "AA1AAA");
        w.tuned_hz = Some(14_000_000);
        // 1 MHz above the tuned frequency is not in the receive audio: never called busy.
        load_channel(&mut w, 15_000_000);
        assert!(!w.channel_busy(15_000_000.0));
    }

    #[test]
    fn spectrum_feeds_one_carrier_frame_per_period() {
        let mut w = bare_worker("carrier-feed", "AA1AAA");
        let bins = vec![-60.0f32; SPEC_BINS];
        // The plan comes from the band plan, so take a real channel inside the receive audio.
        let ch = w.plan.channels[0].0;
        w.tuned_hz = Some((ch - 1_000.0) as u64);
        let key = carrier_key(ch);
        w.feed_carrier(&bins);
        assert!(w.carrier.contains_key(&key), "audible channel not fed");
        // A channel 5 kHz up is outside the audio and is not fed.
        assert!(
            !w.carrier.contains_key(&carrier_key(ch + 5_000.0)),
            "channel 5 kHz up is audible"
        );
        let before = w.carrier[&key].level();
        // A second call inside the frame period adds nothing.
        w.feed_carrier(&bins);
        assert_eq!(w.carrier[&key].level(), before);
    }

    #[test]
    fn sign_body_round_trips_and_detects_changes() {
        let key = StationKey::generate();
        let signed = sign_body(&key, "AA1AAA", "hello there");
        let (text, label) = split_signature(&signed, "AA1AAA");
        assert_eq!(text, "hello there");
        assert!(label.starts_with("Signed by AA1AAA, key "), "label {label}");
        // Claimed by someone else.
        let (_, other) = split_signature(&signed, "BB1BBB");
        assert!(other.contains("not the sender"), "label {other}");
        // Changed text.
        let (_, tampered) = split_signature(&signed.replace("hello", "hellO"), "AA1AAA");
        assert!(tampered.starts_with("Signature not valid"), "label {tampered}");
        // No signature at all, and a damaged one.
        assert_eq!(split_signature("plain", "AA1AAA").1, "Not signed");
        let broken = format!("hi{SIG_MARK}zz\n");
        assert!(split_signature(&broken, "AA1AAA").1.contains("not hex"));
    }

    #[test]
    fn engine_creates_station_key_at_start_up() {
        let dir = temp_dir("key-start");
        let engine = Engine::new(config("AA1AAA", dir.clone())).unwrap();
        assert!(dir.join(auth::KEY_FILE).exists(), "station key not created");
        assert!(engine.shared.station_key.is_some());
        // Mail is signed by default and can be turned off.
        assert!(engine.shared.flags.sign_mail.load(Ordering::SeqCst));
        engine.set_mail_signing(false);
        assert!(!engine.shared.flags.sign_mail.load(Ordering::SeqCst));
        let _ = engine;
    }

    #[test]
    fn relay_schedule_is_checked_and_held_outside_transmit_slots() {
        let mut w = bare_worker("relay-gate", "AA1AAA");
        let now = now_unix_secs() as u64;
        let transmit = |start: u64| Slot {
            channel_khz: 14_000.0,
            start_unix: start,
            duration_s: 600,
            direction: Direction::Transmit,
        };
        let receive = Slot {
            channel_khz: 14_000.0,
            start_unix: now - 300,
            duration_s: 600,
            direction: Direction::Receive,
        };
        // No role: nothing is held.
        assert!(w.relay_slot_open());
        *lock(&w.shared.relay) =
            Some(Relay::new("AA1AAA", Role::Relay, relay::Radio::HalfDuplex, &[14_000.0]).unwrap());
        // A role with no schedule is held: no slot, no send.
        assert!(!w.relay_slot_open(), "relay with no schedule was allowed");
        // Receive slot only: held.
        lock(&w.shared.relay)
            .as_mut()
            .unwrap()
            .accept_schedule(vec![receive])
            .unwrap();
        assert!(!w.relay_slot_open(), "relay sent in a receive slot");
        // A transmit slot covering now: open, but only with relay mode running.
        lock(&w.shared.relay)
            .as_mut()
            .unwrap()
            .accept_schedule(vec![transmit(now - 10)])
            .unwrap();
        assert!(!w.relay_slot_open(), "slot open while relay mode is off");
        w.shared.flags.relay_running.store(true, Ordering::SeqCst);
        assert!(w.relay_slot_open());
        // Wrong channel is refused and the old schedule stays.
        let bad = Slot {
            channel_khz: 7_000.0,
            ..transmit(now - 10)
        };
        assert!(lock(&w.shared.relay)
            .as_mut()
            .unwrap()
            .accept_schedule(vec![bad])
            .is_err());
        assert!(w.relay_slot_open());
        w.record_forward(9);
    }

    #[test]
    fn forwards_are_counted_and_recorded_by_relay_only() {
        let mut w = bare_worker("relay-forward", "AA1AAA");
        // A plain station records nothing.
        w.record_forward(1);
        assert_eq!(lock(&w.shared.relay_stats).sent_total, 0);
        *lock(&w.shared.relay) =
            Some(Relay::new("AA1AAA", Role::Relay, relay::Radio::HalfDuplex, &[14_000.0]).unwrap());
        w.tuned_hz = Some(14_000_000);
        w.record_forward(42);
        w.record_forward(42);
        assert_eq!(lock(&w.shared.relay_stats).sent_total, 2);
        let hops = lock(&w.shared.hops).chain_for(42);
        assert_eq!(hops.len(), 2);
        assert_eq!(hops[0].relay, "AA1AAA");
        assert_eq!(hops[0].channel_khz, 14_000.0);
        assert!(lock(&w.shared.hops).chain_for(7).is_empty());
    }

    #[test]
    fn relay_start_needs_both_acknowledgements() {
        let dir = temp_dir("relay-ack");
        let mut e = Engine::new(config("AA1AAA", dir)).unwrap();
        // No role yet: refused with a plain message.
        let both = RelayAck {
            licence_held: true,
            terms_accepted: true,
        };
        assert!(e.start_relay(both).unwrap_err().contains("relay role"));
        e.configure_relay(Role::Relay, relay::Radio::HalfDuplex, &[14_000.0])
            .unwrap();
        // Either flag missing: refused, and relay stays off.
        let no_licence = RelayAck {
            licence_held: false,
            terms_accepted: true,
        };
        let no_terms = RelayAck {
            licence_held: true,
            terms_accepted: false,
        };
        let neither = RelayAck::default();
        for ack in [no_licence, no_terms, neither] {
            let err = e.start_relay(ack).unwrap_err();
            assert!(err.starts_with("Relay mode did not start"), "got {err}");
            assert!(!e.relay_running(), "relay started with {ack:?}");
        }
        // Both true: accepted.
        e.start_relay(both).unwrap();
        assert!(e.relay_running());
        // Operator stops it: off again, and nothing was saved to start it next time.
        e.stop_relay();
        assert!(!e.relay_running());
    }

    #[test]
    fn relay_start_is_per_run_and_not_saved() {
        let dir = temp_dir("relay-per-run");
        {
            let mut e = Engine::new(config("AA1AAA", dir.clone())).unwrap();
            e.configure_relay(Role::Relay, relay::Radio::HalfDuplex, &[14_000.0])
                .unwrap();
            e.start_relay(RelayAck {
                licence_held: true,
                terms_accepted: true,
            })
            .unwrap();
        }
        // A new run starts with relay mode off, whatever the old run had.
        let mut again = Engine::new(config("AA1AAA", dir)).unwrap();
        assert!(!again.relay_running());
        again
            .configure_relay(Role::Relay, relay::Radio::HalfDuplex, &[14_000.0])
            .unwrap();
        assert!(!again.relay_running());
    }

    #[test]
    fn relay_with_no_schedule_is_held() {
        let mut w = bare_worker("relay-no-schedule", "AA1AAA");
        *lock(&w.shared.relay) =
            Some(Relay::new("AA1AAA", Role::Relay, relay::Radio::HalfDuplex, &[14_000.0]).unwrap());
        w.shared.flags.relay_running.store(true, Ordering::SeqCst);
        w.shared.flags.operator_link.store(true, Ordering::SeqCst);
        assert!(!w.relay_slot_open(), "relay with no schedule was allowed");
        // Queued items stay queued too.
        w.enqueue(vec![chat("AA1AAA", "BB1BBB", "held")], vec![3], false);
        w.link_state = LinkState::Connected {
            peer: "BB1BBB".into(),
        };
        assert!(w.next_due().is_none(), "relay with no schedule sent an item");
    }

    #[test]
    fn station_without_relay_role_is_not_held_by_slots() {
        let w = bare_worker("station-no-role", "AA1AAA");
        assert!(w.relay_slot_open());
    }

    #[test]
    fn unattended_sends_need_relay_mode_or_an_operator_connection() {
        let mut w = bare_worker("unattended", "AA1AAA");
        w.enqueue(vec![chat("AA1AAA", "BB1BBB", "queued")], vec![5], false);
        w.link_state = LinkState::Connected {
            peer: "BB1BBB".into(),
        };
        // Neither relay mode nor an operator connection: no send, no retry.
        assert!(!w.transmit_allowed());
        assert!(w.next_due().is_none(), "outbox sent without an operator or relay");
        // The operator starts a connection: the send may go.
        w.shared.flags.operator_link.store(true, Ordering::SeqCst);
        assert!(w.transmit_allowed());
        assert!(w.next_due().is_some());
        // Disconnect clears the operator flag (the engine does this on disconnect).
        w.shared.flags.operator_link.store(false, Ordering::SeqCst);
        assert!(!w.transmit_allowed());
    }

    #[test]
    fn listen_only_blocks_every_send_even_in_relay_mode() {
        let mut w = bare_worker("listen-blocks", "AA1AAA");
        w.enqueue(vec![chat("AA1AAA", "BB1BBB", "blocked")], vec![9], false);
        w.link_state = LinkState::Connected {
            peer: "BB1BBB".into(),
        };
        w.shared.flags.operator_link.store(true, Ordering::SeqCst);
        w.shared.flags.relay_running.store(true, Ordering::SeqCst);
        *lock(&w.shared.relay) =
            Some(Relay::new("AA1AAA", Role::Relay, relay::Radio::HalfDuplex, &[14_000.0]).unwrap());
        let now = now_unix_secs() as u64;
        lock(&w.shared.relay)
            .as_mut()
            .unwrap()
            .accept_schedule(vec![Slot {
                channel_khz: 14_000.0,
                start_unix: now - 10,
                duration_s: 600,
                direction: Direction::Transmit,
            }])
            .unwrap();
        assert!(w.next_due().is_some(), "setup: send should be allowed");
        w.shared.flags.listen_only.store(true, Ordering::SeqCst);
        assert!(w.next_due().is_none(), "Listen only let an outbox send out");
        w.shared.flags.listen_only.store(false, Ordering::SeqCst);
        assert!(w.next_due().is_some());
    }

    #[test]
    fn listen_only_refuses_engine_send_in_relay_mode() {
        let dir = temp_dir("listen-relay-engine");
        let mut e = Engine::new(config("AA1AAA", dir)).unwrap();
        e.configure_relay(Role::Relay, relay::Radio::HalfDuplex, &[14_000.0])
            .unwrap();
        e.start_relay(RelayAck {
            licence_held: true,
            terms_accepted: true,
        })
        .unwrap();
        e.set_listen_only(true);
        assert_eq!(e.send_message("BB1BBB", "hi").unwrap_err(), LISTEN_ONLY_TEXT);
        assert_eq!(e.connect("BB1BBB").unwrap_err(), LISTEN_ONLY_TEXT);
    }

    // -----------------------------------------------------------------------
    // Radio relay (plan 4 and 6.3)
    // -----------------------------------------------------------------------

    /// Shared air: a station hears every pipe it reads and transmits into each listener's pipe.
    struct AirRadio {
        rx: Pipe,
        tx: Vec<Pipe>,
    }

    impl Backend for AirRadio {
        fn read_audio(&mut self, out: &mut Vec<f32>) -> Result<(), String> {
            out.extend(self.rx.lock().unwrap().drain(..));
            Ok(())
        }

        fn tx_start(&mut self) -> Result<(), String> {
            Ok(())
        }

        fn tx_write(&mut self, samples: &[f32], start: usize, end: usize) -> Result<(), String> {
            for p in &self.tx {
                p.lock().unwrap().extend(samples[start..end].iter().copied());
            }
            Ok(())
        }

        fn tx_stop(&mut self) -> Result<(), String> {
            Ok(())
        }
    }

    fn air_factory(rx: Pipe, tx: Vec<Pipe>) -> Factory {
        Box::new(move || {
            Ok(Box::new(AirRadio {
                rx: rx.clone(),
                tx: tx.clone(),
            }) as Box<dyn Backend>)
        })
    }

    fn pipe() -> Pipe {
        Arc::new(Mutex::new(VecDeque::new()))
    }

    /// A transmit slot on 14 MHz covering the next hour.
    fn open_slot() -> Slot {
        Slot {
            channel_khz: 14_000.0,
            start_unix: now_unix_secs() as u64 - 10,
            duration_s: 3_600,
            direction: Direction::Transmit,
        }
    }

    fn running_relay(e: &Engine) {
        e.configure_relay(Role::Relay, relay::Radio::HalfDuplex, &[14_000.0])
            .unwrap();
        e.accept_relay_schedule(vec![open_slot()]).unwrap();
        e.start_relay(RelayAck {
            licence_held: true,
            terms_accepted: true,
        })
        .unwrap();
    }

    /// A and B cannot hear each other; R hears both. A's message reaches B through R, which
    /// calls B by itself in relay mode and keeps the message out of its own inbox.
    #[test]
    fn message_reaches_an_unheard_station_through_a_radio_relay() {
        let (to_a, to_r, to_b) = (pipe(), pipe(), pipe());
        let mut a = Engine::spawn(
            config("AA1AAA", temp_dir("relay3-a")),
            air_factory(to_a.clone(), vec![to_r.clone()]),
        )
        .unwrap();
        let mut r = Engine::spawn(
            config("RR1RRR", temp_dir("relay3-r")),
            air_factory(to_r.clone(), vec![to_a.clone(), to_b.clone()]),
        )
        .unwrap();
        let mut b = Engine::spawn(
            config("BB1BBB", temp_dir("relay3-b")),
            air_factory(to_b.clone(), vec![to_r.clone()]),
        )
        .unwrap();
        running_relay(&r);
        r.listen().unwrap();
        b.listen().unwrap();

        a.set_relay_paths(&["RR1RRR"]).unwrap();
        a.set_relay_timeout_ms(200);
        a.send_message("BB1BBB", "through the relay").unwrap();
        assert!(
            wait_for(
                &mut a,
                |e| matches!(e, Event::Log(s) if s.contains("radio relay through RR1RRR")),
                10
            )
            .is_some(),
            "A did not switch to the relay path"
        );
        a.connect("RR1RRR").unwrap();
        assert!(
            wait_until(60, || a.outbox_pending().is_empty()),
            "A never handed the message to R"
        );
        assert!(
            wait_for(
                &mut r,
                |e| matches!(e, Event::Log(s) if s.starts_with("Holding a message from AA1AAA for BB1BBB")),
                30
            )
            .is_some(),
            "R did not hold the message"
        );
        a.disconnect().unwrap();

        let got = wait_for(
            &mut b,
            |e| matches!(e, Event::Message { from, text } if from == "AA1AAA" && text == "through the relay"),
            90,
        );
        assert!(got.is_some(), "B never got the relayed message");
        assert!(
            wait_until(30, || r.outbox_pending().is_empty()),
            "R kept the forward"
        );
        assert_eq!(r.relay_stats().sent_total, 1, "one forward counted");
        let r_saw_it = r
            .poll_events()
            .iter()
            .any(|e| matches!(e, Event::Message { text, .. } if text == "through the relay"));
        assert!(!r_saw_it, "a relayed message must not land in the relay's inbox");
    }

    /// Four stations in a line, each hearing only its neighbours: A - R1 - R2 - B. R1 cannot
    /// reach B, so after its relay timeout it hands the message to R2, which reaches B.
    #[test]
    fn message_crosses_two_radio_relays() {
        let (to_a, to_r1, to_r2, to_b) = (pipe(), pipe(), pipe(), pipe());
        let spawn = |call: &str, tag: &str, rx: &Pipe, tx: Vec<Pipe>| {
            Engine::spawn(config(call, temp_dir(tag)), air_factory(rx.clone(), tx)).unwrap()
        };
        let mut a = spawn("AA1AAA", "hop2-a", &to_a, vec![to_r1.clone()]);
        let mut r1 = spawn("RR1RRR", "hop2-r1", &to_r1, vec![to_a.clone(), to_r2.clone()]);
        let mut r2 = spawn("RR2RRR", "hop2-r2", &to_r2, vec![to_r1.clone(), to_b.clone()]);
        let mut b = spawn("BB1BBB", "hop2-b", &to_b, vec![to_r2.clone()]);
        for r in [&r1, &r2] {
            running_relay(r);
            r.set_relay_timeout_ms(200);
        }
        r1.set_relay_paths(&["RR2RRR"]).unwrap();
        for e in [&mut r1, &mut r2, &mut b] {
            e.listen().unwrap();
        }

        a.set_relay_paths(&["RR1RRR"]).unwrap();
        a.set_relay_timeout_ms(200);
        a.send_message("BB1BBB", "two hops").unwrap();
        thread::sleep(Duration::from_millis(400));
        a.connect("RR1RRR").unwrap();
        assert!(
            wait_until(60, || a.outbox_pending().is_empty()),
            "A never handed the message to R1"
        );
        a.disconnect().unwrap();

        let got = wait_for(
            &mut b,
            |e| matches!(e, Event::Message { from, text } if from == "AA1AAA" && text == "two hops"),
            120,
        );
        assert!(got.is_some(), "B never got the message over two relays");
        assert!(wait_until(30, || r1.outbox_pending().is_empty()
            && r2.outbox_pending().is_empty()));
        assert_eq!(r1.relay_stats().sent_total, 1);
        assert_eq!(r2.relay_stats().sent_total, 1);
        let sent = |e: &Event| matches!(e, Event::Relay { step: RelayStep::Sent, to, .. } if to == "BB1BBB");
        assert!(r2.poll_events().iter().any(sent) || wait_for(&mut r2, sent, 5).is_some());
    }

    fn relaying_worker(tag: &str) -> Worker {
        let w = bare_worker(tag, "RR1RRR");
        let mut relay = Relay::new("RR1RRR", Role::Relay, relay::Radio::HalfDuplex, &[14_000.0]).unwrap();
        relay.accept_schedule(vec![open_slot()]).unwrap();
        *lock(&w.shared.relay) = Some(relay);
        w.shared.flags.relay_running.store(true, Ordering::SeqCst);
        w
    }

    /// Feeds a whole message to the worker as the link would deliver it.
    fn hear(w: &mut Worker, msg: &Message) {
        for c in session::encode(msg) {
            w.deliver(c);
        }
    }

    #[test]
    fn relay_holds_each_message_once_and_never_its_own() {
        let mut w = relaying_worker("relay-hold");
        let m = chat("AA1AAA", "BB1BBB", "once only");
        hear(&mut w, &m);
        hear(&mut w, &m);
        hear(&mut w, &chat("RR1RRR", "BB1BBB", "came back"));
        let held = w.outbox.due(u64::MAX);
        assert_eq!(held.len(), 1, "a copy or our own message was held");
        assert_eq!(held[0].1.kind, KIND_RELAY);
        match decode_payload(&held[0].1.payload) {
            Some(Payload::Forward { from, to, chunks, .. }) => {
                assert_eq!((from.as_str(), to.as_str()), ("AA1AAA", "BB1BBB"));
                assert_eq!(chunks, session::encode(&m), "chunks must go on unchanged");
            }
            other => panic!("not a forward: {other:?}"),
        }
        assert_eq!(w.dial_targets(), vec!["BB1BBB".to_string()]);
    }

    #[test]
    fn nothing_is_held_without_relay_mode_or_setting_b() {
        let mut off = relaying_worker("relay-b-off");
        off.shared.flags.third_party_relay.store(false, Ordering::SeqCst);
        hear(&mut off, &chat("AA1AAA", "BB1BBB", "x"));
        assert!(
            off.outbox.due(u64::MAX).is_empty(),
            "setting B off must not relay"
        );

        let mut stopped = relaying_worker("relay-stopped");
        stopped.shared.flags.relay_running.store(false, Ordering::SeqCst);
        hear(&mut stopped, &chat("AA1AAA", "BB1BBB", "x"));
        assert!(
            stopped.outbox.due(u64::MAX).is_empty(),
            "relay mode off must not relay"
        );

        // Our own mail is never held, even while relaying.
        let mut own = relaying_worker("relay-own");
        hear(&mut own, &chat("AA1AAA", "RR1RRR", "for the relay itself"));
        assert!(own.outbox.due(u64::MAX).is_empty());
    }

    #[test]
    fn urgent_messages_stay_urgent_through_a_relay() {
        let mut w = relaying_worker("relay-urgent");
        hear(&mut w, &chat("AA1AAA", "BB1BBB", "routine"));
        hear(&mut w, &chat("AA1AAA", "BB1BBB", "URGENT: need help"));
        let due = w.outbox.due(u64::MAX);
        let first = decode_payload(&due[0].1.payload).unwrap();
        assert!(
            first.urgent(),
            "the urgent forward must go first and use the urgent pool"
        );
    }

    #[test]
    fn relay_remembers_held_messages_across_a_restart() {
        let dir = temp_dir("relay-restart");
        let m = chat("AA1AAA", "BB1BBB", "only once");
        let make = |dir: &PathBuf| {
            let w = bare_worker_in(dir.clone(), "RR1RRR");
            let mut relay = Relay::new("RR1RRR", Role::Relay, relay::Radio::HalfDuplex, &[14_000.0]).unwrap();
            relay.accept_schedule(vec![open_slot()]).unwrap();
            *lock(&w.shared.relay) = Some(relay);
            w.shared.flags.relay_running.store(true, Ordering::SeqCst);
            w
        };
        let mut w = make(&dir);
        hear(&mut w, &m);
        assert_eq!(w.outbox.due(u64::MAX).len(), 1);
        drop(w);
        let mut again = make(&dir);
        assert_eq!(again.outbox.due(u64::MAX).len(), 1, "held forward reloads");
        hear(&mut again, &m);
        assert_eq!(
            again.outbox.due(u64::MAX).len(),
            1,
            "a copy heard after restart was held twice"
        );
    }

    #[test]
    fn relay_queue_is_capped() {
        let mut w = relaying_worker("relay-cap");
        for i in 0..MAX_HELD_FORWARDS + 3 {
            hear(&mut w, &chat("AA1AAA", "BB1BBB", &format!("n{i}")));
        }
        assert_eq!(w.outbox.due(u64::MAX).len(), MAX_HELD_FORWARDS);
    }

    #[test]
    fn forward_payload_round_trips_and_peeks() {
        let chunks = vec![vec![1, 2, 3], vec![4]];
        let bytes = encode_forward("AA1AAA", "BB1BBB", true, &chunks);
        assert_eq!(peek_route(&bytes), Some(("AA1AAA".into(), "BB1BBB".into())));
        assert_eq!(
            decode_payload(&bytes),
            Some(Payload::Forward {
                from: "AA1AAA".into(),
                to: "BB1BBB".into(),
                urgent: true,
                chunks
            })
        );
        let m = chat("AA1AAA", "CC1CCC", "hi");
        let own = encode_message(&m, true);
        assert_eq!(peek_route(&own), Some(("AA1AAA".into(), "CC1CCC".into())));
        assert_eq!(
            decode_payload(&own),
            Some(Payload::Msg {
                msg: m,
                private: true
            })
        );
        assert_eq!(decode_payload(&bytes[..bytes.len() - 1]), None);
    }

    #[test]
    fn relay_paths_are_checked() {
        let e = Engine::new(config("AA1AAA", temp_dir("relay-paths"))).unwrap();
        assert!(e.set_relay_paths(&["not a call"]).is_err());
        assert!(e.set_relay_paths(&["AA1AAA"]).is_err(), "relay through itself");
        e.set_relay_paths(&["RR1RRR", "rr1rrr", "QQ1QQQ"]).unwrap();
        assert_eq!(e.relay_paths(), vec!["RR1RRR".to_string(), "QQ1QQQ".to_string()]);
        e.set_relay_paths(&[]).unwrap();
        assert!(e.relay_paths().is_empty());
    }

    #[test]
    fn relay_paths_come_from_the_route_planner() {
        let e = Engine::new(config("OH2AAA", temp_dir("relay-plan"))).unwrap();
        // Helsinki to Tallinn is in the 17 m skip zone; Madrid bridges it by day.
        let me = route::Node::new("OH2AAA", (60.17, 24.94));
        let to = route::Node::new("ES1BBB", (59.44, 24.75));
        let relays = [route::Node::new("EA4DDD", (40.42, -3.70))];
        let day = route::Conditions {
            solar_flux: 150.0,
            ..route::Conditions::band_17m(12, 6)
        };
        let paths = e.plan_relay_paths(&me, &to, &relays, &day).unwrap();
        assert_eq!(paths, vec!["EA4DDD".to_string()]);
        assert_eq!(e.relay_paths(), paths);
    }

    /// Connected to B, an item for C at the head of the queue must not hold back B's.
    #[test]
    fn items_for_other_stations_do_not_block_the_connected_one() {
        let mut w = bare_worker("no-hol", "AA1AAA");
        w.shared.flags.operator_link.store(true, Ordering::SeqCst);
        w.enqueue(vec![chat("AA1AAA", "CC1CCC", "first")], vec![1], false);
        w.enqueue(vec![chat("AA1AAA", "BB1BBB", "second")], vec![2], false);
        w.link_state = LinkState::Connected {
            peer: "BB1BBB".into(),
        };
        let (_, p) = w.next_due().expect("B's item must be due");
        assert_eq!(
            p,
            Payload::Msg {
                msg: chat("AA1AAA", "BB1BBB", "second"),
                private: false
            }
        );
    }
}
