//! The shapes shared by the window and the engine glue. Field names match the JSON that
//! `kahf-app-core` produces and the settings file stores, so older settings keep working.

use serde::{Deserialize, Serialize};

use crate::util::bands;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum LinkState {
    #[default]
    Idle,
    Calling {
        peer: String,
        #[serde(default)]
        attempt: u32,
        #[serde(default)]
        of: u32,
    },
    Linked {
        peer: String,
    },
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Metrics {
    /// KAMI mode of the last burst we sent, for example "KAMI-32".
    pub mode: String,
    pub auto: bool,
    /// Net information speed of that mode in a full burst, bit/s.
    pub net_bps: f64,
    pub own_snr: Option<f64>,
    pub peer_snr: Option<f64>,
    pub fer_pct: f64,
    pub sent: u64,
    pub received: u64,
    pub retransmits: u64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Heard {
    pub call: String,
    pub snr: Option<f64>,
    /// Unix milliseconds.
    pub at: i64,
    pub hops: Option<u32>,
    pub relay: Option<bool>,
    pub online: Option<bool>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageKind {
    #[default]
    Chat,
    Private,
    Urgent,
    Signed,
}

impl MessageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MessageKind::Chat => "chat",
            MessageKind::Private => "private",
            MessageKind::Urgent => "urgent",
            MessageKind::Signed => "signed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttachmentKind {
    File,
    Voice,
}

/// A file or a voice note that travels with a message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attachment {
    pub kind: AttachmentKind,
    pub name: String,
    pub size: u64,
    /// Where it is on this computer (sent from here, or saved to the inbox).
    #[serde(default)]
    pub path: Option<String>,
    /// Voice notes: length in seconds.
    #[serde(default)]
    pub seconds: Option<f64>,
    /// Transfer progress in bytes, while it is on the air.
    #[serde(default)]
    pub done: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutState {
    Queued,
    Calling,
    Sending,
    Sent,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    In,
    Out,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Msg {
    pub id: String,
    pub peer: String,
    pub dir: Dir,
    pub text: String,
    pub at: i64,
    #[serde(default)]
    pub kind: MessageKind,
    #[serde(default)]
    pub state: Option<OutState>,
    #[serde(default)]
    pub attachment: Option<Attachment>,
    #[serde(default)]
    pub read: bool,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailFile {
    pub name: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailItem {
    pub id: String,
    pub peer: String,
    pub dir: Dir,
    pub subject: String,
    pub body: String,
    pub at: i64,
    #[serde(default)]
    pub files: Vec<MailFile>,
    #[serde(default)]
    pub state: Option<OutState>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub read: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RigModel {
    pub id: u32,
    pub maker: String,
    pub model: String,
    #[serde(default)]
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PickedFile {
    pub path: String,
    pub name: String,
    pub size: u64,
}

/// A recorded voice clip, ready to send.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct VoiceClip {
    pub id: String,
    pub seconds: f64,
    pub bytes: u64,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RelayHop {
    pub at: i64,
    pub from: String,
    pub to: String,
    pub kind: String,
    pub channel_khz: Option<f64>,
}

/// One slot of the relay's accepted schedule, from kahf.dev.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct RelaySlot {
    pub channel_khz: f64,
    pub start_unix: i64,
    pub duration_s: i64,
    pub direction: String,
}

/// What the relay's link to kahf.dev last did.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct ServerSync {
    pub registered: bool,
    pub last_schedule_unix: Option<i64>,
    pub slots: u32,
    pub last_error: Option<String>,
    pub next_attempt_unix: i64,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RelayStatus {
    pub running: bool,
    pub forwarded_today: u64,
    pub forwarded_total: u64,
    /// Messages sent per hour, as (hour start in unix seconds, count), oldest first.
    pub per_hour: Vec<(i64, u32)>,
    pub recent: Vec<RelayHop>,
    pub slots: Vec<RelaySlot>,
    pub outbox: Vec<OutboxItem>,
    pub server: Option<ServerSync>,
    pub server_note: Option<String>,
    pub now_unix: i64,
    /// The one-degree square the relay is listed in, if it is on the map.
    pub area: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct OutboxItem {
    pub id: u64,
    pub what: String,
}

/// A station from the address book the engine keeps.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct Contact {
    pub call: String,
    pub name: String,
    pub qth: String,
    pub notes: String,
    /// YYYY-MM-DD, or empty when never heard.
    pub last_heard: String,
}

/// One logged contact.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct Qso {
    pub call: String,
    pub date: String,
    pub time: String,
    pub band: String,
    pub mode: String,
    pub freq_mhz: Option<f64>,
    pub rst_sent: String,
    pub rst_rcvd: String,
    pub name: String,
    pub comment: String,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct Logbook {
    pub stations: Vec<Contact>,
    pub qsos: Vec<Qso>,
    /// Why a file could not be read, when one could not.
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct UpdateCheck {
    pub outcome: String,
    pub version: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ServerCheck {
    pub reachable: bool,
    pub millis: u64,
    pub stations_listed: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub data_dir: String,
    pub log_dir: String,
}

/// A relay from the directory.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RelayInfo {
    pub call: String,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub km: Option<f64>,
    /// "server", "heard" or "manual".
    pub source: String,
    pub last_seen: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DirectRig {
    pub id: String,
    pub label: String,
    pub baud: u32,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LevelCheck {
    /// "silent", "low", "good" or "high".
    pub verdict: String,
    pub advice: String,
    pub level_db: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LocationMode {
    #[default]
    Off,
    Area,
    Precise,
}

impl LocationMode {
    pub fn as_str(self) -> &'static str {
        match self {
            LocationMode::Off => "off",
            LocationMode::Area => "area",
            LocationMode::Precise => "precise",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Location {
    pub mode: LocationMode,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    /// Radius of the circle shown for "area", in km.
    pub radius_km: u32,
}

impl Default for Location {
    fn default() -> Self {
        Location { mode: LocationMode::Off, lat: None, lon: None, radius_km: 50 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ExportServices {
    pub qrz_key: String,
    pub clublog_email: String,
    pub clublog_password: String,
    pub clublog_app_key: String,
    pub tqsl_path: String,
    pub lotw_location: String,
    pub n1mm_addr: String,
}

impl Default for ExportServices {
    fn default() -> Self {
        ExportServices {
            qrz_key: String::new(),
            clublog_email: String::new(),
            clublog_password: String::new(),
            clublog_app_key: String::new(),
            tqsl_path: String::new(),
            lotw_location: String::new(),
            n1mm_addr: "127.0.0.1:12060".into(),
        }
    }
}

/// A radio chosen by name, for frequency control through Hamlib.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RigChoice {
    pub model: Option<u32>,
    pub name: String,
    pub port: String,
    pub baud: u32,
}

impl Default for RigChoice {
    fn default() -> Self {
        RigChoice { model: None, name: String::new(), port: String::new(), baud: 19_200 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub callsign: String,
    pub audio_in: String,
    pub audio_out: String,
    pub sdr: String,
    pub rigctld: String,
    /// A radio controlled directly by name, such as `ic7300:/dev/cu.usbserial-1:115200:cat`.
    pub rig_spec: String,
    /// Fields for sending the logbook to other services. Kept on this computer only.
    pub export_services: ExportServices,
    pub location: Location,
    pub auto_link: bool,
    /// Receive only: never transmit.
    pub listen_only: bool,
    pub auto_mode: bool,
    pub fixed_mode: String,
    /// Acts as a relay for other stations.
    pub relay: bool,
    /// Relays to try, in order, when a station cannot be reached directly.
    pub relay_hints: Vec<String>,
    pub rig: RigChoice,
    /// Where voice notes are recorded: a sound device name, empty for the computer's microphone.
    pub mic_in: String,
    /// Speakers for playing voice notes. Empty: the system's default output.
    pub speaker_out: String,
    /// Tuned frequency in kHz.
    pub freq_khz: f64,
    pub tx_level: u8,
    pub setup_done: bool,
    pub tour_done: bool,
    pub theme: Theme,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            callsign: String::new(),
            audio_in: String::new(),
            audio_out: String::new(),
            sdr: String::new(),
            rigctld: String::new(),
            rig_spec: String::new(),
            export_services: ExportServices::default(),
            // Privacy first: nothing is shared until the person chooses to.
            location: Location::default(),
            auto_link: true,
            listen_only: false,
            auto_mode: true,
            fixed_mode: "KAMI-32".into(),
            relay: false,
            relay_hints: Vec::new(),
            rig: RigChoice::default(),
            mic_in: String::new(),
            speaker_out: String::new(),
            freq_khz: bands::default_khz(),
            tx_level: 50,
            setup_done: false,
            tour_done: false,
            theme: Theme::Dark,
        }
    }
}

/// What the engine told the window. See `kahf_app_core::convert`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum EngineEvent {
    Link { link: LinkState },
    Message { from: String, text: String, at: i64 },
    Heard { heard: Heard },
    Metrics { metrics: Metrics },
    Spectrum { bins: Vec<f32> },
    Tx { state: String, #[serde(default)] reason: Option<String> },
    Sent { id: String },
    Failed { id: String, reason: String },
    File {
        name: String,
        done: u64,
        total: u64,
        dir: Dir,
        id: String,
        #[serde(default)]
        from: Option<String>,
        #[serde(default)]
        path: Option<String>,
    },
    Mail {
        from: String,
        subject: String,
        body: String,
        at: i64,
        #[serde(default)]
        files: Vec<MailFile>,
    },
    Level { level: f32 },
    Relay { status: RelayStatus },
    Log { text: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn settings_from_an_older_file_fill_in_the_rest() {
        let s: Settings = serde_json::from_value(json!({ "callsign": "KA1ABC", "setupDone": true })).unwrap();
        assert_eq!(s.callsign, "KA1ABC");
        assert!(s.setup_done && s.auto_link);
        assert_eq!(s.location.radius_km, 50);
        // And back: the keys the engine glue reads keep their names.
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["audioIn"], "");
        assert_eq!(v["rig"]["baud"], 19_200);
        assert_eq!(v["location"]["mode"], "off");
        assert_eq!(v["relayHints"], json!([]));
    }

    #[test]
    fn engine_events_parse() {
        let e: EngineEvent = serde_json::from_value(json!({ "type": "link", "link": { "state": "calling", "peer": "K1AB", "attempt": 2, "of": 5 } })).unwrap();
        assert_eq!(e, EngineEvent::Link { link: LinkState::Calling { peer: "K1AB".into(), attempt: 2, of: 5 } });
        let e: EngineEvent = serde_json::from_value(json!({ "type": "link", "link": { "state": "idle" } })).unwrap();
        assert_eq!(e, EngineEvent::Link { link: LinkState::Idle });
        let e: EngineEvent = serde_json::from_value(json!({ "type": "file", "name": "a.txt", "done": 5, "total": 10, "dir": "in", "id": "x", "from": "K1AB" })).unwrap();
        assert!(matches!(e, EngineEvent::File { done: 5, .. }));
        let e: EngineEvent = serde_json::from_value(json!({ "type": "tx", "state": "sending" })).unwrap();
        assert!(matches!(e, EngineEvent::Tx { .. }));
    }
}
