//! The application: the model plus the form state of every screen, and everything that talks to
//! the engine glue. Handlers in the views call the methods here; work that can block runs on a
//! thread and comes back through the job queue, drained by `pump` 25 times a second.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use kahf_app_core::{self as core, Shared};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::model::*;
use crate::store;
use crate::types::*;
use crate::util::{callsign, fmt};

type Job = Box<dyn FnOnce(&mut Kahf) + Send>;

/// The way to the engine glue and the queues coming back from it.
struct Bridge {
    shared: Arc<Shared>,
    events: Arc<Mutex<VecDeque<Value>>>,
    jobs: Arc<Mutex<VecDeque<Job>>>,
}

impl Bridge {
    fn new(data_dir: PathBuf) -> Self {
        let events: Arc<Mutex<VecDeque<Value>>> = Arc::default();
        let sink = events.clone();
        let shared = Arc::new(Shared::new(
            Arc::new(move |v| {
                if let Ok(mut q) = sink.lock() {
                    q.push_back(v);
                }
            }),
            data_dir,
        ));
        core::start_poll_loop(shared.clone());
        Bridge {
            shared,
            events,
            jobs: Arc::default(),
        }
    }
}

/// The answer of a test button.
#[derive(Debug, Clone, PartialEq)]
pub enum TestState {
    Running,
    Done { ok: bool, text: String },
}

/// How the radio's sound reaches the computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundKind {
    Sound,
    Sdr,
    None,
}

/// The hardware choices of the Radio screen and of setup, edited together and applied together,
/// because the radio restarts.
#[derive(Debug, Clone, PartialEq)]
pub struct Hw {
    pub kind: SoundKind,
    pub audio_in: String,
    pub audio_out: String,
    pub sdr: String,
    pub rig: RigChoice,
    pub rigctld: String,
    /// Direct control: model id, port, speed, keying.
    pub d_model: String,
    pub d_port: String,
    pub d_baud: String,
    pub d_ptt: String,
}

impl Hw {
    pub fn from_settings(s: &Settings) -> Self {
        let parts: Vec<&str> = s.rig_spec.split(':').collect();
        let direct =
            !s.rig_spec.is_empty() && !s.rig_spec.starts_with("rigctld") && !s.rig_spec.starts_with("flrig");
        Hw {
            kind: if !s.sdr.is_empty() {
                SoundKind::Sdr
            } else {
                SoundKind::Sound
            },
            audio_in: s.audio_in.clone(),
            audio_out: s.audio_out.clone(),
            sdr: s.sdr.clone(),
            rig: s.rig.clone(),
            rigctld: s.rigctld.clone(),
            d_model: if direct {
                parts[0].to_string()
            } else {
                String::new()
            },
            d_port: if direct {
                parts.get(1).unwrap_or(&"").to_string()
            } else {
                String::new()
            },
            d_baud: if direct {
                parts
                    .get(2)
                    .filter(|p| p.chars().all(|c| c.is_ascii_digit()))
                    .unwrap_or(&"")
                    .to_string()
            } else {
                String::new()
            },
            d_ptt: if direct {
                parts
                    .last()
                    .filter(|p| PTTS.iter().any(|(id, _)| id == *p))
                    .unwrap_or(&"cat")
                    .to_string()
            } else {
                "cat".into()
            },
        }
    }

    /// The text the engine opens for a directly controlled radio, or empty.
    pub fn spec(&self) -> String {
        if self.d_model.is_empty() || self.d_port.is_empty() {
            return String::new();
        }
        let baud = self.d_baud.trim().parse::<u32>().ok().filter(|b| *b > 0);
        match baud {
            Some(b) => format!("{}:{}:{b}:{}", self.d_model, self.d_port, self.d_ptt),
            None => format!("{}:{}:{}", self.d_model, self.d_port, self.d_ptt),
        }
    }

    /// Writes the choices into settings.
    pub fn apply_to(&self, s: &mut Settings) {
        s.audio_in = if self.kind == SoundKind::Sound {
            self.audio_in.clone()
        } else {
            String::new()
        };
        s.audio_out = if self.kind == SoundKind::Sound {
            self.audio_out.clone()
        } else {
            String::new()
        };
        s.sdr = if self.kind == SoundKind::Sdr {
            self.sdr.clone()
        } else {
            String::new()
        };
        s.rig = self.rig.clone();
        s.rigctld = self.rigctld.trim().to_string();
        s.rig_spec = self.spec();
    }
}

/// Keying choices for a directly controlled radio.
pub const PTTS: [(&str, &str); 4] = [
    ("cat", "Radio command (needs no wiring)"),
    ("rts", "RTS line of the serial port"),
    ("dtr", "DTR line of the serial port"),
    ("vox", "VOX (the radio keys itself from the sound)"),
];

pub const BAUDS: [u32; 6] = [4800, 9600, 19_200, 38_400, 57_600, 115_200];

/// The steps of setup.
pub const SETUP_STEPS: [&str; 5] = ["You", "Radio", "Channel", "Place", "Ready"];

/// Text fields behind the position editor.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PosText {
    pub locator: String,
    pub lat: String,
    pub lon: String,
}

impl PosText {
    pub fn from(loc: &Location) -> Self {
        PosText {
            locator: String::new(),
            lat: loc.lat.map(|v| v.to_string()).unwrap_or_default(),
            lon: loc.lon.map(|v| v.to_string()).unwrap_or_default(),
        }
    }

    pub fn lat(&self) -> Option<f64> {
        self.lat.trim().parse().ok().filter(|v: &f64| v.is_finite())
    }

    pub fn lon(&self) -> Option<f64> {
        self.lon.trim().parse().ok().filter(|v: &f64| v.is_finite())
    }
}

/// Setup works on a copy of the settings that only becomes real when the person finishes.
pub struct SetupState {
    pub step: usize,
    pub have_licence: bool,
    pub draft: Settings,
    pub hw: Hw,
    pub pos: PosText,
    pub previewing: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RelayTab {
    #[default]
    Use,
    Run,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MailView {
    #[default]
    Inbox,
    Sent,
}

/// Everything the screens remember about what the person is typing or choosing.
pub struct Ui {
    pub peer: String,
    pub draft: String,
    pub kind: MessageKind,
    pub mail_to: String,
    pub mail_subject: String,
    pub mail_body: String,
    pub mail_files: Vec<PickedFile>,
    pub mail_open: Option<String>,
    pub mail_compose: bool,
    pub mail_view: MailView,
    pub hw: Hw,
    pub devices: Vec<String>,
    pub sdrs: Vec<String>,
    pub rig_available: Option<bool>,
    pub rig_models: Vec<RigModel>,
    pub rig_ports: Vec<String>,
    pub direct_rigs: Vec<DirectRig>,
    pub rig_query: String,
    pub tests: HashMap<String, TestState>,
    pub applying: bool,
    pub setup: SetupState,
    pub pos: PosText,
    pub relay_tab: RelayTab,
    pub licence: bool,
    pub terms: bool,
    pub manual_relay: String,
    pub book_filter: String,
    pub uploads_open: bool,
    pub upload_busy: bool,
    pub update: Option<Result<UpdateCheck, String>>,
    pub server: Option<ServerCheck>,
    pub help_topic: String,
    /// The voice note playing: message id, when it started, how long it is.
    pub playing: Option<(String, Instant, f64)>,
    pub rec_start: Option<Instant>,
    pub find: String,
    pub map: crate::ui::map::MapState,
    pub tour_step: usize,
    pub support_nudge: bool,
    pub open_drop: Option<&'static str>,
    pub call_edit: String,
    pub help_query: String,
    /// The moment the support note may appear.
    pub nudge_at: Option<Instant>,
    /// The saved theme still has to be applied: the window does not exist yet while `init` runs.
    pub theme_pending: bool,
}

impl Ui {
    fn new(settings: &Settings) -> Self {
        Ui {
            peer: String::new(),
            draft: String::new(),
            kind: MessageKind::Chat,
            mail_to: String::new(),
            mail_subject: String::new(),
            mail_body: String::new(),
            mail_files: Vec::new(),
            mail_open: None,
            mail_compose: false,
            mail_view: MailView::Inbox,
            hw: Hw::from_settings(settings),
            devices: Vec::new(),
            sdrs: Vec::new(),
            rig_available: None,
            rig_models: Vec::new(),
            rig_ports: Vec::new(),
            direct_rigs: Vec::new(),
            rig_query: String::new(),
            tests: HashMap::new(),
            applying: false,
            setup: SetupState::new(settings),
            pos: PosText::from(&settings.location),
            relay_tab: RelayTab::Use,
            licence: false,
            terms: false,
            manual_relay: String::new(),
            book_filter: String::new(),
            uploads_open: false,
            upload_busy: false,
            update: None,
            server: None,
            help_topic: String::new(),
            playing: None,
            rec_start: None,
            find: String::new(),
            map: Default::default(),
            tour_step: 0,
            support_nudge: false,
            open_drop: None,
            call_edit: settings.callsign.clone(),
            help_query: String::new(),
            nudge_at: None,
            theme_pending: true,
        }
    }
}

impl SetupState {
    pub fn new(s: &Settings) -> Self {
        SetupState {
            step: 0,
            have_licence: !s.callsign.is_empty() || !s.listen_only,
            draft: s.clone(),
            hw: Hw::from_settings(s),
            pos: PosText::from(&s.location),
            previewing: false,
        }
    }
}

pub struct Kahf {
    pub m: Model,
    pub ui: Ui,
    bridge: Bridge,
    dirty_since: Option<Instant>,
    last_poll: Instant,
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn now_unix() -> i64 {
    now_ms() / 1000
}

fn parse<T: DeserializeOwned>(v: Value) -> Result<T, String> {
    serde_json::from_value(v).map_err(|e| format!("Unexpected answer from the engine: {e}"))
}

impl Kahf {
    pub fn new() -> Self {
        let data_dir = core::data_dir();
        let bridge = Bridge::new(data_dir.clone());
        let mut m = Model::default();
        m.settings = serde_json::from_value(core::load_settings(&bridge.shared)).unwrap_or_default();
        m.fix_frequency();
        m.restore(store::load(&data_dir));
        m.page = m.first_page();
        let ui = Ui::new(&m.settings);
        Kahf {
            m,
            ui,
            bridge,
            dirty_since: None,
            last_poll: Instant::now(),
        }
    }

    /// A state with nothing saved and no engine, for tests and screenshots.
    #[cfg(test)]
    pub fn for_tests(dir: PathBuf) -> Self {
        let bridge = Bridge::new(dir);
        let m = Model::default();
        let ui = Ui::new(&m.settings);
        Kahf {
            m,
            ui,
            bridge,
            dirty_since: None,
            last_poll: Instant::now(),
        }
    }

    pub fn shared(&self) -> Arc<Shared> {
        self.bridge.shared.clone()
    }

    pub fn data_dir(&self) -> PathBuf {
        self.bridge.shared.data_dir().to_path_buf()
    }

    /// Runs `work` on its own thread and, when it is done, `done` on the window's state.
    pub fn spawn<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Shared) -> T + Send + 'static,
        done: impl FnOnce(&mut Kahf, T) + Send + 'static,
    ) {
        let shared = self.bridge.shared.clone();
        let jobs = self.bridge.jobs.clone();
        std::thread::spawn(move || {
            let out = work(&shared);
            if let Ok(mut q) = jobs.lock() {
                q.push_back(Box::new(move |k: &mut Kahf| done(k, out)));
            }
        });
    }

    /// Takes in what came from the engine and from finished work. Called 25 times a second.
    /// The live spectrum, a recording and a playing note need frequent redraws; nothing else does.
    pub fn wants_fast_tick(&self) -> bool {
        self.m.page == Page::Radio
            || self.ui.rec_start.is_some()
            || self.ui.playing.is_some()
            || self.ui.setup.previewing
    }

    pub fn pump(&mut self) {
        if self.ui.theme_pending {
            self.ui.theme_pending = false;
            self.apply_theme();
        }
        if self.ui.nudge_at.is_some_and(|t| Instant::now() >= t) {
            self.ui.nudge_at = None;
            self.ui.support_nudge = true;
        }
        let events: Vec<Value> = self
            .bridge
            .events
            .lock()
            .map(|mut q| q.drain(..).collect())
            .unwrap_or_default();
        let now = now_ms();
        let mut changed = !events.is_empty();
        for ev in events {
            match serde_json::from_value::<EngineEvent>(ev) {
                Ok(e) => {
                    let persist = !matches!(
                        e,
                        EngineEvent::Spectrum { .. } | EngineEvent::Level { .. } | EngineEvent::Log { .. }
                    );
                    self.m.handle(e, now);
                    if persist {
                        self.mark_dirty();
                    }
                }
                Err(e) => log_line(&format!("ignored an engine event: {e}")),
            }
        }
        let jobs: Vec<Job> = self
            .bridge
            .jobs
            .lock()
            .map(|mut q| q.drain(..).collect())
            .unwrap_or_default();
        changed |= !jobs.is_empty();
        for job in jobs {
            job(self);
        }
        if self
            .ui
            .playing
            .as_ref()
            .is_some_and(|(_, t, secs)| t.elapsed().as_secs_f64() > secs + 0.3)
        {
            self.ui.playing = None;
        }
        if self
            .ui
            .rec_start
            .is_some_and(|t| t.elapsed() >= Duration::from_secs(30))
        {
            let to = callsign::normalise(&self.ui.peer);
            self.finish_recording(to);
        }
        for t in std::mem::take(&mut self.m.toasts) {
            show_toast(&t.text, t.kind);
        }
        if self.last_poll.elapsed() > Duration::from_secs(3) {
            self.last_poll = Instant::now();
            self.refresh_live();
        }
        if changed {
            self.flush_if_due();
        }
        if let Some(t) = self.dirty_since {
            if t.elapsed() > Duration::from_secs(1) {
                self.save_now();
            }
        }
    }

    fn mark_dirty(&mut self) {
        self.dirty_since.get_or_insert_with(Instant::now);
    }

    fn flush_if_due(&mut self) {}

    /// Writes the conversations, mail and heard stations to the data folder.
    pub fn save_now(&mut self) {
        self.dirty_since = None;
        store::save(&self.data_dir(), &self.m.saved());
    }

    pub fn toast(&mut self, text: impl Into<String>, kind: ToastKind) {
        show_toast(&text.into(), kind);
    }

    pub fn warn(&mut self, text: impl Into<String>) {
        self.toast(text, ToastKind::Warn);
    }

    pub fn go(&mut self, page: Page) {
        self.m.go(page);
        match page {
            Page::Logbook | Page::Stations => self.refresh_book(),
            Page::Relay => self.refresh_live(),
            Page::Radio => self.ui.hw = Hw::from_settings(&self.m.settings),
            Page::Setup => self.ui.setup = SetupState::new(&self.m.settings),
            _ => {}
        }
        self.mark_dirty();
    }

    // ---- startup ---------------------------------------------------------------------------

    /// Starts everything that needs the engine. Called once, after the window exists.
    pub fn init(&mut self) {
        let setup_done = self.m.settings.setup_done;
        self.load_hardware_lists();
        if setup_done {
            self.start_engine(|_, _| {});
            let hints = self.m.settings.relay_hints.clone();
            self.spawn(move |s| core::engine_set_relay_hints(s, hints), |_, _| {});
            self.refresh_relay_directory(false);
        }
        self.spawn(core::app_info, |k, v| {
            if let Ok(i) = parse::<AppInfo>(v) {
                k.m.info = i;
            }
        });
        self.refresh_book();
        self.m.ready = true;
        self.maybe_nudge();
    }

    /// Asks once in a while, 30 seconds after start, if the person would like to support KAHF.
    fn maybe_nudge(&mut self) {
        if !self.m.settings.setup_done {
            return;
        }
        let now = now_ms();
        if self.m.support.first == 0 {
            self.m.support.first = now;
            self.mark_dirty();
        }
        if self.m.support.due(now) {
            self.ui.nudge_at = Some(Instant::now() + Duration::from_secs(30));
        }
    }

    pub fn close_nudge(&mut self, never: bool) {
        self.ui.support_nudge = false;
        self.m.support.last = now_ms();
        self.m.support.off |= never;
        self.mark_dirty();
    }

    pub fn tour_to(&mut self, step: usize) {
        self.ui.tour_step = step;
        if let Some(s) = crate::ui::overlays::STOPS.get(step) {
            self.go(s.page);
        }
    }

    pub fn finish_tour(&mut self) {
        self.m.tour_open = false;
        self.set(|s| s.tour_done = true);
        self.go(Page::Chats);
    }

    pub fn apply_theme(&self) {
        alloy_ui::set_dark(self.m.settings.theme == Theme::Dark);
    }

    fn load_hardware_lists(&mut self) {
        self.spawn(
            |_| {
                (
                    core::list_audio_devices(),
                    core::detect_sdr(),
                    core::rig_direct_models(),
                )
            },
            |k, (d, s, r)| {
                k.ui.devices = d;
                k.ui.sdrs = s;
                k.ui.direct_rigs = r.into_iter().filter_map(|v| parse::<DirectRig>(v).ok()).collect();
            },
        );
        self.check_hamlib();
    }

    pub fn check_hamlib(&mut self) {
        self.spawn(
            |s| {
                let available = core::rig_available();
                let models = if available {
                    core::rig_models().unwrap_or_default()
                } else {
                    Vec::new()
                };
                let _ = s;
                (available, models, core::rig_ports())
            },
            |k, (available, models, ports)| {
                k.ui.rig_available = Some(available);
                k.ui.rig_models = models
                    .into_iter()
                    .filter_map(|m| serde_json::from_value(serde_json::to_value(m).ok()?).ok())
                    .collect();
                k.ui.rig_ports = ports;
            },
        );
    }

    // ---- settings and the radio ------------------------------------------------------------

    pub fn save_settings(&mut self) {
        let v = serde_json::to_value(&self.m.settings).unwrap_or(Value::Null);
        if let Err(e) = core::save_settings(&self.bridge.shared, v) {
            self.warn(e);
        }
        self.apply_theme();
    }

    /// Changes settings and saves them.
    pub fn set(&mut self, f: impl FnOnce(&mut Settings)) {
        f(&mut self.m.settings);
        self.save_settings();
    }

    /// Starts (or restarts) the engine. Never fails loudly: a problem becomes a sentence.
    pub fn start_engine(&mut self, then: impl FnOnce(&mut Kahf, bool) + Send + 'static) {
        let v = serde_json::to_value(&self.m.settings).unwrap_or(Value::Null);
        self.spawn(
            move |s| core::engine_start(s, v),
            move |k, r| {
                let ok = r.is_ok();
                k.m.engine_error = match r {
                    Ok(()) => String::new(),
                    Err(e) => explain_start_error(&e),
                };
                then(k, ok);
            },
        );
    }

    /// The end of setup: remember it and start the radio.
    pub fn finish_setup(&mut self) {
        let st = &self.ui.setup;
        let call = callsign::normalise(&st.draft.callsign);
        let mut s = self.m.settings.clone();
        s.callsign = if st.have_licence { call } else { String::new() };
        s.listen_only = !st.have_licence;
        st.hw.apply_to(&mut s);
        if s.sdr.is_empty() && st.hw.kind == SoundKind::Sdr {
            s.sdr = "HackRF".into();
        }
        s.freq_khz = st.draft.freq_khz;
        let loc = &st.draft.location;
        let on = loc.mode != LocationMode::Off;
        s.location = Location {
            mode: loc.mode,
            lat: if on { st.pos.lat() } else { None },
            lon: if on { st.pos.lon() } else { None },
            radius_km: loc.radius_km,
        };
        s.auto_link = st.draft.auto_link;
        s.auto_mode = st.draft.auto_mode;
        s.setup_done = true;
        self.m.settings = s;
        self.save_settings();
        self.start_engine(|k, ok| {
            k.go(Page::Chats);
            if ok {
                k.toast("All set. KAHF is listening for stations.", ToastKind::Good);
            }
            k.refresh_relay_directory(false);
        });
        self.m.tour_open = !self.m.settings.tour_done;
    }

    /// Looks around without transmitting.
    pub fn explore(&mut self) {
        self.set(|s| {
            s.listen_only = true;
            s.setup_done = true;
        });
        self.start_engine(|_, _| {});
        self.go(Page::Chats);
    }

    /// Applies the Radio screen's hardware choices and restarts the radio.
    pub fn apply_hardware(&mut self) {
        let hw = self.ui.hw.clone();
        hw.apply_to(&mut self.m.settings);
        self.save_settings();
        self.ui.applying = true;
        self.start_engine(|k, ok| {
            k.ui.applying = false;
            if ok {
                k.toast("Radio restarted with the new settings.", ToastKind::Good);
            }
        });
    }

    pub fn hardware_dirty(&self) -> bool {
        let mut applied = self.m.settings.clone();
        self.ui.hw.apply_to(&mut applied);
        applied != self.m.settings
    }

    pub fn set_listen_only(&mut self, on: bool) {
        self.set(|s| s.listen_only = on);
        self.spawn(move |s| core::engine_set_listen_only(s, on), |_, _| {});
    }

    pub fn set_frequency(&mut self, khz: f64) {
        if self.m.transmitting {
            return;
        }
        self.set(|s| s.freq_khz = khz);
        // The transmit gate checks the dial, so the engine must know it at once.
        self.spawn(move |st| core::engine_set_dial(st, khz), |_, _| {});
    }

    // ---- tests ("Test" buttons: they only listen or look, never transmit) --------------------

    pub fn run_test(
        &mut self,
        key: &str,
        work: impl FnOnce(&Shared) -> Result<String, String> + Send + 'static,
    ) {
        let key = key.to_string();
        self.ui.tests.insert(key.clone(), TestState::Running);
        self.spawn(work, move |k, r| {
            let state = match r {
                Ok(text) => TestState::Done { ok: true, text },
                Err(text) => TestState::Done { ok: false, text },
            };
            k.ui.tests.insert(key, state);
        });
    }

    pub fn test_audio_in(&mut self, key: &str, device: String) {
        self.run_test(key, move |_| core::test_audio_in(device));
    }

    pub fn test_audio_out(&mut self, key: &str, device: String) {
        self.run_test(key, move |_| core::test_audio_out(device));
    }

    /// Listens for three seconds and says whether the radio's volume is right.
    pub fn check_level(&mut self, key: &str, device: String) {
        self.run_test(key, move |_| {
            let v = core::audio_level_check(device)?;
            let r: LevelCheck = parse(v)?;
            Ok(format!("{} ({} dB)", r.advice, r.level_db))
        });
    }

    pub fn test_rig_address(&mut self, key: &str, addr: String) {
        self.run_test(key, move |_| core::test_rig(addr));
    }

    pub fn test_rig_spec(&mut self, key: &str, spec: String) {
        self.run_test(key, move |_| core::rig_test_spec(spec));
    }

    /// Starts Hamlib's radio control for the chosen radio and reads its frequency.
    pub fn test_rig_model(&mut self, key: &str, hw: RigChoice) {
        let Some(model) = hw.model else { return };
        self.run_test(key, move |s| {
            let hz = core::rig_connect(s, model, hw.port.clone(), hw.baud)?;
            Ok(format!("Connected. Your radio is on {:.6} MHz.", hz as f64 / 1e6))
        });
    }

    // ---- text, files and voice ---------------------------------------------------------------

    fn can_send(&mut self) -> bool {
        if self.m.settings.listen_only {
            self.warn("Listen-only is on, so nothing can be sent. Turn it off in Settings, under Sending.");
            return false;
        }
        true
    }

    pub fn call_station(&mut self, peer: &str) {
        let to = callsign::normalise(peer);
        if callsign::problem(&to).is_some() || to.is_empty() {
            self.warn("That does not look like a callsign.");
            return;
        }
        self.spawn(
            move |s| core::engine_connect(s, to),
            |k, r| {
                if let Err(e) = r {
                    k.warn(e);
                }
            },
        );
    }

    pub fn hang_up(&mut self) {
        self.spawn(core::engine_disconnect, |_, _| {});
    }

    pub fn send_message(&mut self, to: &str, text: &str, kind: MessageKind) {
        let peer = callsign::normalise(to);
        let body = text.trim().to_string();
        if peer.is_empty() || body.is_empty() || !self.can_send() {
            return;
        }
        if callsign::problem(&peer).is_some() {
            self.warn(format!("\"{peer}\" does not look like a callsign."));
            return;
        }
        let id = self.m.new_id(now_ms());
        self.m.add_out(Msg {
            id: id.clone(),
            peer: peer.clone(),
            dir: Dir::Out,
            text: body.clone(),
            at: now_ms(),
            kind,
            state: Some(OutState::Queued),
            attachment: None,
            read: true,
            note: None,
        });
        self.mark_dirty();
        let (uid, kind_s) = (id.clone(), kind.as_str().to_string());
        self.spawn(
            move |s| core::engine_send(s, uid, peer, body, kind_s),
            move |k, r| {
                if let Err(e) = r {
                    k.m.fail_msg(&id, e);
                }
            },
        );
    }

    /// Checks the size limit, then asks the person to confirm.
    pub fn ask_send_files(&mut self, to: &str, files: Vec<PickedFile>) {
        let ok: Vec<PickedFile> = files
            .iter()
            .filter(|f| f.size <= fmt::FILE_LIMIT_BYTES)
            .cloned()
            .collect();
        if ok.len() < files.len() {
            self.warn(format!(
                "Files over {} cannot be sent: radio is too slow for them. Zip or shrink the file, or send it another way.",
                fmt::size_words(fmt::FILE_LIMIT_BYTES)
            ));
        }
        if !ok.is_empty() {
            self.m.pending_files = Some(PendingFiles {
                to: to.to_string(),
                files: ok,
            });
        }
    }

    pub fn confirm_files(&mut self) {
        if let Some(p) = self.m.pending_files.take() {
            for f in p.files {
                self.send_file(&p.to, f);
            }
        }
    }

    pub fn send_file(&mut self, to: &str, file: PickedFile) {
        let peer = callsign::normalise(to);
        if peer.is_empty() || callsign::problem(&peer).is_some() || !self.can_send() {
            return;
        }
        let id = self.m.new_id(now_ms());
        self.m.add_out(Msg {
            id: id.clone(),
            peer: peer.clone(),
            dir: Dir::Out,
            text: String::new(),
            at: now_ms(),
            kind: MessageKind::Chat,
            state: Some(OutState::Queued),
            attachment: Some(Attachment {
                kind: AttachmentKind::File,
                name: file.name.clone(),
                size: file.size,
                path: Some(file.path.clone()),
                seconds: None,
                done: Some(0),
            }),
            read: true,
            note: None,
        });
        self.mark_dirty();
        let (uid, path) = (id.clone(), file.path);
        self.spawn(
            move |s| core::engine_send_file(s, uid, peer, path),
            move |k, r| {
                if let Err(e) = r {
                    k.m.fail_msg(&id, e);
                }
            },
        );
    }

    pub fn send_voice(&mut self, to: &str, clip: VoiceClip) {
        let peer = callsign::normalise(to);
        if peer.is_empty() || callsign::problem(&peer).is_some() || !self.can_send() {
            return;
        }
        let id = self.m.new_id(now_ms());
        self.m.add_out(Msg {
            id: id.clone(),
            peer: peer.clone(),
            dir: Dir::Out,
            text: String::new(),
            at: now_ms(),
            kind: MessageKind::Chat,
            state: Some(OutState::Queued),
            attachment: Some(Attachment {
                kind: AttachmentKind::Voice,
                name: "Voice note".into(),
                size: clip.bytes,
                path: clip.path.clone(),
                seconds: Some(clip.seconds),
                done: Some(0),
            }),
            read: true,
            note: None,
        });
        self.mark_dirty();
        let (uid, clip_id) = (id.clone(), clip.id);
        self.spawn(
            move |s| core::voice_send(s, uid, peer, clip_id),
            move |k, r| {
                if let Err(e) = r {
                    k.m.fail_msg(&id, e);
                }
            },
        );
    }

    pub fn send_mail(&mut self) -> bool {
        let peer = callsign::normalise(&self.ui.mail_to);
        if peer.is_empty() || callsign::problem(&peer).is_some() {
            self.warn("Enter the callsign of the station you are writing to.");
            return false;
        }
        if !self.can_send() {
            return false;
        }
        let id = self.m.new_id(now_ms());
        let subject = if self.ui.mail_subject.trim().is_empty() {
            "(no subject)".to_string()
        } else {
            self.ui.mail_subject.trim().to_string()
        };
        let body = self.ui.mail_body.clone();
        let files = std::mem::take(&mut self.ui.mail_files);
        self.m.mail.push(MailItem {
            id: id.clone(),
            peer: peer.clone(),
            dir: Dir::Out,
            subject: subject.clone(),
            body: body.clone(),
            at: now_ms(),
            files: files
                .iter()
                .map(|f| MailFile {
                    name: f.name.clone(),
                    size: f.size,
                    path: Some(f.path.clone()),
                })
                .collect(),
            state: Some(OutState::Queued),
            note: None,
            read: true,
        });
        self.mark_dirty();
        let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
        let uid = id.clone();
        self.spawn(
            move |s| core::engine_send_mail(s, uid, peer, subject, body, paths),
            move |k, r| {
                if let Err(e) = r {
                    k.m.update_mail(&id, |m| {
                        m.state = Some(OutState::Failed);
                        m.note = Some(e);
                    });
                }
            },
        );
        self.ui.mail_subject.clear();
        self.ui.mail_body.clear();
        true
    }

    pub fn start_recording(&mut self) {
        if self.m.recording {
            return;
        }
        self.spawn(core::voice_record, |k, r| match r {
            Ok(()) => {
                k.m.recording = true;
                k.ui.rec_start = Some(Instant::now());
            }
            Err(e) => k.warn(format!(
                "Could not record: {e}. Check the microphone in Settings, under Radio."
            )),
        });
    }

    /// Stops recording and sends the clip to `to`.
    pub fn finish_recording(&mut self, to: String) {
        self.m.recording = false;
        self.m.record_level = 0.0;
        self.ui.rec_start = None;
        self.spawn(core::voice_stop, move |k, r| match r {
            Ok(Some(v)) => match parse::<VoiceClip>(v) {
                Ok(clip) => k.send_voice(&to, clip),
                Err(e) => k.warn(e),
            },
            Ok(None) => k.toast("That was too short to send.", ToastKind::Info),
            Err(e) => k.warn(e),
        });
    }

    pub fn cancel_recording(&mut self) {
        self.m.recording = false;
        self.m.record_level = 0.0;
        self.ui.rec_start = None;
        self.spawn(core::voice_cancel, |_, _| {});
    }

    /// Plays or stops a voice note.
    pub fn toggle_play(&mut self, id: &str, path: Option<String>, seconds: f64) {
        if self.ui.playing.as_ref().is_some_and(|p| p.0 == id) {
            self.ui.playing = None;
            self.spawn(core::voice_stop_play, |_, _| {});
            return;
        }
        let Some(path) = path else {
            self.warn("This voice note is not saved on this computer, so it cannot be played.");
            return;
        };
        self.ui.playing = Some((id.to_string(), Instant::now(), seconds));
        self.spawn(
            move |s| core::voice_play(s, path),
            |k, r| {
                if let Err(e) = r {
                    k.ui.playing = None;
                    k.warn(format!("Could not play it: {e}"));
                }
            },
        );
    }

    /// Opens the system's file chooser; `done` gets the files chosen.
    pub fn pick_files(&mut self, done: impl FnOnce(&mut Kahf, Vec<PickedFile>)) {
        let picked = alloy_ui::files::open_many()
            .title("Choose files to send")
            .pick_all();
        let described = core::describe_files(&picked);
        let files: Vec<PickedFile> = described.into_iter().filter_map(|v| parse(v).ok()).collect();
        if !files.is_empty() {
            done(self, files);
        }
    }

    pub fn open_path(&mut self, path: String) {
        if let Err(e) = core::open_path(path) {
            self.warn(e);
        }
    }

    pub fn open_url(&mut self, url: &str) {
        if let Err(e) = core::open_url(url.to_string()) {
            self.warn(e);
        }
    }

    pub fn discard(&mut self, id: &str) {
        self.m.discard(id);
        self.mark_dirty();
    }

    // ---- relays ------------------------------------------------------------------------------

    pub fn set_relay(&mut self, on: bool) {
        let (licence, terms) = (self.ui.licence, self.ui.terms);
        self.spawn(
            move |s| core::engine_set_relay(s, on, licence, terms),
            move |k, r| match r {
                Err(e) => k.warn(e),
                Ok(()) => {
                    k.set(|s| s.relay = on);
                    k.refresh_live();
                    if on {
                        k.share_on_map();
                    }
                }
            },
        );
    }

    pub fn share_on_map(&mut self) {
        self.spawn(core::map_share, |k, r| match r {
            Ok(t) => k.toast(t, ToastKind::Info),
            Err(e) => k.warn(e),
        });
    }

    pub fn refresh_relay_directory(&mut self, refresh: bool) {
        self.spawn(
            move |s| core::relay_directory(s, refresh),
            move |k, r| match r {
                Ok(rows) => {
                    k.m.relays = rows.into_iter().filter_map(|v| parse(v).ok()).collect();
                    if refresh {
                        let n = k.m.relays.len();
                        k.toast(
                            format!("{n} relay{} known.", if n == 1 { "" } else { "s" }),
                            ToastKind::Info,
                        );
                    }
                }
                Err(e) => {
                    if refresh {
                        k.warn(format!(
                            "Could not reach the server: {e}. KAHF still works without it."
                        ));
                    }
                }
            },
        );
    }

    pub fn set_relay_hints(&mut self, calls: Vec<String>) {
        self.set(|s| s.relay_hints = calls.clone());
        self.spawn(
            move |s| core::engine_set_relay_hints(s, calls),
            |k, r| {
                if let Err(e) = r {
                    k.warn(e);
                }
            },
        );
    }

    /// Rereads what the engine keeps outside the window: the relay's numbers and the logbook.
    pub fn refresh_live(&mut self) {
        if !self.m.settings.setup_done {
            return;
        }
        self.spawn(core::tx_gate_status, |k, r| {
            if let Ok(v) = r {
                if let Ok(g) = parse::<TxGate>(v) {
                    k.m.tx_gate = g;
                }
            }
        });
        if self.m.relay.running || self.m.page == Page::Relay {
            self.spawn(core::relay_status, |k, r| {
                if let Ok(v) = r {
                    if let Ok(st) = parse::<RelayStatus>(v) {
                        k.m.relay = st;
                    }
                }
            });
        }
        if matches!(self.m.page, Page::Logbook | Page::Stations) {
            self.refresh_book();
        }
    }

    pub fn refresh_book(&mut self) {
        self.spawn(core::logbook, |k, v| {
            if let Ok(b) = parse::<Logbook>(v) {
                k.m.book = b;
            }
        });
    }

    pub fn export_logbook(&mut self) {
        self.spawn(core::export_adif, |k, r| match r {
            Ok(p) => k.toast(format!("Logbook saved as {p}"), ToastKind::Good),
            Err(e) => k.warn(e.trim_start_matches("Error: ").to_string()),
        });
    }

    pub fn upload_logbook(&mut self, service: &'static str) {
        let cfg = serde_json::to_value(&self.m.settings.export_services).unwrap_or(json!({}));
        self.ui.upload_busy = true;
        self.spawn(
            move |s| core::upload_logbook(s, service.to_string(), cfg),
            |k, r| {
                k.ui.upload_busy = false;
                match r {
                    Ok(t) => k.toast(t, ToastKind::Good),
                    Err(e) => k.warn(e),
                }
            },
        );
    }

    pub fn check_update(&mut self) {
        self.ui.update = None;
        self.spawn(
            |_| core::check_update(),
            |k, r| {
                k.ui.update = Some(r.and_then(parse::<UpdateCheck>));
            },
        );
    }

    pub fn check_server(&mut self) {
        self.spawn(
            |_| core::server_status(),
            |k, v| k.ui.server = parse::<ServerCheck>(v).ok(),
        );
    }
}

fn show_toast(text: &str, kind: ToastKind) {
    let t = alloy_ui::toast(text).secs(5.0);
    match kind {
        ToastKind::Info => t.info(),
        ToastKind::Good => t.success(),
        ToastKind::Warn => t.warning(),
    };
}

fn log_line(text: &str) {
    eprintln!("kahf: {text}");
}

impl Kahf {
    /// Starts the engine with the draft settings of setup, so the person can see the spectrum
    /// move before finishing. Nothing is saved.
    pub fn preview_engine(&mut self, s: Settings) {
        let v = serde_json::to_value(&s).unwrap_or(Value::Null);
        self.spawn(
            move |sh| core::engine_start(sh, v),
            |k, r| {
                if let Err(e) = r {
                    k.m.engine_error = explain_start_error(&e);
                }
            },
        );
    }
}
