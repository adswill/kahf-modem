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
use kahf_core::frame;
use kahf_core::io::{IqSink, IqSource, RigControl, RigctldClient};
use kahf_core::ldpc;
use kahf_core::link::{Action, Link, LinkState};
use kahf_core::outbox::{OutItem, Outbox, Priority};
use kahf_core::phy::{self, Modulation};
use kahf_core::select::{self, Refusal};
use kahf_core::sense::Sense;
use kahf_core::session::{self, Message};
use kahf_core::spectrum::Analyser;
use kahf_log::contacts::{self, Contact};
use kahf_log::{valid_call, Qso};
use kahf_sdr::hackrf::{HackrfRx, HackrfTx};

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

/// HackRF sample rate (2.4 Msps = 300 x 8 kHz). Audio is real, sent as I with Q = 0.
const SDR_RATE_HZ: u32 = 2_400_000;
const SDR_OVERSAMPLE: usize = (SDR_RATE_HZ / RATE) as usize;
/// Half-length of the interpolation filter, in audio samples.
const SDR_HALF: usize = 8;
/// Used when the sdr setting has no "@frequency" part: 14.100 MHz.
const DEFAULT_SDR_FREQ_HZ: u64 = 14_100_000;
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
}

/// Settings the handle sets and the worker reads. All off by default.
struct Flags {
    listen_only: AtomicBool,
    update_locked: AtomicBool,
    internet_relay: AtomicBool,
    relay_timeout_ms: AtomicU64,
}

impl Flags {
    fn new() -> Self {
        Flags {
            listen_only: AtomicBool::new(false),
            update_locked: AtomicBool::new(false),
            internet_relay: AtomicBool::new(false),
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
        self.send_cmd(Cmd::Connect { peer })
    }

    pub fn disconnect(&mut self) -> Result<(), String> {
        self.send_cmd(Cmd::Disconnect)
    }

    /// Listen only (plan 0): while on, no transmit path runs. Receive keeps working.
    pub fn set_listen_only(&self, on: bool) {
        self.shared.flags.listen_only.store(on, Ordering::SeqCst);
    }

    /// Set while a required update is pending (plan 0 and 8.5). While set, no transmit path runs.
    pub fn set_update_locked(&self, on: bool) {
        self.shared.flags.update_locked.store(on, Ordering::SeqCst);
    }

    /// Internet relay (plan 6.1). Off by default; when off, no traffic leaves over the internet.
    pub fn set_internet_relay(&self, on: bool) {
        self.shared.flags.internet_relay.store(on, Ordering::SeqCst);
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
        let mut msgs = vec![Message::Mail {
            from: from.clone(),
            to: to.clone(),
            subject: subject.to_string(),
            body: body.to_string(),
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
        let shared = Arc::new(Shared {
            events: Mutex::new(EventQueue::default()),
            pending: Mutex::new(Vec::new()),
            flags: Flags::new(),
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

/// Sound card in and out, with optional rigctld PTT. No PTT unless rigctld is set (VOX).
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

/// HackRF over libhackrf. Real audio goes out as I with Q = 0, after
/// interpolation to 2.4 Msps. Receive takes I and decimates to 8 kHz.
/// The HackRF cannot receive and transmit at once, so RX is closed while sending.
struct SdrBackend {
    spec: SdrSpec,
    rx: Option<HackrfRx>,
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
}

impl Backend for SdrBackend {
    fn read_audio(&mut self, out: &mut Vec<f32>) -> Result<(), String> {
        if self.rx.is_none() {
            let rx = HackrfRx::open(
                self.spec.serial.as_deref(),
                self.spec.freq_hz,
                SDR_RATE_HZ as f64,
                SDR_RX_LNA_DB,
                SDR_RX_VGA_DB,
                false,
            )?;
            self.rx = Some(rx);
            self.rx_resampler = Resampler::new(SDR_RATE_HZ, RATE);
        }
        let Some(rx) = self.rx.as_mut() else {
            return Ok(());
        };
        let n = rx.read_iq(&mut self.iq)?;
        let mono: Vec<f32> = self.iq[..n].iter().map(|s| s.0).collect();
        self.rx_resampler.process(&mono, out);
        Ok(())
    }

    fn tx_start(&mut self) -> Result<(), String> {
        self.rx = None;
        let tx = HackrfTx::open(
            self.spec.serial.as_deref(),
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
    serial: Option<String>,
    freq_hz: u64,
}

/// Parses "hackrf[:serial][@freq_hz]", for example "hackrf@14074000".
fn parse_sdr(s: &str) -> Result<SdrSpec, String> {
    let (dev, freq) = match s.split_once('@') {
        Some((d, f)) => (d, Some(f)),
        None => (s, None),
    };
    let (kind, serial) = match dev.split_once(':') {
        Some((k, sn)) => (k, Some(sn)),
        None => (dev, None),
    };
    if kind.trim() != "hackrf" {
        return Err(format!(
            "unsupported SDR {kind:?}: only \"hackrf[:serial][@freq_hz]\" is wired up"
        ));
    }
    let freq_hz = match freq {
        Some(f) => f
            .trim()
            .parse::<u64>()
            .ok()
            .filter(|&hz| hz > 0)
            .ok_or_else(|| format!("bad SDR frequency {f:?}"))?,
        None => DEFAULT_SDR_FREQ_HZ,
    };
    let serial = serial
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Ok(SdrSpec { serial, freq_hz })
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

/// IQ samples for audio input samples m0..m1 (taps reach outside the range).
fn sdr_iq_range(x: &[f32], m0: usize, m1: usize) -> Vec<(f32, f32)> {
    let kernel = sdr_kernel();
    let mut out = Vec::with_capacity((m1 - m0) * SDR_OVERSAMPLE);
    for m in m0..m1 {
        for taps in kernel {
            let mut acc = 0f32;
            for (j, &t) in taps.iter().enumerate() {
                let idx = m as isize + j as isize - (SDR_HALF as isize - 1);
                if let Some(&v) = usize::try_from(idx).ok().and_then(|i| x.get(i)) {
                    acc += t * v;
                }
            }
            out.push((acc, 0.0));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Frame layer helpers (PHY + LDPC + frame), shared by TX and RX
// ---------------------------------------------------------------------------

/// Samples of one zero-payload frame. Every frame has this length.
fn frame_template() -> &'static [f32] {
    static FRAME: OnceLock<Vec<f32>> = OnceLock::new();
    FRAME.get_or_init(|| phy::modulate_frame(&vec![0u8; ldpc::N], MODULATION))
}

fn frame_len() -> usize {
    frame_template().len()
}

fn frame_samples(payload: &[u8]) -> Result<Vec<f32>, String> {
    if payload.len() > MAX_CHUNK {
        return Err(format!(
            "frame payload of {} bytes is over {MAX_CHUNK}",
            payload.len()
        ));
    }
    let coded = ldpc::encode(&frame::build_payload(payload));
    Ok(phy::modulate_frame(&coded, MODULATION))
}

/// One transmit burst: lead-in silence, the frames with gaps, tail silence.
fn burst(frames: &[Vec<u8>]) -> Result<Vec<f32>, String> {
    let mut out = vec![0f32; LEAD_SILENCE];
    for (i, payload) in frames.iter().enumerate() {
        if i > 0 {
            out.extend(std::iter::repeat_n(0f32, GAP_SILENCE));
        }
        out.extend(frame_samples(payload)?);
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

/// Decode exactly one frame that starts at the preamble. None on a CRC or LDPC failure.
fn decode_at(samples: &[f32]) -> Option<Vec<u8>> {
    let llr = phy::demodulate_frame(samples, MODULATION, ldpc::N)?;
    let info = ldpc::decode(&llr, LDPC_ITER)?;
    frame::parse_payload(&info)
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
    /// Failed channel checks for the next transmit (plan 2.3).
    access_failures: u32,
    /// No transmit starts before this time after a busy channel asked us to wait.
    access_retry_at: Option<Instant>,
    /// Our callsign. Private messages are kept only when addressed to it.
    callsign: String,
    backend: Option<Box<dyn Backend>>,
    link: Link,
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
            access_failures: 0,
            access_retry_at: None,
            callsign: config.callsign.clone(),
            backend: None,
            link: Link::new(&config.callsign),
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
        };
        w.publish_pending();
        w
    }

    /// One pass per loop: link timers, commands, then at most one unit of work.
    /// Transmit runs in small steps here, so commands and receive keep moving.
    fn run(mut self) {
        self.log("engine started");
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
            if let Some((id, msg, private)) = self.next_due() {
                if self.clear_channel(id, &msg) {
                    self.send_item(id, msg, private);
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
        for (oid, dest) in due {
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
        if self.access_retry_at.is_some_and(|t| Instant::now() < t) {
            return None;
        }
        let peer = match &self.link_state {
            LinkState::Connected { peer } => peer.clone(),
            _ => return None,
        };
        let now = self.now_ms();
        let (id, item) = self.outbox.next(now)?;
        match decode_item(&item.payload) {
            Some((msg, private)) if dest_of(&msg).eq_ignore_ascii_case(&peer) => Some((id, msg, private)),
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
        match self.audible_offset_hz(channel_hz) {
            Some(off) => self
                .sense
                .channel_busy(off / 1000.0, select::DEFAULT_SPACING_HZ / 1000.0),
            None => false,
        }
    }

    /// Plan 2.3: picks a free channel before a transmit. Urgent messages try the Emergency
    /// pool first, others the Open pool. Returns true when the transmit may go now. A busy
    /// plan holds the item for the returned seconds; the last try gives up and the item stays
    /// in the outbox for a later retry.
    fn clear_channel(&mut self, id: u64, msg: &Message) -> bool {
        matches!(
            self.pick_channel(Some(id), priority_of(msg) == Priority::Urgent),
            Pick::Go
        )
    }

    /// The channel check shared by data and control bursts. `id` is the outbox item, if any;
    /// a control burst has none and is dropped on the last try.
    fn pick_channel(&mut self, id: Option<u64>, urgent: bool) -> Pick {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
            ^ id.unwrap_or(0);
        let pick = {
            let busy = |f: f32| self.channel_busy(f);
            if urgent {
                select::choose_urgent(&self.plan, &busy, seed, self.access_failures)
            } else {
                select::choose(&self.plan, select::Pool::Open, &busy, seed, self.access_failures)
            }
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
        self.outbox.done(id);
        self.persist_outbox();
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
    fn tick_link(&mut self) {
        let now_ms = self.started.elapsed().as_millis() as u64;
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
        match burst(frames) {
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
        let flen = frame_len();
        loop {
            let Some(start) = find_preamble(&self.rx_buf) else {
                let keep = self.rx_buf.len().saturating_sub(PREAMBLE_LEN);
                self.rx_buf.drain(..keep);
                return;
            };
            if start + flen > self.rx_buf.len() {
                self.rx_buf.drain(..start);
                return;
            }
            let decoded = decode_at(&self.rx_buf[start..start + flen]);
            self.rx_buf.drain(..start + flen);
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
        match guard("session decode", || session::decode_signed(&self.rx_chunks)) {
            Ok(Some(signed)) => {
                self.rx_chunks.clear();
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
                let mut text = format!("From: {from}\nTo: {to}\nSubject: {subject}\n\n{body}\n");
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
        c.sdr = Some("rtlsdr".into());
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
    fn parses_sdr_settings() {
        let s = parse_sdr("hackrf:0000abc@14074000").unwrap();
        assert_eq!(
            s,
            SdrSpec {
                serial: Some("0000abc".into()),
                freq_hz: 14_074_000
            }
        );
        assert_eq!(parse_sdr("hackrf").unwrap().freq_hz, DEFAULT_SDR_FREQ_HZ);
        assert!(parse_sdr("hackrf@0").is_err());
        assert!(parse_sdr("airspy").is_err());
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
        air.extend(burst(&[payload.clone()]).unwrap());
        air.extend(vec![0f32; 500]);
        let start = find_preamble(&air).expect("preamble found");
        let expected = 1234 + LEAD_SILENCE;
        assert!(
            start.abs_diff(expected) <= 1,
            "start {start}, expected {expected}"
        );
        let got = decode_at(&air[start..start + frame_len()]).expect("frame decodes");
        assert_eq!(got, payload);
    }

    #[test]
    fn back_to_back_frames_decode_in_order() {
        let payloads: Vec<Vec<u8>> = vec![b"one".to_vec(), b"two".to_vec(), b"three".to_vec()];
        let air = burst(&payloads).unwrap();
        let flen = frame_len();
        let mut pos = 0;
        let mut got = Vec::new();
        while let Some(rel) = find_preamble(&air[pos..]) {
            let start = pos + rel;
            if start + flen > air.len() {
                break;
            }
            got.push(decode_at(&air[start..start + flen]).expect("frame decodes"));
            pos = start + flen;
        }
        assert_eq!(got, payloads);
    }

    #[test]
    fn sdr_interpolation_keeps_dc() {
        let x = vec![0.5f32; 64];
        let iq = sdr_iq_range(&x, 0, 64);
        assert_eq!(iq.len(), 64 * SDR_OVERSAMPLE);
        for &(i, q) in &iq[SDR_OVERSAMPLE * 16..SDR_OVERSAMPLE * 48] {
            assert!((i - 0.5).abs() < 1e-3 && q == 0.0);
        }
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
        });
        let factory: Factory = Box::new(|| Err("no radio in this test".into()));
        let mut cfg = config(callsign, temp_dir(tag));
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
}
