//! Relays: use the ones that exist, or run one.

use alloy_ui::prelude::*;

use super::kit::*;
use crate::app::*;
use crate::types::*;
use crate::util::callsign;
use crate::util::fmt::{self, channel_khz, clock, duration, relative, time_ago, utc_hm};

pub fn view(k: &Kahf) -> V {
    let running = k.m.relay.running;
    let tab = if running && k.ui.relay_tab == RelayTab::Use && false {
        RelayTab::Run
    } else {
        k.ui.relay_tab
    };
    let tabs_row = tabs(
        [
            "Use relays",
            if running {
                "Run a relay  ●"
            } else {
                "Run a relay"
            },
        ],
        if tab == RelayTab::Use { 0 } else { 1 },
    )
    .act(|k: &mut Kahf, i: usize| k.ui.relay_tab = if i == 0 { RelayTab::Use } else { RelayTab::Run });
    let body = if tab == RelayTab::Use {
        use_relays(k)
    } else {
        run_relay(k)
    };
    scroll(
        col![row![heading("Relay"), tabs_row].gap(14.0).align_center(), body]
            .gap(14.0)
            .pad(16.0),
    )
    .fill()
    .into()
}

fn use_relays(k: &Kahf) -> V {
    let hints = &k.m.settings.relay_hints;
    let mut c = col![
        text("A relay is a station that stays on the air and passes messages along, so people who cannot hear each other can still talk. When a station does not answer, KAHF tries the relays you chose here, in order.")
            .dim()
            .fill_w(),
        row![text("You").label(), text("→ Relay →").accent(), text("Them").label()].gap(12.0),
        row![
            heading("Relays I know").sub(),
            space(),
            button("Update from kahf.dev").act(|k: &mut Kahf| k.refresh_relay_directory(true)),
        ]
        .align_center(),
        caption("This list comes from kahf.dev when you are online, and is remembered for when you are not. KAHF works without the internet: relays you add by callsign are used the same way.")
            .dim()
            .fill_w(),
    ]
    .gap(10.0);
    if k.m.relays.is_empty() && hints.is_empty() {
        c = c.push(note(
            "No relays known yet. Update from kahf.dev when you are online, or add one by callsign below.",
            false,
        ));
    }
    for r in &k.m.relays {
        let call = r.call.clone();
        let used = hints.contains(&r.call);
        let h = hints.clone();
        c = c.push(
            row![
                text(r.call.clone()).label().w(110.0),
                text(
                    r.km.map(|d| format!("{} km", d.round()))
                        .unwrap_or_else(|| "-".into())
                )
                .w(90.0),
                caption(match r.source.as_str() {
                    "server" => "kahf.dev",
                    "heard" => "heard",
                    _ => "added",
                })
                .dim()
                .w(80.0),
                caption(
                    r.last_seen
                        .map(|t| time_ago(t, now_ms()))
                        .unwrap_or_else(|| "-".into())
                )
                .dim()
                .fill_w(),
                check("Use it", used).act(move |k: &mut Kahf, _| toggle(k, &h, &call)),
            ]
            .gap(10.0)
            .align_center(),
        );
    }
    for h in hints.iter().filter(|h| !k.m.relays.iter().any(|r| &r.call == *h)) {
        let (call, all) = (h.clone(), hints.clone());
        c = c.push(
            row![
                text(h.clone()).label().w(110.0),
                caption("added").dim().fill_w(),
                button("Remove")
                    .ghost()
                    .act(move |k: &mut Kahf| toggle(k, &all, &call)),
            ]
            .gap(10.0)
            .align_center(),
        );
    }
    let problem = callsign::problem(&k.ui.manual_relay);
    let hints2 = hints.clone();
    c = c.push(
        row![
            edit(&k.ui.manual_relay, "Add a relay by callsign", |k, v| k
                .ui
                .manual_relay =
                v.to_uppercase())
            .w(260.0),
            button("Add")
                .enabled(!k.ui.manual_relay.is_empty() && problem.is_none())
                .act(move |k: &mut Kahf| {
                    let c = callsign::normalise(&k.ui.manual_relay);
                    if !c.is_empty() && !hints2.contains(&c) {
                        let mut next = hints2.clone();
                        next.push(c);
                        k.set_relay_hints(next);
                    }
                    k.ui.manual_relay.clear();
                }),
        ]
        .gap(10.0)
        .align_center(),
    );
    if let Some(p) = problem {
        c = c.push(text(p).caption().danger());
    }
    c = c.push(
        caption(if hints.is_empty() {
            "No relay is chosen, so messages go directly or not at all.".to_string()
        } else {
            format!("Order of use: {}.", hints.join(", "))
        })
        .dim(),
    );
    c.into()
}

fn toggle(k: &mut Kahf, hints: &[String], call: &str) {
    let next: Vec<String> = if hints.iter().any(|c| c == call) {
        hints.iter().filter(|c| *c != call).cloned().collect()
    } else {
        let mut n = hints.to_vec();
        n.push(call.to_string());
        n
    };
    k.set_relay_hints(next);
}

fn run_relay(k: &Kahf) -> V {
    if !k.m.relay.running {
        return start_panel(k);
    }
    let r = &k.m.relay;
    let mut c = col![
        row![
            text("● Relay mode is on").success(),
            space(),
            button("Stop relay mode").act(|k: &mut Kahf| k.set_relay(false)),
        ]
        .align_center(),
        row![
            stat_box(r.forwarded_today.to_string(), "Sent today"),
            stat_box(r.forwarded_total.to_string(), "Sent in total"),
            stat_box(r.outbox.len().to_string(), "Waiting to go out")
        ]
        .gap(14.0),
        heading("kahf.dev").sub(),
    ]
    .gap(10.0);
    let kv = |a: &str, b: V| -> V { row![text(a).dim().w(120.0), b].gap(12.0).into() };
    c = c.push(kv(
        "Link",
        match &r.server {
            Some(s) if s.registered => text("Registered").success().into(),
            Some(_) => text("Not registered yet").accent().into(),
            None => text("Not connected").accent().into(),
        },
    ));
    if let Some(s) = &r.server {
        c = c.push(kv(
            "Schedule",
            match s.last_schedule_unix {
                Some(t) => text(format!(
                    "{} slot{}, received at {} UTC",
                    s.slots,
                    if s.slots == 1 { "" } else { "s" },
                    utc_hm(t)
                ))
                .into(),
                None => text("None received yet").dim().into(),
            },
        ));
        c = c.push(kv(
            "Next ask",
            text(format!("{} UTC", utc_hm(s.next_attempt_unix))).into(),
        ));
        if let Some(e) = &s.last_error {
            c = c.push(kv("Last problem", text(e.clone()).danger().fill_w().into()));
        }
    } else if let Some(n) = &r.server_note {
        c = c.push(kv("Why", text(n.clone()).dim().fill_w().into()));
    }
    c = c.push(kv(
        "On the map",
        match &r.area {
            Some(a) => text(a.clone()).into(),
            None => text("- Not listed: share a location in Settings to appear on the map.")
                .dim()
                .into(),
        },
    ));
    let mut reach = row![].gap(10.0).align_center();
    if let Some(s) = &k.ui.server {
        reach = reach.push(if s.reachable {
            text("Yes").success()
        } else {
            text("No").danger()
        });
        reach = reach.push(
            caption(if s.reachable {
                format!("{} ms, {} stations listed", s.millis, s.stations_listed)
            } else {
                s.error.clone().unwrap_or_default()
            })
            .dim(),
        );
    }
    reach = reach.push(button("Check kahf.dev").act(|k: &mut Kahf| k.check_server()));
    c = c.push(kv("Reachable", reach.into()));

    c = c.push(heading("Lane referee").sub());
    let rf = r.referee.clone().unwrap_or_default();
    c = c.push(
        row![
            stat_box(rf.rounds.to_string(), "rounds run"),
            stat_box(rf.requests.to_string(), "requests heard"),
            stat_box(rf.grants.to_string(), "turns granted"),
            stat_box(rf.heres.to_string(), "operators said hello"),
        ]
        .gap(10.0),
    );
    c = c.push(caption("Every twelve seconds the relay announces itself and gives stations a turn on its lane. Nothing is stored on a server: the server only plans which relay serves which lane.").dim().fill_w());
    if r.slots.is_empty() {
        c = c.push(caption("No fixed schedule is shown: the relay plans its own turns on the lane.").dim().fill_w());
    } else {
        let now = if r.now_unix > 0 { r.now_unix } else { now_unix() };
        for s in &r.slots {
            c = c.push(
                row![
                    text(format!("{} UTC", utc_hm(s.start_unix))).w(110.0),
                    caption(relative(s.start_unix, now)).dim().w(120.0),
                    text(channel_khz(s.channel_khz)).w(150.0),
                    text(duration(s.duration_s)).w(90.0),
                    text(if s.direction == "transmit" {
                        "Sends"
                    } else {
                        "Listens"
                    }),
                ]
                .gap(10.0),
            );
        }
    }
    if !r.outbox.is_empty() {
        c = c.push(heading("Outbox").sub());
        for o in &r.outbox {
            c = c.push(row![text(format!("#{}", o.id)).w(80.0), text(o.what.clone())].gap(10.0));
        }
    }
    if !r.per_hour.is_empty() {
        c = c.push(heading("Sent per hour").sub());
        let max = r.per_hour.iter().map(|(_, n)| *n).max().unwrap_or(1).max(1);
        let mut hours = row![].gap(8.0);
        for (h, n) in &r.per_hour {
            let frac = (*n as f32 / max as f32).max(0.06);
            hours = hours.push(
                col![
                    alloy_ui::progress::<A>(0.0..=1.0, frac).w(28.0),
                    text(n.to_string()).caption(),
                    caption(utc_hm(*h)).dim(),
                ]
                .gap(2.0),
            );
        }
        c = c.push(hours);
    }
    c = c.push(heading("What it has done").sub());
    if r.recent.is_empty() {
        c = c.push(text("Nothing forwarded yet. Quiet is normal: relays are used when stations cannot reach each other.").dim().fill_w());
    } else {
        for h in &r.recent {
            c = c.push(
                row![
                    text(clock(h.at)).w(80.0),
                    text(h.from.clone()).label().w(110.0),
                    text(h.channel_khz.map(channel_khz).unwrap_or_else(|| "-".into())).w(150.0),
                    caption(h.kind.clone()).dim(),
                ]
                .gap(10.0),
            );
        }
    }
    let _ = fmt::mhz;
    c.into()
}

fn stat_box(n: String, label: &str) -> V {
    col![text(n).heading(), caption(label).dim()]
        .gap(0.0)
        .pad(12.0)
        .round(8.0)
        .outline()
        .w(170.0)
        .into()
}

fn start_panel(k: &Kahf) -> V {
    let sharing = k.m.settings.location.mode != LocationMode::Off && k.m.settings.location.lat.is_some();
    let ready = k.ui.licence && k.ui.terms && !k.m.settings.listen_only;
    let mut c = col![
        text("Running a relay means your station passes messages between others, by itself, while your radio stays on. It helps the whole network, and it is a responsibility.")
            .dim()
            .fill_w(),
        heading("Before you start").sub(),
        text("• Your radio and computer stay on and attended."),
        text("• Your licence allows automatic or unattended operation."),
        text(format!("• You share a location, so the relay is listed on the map.{}", if sharing { "" } else { " Not set yet: do it in Settings." })).fill_w(),
        check("My licence allows unattended or automatic operation.", k.ui.licence).act(|k: &mut Kahf, v| k.ui.licence = v),
        check("I accept the relay terms: I will keep the station running as agreed.", k.ui.terms).act(|k: &mut Kahf, v| k.ui.terms = v),
        button("Start relay mode").primary().enabled(ready).act(|k: &mut Kahf| k.set_relay(true)),
    ]
    .gap(8.0);
    if k.m.settings.listen_only {
        c = c.push(
            text("Listen-only is on, so a relay cannot transmit.")
                .caption()
                .danger(),
        );
    }
    c.into()
}
