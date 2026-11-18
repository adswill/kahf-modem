//! Application state, messages and update logic. Drawing lives in view.rs and screens.rs.

use std::collections::VecDeque;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::keyboard::{self, key::Named, Key, Modifiers};
use iced::widget::{canvas, image, operation, text_editor};
use iced::{event, Event, Subscription, Task, Theme};
use kahf_core::link::{LinkState, CONNECT_TIMEOUT_MS, MAX_CONNECT_RETRIES};
use kahf_daemon::{Config, Engine, Event as EngineEvent, RelayAck, TxState};

use crate::sample::{self, Kind};
use crate::spectrum::{self, Sim, Waterfall};
use crate::strings as s;
use crate::theme;

pub const PALETTE_INPUT: &str = "palette-input";
const TOAST_SECONDS: u64 = 4;
const MODAL_FADE_MS: u64 = 150;
const PULSE_SECONDS: f32 = 1.8;
pub const SNR_SECONDS: usize = 60;

/// What the radio is busy with, for the pulsing status dot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Busy {
    Connecting,
    Transmitting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Operate,
    Stations,
    Messages,
    Mail,
    Files,
    Relay,
    Logbook,
    Map,
    Settings,
}

impl Section {
    pub const ALL: [Section; 9] = [
        Section::Operate,
        Section::Stations,
        Section::Messages,
        Section::Mail,
        Section::Files,
        Section::Relay,
        Section::Logbook,
        Section::Map,
        Section::Settings,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Section::Operate => s::SECTION_OPERATE,
            Section::Stations => s::SECTION_STATIONS,
            Section::Messages => s::SECTION_MESSAGES,
            Section::Mail => s::SECTION_MAIL,
            Section::Files => s::SECTION_FILES,
            Section::Relay => s::SECTION_RELAY,
            Section::Logbook => s::SECTION_LOGBOOK,
            Section::Map => s::SECTION_MAP,
            Section::Settings => s::SECTION_SETTINGS,
        }
    }
}

impl Section {
    pub fn tip(self) -> &'static str {
        match self {
            Section::Operate => s::TIP_RAIL_OPERATE,
            Section::Stations => s::TIP_RAIL_STATIONS,
            Section::Messages => s::TIP_RAIL_MESSAGES,
            Section::Mail => s::TIP_RAIL_MAIL,
            Section::Files => s::TIP_RAIL_FILES,
            Section::Relay => s::TIP_RAIL_RELAY,
            Section::Logbook => s::TIP_RAIL_LOGBOOK,
            Section::Map => s::TIP_RAIL_MAP,
            Section::Settings => s::TIP_RAIL_SETTINGS,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Station,
    Relay,
    Both,
}

impl Role {
    pub fn is_relay(self) -> bool {
        self != Role::Station
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Role::Station => s::ROLE_STATION,
            Role::Relay => s::ROLE_RELAY,
            Role::Both => s::ROLE_BOTH,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RigControl {
    None,
    Cat,
    Vox,
}

impl RigControl {
    pub const ALL: [RigControl; 3] = [RigControl::None, RigControl::Cat, RigControl::Vox];
}

impl fmt::Display for RigControl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RigControl::None => s::RIG_NONE,
            RigControl::Cat => s::RIG_CAT,
            RigControl::Vox => s::RIG_VOX,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Location {
    Off,
    Precise,
    Area,
}

impl Location {
    pub const ALL: [Location; 3] = [Location::Off, Location::Precise, Location::Area];
    pub fn label(self) -> &'static str {
        match self {
            Location::Off => s::LOC_OFF,
            Location::Precise => s::LOC_PRECISE,
            Location::Area => s::LOC_AREA,
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Location::Off => s::LOC_OFF_DESC,
            Location::Precise => s::LOC_PRECISE_DESC,
            Location::Area => s::LOC_AREA_DESC,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxAction {
    Connect,
    Send,
    SendFile,
    Beacon,
}

impl TxAction {
    pub fn label(self) -> &'static str {
        match self {
            TxAction::Connect => s::ACTION_CONNECT,
            TxAction::Send => s::ACTION_SEND,
            TxAction::SendFile => s::ACTION_SEND_FILE,
            TxAction::Beacon => s::ACTION_BEACON,
        }
    }
}

/// One thing the app asks the engine to do, in order, for a send.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Connect(String),
    /// To, text, kind.
    Message(String, String, Kind),
    File(String, PathBuf),
    Mail(String, String, String),
}

/// A connection the app started and is waiting on.
#[derive(Debug, Clone)]
pub struct ConnectTry {
    pub peer: String,
    pub since: Instant,
    /// What is waiting in the outbox: "message" or "file".
    pub what: &'static str,
}

pub const SDR_CHOICES: [&str; 3] = [s::SDR_NONE_CHOICE, s::SDR_HACKRF, s::SDR_RTL];
pub const SETUP_STEPS: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirm {
    Transmit(TxAction),
    Disconnect,
    Relay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileCmd {
    Start,
    Pause,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Warning,
}

pub struct Toast {
    pub text: String,
    pub kind: ToastKind,
    until: Instant,
}

pub struct Palette {
    pub query: String,
    pub selected: usize,
}

#[derive(Debug, Clone)]
pub struct Command {
    pub label: String,
    pub msg: Message,
}

#[derive(Debug, Clone)]
pub enum Message {
    Tick(Instant),
    Anim(Instant),
    Key(Key, Modifiers),
    Go(Section),
    ToggleRail,
    TogglePanel,
    ToggleBand,
    SelectFile(usize),
    AttachOpen(bool),
    AttachPath(String),
    RigAddr(String),
    Rescan,
    OpenSetup(Option<usize>),
    SetupFinish,
    SetupClose,
    ToggleListen(bool),
    Tune(f32),
    Target(String),
    Composer(text_editor::Action),
    KindPicked(Kind),
    RequestTx(TxAction),
    ConfirmTx,
    CancelConfirm,
    RequestDisconnect,
    ConfirmDisconnect,
    OpenPalette,
    PaletteQuery(String),
    PaletteRun,
    PaletteRunAt(usize),
    StationFilter(String),
    SelectStation(usize),
    SelectConversation(usize),
    SelectMsg(usize),
    SelectMail(usize),
    MailBack,
    MailCompose,
    MailTo(String),
    MailSubject(String),
    MailBody(text_editor::Action),
    MailSigned(bool),
    MailQueue,
    File(usize, FileCmd),
    RelayAccept,
    RelayLicence(bool),
    RelayTerms(bool),
    RelayStart,
    RelayDecline,
    LogSearch(String),
    LogExport,
    OpenMap,
    Callsign(String),
    RoleChosen(Role),
    AudioIn(String),
    AudioOut(String),
    BandChosen(spectrum::Band),
    VolIn(f32),
    VolOut(f32),
    RigChosen(RigControl),
    SdrChosen(String),
    ToggleRecord,
    LocationChosen(Location),
    Radius(f32),
    InternetRelay(bool),
    ShowSample(bool),
    Profile(&'static str),
    Level(f32),
    CheckUpdates,
}

pub const PROFILES: &[&str] = &[s::PROFILE_AUTO, s::PROFILE_ROBUST, s::PROFILE_FAST];
pub const KINDS: [Kind; 4] = [Kind::Chat, Kind::Urgent, Kind::Signed, Kind::Private];

pub struct App {
    pub section: Section,
    pub rail_collapsed: bool,
    pub panel_shown: bool,
    pub recording: bool,
    pub band_open: bool,
    pub clock: Instant,
    pub ticks: u64,
    pub file_sel: Option<usize>,
    pub listen_only: bool,
    pub sample: bool,
    pub role: Role,
    pub callsign: String,
    pub palette: Option<Palette>,
    pub confirm: Option<Confirm>,
    pub modal_since: Option<Instant>,
    pub modal_alpha: f32,
    pub anim_start: Instant,
    pub pulse: f32,
    pub busy: Option<(Busy, Instant)>,
    pub snr_hist: VecDeque<f32>,
    pub level_now: f32,
    pub level_peak: f32,
    last_snr: Instant,
    pub toast: Option<Toast>,

    // Operate
    pub bins: Vec<f32>,
    pub sim: Sim,
    pub wf: Waterfall,
    pub wf_handle: image::Handle,
    pub grid: canvas::Cache,
    pub tune: f32,
    pub link: Option<String>,
    pub target: String,
    pub engine: Option<Engine>,
    /// Test switch: record the engine steps but do not run them.
    pub dry_run: bool,
    /// Steps of the latest sends, in order.
    pub steps: Vec<Step>,
    pub data_dir: PathBuf,
    pub persist: bool,
    pub attach_open: bool,
    pub attach_path: String,
    pub connect_try: Option<ConnectTry>,
    pub no_reply: Option<(String, &'static str)>,
    pub engine_note: String,
    pub events: Vec<(String, String)>,
    /// Relay start panel: the role asked for and the two tick boxes. Never saved.
    pub relay_role: Option<Role>,
    pub relay_licence: bool,
    pub relay_terms: bool,
    live_spectrum: Option<Instant>,
    pub setup_open: bool,
    pub setup_done: bool,
    pub setup_focus: Option<usize>,
    pub tx_intro_seen: bool,
    pub rig_addr: String,
    pub detected_audio: Vec<String>,
    pub detected_sdr: Vec<String>,
    pub composer: text_editor::Content,
    pub kind: Kind,
    pub outbox: usize,
    pub last_sent: Option<String>,
    pub thread: Vec<sample::Chat>,

    // Lists
    pub station_filter: String,
    pub station_sel: Option<usize>,
    pub conv_sel: usize,
    pub msg_sel: Option<usize>,
    pub conv_unread: Vec<u32>,
    pub mail_sel: Option<usize>,
    pub mail_unread: Vec<bool>,
    pub mail_composing: bool,
    pub mail_to: String,
    pub mail_subject: String,
    pub mail_body: text_editor::Content,
    pub mail_signed: bool,
    pub files: Vec<sample::FileRow>,
    pub log_search: String,

    // Settings
    /// Empty means not chosen yet.
    pub audio_in: String,
    pub audio_out: String,
    pub band: spectrum::Band,
    pub vol_in: f32,
    pub vol_out: f32,
    pub rig: RigControl,
    /// Empty means not chosen yet.
    pub sdr: String,
    pub location: Location,
    pub radius_km: f32,
    pub internet_relay: bool,
    pub profile: &'static str,
    pub level: f32,
}

pub fn limit_edit(current_chars: usize, edit: text_editor::Edit) -> Option<text_editor::Edit> {
    use text_editor::Edit;
    let room = s::MESSAGE_LIMIT.saturating_sub(current_chars);
    match edit {
        Edit::Insert(_) | Edit::Enter | Edit::Indent => (room > 0).then_some(edit),
        Edit::Paste(text) => {
            if room == 0 {
                None
            } else if text.chars().count() <= room {
                Some(Edit::Paste(text))
            } else {
                Some(Edit::Paste(Arc::new(text.chars().take(room).collect())))
            }
        }
        other => Some(other),
    }
}

impl App {
    pub fn new() -> (Self, Task<Message>) {
        let mut app = Self::init();
        app.persist = true;
        app.apply_state(&load_state());
        app.setup_open = app.needs_setup();
        if app.setup_open {
            app.rescan();
        } else {
            app.start_engine();
        }
        // Test hook: KAHF_START=messages opens that screen first.
        if let Ok(name) = std::env::var("KAHF_START") {
            if let Some(sec) = Section::ALL
                .iter()
                .find(|s| s.label().eq_ignore_ascii_case(&name))
            {
                app.section = *sec;
            }
        }
        // Test hook: KAHF_DEMO=tx shows the transmitting state, =modal a confirmation.
        match std::env::var("KAHF_DEMO").as_deref() {
            Ok("tx") => {
                app.listen_only = false;
                app.callsign = "XX0AB".into();
                app.link = Some("XX1XX".into());
                app.busy = Some((Busy::Transmitting, Instant::now() + Duration::from_secs(600)));
            }
            Ok("empty") => app.load_sample(false),
            Ok("modal") => {
                app.listen_only = false;
                app.callsign = "XX0AB".into();
                app.confirm = Some(Confirm::Disconnect);
                app.link = Some("XX1XX".into());
            }
            _ => {}
        }
        (app, Task::none())
    }

    pub fn init() -> Self {
        let bins = vec![-106.0; spectrum::BINS];
        let wf = Waterfall::new();
        let wf_handle = wf.handle();
        let mut app = Self {
            section: Section::Operate,
            rail_collapsed: false,
            panel_shown: true,
            recording: false,
            band_open: true,
            clock: Instant::now(),
            ticks: 0,
            file_sel: None,
            listen_only: true,
            sample: false,
            role: Role::Station,
            callsign: String::new(),
            palette: None,
            confirm: None,
            modal_since: None,
            modal_alpha: 1.0,
            anim_start: Instant::now(),
            pulse: 0.0,
            busy: None,
            snr_hist: VecDeque::new(),
            level_now: 0.0,
            level_peak: 0.0,
            last_snr: Instant::now(),
            toast: None,
            bins,
            sim: Sim::new(),
            wf,
            wf_handle,
            grid: canvas::Cache::new(),
            tune: 0.5,
            link: None,
            target: String::new(),
            engine: None,
            dry_run: false,
            steps: Vec::new(),
            data_dir: default_data_dir(),
            persist: false,
            attach_open: false,
            attach_path: String::new(),
            connect_try: None,
            no_reply: None,
            engine_note: String::new(),
            events: Vec::new(),
            relay_role: None,
            relay_licence: false,
            relay_terms: false,
            live_spectrum: None,
            setup_open: false,
            setup_done: false,
            setup_focus: None,
            tx_intro_seen: false,
            rig_addr: String::new(),
            detected_audio: Vec::new(),
            detected_sdr: Vec::new(),
            composer: text_editor::Content::new(),
            kind: Kind::Chat,
            outbox: 0,
            last_sent: None,
            thread: Vec::new(),
            station_filter: String::new(),
            station_sel: None,
            conv_sel: 0,
            msg_sel: None,
            conv_unread: Vec::new(),
            mail_sel: None,
            mail_unread: Vec::new(),
            mail_composing: false,
            mail_to: String::new(),
            mail_subject: String::new(),
            mail_body: text_editor::Content::new(),
            mail_signed: false,
            files: Vec::new(),
            log_search: String::new(),
            audio_in: String::new(),
            audio_out: String::new(),
            band: spectrum::Band::M40,
            vol_in: 70.0,
            vol_out: 70.0,
            rig: RigControl::None,
            sdr: String::new(),
            location: Location::Precise,
            radius_km: 25.0,
            internet_relay: false,
            profile: s::PROFILE_AUTO,
            level: 40.0,
        };
        app.load_sample(false);
        app.warm_up();
        app
    }

    pub fn theme(&self) -> Theme {
        theme::kahf()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        // The band is on every screen, so the spectrum runs everywhere.
        let period = Duration::from_millis(1000 / spectrum::FRAMES_PER_SECOND);
        // The animation clock only runs while a dot pulses or a modal fades in.
        let animating = self.busy.is_some() || (self.modal_open() && self.modal_alpha < 1.0);
        let anim = if animating {
            iced::time::every(Duration::from_millis(33)).map(Message::Anim)
        } else {
            Subscription::none()
        };
        Subscription::batch([
            iced::time::every(period).map(Message::Tick),
            anim,
            event::listen_with(|event, _status, _window| match event {
                Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                    Some(Message::Key(key, modifiers))
                }
                _ => None,
            }),
        ])
    }

    // ------------------------------------------------------------------ queries

    pub fn my_call(&self) -> &str {
        self.callsign.trim()
    }

    /// Setup opens when it was never finished, no callsign is set, or neither audio nor an SDR is chosen.
    pub fn needs_setup(&self) -> bool {
        !self.setup_done || self.my_call().is_empty() || !self.receiving_set()
    }

    /// A way to receive is chosen: an SDR, or both sound devices.
    pub fn receiving_set(&self) -> bool {
        self.sdr_in_use() || (!self.audio_in.is_empty() && !self.audio_out.is_empty())
    }

    pub fn sdr_in_use(&self) -> bool {
        self.sdr == s::SDR_HACKRF || self.sdr == s::SDR_RTL
    }

    /// State of each setup step: true when done.
    pub fn setup_state(&self) -> [bool; SETUP_STEPS] {
        [
            !self.my_call().is_empty(),
            !(self.rig == RigControl::Cat && self.rig_addr.trim().is_empty()),
            !self.sdr.is_empty() || self.receiving_set(),
            (!self.audio_in.is_empty() && !self.audio_out.is_empty()) || self.sdr_in_use(),
            true,
            true,
        ]
    }

    pub fn audio_options(&self) -> Vec<String> {
        let mut v = vec![s::DEVICE_SYSTEM.to_string()];
        v.extend(self.detected_audio.iter().cloned());
        v
    }

    pub fn sdr_options(&self) -> Vec<String> {
        let mut v: Vec<String> = SDR_CHOICES.iter().map(|d| d.to_string()).collect();
        v.extend(self.detected_sdr.iter().cloned());
        v
    }

    pub fn sdr_selected(&self) -> String {
        if self.sdr.is_empty() {
            s::SDR_NONE_CHOICE.to_string()
        } else {
            self.sdr.clone()
        }
    }

    pub fn linked_to(&self, call: &str) -> bool {
        self.link
            .as_deref()
            .is_some_and(|l| l.eq_ignore_ascii_case(call.trim()))
    }

    /// What the engine is asked to do for a transmit action, in order.
    pub fn tx_plan(&self, action: TxAction) -> Vec<Step> {
        let to = self.target.trim().to_uppercase();
        let waiting = self
            .connect_try
            .as_ref()
            .is_some_and(|t| t.peer.eq_ignore_ascii_case(&to));
        let mut plan = Vec::new();
        if matches!(action, TxAction::Connect | TxAction::Send | TxAction::SendFile)
            && !self.linked_to(&to)
            && !waiting
        {
            plan.push(Step::Connect(to.clone()));
        }
        match action {
            TxAction::Send => plan.push(Step::Message(
                to,
                self.composer.text().trim().to_string(),
                self.kind,
            )),
            TxAction::SendFile => plan.push(Step::File(to, PathBuf::from(self.attach_path.trim()))),
            _ => {}
        }
        plan
    }

    /// Plain status line of the send flow, if there is something to say.
    pub fn status_line(&self) -> Option<(String, bool)> {
        if let Some(t) = &self.connect_try {
            let attempt = (t.since.elapsed().as_millis() as u64 / CONNECT_TIMEOUT_MS) as u32 + 1;
            return Some((
                s::connecting(
                    &t.peer,
                    attempt.min(MAX_CONNECT_RETRIES + 1),
                    MAX_CONNECT_RETRIES + 1,
                ),
                true,
            ));
        }
        self.no_reply
            .as_ref()
            .map(|(peer, what)| (s::no_reply(peer, what), false))
    }

    pub fn composer_len(&self) -> usize {
        self.composer.text().chars().count()
    }

    /// The messages of the open conversation (sample data only).
    pub fn conv_chats(&self) -> Vec<sample::Chat> {
        if !self.sample {
            return Vec::new();
        }
        if self.conv_sel == 0 {
            return self.thread.clone();
        }
        sample::CONVERSATIONS
            .get(self.conv_sel)
            .map(|c| {
                vec![sample::Chat {
                    time: c.when.to_string(),
                    from: c.call.to_string(),
                    text: c.preview.to_string(),
                    kind: Kind::Chat,
                    mine: false,
                }]
            })
            .unwrap_or_default()
    }

    /// Route of the selected message (the newest one when none is chosen).
    pub fn selected_route(&self) -> Option<Vec<sample::Hop>> {
        let chats = self.conv_chats();
        let index = self.msg_sel.unwrap_or(chats.len().saturating_sub(1));
        let chat = chats.get(index)?;
        let me = if self.my_call().is_empty() {
            "You"
        } else {
            self.my_call()
        };
        let peer = sample::CONVERSATIONS.get(self.conv_sel)?.call;
        Some(sample::route(chat, index, me, peer))
    }

    pub fn tuned_hz(&self) -> f64 {
        spectrum::hz_at(self.tune)
    }

    pub fn section_unread(&self, section: Section) -> u32 {
        match section {
            Section::Messages => self.conv_unread.iter().sum(),
            Section::Mail => self.mail_unread.iter().filter(|u| **u).count() as u32,
            _ => 0,
        }
    }

    pub fn commands(&self) -> Vec<Command> {
        let cmd = |label: &str, msg: Message| Command {
            label: label.to_string(),
            msg,
        };
        let mut out = vec![if self.listen_only {
            cmd(s::CMD_LISTEN_OFF, Message::ToggleListen(false))
        } else {
            cmd(s::CMD_LISTEN_ON, Message::ToggleListen(true))
        }];
        if !self.listen_only && self.link.is_none() {
            let label = if self.target.trim().is_empty() {
                s::CMD_CONNECT.to_string()
            } else {
                s::connect_to(self.target.trim())
            };
            out.push(Command {
                label,
                msg: Message::RequestTx(TxAction::Connect),
            });
        }
        if self.link.is_some() {
            out.push(cmd(s::CMD_DISCONNECT, Message::RequestDisconnect));
        }
        out.push(cmd(s::CMD_TOGGLE_RAIL, Message::ToggleRail));
        out.push(cmd(s::CMD_TOGGLE_PANEL, Message::TogglePanel));
        for sec in Section::ALL {
            out.push(Command {
                label: format!("{} {}", s::CMD_GO, sec.label()),
                msg: Message::Go(sec),
            });
        }
        out
    }

    pub fn filtered_commands(&self, query: &str) -> Vec<Command> {
        let q = query.to_lowercase();
        let words: Vec<&str> = q.split_whitespace().collect();
        self.commands()
            .into_iter()
            .filter(|c| {
                let l = c.label.to_lowercase();
                words.iter().all(|w| l.contains(w))
            })
            .collect()
    }

    // ------------------------------------------------------------------ helpers

    /// Fills the waterfall and the 60 s history so the first frame is not empty.
    fn warm_up(&mut self) {
        for frame in 0..(spectrum::WF_ROWS + SNR_SECONDS * spectrum::FRAMES_PER_SECOND as usize) {
            self.sim.step(&mut self.bins);
            if frame >= SNR_SECONDS * spectrum::FRAMES_PER_SECOND as usize - spectrum::WF_ROWS {
                self.wf.push(&self.bins);
            }
            if frame % spectrum::FRAMES_PER_SECOND as usize == 0 {
                self.snr_hist.push_back(self.snr_now());
                if self.snr_hist.len() > SNR_SECONDS {
                    self.snr_hist.pop_front();
                }
            }
        }
        self.wf_handle = self.wf.handle();
        self.update_level();
    }

    /// Simulated signal to noise ratio in dB (peak over the noise floor).
    pub fn snr_now(&self) -> f32 {
        let lo = (((self.tune + (spectrum::PASS_LO_HZ / spectrum::SPAN_HZ) as f32) * spectrum::BINS as f32)
            .max(0.0)) as usize;
        let hi = ((((self.tune + (spectrum::PASS_HI_HZ / spectrum::SPAN_HZ) as f32) * spectrum::BINS as f32)
            as usize)
            .min(self.bins.len()))
        .max(lo + 1);
        let peak = self.bins[lo.min(self.bins.len() - 1)..hi]
            .iter()
            .copied()
            .fold(f32::MIN, f32::max);
        (peak + 106.0).max(0.0)
    }

    pub fn modal_open(&self) -> bool {
        self.palette.is_some() || self.confirm.is_some()
    }

    fn update_level(&mut self) {
        self.level_now =
            ((self.sim.peak_db - spectrum::DB_MIN) / (spectrum::DB_MAX - spectrum::DB_MIN)).clamp(0.0, 1.0);
        self.level_peak = self.level_now.max(self.level_peak - 0.006);
    }

    /// Pulse phase for a dot, only while something is going on.
    pub fn pulse_for(&self, active: bool) -> Option<f32> {
        active.then_some(self.pulse)
    }

    pub fn is_busy(&self, what: Busy) -> bool {
        self.busy.is_some_and(|(b, _)| b == what)
    }

    fn load_sample(&mut self, on: bool) {
        self.sample = on;
        if on {
            let me = if self.my_call().is_empty() {
                "You"
            } else {
                self.my_call()
            }
            .to_string();
            self.thread = sample::thread(&me);
            self.conv_unread = sample::CONVERSATIONS.iter().map(|c| c.unread).collect();
            self.mail_unread = sample::MAILS.iter().map(|m| m.unread).collect();
            self.files = sample::files();
        } else {
            self.thread.clear();
            self.conv_unread.clear();
            self.mail_unread.clear();
            self.files.clear();
        }
        self.conv_sel = 0;
        self.msg_sel = None;
        self.mail_sel = on.then_some(0);
        self.file_sel = None;
        self.station_sel = None;
    }

    fn audio_choice(name: &str) -> Option<String> {
        (!name.is_empty() && name != s::DEVICE_SYSTEM).then(|| name.to_string())
    }

    fn ensure_engine(&mut self) -> Result<&mut Engine, String> {
        if self.engine.is_none() {
            let config = Config {
                callsign: self.my_call().to_string(),
                audio_in: Self::audio_choice(&self.audio_in),
                audio_out: Self::audio_choice(&self.audio_out),
                rigctld: (self.rig == RigControl::Cat && !self.rig_addr.trim().is_empty())
                    .then(|| self.rig_addr.trim().to_string()),
                sdr: (self.sdr == s::SDR_HACKRF).then(|| "hackrf".to_string()),
                data_dir: self.data_dir.clone(),
            };
            let engine = Engine::new(config)?;
            engine.set_listen_only(self.listen_only);
            engine.set_internet_relay(self.internet_relay);
            self.engine = Some(engine);
        }
        self.engine.as_mut().ok_or_else(|| s::BLOCK_NO_ENGINE.to_string())
    }

    /// Starts the engine and receiving. Quiet when the callsign is not set or in sample mode.
    fn start_engine(&mut self) {
        if self.dry_run || self.sample || self.my_call().is_empty() {
            return;
        }
        let result = self.ensure_engine().and_then(|e| e.listen());
        if let Err(e) = result {
            self.engine_note = e;
        }
    }

    /// Drops the engine so the next use rebuilds it with the new settings.
    fn restart_engine(&mut self) {
        self.engine = None;
        self.connect_try = None;
        self.link = None;
    }

    fn set_listen(&mut self, on: bool) {
        self.listen_only = on;
        if let Some(e) = &self.engine {
            e.set_listen_only(on);
        }
    }

    pub fn rescan(&mut self) {
        self.detected_audio = kahf_audio::list_devices();
        let mut sdrs: Vec<String> = kahf_sdr::hackrf::list_devices()
            .into_iter()
            .map(|d| format!("{} {d}", s::SDR_HACKRF))
            .collect();
        sdrs.extend(kahf_sdr::rtlsdr::list_devices());
        self.detected_sdr = sdrs;
    }

    fn save(&self) {
        if self.persist {
            save_state(&self.state());
        }
    }

    fn state(&self) -> Vec<(&'static str, String)> {
        vec![
            ("band", if self.band_open { "open" } else { "closed" }.into()),
            ("setup", if self.setup_done { "done" } else { "new" }.into()),
            ("tx_intro", if self.tx_intro_seen { "seen" } else { "new" }.into()),
            ("call", self.callsign.clone()),
            ("audio_in", self.audio_in.clone()),
            ("audio_out", self.audio_out.clone()),
            (
                "location",
                match self.location {
                    Location::Off => "off",
                    Location::Precise => "precise",
                    Location::Area => "area",
                }
                .into(),
            ),
            ("vol_in", format!("{:.0}", self.vol_in)),
            ("vol_out", format!("{:.0}", self.vol_out)),
            ("sdr", self.sdr.clone()),
            (
                "rig",
                match self.rig {
                    RigControl::None => "none",
                    RigControl::Cat => "cat",
                    RigControl::Vox => "vox",
                }
                .into(),
            ),
            ("rigctld", self.rig_addr.clone()),
        ]
    }

    fn apply_state(&mut self, st: &std::collections::HashMap<String, String>) {
        let get = |k: &str| st.get(k).cloned().unwrap_or_default();
        self.band_open = get("band") != "closed";
        self.setup_done = get("setup") == "done";
        self.tx_intro_seen = get("tx_intro") == "seen";
        self.callsign = get("call");
        self.audio_in = get("audio_in");
        self.audio_out = get("audio_out");
        self.location = match get("location").as_str() {
            "off" => Location::Off,
            "area" => Location::Area,
            _ => Location::Precise,
        };
        self.vol_in = get("vol_in").parse().unwrap_or(70.0);
        self.vol_out = get("vol_out").parse().unwrap_or(70.0);
        self.sdr = get("sdr");
        self.rig = match get("rig").as_str() {
            "cat" => RigControl::Cat,
            "vox" => RigControl::Vox,
            _ => RigControl::None,
        };
        self.rig_addr = get("rigctld");
    }

    /// Finishes setup. Refused, with a sentence on what to do, until a callsign is set.
    fn finish_setup(&mut self) -> bool {
        if self.my_call().is_empty() {
            self.setup_focus = None;
            self.toast(s::SETUP_NEEDS_CALL, ToastKind::Warning);
            return false;
        }
        self.setup_done = true;
        self.setup_open = false;
        self.setup_focus = None;
        self.save();
        self.start_engine();
        true
    }

    /// Runs one engine step. Returns the engine id for a file.
    fn exec(&mut self, step: Step, what: &'static str) -> Result<Option<u64>, String> {
        self.steps.push(step.clone());
        if self.dry_run {
            return Ok(None);
        }
        if self.sample {
            if let Step::Connect(call) = step {
                self.link = Some(call);
                self.busy = Some((Busy::Connecting, Instant::now() + Duration::from_millis(2200)));
            }
            return Ok(None);
        }
        let id = match step {
            Step::Connect(to) => {
                self.ensure_engine()?.connect(&to)?;
                self.no_reply = None;
                self.connect_try = Some(ConnectTry {
                    peer: to,
                    since: Instant::now(),
                    what,
                });
                self.busy = Some((Busy::Connecting, Instant::now() + Duration::from_secs(30)));
                None
            }
            Step::Message(to, text, kind) => {
                let e = self.ensure_engine()?;
                match kind {
                    Kind::Private => e.send_message_private(&to, &text)?,
                    Kind::Urgent if !text.starts_with("URGENT:") => {
                        e.send_message(&to, &format!("URGENT: {text}"))?
                    }
                    _ => e.send_message(&to, &text)?,
                }
                None
            }
            Step::File(to, path) => Some(self.ensure_engine()?.send_file(&to, &path)?),
            Step::Mail(to, subject, body) => {
                self.ensure_engine()?.send_mail(&to, &subject, &body, &[])?;
                None
            }
        };
        Ok(id)
    }

    /// Ends a connection attempt that got no answer.
    fn connect_failed(&mut self) {
        if let Some(t) = self.connect_try.take() {
            self.no_reply = Some((t.peer, t.what));
            if self.is_busy(Busy::Connecting) {
                self.busy = None;
            }
        }
    }

    fn poll_engine(&mut self, now: Instant) {
        let Some(engine) = self.engine.as_mut() else {
            return;
        };
        let events = engine.poll_events();
        // The daemon decides whether relay mode runs; the role shown follows it.
        if self.role.is_relay() && !engine.relay_running() {
            self.role = Role::Station;
        }
        self.outbox = engine.outbox_pending().len();
        for ev in events {
            self.on_engine_event(ev, now);
        }
        let limit = Duration::from_millis((MAX_CONNECT_RETRIES as u64 + 1) * CONNECT_TIMEOUT_MS + 2500);
        if self
            .connect_try
            .as_ref()
            .is_some_and(|t| t.since.elapsed() > limit)
        {
            self.connect_failed();
        }
    }

    pub fn on_engine_event(&mut self, ev: EngineEvent, now: Instant) {
        match ev {
            EngineEvent::Link(LinkState::Connected { peer }) => {
                self.connect_try = None;
                self.no_reply = None;
                if self.is_busy(Busy::Connecting) {
                    self.busy = None;
                }
                if self.target.trim().is_empty() {
                    self.target = peer.clone();
                }
                self.link = Some(peer);
                self.toast(s::TOAST_LINKED, ToastKind::Info);
            }
            EngineEvent::Link(LinkState::Connecting { peer }) => {
                if self.connect_try.is_none() {
                    self.connect_try = Some(ConnectTry {
                        peer,
                        since: now,
                        what: "message",
                    });
                }
            }
            EngineEvent::Link(LinkState::Idle) => {
                if self.connect_try.is_some() {
                    self.connect_failed();
                } else if self.link.take().is_some() {
                    self.toast(s::TOAST_UNLINKED, ToastKind::Info);
                }
            }
            EngineEvent::Link(LinkState::Disconnecting) => {}
            EngineEvent::Message { from, text } => {
                self.thread.push(sample::Chat {
                    time: crate::view::utc_hm(),
                    from,
                    text,
                    kind: Kind::Chat,
                    mine: false,
                });
            }
            EngineEvent::Mail { from, subject, .. } => {
                self.toast(&format!("Mail from {from}: {subject}"), ToastKind::Info);
            }
            EngineEvent::FileProgress {
                id,
                name,
                done,
                total,
            } => {
                let progress = if total == 0 {
                    1.0
                } else {
                    (done as f32 / total as f32).clamp(0.0, 1.0)
                };
                let state = if done >= total {
                    sample::FileState::Done
                } else {
                    sample::FileState::Active
                };
                if let Some(f) = self.files.iter_mut().find(|f| f.id == Some(id)) {
                    f.progress = progress;
                    f.state = state;
                } else {
                    self.files.push(sample::FileRow {
                        id: Some(id),
                        name: name.clone(),
                        to: String::new(),
                        sending: false,
                        bytes: total,
                        progress,
                        state,
                        retries: 0,
                    });
                    let from = self.link.clone().unwrap_or_default();
                    self.thread.push(sample::Chat {
                        time: crate::view::utc_hm(),
                        from,
                        text: s::file_line(&name, total),
                        kind: Kind::File,
                        mine: false,
                    });
                }
            }
            EngineEvent::Spectrum(bins) => {
                if bins.len() == self.bins.len() {
                    self.bins = bins;
                    self.live_spectrum = Some(now);
                }
            }
            EngineEvent::Heard { .. } | EngineEvent::FileReceived { .. } | EngineEvent::MailReceived { .. } => {}
            EngineEvent::Metrics(st) => {
                self.engine_note = format!(
                    "Link: {} (auto {}), net {} bit/s, we hear {:?} dB, they hear {:?} dB, lost {:.0}%",
                    st.mode, st.auto, st.net_bps, st.own_snr_db, st.peer_snr_db, st.fer_pct
                );
            }
            EngineEvent::Log(msg) => {
                if msg.starts_with("Could not reach") {
                    let peer = self.target.trim().to_uppercase();
                    self.connect_try = None;
                    self.no_reply = Some((peer, "message"));
                }
                self.engine_note = msg;
            }
            EngineEvent::Tx { state } => match state {
                TxState::Starting | TxState::Sending => {
                    self.busy = Some((Busy::Transmitting, now + Duration::from_secs(30)));
                }
                TxState::Done => {
                    if self.is_busy(Busy::Transmitting) {
                        self.busy = None;
                    }
                }
                TxState::Failed(why) => {
                    self.busy = None;
                    self.toast(&why, ToastKind::Warning);
                }
            },
        }
    }

    fn relay_reset(&mut self) {
        self.relay_role = None;
        self.relay_licence = false;
        self.relay_terms = false;
    }

    /// Sends the relay start request to the daemon, with the acknowledgement of both ticks.
    /// The daemon's own relay start must also refuse without it; the panel is not the only gate.
    fn start_relay(&mut self, role: Role) {
        let channels: Vec<f32> = spectrum::channels()
            .iter()
            .map(|(hz, _)| (*hz / 1000.0) as f32)
            .collect();
        let acknowledged = self.relay_licence && self.relay_terms;
        if !acknowledged {
            return;
        }
        let core_role = if role == Role::Both {
            kahf_core::relay::Role::RelayAndStation
        } else {
            kahf_core::relay::Role::Relay
        };
        let ack = RelayAck {
            licence_held: self.relay_licence,
            terms_accepted: self.relay_terms,
        };
        let result = match self.ensure_engine() {
            Ok(e) => e
                .configure_relay(core_role, kahf_core::relay::Radio::HalfDuplex, &channels)
                .and_then(|()| e.start_relay(ack)),
            Err(why) => Err(why),
        };
        match result {
            Ok(()) => {
                self.role = role;
                self.toast(s::TOAST_RELAY_STARTED, ToastKind::Info);
            }
            Err(why) => {
                // Relay stays off.
                self.role = Role::Station;
                self.toast(&why, ToastKind::Warning);
            }
        }
    }

    fn toast(&mut self, text: &str, kind: ToastKind) {
        self.events.push((crate::view::utc_clock(), text.to_string()));
        if self.events.len() > 8 {
            self.events.remove(0);
        }
        self.toast = Some(Toast {
            text: text.to_string(),
            kind,
            until: Instant::now() + Duration::from_secs(TOAST_SECONDS),
        });
    }

    fn open_palette(&mut self) -> Task<Message> {
        self.palette = Some(Palette {
            query: String::new(),
            selected: 0,
        });
        operation::focus(PALETTE_INPUT)
    }

    /// Checks the preconditions for a transmit action and opens its confirmation.
    /// Every refusal says what to do next. Nothing can be sent while Listen only is on.
    fn request_tx(&mut self, action: TxAction) {
        let warn = |app: &mut Self, text: &str| app.toast(text, ToastKind::Warning);
        if self.listen_only {
            let text = if action == TxAction::Beacon {
                s::BLOCK_BEACON
            } else {
                s::BLOCK_LISTEN
            };
            return warn(self, text);
        }
        if self.my_call().is_empty() {
            return warn(self, s::BLOCK_NO_CALL);
        }
        if !self.sample && action == TxAction::Beacon {
            return;
        }
        if action != TxAction::Beacon && self.target.trim().is_empty() {
            return warn(self, s::BLOCK_NO_TO);
        }
        match action {
            TxAction::Send if self.composer.text().trim().is_empty() => return warn(self, s::BLOCK_NO_TEXT),
            TxAction::SendFile if self.attach_path.trim().is_empty() => return warn(self, s::BLOCK_NO_FILE),
            TxAction::SendFile if !std::path::Path::new(self.attach_path.trim()).is_file() => {
                return warn(self, s::BLOCK_FILE_MISSING)
            }
            _ => {}
        }
        self.confirm = Some(Confirm::Transmit(action));
    }

    /// Size in bytes of the file in the attach field, 0 if unknown.
    pub fn attach_bytes(&self) -> u64 {
        std::fs::metadata(self.attach_path.trim())
            .map(|m| m.len())
            .unwrap_or(0)
    }

    fn do_tx(&mut self, action: TxAction) {
        if self.listen_only {
            return;
        }
        let what = if action == TxAction::SendFile {
            "file"
        } else {
            "message"
        };
        let to = self.target.trim().to_uppercase();
        let connecting = !self.linked_to(&to);
        for step in self.tx_plan(action) {
            let file_path = if let Step::File(_, p) = &step {
                Some(p.clone())
            } else {
                None
            };
            match self.exec(step, what) {
                Ok(id) => {
                    if let Some(path) = file_path {
                        self.add_sent_file(id, &to, &path);
                    }
                }
                Err(e) => {
                    self.toast(&e, ToastKind::Warning);
                    return;
                }
            }
        }
        match action {
            TxAction::Connect => {
                if self.sample {
                    self.toast(s::TOAST_LINKED, ToastKind::Info);
                }
            }
            TxAction::Send => {
                let text = self.composer.text().trim().to_string();
                self.thread.push(sample::Chat {
                    time: crate::view::utc_hm(),
                    from: self.my_call().to_string(),
                    text: text.clone(),
                    kind: self.kind,
                    mine: true,
                });
                self.last_sent = Some(text);
                self.composer = text_editor::Content::new();
                if self.sample {
                    self.busy = Some((Busy::Transmitting, Instant::now() + Duration::from_millis(2600)));
                    self.outbox += 1;
                }
                if !connecting {
                    self.toast(s::TOAST_SENT, ToastKind::Info);
                }
            }
            TxAction::SendFile => {
                self.attach_open = false;
                self.attach_path.clear();
                if self.sample {
                    self.outbox += 1;
                }
                if !connecting {
                    self.toast(s::TOAST_FILE_QUEUED, ToastKind::Info);
                }
            }
            TxAction::Beacon => {
                self.busy = Some((Busy::Transmitting, Instant::now() + Duration::from_millis(2600)));
                self.outbox += 1;
                self.toast(s::TOAST_BEACON, ToastKind::Info);
            }
        }
    }

    /// A file that was handed to the engine: a row in Files and a line in the conversation.
    fn add_sent_file(&mut self, id: Option<u64>, to: &str, path: &std::path::Path) {
        let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.files.push(sample::FileRow {
            id,
            name: name.clone(),
            to: to.to_string(),
            sending: true,
            bytes,
            progress: 0.0,
            state: sample::FileState::Waiting,
            retries: 0,
        });
        self.thread.push(sample::Chat {
            time: crate::view::utc_hm(),
            from: self.my_call().to_string(),
            text: s::file_line(&name, bytes),
            kind: Kind::File,
            mine: true,
        });
    }

    fn on_key(&mut self, key: Key, mods: Modifiers) -> Task<Message> {
        let ctrl = mods.command() || mods.control();
        if key == Key::Named(Named::Escape) {
            if self.palette.is_some() {
                self.palette = None;
            } else {
                self.confirm = None;
            }
            return Task::none();
        }
        if self.palette.is_some() {
            let count = self
                .palette
                .as_ref()
                .map(|p| self.filtered_commands(&p.query).len())
                .unwrap_or(0);
            if let Some(p) = self.palette.as_mut() {
                match key {
                    Key::Named(Named::ArrowDown) => {
                        p.selected = (p.selected + 1).min(count.saturating_sub(1))
                    }
                    Key::Named(Named::ArrowUp) => p.selected = p.selected.saturating_sub(1),
                    _ => {}
                }
            }
            if ctrl && matches!(&key, Key::Character(c) if c.as_str() == "k") {
                self.palette = None;
            }
            return Task::none();
        }
        if self.confirm.is_some() || !ctrl {
            return Task::none();
        }
        match key {
            Key::Character(c) => match c.as_str() {
                "k" => return self.open_palette(),
                "l" => self.set_listen(!self.listen_only),
                d => {
                    if let Some(n) = d.parse::<usize>().ok().filter(|n| (1..=9).contains(n)) {
                        self.section = Section::ALL[n - 1];
                    }
                }
            },
            Key::Named(Named::Enter) if self.section == Section::Operate => {
                self.request_tx(TxAction::Send);
            }
            _ => {}
        }
        Task::none()
    }

    // ------------------------------------------------------------------- update

    pub fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.update_inner(message);
        match (self.modal_open(), self.modal_since) {
            (true, None) => {
                self.modal_since = Some(Instant::now());
                self.modal_alpha = 0.0;
            }
            (false, Some(_)) => {
                self.modal_since = None;
                self.modal_alpha = 1.0;
            }
            _ => {}
        }
        task
    }

    fn update_inner(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick(now) => {
                self.clock = now;
                self.ticks += 1;
                self.poll_engine(now);
                let live = self
                    .live_spectrum
                    .is_some_and(|t| now.duration_since(t) < Duration::from_secs(1));
                if !live {
                    self.sim.step(&mut self.bins);
                }
                self.wf.push(&self.bins);
                self.wf_handle = self.wf.handle();
                self.update_level();
                if now.duration_since(self.last_snr) >= Duration::from_secs(1) {
                    self.last_snr = now;
                    self.snr_hist.push_back(self.snr_now());
                    while self.snr_hist.len() > SNR_SECONDS {
                        self.snr_hist.pop_front();
                    }
                }
                if self.toast.as_ref().is_some_and(|t| now >= t.until) {
                    self.toast = None;
                }
            }
            Message::Anim(now) => {
                self.clock = now;
                let t = now.duration_since(self.anim_start).as_secs_f32() / PULSE_SECONDS;
                self.pulse = 0.5 - 0.5 * (std::f32::consts::TAU * t).cos();
                if self.busy.is_some_and(|(_, until)| now >= until) {
                    self.busy = None;
                }
                if let Some(since) = self.modal_since {
                    let ms = now.duration_since(since).as_millis() as f32;
                    self.modal_alpha = (ms / MODAL_FADE_MS as f32).clamp(0.0, 1.0);
                }
            }
            Message::Key(key, mods) => return self.on_key(key, mods),
            Message::Go(section) => self.section = section,
            Message::ToggleRail => self.rail_collapsed = !self.rail_collapsed,
            Message::ToggleRecord => self.recording = !self.recording,
            Message::TogglePanel => self.panel_shown = !self.panel_shown,
            Message::ToggleBand => {
                self.band_open = !self.band_open;
                self.save();
            }
            Message::SelectFile(i) => self.file_sel = Some(i),
            Message::ToggleListen(on) => self.set_listen(on),
            Message::AttachOpen(open) => {
                self.attach_open = open;
                if open {
                    self.section = Section::Operate;
                }
            }
            Message::AttachPath(p) => self.attach_path = p,
            Message::RigAddr(a) => {
                self.rig_addr = a;
                self.restart_engine();
                self.save();
            }
            Message::Rescan => self.rescan(),
            Message::OpenSetup(step) => {
                self.setup_open = true;
                self.setup_focus = step;
            }
            Message::SetupFinish => {
                self.finish_setup();
            }
            Message::SetupClose => {
                self.setup_open = false;
                self.setup_focus = None;
                self.section = Section::Settings;
            }
            Message::Tune(frac) => self.tune = frac.clamp(0.0, 1.0),
            Message::Target(t) => self.target = t.chars().take(12).collect(),
            Message::Composer(action) => match action {
                text_editor::Action::Edit(edit) => {
                    if let Some(edit) = limit_edit(self.composer_len(), edit) {
                        self.composer.perform(text_editor::Action::Edit(edit));
                    }
                }
                other => self.composer.perform(other),
            },
            Message::KindPicked(k) => self.kind = k,
            Message::RequestTx(action) => self.request_tx(action),
            Message::ConfirmTx => {
                if let Some(Confirm::Transmit(action)) = self.confirm.take() {
                    if !self.tx_intro_seen {
                        self.tx_intro_seen = true;
                        self.save();
                    }
                    self.do_tx(action);
                }
            }
            Message::CancelConfirm => {
                if self.confirm == Some(Confirm::Relay) {
                    self.relay_reset();
                }
                self.confirm = None;
                self.palette = None;
            }
            Message::RequestDisconnect => {
                if self.link.is_some() {
                    self.confirm = Some(Confirm::Disconnect);
                } else if self.connect_try.is_some() {
                    self.connect_try = None;
                    if self.is_busy(Busy::Connecting) {
                        self.busy = None;
                    }
                }
            }
            Message::ConfirmDisconnect => {
                if self.confirm.take() == Some(Confirm::Disconnect) {
                    self.link = None;
                    if let Some(e) = self.engine.as_mut() {
                        if let Err(why) = e.disconnect() {
                            self.engine_note = why;
                        }
                    }
                    self.toast(s::TOAST_UNLINKED, ToastKind::Info);
                }
            }
            Message::OpenPalette => return self.open_palette(),
            Message::PaletteQuery(q) => {
                self.palette = Some(Palette {
                    query: q,
                    selected: 0,
                });
            }
            Message::PaletteRun => {
                let selected = self.palette.as_ref().map(|p| p.selected).unwrap_or(0);
                return self.update_inner(Message::PaletteRunAt(selected));
            }
            Message::PaletteRunAt(index) => {
                let query = self.palette.take().map(|p| p.query).unwrap_or_default();
                if let Some(cmd) = self.filtered_commands(&query).into_iter().nth(index) {
                    return self.update_inner(cmd.msg);
                }
            }
            Message::StationFilter(f) => self.station_filter = f,
            Message::SelectStation(i) => {
                self.station_sel = Some(i);
                if let Some(st) = sample::STATIONS.get(i) {
                    self.target = st.call.to_string();
                }
            }
            Message::SelectConversation(i) => {
                self.conv_sel = i;
                self.msg_sel = None;
                if let Some(u) = self.conv_unread.get_mut(i) {
                    *u = 0;
                }
            }
            Message::SelectMsg(i) => self.msg_sel = Some(i),
            Message::SelectMail(i) => {
                self.mail_sel = Some(i);
                self.mail_composing = false;
                if let Some(u) = self.mail_unread.get_mut(i) {
                    *u = false;
                }
            }
            Message::MailBack => {
                self.mail_sel = None;
                self.mail_composing = false;
            }
            Message::MailCompose => {
                self.mail_composing = true;
                self.mail_sel = None;
            }
            Message::MailTo(t) => self.mail_to = t,
            Message::MailSubject(t) => self.mail_subject = t,
            Message::MailBody(action) => self.mail_body.perform(action),
            Message::MailSigned(v) => self.mail_signed = v,
            Message::MailQueue => {
                if self.my_call().is_empty() {
                    self.toast(s::BLOCK_NO_CALL, ToastKind::Warning);
                } else if self.mail_to.trim().is_empty() {
                    self.toast(s::BLOCK_MAIL_NO_TO, ToastKind::Warning);
                } else if self.listen_only {
                    self.toast(s::BLOCK_LISTEN, ToastKind::Warning);
                } else {
                    let to = self.mail_to.trim().to_uppercase();
                    let mut ok = true;
                    if !self.sample {
                        let mut plan = Vec::new();
                        if !self.linked_to(&to) {
                            plan.push(Step::Connect(to.clone()));
                        }
                        plan.push(Step::Mail(to, self.mail_subject.clone(), self.mail_body.text()));
                        for step in plan {
                            if let Err(e) = self.exec(step, "message") {
                                self.toast(&e, ToastKind::Warning);
                                ok = false;
                                break;
                            }
                        }
                    }
                    if ok {
                        if self.sample {
                            self.outbox += 1;
                        }
                        self.mail_to.clear();
                        self.mail_subject.clear();
                        self.mail_body = text_editor::Content::new();
                        self.mail_composing = false;
                        self.toast(s::TOAST_MAIL, ToastKind::Info);
                    }
                }
            }
            Message::File(i, cmd) => {
                let listen_only = self.listen_only;
                if let Some(f) = self.files.get_mut(i) {
                    use sample::FileState::*;
                    if cmd == FileCmd::Start && f.sending && listen_only {
                        self.toast(s::BLOCK_START_FILE, ToastKind::Warning);
                        return Task::none();
                    }
                    f.state = match (cmd, f.state) {
                        (FileCmd::Start, Waiting | Paused) => Active,
                        (FileCmd::Pause, Active) => Paused,
                        (FileCmd::Cancel, Done) => Done,
                        (FileCmd::Cancel, _) => Cancelled,
                        (_, other) => other,
                    };
                }
            }
            Message::RelayAccept => self.toast(s::TOAST_SCHEDULE_ACCEPTED, ToastKind::Info),
            Message::RelayDecline => self.toast(s::TOAST_SCHEDULE_DECLINED, ToastKind::Info),
            Message::LogSearch(q) => self.log_search = q,
            Message::LogExport => self.toast(s::LOG_EXPORT_DONE, ToastKind::Warning),
            Message::OpenMap => {
                if open_in_browser(s::MAP_URL).is_err() {
                    self.toast(s::TOAST_MAP_FAIL, ToastKind::Warning);
                }
            }
            Message::Callsign(c) => {
                self.callsign = c
                    .to_uppercase()
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .take(12)
                    .collect();
                self.restart_engine();
                self.save();
            }
            Message::RoleChosen(r) => {
                if r.is_relay() && !self.role.is_relay() {
                    // Starting relay mode always asks, every time.
                    self.relay_reset();
                    self.relay_role = Some(r);
                    self.confirm = Some(Confirm::Relay);
                } else {
                    if !r.is_relay() {
                        if let Some(e) = self.engine.as_ref() {
                            e.stop_relay();
                        }
                        self.relay_reset();
                    }
                    self.role = r;
                }
            }
            Message::RelayLicence(v) => self.relay_licence = v,
            Message::RelayTerms(v) => self.relay_terms = v,
            Message::RelayStart => {
                if self.confirm == Some(Confirm::Relay) && self.relay_licence && self.relay_terms {
                    if let Some(r) = self.relay_role {
                        self.start_relay(r);
                    }
                    self.confirm = None;
                    self.relay_reset();
                }
            }
            Message::AudioIn(d) => {
                self.audio_in = d;
                self.restart_engine();
                self.save();
            }
            Message::BandChosen(b) => {
                self.band = b;
                spectrum::set_centre_hz(b.centre_hz());
                self.tune = 0.5;
            }
            Message::VolIn(v) => {
                self.vol_in = v;
                self.save();
            }
            Message::VolOut(v) => {
                self.vol_out = v;
                self.save();
            }
            Message::AudioOut(d) => {
                self.audio_out = d;
                self.restart_engine();
                self.save();
            }
            Message::RigChosen(r) => {
                self.rig = r;
                self.restart_engine();
                self.save();
            }
            Message::SdrChosen(d) => {
                self.sdr = d;
                self.restart_engine();
                self.save();
            }
            Message::LocationChosen(l) => self.location = l,
            Message::Radius(r) => self.radius_km = r,
            Message::InternetRelay(v) => {
                self.internet_relay = v;
                if let Some(e) = &self.engine {
                    e.set_internet_relay(v);
                }
            }
            Message::ShowSample(on) => self.load_sample(on),
            Message::Profile(p) => self.profile = p,
            Message::Level(v) => self.level = v,
            Message::CheckUpdates => self.toast(s::SET_CHECK_DONE, ToastKind::Info),
        }
        Task::none()
    }
}

/// Where the UI state is remembered between runs.
fn state_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("KAHF_UI_STATE") {
        return Some(p.into());
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config").join("kahf").join("ui-state"))
}

fn default_data_dir() -> PathBuf {
    if let Ok(p) = std::env::var("KAHF_DATA_DIR") {
        return p.into();
    }
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".config").join("kahf").join("data"))
        .unwrap_or_else(|| PathBuf::from("kahf-data"))
}

fn load_state() -> std::collections::HashMap<String, String> {
    let text = state_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    text.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

fn save_state(pairs: &[(&'static str, String)]) {
    if let Some(p) = state_path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let text: String = pairs
            .iter()
            .map(|(k, v)| format!("{k}={}\n", v.replace('\n', " ")))
            .collect();
        let _ = std::fs::write(p, text);
    }
}

fn open_in_browser(url: &str) -> std::io::Result<()> {
    use std::process::Command;
    #[cfg(target_os = "macos")]
    let mut cmd = Command::new("open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = Command::new("xdg-open");
    cmd.arg(url).spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> App {
        let mut app = App::init();
        app.callsign = "XX0AB".into();
        app.target = "XX1XX".into();
        app
    }

    fn char_key(c: &str, mods: Modifiers) -> Message {
        Message::Key(Key::Character(c.into()), mods)
    }

    #[test]
    fn starts_in_listen_only() {
        assert!(App::init().listen_only);
    }

    #[test]
    fn listen_only_blocks_every_transmit() {
        let mut app = ready();
        for action in [TxAction::Connect, TxAction::Send, TxAction::Beacon] {
            let _ = app.update(Message::RequestTx(action));
            assert!(app.confirm.is_none(), "{action:?} opened a confirmation");
        }
        // Even a stale confirmation cannot transmit while Listen only is on.
        app.confirm = Some(Confirm::Transmit(TxAction::Beacon));
        let _ = app.update(Message::ConfirmTx);
        assert_eq!(app.outbox, 0);
        assert!(app.link.is_none());
    }

    #[test]
    fn transmit_needs_confirmation() {
        let mut app = ready();
        app.listen_only = false;
        app.dry_run = true;
        let _ = app.update(Message::RequestTx(TxAction::Connect));
        assert_eq!(app.confirm, Some(Confirm::Transmit(TxAction::Connect)));
        assert!(app.steps.is_empty(), "nothing runs before confirmation");
        let _ = app.update(Message::ConfirmTx);
        assert_eq!(app.steps, vec![Step::Connect("XX1XX".into())]);
        assert!(app.confirm.is_none());
    }

    #[test]
    fn escape_cancels_confirmation_and_palette() {
        let mut app = ready();
        app.listen_only = false;
        let _ = app.update(Message::RequestTx(TxAction::Connect));
        assert!(app.confirm.is_some());
        let _ = app.update(Message::Key(Key::Named(Named::Escape), Modifiers::default()));
        assert!(app.confirm.is_none());
        assert_eq!(app.outbox, 0);
        let _ = app.update(Message::OpenPalette);
        assert!(app.palette.is_some());
        let _ = app.update(Message::Key(Key::Named(Named::Escape), Modifiers::default()));
        assert!(app.palette.is_none());
    }

    #[test]
    fn disconnect_asks_first() {
        let mut app = ready();
        app.link = Some("XX1XX".into());
        let _ = app.update(Message::RequestDisconnect);
        assert_eq!(app.confirm, Some(Confirm::Disconnect));
        assert!(app.link.is_some());
        let _ = app.update(Message::ConfirmDisconnect);
        assert!(app.link.is_none());
    }

    #[test]
    fn send_connects_first_when_not_linked() {
        let mut app = ready();
        app.listen_only = false;
        app.dry_run = true;
        let _ = app.update(Message::Composer(text_editor::Action::Edit(
            text_editor::Edit::Insert('a'),
        )));
        let _ = app.update(Message::RequestTx(TxAction::Send));
        assert_eq!(app.confirm, Some(Confirm::Transmit(TxAction::Send)));
        let _ = app.update(Message::ConfirmTx);
        assert_eq!(app.steps.len(), 2);
        assert_eq!(app.steps[0], Step::Connect("XX1XX".into()));
        assert!(matches!(app.steps[1], Step::Message(..)));
    }

    #[test]
    fn send_without_text_is_refused() {
        let mut app = ready();
        app.listen_only = false;
        let _ = app.update(Message::RequestTx(TxAction::Send));
        assert!(app.confirm.is_none());
    }

    #[test]
    fn message_is_limited_to_500_characters() {
        let mut app = ready();
        for _ in 0..(s::MESSAGE_LIMIT + 20) {
            let _ = app.update(Message::Composer(text_editor::Action::Edit(
                text_editor::Edit::Insert('x'),
            )));
        }
        assert_eq!(app.composer_len(), s::MESSAGE_LIMIT);
        let long = Arc::new("y".repeat(100));
        let _ = app.update(Message::Composer(text_editor::Action::Edit(
            text_editor::Edit::Paste(long),
        )));
        assert_eq!(app.composer_len(), s::MESSAGE_LIMIT);
    }

    #[test]
    fn paste_is_cut_to_the_room_left() {
        let kept = limit_edit(490, text_editor::Edit::Paste(Arc::new("z".repeat(50))));
        match kept {
            Some(text_editor::Edit::Paste(t)) => assert_eq!(t.chars().count(), 10),
            other => panic!("unexpected {other:?}"),
        }
        assert!(limit_edit(500, text_editor::Edit::Enter).is_none());
        assert!(limit_edit(500, text_editor::Edit::Backspace).is_some());
    }

    #[test]
    fn shortcuts() {
        let mut app = ready();
        let ctrl = Modifiers::CTRL;
        let _ = app.update(char_key("3", ctrl));
        assert_eq!(app.section, Section::Messages);
        let _ = app.update(char_key("9", ctrl));
        assert_eq!(app.section, Section::Settings);
        let _ = app.update(char_key("l", ctrl));
        assert!(!app.listen_only);
        let _ = app.update(char_key("k", ctrl));
        assert!(app.palette.is_some());
        // Section shortcuts are ignored while a modal is open.
        let _ = app.update(char_key("1", ctrl));
        assert_eq!(app.section, Section::Settings);
    }

    #[test]
    fn palette_filters_and_runs() {
        let mut app = ready();
        let _ = app.update(Message::OpenPalette);
        let _ = app.update(Message::PaletteQuery("go logbook".into()));
        let hits = app.filtered_commands("go logbook");
        assert_eq!(hits.len(), 1);
        let _ = app.update(Message::PaletteRun);
        assert_eq!(app.section, Section::Logbook);
        assert!(app.palette.is_none());
    }

    #[test]
    fn relay_start_asks_every_time_and_needs_both_ticks() {
        let mut app = App::init();
        let _ = app.update(Message::RoleChosen(Role::Relay));
        assert_eq!(app.confirm, Some(Confirm::Relay));
        assert!(!app.role.is_relay());
        // One tick is not enough.
        let _ = app.update(Message::RelayLicence(true));
        let _ = app.update(Message::RelayStart);
        assert_eq!(app.confirm, Some(Confirm::Relay));
        // Cancel leaves relay off and clears the ticks.
        let _ = app.update(Message::CancelConfirm);
        assert!(app.confirm.is_none() && !app.role.is_relay() && !app.relay_licence);
        // Asked again, boxes empty.
        let _ = app.update(Message::RoleChosen(Role::Relay));
        assert_eq!(app.confirm, Some(Confirm::Relay));
        assert!(!app.relay_licence && !app.relay_terms);
    }

    #[test]
    fn segments_are_found_and_outside_is_reported() {
        assert!(spectrum::segment_at(7_042_000.0).is_some());
        let _ = App::init().update(Message::BandChosen(spectrum::Band::M20));
        assert!(!spectrum::channels().is_empty());
        let _ = App::init().update(Message::BandChosen(spectrum::Band::M40));
        assert!(spectrum::segment_at(14_105_000.0).is_some());
        assert!(spectrum::segment_at(7_050_000.0).is_none());
        assert_eq!(spectrum::segment_text(7_050_000.0), "No segment");
        assert!(spectrum::channels()
            .iter()
            .all(|(hz, _)| spectrum::segment_at(*hz).is_some()));
    }

    #[test]
    fn relay_section_exists_for_every_role() {
        assert_eq!(Section::ALL[5], Section::Relay);
        assert_eq!(Section::ALL.len(), 9);
    }

    #[test]
    fn sample_switch_empties_and_fills() {
        let mut app = ready();
        let _ = app.update(Message::ShowSample(true));
        assert!(!app.files.is_empty());
        let _ = app.update(Message::ShowSample(false));
        assert!(app.files.is_empty() && app.thread.is_empty());
        let _ = app.update(Message::ShowSample(true));
        assert!(!app.files.is_empty() && !app.thread.is_empty());
    }
}
