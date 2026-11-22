//! Sample data shown until the engine is connected. Callsigns are made up.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Chat,
    Urgent,
    Signed,
    Private,
    /// A line in the conversation that stands for a file sent or received.
    File,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Chat => crate::strings::KIND_CHAT,
            Kind::Urgent => crate::strings::KIND_URGENT,
            Kind::Signed => crate::strings::KIND_SIGNED,
            Kind::Private => crate::strings::KIND_PRIVATE,
            Kind::File => crate::strings::KIND_FILE,
        }
    }
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

pub struct Station {
    pub call: &'static str,
    pub name: &'static str,
    pub last_heard: &'static str,
    pub times: u32,
    pub last_msg: &'static str,
    pub role: &'static str,
}

pub const STATIONS: &[Station] = &[
    Station {
        call: "XX1XX",
        name: "Amir",
        last_heard: "2 min ago",
        times: 41,
        last_msg: "Copy, thanks for the report.",
        role: "Station",
    },
    Station {
        call: "XX2KB",
        name: "Lena",
        last_heard: "9 min ago",
        times: 17,
        last_msg: "Back at 18:00 UTC.",
        role: "Station",
    },
    Station {
        call: "XX3RL",
        name: "Relay North",
        last_heard: "14 min ago",
        times: 212,
        last_msg: "Schedule updated.",
        role: "Relay",
    },
    Station {
        call: "XX4TS",
        name: "Tomas",
        last_heard: "1 h ago",
        times: 5,
        last_msg: "Can you hear me on 17 m?",
        role: "Station",
    },
    Station {
        call: "XX5MW",
        name: "Mei",
        last_heard: "3 h ago",
        times: 9,
        last_msg: "Sent you the photo.",
        role: "Station and relay",
    },
    Station {
        call: "XX6PD",
        name: "Pavel",
        last_heard: "Yesterday",
        times: 3,
        last_msg: "Signal was weak here.",
        role: "Station",
    },
    Station {
        call: "XX7HN",
        name: "Hana",
        last_heard: "Yesterday",
        times: 26,
        last_msg: "Thanks, 73.",
        role: "Station",
    },
    Station {
        call: "XX8JF",
        name: "Jonas",
        last_heard: "3 days ago",
        times: 2,
        last_msg: "Testing the new antenna.",
        role: "Station",
    },
];

pub struct Conversation {
    pub call: &'static str,
    pub preview: &'static str,
    pub when: &'static str,
    pub unread: u32,
}

pub const CONVERSATIONS: &[Conversation] = &[
    Conversation {
        call: "XX1XX",
        preview: "Copy, thanks for the report.",
        when: "14:02",
        unread: 2,
    },
    Conversation {
        call: "XX3RL",
        preview: "Schedule updated.",
        when: "13:40",
        unread: 0,
    },
    Conversation {
        call: "XX2KB",
        preview: "Back at 18:00 UTC.",
        when: "Yesterday",
        unread: 0,
    },
];

#[derive(Clone)]
pub struct Chat {
    pub time: String,
    pub from: String,
    pub text: String,
    pub kind: Kind,
    pub mine: bool,
}

fn chat(time: &str, from: &str, text: &str, kind: Kind, mine: bool) -> Chat {
    Chat {
        time: time.into(),
        from: from.into(),
        text: text.into(),
        kind,
        mine,
    }
}

pub fn thread(my_call: &str) -> Vec<Chat> {
    vec![
        chat(
            "13:58",
            "XX1XX",
            "Good afternoon. I get you at 59 here.",
            Kind::Chat,
            false,
        ),
        chat(
            "13:59",
            my_call,
            "Thanks. You are 57 here with some fading.",
            Kind::Chat,
            true,
        ),
        chat(
            "14:00",
            "XX1XX",
            "Storm cell to the south, the noise is up.",
            Kind::Chat,
            false,
        ),
        chat(
            "14:01",
            my_call,
            "Copy. I will keep messages short.",
            Kind::Signed,
            true,
        ),
        chat(
            "14:02",
            "XX1XX",
            "Copy, thanks for the report.",
            Kind::Chat,
            false,
        ),
    ]
}

pub struct Mail {
    pub from: &'static str,
    pub subject: &'static str,
    pub when: &'static str,
    pub unread: bool,
    pub signed: bool,
    pub body: &'static str,
}

pub const MAILS: &[Mail] = &[
    Mail {
        from: "XX3RL",
        subject: "Relay schedule for this week",
        when: "13:40",
        unread: true,
        signed: true,
        body: "The relay will run on the usual channels.\n\nMonday to Friday at 06:00, 12:00 and 18:00 UTC.\nPlease accept the schedule in the Relay screen.\n\n73, Relay North",
    },
    Mail {
        from: "XX5MW",
        subject: "Photo from the hill",
        when: "Yesterday",
        unread: false,
        signed: false,
        body: "Sent you the photo as a file. It should arrive in a few minutes.\n\nMei",
    },
    Mail {
        from: "XX7HN",
        subject: "Thanks for the contact",
        when: "2 days ago",
        unread: false,
        signed: true,
        body: "Good contact on Sunday. Hope to speak again soon.\n\nHana",
    },
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FileState {
    Active,
    Paused,
    Done,
    Cancelled,
    Waiting,
}

#[derive(Clone)]
pub struct FileRow {
    /// Engine id of a live transfer; None for sample rows.
    pub id: Option<u64>,
    pub name: String,
    pub to: String,
    pub sending: bool,
    pub bytes: u64,
    pub progress: f32,
    pub state: FileState,
    pub retries: u32,
}

fn sample_file(name: &str, sending: bool, kb: u64, progress: f32, state: FileState) -> FileRow {
    FileRow {
        id: None,
        name: name.into(),
        to: String::new(),
        sending,
        bytes: kb * 1024,
        progress,
        state,
        retries: 0,
    }
}

pub fn files() -> Vec<FileRow> {
    vec![
        sample_file("hill-view.jpg", false, 48, 0.62, FileState::Active),
        sample_file("net-notes.txt", true, 6, 0.0, FileState::Waiting),
        sample_file("schedule.csv", false, 3, 1.0, FileState::Done),
        sample_file("antenna.pdf", true, 212, 0.31, FileState::Paused),
    ]
}

pub struct Qso {
    pub date: &'static str,
    pub time: &'static str,
    pub call: &'static str,
    pub band: &'static str,
    pub sent: &'static str,
    pub received: &'static str,
    pub note: &'static str,
}

pub const QSOS: &[Qso] = &[
    Qso {
        date: "2026-11-16",
        time: "14:05",
        call: "XX1XX",
        band: "17 m",
        sent: "57",
        received: "59",
        note: "Storm noise",
    },
    Qso {
        date: "2026-11-16",
        time: "09:12",
        call: "XX2KB",
        band: "17 m",
        sent: "59",
        received: "58",
        note: "",
    },
    Qso {
        date: "2026-11-15",
        time: "18:40",
        call: "XX7HN",
        band: "17 m",
        sent: "55",
        received: "57",
        note: "First contact",
    },
    Qso {
        date: "2026-11-14",
        time: "07:31",
        call: "XX3RL",
        band: "17 m",
        sent: "59",
        received: "59",
        note: "Relay test",
    },
    Qso {
        date: "2026-11-12",
        time: "21:03",
        call: "XX4TS",
        band: "17 m",
        sent: "53",
        received: "54",
        note: "",
    },
];

pub struct Slot {
    pub channel: &'static str,
    pub time: &'static str,
    pub duration: &'static str,
    pub direction: &'static str,
}

pub const SLOTS: &[Slot] = &[
    Slot {
        channel: "7 078.000 kHz",
        time: "06:00",
        duration: "10 min",
        direction: "Receive",
    },
    Slot {
        channel: "7 078.000 kHz",
        time: "12:00",
        duration: "10 min",
        direction: "Send",
    },
    Slot {
        channel: "14 105.000 kHz",
        time: "18:00",
        duration: "15 min",
        direction: "Receive and send",
    },
];

pub const HOPS: &[(&str, &str, &str)] = &[
    ("12:03", "XX5MW to XX3RL", "Delivered"),
    ("12:01", "XX3RL to XX1XX", "Delivered"),
    ("06:04", "XX2KB to XX3RL", "Retried once"),
];

pub const RELAY_TODAY: u32 = 14;
pub const RELAY_TOTAL: u32 = 1208;

pub struct Hop {
    pub who: String,
    pub role: &'static str,
    pub time: String,
    pub channel: &'static str,
    pub heard: bool,
}

/// Made-up route for a sample message: sender, one relay, recipient.
pub fn route(chat: &Chat, index: usize, me: &str, peer: &str) -> Vec<Hop> {
    use crate::strings as s;
    let (from, to) = if chat.mine { (me, peer) } else { (peer, me) };
    let at = |sec: &str| format!("{}:{sec}", chat.time);
    let lost = chat.mine && index == 3;
    vec![
        Hop {
            who: from.into(),
            role: s::HOP_SENDER,
            time: at("02"),
            channel: "7 078.000 kHz",
            heard: true,
        },
        Hop {
            who: "XX3RL".into(),
            role: s::HOP_RELAY,
            time: at("09"),
            channel: "7 078.000 kHz",
            heard: true,
        },
        Hop {
            who: to.into(),
            role: s::HOP_RECIPIENT,
            time: if lost { String::new() } else { at("16") },
            channel: "14 105.000 kHz",
            heard: !lost,
        },
    ]
}
