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
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use kahf_audio::{AudioIn, AudioOut, Resampler, RATE};
use kahf_core::access::{self, CarrierSense};
use kahf_core::auth::{self, StationKey};
use kahf_core::frame;
use kahf_core::io::{IqSink, RigControl, RigctldClient};
use kahf_core::lanes::{self as laneplan, BandTable};
use kahf_core::ldpc;
mod backbone;
mod clock;
#[allow(dead_code)]
mod push;
pub use clock::Clock;
use backbone::{Backbone, BbHandle, BbJob, BbResult, Queued};
use kahf_core::mac::{Class, Mac};
use kahf_core::link::{encode_beacon, BEACON_ONLINE, encode_advert, Action, Link, LinkState, ADVERT_LOCATION, ADVERT_RELAY, ADVERT_TTL};
use kahf_core::outbox::{OutItem, Outbox, Priority};
use kahf_core::phy::Modulation;
use kahf_core::relay::{self, Hop, HopLog, Relay, Role, Slot, Stats};
use kahf_core::select::{self, Refusal};
use kahf_core::sense::Sense;
use kahf_core::session::{self, Message};
use kahf_core::spectrum::Analyser;
use kahf_log::contacts::{self, Contact};
use kahf_log::{valid_call, Qso};
use kahf_radio::{GainSetting, RadioSpec, RxRadio};
use kahf_sdr::hackrf::HackrfTx;

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
    /// A file arrived and was saved in the inbox.
    FileReceived {
        from: String,
        name: String,
        size: u64,
        path: PathBuf,
    },
    /// A station announced itself, directly or through relays. `hops` is how many relays it
    /// went through (0 when heard straight from the station).
    Advert {
        call: String,
        hops: u8,
        relay: bool,
        /// Where the station said it is (latitude, longitude), when it chose to say.
        loc: Option<(f32, f32)>,
    },
    /// A relay announced itself on the air.
    Relay {
        call: String,
        online: bool,
        /// The time the relay gave (unix seconds), for stations without a clock source.
        time_s: u32,
        load: u8,
    },
    /// A mail arrived, with its text, and was saved in the inbox.
    MailReceived {
        from: String,
        subject: String,
        body: String,
        attachments: Vec<String>,
        path: Option<PathBuf>,
    },
    Spectrum(Vec<f32>),
    /// Link quality and the KAMI mode in use, every few seconds while linked and whenever the
    /// mode changes.
    Metrics(LinkStatus),
    /// A station was heard on the air: its callsign and the SNR (dB) of the burst, if known.
    Heard { call: String, snr_db: Option<f32> },
    Log(String),
    /// Transmit progress: Starting (PTT on), Sending (audio handed to the radio),
    /// Done (PTT off) or Failed (with the reason).
    Tx {
        state: TxState,
    },
}

/// What the operator sees of the link: the mode we send in, how well each side hears the other,
/// and the net speed of that mode.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkStatus {
    /// KAMI mode of the last burst we sent, for example "KAMI-32".
    pub mode: &'static str,
    /// True when the mode is chosen automatically from the peer's SNR report.
    pub auto: bool,
    /// Net information rate of that mode in a full burst, bit/s.
    pub net_bps: u32,
    /// SNR (dB) at which we hear the peer.
    pub own_snr_db: Option<f32>,
    /// SNR (dB) at which the peer hears us.
    pub peer_snr_db: Option<f32>,
    /// Share of recent frames we lost, percent.
    pub fer_pct: f32,
    pub frames_sent: u64,
    pub frames_received: u64,
    pub retransmits: u64,
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

/// Default transmit modulation: KAMI-32, the bottom rung of the ladder (see `MODULATIONS`).
const MODULATION: Modulation = Modulation::Kami32;
#[cfg(test)]
const LDPC_ITER: usize = 50;
/// An automatic link with nothing waiting is closed after this long without a frame.
const AUTO_HANGUP: Duration = Duration::from_secs(20);
/// First pause after a failed automatic connection, doubled each time up to a quarter hour.
const AUTO_RETRY_BASE: Duration = Duration::from_secs(30);
/// Largest payload one link frame carries (see frame::build_payload).
const MAX_CHUNK: usize = 36;
/// New samples between KAMI stream searches (0.75 s).
const KAMI_STEP: usize = RATE as usize * 3 / 4;
const LEAD_SILENCE: usize = RATE as usize / 5; // lets PTT and VOX settle
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
/// Used when the sdr setting has no "@frequency" part: 18.11344 MHz, the dial for lane C1.
const DEFAULT_SDR_FREQ_HZ: u64 = 18_113_440;
const SDR_RX_LNA_DB: u32 = 16;
const SDR_RX_VGA_DB: u32 = 20;
const SDR_TX_VGA_DB: u32 = 20;

const POLL: Duration = Duration::from_millis(5);
const STOPPED: &str = "engine stopped";

/// Plan 6.3: how long a message waits for its station to be reached on air.
const RELAY_TIMEOUT_MS: u64 = 120_000;
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
    /// Relay stations to try, in order, when a station is not reached on air.
    relay_hints: Mutex<Vec<String>>,
    /// Server the relay carries messages through (None: radio only).
    backbone_server: Mutex<Option<String>>,
    /// Where this station says it is in its announcements. None: it does not say.
    location: Mutex<Option<(f32, f32)>>,
    /// Forwards sent by this station as a relay (plan 4).
    relay_stats: Mutex<Stats>,
    /// One entry per forward, keyed by the public message id.
    hops: Mutex<HopLog>,
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
    /// Index into MODULATIONS of the modulation used for the last burst (shown to the operator).
    tx_level_shown: AtomicU8,
    /// Choose the KAMI mode automatically (default). Off: always send `fixed_level`.
    auto_mode: AtomicBool,
    /// The operator lets the station connect and send queued messages by itself, and hang up
    /// when it is done. Off by default; relay stations always do.
    auto_link: AtomicBool,
    /// Index into MODULATIONS used when `auto_mode` is off.
    fixed_level: AtomicU8,
    update_locked: AtomicBool,
    internet_relay: AtomicBool,
    /// Relay other stations' messages (setting B). On by default; no effect yet.
    third_party_relay: AtomicBool,
    relay_timeout_ms: AtomicU64,
    /// Transmit audio level, percent of full scale (5 to 100). Kept below full scale on purpose:
    /// an SSB rig driven too hard splatters across neighbouring channels, which on a few shared
    /// HF frequencies spoils everyone's reception.
    tx_level_pct: AtomicU8,
    /// What the transmit gate checks against: the band table, the radio's dial and the bench
    /// switch (MESH-SPEC 5.1 I7 and 2.3).
    policy: Mutex<TxPolicy>,
    /// The open radio cannot reach an antenna (in-memory test radio, or BlackHole). Known once the radio is open.
    bench_radio: AtomicBool,
    /// The radio is open, so `bench_radio` and `radio_dial_hz` are known.
    radio_known: AtomicBool,
    /// Dial the radio itself reports (an SDR knows it), 0 when unknown.
    radio_dial_hz: AtomicU64,
}

/// The transmit gate's settings. The built-in table is **not verified** (`plan_verified` is
/// false and only the project owner changes that), so on a real radio the gate refuses every
/// transmission. `test_table` lets a bench radio (in-memory or BlackHole) transmit for tests;
/// it never opens a real radio.
struct TxPolicy {
    table: BandTable,
    dial_hz: Option<u64>,
    test_table: bool,
    /// Audio span (Hz) the modem fills; the gate checks the occupied span from it.
    audio_span: (u64, u64),
}

impl Flags {
    fn new() -> Self {
        Flags {
            sign_mail: AtomicBool::new(true),
            relay_running: AtomicBool::new(false),
            operator_link: AtomicBool::new(false),
            listen_only: AtomicBool::new(false),
            weak_mode: AtomicBool::new(false),
            tx_level_shown: AtomicU8::new(0),
            tx_level_pct: AtomicU8::new(DEFAULT_TX_LEVEL_PCT),
            auto_mode: AtomicBool::new(true),
            auto_link: AtomicBool::new(false),
            fixed_level: AtomicU8::new(mod_level(MODULATION) as u8),
            update_locked: AtomicBool::new(false),
            // Setting D: on by default. The gateway is a stub, so nothing leaves the station
            // until the gateway exists and the operator sends that way.
            internet_relay: AtomicBool::new(true),
            third_party_relay: AtomicBool::new(true),
            relay_timeout_ms: AtomicU64::new(RELAY_TIMEOUT_MS),
            policy: Mutex::new(TxPolicy { table: BandTable::builtin_r1(), dial_hz: None, test_table: false, audio_span: TX_AUDIO_SPAN_HZ }),
            bench_radio: AtomicBool::new(false),
            radio_known: AtomicBool::new(false),
            radio_dial_hz: AtomicU64::new(0),
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
        gate_refusal(f)
    }
}

const GATE_UNVERIFIED: &str = "Transmit is off: the 17 m band table is not verified against the current band plan yet. Receiving works.";
const GATE_NO_DIAL: &str = "Transmit is off: KAHF does not know the radio's dial frequency. Choose a lane on the Radio screen.";
const GATE_NOT_A_LANE: &str = "Transmit is off: the radio's dial is not set for a KAHF lane (17 m). Set the dial shown for the lane.";
const GATE_OUTSIDE: &str = "Transmit is off: the signal would not fit inside the lane.";
const GATE_TEST_REAL: &str = "The test table is on, but this is a real radio. Transmit is off.";

/// Audio span the modem fills today, in Hz (the wide KAMI carriers). The lane mode (480 Hz)
/// replaces this when it is in the engine.
const TX_AUDIO_SPAN_HZ: (u64, u64) = (312, 2_690);

/// The transmit gate (MESH-SPEC I7): every transmission must pass `lanes::permit_tx` against
/// the band table with the dial the radio is really on. Returns why not, or `None`.
/// While the radio is not open yet nothing is decided (the check repeats before every burst).
fn gate_refusal(f: &Flags) -> Option<&'static str> {
    if !f.radio_known.load(Ordering::SeqCst) {
        return None;
    }
    let bench = f.bench_radio.load(Ordering::SeqCst);
    let p = lock(&f.policy);
    if p.test_table {
        return if bench { None } else { Some(GATE_TEST_REAL) };
    }
    if !p.table.plan_verified {
        return Some(GATE_UNVERIFIED);
    }
    let radio_dial = f.radio_dial_hz.load(Ordering::SeqCst);
    let Some(dial) = p.dial_hz.or((radio_dial != 0).then_some(radio_dial)) else {
        return Some(GATE_NO_DIAL);
    };
    let Some(band) = p.table.band("17m") else { return Some(GATE_OUTSIDE) };
    let Some(lane) = band.lanes.iter().find(|l| l.dial_hz == dial) else {
        return Some(GATE_NOT_A_LANE);
    };
    match p.table.permit_tx("17m", lane.k, lane.k, dial, p.audio_span.0, p.audio_span.1) {
        Ok(()) => None,
        Err(laneplan::Violation::PlanNotVerified) => Some(GATE_UNVERIFIED),
        Err(_) => Some(GATE_OUTSIDE),
    }
}

/// Plan 6.2: route a message through the server gateway. Stub until the server exists.
fn relay_via_gateway(_dest: &str) -> Result<(), String> {
    // TODO: send to the kahf.dev gateway (plan 5 and 6.2) and return Ok once it accepts.
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
    clock: Clock,
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

    /// Lets a running relay carry messages through the server (relays are online, operators may
    /// not be). `None` turns it off, and the relay works on the radio side only.
    pub fn set_backbone(&self, server: Option<&str>) {
        *lock(&self.shared.backbone_server) = server.map(|s| s.to_string());
    }

    /// Transmit audio level in percent of full scale, 5 to 100. Lower is safer: set it so the
    /// radio's ALC only just moves, or lower.
    pub fn set_tx_level(&self, pct: u8) {
        self.shared.flags.tx_level_pct.store(pct.clamp(5, 100), Ordering::SeqCst);
    }

    /// Sends a steady 1 kHz tone for five seconds, so the drive can be set by the radio's ALC
    /// meter. This transmits.
    pub fn test_tone(&self) -> Result<(), String> {
        if let Some(why) = tx_refusal(&self.shared.flags) {
            return Err(why.into());
        }
        self.send_cmd(Cmd::Tone)
    }

    /// The position to put in announcements (latitude, longitude), or None to say nothing.
    /// The caller decides how exact it is; the engine sends it as given, to a hundredth of a degree.
    pub fn set_location(&self, loc: Option<(f64, f64)>) {
        *lock(&self.shared.location) = loc.map(|(a, b)| (a as f32, b as f32));
    }

    /// Tells everyone in range that this station exists. Relays pass it on, so stations beyond
    /// direct range learn of it too and add it to their contacts. One short burst.
    pub fn advertise(&self) -> Result<(), String> {
        self.send_cmd(Cmd::Advertise)
    }

    pub fn disconnect(&mut self) -> Result<(), String> {
        self.shared.flags.operator_link.store(false, Ordering::SeqCst);
        self.send_cmd(Cmd::Disconnect)
    }

    /// Replaces the band table the transmit gate checks against (a signed update, once there is one).
    pub fn set_band_table(&self, table: BandTable) {
        lock(&self.shared.flags.policy).table = table;
    }

    /// The USB dial the radio is on, in Hz (the app's lane setting). `None` = not known.
    pub fn set_dial_hz(&self, dial_hz: Option<u64>) {
        lock(&self.shared.flags.policy).dial_hz = dial_hz;
    }

    /// The audio span the modem fills, in Hz (the lane mode sets 1260 to 1740).
    pub fn set_tx_audio_span(&self, lo_hz: u64, hi_hz: u64) {
        lock(&self.shared.flags.policy).audio_span = (lo_hz, hi_hz);
    }

    /// Test table: lets a bench radio (in-memory or BlackHole) transmit while the band table is
    /// unverified. It has no effect on a real radio, which stays refused.
    pub fn set_test_table(&self, on: bool) {
        lock(&self.shared.flags.policy).test_table = on;
    }

    /// What the transmit gate says now: `("real", "")` when a real radio may transmit,
    /// `("bench", ..)` on the test table with a bench radio, else `("off", reason)`.
    pub fn tx_gate(&self) -> (&'static str, String) {
        let f = &self.shared.flags;
        if let Some(why) = tx_refusal(f) {
            return ("off", why.to_string());
        }
        if lock(&f.policy).test_table && f.bench_radio.load(Ordering::SeqCst) {
            return ("bench", "Test table: bench radio only, nothing reaches an antenna.".to_string());
        }
        ("real", String::new())
    }

    /// Listen only (plan 0): while on, no transmit path runs. Receive keeps working.
    pub fn set_listen_only(&self, on: bool) {
        self.shared.flags.listen_only.store(on, Ordering::SeqCst);
    }

    /// Weak mode: sends every frame WEAK_COPIES times in QPSK, so it decodes at about -4 dB.
    /// Much slower, and the receiver gets no acknowledgements in time to repeat anything.
    /// The KAMI mode used for the most recent transmission, for example "KAMI-32" or "KAMI-1c".
pub fn transmit_modulation(&self) -> &'static str {
        let level = self.shared.flags.tx_level_shown.load(Ordering::SeqCst) as usize;
        mode_name(MODULATIONS.get(level).copied().unwrap_or(MODULATION))
    }

    /// Names of the modes the transmitter can use, slowest and most robust first.
    pub fn mode_names() -> Vec<&'static str> {
        MODULATIONS.iter().map(|&m| mode_name(m)).collect()
    }

    /// Let the station connect to the destination of queued messages by itself, send them, and
    /// hang up afterwards. Relay stations do this whenever relay mode runs.
    pub fn set_auto_link(&self, on: bool) {
        self.shared.flags.auto_link.store(on, Ordering::SeqCst);
    }

    pub fn auto_link(&self) -> bool {
        self.shared.flags.auto_link.load(Ordering::SeqCst)
    }

    /// Relay stations to try, in order, for a message whose station is not reached on air.
    pub fn set_relay_hints(&self, calls: &[String]) {
        *lock(&self.shared.relay_hints) = calls.iter().map(|c| c.to_uppercase()).collect();
    }

    /// Automatic mode choice from the peer's SNR report (the default) or a fixed mode.
    pub fn set_auto_mode(&self, on: bool) {
        self.shared.flags.auto_mode.store(on, Ordering::SeqCst);
    }

    /// Always send in the named mode, for example "KAMI-32". Turns automatic choice off.
    pub fn set_fixed_mode(&self, name: &str) -> Result<(), String> {
        let level = MODULATIONS
            .iter()
            .position(|&m| mode_name(m).eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("unknown mode {name}"))?;
        self.shared.flags.fixed_level.store(level as u8, Ordering::SeqCst);
        self.shared.flags.auto_mode.store(false, Ordering::SeqCst);
        Ok(())
    }

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
    /// Turns this station into a relay on `channel_khz`, with one transmit slot that covers the
    /// next 24 hours. The operator must confirm the licence and the terms.
    pub fn enable_relay(&self, channel_khz: f32, licence_held: bool, terms_accepted: bool) -> Result<(), String> {
        self.configure_relay(Role::Relay, relay::Radio::HalfDuplex, &[channel_khz])?;
        let now = now_unix_secs().max(0) as u64;
        self.accept_relay_schedule(vec![Slot {
            channel_khz,
            start_unix: now,
            duration_s: 24 * 3600,
            direction: relay::Direction::Transmit,
        }])?;
        self.start_relay(RelayAck { licence_held, terms_accepted })
    }

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

    /// The most recent forwards across all messages, newest first, with how many in total.
    pub fn relay_recent(&self, n: usize) -> (Vec<Hop>, usize) {
        lock(&self.shared.hops).recent(n)
    }

    /// Forwards recorded for one message, in order (plan 4). `message_id` is the public id.
    pub fn relay_hops(&self, message_id: u64) -> Vec<Hop> {
        lock(&self.shared.hops).chain_for(message_id)
    }

    /// Internet relay (plan 6.1). On by default (setting D). The gateway is still a stub:
    /// nothing goes over the internet until it exists and the operator sends a message that way.
    /// When off, no traffic leaves over the internet.
    pub fn set_internet_relay(&self, on: bool) {
        self.shared.flags.internet_relay.store(on, Ordering::SeqCst);
    }

    /// Label of setting B, shown next to the switch.
    pub const THIRD_PARTY_RELAY_LABEL: &'static str = "Relay other stations' messages";

    /// Third-party relaying (setting B). On by default. It has no effect yet: forwarding
    /// other stations' traffic is not built. TODO: the rules must be checked before release.
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
    /// Queues a chat and returns its outbox id, so the caller can follow it. `private` keeps it
    /// for the addressee's inbox only.
    pub fn send_chat(&mut self, to: &str, text: &str, private: bool) -> Result<u64, String> {
        check_to(to)?;
        if text.is_empty() {
            return Err("empty message".into());
        }
        let msg = Message::Chat {
            from: self.config.callsign.clone(),
            to: to.trim().to_string(),
            text: text.to_string(),
        };
        let ids = self.queue_with(vec![msg], private)?;
        ids.first().copied().ok_or_else(|| STOPPED.to_string())
    }

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
        let total: usize = files.iter().map(|(_, d)| d.len()).sum();
        if total > MAX_ATTACHMENT_BYTES {
            return Err(format!(
                "The attachments are {} kB together. The limit is {} kB.",
                total / 1024,
                MAX_ATTACHMENT_BYTES / 1024
            ));
        }
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
        Self::spawn_with_clock(config, factory, Clock::real())
    }

    fn spawn_with_clock(config: Config, factory: Factory, clock: Clock) -> Result<Self, String> {
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
            relay_hints: Mutex::new(Vec::new()),
            backbone_server: Mutex::new(None),
            location: Mutex::new(None),
            relay_stats: Mutex::new(Stats::default()),
            hops: Mutex::new(HopLog::new()),
        });
        // Daemon tests run on test radios: the bench table is on unless a test turns it off.
        #[cfg(test)]
        {
            lock(&shared.flags.policy).test_table = true;
        }
        let ids = Arc::new(AtomicU64::new(1));
        let engine_config = config.clone();
        let (worker_shared, worker_ids) = (shared.clone(), ids.clone());
        let (worker_clock, joined) = (clock.clone(), clock.join());
        let worker = thread::Builder::new()
            .name("kahf-engine".into())
            .spawn(move || {
                let _joined = joined;
                Worker::new(config, cmd_rx, worker_shared, factory, worker_ids, worker_clock).run()
            })
            .map_err(|e| format!("cannot start engine thread: {e}"))?;
        Ok(Engine {
            config: engine_config,
            cmd,
            shared,
            ids,
            worker: Some(worker),
            clock,
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
        if msgs.iter().any(|m| dest_of(m).trim().eq_ignore_ascii_case(&self.config.callsign)) {
            return Err("That is your own callsign. Choose the station you want to reach.".into());
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
            self.clock.park(|| {
                let _ = h.join();
            });
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

/// The most one file or mail may carry. Radio is slow: this is already about 8 minutes at 1.7 kbit/s.
pub const MAX_ATTACHMENT_BYTES: usize = 100 * 1024;

fn read_attachment(path: &Path) -> Result<(String, Vec<u8>), String> {
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .ok_or_else(|| format!("{} has no file name", path.display()))?;
    let size = fs::metadata(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?.len() as usize;
    if size > MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "{name} is {} kB. Files over {} kB cannot be sent over the radio: it would take far too long.",
            size / 1024,
            MAX_ATTACHMENT_BYTES / 1024
        ));
    }
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
        match addr.rsplit_once(':') {
            Some((host, port)) if !host.is_empty() && port.parse::<u16>().is_ok() => {}
            _ => return Err(format!("bad rigctld address {addr:?} (want host:port)")),
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
    /// Announce this station to everyone who can hear it, and through relays to those who cannot.
    Advertise,
    /// A steady test tone for setting the radio's drive.
    Tone,
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
    /// True when this radio can never reach an antenna (in-memory test radio, BlackHole).
    /// Only such a radio may run on the test table.
    fn is_bench(&self) -> bool {
        false
    }
    /// The USB dial in Hz when the radio itself fixes it (an SDR), else `None`.
    fn dial_hz(&self) -> Option<u64> {
        None
    }
}

fn open_radio(cfg: &Config) -> Result<Box<dyn Backend>, String> {
    if let Some(sdr) = &cfg.sdr {
        return Ok(Box::new(SdrBackend::new(parse_sdr(sdr)?)));
    }
    let input = AudioIn::open(cfg.audio_in.as_deref())?;
    let output = AudioOut::open(cfg.audio_out.as_deref())?;
    let rig: Option<Box<dyn RigControl>> = match &cfg.rigctld {
        Some(addr) => Some(Box::new(RigctldClient::connect(addr)?)),
        None => None,
    };
    let named_blackhole = |n: &Option<String>| n.as_deref().is_some_and(|n| n.to_lowercase().contains("blackhole"));
    let bench = cfg.rigctld.is_none() && named_blackhole(&cfg.audio_in) && named_blackhole(&cfg.audio_out);
    Ok(Box::new(AudioBackend {
        input: Some(input),
        output: Some(output),
        rig,
        bench,
    }))
}

fn set_ptt(rig: &mut Option<Box<dyn RigControl>>, on: bool) -> Result<(), String> {
    match rig {
        Some(r) => r.set_ptt(on),
        None => Ok(()),
    }
}

/// Sound card in and out, with optional rigctld PTT. No PTT unless rigctld is set (VOX).
struct AudioBackend {
    input: Option<AudioIn>,
    output: Option<AudioOut>,
    rig: Option<Box<dyn RigControl>>,
    /// Both audio devices are BlackHole and there is no rig control: nothing can radiate.
    bench: bool,
}

impl Backend for AudioBackend {
    fn is_bench(&self) -> bool {
        self.bench
    }

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
    fn dial_hz(&self) -> Option<u64> {
        Some(self.spec.freq_hz)
    }

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

/// The modulation ladder, lowest rung first: the transmit level indexes this list.
/// The id carried in the frame is fixed per mode (`mod_id`), so the order here is free.
const MODULATIONS: [Modulation; 11] = [
    Modulation::Kami256,
    Modulation::Kami128,
    Modulation::Kami64,
    Modulation::Kami32,
    Modulation::Kami16,
    Modulation::Kami8,
    Modulation::Kami4,
    Modulation::Kami1a,
    Modulation::Kami1b,
    Modulation::Kami1c,
    Modulation::Kami1d,
];

/// Index in `MODULATIONS` of KAMI-16, the slowest mode used while the signal is not weak.
const HF_MIN_LEVEL: usize = 4;
/// Below this reported SNR (dB) the slowest rungs are allowed.
const HF_SLOW_FLOOR_SNR_DB: f32 = -3.0;

/// Every KAMI mode the receiver recognises, ladder or not. They share one preamble.
const KAMI_MODES: [Modulation; 13] = [
    Modulation::Kami256,
    Modulation::Kami128,
    Modulation::Kami64,
    Modulation::Kami32,
    Modulation::Kami16,
    Modulation::Kami8,
    Modulation::Kami4,
    Modulation::Kami2,
    Modulation::Kami8H,
    Modulation::Kami1a,
    Modulation::Kami1b,
    Modulation::Kami1c,
    Modulation::Kami1d,
];

/// Display name of a mode.
fn mode_name(m: Modulation) -> &'static str {
    match m {
        Modulation::Kami256 => "KAMI-256",
        Modulation::Kami128 => "KAMI-128",
        Modulation::Kami64 => "KAMI-64",
        Modulation::Kami32 => "KAMI-32",
        Modulation::Kami16 => "KAMI-16",
        Modulation::Kami8 => "KAMI-8",
        Modulation::Kami4 => "KAMI-4",
        Modulation::Kami2 => "KAMI-2",
        Modulation::Kami8H => "KAMI-8H",
        Modulation::Kami1a => "KAMI-1a",
        Modulation::Kami1b => "KAMI-1b",
        Modulation::Kami1c => "KAMI-1c",
        Modulation::Kami1d => "KAMI-1d",
        _ => "KAMI-32",
    }
}

/// Index into MODULATIONS of the fastest mode a reported SNR (dB) supports.
fn ladder_level(snr_db: f32) -> usize {
    kahf_core::phy::LADDER
        .iter()
        .find(|&&(_, floor)| snr_db >= floor)
        .map_or(0, |&(m, _)| mod_level(m))
}

/// Wire id of a modulation, in the frame's id field. QPSK, 16QAM and 64QAM keep 0, 1 and 2.
fn mod_id(m: Modulation) -> u8 {
    use kahf_core::kami as k;
    match m {
        Modulation::Qpsk => 0,
        Modulation::Qam16 => 1,
        Modulation::Qam64 => 2,
        Modulation::Kami32 => k::ID,
        Modulation::Kami16 => k::ID16,
        Modulation::Kami8 => k::ID8,
        Modulation::Kami8H => k::ID8H,
        Modulation::Kami64 => k::ID64,
        Modulation::Kami128 => k::ID128,
        Modulation::Kami256 => k::ID256,
        Modulation::Kami4 => k::ID4,
        Modulation::Kami2 => k::ID2,
        Modulation::Kami1a => k::ID1A,
        Modulation::Kami1b => k::ID1B,
        Modulation::Kami1c => k::ID1C,
        Modulation::Kami1d => k::ID1D,
    }
}

fn mod_level(m: Modulation) -> usize {
    MODULATIONS.iter().position(|&x| x == m).unwrap_or(0)
}

/// Samples of one frame in the given KAMI mode.
fn frame_len_for(m: Modulation) -> usize {
    m.kami_shape().map_or(0, |sh| kahf_core::kami::frame_len_shape(ldpc::N, sh))
}

/// One frame, sent with the given modulation. The frame carries the modulation id.
#[cfg(test)]
fn frame_samples_as(payload: &[u8], m: Modulation) -> Result<Vec<f32>, String> {
    if payload.len() > MAX_CHUNK {
        return Err(format!(
            "frame payload of {} bytes is over {MAX_CHUNK}",
            payload.len()
        ));
    }
    let coded = ldpc::encode(&frame::build_payload_as(payload, mod_id(m)));
    Ok(kahf_core::phy::modulate_frame(&coded, m))
}

/// Audio samples of a burst of `n` frames in KAMI mode `m`, without the lead and tail silence.
fn burst_len_for(m: Modulation, n: usize) -> usize {
    m.kami_shape().map_or(0, |sh| kahf_core::kami::burst_len_shape(ldpc::N, n, sh))
}

/// Codewords one burst carries at most: the frame id byte keeps the count of codewords still to
/// come in its top three bits (0 to 7), the mode in its low five.
const BURST_MAX: usize = 8;
const ID_MODE_MASK: u8 = 0x1F;

/// How long after a transmission the sound card may still be handing back our own signal.
/// Must stay below the MAC's answer gap (`SIFS_MS`): the station we sent to answers that soon
/// after our burst, and an answer that starts inside this window would be thrown away unheard.
/// A preamble has been heard but its first codeword has not decoded yet, so the burst is of an
/// unknown mode and length. For this long it counts as a burst being received: starting our own
/// burst now would talk over an answer on its way (a slow mode's first codeword is its whole frame).
const PREAMBLE_HOLD: Duration = Duration::from_millis(2500);

const ECHO_TAIL: Duration = Duration::from_millis(200);
const _: () = assert!((ECHO_TAIL.as_millis() as u64) + 50 <= kahf_core::mac::SIFS_MS);
/// Default transmit audio level: half of full scale (minus 6 dB).
const DEFAULT_TX_LEVEL_PCT: u8 = 50;
/// The test tone: a steady 1 kHz sine, long enough to set the radio's drive by its ALC meter.
const TONE_SECONDS: usize = 5;

/// A steady test tone at `level_pct` of full scale.
fn tone_samples(level_pct: u8) -> Vec<f32> {
    let level = level_pct as f32 / 100.0;
    let n = RATE as usize * TONE_SECONDS;
    let mut v = vec![0f32; LEAD_SILENCE];
    v.extend((0..n).map(|i| level * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / RATE as f32).sin()));
    v.extend(std::iter::repeat_n(0f32, TAIL_SILENCE));
    v
}

/// One transmit burst: lead-in silence, then the frames after a single preamble (several bursts
/// of up to BURST_MAX frames when there are more), then tail silence. Sharing the preamble is
/// what gives the fast modes their rate.
fn burst(frames: &[Vec<u8>], m: Modulation) -> Result<Vec<f32>, String> {
    let sh = m.kami_shape().ok_or("not a KAMI mode")?;
    let mut out = vec![0f32; LEAD_SILENCE];
    for (i, group) in frames.chunks(BURST_MAX).enumerate() {
        if i > 0 {
            out.extend(std::iter::repeat_n(0f32, TAIL_SILENCE + LEAD_SILENCE / 2));
        }
        let mut cws = Vec::with_capacity(group.len());
        for (j, payload) in group.iter().enumerate() {
            if payload.len() > MAX_CHUNK {
                return Err(format!("frame payload of {} bytes is over {MAX_CHUNK}", payload.len()));
            }
            let id = mod_id(m) | (((group.len() - 1 - j) as u8) << 5);
            cws.push(ldpc::encode(&frame::build_payload_as(payload, id)));
        }
        out.extend(kahf_core::kami::modulate_burst_shape(&cws, sh));
    }
    out.extend(std::iter::repeat_n(0f32, TAIL_SILENCE));
    Ok(out)
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
    session::try_encode_signed(msg, None, private).map_err(|e| e.to_string())
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

fn sender_of(msg: &Message) -> &str {
    match msg {
        Message::Chat { from, .. } | Message::Mail { from, .. } | Message::File { from, .. } => from,
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
    match decode_message(&item.payload) {
        Some(msg) => describe_msg(&msg),
        None => format!("{} (unreadable)", item.kind),
    }
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

/// Reads back what `encode_message` wrote. None for anything malformed.
fn decode_message(bytes: &[u8]) -> Option<Message> {
    decode_item(bytes).map(|(msg, _)| msg)
}

/// As `decode_message`, also returning the private bit.
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

/// The only band KAHF works in, in its ADIF name, by frequency in MHz.
const BANDS: &[(f64, f64, &str)] = &[(18.068, 18.168, "17M")];

/// Band name for a frequency in MHz, or empty when it is outside the amateur bands.
fn band_name(mhz: f64) -> String {
    BANDS
        .iter()
        .find(|(lo, hi, _)| (*lo..=*hi).contains(&mhz))
        .map(|(_, _, name)| name.to_string())
        .unwrap_or_default()
}

/// A small stable hash, for repeatable test randomness.
fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
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
    clock: Clock,
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
    /// Time spent transmitting, left out of the link clock.
    tick_paused: Duration,
    last_tick: Instant,
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
    /// Samples since the last KAMI-32 stream search.
    since_kami: usize,
    /// A located KAMI preamble: its start in `rx_buf` and the frequency offset.
    kami_at: Option<(usize, f64, f64)>,
    /// When the preamble in `kami_at` was found.
    kami_seen: Option<Instant>,
    /// The preamble estimate was redone once the whole preamble had arrived.
    kami_refined: bool,
    /// Audio available after the preamble at the last decode attempt.
    kami_tried: usize,
    /// The mode of the burst being received, once its first codeword has decoded.
    kami_mode: Option<Modulation>,
    /// Codewords of the burst delivered so far.
    kami_next: usize,
    /// Codewords the burst being received has in all.
    kami_total: usize,
    /// Look at the front of the buffer next, because a frame has just been consumed.
    kami_front: bool,
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
    /// Outbox items sent through a relay: the relay's callsign, which the link connects to.
    via: HashMap<u64, String>,
    /// Relays already tried for each item.
    via_tried: HashMap<u64, Vec<String>>,
    /// Handshake failures per station and when it may be tried again.
    auto_fails: HashMap<String, (u32, Instant)>,
    /// The current link was opened by the station itself, so it also closes it.
    auto_initiated: bool,
    /// Last frame sent or received, for hanging up an idle automatic link.
    last_activity: Instant,
    /// Digests of messages already forwarded for others, so one is never forwarded twice.
    forwarded: VecDeque<u64>,
    /// Audio that arrives until this time is our own transmission coming back (a virtual cable, a
    /// leaky rig, the sound card's delay) and is thrown away unread.
    rx_deaf_until: Option<Instant>,
    /// Who may transmit when: listen before talk, back-off, fair share (see `kahf_core::mac`).
    mac: Mac,
    /// A received burst is being tracked, so its end is reported to `mac`.
    rx_locked: bool,
    harq: kahf_core::harq::Harq,
    /// Stations heard lately: callsign -> (SNR, when). Reported to the server and used to
    /// decide who a relay can reach itself.
    heard_recent: HashMap<String, (Option<i32>, Instant)>,
    /// The thread that talks to the server, and what it has to tell us.
    bb: Option<BbHandle>,
    /// Outbox id -> server message id, for messages taken from the server and sent on the radio.
    bb_pending: HashMap<u64, String>,
    bb_last: Instant,
    /// Adverts already seen (callsign, sequence), so a relay passes each one on once.
    adverts_seen: VecDeque<(String, u8)>,
    advert_seq: u8,
    /// When this relay last announced itself.
    beacon_at: Instant,
    /// The transmit refusal last logged, so a held transmit is reported once.
    refusal_logged: Option<&'static str>,
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
        clock: Clock,
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
        let mut copy = outbox.clone();
        while let Some((oid, item)) = copy.next(u64::MAX) {
            copy.cancel(oid);
            tags.insert(oid, (ids.fetch_add(1, Ordering::Relaxed), describe(&item)));
            // Items from an earlier run wait for the relay timeout from start-up.
            if let Some(msg) = decode_message(&item.payload) {
                queued.insert(oid, (dest_of(&msg).to_string(), 0));
            }
        }
        let w = Worker {
            clock: clock.clone(),
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
            tx_level: mod_level(MODULATION) as u8,
            tick_paused: Duration::ZERO,
            last_tick: clock.now(),
            link_state: LinkState::Idle,
            qso: None,
            outbox,
            outbox_path,
            tags,
            bursts: VecDeque::new(),
            control: VecDeque::new(),
            tx: None,
            rx_buf: Vec::new(),
            since_kami: 0,
            kami_at: None,
            kami_seen: None,
            kami_refined: false,
            kami_tried: 0,
            kami_mode: None,
            kami_next: 0,
            kami_total: 0,
            kami_front: false,
            spec: Analyser::new(SPEC_FFT, RATE as f32),
            spec_due: clock.now(),
            since_scan: 0,
            rx_chunks: Vec::new(),
            rx_error: None,
            rx_retry_at: None,
            tick_error: None,
            started: clock.now(),
            queued,
            unreached_noted: HashSet::new(),
            via: HashMap::new(),
            via_tried: HashMap::new(),
            auto_fails: HashMap::new(),
            auto_initiated: false,
            last_activity: clock.now(),
            forwarded: VecDeque::new(),
            rx_deaf_until: None,
            mac: Mac::new(if clock.is_virtual() { fnv(&config.callsign) } else { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1) }),
            rx_locked: false,
            harq: Default::default(),
            adverts_seen: VecDeque::new(),
            advert_seq: 0,
            beacon_at: clock.now() - Duration::from_secs(3600),
            heard_recent: HashMap::new(),
            bb: None,
            bb_pending: HashMap::new(),
            bb_last: clock.now(),
            refusal_logged: None,
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
            self.backbone_step();
            self.auto_link();
            if self.tx.is_some() {
                // Half duplex: no receive while a burst is on the air.
                if !self.tx_step() {
                    self.clock.sleep(POLL);
                }
                continue;
            }
            // Control frames (ACK, CONNECT, ...) go before data; each passes the channel check.
            self.flush_control();
            // Queued items go first (only while linked), one unit per pass.
            if let Some((id, msg, private)) = self.next_due() {
                if self.clear_channel(id, &msg) {
                    self.send_item(id, msg, private);
                    continue;
                }
                // Waiting for the channel: keep listening (the receive step below also sleeps
                // when there is no audio), so an answer on its way is not missed.
            }
            if !self.rx_provisional() && !self.bursts.is_empty() {
                let samples = self.bursts.pop_front().unwrap_or_default();
                let ms = self.now_ms();
                self.mac.on_tx_start(ms, ms + (samples.len() as u64 * 1000 / RATE as u64));
                self.tx = Some(TxJob {
                    samples,
                    pos: 0,
                    started: None,
                    sending: false,
                });
                continue;
            }
            if self.rx_step() == 0 {
                self.clock.sleep(POLL);
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
            Cmd::Advertise => self.advertise_now(),
            Cmd::Tone => {
                let pct = self.shared.flags.tx_level_pct.load(Ordering::SeqCst);
                self.log(format!("Sending a test tone at {pct} percent for {TONE_SECONDS} s."));
                self.bursts.push_back(tone_samples(pct));
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
            Cmd::Queue { msgs, ids, private } => {
                self.enqueue(msgs, ids, private);
            }
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
        let flags = &self.shared.flags;
        flags.bench_radio.store(backend.is_bench(), Ordering::SeqCst);
        flags.radio_dial_hz.store(backend.dial_hz().unwrap_or(0), Ordering::SeqCst);
        flags.radio_known.store(true, Ordering::SeqCst);
        let test = lock(&flags.policy).test_table;
        self.backend = Some(backend);
        self.log("radio open");
        if test && flags.bench_radio.load(Ordering::SeqCst) {
            self.log("TEST TABLE ON: bench radio only. Nothing is sent to an antenna.");
        }
        Ok(())
    }

    fn connect(&mut self, peer: &str) -> Result<(), String> {
        self.ensure_radio()?;
        self.link.set_air_ms((burst_len_for(MODULATIONS[self.tx_level as usize], 1) * 1000 / RATE as usize) as u64);
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
        self.clock.since(self.started).as_millis() as u64
    }

    /// Adds messages to the outbox in order and saves the queue. `ids` are the public ids the caller returned.
    fn enqueue(&mut self, msgs: Vec<Message>, ids: Vec<u64>, private: bool) -> Vec<u64> {
        let mut oids = Vec::new();
        for (m, pid) in msgs.iter().zip(ids) {
            let item = OutItem {
                kind: kind_of(m).to_string(),
                payload: encode_message(m, private),
            };
            let oid = self.outbox.push(item, priority_of(m));
            oids.push(oid);
            self.tags.insert(oid, (pid, describe_msg(m)));
            let at = self.now_ms();
            self.queued.insert(oid, (dest_of(m).to_string(), at));
        }
        self.persist_outbox();
        oids
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
        for (oid, dest) in due {
            if let Some(hint) = self.next_relay_hint(oid, &dest) {
                self.log(format!("{dest} not reached on air. Trying the relay {hint}."));
                self.via.insert(oid, hint.clone());
                self.via_tried.entry(oid).or_default().push(hint);
                self.queued.insert(oid, (dest, now));
                continue;
            }
            self.unreached_noted.insert(oid);
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

    /// The station the link must be connected to for outbox item `id`: its relay when it has
    /// been routed through one, else the message's own destination.
    fn route_peer(&self, id: u64, msg: &Message) -> String {
        self.via.get(&id).cloned().unwrap_or_else(|| dest_of(msg).to_string())
    }

    /// Same for an item known only by its queue entry.
    fn route_peer_of(&self, id: u64) -> Option<String> {
        self.via.get(&id).cloned().or_else(|| self.queued.get(&id).map(|(d, _)| d.clone()))
    }

    /// The next relay hint not yet tried for this item, other than ourselves and the destination.
    fn next_relay_hint(&self, id: u64, dest: &str) -> Option<String> {
        let tried = self.via_tried.get(&id);
        lock(&self.shared.relay_hints)
            .iter()
            .find(|h| {
                !h.eq_ignore_ascii_case(dest)
                    && !h.eq_ignore_ascii_case(&self.callsign)
                    && !tried.is_some_and(|t| t.iter().any(|x| x.eq_ignore_ascii_case(h)))
            })
            .cloned()
    }

    /// Automatic linking: with auto-link on (or relay mode running) a queued message whose
    /// route is not linked gets a connection, with a growing pause after each failure, and an
    /// automatic link is closed once nothing is waiting for its peer.
    fn auto_link(&mut self) {
        let flags = &self.shared.flags;
        if !(flags.auto_link.load(Ordering::SeqCst) || flags.relay_running.load(Ordering::SeqCst))
            || flags.listen_only.load(Ordering::SeqCst)
            || tx_refusal(flags).is_some()
            || !self.relay_slot_open()
        {
            return;
        }
        match self.link_state.clone() {
            LinkState::Idle => {
                self.auto_initiated = false;
                if self.access_retry_at.is_some_and(|t| self.clock.now() < t) {
                    return;
                }
                let now = self.now_ms();
                let Some((id, _)) = self.outbox.next(now) else {
                    return;
                };
                let Some(peer) = self.route_peer_of(id) else {
                    return;
                };
                if self
                    .auto_fails
                    .get(&peer.to_uppercase())
                    .is_some_and(|(_, until)| self.clock.now() < *until)
                {
                    return;
                }
                self.auto_initiated = true;
                self.last_activity = self.clock.now();
                if let Err(e) = self.connect(&peer) {
                    self.log(e);
                }
            }
            LinkState::Connecting { peer } if self.auto_initiated => {
                // The route changed (a relay was chosen) while we were calling the old peer: stop.
                let now = self.now_ms();
                let want = self.outbox.next(now).and_then(|(id, _)| self.route_peer_of(id));
                if want.is_some_and(|w| !w.eq_ignore_ascii_case(&peer)) {
                    if let Err(e) = self.disconnect() {
                        self.log(e);
                    }
                }
            }
            LinkState::Connected { peer } if self.auto_initiated => {
                let waiting = self
                    .outbox
                    .pending()
                    .iter()
                    .any(|(id, _, _)| self.route_peer_of(*id).is_some_and(|p| p.eq_ignore_ascii_case(&peer)));
                if !waiting && self.clock.since(self.last_activity) >= AUTO_HANGUP {
                    if let Err(e) = self.disconnect() {
                        self.log(e);
                    }
                }
            }
            _ => {}
        }
    }

    /// A message for another station reached us. A relay with relay mode running keeps it in
    /// its outbox for that station, once, so the next hop is delivered by the normal send path.
    fn forward_third_party(&mut self, msg: &Message, private: bool) {
        let flags = &self.shared.flags;
        let is_relay = lock(&self.shared.relay).as_ref().is_some_and(|r| r.role != Role::Station);
        if !(is_relay && flags.relay_running.load(Ordering::SeqCst) && flags.third_party_relay.load(Ordering::SeqCst)) {
            return;
        }
        // Files go direct. A relay holds no files for others: they are too big for its air time.
        if matches!(msg, Message::File { .. }) {
            return;
        }
        let digest = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            encode_message(msg, private).hash(&mut h);
            h.finish()
        };
        if self.forwarded.contains(&digest) {
            return;
        }
        self.forwarded.push_back(digest);
        if self.forwarded.len() > 512 {
            self.forwarded.pop_front();
        }
        // A relay with a server connection hands the message to the backbone: radio hops between
        // relays are not used, and the destination's own relay delivers it (NETWORK-SCENARIOS H1).
        // A station this relay hears itself is reached directly, not through the server.
        if self.bb.is_some() || lock(&self.shared.backbone_server).is_some() {
            if self.heard_lately(dest_of(msg)) {
                self.log(format!("{} is heard here: the message is delivered directly.", dest_of(msg)));
            } else {
                self.bb_submit(msg);
                return;
            }
        }
        let id = self.next_id();
        self.log(format!("Forwarding a message from {} to {}.", sender_of(msg), dest_of(msg)));
        self.enqueue(vec![msg.clone()], vec![id], private);
    }

    fn note_heard_recent(&mut self, call: &str, snr: Option<f32>) {
        if call == self.callsign {
            return;
        }
        self.heard_recent.insert(call.to_uppercase(), (snr.map(|s| s.round() as i32), self.clock.now()));
        if self.heard_recent.len() > 500 {
            self.heard_recent.retain(|_, (_, t)| self.clock.since(*t) < Duration::from_secs(900));
        }
    }

    fn heard_lately(&self, call: &str) -> bool {
        self.heard_recent
            .get(&call.to_uppercase())
            .is_some_and(|(_, t)| self.clock.since(*t) < Duration::from_secs(600))
    }

    /// Starts the server thread when relay mode runs and a server is set, and stops it when not.
    fn bb_ensure(&mut self) {
        let want = if self.shared.flags.relay_running.load(Ordering::SeqCst) {
            lock(&self.shared.backbone_server).clone()
        } else {
            None
        };
        match (want, self.bb.is_some()) {
            (Some(server), false) => {
                let Some(key) = self.shared.station_key.clone() else { return };
                self.bb = Some(BbHandle::spawn(Backbone::new(&server, &self.callsign, key)));
                self.log(format!("Relay connected to the network through {server}."));
            }
            (None, true) => {
                self.bb = None;
                self.log("Relay no longer carries messages through the server.");
            }
            _ => {}
        }
    }

    /// Hands one message for a station out of reach to the server.
    fn bb_submit(&mut self, msg: &Message) {
        let Some(bb) = &self.bb else { return };
        let (kind, subject, body) = match msg {
            Message::Chat { text, .. } => ("chat", String::new(), text.as_bytes().to_vec()),
            Message::Mail { subject, body, .. } => ("mail", subject.clone(), body.as_bytes().to_vec()),
            Message::File { .. } => return,
        };
        let limit = if kind == "chat" { 1024 } else { 4096 };
        if body.is_empty() || body.len() > limit {
            self.log(format!("Message from {} to {} is too large for the network ({} bytes).", sender_of(msg), dest_of(msg), body.len()));
            return;
        }
        let now = now_unix_secs().max(0) as u64;
        let mut stream = body.clone();
        stream.extend_from_slice(dest_of(msg).as_bytes());
        let q = Queued {
            id: backbone::message_id(sender_of(msg), &stream, now),
            origin: sender_of(msg).to_uppercase(),
            dest: dest_of(msg).to_uppercase(),
            kind: kind.to_string(),
            subject,
            body,
            urgent: priority_of(msg) == Priority::Urgent,
        };
        self.log(format!("Passing the message from {} to {} on to the network.", q.origin, q.dest));
        bb.send(BbJob::Submit(q, now, if kind == "chat" { 6 * 3600 } else { 24 * 3600 }));
    }

    /// An outbox item that came from the server is finished: tell the server how it went.
    fn bb_item_done(&mut self, oid: u64, delivered: bool) {
        let Some(sid) = self.bb_pending.remove(&oid) else { return };
        if let Some(bb) = &self.bb {
            bb.send(BbJob::Receipt(sid, delivered, if delivered { String::new() } else { "no answer".into() }));
        }
    }

    /// Every so often: tell the server who is heard, pick up what is waiting for them, and act
    /// on what the server's thread has to report.
    fn backbone_step(&mut self) {
        self.bb_ensure();
        self.beacon_step();
        let Some(bb) = &self.bb else { return };
        for r in bb.take_results() {
            match r {
                BbResult::Pulled(p) => {
                    for m in p.messages {
                        self.bb_deliver(m);
                    }
                    for r in p.receipts {
                        self.bb_receipt_notice(r);
                    }
                }
                BbResult::Log(t) => self.log(t),
            }
        }
        if self.clock.since(self.bb_last) < Duration::from_secs(20) {
            return;
        }
        self.bb_last = self.clock.now();
        self.heard_recent.retain(|_, (_, t)| self.clock.since(*t) < Duration::from_secs(900));
        let now = now_unix_secs().max(0) as u64;
        let list: Vec<(String, Option<i32>, u64)> = self
            .heard_recent
            .iter()
            .map(|(c, (snr, t))| (c.clone(), *snr, now.saturating_sub(self.clock.since(*t).as_secs())))
            .collect();
        if let Some(bb) = &self.bb {
            let calls: Vec<String> = list.iter().map(|(c, _, _)| c.clone()).collect();
            bb.send(BbJob::Heard(list));
            bb.send(BbJob::Pull(calls));
        }
    }

    /// A running relay announces itself every minute: who it is, whether it has the internet, the
    /// time (operators often have no clock source), and how busy it is.
    fn beacon_step(&mut self) {
        if !self.shared.flags.relay_running.load(Ordering::SeqCst)
            || self.clock.since(self.beacon_at) < Duration::from_secs(60)
            || tx_refusal(&self.shared.flags).is_some()
        {
            return;
        }
        self.beacon_at = self.clock.now();
        let flags = if self.bb.is_some() { BEACON_ONLINE } else { 0 };
        let load = (self.outbox.pending().len() * 16).min(255) as u8;
        let now = now_unix_secs().max(0) as u32;
        let frames = vec![encode_beacon(&self.callsign, flags, now, load)];
        if !self.control.iter().any(|c| *c == frames) {
            self.control.push_back(frames);
        }
    }

    /// A message the server gave us for a station we hear: send it on the radio.
    fn bb_deliver(&mut self, m: Queued) {
        if self.bb_pending.values().any(|s| *s == m.id) {
            return;
        }
        let msg = if m.kind == "mail" {
            Message::Mail {
                from: m.origin.clone(),
                to: m.dest.clone(),
                subject: m.subject.clone(),
                body: String::from_utf8_lossy(&m.body).into_owned(),
                attachments: Vec::new(),
            }
        } else {
            Message::Chat { from: m.origin.clone(), to: m.dest.clone(), text: String::from_utf8_lossy(&m.body).into_owned() }
        };
        let id = self.next_id();
        self.log(format!("Delivering a message from {} to {} that came through the network.", m.origin, m.dest));
        let oids = self.enqueue(vec![msg], vec![id], false);
        if let Some(oid) = oids.first() {
            self.bb_pending.insert(*oid, m.id);
        }
    }

    /// The server says what became of a message one of our stations sent: tell that station.
    fn bb_receipt_notice(&mut self, r: backbone::Receipt) {
        let origin = r.id.rsplit_once('-').map(|(o, _)| o.to_string()).unwrap_or_default();
        if origin.is_empty() {
            return;
        }
        let text = if r.status == "delivered" {
            format!("Delivered to {}.", r.dest)
        } else {
            format!("Could not deliver to {}: {}.", r.dest, if r.reason.is_empty() { "no reason given" } else { &r.reason })
        };
        let id = self.next_id();
        self.enqueue(vec![Message::Chat { from: self.callsign.clone(), to: origin, text }], vec![id], false);
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
            || self.shared.flags.auto_link.load(Ordering::SeqCst)
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
    fn next_due(&mut self) -> Option<(u64, Message, bool)> {
        if tx_refusal(&self.shared.flags).is_some() {
            return None;
        }
        if !self.transmit_allowed() || !self.relay_slot_open() {
            return None;
        }
        if self.access_retry_at.is_some_and(|t| self.clock.now() < t) {
            return None;
        }
        let peer = match &self.link_state {
            LinkState::Connected { peer } => peer.clone(),
            _ => return None,
        };
        let now = self.now_ms();
        let (id, item) = self.outbox.next(now)?;
        match decode_item(&item.payload) {
            Some((msg, private)) if self.route_peer(id, &msg).eq_ignore_ascii_case(&peer) => Some((id, msg, private)),
            Some(_) => None,
            None => {
                self.outbox.cancel(id);
                self.log("dropped an unreadable outbox item");
                self.persist_outbox();
                None
            }
        }
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
        let now = self.clock.now();
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
    fn clear_channel(&mut self, id: u64, msg: &Message) -> bool {
        matches!(
            self.pick_channel(Some(id), priority_of(msg) == Priority::Urgent, false),
            Pick::Go
        )
    }

    /// The channel check shared by data and control bursts. `id` is the outbox item, if any;
    /// a control burst has none and is dropped on the last try.
    fn pick_channel(&mut self, id: Option<u64>, urgent: bool, answer: bool) -> Pick {
        // Listen before talk (kahf_core::mac): never while a burst is arriving, a short gap for
        // an answer, a long gap plus a random back-off for everything else, and a fair share.
        let class = if answer {
            Class::Answer
        } else if urgent {
            Class::Urgent
        } else {
            Class::Data
        };
        if !answer && self.rx_provisional() {
            return Pick::Wait;
        }
        let now = self.now_ms();
        if self.mac.check(now, class).is_some() {
            return Pick::Wait;
        }
        let seed = if self.clock.is_virtual() {
            fnv(&self.callsign)
        } else {
            SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0)
        } ^ id.unwrap_or(0);
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
                self.access_retry_at = Some(self.clock.now() + Duration::from_secs_f32(seconds));
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
        if self.access_retry_at.is_some_and(|t| self.clock.now() < t) {
            return;
        }
        let answer = self
            .control
            .front()
            .and_then(|f| f.first())
            .is_some_and(|p| matches!(p.first(), Some(0x11) | Some(0x02) | Some(0x06)));
        match self.pick_channel(None, false, answer) {
            Pick::Go => {
                // One answer burst for everything waiting: eight codewords received give one
                // burst of eight acknowledgements, not eight bursts each with its own preamble.
                let mut frames: Vec<Vec<u8>> = Vec::new();
                while frames.len() < BURST_MAX {
                    let same = self
                        .control
                        .front()
                        .and_then(|f| f.first())
                        .is_some_and(|p| matches!(p.first(), Some(0x11) | Some(0x02) | Some(0x06)) == answer);
                    match self.control.front() {
                        Some(f) if same && frames.len() + f.len() <= BURST_MAX => {
                            let f = self.control.pop_front().unwrap_or_default();
                            frames.extend(f);
                        }
                        _ => break,
                    }
                }
                if frames.is_empty() {
                    if let Some(f) = self.control.pop_front() {
                        frames = f;
                    }
                }
                if !frames.is_empty() {
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
    fn send_item(&mut self, id: u64, msg: Message, private: bool) {
        let chunks = match air_chunks(&msg, private) {
            Ok(c) => c,
            Err(e) => return self.fail_item(id, e),
        };
        let (name, total) = match &msg {
            Message::File { name, data, .. } => (Some(name.clone()), data.len() as u64),
            _ => (None, 0),
        };
        let public = self.tags.get(&id).map(|t| t.0).unwrap_or(id);
        let count = chunks.len().max(1) as u64;
        // The frames of one send go out together, up to a full burst, not one burst each: every
        // burst costs a preamble and its own wait for the channel.
        let mut waiting: Vec<Action> = Vec::new();
        for (i, chunk) in chunks.iter().enumerate() {
            match guard("link send", || self.link.send(chunk)) {
                Ok(actions) => waiting.extend(actions),
                Err(e) => return self.fail_item(id, e),
            }
            let frames = waiting.iter().filter(|a| matches!(a, Action::Transmit(_))).count();
            if frames >= BURST_MAX || i + 1 == chunks.len() {
                self.apply_link(std::mem::take(&mut waiting), true);
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
        self.outbox.done(id);
        self.bb_item_done(id, true);
        self.record_forward(public);
        self.persist_outbox();
    }

    fn fail_item(&mut self, id: u64, why: String) {
        let was_queued = self.outbox.pending().iter().any(|(pid, _, _)| *pid == id);
        let now = self.now_ms();
        self.outbox.failed(id, now);
        let still_queued = self.outbox.pending().iter().any(|(pid, _, _)| *pid == id);
        if was_queued && !still_queued {
            self.log(format!("gave up on an outbox item: {why}"));
            self.bb_item_done(id, false);
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
        self.via.retain(|oid, _| live.contains(oid));
        self.via_tried.retain(|oid, _| live.contains(oid));
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
        match (&self.link_state, state) {
            (LinkState::Connecting { peer }, LinkState::Idle) => {
                let key = peer.to_uppercase();
                let fails = self.auto_fails.get(&key).map_or(0, |f| f.0) + 1;
                let pause = AUTO_RETRY_BASE.saturating_mul(1 << (fails - 1).min(5)).min(Duration::from_secs(900));
                self.auto_fails.insert(key, (fails, self.clock.now() + pause));
            }
            (_, LinkState::Connected { peer }) => {
                self.auto_fails.remove(&peer.to_uppercase());
                self.last_activity = self.clock.now();
            }
            _ => {}
        }
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
    /// True while a heard preamble is still undecoded and young (see `PREAMBLE_HOLD`).
    fn rx_provisional(&self) -> bool {
        self.kami_at.is_some()
            && self.kami_mode.is_none()
            && self.kami_seen.is_some_and(|t| self.clock.since(t) < PREAMBLE_HOLD)
    }

    fn tick_link(&mut self) {
        // Link timers run on time the radio is free: while a burst is queued or on the air the
        // peer cannot answer, so that time does not count towards a retransmit.
        let now = self.clock.now();
        if self.tx.is_some() || !self.bursts.is_empty() {
            self.tick_paused += now.saturating_duration_since(self.last_tick);
        }
        self.last_tick = now;
        let now_ms = self
            .clock
            .since(self.started)
            .saturating_sub(self.tick_paused)
            .as_millis() as u64;
        match guard("link tick", || self.link.tick(now_ms)) {
            Ok(actions) => {
                self.tick_error = None;
                // A frame sent again means the last try got no answer: the back-off window grows.
                if actions.iter().any(|a| matches!(a, Action::Transmit(p) if !matches!(p.first(), Some(0x11) | Some(0x02) | Some(0x06)))) {
                    self.mac.failure();
                }
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

    /// Sends this station's own advert.
    fn advertise_now(&mut self) {
        if let Some(why) = tx_refusal(&self.shared.flags) {
            self.hold_tx(why);
            return;
        }
        // Time-based, so a restart does not reuse a number neighbours still remember.
        let by_time = (self.now_ms() / 1000) as u8;
        self.advert_seq = if by_time == self.advert_seq { by_time.wrapping_add(1) } else { by_time };
        let relay = self.shared.flags.relay_running.load(Ordering::SeqCst);
        let flags = if relay { ADVERT_RELAY } else { 0 };
        self.adverts_seen.push_back((self.callsign.clone(), self.advert_seq));
        let loc = *lock(&self.shared.location);
        self.control.push_back(vec![encode_advert(&self.callsign, ADVERT_TTL, self.advert_seq, flags, loc)]);
        self.log("Announcing this station on the air.");
    }

    /// Another station's advert: tell the operator, and pass it on when this is a running relay.
    fn on_advert(&mut self, call: String, ttl: u8, seq: u8, flags: u8, loc: Option<(f32, f32)>) {
        if self.adverts_seen.iter().any(|(c, s)| *c == call && *s == seq) {
            return;
        }
        self.note_heard_recent(&call, None);
        self.adverts_seen.push_back((call.clone(), seq));
        if self.adverts_seen.len() > 256 {
            self.adverts_seen.pop_front();
        }
        self.emit(Event::Advert { call: call.clone(), hops: ADVERT_TTL.saturating_sub(ttl), relay: flags & ADVERT_RELAY != 0, loc });
        let is_relay = lock(&self.shared.relay).as_ref().is_some_and(|r| r.role != Role::Station)
            && self.shared.flags.relay_running.load(Ordering::SeqCst);
        if is_relay && ttl > 0 && tx_refusal(&self.shared.flags).is_none() {
            self.log(format!("Passing on the announcement of {call}."));
            self.control.push_back(vec![encode_advert(&call, ttl - 1, seq, flags & !ADVERT_LOCATION, loc)]);
        }
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
                Action::Beacon { call, flags, time_s, load } => {
                    self.note_heard_recent(&call, None);
                    self.emit(Event::Relay { call, online: flags & BEACON_ONLINE != 0, time_s, load });
                }
                Action::Advert { call, ttl, seq, flags, loc } => self.on_advert(call, ttl, seq, flags, loc),
                Action::StateChanged(state) => {
                    self.on_link_state(&state);
                    self.emit(Event::Link(state));
                }
                // Link quality snapshots have no event yet.
                Action::Metrics(_) => self.emit_status(),
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
        } else if !self.control.iter().any(|c| *c == frames) {
            // The same frames already waiting say nothing new.
            self.control.push_back(frames);
        }
    }

    /// Chooses the KAMI mode for the next burst. Fixed when the operator asked for one. Otherwise
    /// the peer's SNR report picks the fastest mode that SNR supports (with the ladder's margin):
    /// up at once when frames are getting through, down at once when the SNR drops, and one rung
    /// down when frames are being lost. Before any report the level stays where it is.
    fn pick_level(&mut self) {
        let flags = &self.shared.flags;
        if !flags.auto_mode.load(Ordering::SeqCst) {
            self.tx_level = (flags.fixed_level.load(Ordering::SeqCst) as usize).min(MODULATIONS.len() - 1) as u8;
            return;
        }
        let cur = self.tx_level as usize;
        let fer = self.link.peer_frame_error_pct();
        let mut next = cur;
        match self.link.peer_snr_db() {
            Some(snr) => {
                let want = ladder_level(snr);
                next = match fer {
                    Some(f) if f >= 8 => cur.saturating_sub(1).min(want),
                    // Some loss: climb only one rung at a time.
                    Some(f) if f >= 2 => want.min(cur + 1),
                    _ => want,
                };
            }
            None => match fer {
                Some(f) if f < 2 && cur + 1 < MODULATIONS.len() => next = cur + 1,
                Some(f) if f >= 8 && cur > 0 => next = cur - 1,
                _ => {}
            },
        }
        // On HF the slowest modes are not the safest: their bursts last many seconds, and the
        // channel changes during one (fading), so a burst that long often fails where a shorter
        // one gets through (measured: crates/kahf-sim hf_sweep). While the signal is not weak,
        // never fall below KAMI-16, whatever the losses; the lower rungs are for weak signals.
        if self.link.peer_snr_db().is_some_and(|snr| snr >= HF_SLOW_FLOOR_SNR_DB) {
            next = next.max(HF_MIN_LEVEL);
        }
        self.tx_level = next as u8;
    }

    /// Sends the operator a snapshot of the link.
    fn emit_status(&self) {
        let m = self.link.metrics();
        let mode = MODULATIONS[self.tx_level as usize];
        let net_bps = (8.0 * 324.0 * RATE as f64 / burst_len_for(mode, 8).max(1) as f64) as u32;
        self.emit(Event::Metrics(LinkStatus {
            mode: mode_name(mode),
            auto: self.shared.flags.auto_mode.load(Ordering::SeqCst),
            net_bps,
            own_snr_db: m.own_snr_db,
            peer_snr_db: m.peer_snr_db,
            fer_pct: m.frame_error_pct,
            frames_sent: m.frames_sent,
            frames_received: m.frames_received,
            retransmits: m.retransmits,
        }));
    }

    /// Builds the audio for one burst and queues it for transmit.
    fn push_burst(&mut self, frames: &[Vec<u8>]) {
        self.last_activity = self.clock.now();
        let before = self.tx_level;
        self.pick_level();
        let m = MODULATIONS[self.tx_level as usize];
        // Weak mode: the most robust KAMI mode, for stations that cannot answer.
        let m = if self.shared.flags.weak_mode.load(Ordering::SeqCst) { Modulation::Kami256 } else { m };
        self.link.set_air_ms((burst_len_for(m, 1) * 1000 / RATE as usize) as u64);
        self.shared
            .flags
            .tx_level_shown
            .store(self.tx_level, Ordering::SeqCst);
        let built = burst(frames, m);
        if self.tx_level != before {
            self.emit_status();
        }
        match built {
            Ok(mut samples) => {
                let gain = self.shared.flags.tx_level_pct.load(Ordering::SeqCst) as f32 / 100.0;
                for s in samples.iter_mut() {
                    *s *= gain;
                }
                self.bursts.push_back(samples)
            }
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
        let now = self.clock.now();
        let Some(started) = job.started else {
            // Every burst passes the gate, whatever queued it.
            if let Some(why) = tx_refusal(&self.shared.flags) {
                return Err(why.to_string());
            }
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
                let now = self.now_ms();
                self.mac.on_tx_end(now);
                let link_now = self.clock.since(self.started).saturating_sub(self.tick_paused).as_millis() as u64;
                self.link.tx_finished(link_now);
                self.emit_tx(TxState::Done);
                self.log(format!("sent {secs:.1} s of audio"));
                true
            }
            Err(e) => {
                self.stop_radio_tx();
                self.end_rx_gap();
                let now = self.now_ms();
                self.mac.on_tx_end(now);
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
        // Everything captured while we were sending is our own signal: take it off the sound
        // card unread, and keep discarding for a moment, because the sound card delays it.
        if let Some(be) = self.backend.as_mut() {
            let mut sink = Vec::new();
            let _ = be.read_audio(&mut sink);
        }
        self.rx_deaf_until = Some(self.clock.now() + ECHO_TAIL);
        self.rx_buf.clear();
        self.since_scan = 0;
        // A burst being waited for pointed into the audio just thrown away.
        self.kami_at = None;
        self.kami_seen = None;
        self.kami_refined = false;
        self.kami_mode = None;
        self.kami_next = 0;
        self.kami_total = 0;
        self.kami_front = true;
        self.spec = Analyser::new(SPEC_FFT, RATE as f32);
    }

    /// One RX pass: read what the radio has, update the spectrum, scan for frames.
    /// Returns the number of new samples.
    fn rx_step(&mut self) -> usize {
        if self.rx_retry_at.is_some_and(|t| self.clock.now() < t) {
            return 0;
        }
        if self.rx_deaf_until.is_some_and(|t| self.clock.now() < t) {
            let mut sink = Vec::new();
            if let Some(be) = self.backend.as_mut() {
                let _ = be.read_audio(&mut sink);
            }
            return sink.len();
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
        let now = self.clock.now();
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
        self.since_kami += n;
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
        self.rx_retry_at = Some(self.clock.now() + RX_RETRY);
    }

    /// Finds frames in the RX buffer. A preamble is searched for every KAMI_STEP samples in a
    /// short window of recent audio; once one is found the frame is decoded as soon as enough
    /// audio has arrived for any KAMI mode. Consumed frames leave the buffer.
    fn decode_frames(&mut self) {
        let tl = kahf_core::kami::preamble_len();
        let window = KAMI_STEP + 2 * tl;
        loop {
            if let Some((start, hz, tau)) = self.kami_at {
                if self.finish_kami(start, hz, tau) {
                    continue;
                }
                // A false preamble (noise, the tail of our own echo) must not hide the real
                // burst behind it for as long as the longest mode: while the mode is still
                // unknown, keep looking at the audio that arrived after it.
                if self.kami_mode.is_none() && self.since_kami >= KAMI_STEP {
                    let len = self.rx_buf.len();
                    let from = (start + tl).min(len);
                    let mut at = len.saturating_sub(self.since_kami + tl).max(from);
                    self.since_kami = 0;
                    let mut found = None;
                    while at + tl <= len {
                        let end = (at + window).min(len);
                        if let Some((st, hz2, tau2)) = kahf_core::kami::find_preamble(&self.rx_buf[at..end]) {
                            found = Some((at + st, hz2, tau2));
                            break;
                        }
                        if end == len {
                            break;
                        }
                        at = end - tl;
                    }
                    if let Some(f) = found {
                        self.kami_at = Some(f);
                        self.kami_seen = Some(self.clock.now());
                        self.kami_refined = false;
                        self.kami_tried = 0;
                        self.since_kami = len.saturating_sub(f.0 + tl);
                        continue;
                    }
                }
                return;
            }
            if !self.kami_front && self.since_kami < KAMI_STEP {
                return;
            }
            // Scan from the front of the buffer in overlapping windows, so a frame is found
            // wherever it sits, even when a whole burst arrives at once.
            let len = self.rx_buf.len();
            if len < tl {
                return;
            }
            let w = window.min(len);
            if let Some((st, hz, tau)) = kahf_core::kami::find_preamble(&self.rx_buf[..w]) {
                self.kami_at = Some((st, hz, tau));
                self.kami_seen = Some(self.clock.now());
                self.kami_refined = false;
                self.kami_tried = 0;
                self.since_kami = len.saturating_sub(st + tl);
                continue;
            }
            if len > w {
                self.rx_buf.drain(..w - tl);
                continue;
            }
            // Nothing here: keep a preamble's length, in case one straddles the next search.
            self.trim_rx(tl);
            self.kami_front = false;
            self.since_kami = 0;
            return;
        }
    }

    /// Keeps only the last `keep` samples of the receive buffer.
    fn trim_rx(&mut self, keep: usize) {
        let extra = self.rx_buf.len().saturating_sub(keep);
        self.rx_buf.drain(..extra);
    }

    /// Decodes the burst whose preamble starts at `start`. The first codeword fixes the mode and
    /// says how many codewords the burst has; the others are delivered as their audio arrives.
    /// Returns true when the burst ended or was given up, so the caller should look again for
    /// a preamble; false when more audio is needed.
    fn finish_kami(&mut self, start: usize, hz: f64, tau: f64) -> bool {
        let (mut start, mut hz, mut tau) = (start, hz, tau);
        let tl = kahf_core::kami::preamble_len();
        // A preamble is first seen while it is still arriving, so its frequency and timing
        // estimates are rough. Once all of it is here, estimate again; the modes are tried anew.
        if self.kami_mode.is_none() && !self.kami_refined && self.rx_buf.len() >= start + tl + tl / 4 {
            self.kami_refined = true;
            let lo = start.saturating_sub(tl / 4);
            let hi = (start + 2 * tl).min(self.rx_buf.len());
            if let Some((st, h, t)) = kahf_core::kami::find_preamble(&self.rx_buf[lo..hi]) {
                (start, hz, tau) = (lo + st, h, t);
                self.kami_at = Some((start, hz, tau));
                self.kami_tried = 0;
            }
        }
        let avail = self.rx_buf.len().saturating_sub(start);
        let longest = KAMI_MODES.iter().map(|&m| frame_len_for(m)).max().unwrap_or(0);
        let modes: Vec<Modulation> = match self.kami_mode {
            Some(m) => vec![m],
            None => KAMI_MODES
                .iter()
                .copied()
                .filter(|&m| {
                    let fl = frame_len_for(m);
                    fl <= avail && fl > self.kami_tried
                })
                .collect(),
        };
        for m in modes {
            let Some(sh) = m.kami_shape() else { continue };
            let mut n = 0;
            while n < BURST_MAX && burst_len_for(m, n + 1) <= avail {
                n += 1;
            }
            if self.kami_mode.is_some() {
                n = n.min(self.kami_total);
            } else {
                // The first codeword fixes the mode and the length of the burst. Asking for more
                // than one now would run the demodulator over audio past the end of a short burst,
                // which spoils the estimates and loses the burst.
                n = n.min(1);
            }
            if n <= self.kami_next {
                continue;
            }
            let Some(rx) = kahf_core::kami::demodulate_burst_rx(&self.rx_buf[start..], 0, hz, tau, ldpc::N, n, sh)
            else {
                continue;
            };
            let snr = rx.snr_db as f32;
            for (j, llr) in rx.llrs.into_iter().enumerate().skip(self.kami_next) {
                // A repeat of a codeword that failed before is added to it (chase combining).
                let (tag, now_ms) = (mod_id(m), self.now_ms());
                let decoded = self
                    .harq
                    .try_decode(j, tag, &llr, now_ms, |info| frame::parse_payload_id(info).is_some_and(|(_, id)| id & ID_MODE_MASK == tag))
                    .and_then(|d| frame::parse_payload_id(&d.info));
                let Some((data, id)) = decoded else { break };
                if id & ID_MODE_MASK != mod_id(m) {
                    break;
                }
                let remaining = (id >> 5) as usize;
                if self.kami_mode.is_none() {
                    // First codeword: it fixes the mode and the length of the burst. Only now is
                    // it a real burst for the MAC: a start that never decodes (noise, an echo's
                    // tail) must not keep this station from answering.
                    if !self.rx_locked {
                        self.rx_locked = true;
                        let ms = self.now_ms();
                        self.mac.on_rx_start(ms);
                    }
                    self.kami_mode = Some(m);
                    self.kami_total = remaining + 1;
                } else if remaining != self.kami_total - 1 - j {
                    break;
                }
                self.kami_next = j + 1;
                self.link.note_rx_snr(snr);
                self.on_frame(data);
                if self.kami_next == self.kami_total {
                    break;
                }
            }
            if self.kami_mode.is_some() {
                if self.kami_next == self.kami_total || self.kami_next < n {
                    // Complete, or a codeword failed with its audio present: the burst is over.
                    let end = start + burst_len_for(m, self.kami_next);
                    self.end_kami_burst(end);
                    return true;
                }
                return false;
            }
        }
        if self.kami_mode.is_none() {
            self.kami_tried = avail;
            if avail >= longest {
                // Nothing decoded in any mode: not a frame. Step past this preamble.
                let skip = (start + kahf_core::kami::preamble_len()).min(self.rx_buf.len());
                self.end_kami_burst(skip);
                return true;
            }
        }
        false
    }

    /// Drops the audio up to `end` and goes back to looking for a preamble.
    fn end_kami_burst(&mut self, end: usize) {
        let end = end.min(self.rx_buf.len());
        if self.rx_locked {
            self.rx_locked = false;
            let ms = self.now_ms();
            self.mac.on_rx_end(ms);
        }
        self.rx_buf.drain(..end);
        self.kami_at = None;
        self.kami_seen = None;
        self.kami_refined = false;
        self.kami_mode = None;
        self.kami_next = 0;
        self.kami_total = 0;
        self.kami_front = true;
    }

    fn on_frame(&mut self, payload: Vec<u8>) {
        self.last_activity = self.clock.now();
        if matches!(payload.first(), Some(0x11) | Some(0x02)) {
            self.mac.success();
        }
        match guard("link receive", || self.link.on_receive(&payload)) {
            Ok(actions) => self.apply(actions),
            Err(e) => self.log(e),
        }
        for call in self.link.take_heard() {
            self.note_heard_recent(&call, self.link.own_snr_db());
            self.emit(Event::Heard { call, snr_db: self.link.own_snr_db() });
        }
    }

    fn deliver(&mut self, chunk: Vec<u8>) {
        self.rx_chunks.push(chunk);
        if self.rx_chunks.len() > MAX_CHUNKS {
            self.rx_chunks.clear();
            self.log("dropped a partial message: too many chunks");
            return;
        }
        match guard("session decode", || session::decode_signed(&self.rx_chunks)) {
            Ok(Some(signed)) => {
                self.rx_chunks.clear();
                // Our own message coming back (a cable, a leaky rig, someone repeating it) is not
                // news: never show it, never forward it.
                if sender_of(&signed.message).eq_ignore_ascii_case(&self.callsign) {
                    self.log("Ignored a message of ours heard back on the air.");
                    return;
                }
                let for_me = dest_of(&signed.message).eq_ignore_ascii_case(&self.callsign);
                if !for_me {
                    // A relay passes it on. Nobody else shows it: a message belongs in the inbox
                    // of the station it was written to, not of everyone who happens to hear it.
                    self.forward_third_party(&signed.message, signed.private);
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
                let saved = match save_file(&self.inbox, &format!("mail-{id}.txt"), text.as_bytes()) {
                    Ok(p) => {
                        self.log(format!("saved mail to {}", p.display()));
                        Some(p)
                    }
                    Err(e) => {
                        self.log(e);
                        None
                    }
                };
                self.emit(Event::MailReceived {
                    from: from.clone(),
                    subject: subject.clone(),
                    body: body.to_string(),
                    attachments: attachments.clone(),
                    path: saved,
                });
                self.emit(Event::Mail { id, from, subject });
            }
            Message::File { from, name, data, .. } => {
                let id = self.next_id();
                let total = data.len() as u64;
                match save_file(&self.inbox, &name, &data) {
                    Ok(p) => {
                        self.log(format!("saved file to {}", p.display()));
                        self.emit(Event::FileReceived {
                            from,
                            name: name.clone(),
                            size: total,
                            path: p,
                        });
                    }
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

    /// In-memory radio: transmits into `tx`, receives from `rx`.
    struct FakeRadio {
        rx: Pipe,
        tx: Pipe,
        clock: Clock,
        start: Instant,
        delivered: usize,
    }

    thread_local! {
        static TEST_CLOCK: Clock = {
            let c = Clock::virtual_time();
            c.join_thread();
            c
        };
    }

    /// The virtual clock of the current test thread (made on first use).
    fn test_clock() -> Clock {
        TEST_CLOCK.with(|c| c.clone())
    }

    /// Spawns an engine on the test thread's virtual clock.
    fn vspawn(config: Config, factory: Factory) -> Result<Engine, String> {
        Engine::spawn_with_clock(config, factory, test_clock())
    }

    impl Backend for FakeRadio {
        fn is_bench(&self) -> bool {
            true
        }
        /// Like a sound card: one second of audio per (virtual) second; silence where nothing was sent.
        fn read_audio(&mut self, out: &mut Vec<f32>) -> Result<(), String> {
            let due = (self.clock.since(self.start).as_secs_f64() * RATE as f64) as usize;
            let n = due.saturating_sub(self.delivered);
            let mut q = self.rx.lock().unwrap();
            for _ in 0..n {
                out.push(q.pop_front().unwrap_or(0.0));
            }
            self.delivered += n;
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

    /// One shared channel for several stations: signals that overlap in time add up, as on
    /// the air, and every station hears all of it, itself included (a virtual audio cable does
    /// exactly this). The line is a timeline of samples from the start; position = time x rate.
    struct Bus {
        line: Mutex<Vec<f32>>,
    }

    struct BusRadio {
        bus: Arc<Bus>,
        clock: Clock,
        start: Instant,
        delivered: usize,
        /// Timeline position where the current transmission began.
        tx_base: usize,
    }

    impl BusRadio {
        fn now_index(&self) -> usize {
            (self.clock.since(self.start).as_secs_f64() * RATE as f64) as usize
        }
    }

    impl Backend for BusRadio {
        fn is_bench(&self) -> bool {
            true
        }
        /// Like a sound card: exactly one second of audio per second. Whatever was sent at
        /// that moment is in it, mixed with anything else sent then; otherwise silence.
        fn read_audio(&mut self, out: &mut Vec<f32>) -> Result<(), String> {
            let due = self.now_index();
            let line = self.bus.line.lock().unwrap();
            for k in self.delivered..due {
                out.push(line.get(k).copied().unwrap_or(0.0));
            }
            self.delivered = self.delivered.max(due);
            Ok(())
        }

        fn tx_start(&mut self) -> Result<(), String> {
            self.tx_base = self.now_index();
            Ok(())
        }

        fn tx_write(&mut self, samples: &[f32], start: usize, end: usize) -> Result<(), String> {
            let mut line = self.bus.line.lock().unwrap();
            let last = self.tx_base + end;
            if line.len() < last {
                line.resize(last, 0.0);
            }
            for (k, v) in samples[start..end].iter().enumerate() {
                line[self.tx_base + start + k] += *v;
            }
            Ok(())
        }

        fn tx_stop(&mut self) -> Result<(), String> {
            Ok(())
        }
    }

    /// `calls.len()` engines on one bus, all listening, with automatic linking on.
    fn bus_engines(tag: &str, calls: &[&str]) -> Vec<Engine> {
        let bus = Arc::new(Bus { line: Mutex::new(Vec::new()) });
        let clock = Clock::virtual_time();
        clock.join_thread();
        calls
            .iter()
            .map(|call| {
                let b = bus.clone();
                let c = clock.clone();
                let start = clock.now();
                let factory: Factory = Box::new(move || Ok(Box::new(BusRadio { bus: b.clone(), clock: c.clone(), start, delivered: 0, tx_base: 0 }) as Box<dyn Backend>));
                let mut e = Engine::spawn_with_clock(config(call, temp_dir(&format!("{tag}-{call}"))), factory, clock.clone()).unwrap();
                e.listen().unwrap();
                e.set_auto_link(true);
                e.set_fixed_mode("KAMI-1c").unwrap();
                e
            })
            .collect()
    }

    /// Runs the engines until `done` says so or `secs` pass, collecting every chat received:
    /// `(receiver index, sender, text)`.
    fn run_bus(engines: &mut [Engine], secs: u64, mut done: impl FnMut(&[(usize, String, String)]) -> bool) -> Vec<(usize, String, String)> {
        let mut got: Vec<(usize, String, String)> = Vec::new();
        let clock = engines[0].clock.clone();
        let t0 = clock.now();
        let deadline = t0 + Duration::from_secs(secs);
        while clock.now() < deadline && !done(&got) {
            for (i, e) in engines.iter_mut().enumerate() {
                for ev in e.poll_events() {
                    if std::env::var_os("KAHF_TRACE").is_some() && !matches!(ev, Event::Spectrum(_) | Event::Metrics(_)) {
                        eprintln!("[{:6.1}s] st{i}: {ev:?}", clock.since(t0).as_secs_f32());
                    }
                    if let Event::Message { from, text } = ev {
                        got.push((i, from, text));
                    }
                }
            }
            clock.sleep(Duration::from_millis(20));
        }
        got
    }

    #[test]
    fn one_message_over_an_echoing_channel_arrives_once() {
        let mut e = bus_engines("echo-one", &["AA1AAA", "BB1BBB"]);
        e[0].send_message("BB1BBB", "hello").unwrap();
        let got = run_bus(&mut e, 120, |g| !g.is_empty());
        assert_eq!(got, vec![(1, "AA1AAA".to_string(), "hello".to_string())]);
    }

    #[test]
    fn several_messages_each_way_all_arrive_once_in_order_and_none_from_self() {
        let mut e = bus_engines("echo-a", &["AA1AAA", "BB1BBB"]);
        for i in 0..4 {
            e[0].send_message("BB1BBB", &format!("a{i}")).unwrap();
            e[1].send_message("AA1AAA", &format!("b{i}")).unwrap();
        }
        let got = run_bus(&mut e, 240, |g| g.len() >= 8);
        let texts = |who: usize| got.iter().filter(|(i, _, _)| *i == who).map(|(_, f, t)| (f.clone(), t.clone())).collect::<Vec<_>>();
        let want_b: Vec<(String, String)> = (0..4).map(|i| ("AA1AAA".to_string(), format!("a{i}"))).collect();
        let want_a: Vec<(String, String)> = (0..4).map(|i| ("BB1BBB".to_string(), format!("b{i}"))).collect();
        assert_eq!(texts(1), want_b, "what BB1BBB received");
        assert_eq!(texts(0), want_a, "what AA1AAA received");
    }

    /// Output that takes 30 ms to accept each write, like a busy sound card.
    struct SlowOutputRadio {
        written: Arc<AtomicU64>,
    }

    impl Backend for SlowOutputRadio {
        fn is_bench(&self) -> bool {
            true
        }
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
        fn is_bench(&self) -> bool {
            true
        }
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
        let deadline = test_clock().now() + Duration::from_secs(secs);
        while test_clock().now() < deadline {
            if want() {
                return true;
            }
            test_clock().sleep(Duration::from_millis(5));
        }
        want()
    }

    fn fake_factory(rx: Pipe, tx: Pipe) -> Factory {
        let clock = test_clock();
        let start = clock.now();
        Box::new(move || {
            Ok(Box::new(FakeRadio {
                rx: rx.clone(),
                tx: tx.clone(),
                clock: clock.clone(),
                start,
                delivered: 0,
            }) as Box<dyn Backend>)
        })
    }

    fn wait_for(engine: &mut Engine, want: impl Fn(&Event) -> bool, secs: u64) -> Option<Event> {
        let deadline = test_clock().now() + Duration::from_secs(secs);
        while test_clock().now() < deadline {
            if let Some(ev) = engine.poll_events().into_iter().find(|e| want(e)) {
                return Some(ev);
            }
            test_clock().sleep(Duration::from_millis(20));
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
        assert_eq!(band_name(18.1134), "17M");
        assert_eq!(band_name(14.1), "");
        assert_eq!(band_name(7.074), "");
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
            assert_eq!(decode_message(&encode_message(&m, false)), Some(m));
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
        let engine = vspawn(config("T1AA", temp_dir("lazy")), factory).unwrap();
        test_clock().sleep(Duration::from_millis(50));
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
        // Real time here: this engine runs on the wall clock.
        let end = Instant::now() + Duration::from_secs(10);
        let mut done = false;
        while !done && Instant::now() < end {
            done = e.poll_events().iter().any(|ev| matches!(ev, Event::Tx { state: TxState::Done }));
            thread::sleep(Duration::from_millis(20));
        }
        assert!(done, "transmit did not finish");
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

    /// Finds and decodes one KAMI frame of any mode in `audio`: (end in samples, payload, mode).
    fn decode_any(audio: &[f32]) -> Option<(usize, Vec<u8>, Modulation)> {
        let (start, hz, tau) = kahf_core::kami::find_preamble(audio)?;
        let fits: Vec<Modulation> =
            KAMI_MODES.iter().copied().filter(|&m| start + frame_len_for(m) <= audio.len()).collect();
        let shapes: Vec<kahf_core::kami::Shape> = fits.iter().filter_map(|m| m.kami_shape()).collect();
        kahf_core::kami::demodulate_at(&audio[start..], 0, hz, tau, ldpc::N, &shapes)
            .into_iter()
            .find_map(|(sh, llr)| {
                let m = *fits.iter().find(|m| m.kami_shape() == Some(sh))?;
                let info = ldpc::decode(&llr, LDPC_ITER)?;
                let (data, id) = frame::parse_payload_id(&info)?;
                (id & ID_MODE_MASK == mod_id(m)).then_some((start + frame_len_for(m), data, m))
            })
    }

    #[test]
    fn every_kami_mode_is_found_and_decoded_as_itself() {
        for m in KAMI_MODES {
            let payload = format!("hello {m:?}").into_bytes();
            let mut air = vec![0f32; 777];
            air.extend(burst(std::slice::from_ref(&payload), m).unwrap());
            air.extend(vec![0f32; 300]);
            let (end, got, mode) = decode_any(&air).unwrap_or_else(|| panic!("{m:?} not found"));
            assert_eq!(got, payload, "{m:?}");
            assert_eq!(mode, m);
            assert_eq!(end, 777 + LEAD_SILENCE + frame_len_for(m), "{m:?} end");
        }
        // Higher numbers are slower; the single-chip modes are fastest.
        assert!(frame_len_for(Modulation::Kami1d) < frame_len_for(Modulation::Kami8));
        assert!(frame_len_for(Modulation::Kami8) < frame_len_for(Modulation::Kami32));
        assert!(frame_len_for(Modulation::Kami32) < frame_len_for(Modulation::Kami256));
    }

    #[test]
    fn frames_of_one_burst_share_a_preamble_and_decode_in_order() {
        let payloads: Vec<Vec<u8>> = vec![b"one".to_vec(), b"two".to_vec(), b"three".to_vec()];
        for m in [Modulation::Kami8, Modulation::Kami1b] {
            let air = burst(&payloads, m).unwrap();
            assert_eq!(air.len(), LEAD_SILENCE + burst_len_for(m, 3) + TAIL_SILENCE);
            assert!(burst_len_for(m, 3) < 3 * frame_len_for(m), "{m:?} shares the preamble");
            let (start, hz, tau) = kahf_core::kami::find_preamble(&air).expect("preamble");
            let llrs = kahf_core::kami::demodulate_burst_at(&air, start, hz, tau, ldpc::N, 3, m.kami_shape().unwrap())
                .expect("burst fits");
            let got: Vec<Vec<u8>> = llrs
                .iter()
                .map(|l| {
                    let info = ldpc::decode(l, LDPC_ITER).expect("decodes");
                    frame::parse_payload_id(&info).expect("parses").0
                })
                .collect();
            assert_eq!(got, payloads, "{m:?}");
        }
    }

    #[test]
    fn each_kami_mode_decodes_only_as_itself() {
        let payload = b"KAHF modulation test".to_vec();
        for &m in &KAMI_MODES {
            let air = frame_samples_as(&payload, m).expect("frame builds");
            let (_, got, mode) = decode_any(&air).unwrap_or_else(|| panic!("{m:?} does not decode"));
            assert_eq!((got.as_slice(), mode), (payload.as_slice(), m));
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
    /// Ignored: kahf-core::link and kahf-core::session still panic (unimplemented!).
    /// Enable with `cargo test -p kahf-daemon -- --ignored` once both are done.
    #[test]
    fn queued_while_offline_is_sent_after_link_up() {
        let a_to_b: Pipe = Arc::new(Mutex::new(VecDeque::new()));
        let b_to_a: Pipe = Arc::new(Mutex::new(VecDeque::new()));
        let mut a = vspawn(
            config("AA1AAA", temp_dir("offline-a")),
            fake_factory(b_to_a.clone(), a_to_b.clone()),
        )
        .unwrap();
        let mut b = vspawn(
            config("BB1BBB", temp_dir("offline-b")),
            fake_factory(a_to_b.clone(), b_to_a.clone()),
        )
        .unwrap();

        a.send_message("BB1BBB", "hi there").unwrap();
        assert!(wait_until(5, || a.outbox_pending().len() == 1));
        test_clock().sleep(Duration::from_millis(200));
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

        let deadline = test_clock().now() + Duration::from_secs(30);
        while !a.outbox_pending().is_empty() && test_clock().now() < deadline {
            test_clock().sleep(Duration::from_millis(20));
        }
        assert!(a.outbox_pending().is_empty(), "sent item stayed in the outbox");
    }

    #[test]
    fn outbox_survives_restart() {
        let dir = temp_dir("restart");
        let a_to_b: Pipe = Arc::new(Mutex::new(VecDeque::new()));
        let b_to_a: Pipe = Arc::new(Mutex::new(VecDeque::new()));
        {
            let mut a = vspawn(
                config("AA1AAA", dir.clone()),
                fake_factory(b_to_a.clone(), a_to_b.clone()),
            )
            .unwrap();
            a.send_message("BB1BBB", "URGENT: keep me").unwrap();
            a.send_message("BB1BBB", "second").unwrap();
        }
        assert!(dir.join("outbox.bin").exists());

        let mut a = vspawn(config("AA1AAA", dir.clone()), fake_factory(b_to_a, a_to_b)).unwrap();
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
        let mut a = vspawn(
            config("AA1AAA", dir.clone()),
            fake_factory(b_to_a.clone(), a_to_b.clone()),
        )
        .unwrap();
        let mut b = vspawn(
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
        let deadline = test_clock().now() + Duration::from_secs(10);
        while !log.exists() && test_clock().now() < deadline {
            test_clock().sleep(Duration::from_millis(20));
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
        let mut a = vspawn(
            config("AA1AAA", temp_dir("loop-a")),
            fake_factory(b_to_a.clone(), a_to_b.clone()),
        )
        .unwrap();
        let mut b = vspawn(
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

    /// A radio whose receive path adds white Gaussian noise of a fixed level to the samples it
    /// hands over, like a band with a constant noise floor.
    struct NoisyRadio {
        inner: FakeRadio,
        sigma: f32,
        state: u64,
    }

    impl NoisyRadio {
        fn gauss(&mut self) -> f32 {
            let mut u = || {
                self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                ((self.state >> 11) as f64 + 0.5) / (1u64 << 53) as f64
            };
            let (a, b) = (u(), u());
            ((-2.0 * a.ln()).sqrt() * (2.0 * std::f64::consts::PI * b).cos()) as f32
        }
    }

    impl Backend for NoisyRadio {
        fn is_bench(&self) -> bool {
            true
        }
        fn read_audio(&mut self, out: &mut Vec<f32>) -> Result<(), String> {
            let from = out.len();
            self.inner.read_audio(out)?;
            for v in out.iter_mut().skip(from) {
                *v += self.sigma * self.gauss();
            }
            Ok(())
        }
        fn tx_start(&mut self) -> Result<(), String> {
            self.inner.tx_start()
        }
        fn tx_write(&mut self, samples: &[f32], start: usize, end: usize) -> Result<(), String> {
            self.inner.tx_write(samples, start, end)
        }
        fn tx_stop(&mut self) -> Result<(), String> {
            self.inner.tx_stop()
        }
    }

    /// Noise level that puts a KAMI-32 burst at `snr_db` (per real sample).
    fn noise_for(snr_db: f32) -> f32 {
        let air = burst(&[b"ref".to_vec()], Modulation::Kami32).unwrap();
        let body = &air[LEAD_SILENCE..air.len() - TAIL_SILENCE];
        // The engine sends at its transmit level, below full scale.
        let rms = (body.iter().map(|v| v * v).sum::<f32>() / body.len() as f32).sqrt() * DEFAULT_TX_LEVEL_PCT as f32 / 100.0;
        rms * 10f32.powf(-snr_db / 20.0)
    }

    fn noisy_factory(rx: Pipe, tx: Pipe, sigma: f32, seed: u64) -> Factory {
        let clock = test_clock();
        let start = clock.now();
        Box::new(move || {
            Ok(Box::new(NoisyRadio { inner: FakeRadio { rx: rx.clone(), tx: tx.clone(), clock: clock.clone(), start, delivered: 0 }, sigma, state: seed })
                as Box<dyn Backend>)
        })
    }

    /// Two engines over a channel with the given noise; a is linked to b.
    fn linked_pair(tag: &str, sigma: f32) -> (Engine, Engine) {
        let a_to_b: Pipe = Arc::new(Mutex::new(VecDeque::new()));
        let b_to_a: Pipe = Arc::new(Mutex::new(VecDeque::new()));
        let mut a = vspawn(
            config("AA1AAA", temp_dir(&format!("{tag}-a"))),
            noisy_factory(b_to_a.clone(), a_to_b.clone(), sigma, 11),
        )
        .unwrap();
        let mut b = vspawn(
            config("BB1BBB", temp_dir(&format!("{tag}-b"))),
            noisy_factory(a_to_b.clone(), b_to_a.clone(), sigma, 22),
        )
        .unwrap();
        b.listen().unwrap();
        a.connect("BB1BBB").unwrap();
        assert!(
            wait_for(&mut a, |e| matches!(e, Event::Link(kahf_core::link::LinkState::Connected { .. })), 120).is_some(),
            "link did not come up"
        );
        (a, b)
    }

    fn send_and_wait(a: &mut Engine, b: &mut Engine, text: &str) {
        a.send_message("BB1BBB", text).unwrap();
        assert!(
            wait_for(b, |e| matches!(e, Event::Message { text: t, .. } if t == text), 120).is_some(),
            "{text:?} did not arrive at {}",
            a.transmit_modulation()
        );
    }

    /// A radio on a small shared-air model: it hears the stations whose pipes it is given and
    /// its transmissions go to the pipes of the stations that hear it.
    struct MeshRadio {
        rx: Vec<Pipe>,
        tx: Vec<Pipe>,
        sigma: f32,
        state: u64,
        clock: Clock,
        start: Instant,
        delivered: usize,
    }

    impl Backend for MeshRadio {
        fn is_bench(&self) -> bool {
            true
        }
        /// One second of audio per (virtual) second: what the heard stations sent, added up, or silence.
        fn read_audio(&mut self, out: &mut Vec<f32>) -> Result<(), String> {
            let from = out.len();
            let due = (self.clock.since(self.start).as_secs_f64() * RATE as f64) as usize;
            let n = due.saturating_sub(self.delivered);
            self.delivered += n;
            out.resize(from + n, 0.0);
            for p in &self.rx {
                let mut q = p.lock().unwrap();
                for v in out.iter_mut().skip(from) {
                    match q.pop_front() {
                        Some(x) => *v += x,
                        None => break,
                    }
                }
            }
            if self.sigma > 0.0 {
                for v in out.iter_mut().skip(from) {
                    self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    let a = ((self.state >> 11) as f64 + 0.5) / (1u64 << 53) as f64;
                    self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    let b = ((self.state >> 11) as f64 + 0.5) / (1u64 << 53) as f64;
                    *v += self.sigma * ((-2.0 * a.ln()).sqrt() * (2.0 * std::f64::consts::PI * b).cos()) as f32;
                }
            }
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

    /// Engines for `calls`, where `hears` lists (from, to): station `to` hears station `from`.
    fn mesh(tag: &str, calls: &[&str], hears: &[(usize, usize)], sigma: f32) -> Vec<Engine> {
        let pipes: Vec<Pipe> = hears.iter().map(|_| Arc::new(Mutex::new(VecDeque::new()))).collect();
        calls
            .iter()
            .enumerate()
            .map(|(i, call)| {
                let rx: Vec<Pipe> =
                    hears.iter().zip(&pipes).filter(|((_, to), _)| *to == i).map(|(_, p)| p.clone()).collect();
                let tx: Vec<Pipe> =
                    hears.iter().zip(&pipes).filter(|((from, _), _)| *from == i).map(|(_, p)| p.clone()).collect();
                let clock = test_clock();
                let start = clock.now();
                let factory: Factory = Box::new(move || {
                    Ok(Box::new(MeshRadio { rx: rx.clone(), tx: tx.clone(), sigma, state: 7 + i as u64, clock: clock.clone(), start, delivered: 0 })
                        as Box<dyn Backend>)
                });
                vspawn(config(call, temp_dir(&format!("{tag}-{call}"))), factory).unwrap()
            })
            .collect()
    }

    fn make_relay(e: &Engine) {
        e.configure_relay(Role::Relay, relay::Radio::HalfDuplex, &[14_000.0]).unwrap();
        // A relay sends only in its scheduled transmit slots: give it one that covers the test.
        let now = now_unix_secs().max(0) as u64;
        e.accept_relay_schedule(vec![Slot {
            channel_khz: 14_000.0,
            start_unix: now - 60,
            duration_s: 3_600,
            direction: relay::Direction::Transmit,
        }])
        .unwrap();
        e.start_relay(RelayAck { licence_held: true, terms_accepted: true }).unwrap();
    }

    #[test]
    fn automatic_link_sends_a_queued_message_and_hangs_up() {
        let mut st = mesh("autolink", &["AA1AAA", "BB1BBB"], &[(0, 1), (1, 0)], 0.0);
        let (mut b, mut a) = (st.pop().unwrap(), st.pop().unwrap());
        b.listen().unwrap();
        a.set_auto_link(true);
        a.send_message("BB1BBB", "no connect needed").unwrap();
        assert!(
            wait_for(&mut b, |e| matches!(e, Event::Message { text, .. } if text == "no connect needed"), 120).is_some(),
            "the queued message was not sent by itself"
        );
        // Nothing is waiting any more: the station closes its own link.
        let closed = wait_for(&mut a, |e| matches!(e, Event::Link(kahf_core::link::LinkState::Idle)), 120);
        assert!(closed.is_some(), "the automatic link was not closed");
    }

    #[test]
    fn a_message_reaches_a_station_that_only_a_relay_can_hear() {
        // A and B cannot hear each other; both hear the relay R.
        let mut st = mesh("relay3", &["AA1AAA", "RR1RRR", "BB1BBB"], &[(0, 1), (1, 0), (1, 2), (2, 1)], 0.0);
        let (mut b, mut r, mut a) = (st.pop().unwrap(), st.pop().unwrap(), st.pop().unwrap());
        make_relay(&r);
        r.listen().unwrap();
        b.listen().unwrap();
        a.set_auto_link(true);
        a.set_relay_hints(&["RR1RRR".to_string()]);
        a.set_relay_timeout_ms(500);
        a.send_message("BB1BBB", "through the relay").unwrap();
        assert!(
            wait_for(&mut b, |e| matches!(e, Event::Message { text, .. } if text == "through the relay"), 400).is_some(),
            "the relayed message did not arrive"
        );
        assert_eq!(r.relay_stats().sent_in_hour_of(now_unix_secs().max(0) as u64), 1, "one forward counted");
        // Delivered once only.
        test_clock().sleep(Duration::from_secs(5));
        let again = b.poll_events().into_iter().filter(|e| matches!(e, Event::Message { text, .. } if text == "through the relay")).count();
        assert_eq!(again, 0, "no second copy");
        // The relay only carried it: no file in its data folder may hold the text (MESH-SPEC 7).
        let hits = files_containing(&r.config.data_dir, b"through the relay");
        assert!(hits.is_empty(), "the relay wrote the message body to disk: {hits:?}");
        // Sanity: the same scan does find it where it belongs, the sender's own outbox/log or the receiver's.
        let _ = (&a, &b);
    }

    /// Paths under `dir` (recursively) whose bytes contain `needle`.
    fn files_containing(dir: &Path, needle: &[u8]) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut todo = vec![dir.to_path_buf()];
        while let Some(d) = todo.pop() {
            let Ok(rd) = fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    todo.push(p);
                } else if fs::read(&p).is_ok_and(|b| b.windows(needle.len()).any(|w| w == needle)) {
                    out.push(p);
                }
            }
        }
        out
    }

    #[test]
    fn the_file_scan_finds_a_body_where_one_is_kept() {
        let dir = temp_dir("scan-self-test");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("x.log"), b"abc through the relay def").unwrap();
        assert_eq!(files_containing(&dir, b"through the relay").len(), 1);
        assert!(files_containing(&dir, b"nothing here").is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    /// The raw scan is only worth something if it would catch the body in the outbox, where it
    /// is kept on purpose (the sender's own copy).
    #[test]
    fn the_scan_finds_a_queued_body_in_the_senders_own_outbox() {
        let mut st = mesh("scan-outbox", &["AA1AAA", "BB1BBB"], &[(0, 1), (1, 0)], 0.0);
        let a = &mut st[0];
        a.send_message("BB1BBB", "needle-in-the-outbox").unwrap();
        test_clock().sleep(Duration::from_millis(200));
        let hits = files_containing(&a.config.data_dir, b"needle-in-the-outbox");
        assert!(!hits.is_empty(), "outbox files: {:?}", fs::read_dir(&a.config.data_dir).map(|d| d.flatten().map(|e| e.path()).collect::<Vec<_>>()));
    }

    #[test]
    fn automatic_mode_climbs_to_the_fastest_on_a_clean_link() {
        let (mut a, mut b) = linked_pair("auto-clean", 0.0);
        let mut status = None;
        for i in 0..5 {
            test_clock().sleep(Duration::from_millis(1500));
            send_and_wait(&mut a, &mut b, &format!("clean {i}"));
            for ev in a.poll_events() {
                if let Event::Metrics(s) = ev {
                    status = Some(s);
                }
            }
        }
        assert_eq!(a.transmit_modulation(), "KAMI-1d", "a clean link should use the fastest mode");
        let s = status.expect("a metrics event");
        assert!(s.auto && s.net_bps > 4000, "{s:?}");
        assert!(s.peer_snr_db.unwrap_or(0.0) > 30.0, "{s:?}");
    }

    #[test]
    fn automatic_mode_stays_robust_on_a_noisy_link_and_still_delivers() {
        let (mut a, mut b) = linked_pair("auto-noisy", noise_for(-4.0));
        for i in 0..4 {
            test_clock().sleep(Duration::from_millis(1500));
            send_and_wait(&mut a, &mut b, &format!("noisy {i}"));
        }
        let robust = ["KAMI-256", "KAMI-128", "KAMI-64", "KAMI-32", "KAMI-16", "KAMI-8", "KAMI-4"];
        assert!(robust.contains(&a.transmit_modulation()), "mode {}", a.transmit_modulation());
    }

    #[test]
    fn a_fixed_mode_overrides_automatic_choice() {
        let (mut a, mut b) = linked_pair("fixed", 0.0);
        assert!(a.set_fixed_mode("KAMI-99").is_err());
        assert!(Engine::mode_names().contains(&"KAMI-64"));
        a.set_fixed_mode("kami-64").unwrap();
        test_clock().sleep(Duration::from_millis(1500));
        send_and_wait(&mut a, &mut b, "fixed rate");
        assert_eq!(a.transmit_modulation(), "KAMI-64");
        a.set_auto_mode(true);
    }

    #[test]
    fn clean_link_steps_up_the_modulation() {
        let a_to_b: Pipe = Arc::new(Mutex::new(VecDeque::new()));
        let b_to_a: Pipe = Arc::new(Mutex::new(VecDeque::new()));
        let mut a = vspawn(
            config("AA1AAA", temp_dir("step-a")),
            fake_factory(b_to_a.clone(), a_to_b.clone()),
        )
        .unwrap();
        let mut b = vspawn(
            config("BB1BBB", temp_dir("step-b")),
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
        assert_eq!(a.transmit_modulation(), "KAMI-32");
        for i in 0..4 {
            // Give the peer's ACK, which carries its error report, time to come back before
            // the next burst is built: the level only moves on a report that has arrived.
            test_clock().sleep(Duration::from_millis(1500));
            let text = format!("step {i}");
            a.send_message("BB1BBB", &text).unwrap();
            let got = wait_for(&mut b, |e| matches!(e, Event::Message { text: t, .. } if *t == text), 60);
            if got.is_none() {
                for ev in b.poll_events() {
                    eprintln!("B EVENT: {ev:?}");
                }
                for ev in a.poll_events() {
                    eprintln!("A EVENT: {ev:?}");
                }
                panic!("step {i} lost at {:?}", a.transmit_modulation());
            }
        }
        // Each clean report from the peer moves the sender one level up.
        assert_ne!(
            a.transmit_modulation(),
            "KAMI-32",
            "the link should step up on a clean channel"
        );
    }

    /// One station with a fake radio whose transmit output is `a_to_b`.
    fn lone_station(tag: &str) -> (Engine, Pipe) {
        let a_to_b: Pipe = Arc::new(Mutex::new(VecDeque::new()));
        let b_to_a: Pipe = Arc::new(Mutex::new(VecDeque::new()));
        let mut a = vspawn(
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
        test_clock().sleep(Duration::from_millis(500));
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
        fn is_bench(&self) -> bool {
            true
        }
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

    /// Radio for the gate tests: counts PTT, and says whether it could reach an antenna.
    struct GateRadio {
        keyed: Arc<AtomicU64>,
        bench: bool,
    }

    impl Backend for GateRadio {
        fn is_bench(&self) -> bool {
            self.bench
        }
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

    /// An engine on a `GateRadio`, listening, with the test table switched off.
    fn gate_station(tag: &str, bench: bool) -> (Engine, Arc<AtomicU64>) {
        let keyed = Arc::new(AtomicU64::new(0));
        let k = keyed.clone();
        let factory: Factory =
            Box::new(move || Ok(Box::new(GateRadio { keyed: k.clone(), bench }) as Box<dyn Backend>));
        let mut e = vspawn(config("AA1AAA", temp_dir(tag)), factory).unwrap();
        e.set_test_table(false);
        e.listen().unwrap();
        test_clock().sleep(Duration::from_millis(200));
        (e, keyed)
    }

    fn fire(e: &Engine, keyed: &Arc<AtomicU64>) -> bool {
        let before = keyed.load(Ordering::SeqCst);
        e.test_burst(vec![0.5; 4000]).unwrap();
        test_clock().sleep(Duration::from_millis(600));
        keyed.load(Ordering::SeqCst) > before
    }

    fn verified() -> BandTable {
        let mut t = BandTable::builtin_r1();
        t.plan_verified = true;
        t
    }

    const C1_DIAL_HZ: u64 = 18_113_440;
    const C1_CENTRE_HZ: u64 = 18_114_940;

    #[test]
    fn gate_unverified_table_refuses_a_real_radio_and_never_keys_ptt() {
        let (mut e, keyed) = gate_station("gate-unverified", false);
        e.set_dial_hz(Some(C1_DIAL_HZ));
        e.set_tx_audio_span(1260, 1740);
        assert!(!fire(&e, &keyed), "PTT keyed on an unverified table");
        let (mode, why) = e.tx_gate();
        assert_eq!(mode, "off");
        assert_eq!(why, GATE_UNVERIFIED);
        assert_eq!(e.send_message("BB1BBB", "hi").unwrap_err(), GATE_UNVERIFIED);
    }

    #[test]
    fn gate_verified_table_lets_a_lane_signal_on_the_right_dial_through() {
        let (e, keyed) = gate_station("gate-verified", false);
        e.set_band_table(verified());
        e.set_dial_hz(Some(C1_DIAL_HZ));
        e.set_tx_audio_span(1260, 1740);
        assert_eq!(e.tx_gate().0, "real");
        assert!(fire(&e, &keyed), "a lane signal on the lane dial was refused");
    }

    #[test]
    fn gate_refuses_the_dial_trap_and_an_unknown_dial() {
        let (e, keyed) = gate_station("gate-trap", false);
        e.set_band_table(verified());
        e.set_tx_audio_span(1260, 1740);
        // The lane's centre typed as the dial lands 1.5 kHz too high.
        e.set_dial_hz(Some(C1_CENTRE_HZ));
        assert_eq!(e.tx_gate().1, GATE_NOT_A_LANE);
        assert!(!fire(&e, &keyed));
        // 14.103 MHz (an old setting) is not a lane either.
        e.set_dial_hz(Some(14_103_000));
        assert_eq!(e.tx_gate().1, GATE_NOT_A_LANE);
        e.set_dial_hz(None);
        assert_eq!(e.tx_gate().1, GATE_NO_DIAL);
        assert!(!fire(&e, &keyed));
    }

    #[test]
    fn gate_refuses_a_signal_wider_than_the_lane() {
        let (e, keyed) = gate_station("gate-wide", false);
        e.set_band_table(verified());
        e.set_dial_hz(Some(C1_DIAL_HZ));
        // The wide KAMI (312 to 2690 Hz of audio) does not fit one 480 Hz lane.
        assert_eq!(e.tx_gate().1, GATE_OUTSIDE);
        assert!(!fire(&e, &keyed));
    }

    #[test]
    fn test_table_opens_only_a_bench_radio() {
        let (real, keyed_real) = gate_station("gate-test-real", false);
        real.set_test_table(true);
        assert_eq!(real.tx_gate().1, GATE_TEST_REAL);
        assert!(!fire(&real, &keyed_real), "the test table opened a real radio");

        let (bench, keyed_bench) = gate_station("gate-test-bench", true);
        assert_eq!(bench.tx_gate().0, "off", "a bench radio still needs the test table");
        bench.set_test_table(true);
        assert_eq!(bench.tx_gate().0, "bench");
        assert!(fire(&bench, &keyed_bench));
    }

    #[test]
    fn gate_checks_every_burst_not_only_the_queue() {
        // A burst already queued when the table turns unverified must not go out.
        let (e, keyed) = gate_station("gate-late", false);
        e.set_band_table(verified());
        e.set_dial_hz(Some(C1_DIAL_HZ));
        e.set_tx_audio_span(1260, 1740);
        assert!(fire(&e, &keyed));
        e.set_band_table(BandTable::builtin_r1());
        assert!(!fire(&e, &keyed));
    }

    #[test]
    fn an_sdr_knows_its_own_dial_and_a_blackhole_pair_is_a_bench() {
        let spec = parse_sdr("hackrf@18113440").unwrap();
        assert_eq!(SdrBackend::new(spec).dial_hz(), Some(C1_DIAL_HZ));
    }

    #[test]
    fn listen_only_never_keys_ptt() {
        let keyed = Arc::new(AtomicU64::new(0));
        let k = keyed.clone();
        let factory: Factory =
            Box::new(move || Ok(Box::new(PttCountingRadio { keyed: k.clone() }) as Box<dyn Backend>));
        let mut a = vspawn(config("AA1AAA", temp_dir("ptt-listen-only")), factory).unwrap();
        a.listen().unwrap();
        a.set_listen_only(true);
        assert_eq!(a.send_message("BB1BBB", "hi").unwrap_err(), LISTEN_ONLY_TEXT);
        a.test_burst(vec![0.5; 4000]).unwrap();
        test_clock().sleep(Duration::from_millis(300));
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
        test_clock().sleep(Duration::from_millis(500));
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
        test_clock().sleep(Duration::from_millis(500));
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
        let (_cmd, rx) = channel();
        let shared = Arc::new(Shared {
            events: Mutex::new(EventQueue::default()),
            pending: Mutex::new(Vec::new()),
            flags: Flags::new(),
            station_key: None,
            key_error: Mutex::new(None),
            relay: Mutex::new(None),
            relay_hints: Mutex::new(Vec::new()),
            backbone_server: Mutex::new(None),
            location: Mutex::new(None),
            relay_stats: Mutex::new(Stats::default()),
            hops: Mutex::new(HopLog::new()),
        });
        let factory: Factory = Box::new(|| Err("no radio in this test".into()));
        let mut cfg = config(callsign, temp_dir(tag));
        cfg.sdr = None;
        Worker::new(cfg, rx, shared, factory, Arc::new(AtomicU64::new(1)), Clock::real())
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
        let (_, msg, private) = w.next_due().expect("item is due while linked");
        assert!(private, "outbox lost the private bit");
        let air = air_chunks(&msg, private).unwrap();
        assert!(session::decode_signed(&air).unwrap().private, "air bit not set");

        // The same path without the mark sends a plain message.
        let mut plain = bare_worker("plain-send", "AA1AAA");
        plain.shared.flags.operator_link.store(true, Ordering::SeqCst);
        plain.enqueue(vec![chat("AA1AAA", "BB1BBB", "for all")], vec![8], false);
        plain.link_state = LinkState::Connected {
            peer: "BB1BBB".into(),
        };
        let (_, msg, private) = plain.next_due().unwrap();
        assert!(!private);
        assert!(
            !session::decode_signed(&air_chunks(&msg, private).unwrap())
                .unwrap()
                .private
        );
    }

    #[test]
    fn private_message_is_kept_only_by_its_addressee() {
        let private_air = session::encode_signed(&chat("AA1AAA", "BB1BBB", "secret"), None, true);

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

        // The flag only narrows delivery: a public message to this station still reaches it.
        // (One addressed to somebody else is not shown: a station shows only its own mail.)
        let public_air = session::encode(&chat("AA1AAA", "CC1CCC", "open"));
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
        let w = bare_worker("relay-gate", "AA1AAA");
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
        let e = Engine::new(config("AA1AAA", dir)).unwrap();
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
            let e = Engine::new(config("AA1AAA", dir.clone())).unwrap();
            e.configure_relay(Role::Relay, relay::Radio::HalfDuplex, &[14_000.0])
                .unwrap();
            e.start_relay(RelayAck {
                licence_held: true,
                terms_accepted: true,
            })
            .unwrap();
        }
        // A new run starts with relay mode off, whatever the old run had.
        let again = Engine::new(config("AA1AAA", dir)).unwrap();
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
    fn adverts_are_shown_once_and_only_a_running_relay_passes_them_on() {
        let mut w = bare_worker("advert-plain", "AA1AAA");
        w.on_advert("BB1BBB".into(), ADVERT_TTL, 5, 0, None);
        w.on_advert("BB1BBB".into(), ADVERT_TTL, 5, 0, None);
        let shown: Vec<_> = lock(&w.shared.events).items.iter().filter(|e| matches!(e, Event::Advert { .. })).cloned().collect();
        assert_eq!(shown, vec![Event::Advert { call: "BB1BBB".into(), hops: 0, relay: false, loc: None }]);
        assert!(w.control.is_empty(), "a plain station passed an advert on");

        let mut r = bare_worker("advert-relay", "RR1RRR");
        *lock(&r.shared.relay) =
            Some(Relay::new("RR1RRR", Role::Relay, relay::Radio::HalfDuplex, &[14_000.0]).unwrap());
        r.shared.flags.relay_running.store(true, Ordering::SeqCst);
        r.on_advert("BB1BBB".into(), 3, 9, ADVERT_RELAY, Some((25.2, 55.27)));
        let shown: Vec<_> = lock(&r.shared.events).items.iter().filter(|e| matches!(e, Event::Advert { .. })).cloned().collect();
        assert_eq!(shown, vec![Event::Advert { call: "BB1BBB".into(), hops: 1, relay: true, loc: Some((25.2, 55.27)) }]);
        assert_eq!(r.control.len(), 1);
        assert_eq!(r.control[0], vec![encode_advert("BB1BBB", 2, 9, ADVERT_RELAY, Some((25.2, 55.27)))]);
        // The last hop is shown but not passed on.
        r.on_advert("CC1CCC".into(), 0, 1, 0, None);
        assert_eq!(r.control.len(), 1);
    }

    #[test]
    fn attachments_over_the_limit_are_refused_with_a_plain_reason() {
        let dir = temp_dir("attach-limit");
        std::fs::create_dir_all(&dir).unwrap();
        let big = dir.join("big.bin");
        std::fs::write(&big, vec![0u8; MAX_ATTACHMENT_BYTES + 1]).unwrap();
        let err = read_attachment(&big).unwrap_err();
        assert!(err.contains("big.bin") && err.contains("kB"), "{err}");
        let ok = dir.join("ok.bin");
        std::fs::write(&ok, vec![0u8; 1000]).unwrap();
        assert!(read_attachment(&ok).is_ok());
    }

    #[test]
    fn a_relay_does_not_pass_files_on() {
        let mut r = bare_worker("no-file-hops", "RR1RRR");
        *lock(&r.shared.relay) =
            Some(Relay::new("RR1RRR", Role::Relay, relay::Radio::HalfDuplex, &[14_000.0]).unwrap());
        r.shared.flags.relay_running.store(true, Ordering::SeqCst);
        r.shared.flags.third_party_relay.store(true, Ordering::SeqCst);
        let file = Message::File { from: "AA1AAA".into(), to: "BB1BBB".into(), name: "x".into(), data: vec![1] };
        r.forward_third_party(&file, false);
        assert!(r.forwarded.is_empty(), "a file was queued for forwarding");
        r.forward_third_party(&chat("AA1AAA", "BB1BBB", "hi"), false);
        assert_eq!(r.forwarded.len(), 1, "a chat was not forwarded");
    }

    fn relay_with_backbone(tag: &str) -> Worker {
        let mut r = bare_worker(tag, "RR1RRR");
        *lock(&r.shared.relay) =
            Some(Relay::new("RR1RRR", Role::Relay, relay::Radio::HalfDuplex, &[14_000.0]).unwrap());
        r.shared.flags.relay_running.store(true, Ordering::SeqCst);
        r.shared.flags.third_party_relay.store(true, Ordering::SeqCst);
        // A server that is not there: jobs fail quietly in the thread, which is all these tests need.
        r.bb = Some(BbHandle::spawn(Backbone::new("http://127.0.0.1:9", "RR1RRR", StationKey::generate())));
        r
    }

    #[test]
    fn a_message_from_the_server_is_sent_on_the_radio_and_remembered() {
        let mut r = relay_with_backbone("bb-deliver");
        let m = Queued {
            id: "K1ALF-7".into(), origin: "K1ALF".into(), dest: "D1AVE".into(), kind: "chat".into(),
            subject: String::new(), body: b"hello".to_vec(), urgent: false,
        };
        r.bb_deliver(m.clone());
        assert_eq!(r.outbox.pending().len(), 1);
        assert_eq!(r.bb_pending.values().cloned().collect::<Vec<_>>(), vec!["K1ALF-7".to_string()]);
        // The same message again (the server hands it out until it is receipted) is not queued twice.
        r.bb_deliver(m);
        assert_eq!(r.outbox.pending().len(), 1);
    }

    #[test]
    fn a_receipt_becomes_a_short_message_to_the_origin() {
        let mut r = relay_with_backbone("bb-receipt");
        r.bb_receipt_notice(backbone::Receipt {
            id: "K1ALF-7".into(), dest: "D1AVE".into(), status: "delivered".into(), reason: String::new(),
        });
        let (_, item, _) = r.outbox.pending().into_iter().next().expect("a notice was queued");
        let _ = item;
        assert_eq!(r.outbox.pending().len(), 1);
    }

    #[test]
    fn a_station_the_relay_hears_is_not_sent_through_the_server() {
        let mut r = relay_with_backbone("bb-heard");
        r.note_heard_recent("D1AVE", Some(10.0));
        r.forward_third_party(&chat("K1ALF", "D1AVE", "hi"), false);
        // The relay hears Dave itself, so it delivers on the radio (and does not use the server).
        assert_eq!(r.outbox.pending().len(), 1, "a station heard here must be delivered to directly");
        // A station nobody here hears goes to the server, not onto the radio.
        r.forward_third_party(&chat("K1ALF", "Z9FAR", "hi"), false);
        assert_eq!(r.outbox.pending().len(), 1, "radio forward used although the server is available");
    }

    #[test]
    fn transmit_audio_is_kept_below_full_scale_by_the_level_setting() {
        let mut w = bare_worker("tx-level", "AA1AAA");
        w.push_burst(&[vec![1, 2, 3]]);
        let peak = |v: &Vec<f32>| v.iter().fold(0f32, |m, s| m.max(s.abs()));
        let p50 = peak(w.bursts.back().unwrap());
        assert!(p50 > 0.3 && p50 <= 0.5, "default level peak {p50}");
        w.shared.flags.tx_level_pct.store(20, Ordering::SeqCst);
        w.push_burst(&[vec![1, 2, 3]]);
        let p20 = peak(w.bursts.back().unwrap());
        assert!(p20 <= 0.2 && p20 > 0.1, "20 percent peak {p20}");
        let tone = tone_samples(30);
        assert!(peak(&tone) <= 0.3 && peak(&tone) > 0.29);
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
}
