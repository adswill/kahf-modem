//! Every visible string in the UI lives here (sample data excepted: see sample.rs).

pub const APP_NAME: &str = "KAHF";
pub const APP_TITLE: &str = "KAHF";

// Top bar
pub const CALLSIGN_UNSET: &str = "No callsign set";
pub const LINK_IDLE: &str = "No link";
pub const LINK_UP: &str = "Linked";
pub const TX_IDLE: &str = "Not transmitting";
pub const TX_ON_AIR: &str = "Transmitting";
pub const TX_BLOCKED: &str = "Transmit off";
pub const LISTEN_ONLY_HINT: &str = "Listen only (Ctrl+L)";
pub const UTC: &str = "UTC";
pub const COMMANDS_BUTTON: &str = "Commands";
pub const COMMANDS_HINT: &str = "Ctrl+K";

// Sections
pub const SECTION_OPERATE: &str = "Operate";
pub const SECTION_STATIONS: &str = "Stations";
pub const SECTION_MESSAGES: &str = "Messages";
pub const SECTION_MAIL: &str = "Mail";
pub const SECTION_FILES: &str = "Files";
pub const SECTION_RELAY: &str = "Relay";
pub const SECTION_LOGBOOK: &str = "Logbook";
pub const SECTION_MAP: &str = "Map";
pub const SECTION_SETTINGS: &str = "Settings";

// Simulated data
pub const SIMULATED_INPUT: &str = "Simulated input";
pub const SIMULATED_HINT: &str = "No radio is connected, so the spectrum is generated here.";
pub const SAMPLE_BANNER: &str =
    "Sample data. Nothing on this page comes from a radio yet. Turn it off in Settings, Appearance.";

// Operate
pub const BAND_LABEL: &str = "Band";
pub const MODE_LABEL: &str = "Mode";
pub const MODE_VALUE: &str = "Data";
pub const DATA_NOT_CHOSEN: &str = "Not chosen yet";
pub const WIDTH_LABEL: &str = "Width";
pub const OFFSET_LABEL: &str = "Offset";
pub const PANEL_HIDE: &str = "Hide panel";
pub const PANEL_SHOW: &str = "Show panel";
pub const THREAD_EMPTY_TITLE: &str = "No messages yet";
pub const THREAD_EMPTY_HINT: &str =
    "Type a callsign in To, write a message below and press Send. The app connects for you.";
pub const COMPOSER_PLACEHOLDER: &str = "Write a message";
pub const SEND: &str = "Send";
pub const SEND_HINT: &str = "Ctrl+Enter";
pub const KIND_FILE: &str = "File";
pub const KIND_LABEL: &str = "Kind";
pub const MESSAGE_LIMIT: usize = 500;

// Right column
pub const LINK_HEADING: &str = "Link";
pub const TRANSMIT_HEADING: &str = "Transmit";
pub const TARGET_LABEL: &str = "Station";
pub const TARGET_PLACEHOLDER: &str = "Callsign to connect to";
pub const STATE_LABEL: &str = "State";
pub const SPEED_LABEL: &str = "Speed";
pub const PROFILE_LABEL: &str = "Speed profile";
pub const LEVEL_LABEL: &str = "Output level";
pub const CONNECTION_LABEL: &str = "Connection";
pub const DEVICE_LABEL: &str = "Device";
pub const CONNECT_ELLIPSIS: &str = "Connect...";
pub const DISCONNECT: &str = "Disconnect";
pub const SEND_BEACON: &str = "Send beacon";
pub const NOT_REPORTED: &str = "Not reported yet";
pub const NO_LINK_TEXT: &str = "Not linked to any station.";
pub const LISTEN_ONLY_WHY: &str = "Transmit is off because Listen only is on. Nothing leaves this computer.";
pub const LISTEN_ONLY_TURN_OFF: &str = "Turn off Listen only";
pub const ATTACH: &str = "Attach file";
pub const ATTACH_PATH_LABEL: &str = "File to send";
pub const ATTACH_PATH_PLACEHOLDER: &str = "Path of the file, for example /Users/me/photo.jpg";
pub const ATTACH_SEND: &str = "Send file";
pub const ATTACH_CANCEL: &str = "Cancel";
pub const TIP_ATTACH: &str = "Send a file to the station in To. You confirm first.";
pub const FILES_SEND_BUTTON: &str = "Send a file";
pub const COL_RETRIES: &str = "Retries";
pub const ENGINE_LAST: &str = "Last engine message";
pub const PROFILE_AUTO: &str = "Automatic";
pub const PROFILE_ROBUST: &str = "Robust";
pub const PROFILE_FAST: &str = "Fast";

// Confirmation panels
pub const CONFIRM_TX_TITLE: &str = "Transmit now?";
pub const CONFIRM_TX_BODY: &str =
    "This sends a signal on air. Check that the frequency is clear and that you are licensed to use it.";
pub const CONFIRM_TX_ACTION: &str = "Action";
pub const CONFIRM_TX_FREQ: &str = "Frequency";
pub const CONFIRM_TX_CALL: &str = "Your callsign";
pub const CONFIRM_TX_BUTTON: &str = "Transmit";
pub const CONFIRM_CANCEL: &str = "Cancel";
pub const CONFIRM_DISCONNECT_TITLE: &str = "Disconnect?";
pub const CONFIRM_DISCONNECT_BODY: &str = "The link will close and any message still being sent will stop.";
pub const CONFIRM_DISCONNECT_KEEP: &str = "Keep link";
pub const CONFIRM_ESC_HINT: &str = "Press Escape to cancel";
pub const ACTION_CONNECT: &str = "Connect";
pub const ACTION_SEND: &str = "Send message";
pub const ACTION_BEACON: &str = "Send beacon";
pub const ACTION_SEND_FILE: &str = "Send file";
pub const ACTION_CONNECT_SEND: &str = "Connect, then send message";
pub const ACTION_CONNECT_SEND_FILE: &str = "Connect, then send file";
pub const CONFIRM_TX_TO: &str = "To";
pub const CONFIRM_TX_FILE: &str = "File";

// Command palette
pub const PALETTE_PLACEHOLDER: &str = "Type a command";
pub const PALETTE_EMPTY: &str = "No command matches. Try a shorter word.";
pub const CMD_LISTEN_ON: &str = "Turn Listen only on";
pub const CMD_LISTEN_OFF: &str = "Turn Listen only off";
pub const CMD_CONNECT: &str = "Connect to the chosen station";
pub const CMD_DISCONNECT: &str = "Disconnect";
pub const CMD_TOGGLE_RAIL: &str = "Collapse or expand the section list";
pub const CMD_TOGGLE_PANEL: &str = "Hide or show the right panel";
pub const CMD_GO: &str = "Go to";

// Stations
pub const STATIONS_TITLE: &str = "Stations";
pub const STATIONS_FILTER: &str = "Filter by callsign or name";
pub const COL_CALLSIGN: &str = "Callsign";
pub const COL_NAME: &str = "Name";
pub const COL_LAST_HEARD: &str = "Last heard";
pub const COL_TIMES: &str = "Times heard";
pub const COL_LAST_MESSAGE: &str = "Last message";
pub const COL_ROLE: &str = "Role";
pub const STATIONS_EMPTY_TITLE: &str = "No stations heard yet";
pub const STATIONS_EMPTY_HINT: &str = "Listen on this frequency for a few minutes.";
pub const STATIONS_NO_MATCH: &str = "No station matches the filter.";

// Messages
pub const MESSAGES_EMPTY_TITLE: &str = "No conversations yet";
pub const MESSAGES_EMPTY_HINT: &str = "Messages you send or receive appear here.";
pub const KIND_CHAT: &str = "Chat";
pub const KIND_URGENT: &str = "Urgent";
pub const KIND_SIGNED: &str = "Signed";
pub const KIND_PRIVATE: &str = "Private";

// Mail
pub const MAIL_TITLE: &str = "Mail";
pub const MAIL_COMPOSE: &str = "Write mail";
pub const MAIL_BACK: &str = "Back to inbox";
pub const MAIL_EMPTY_TITLE: &str = "Inbox is empty";
pub const MAIL_EMPTY_HINT: &str = "Mail arrives over the air or through a relay.";
pub const MAIL_PICK_HINT: &str = "Choose a message to read it.";
pub const MAIL_TO: &str = "To";
pub const MAIL_SUBJECT: &str = "Subject";
pub const MAIL_SIGNED: &str = "Sign this mail";
pub const MAIL_ATTACH: &str = "Attach file";
pub const MAIL_ATTACH_NONE: &str = "No files attached.";
pub const MAIL_QUEUE: &str = "Queue for sending";
pub const MAIL_BODY_PLACEHOLDER: &str = "Write your mail";

// Files
pub const FILES_TITLE: &str = "File transfers";
pub const COL_FILE: &str = "File";
pub const COL_DIRECTION: &str = "Direction";
pub const COL_SIZE: &str = "Size";
pub const COL_PROGRESS: &str = "Progress";
pub const COL_STATE: &str = "State";
pub const FILES_EMPTY_TITLE: &str = "No transfers";
pub const FILES_EMPTY_HINT: &str = "Files you send or receive show their progress here.";
pub const FILE_START: &str = "Start";
pub const FILE_PAUSE: &str = "Pause";
pub const FILE_CANCEL: &str = "Cancel";
pub const FILE_SENDING: &str = "Sending";
pub const FILE_RECEIVING: &str = "Receiving";
pub const STATE_ACTIVE: &str = "Active";
pub const STATE_PAUSED: &str = "Paused";
pub const STATE_DONE: &str = "Done";
pub const STATE_CANCELLED: &str = "Cancelled";
pub const STATE_WAITING: &str = "Waiting";

// Relay
pub const RELAY_TITLE: &str = "Relay";
pub const RELAY_OFF_TITLE: &str = "This station is not a relay";
pub const RELAY_OFF_HINT: &str =
    "Relay tools appear when the role in Settings is Relay or Station and relay.";
pub const RELAY_OPEN_SETTINGS: &str = "Open Settings";
pub const RELAY_SCHEDULE: &str = "Schedule";
pub const RELAY_HOPS: &str = "Hop log";
pub const RELAY_STATS: &str = "Statistics";
pub const RELAY_SENT_TODAY: &str = "Sent today";
pub const RELAY_SENT_TOTAL: &str = "Sent in total";
pub const RELAY_ACCEPT: &str = "Accept";
pub const RELAY_DECLINE: &str = "Decline";
pub const COL_CHANNEL: &str = "Channel";
pub const COL_TIME: &str = "Time (UTC)";
pub const COL_DURATION: &str = "Duration";
pub const COL_DIR: &str = "Direction";
pub const COL_FROM_TO: &str = "From and to";
pub const RELAY_EMPTY_TITLE: &str = "No relay schedule yet";
pub const RELAY_EMPTY_HINT: &str = "A schedule from the server appears here for you to accept.";

// Logbook
pub const LOG_TITLE: &str = "Logbook";
pub const LOG_SEARCH: &str = "Search callsign or note";
pub const LOG_EXPORT: &str = "Export ADIF";
pub const COL_DATE: &str = "Date";
pub const COL_BAND: &str = "Band";
pub const COL_SENT: &str = "Sent";
pub const COL_RECEIVED: &str = "Received";
pub const COL_NOTE: &str = "Note";
pub const LOG_EMPTY_TITLE: &str = "No contacts logged";
pub const LOG_EMPTY_HINT: &str = "Each completed link is added here.";
pub const LOG_NO_MATCH: &str = "No contact matches the search.";
pub const LOG_EXPORT_DONE: &str = "Export is not connected to the engine yet. Nothing was written.";

// Map
pub const MAP_TITLE: &str = "Map";
pub const MAP_BODY: &str = "The full map is on the website. Stations that share their location appear there.";
pub const MAP_HEADLINE: &str = "The map is on the website";
pub const MAP_NO_MAP_HERE: &str = "This app does not draw a map. Open the website map in your browser; the hop path of the message you selected is listed on the right.";
pub const MAP_BUTTON: &str = "Open kahf.dev/map";
pub const MAP_URL: &str = "https://kahf.dev/map";
pub const MAP_PRIVACY: &str =
    "Your own location is only shared if you choose Precise or Area in Settings, Location sharing.";

// Settings
pub const SETTINGS_TITLE: &str = "Settings";
pub const SET_STATION: &str = "Station";
pub const SET_STATION_DESC: &str = "Who you are on air.";
pub const SET_CALLSIGN: &str = "Callsign";
pub const SET_CALLSIGN_DESC: &str = "Sent with every transmission.";
pub const SET_FINGERPRINT: &str = "Key fingerprint";
pub const SET_FINGERPRINT_DESC: &str = "Other stations use this to check your signed messages.";
pub const SET_FINGERPRINT_NONE: &str = "No key created yet";
pub const SET_ROLE: &str = "Role";
pub const SET_ROLE_DESC: &str = "A relay passes messages between stations that cannot hear each other.";
pub const ROLE_STATION: &str = "Station";
pub const ROLE_RELAY: &str = "Relay";
pub const ROLE_BOTH: &str = "Station and relay";
pub const SET_AUDIO: &str = "Audio";
pub const SET_AUDIO_DESC: &str = "Sound card connected to your radio.";
pub const EVENTS_HEADING: &str = "Last events";
pub const NO_EVENTS: &str = "Nothing yet. Events such as links and sends are listed here.";
pub const KEYS_HEADING: &str = "Keys";
pub const NOW_HEADING: &str = "Now";
pub const KEY_LISTEN: &str = "Listen only on or off";
pub const KEY_CONNECT: &str = "Connect to station";
pub const KEY_SEND: &str = "Send message";
pub const KEY_COMMANDS: &str = "All commands";
pub const KEY_CANCEL: &str = "Cancel or close";
pub const NOW_LISTEN_ON: &str = "Listen only, nothing is sent";
pub const NOW_LISTEN_OFF: &str = "Transmit allowed, each send asks first";
pub const NOW_NEXT_CALL: &str = "Next step: set your callsign in Settings.";
pub const NOW_NEXT_TARGET: &str = "Next step: type a station callsign, then Connect.";
pub const NOW_NEXT_SEND: &str = "Next step: write a message and press Ctrl+Enter.";
pub const MAP_LOC_LABEL: &str = "Location sharing";
pub const MAP_HEARD_LABEL: &str = "Stations heard";
pub const MAP_LINK_LABEL: &str = "Linked to";
pub const MAP_SUMMARY: &str = "This session";
// Draft for owner legal review: the relay terms text below must be checked by a lawyer before release.
pub const RELAY_MODAL_TITLE: &str = "Relay mode";
pub const RELAY_MODAL_BODY: &str = "In some countries the law prohibits automatic stations unless the operator holds a licence for them. In Finland, an automatic station needs a licence or permit from Traficom. Relay mode forwards messages while the operator may not be at the station.";
pub const RELAY_TICK_LICENCE: &str =
    "I hold the licence or permit my country requires for an automatic station.";
pub const RELAY_TICK_TERMS: &str =
    "I accept the terms of use. KAHF is not responsible for what I do with it.";
pub const RELAY_CANCEL: &str = "Cancel";
pub const RELAY_START: &str = "Start relay";
pub const TOAST_RELAY_STARTED: &str = "Relay mode started.";
pub const SEGMENT_LABEL: &str = "Segment";
pub const AUDIO_HEADING: &str = "Audio";
pub const RIG_HEADING: &str = "Rig";
pub const HOPS_HEADING: &str = "Hops";
pub const VOL_IN_LABEL: &str = "Input volume";
pub const VOL_OUT_LABEL: &str = "Output volume";
pub const INPUT_LEVEL_LABEL: &str = "Input level";
pub const NO_HOPS: &str = "No hops yet. Send a message to see its path.";
pub const SET_AUDIO_IN: &str = "Input device";
pub const SET_AUDIO_OUT: &str = "Output device";
pub const DEVICE_SYSTEM: &str = "System default";
pub const SET_RIG: &str = "Rig";
pub const SET_RIG_DESC: &str = "How KAHF changes frequency and keys your radio.";
pub const SET_RIG_CONTROL: &str = "Rig control";
pub const RIG_NONE: &str = "None (tune by hand)";
pub const RIG_CAT: &str = "Serial CAT";
pub const RIG_VOX: &str = "VOX";
pub const SET_SDR: &str = "SDR";
pub const SET_SDR_DESC: &str = "Use a software defined radio instead of a sound card.";
pub const SET_SDR_DEVICE: &str = "SDR device";
pub const SET_DATA: &str = "Data folder";
pub const SET_DATA_DESC: &str = "Where messages, mail, files and the logbook are stored.";
pub const SET_DATA_PATH: &str = "Folder";
pub const SET_LOCATION: &str = "Location sharing";
pub const SET_LOCATION_DESC: &str =
    "Precise shows your exact position on the public map. You can choose Off or Area at any time.";
pub const LOC_OFF: &str = "Off";
pub const LOC_OFF_DESC: &str = "Nobody sees your location.";
pub const LOC_PRECISE: &str = "Precise";
pub const LOC_PRECISE_DESC: &str = "Your exact position is shared.";
pub const LOC_AREA: &str = "Area";
pub const LOC_AREA_DESC: &str = "Only a circle around you is shared.";
pub const LOC_RADIUS: &str = "Area radius";
pub const SET_RELAY: &str = "Internet relay";
pub const SET_RELAY_DESC: &str =
    "Lets messages travel over the internet when a station cannot be reached on air. Off by default.";
pub const SET_RELAY_TOGGLE: &str = "Use internet relay";
pub const SET_LISTEN: &str = "Listen only";
pub const SET_LISTEN_DESC: &str = "The app receives but never transmits until you turn this off.";
pub const SET_APPEARANCE: &str = "Appearance";
pub const SET_APPEARANCE_DESC: &str = "How the window looks and what it shows.";
pub const SET_SAMPLE: &str = "Show sample data";
pub const SET_SAMPLE_DESC: &str = "Off by default: the screens show what the live engine does. Turn it on to fill them with example stations, messages and files.";
pub const SET_PANEL: &str = "Show the right panel on Operate";
pub const SET_UPDATES: &str = "Updates";
pub const SET_UPDATES_DESC: &str = "KAHF never updates itself without asking.";
pub const SET_VERSION: &str = "Version";
pub const SET_CHECK: &str = "Check for updates";
pub const SET_CHECK_DONE: &str = "Update checks are not connected yet.";
pub const UPDATE_NOT_CHECKED: &str = "Not checked";

// Toasts
pub const TOAST_LINKED: &str = "Link opened.";
pub const TOAST_UNLINKED: &str = "Link closed.";
pub const TOAST_SENT: &str = "Message queued for sending.";
pub const TOAST_BEACON: &str = "Beacon queued.";
pub const TOAST_MAIL: &str = "Mail queued for sending.";
pub const TOAST_SCHEDULE_ACCEPTED: &str = "Relay schedule accepted.";
pub const TOAST_SCHEDULE_DECLINED: &str = "Relay schedule declined.";
pub const TOAST_MAP_FAIL: &str = "Could not open the browser. Go to kahf.dev/map yourself.";

// Status bar
pub const STATUS_ENGINE: &str = "Engine: not running";
pub const STATUS_RIG: &str = "Rig: not connected";
pub const STATUS_AUDIO: &str = "Audio: none";
pub const STATUS_UPDATES: &str = "Updates: not checked";

pub fn connecting(peer: &str, attempt: u32, of: u32) -> String {
    format!("Connecting to {peer}, attempt {attempt} of {of}")
}
pub fn no_reply(peer: &str, what: &str) -> String {
    format!("No reply from {peer}. Your {what} is kept in the outbox and goes out when the station answers.")
}
pub fn size_text(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.0} kB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / 1048576.0)
    }
}
pub fn file_line(name: &str, bytes: u64) -> String {
    format!("File: {name} ({})", size_text(bytes))
}
pub fn outbox(n: usize) -> String {
    format!("Outbox: {n}")
}
pub fn link_with(call: &str) -> String {
    format!("Linked to {call}")
}
pub fn char_count(n: usize) -> String {
    format!("{n} of {MESSAGE_LIMIT}")
}
pub fn connect_to(call: &str) -> String {
    format!("Connect to {call}")
}
pub fn percent(p: f32) -> String {
    format!("{:.0}%", p * 100.0)
}
pub fn km(v: f32) -> String {
    format!("{v:.0} km")
}
pub fn level_pct(v: f32) -> String {
    format!("{v:.0}%")
}

// Route (hop path)
pub const HOP_SENDER: &str = "Sender";
pub const HOP_RELAY: &str = "Relay";
pub const HOP_RECIPIENT: &str = "Recipient";
pub const HOP_HEARD: &str = "Heard";
pub const HOP_NOT_HEARD: &str = "Not heard";
pub const HOP_NONE_HINT: &str = "Choose a message to see the path it took.";
pub const COL_HEARD: &str = "Heard";

// Tooltips
pub const TIP_CALL_SET: &str =
    "Your callsign. It is sent with every transmission. Change it in Settings, Station.";
pub const TIP_CALL_UNSET: &str =
    "No callsign yet. Transmit stays blocked until you set one in Settings, Station.";
pub const TIP_LINK_IDLE: &str = "No station is connected. Choose a station and press Connect on Operate.";
pub const TIP_LINK_UP: &str = "Messages go to this station. The link stays open until you disconnect.";
pub const TIP_TX_BLOCKED: &str = "Transmit is off because Listen only is on. Turn Listen only off to send.";
pub const TIP_TX_READY: &str =
    "Not transmitting right now. KAHF sends only after you confirm, and waits if it hears another signal on the channel.";
pub const TIP_LISTEN: &str = "When on, nothing is sent. KAHF only receives. Shortcut Ctrl+L.";
pub const TIP_UTC: &str = "Universal time, used for schedules and the logbook.";
pub const TIP_COMMANDS: &str = "Find any action by typing. Shortcut Ctrl+K.";
pub const TIP_STATUS_ENGINE: &str =
    "The part that does the radio work. It is not running yet, so screens show sample or simulated data.";
pub const TIP_STATUS_RIG: &str = "Link to your radio for tuning and keying.";
pub const TIP_STATUS_AUDIO: &str = "Sound card used to hear and send signals.";
pub const TIP_STATUS_UPDATES: &str = "KAHF only checks for updates when you ask.";
pub const TIP_OUTBOX: &str = "Messages and mail waiting to be sent.";
pub const TIP_PANEL: &str = "Show or hide the link and transmit controls.";
pub const TIP_TARGET: &str = "The callsign of the station to link with.";
pub const TIP_CONNECT: &str = "Ask the station to open a link. You confirm before anything is sent.";
pub const TIP_CONNECT_BLOCKED: &str = "Connecting is off while Listen only is on.";
pub const TIP_DISCONNECT: &str = "Close the link. You confirm first.";
pub const TIP_BEACON: &str = "Send one short announcement with your callsign so others know you are here.";
pub const TIP_SEND: &str = "Send the message to the linked station. You confirm first. Shortcut Ctrl+Enter.";
pub const TIP_SEND_BLOCKED: &str = "Listen only is on. Press to see how to turn sending on.";
pub const TIP_KIND: &str = "Chat is a normal message. Urgent is marked for attention. Signed proves it is from you. Private is for one station.";
pub const TIP_SPEED: &str = "How fast data is moving on the link.";
pub const TIP_PROFILE: &str = "Robust is slower but survives poor conditions. Fast needs a clean signal.";
pub const TIP_LEVEL: &str = "How strong the sound sent to your radio is. Too high distorts the signal.";
pub const TIP_TURN_OFF_LISTEN: &str = "Allow transmitting. Every transmit still asks you to confirm.";
pub const TIP_OPEN_SETTINGS: &str = "Go to Settings to change the role.";
pub const TIP_KIND_URGENT: &str = "Marked for attention.";
pub const TIP_KIND_SIGNED: &str = "Carries a signature that proves who sent it.";
pub const TIP_KIND_PRIVATE: &str = "Meant for one station only.";
pub const TIP_HEARD: &str = "This station received the message at this hop.";
pub const TIP_ROLE: &str = "A relay passes messages on for stations that cannot hear each other.";
pub const TIP_ACCEPT: &str = "Agree to run the relay schedule.";
pub const TIP_DECLINE: &str = "Do not run this schedule.";
pub const TIP_START: &str = "Start or resume this transfer.";
pub const TIP_START_BLOCKED: &str = "Sending a file is off while Listen only is on.";
pub const TIP_PAUSE: &str = "Pause this transfer. You can resume later.";
pub const TIP_FILE_CANCEL: &str = "Stop this transfer for good.";
pub const TIP_EXPORT: &str = "Save the logbook as an ADIF file for other logging programs.";
pub const TIP_COMPOSE: &str = "Write a new mail.";
pub const TIP_MAIL_BACK: &str = "Close this mail.";
pub const TIP_QUEUE: &str = "Put the mail in the outbox. It goes out when you transmit.";
pub const TIP_MAP_OPEN: &str = "Open the map in your web browser.";
pub const TIP_CHECK: &str = "Look for a newer version now.";
pub const TIP_TIMES: &str = "How many times this station has been heard.";
pub const TIP_LAST_HEARD: &str = "How long ago this station was last received.";
pub const TIP_SENT: &str = "Signal report you gave, readability then strength.";
pub const TIP_RECEIVED: &str = "Signal report you received, readability then strength.";
pub const TIP_PROGRESS: &str = "How much of the file has moved.";
pub const TIP_STATE: &str = "Whether the item is running, waiting, paused or finished.";
pub const TIP_CHANNEL: &str = "The frequency used.";
pub const TIP_DIRECTION: &str = "Whether you send or receive.";
pub const TIP_PICK_MESSAGE: &str = "Show the route of this message.";
pub const TIP_RAIL_OPERATE: &str = "Tune, watch the spectrum, link with a station and send messages.";
pub const TIP_RAIL_STATIONS: &str = "Stations you have heard.";
pub const TIP_RAIL_MESSAGES: &str = "Your conversations and the route each message took.";
pub const TIP_RAIL_MAIL: &str = "Longer messages that can wait in an inbox.";
pub const TIP_RAIL_FILES: &str = "Files being sent or received.";
pub const TIP_RAIL_RELAY: &str =
    "Schedule and log for passing messages on. Used when this station is a relay.";
pub const TIP_RAIL_LOGBOOK: &str = "A record of your contacts.";
pub const TIP_RAIL_MAP: &str = "Where stations are, and the path of a message.";
pub const TIP_RAIL_SETTINGS: &str = "Callsign, audio, radio and privacy choices.";

// ---- Setup, blocked actions and plain-word explanations
pub const SETUP_TITLE: &str = "Set up KAHF";
pub const SETUP_INTRO: &str = "Six short steps. You can change any of them later in Settings.";
pub const SETUP_START: &str = "Start";
pub const SETUP_NEEDS_CALL: &str = "Enter your callsign in step 1 to start.";
pub const SETUP_DONE_CHIP: &str = "Done";
pub const SETUP_ATTENTION_CHIP: &str = "Needs attention";
pub const SETUP_BACK: &str = "Back to Settings";
pub const SETUP_REVISIT: &str = "Open";
pub const SETUP_SETTINGS_HEADING: &str = "Setup";
pub const SETUP_SETTINGS_DESC: &str = "The five setup steps. Open one to change it.";
pub const SETUP_RUN_ALL: &str = "Run the full setup";
pub const STEP_CALL: &str = "Your callsign";
pub const STEP_RADIO: &str = "Choose your radio";
pub const STEP_SDR: &str = "Choose your SDR";
pub const STEP_AUDIO: &str = "Choose audio input and output";
pub const STEP_LOCATION: &str = "Location sharing";
pub const STEP_LOCATION_HINT: &str =
    "Precise shows your exact position on the public map. You can choose Off or Area at any time.";
pub const STEP_LISTEN: &str = "Listen only";
pub const STEP_CALL_HINT: &str = "Sent with every transmission. Letters and digits only.";
pub const STEP_RADIO_HINT: &str = "KAHF talks to a radio through rigctld. Pick no rig if you tune by hand.";
pub const STEP_SDR_HINT: &str = "Use a software defined radio instead of a sound card, or none.";
pub const STEP_AUDIO_HINT: &str =
    "The sound card connected to your radio. Watch the meter while a signal plays.";
pub const STEP_LISTEN_HINT: &str = "The app receives but never transmits until you turn this off.";
pub const RIG_ADDR_LABEL: &str = "rigctld address";
pub const RIG_ADDR_PLACEHOLDER: &str = "host:port, for example 192.168.1.20:4532";
pub const SDR_NONE_CHOICE: &str = "None, use the sound card";
pub const SDR_HACKRF: &str = "HackRF";
pub const SDR_RTL: &str = "RTL-SDR";
pub const SDR_RTL_NOTE: &str = "RTL-SDR can only receive. Choose a sound card or HackRF to transmit.";
pub const DETECTED: &str = "Detected";
pub const NOTHING_DETECTED: &str = "No devices found. Plug one in and press Rescan.";
pub const RESCAN: &str = "Rescan";
pub const BLOCK_LISTEN: &str =
    "Sending is off because Listen only is on. Turn it off in the top bar, then try again.";
pub const BLOCK_NO_TO: &str = "Type the station's callsign in To, then press Send.";
pub const BLOCK_NO_TEXT: &str = "Write a message first, then press Send.";
pub const BLOCK_NO_CALL: &str = "Set your callsign in Settings, then try again.";
pub const BLOCK_NO_FILE: &str = "Type the path of the file to send, then press Send file.";
pub const BLOCK_FILE_MISSING: &str = "That file cannot be found. Check the path and try again.";
pub const BLOCK_BEACON: &str = "A beacon needs Listen only off. Turn it off in the top bar, then try again.";
pub const BLOCK_NO_ENGINE: &str =
    "The engine did not start. Check your callsign and audio in Settings, then try again.";
pub const BLOCK_MAIL_NO_TO: &str = "Type the station's callsign in To, then press Queue.";
pub const BLOCK_START_FILE: &str =
    "Sending is off because Listen only is on. Turn it off in the top bar, then press Start.";
pub const TOAST_FILE_QUEUED: &str = "File queued for sending.";

// Console frame
pub const SOURCE_LABEL: &str = "Source";
pub const CENTRE_LABEL: &str = "Centre";
pub const RECORD: &str = "Record";
pub const RECORDING: &str = "Recording";
pub const STOP: &str = "Stop";
pub const NAV_HEADING: &str = "Screens";
pub const CHANNELS_HEADING: &str = "Channels";
pub const CH_RECEIVING: &str = "Receiving";
pub const CH_WAITING: &str = "Tuned, waiting";
pub const CH_ACTIVITY: &str = "Activity";
pub const CH_IDLE: &str = "Idle";
pub const STATUS_SIMULATED: &str = "SIMULATED";
pub const STATUS_STOPPED: &str = "STOPPED";
pub const STATUS_SOUND_CARD: &str = "Sound card";
pub const STATUS_SOURCE: &str = "Source";
pub const STATUS_RATE: &str = "Channel rate";
pub const STATUS_CPU: &str = "CPU";
pub const STATUS_STATIONS: &str = "Stations";
pub const STATUS_MESSAGES: &str = "Messages";
pub const CHANNEL_RATE: &str = "12 kS/s";
pub const TIP_SOURCE: &str = "Where the signal comes from: the sound card or a software defined radio.";
pub const TIP_CENTRE: &str = "The centre frequency of the capture. The tuned channel sits inside it.";
pub const TIP_RECORD: &str =
    "Start recording the received signal. Nothing is written while the input is simulated.";
pub const TIP_STOP: &str = "Stop the recording.";
pub const TIP_MODE: &str = "How the signal is decoded. Data is the only mode so far.";
pub const STATIONS_HEARD_HEADING: &str = "Stations heard";
pub const SNR_LABEL: &str = "SNR";
pub const CONVERSATION_HEADING: &str = "Conversation";
pub const LINK_CONNECTING: &str = "Connecting";
pub const INPUT_LEVEL_SHORT: &str = "Level";
pub const MONITOR_HEADING: &str = "Receive and transmit";
pub const SNR_HISTORY_HEADING: &str = "SNR, last 60 s";
