//! Panels over the page: Settings, Help, the tour, the file confirmation and the support nudge.

use alloy_ui::prelude::*;

use super::hardware::{self, Where};
use super::kit::*;
use crate::app::Kahf;
use crate::model::{Page, SettingsTab, ToastKind};
use crate::types::Theme;
use crate::util::callsign;

pub const KOFI_URL: &str = "https://ko-fi.com/adswill";

/// Which panel is open, if any. Only one at a time, the most urgent first.
pub fn current(k: &Kahf) -> Option<V> {
    if k.m.pending_files.is_some() {
        Some(file_confirm(k))
    } else if k.m.settings_open {
        Some(settings(k))
    } else if k.m.help_open {
        Some(help(k))
    } else if k.m.tour_open {
        Some(tour(k))
    } else {
        None
    }
}

/// The small card in the corner, asking once in a while for support.
pub fn nudge(k: &Kahf) -> Option<V> {
    if !k.ui.support_nudge || k.m.pending_files.is_some() {
        return None;
    }
    Some(
        col![
            text("Enjoying KAHF? If you like it, consider supporting its development. Thank you!").caption(),
            row![
                button("Support on Ko-fi").act(|k: &mut Kahf| {
                    k.open_url(KOFI_URL);
                    k.close_nudge(false);
                }),
                button("Not now").ghost().act(|k: &mut Kahf| k.close_nudge(false)),
                button("Don't ask again")
                    .ghost()
                    .act(|k: &mut Kahf| k.close_nudge(true)),
            ]
            .gap(6.0),
        ]
        .gap(8.0)
        .pad(12.0)
        .round(8.0)
        .outline()
        .w(380.0)
        .into(),
    )
}

fn panel(title: &str, body: V, close: fn(&mut Kahf), extra: Option<V>) -> V {
    let mut head = row![heading(title).fill_w()].gap(8.0).align_center();
    if let Some(e) = extra {
        head = head.push(e);
    }
    head = head.push(button("Close").ghost().act(close));
    col![head.pad_xy(16.0, 10.0).fill_w(), body].gap(0.0).into()
}

// ---- file confirmation ---------------------------------------------------------------------

fn file_confirm(k: &Kahf) -> V {
    let Some(p) = &k.m.pending_files else {
        return col![].into();
    };
    let mut list = col![].gap(4.0);
    for f in &p.files {
        list = list.push(
            row![
                text(f.name.clone()).fill_w(),
                caption(crate::util::fmt::size_words(f.size)).dim()
            ]
            .gap(10.0),
        );
    }
    col![
        heading(format!("Send to {}?", p.to)),
        list,
        file_notice(k, &p.files),
        row![
            space(),
            button("Cancel")
                .ghost()
                .act(|k: &mut Kahf| k.m.pending_files = None),
            button("Send").primary().act(|k: &mut Kahf| k.confirm_files()),
        ]
        .gap(8.0),
    ]
    .gap(12.0)
    .w(480.0)
    .into()
}

// ---- settings ------------------------------------------------------------------------------

const TABS: [(SettingsTab, &str); 7] = [
    (SettingsTab::Station, "Your station"),
    (SettingsTab::Radio, "Radio"),
    (SettingsTab::Location, "Location"),
    (SettingsTab::Sending, "Sending"),
    (SettingsTab::Relay, "Relays"),
    (SettingsTab::Look, "Look"),
    (SettingsTab::About, "About"),
];

fn settings(k: &Kahf) -> V {
    let tab = k.m.settings_tab;
    let mut nav = col![].gap(2.0).pad(8.0).w(170.0);
    for (t, label) in TABS {
        nav = nav.push(
            button(label)
                .nav(tab == t)
                .fill_w()
                .act(move |k: &mut Kahf| k.m.settings_tab = t),
        );
    }
    let pane = match tab {
        SettingsTab::Station => station(k),
        SettingsTab::Radio => radio(k),
        SettingsTab::Location => titled(
            "Location on the map",
            hardware::location_editor(k, Where::Settings),
        ),
        SettingsTab::Sending => sending(k),
        SettingsTab::Relay => relays(k),
        SettingsTab::Look => look(k),
        SettingsTab::About => about(k),
    };
    panel(
        "Settings",
        row![nav, scroll(pane.pad(20.0)).fill()]
            .gap(0.0)
            .h(500.0)
            .fill_w()
            .into(),
        |k| k.m.settings_open = false,
        None,
    )
    .w(780.0)
    .into()
}

fn station(k: &Kahf) -> V {
    let typed = &k.ui.call_edit;
    let norm = callsign::normalise(typed);
    let msg = callsign::problem(typed);
    let ok = callsign::valid(&norm) && msg.is_none();
    let mut c = col![
        heading("Your station"),
        lab_hint(
            "Callsign",
            input(typed.clone())
                .hint("For example KA1ABC")
                .act(|k: &mut Kahf, v: String| k.ui.call_edit = v.to_uppercase())
                .w(340.0),
            "Your name on the air. Other stations see it on everything you send."
        ),
    ]
    .gap(12.0);
    if let Some(m) = msg {
        c = c.push(text(m).caption().danger());
    }
    c.push(
        button("Save")
            .primary()
            .enabled(ok && norm != k.m.settings.callsign)
            .act(|k: &mut Kahf| {
                let call = callsign::normalise(&k.ui.call_edit);
                k.set(|s| {
                    s.callsign = call.clone();
                    s.listen_only = false;
                });
                k.toast("Callsign saved.", ToastKind::Good);
            }),
    )
    .into()
}

fn radio(k: &Kahf) -> V {
    // The radio is set on the Radio page only: one place, one status.
    let _ = k;
    col![
        heading("Radio"),
        para("The sound card, the radio and the lane are set on the Radio page.", 60),
        button("Open the Radio page").primary().act(|k: &mut Kahf| {
            k.m.settings_open = false;
            k.go(Page::Radio);
        }),
    ]
    .gap(14.0)
    .into()
}

fn switch_row(title: &str, hint: &str, on: bool, set: impl Fn(&mut Kahf, bool) + Send + Sync + 'static) -> V {
    row![
        col![text(title).label(), caption(hint).dim()].gap(2.0).fill_w(),
        check("", on).act(move |k: &mut Kahf, v: bool| set(k, v))
    ]
    .gap(12.0)
    .align_center()
    .into()
}

fn sending(k: &Kahf) -> V {
    let s = &k.m.settings;
    col![
        heading("Sending"),
        switch_row(
            "Listen only",
            "KAHF receives but never transmits. Safe while you are learning or have no licence.",
            s.listen_only,
            |k, v| k.set_listen_only(v)
        ),
        switch_row(
            "Connect by itself",
            "KAHF reaches a station when you send to it. Turn off to connect by hand.",
            s.auto_link,
            |k, v| k.set(|s| s.auto_link = v)
        ),
        lab_hint(
            "Microphone for voice notes",
            device_pick(k, "mic", &k.ui.devices, &s.mic_in, |k, d| k.set(|s| s.mic_in = d)),
            "Pick your headset or computer microphone, not the radio interface."
        ),
        lab_hint(
            "Speakers for voice notes",
            device_pick(k, "speaker", &k.ui.devices, &s.speaker_out, |k, d| k
                .set(|s| s.speaker_out = d)),
            "Voice notes play here. Not the radio interface, or you will send the sound back out on the air."
        ),
    ]
    .gap(14.0)
    .into()
}

fn relays(k: &Kahf) -> V {
    col![
        heading("Relays"),
        switch_row(
            "Run a relay",
            "Keep this computer on the air to pass messages between stations that cannot hear each other.",
            k.m.settings.relay,
            |k, v| k.set_relay(v)
        ),
        button("Open the relay list").act(|k: &mut Kahf| {
            k.m.settings_open = false;
            k.go(Page::Relay);
        }),
    ]
    .gap(14.0)
    .into()
}

fn look(k: &Kahf) -> V {
    let dark = k.m.settings.theme == Theme::Dark;
    col![
        heading("Look"),
        tabs(["Dark", "Light"], if dark { 0 } else { 1 }).act(|k: &mut Kahf, i: usize| {
            k.set(|s| s.theme = if i == 0 { Theme::Dark } else { Theme::Light });
            k.apply_theme();
        }),
    ]
    .gap(14.0)
    .into()
}

fn about(k: &Kahf) -> V {
    let version = if k.m.info.version.is_empty() {
        "1.0"
    } else {
        &k.m.info.version
    };
    let update: V = match &k.ui.update {
        None => col![].into(),
        Some(Ok(u)) if u.outcome == "newer" => {
            let url = u.url.clone();
            row![
                text(format!("Version {} is out.", u.version)),
                button("Open the download page").act(move |k: &mut Kahf| k.open_url(&url))
            ]
            .gap(8.0)
            .align_center()
            .into()
        }
        Some(Ok(_)) => text("You have the newest version.").caption().dim().into(),
        Some(Err(e)) => text(format!("Could not check: {e}")).caption().danger().into(),
    };
    let server: V = match &k.ui.server {
        None => col![].into(),
        Some(s) if s.reachable => text(format!(
            "Reachable. {} ms, {} stations listed",
            s.millis, s.stations_listed
        ))
        .caption()
        .success()
        .into(),
        Some(s) => text(format!("Not reachable. {}", s.error.clone().unwrap_or_default()))
            .caption()
            .danger()
            .into(),
    };
    let line = |label: &str, body: V| -> V {
        row![text(label.to_string()).dim().w(110.0), body]
            .gap(10.0)
            .align_center()
            .into()
    };
    col![
        heading("About"),
        text(format!("KAHF {version}. Radio messages without the internet.")).dim(),
        line(
            "Updates",
            row![
                button("Check for updates").act(|k: &mut Kahf| k.check_update()),
                update
            ]
            .gap(10.0)
            .align_center()
            .into()
        ),
        line(
            "kahf.dev",
            row![
                button("Check the server").act(|k: &mut Kahf| k.check_server()),
                server
            ]
            .gap(10.0)
            .align_center()
            .into()
        ),
        line(
            "Log files",
            row![
                button("Open the log folder").act(|k: &mut Kahf| {
                    let p = k.m.info.log_dir.clone();
                    k.open_path(p);
                }),
                caption("Attach kahf.log to a bug report.").dim()
            ]
            .gap(10.0)
            .align_center()
            .into()
        ),
        line(
            "Data folder",
            row![
                button("Open the data folder").act(|k: &mut Kahf| {
                    let p = k.m.info.data_dir.clone();
                    k.open_path(p);
                }),
                caption(k.m.info.data_dir.clone()).dim()
            ]
            .gap(10.0)
            .align_center()
            .into()
        ),
        row![
            button("Take the tour again").act(|k: &mut Kahf| {
                k.m.settings_open = false;
                k.ui.tour_step = 0;
                k.m.tour_open = true;
            }),
            button("Run setup again").act(|k: &mut Kahf| {
                k.m.settings_open = false;
                k.go(Page::Setup);
            }),
            button("Support KAHF on Ko-fi").act(|k: &mut Kahf| k.open_url(KOFI_URL)),
        ]
        .gap(8.0),
    ]
    .gap(14.0)
    .into()
}

// ---- help ----------------------------------------------------------------------------------

pub struct Topic {
    pub id: &'static str,
    pub title: &'static str,
    pub body: &'static [&'static str],
}

pub const TOPICS: &[Topic] = &[
    Topic {
        id: "how",
        title: "How KAHF works",
        body: &[
            "KAHF sends messages, mail, files and voice notes over amateur radio instead of the internet. You write on your computer. KAHF turns it into a short burst of sound, your radio sends that into the air, and another KAHF station turns it back into your words.",
            "Three things matter: a callsign (your name on the air), a frequency (where everyone meets) and the signal (how well you hear each other).",
        ],
    },
    Topic {
        id: "first",
        title: "Send your first message",
        body: &[
            "1. Leave KAHF running for a few minutes. Stations that are on the air appear under Chats, in the list on the left.",
            "2. Click one, or type a callsign in the To box.",
            "3. Write a line and press Enter. KAHF reaches the station, sends it, and shows what happened under your message.",
            "You never press Connect. If the other station does not answer, KAHF says why and offers to try again.",
        ],
    },
    Topic {
        id: "kinds",
        title: "Messages, mail, files and voice",
        body: &[
            "Chats are for quick lines. Under the box you can send a message as Normal, Private, Signed or Urgent.",
            "Voice notes: press the microphone, speak, press Send. A voice note is squeezed to a few hundred bytes, so ten seconds is under one kilobyte. It travels as a small file and the other station plays it.",
            "Files: press Attach. A progress bar shows how much has gone out. Big files take a long time on a weak signal.",
            "Mail is for longer messages with a subject and attachments. It has an inbox and a sent folder.",
        ],
    },
    Topic {
        id: "signal",
        title: "Signal quality",
        body: &[
            "KAHF measures how strong the signal is compared with the noise, in decibels (dB). Higher is better. Excellent is 10 dB or more, Good is 3 to 10, Fair is -5 to 3, Weak is -12 to -5, Very weak is below that.",
            "A negative number is normal. KAHF can read a signal that is weaker than the noise around it.",
        ],
    },
    Topic {
        id: "channel",
        title: "Channels and the band",
        body: &[
            "The band is a stretch of frequencies. KAHF uses only 17 metres (18.111 to 18.120 MHz), which holds three KAHF channels, each 2.7 kHz wide. On the Radio screen the shaded area is where KAHF listens. Bright lines outside it are other users of the band, which is normal.",
        ],
    },
    Topic {
        id: "rig",
        title: "Controlling your radio from the computer",
        body: &[
            "On the Radio screen, under Radio control, search for your radio by name, such as IC-7300 or FT-991. Choose the serial port (the USB cable from the radio) and the speed, then press Test. KAHF reads your radio's frequency to prove it works.",
            "This uses Hamlib, a free program that talks to more than 300 radios. Install it with brew install hamlib on a Mac. Sound still comes through your sound card or interface.",
            "If the test fails: check that the radio is on, the cable is in, the speed matches the radio's menu, and, on an Icom, the CI-V address matches.",
        ],
    },
    Topic {
        id: "relay",
        title: "Stations, relays and the map",
        body: &[
            "A station is one person's KAHF setup, named by its callsign. Every KAHF user runs a station.",
            "A relay is a station that stays on the air and passes messages between stations that cannot hear each other. A relay is always a station, but most stations are not relays.",
            "KAHF does not need the internet. The relay list comes from kahf.dev when you are online and is remembered for when you are not. You can also add a relay by callsign. When a station does not answer, KAHF tries your chosen relays in order.",
            "The map at kahf.dev shows relays, and stations that chose to share a location. You are never on it unless you turn it on.",
        ],
    },
    Topic {
        id: "privacy",
        title: "Privacy and location",
        body: &[
            "Messages go out over open radio: anyone with a receiver can hear that something was sent. Private tells other stations to ignore a message, but it does not hide the radio signal.",
            "Your location is off by default. You can share a circle (your exact spot stays private) or your position to about 200 metres.",
            "Your callsign is public by nature. It is how amateur radio works.",
        ],
    },
    Topic {
        id: "trouble",
        title: "When something is wrong",
        body: &[
            "Nobody appears: give it a few minutes, then look at the picture on the Radio screen. If it is flat, KAHF is not hearing sound: check the cable, the radio volume and the sound device.",
            "Did not answer: the other station may be off, too far, or on another frequency. KAHF tries again and uses a relay if it knows one.",
            "Message says waiting: KAHF waits for a quiet moment on the channel so it does not talk over someone. It sends as soon as the channel is clear.",
            "Cannot send anything: listen-only may be on (Settings, Sending), or no callsign is saved.",
            "Voice note will not record: choose your microphone in Settings, under Sending.",
        ],
    },
    Topic {
        id: "words",
        title: "Glossary",
        body: &[
            "Callsign: your unique name on the air, issued with your amateur licence.",
            "Station: one person's KAHF setup. Relay: a station that passes messages on for others.",
            "Band: a range of frequencies. KAHF uses only 17 metres (18 MHz). Channel: a 2.7 kHz slice of the band; there are three.",
            "SNR: signal-to-noise ratio, in dB. KAMI: the way KAHF turns data into sound.",
            "Waterfall: the scrolling picture on the Radio screen. Time runs down, brighter is stronger.",
            "Grid square: a short code, like JN45, that names an area of the Earth.",
            "Connect: your station sends a short signal asking another to answer. KAHF does this for you when you send.",
        ],
    },
];

/// The topics that mention the search words, or all of them when nothing is typed.
pub fn matching(query: &str) -> Vec<&'static Topic> {
    let q = query.trim().to_lowercase();
    TOPICS
        .iter()
        .filter(|t| {
            q.is_empty()
                || format!("{} {}", t.title, t.body.join(" "))
                    .to_lowercase()
                    .contains(&q)
        })
        .collect()
}

fn help(k: &Kahf) -> V {
    let shown = matching(&k.ui.help_query);
    let open = shown
        .iter()
        .find(|t| t.id == k.ui.help_topic)
        .or(shown.first())
        .copied();
    let mut list = col![].gap(2.0).pad(8.0).w(240.0);
    for t in &shown {
        let id = t.id;
        list = list.push(
            button(t.title)
                .nav(open.map(|o| o.id) == Some(id))
                .fill_w()
                .act(move |k: &mut Kahf| k.ui.help_topic = id.to_string()),
        );
    }
    let mut page = col![].gap(10.0).pad(20.0);
    match open {
        Some(t) => {
            page = page.push(heading(t.title));
            for p in t.body {
                let mut lines = col![].gap(2.0);
                for line in wrap_lines(p, 56) {
                    lines = lines.push(text(line));
                }
                page = page.push(lines);
            }
        }
        None => page = page.push(text("Nothing found. Try another word.").dim()),
    }
    panel(
        "Help",
        col![
            input(k.ui.help_query.clone())
                .hint("Search help: relay, voice, signal…")
                .act(|k: &mut Kahf, v: String| k.ui.help_query = v)
                .pad_xy(16.0, 4.0)
                .fill_w(),
            row![scroll(list).fill_h(), scroll(page).fill()]
                .gap(0.0)
                .h(440.0)
                .fill_w(),
        ]
        .gap(6.0)
        .into(),
        |k| k.m.help_open = false,
        Some(
            button("Take the tour")
                .act(|k: &mut Kahf| {
                    k.m.help_open = false;
                    k.ui.tour_step = 0;
                    k.m.tour_open = true;
                })
                .into(),
        ),
    )
    .w(820.0)
    .into()
}

// ---- tour ----------------------------------------------------------------------------------

pub struct Stop {
    pub page: Page,
    pub title: &'static str,
    pub text: &'static str,
}

pub const STOPS: &[Stop] = &[
    Stop { page: Page::Chats, title: "The radio strip", text: "Always on screen at the top. The frequency, a signal meter, RX and TX lamps, and what the radio is doing right now in words." },
    Stop { page: Page::Chats, title: "Three places", text: "Messages holds Chats for quick messages, voice notes and files, and Mail for longer ones. Stations shows who is on the air, the map and the relays. Radio is for the signal and your equipment." },
    Stop { page: Page::Chats, title: "Who to write to", text: "Pick someone from the list, or type a callsign in the To box. Stations KAHF hears appear on the left by themselves." },
    Stop { page: Page::Chats, title: "Just press Enter", text: "No connecting, no settings. KAHF reaches the station and tells you what happened. Attach sends a file and the microphone records a voice note." },
    Stop { page: Page::Radio, title: "The channel, live", text: "The shaded band is where KAHF listens. Moving lines mean your radio is being heard." },
    Stop { page: Page::Relay, title: "Relays", text: "Under Stations, Relays: a relay passes messages on for stations that cannot hear each other. Choose the ones KAHF may use, or run one yourself." },
    Stop { page: Page::Chats, title: "Settings and Help", text: "Top right. Settings holds your callsign, the location you share with the map, and whether KAHF may transmit at all. Help has searchable guides, troubleshooting and a glossary." },
];

fn tour(k: &Kahf) -> V {
    let i = k.ui.tour_step.min(STOPS.len() - 1);
    let s = &STOPS[i];
    let last = i + 1 == STOPS.len();
    col![
        caption(format!("Tour, {} of {}", i + 1, STOPS.len())).dim(),
        heading(s.title),
        text(s.text),
        row![
            button("Skip").ghost().act(|k: &mut Kahf| k.finish_tour()),
            space(),
            button("Back")
                .enabled(i > 0)
                .act(|k: &mut Kahf| k.tour_to(k.ui.tour_step.saturating_sub(1))),
            if last {
                button("Done").primary().act(|k: &mut Kahf| k.finish_tour())
            } else {
                button("Next")
                    .primary()
                    .act(|k: &mut Kahf| k.tour_to(k.ui.tour_step + 1))
            },
        ]
        .gap(8.0),
    ]
    .gap(10.0)
    .w(460.0)
    .into()
}
