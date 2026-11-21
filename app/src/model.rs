//! The one place that holds what the window shows, and how engine events change it. It never
//! talks to the engine: the app layer does that, and calls the methods here with the results.

use serde::{Deserialize, Serialize};

use crate::types::*;
use crate::util::{bands, callsign};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Welcome,
    Setup,
    Chats,
    Mail,
    Stations,
    Radio,
    Relay,
    Logbook,
}

impl Page {
    /// Chats and Mail live under Messages; Stations, Relays and the Logbook under Stations.
    pub fn group(self) -> Option<&'static [Page]> {
        match self {
            Page::Chats | Page::Mail => Some(&[Page::Chats, Page::Mail]),
            Page::Stations | Page::Relay | Page::Logbook => {
                Some(&[Page::Stations, Page::Relay, Page::Logbook])
            }
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Page::Welcome => "Welcome",
            Page::Setup => "Setup",
            Page::Chats => "Chats",
            Page::Mail => "Mail",
            Page::Stations => "Stations and map",
            Page::Radio => "Radio",
            Page::Relay => "Relays",
            Page::Logbook => "Logbook",
        }
    }

    pub fn full_screen(self) -> bool {
        matches!(self, Page::Welcome | Page::Setup)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsTab {
    #[default]
    Station,
    Radio,
    Location,
    Sending,
    Relay,
    Look,
    About,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Good,
    Warn,
}

/// A message for the person, shown by the window as a toast.
#[derive(Debug, Clone, PartialEq)]
pub struct Toast {
    pub text: String,
    pub kind: ToastKind,
}

/// One step of the "getting started" list.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub id: &'static str,
    pub title: &'static str,
    pub why: &'static str,
    pub done: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Idle,
    Good,
    Busy,
    Warn,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub tone: Tone,
    pub title: String,
    pub detail: String,
}

/// Files waiting for the person to confirm, after the warning about time and hops.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingFiles {
    pub to: String,
    pub files: Vec<PickedFile>,
}

/// What survives a restart: the conversations, the mail and who was heard.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    pub messages: Vec<Msg>,
    pub mail: Vec<MailItem>,
    pub heard: Vec<Heard>,
    pub support: Support,
}

/// When to ask, gently, if the person would like to support KAHF.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Support {
    /// Unix milliseconds of the first start, 0 before it is known.
    pub first: i64,
    /// Unix milliseconds of the last time the note was shown and closed.
    pub last: i64,
    /// The person said not to ask again.
    pub off: bool,
}

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

impl Support {
    /// Not in the first week, then at most once a month, and never after "don't ask again".
    pub fn due(&self, now_ms: i64) -> bool {
        !self.off
            && self.first > 0
            && now_ms - self.first >= 7 * DAY_MS
            && (self.last == 0 || now_ms - self.last >= 30 * DAY_MS)
    }
}

const KEEP_MESSAGES: usize = 500;
const KEEP_MAIL: usize = 300;
const KEEP_HEARD: usize = 80;
const WATERFALL_ROWS: usize = 90;

/// Turns what the engine said into advice.
pub fn explain_start_error(raw: &str) -> String {
    let t = raw.to_lowercase();
    if t.contains("audio") || t.contains("device") || t.contains("sound") {
        "KAHF could not open the sound device. Check that your radio interface is plugged in and not used by another program, then choose it again on the Radio page.".into()
    } else if t.contains("sdr") || t.contains("hackrf") || t.contains("rtl") {
        "KAHF could not find the SDR receiver. Plug it in, make sure its driver is installed, and try again."
            .into()
    } else if t.contains("rig") || t.contains("rigctld") || t.contains("4532") {
        "KAHF could not reach the radio control program. Check the radio on the Radio page, or leave radio control off.".into()
    } else {
        format!("The radio could not start. {raw}")
    }
}

pub struct Model {
    pub page: Page,
    pub settings: Settings,
    pub link: LinkState,
    pub heard: Vec<Heard>,
    pub messages: Vec<Msg>,
    pub mail: Vec<MailItem>,
    pub metrics: Option<Metrics>,
    pub log: Vec<(i64, String)>,
    pub spectrum: Vec<f32>,
    /// The last few seconds of spectrum, newest first, for the waterfall.
    pub waterfall: std::collections::VecDeque<Vec<f32>>,
    pub transmitting: bool,
    pub receiving: bool,
    pub toasts: Vec<Toast>,
    pub tour_open: bool,
    pub settings_open: bool,
    pub settings_tab: SettingsTab,
    pub help_open: bool,
    /// Why the radio could not start, in plain words. Empty when all is well.
    pub engine_error: String,
    pub recording: bool,
    pub record_level: f32,
    pub relay: RelayStatus,
    pub tx_gate: TxGate,
    pub relays: Vec<RelayInfo>,
    pub book: Logbook,
    pub info: AppInfo,
    /// How strong the signal in the KAHF channel is, 0 to 1, smoothed. Drives the meter.
    pub signal: f32,
    pub pending_files: Option<PendingFiles>,
    pub support: Support,
    pub ready: bool,
    last_signal_ms: i64,
    next_id: u64,
}

impl Default for Model {
    fn default() -> Self {
        Model {
            page: Page::Welcome,
            settings: Settings::default(),
            link: LinkState::Idle,
            heard: Vec::new(),
            messages: Vec::new(),
            mail: Vec::new(),
            metrics: None,
            log: Vec::new(),
            spectrum: Vec::new(),
            waterfall: std::collections::VecDeque::new(),
            transmitting: false,
            receiving: false,
            toasts: Vec::new(),
            tour_open: false,
            settings_open: false,
            settings_tab: SettingsTab::Station,
            help_open: false,
            engine_error: String::new(),
            recording: false,
            record_level: 0.0,
            relay: RelayStatus::default(),
            tx_gate: TxGate::default(),
            relays: Vec::new(),
            book: Logbook::default(),
            info: AppInfo::default(),
            signal: 0.0,
            pending_files: None,
            support: Support::default(),
            ready: false,
            last_signal_ms: 0,
            next_id: 0,
        }
    }
}

impl Model {
    // ---- derived ---------------------------------------------------------------------------

    pub fn has_call(&self) -> bool {
        callsign::valid(&self.settings.callsign)
    }

    pub fn unread_chats(&self) -> usize {
        self.messages
            .iter()
            .filter(|m| m.dir == Dir::In && !m.read)
            .count()
    }

    pub fn unread_mail(&self) -> usize {
        self.mail.iter().filter(|m| m.dir == Dir::In && !m.read).count()
    }

    pub fn steps(&self) -> Vec<Step> {
        let s = &self.settings;
        vec![
            Step {
                id: "call",
                title: "Your callsign is set",
                why: "It is your name on the air.",
                done: self.has_call(),
            },
            Step {
                id: "radio",
                title: "KAHF can hear your radio",
                why: "A sound card or receiver is chosen.",
                done: s.setup_done && (!s.audio_in.is_empty() || !s.sdr.is_empty()),
            },
            Step {
                id: "listen",
                title: "You have heard a station",
                why: "Leave KAHF running. Stations appear on their own.",
                done: !self.heard.is_empty(),
            },
            Step {
                id: "send",
                title: "You have sent a message",
                why: "Pick a station, write a line, press Send.",
                done: self
                    .messages
                    .iter()
                    .any(|m| m.dir == Dir::Out && m.state == Some(OutState::Sent)),
            },
        ]
    }

    /// What the radio is doing, in one short line: shown in the strip on every screen.
    pub fn status(&self) -> Status {
        let s = |tone, title: &str, detail: &str| Status {
            tone,
            title: title.into(),
            detail: detail.into(),
        };
        if !self.has_call() && !self.settings.listen_only {
            return s(Tone::Warn, "Not set up", "Enter your callsign to start.");
        }
        if !self.engine_error.is_empty() {
            return s(Tone::Warn, "Radio not running", &self.engine_error);
        }
        if self.settings.listen_only {
            return s(Tone::Idle, "Listening only", "KAHF will not transmit.");
        }
        if self.transmitting {
            return s(Tone::Busy, "Sending", "Your signal is on the air.");
        }
        match &self.link {
            LinkState::Calling { peer, attempt, of } => {
                let detail = if *of > 0 {
                    format!("Try {attempt} of {of}")
                } else {
                    "KAHF keeps trying by itself".into()
                };
                return Status {
                    tone: Tone::Busy,
                    title: format!("Reaching {peer}"),
                    detail,
                };
            }
            LinkState::Linked { peer } => {
                return Status {
                    tone: Tone::Good,
                    title: format!("Linked to {peer}"),
                    detail: "Messages go straight to them.".into(),
                };
            }
            LinkState::Idle => {}
        }
        if self.receiving {
            return s(Tone::Good, "Receiving", "A signal is on the channel.");
        }
        let n = self.heard.len();
        Status {
            tone: Tone::Idle,
            title: "Listening".into(),
            detail: if n == 0 {
                "Nobody heard yet.".into()
            } else {
                format!("{n} heard")
            },
        }
    }

    /// The speed to base estimates on: the live link when there is one, else the starting speed.
    pub fn estimate_bps(&self) -> f64 {
        self.metrics
            .as_ref()
            .map(|m| m.net_bps)
            .filter(|b| *b > 0.0)
            .unwrap_or(284.0)
    }

    /// The people you have written to or heard from in conversations, most recent first.
    pub fn conversations(&self) -> Vec<Conversation> {
        let mut out: Vec<Conversation> = Vec::new();
        for m in &self.messages {
            match out.iter_mut().find(|c| c.peer == m.peer) {
                Some(c) => {
                    c.last_at = m.at;
                    c.last_text = preview(m);
                    c.last_out = m.dir == Dir::Out;
                    if m.dir == Dir::In && !m.read {
                        c.unread += 1;
                    }
                }
                None => out.push(Conversation {
                    peer: m.peer.clone(),
                    last_at: m.at,
                    last_text: preview(m),
                    last_out: m.dir == Dir::Out,
                    unread: usize::from(m.dir == Dir::In && !m.read),
                }),
            }
        }
        out.sort_by_key(|c| std::cmp::Reverse(c.last_at));
        out
    }

    pub fn thread(&self, peer: &str) -> Vec<&Msg> {
        self.messages.iter().filter(|m| m.peer == peer).collect()
    }

    /// Heard stations you have not talked to yet.
    pub fn on_the_air(&self) -> Vec<&Heard> {
        let talked: Vec<String> = self.conversations().into_iter().map(|c| c.peer).collect();
        self.heard.iter().filter(|h| !talked.contains(&h.call)).collect()
    }

    // ---- state changes ---------------------------------------------------------------------

    pub fn new_id(&mut self, now_ms: i64) -> String {
        self.next_id += 1;
        format!("{now_ms}-{}", self.next_id)
    }

    pub fn go(&mut self, page: Page) {
        self.page = page;
        match page {
            Page::Chats => self.mark_chats_read(),
            Page::Mail => self.mark_mail_read(),
            _ => {}
        }
    }

    pub fn open_settings(&mut self, tab: SettingsTab) {
        self.settings_tab = tab;
        self.settings_open = true;
    }

    /// Queues a toast; the window shows it and takes it off the queue.
    pub fn toast(&mut self, text: impl Into<String>, kind: ToastKind, _now_ms: i64) {
        self.toasts.push(Toast {
            text: text.into(),
            kind,
        });
    }

    pub fn add_out(&mut self, m: Msg) {
        self.messages.push(m);
        self.trim();
    }

    pub fn update_msg(&mut self, id: &str, f: impl FnOnce(&mut Msg)) {
        if let Some(m) = self.messages.iter_mut().find(|m| m.id == id) {
            f(m);
        }
    }

    pub fn update_mail(&mut self, id: &str, f: impl FnOnce(&mut MailItem)) {
        if let Some(m) = self.mail.iter_mut().find(|m| m.id == id) {
            f(m);
        }
    }

    pub fn fail_msg(&mut self, id: &str, why: String) {
        self.update_msg(id, |m| {
            m.state = Some(OutState::Failed);
            m.note = Some(why);
        });
    }

    pub fn mark_chats_read(&mut self) {
        for m in self.messages.iter_mut().filter(|m| m.dir == Dir::In) {
            m.read = true;
        }
    }

    pub fn mark_mail_read(&mut self) {
        for m in self.mail.iter_mut().filter(|m| m.dir == Dir::In) {
            m.read = true;
        }
    }

    pub fn discard(&mut self, id: &str) {
        self.messages.retain(|m| m.id != id);
    }

    fn trim(&mut self) {
        let excess = self.messages.len().saturating_sub(KEEP_MESSAGES);
        self.messages.drain(..excess);
        let excess = self.mail.len().saturating_sub(KEEP_MAIL);
        self.mail.drain(..excess);
        self.heard.truncate(KEEP_HEARD);
    }

    pub fn saved(&self) -> Saved {
        Saved {
            messages: self.messages.clone(),
            mail: self.mail.clone(),
            heard: self.heard.clone(),
            support: self.support.clone(),
        }
    }

    pub fn restore(&mut self, saved: Saved) {
        self.messages = saved.messages;
        self.mail = saved.mail;
        self.heard = saved.heard;
        self.support = saved.support;
        self.trim();
    }

    // ---- events from the engine --------------------------------------------------------------

    /// Applies one engine event. `now_ms` is the current time in unix milliseconds.
    pub fn handle(&mut self, e: EngineEvent, now_ms: i64) {
        match e {
            EngineEvent::Link { link } => {
                if let LinkState::Calling { peer, .. } = &link {
                    let peer = peer.clone();
                    for m in self.messages.iter_mut() {
                        if m.dir == Dir::Out
                            && m.peer == peer
                            && matches!(m.state, Some(OutState::Queued) | Some(OutState::Calling))
                        {
                            m.state = Some(OutState::Calling);
                        }
                    }
                }
                self.link = link;
            }
            EngineEvent::Message { from, text, at } => {
                let read = self.page == Page::Chats;
                let id = format!("in-{at}-{}", self.messages.len());
                self.messages.push(Msg {
                    id,
                    peer: from.clone(),
                    dir: Dir::In,
                    text,
                    at,
                    kind: MessageKind::Chat,
                    state: None,
                    attachment: None,
                    read,
                    note: None,
                });
                self.trim();
                if !read {
                    self.toast(format!("New message from {from}"), ToastKind::Info, now_ms);
                }
            }
            EngineEvent::Mail {
                from,
                subject,
                body,
                at,
                files,
            } => {
                let read = self.page == Page::Mail;
                let id = format!("in-{at}-{}", self.mail.len());
                self.mail.push(MailItem {
                    id,
                    peer: from.clone(),
                    dir: Dir::In,
                    subject: subject.clone(),
                    body,
                    at,
                    files,
                    state: None,
                    note: None,
                    read,
                });
                self.trim();
                if !read {
                    self.toast(
                        format!("New mail from {from}: {subject}"),
                        ToastKind::Info,
                        now_ms,
                    );
                }
            }
            EngineEvent::File {
                name,
                done,
                total,
                dir,
                id,
                from,
                path,
            } => match dir {
                Dir::Out => self.update_msg(&id, |m| {
                    if let Some(a) = m.attachment.as_mut() {
                        a.done = Some(done);
                    }
                }),
                // A file or voice note that arrived: shown in the conversation with the sender.
                Dir::In => {
                    let Some(from) = from.filter(|_| done >= total) else {
                        return;
                    };
                    let voice = name.to_lowercase().ends_with(".kvm");
                    let read = self.page == Page::Chats;
                    let seconds =
                        voice.then(|| ((((total as f64 - 4.0) / 150.0) * 10.0).round() / 10.0).max(1.0));
                    self.messages.push(Msg {
                        id: format!("in-file-{now_ms}-{}", self.messages.len()),
                        peer: from.clone(),
                        dir: Dir::In,
                        text: String::new(),
                        at: now_ms,
                        kind: MessageKind::Chat,
                        state: None,
                        attachment: Some(Attachment {
                            kind: if voice {
                                AttachmentKind::Voice
                            } else {
                                AttachmentKind::File
                            },
                            name: if voice { "Voice note".into() } else { name },
                            size: total,
                            path,
                            seconds,
                            done: Some(total),
                        }),
                        read,
                        note: None,
                    });
                    self.trim();
                    if !read {
                        let what = if voice { "Voice note" } else { "File" };
                        self.toast(format!("{what} from {from}"), ToastKind::Info, now_ms);
                    }
                }
            },
            EngineEvent::Heard { heard } => {
                let old = self.heard.iter().find(|h| h.call == heard.call).cloned();
                self.heard.retain(|h| h.call != heard.call);
                // A plain "heard" must not wipe what an announcement told us about the station.
                let merged = Heard {
                    lat: heard.lat.or(old.as_ref().and_then(|o| o.lat)),
                    lon: heard.lon.or(old.as_ref().and_then(|o| o.lon)),
                    relay: heard.relay.or(old.as_ref().and_then(|o| o.relay)),
                    online: heard.online.or(old.as_ref().and_then(|o| o.online)),
                    ..heard
                };
                self.heard.insert(0, merged);
                self.heard.truncate(KEEP_HEARD);
            }
            EngineEvent::Metrics { metrics } => self.metrics = Some(metrics),
            EngineEvent::Spectrum { bins } => {
                self.update_signal(&bins, now_ms);
                self.waterfall.push_front(bins.clone());
                self.waterfall.truncate(WATERFALL_ROWS);
                self.spectrum = bins;
            }
            EngineEvent::Level { level } => self.record_level = level,
            EngineEvent::Relay { status } => self.relay = status,
            EngineEvent::Tx { state, .. } => {
                self.transmitting = state == "starting" || state == "sending";
                if state == "sending" {
                    for m in self.messages.iter_mut() {
                        if m.dir == Dir::Out
                            && matches!(m.state, Some(OutState::Queued) | Some(OutState::Calling))
                        {
                            m.state = Some(OutState::Sending);
                        }
                    }
                    for m in self.mail.iter_mut() {
                        if m.dir == Dir::Out
                            && matches!(m.state, Some(OutState::Queued) | Some(OutState::Calling))
                        {
                            m.state = Some(OutState::Sending);
                        }
                    }
                }
            }
            EngineEvent::Sent { id } => {
                if self.mail.iter().any(|m| m.id == id) {
                    self.update_mail(&id, |m| {
                        m.state = Some(OutState::Sent);
                        m.note = None;
                    });
                } else {
                    self.update_msg(&id, |m| {
                        m.state = Some(OutState::Sent);
                        m.note = None;
                    });
                }
            }
            EngineEvent::Failed { id, reason } => {
                if self.mail.iter().any(|m| m.id == id) {
                    self.update_mail(&id, |m| {
                        m.state = Some(OutState::Failed);
                        m.note = Some(reason.clone());
                    });
                } else {
                    self.fail_msg(&id, reason.clone());
                }
                self.toast(reason, ToastKind::Warn, now_ms);
            }
            EngineEvent::Log { text } => {
                self.log.insert(0, (now_ms, text));
                self.log.truncate(100);
            }
        }
    }

    /// A signal inside the KAHF channel (0.3 to 2.7 kHz) well above the floor lights the meter.
    fn update_signal(&mut self, bins: &[f32], now_ms: i64) {
        let n = bins.len();
        if n == 0 {
            return;
        }
        let lo = (300 * n) / 4000;
        let hi = ((2700 * n) / 4000).max(lo + 1).min(n);
        let band = &bins[lo.min(hi - 1)..hi];
        let mean = band.iter().sum::<f32>() / band.len().max(1) as f32;
        let mut sorted = bins.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let floor = sorted[(n as f32 * 0.2) as usize];
        let above = mean - floor;
        if above > 12.0 {
            self.last_signal_ms = now_ms;
        }
        self.receiving = now_ms - self.last_signal_ms < 1500;
        let target = (above / 40.0).clamp(0.0, 1.0);
        self.signal = self.signal * 0.7 + target * 0.3;
    }

    /// Chooses the first page after startup.
    pub fn first_page(&self) -> Page {
        if self.settings.setup_done && (self.has_call() || self.settings.listen_only) {
            Page::Chats
        } else {
            Page::Welcome
        }
    }

    /// A frequency saved by an older version becomes the first 17 m channel.
    pub fn fix_frequency(&mut self) {
        self.settings.freq_khz = bands::khz_or_default(self.settings.freq_khz);
    }
}

fn preview(m: &Msg) -> String {
    if !m.text.is_empty() {
        return m.text.clone();
    }
    match &m.attachment {
        Some(a) if a.kind == AttachmentKind::Voice => "Voice note".into(),
        Some(a) => a.name.clone(),
        None => String::new(),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Conversation {
    pub peer: String,
    pub last_at: i64,
    pub last_text: String,
    pub last_out: bool,
    pub unread: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: &str, peer: &str, dir: Dir, at: i64) -> Msg {
        Msg {
            id: id.into(),
            peer: peer.into(),
            dir,
            text: "hi".into(),
            at,
            kind: MessageKind::Chat,
            state: (dir == Dir::Out).then_some(OutState::Queued),
            attachment: None,
            read: dir == Dir::Out,
            note: None,
        }
    }

    #[test]
    fn a_new_message_is_unread_and_toasted_unless_chats_is_open() {
        let mut m = Model {
            page: Page::Radio,
            ..Model::default()
        };
        m.handle(
            EngineEvent::Message {
                from: "K1AB".into(),
                text: "hello".into(),
                at: 5,
            },
            100,
        );
        assert_eq!(m.unread_chats(), 1);
        assert_eq!(m.toasts.len(), 1);
        m.go(Page::Chats);
        assert_eq!(m.unread_chats(), 0);
        m.handle(
            EngineEvent::Message {
                from: "K1AB".into(),
                text: "again".into(),
                at: 6,
            },
            100,
        );
        assert_eq!(m.unread_chats(), 0);
        assert_eq!(m.toasts.len(), 1);
    }

    #[test]
    fn a_queued_message_follows_the_link_and_the_air() {
        let mut m = Model::default();
        m.add_out(msg("a", "K1AB", Dir::Out, 1));
        m.add_out(msg("b", "W2CD", Dir::Out, 2));
        m.handle(
            EngineEvent::Link {
                link: LinkState::Calling {
                    peer: "K1AB".into(),
                    attempt: 1,
                    of: 3,
                },
            },
            0,
        );
        assert_eq!(m.messages[0].state, Some(OutState::Calling));
        assert_eq!(m.messages[1].state, Some(OutState::Queued));
        m.handle(
            EngineEvent::Tx {
                state: "sending".into(),
                reason: None,
            },
            0,
        );
        assert!(m.transmitting);
        assert_eq!(m.messages[0].state, Some(OutState::Sending));
        m.handle(EngineEvent::Sent { id: "a".into() }, 0);
        assert_eq!(m.messages[0].state, Some(OutState::Sent));
        m.handle(
            EngineEvent::Failed {
                id: "b".into(),
                reason: "No answer".into(),
            },
            9,
        );
        assert_eq!(m.messages[1].state, Some(OutState::Failed));
        assert_eq!(m.messages[1].note.as_deref(), Some("No answer"));
        assert_eq!(m.toasts.last().unwrap().kind, ToastKind::Warn);
    }

    #[test]
    fn a_plain_heard_keeps_what_an_announcement_said() {
        let mut m = Model::default();
        m.handle(
            EngineEvent::Heard {
                heard: Heard {
                    call: "K1AB".into(),
                    at: 1,
                    lat: Some(1.0),
                    lon: Some(2.0),
                    relay: Some(true),
                    ..Default::default()
                },
            },
            0,
        );
        m.handle(
            EngineEvent::Heard {
                heard: Heard {
                    call: "K1AB".into(),
                    at: 2,
                    snr: Some(4.0),
                    ..Default::default()
                },
            },
            0,
        );
        assert_eq!(m.heard.len(), 1);
        let h = &m.heard[0];
        assert_eq!(
            (h.at, h.lat, h.relay, h.snr),
            (2, Some(1.0), Some(true), Some(4.0))
        );
    }

    #[test]
    fn a_received_voice_note_lands_in_the_conversation() {
        let mut m = Model::default();
        m.handle(
            EngineEvent::File {
                name: "x.kvm".into(),
                done: 154,
                total: 154,
                dir: Dir::In,
                id: "f".into(),
                from: Some("K1AB".into()),
                path: Some("/in/x.kvm".into()),
            },
            50,
        );
        let a = m.messages[0].attachment.as_ref().unwrap();
        assert_eq!(a.kind, AttachmentKind::Voice);
        assert_eq!(a.seconds, Some(1.0));
        // A transfer still in progress shows nothing yet.
        m.handle(
            EngineEvent::File {
                name: "y.txt".into(),
                done: 1,
                total: 9,
                dir: Dir::In,
                id: "g".into(),
                from: Some("K1AB".into()),
                path: None,
            },
            50,
        );
        assert_eq!(m.messages.len(), 1);
    }

    #[test]
    fn conversations_list_the_latest_first_and_count_unread() {
        let mut m = Model {
            messages: vec![
                msg("1", "K1AB", Dir::In, 10),
                msg("2", "W2CD", Dir::Out, 20),
                msg("3", "K1AB", Dir::In, 30),
            ],
            ..Model::default()
        };
        let c = m.conversations();
        assert_eq!(c[0].peer, "K1AB");
        assert_eq!(c[0].unread, 2);
        assert_eq!(c[1].peer, "W2CD");
        assert_eq!(m.thread("K1AB").len(), 2);
        m.heard = vec![
            Heard {
                call: "K1AB".into(),
                ..Default::default()
            },
            Heard {
                call: "N3EF".into(),
                ..Default::default()
            },
        ];
        assert_eq!(m.on_the_air().len(), 1);
    }

    #[test]
    fn a_strong_signal_lights_the_meter_and_holds_for_a_moment() {
        let mut m = Model::default();
        let mut bins = vec![-90.0_f32; 100];
        for b in &mut bins[10..60] {
            *b = -40.0;
        }
        m.handle(EngineEvent::Spectrum { bins: bins.clone() }, 1000);
        assert!(m.receiving && m.signal > 0.0);
        m.handle(
            EngineEvent::Spectrum {
                bins: vec![-90.0; 100],
            },
            1500,
        );
        assert!(m.receiving, "holds for 1.5 s");
        m.handle(
            EngineEvent::Spectrum {
                bins: vec![-90.0; 100],
            },
            3000,
        );
        assert!(!m.receiving);
    }

    #[test]
    fn the_status_line_follows_the_state() {
        let mut m = Model::default();
        assert_eq!(m.status().title, "Not set up");
        m.settings.callsign = "KA1ABC".into();
        assert_eq!(m.status().title, "Listening");
        m.link = LinkState::Linked { peer: "K1AB".into() };
        assert_eq!(m.status().title, "Linked to K1AB");
        m.transmitting = true;
        assert_eq!(m.status().title, "Sending");
        m.engine_error = "boom".into();
        assert_eq!(m.status().title, "Radio not running");
    }

    #[test]
    fn the_support_note_waits_a_week_then_a_month() {
        let day = DAY_MS;
        let mut s = Support {
            first: 1,
            last: 0,
            off: false,
        };
        assert!(!s.due(3 * day));
        assert!(s.due(8 * day));
        s.last = 8 * day;
        assert!(!s.due(20 * day));
        assert!(s.due(39 * day));
        s.off = true;
        assert!(!s.due(100 * day));
    }

    #[test]
    fn saved_state_round_trips() {
        let mut m = Model::default();
        m.add_out(msg("a", "K1AB", Dir::Out, 1));
        let json = serde_json::to_string(&m.saved()).unwrap();
        let mut n = Model::default();
        n.restore(serde_json::from_str(&json).unwrap());
        assert_eq!(n.messages, m.messages);
    }

    #[test]
    fn start_errors_become_advice() {
        assert!(explain_start_error("no audio device").contains("sound device"));
        assert!(explain_start_error("rigctld refused").contains("radio control"));
        assert!(explain_start_error("odd").starts_with("The radio could not start."));
    }
}
