//! Chats: the people on the left, the conversation, the composer.

use alloy_ui::prelude::*;

use super::kit::*;
use super::widgets::{level_bar, wave};
use crate::app::*;
use crate::model::*;
use crate::types::*;
use crate::util::callsign;
use crate::util::fmt::{self, Quality};

const KINDS: [(MessageKind, &str, &str); 4] = [
    (MessageKind::Chat, "Normal", "A plain message. Anyone listening to the channel could read it."),
    (MessageKind::Private, "Private", "Only the station you write to keeps it. Other stations that hear it ignore it. The radio signal itself is still open."),
    (MessageKind::Signed, "Signed", "Adds your digital signature, so the reader can check it really is from you."),
    (MessageKind::Urgent, "Urgent", "Goes to the front of the queue. Use it for real emergencies only."),
];

fn mmss(s: f64) -> String {
    format!("{}:{:02}", (s / 60.0) as u32, (s % 60.0) as u32)
}

fn state_text(k: &Kahf, m: &Msg) -> &'static str {
    match m.state {
        Some(OutState::Queued) => {
            if k.m.settings.auto_link {
                "waiting for the channel"
            } else {
                "waiting"
            }
        }
        Some(OutState::Calling) => "reaching them",
        Some(OutState::Sending) => "sending",
        Some(OutState::Sent) => "sent",
        Some(OutState::Failed) => "not delivered",
        None => "",
    }
}

fn select(peer: String) -> impl Fn(&mut Kahf) + Send + Sync + 'static {
    move |k: &mut Kahf| {
        k.ui.peer = peer.clone();
        k.m.mark_chats_read();
    }
}

pub fn view(k: &Kahf) -> V {
    row![people(k), thread(k)].gap(0.0).fill().into()
}

fn people(k: &Kahf) -> V {
    let find = k.ui.find.trim().to_uppercase();
    let peer = callsign::normalise(&k.ui.peer);
    let mut list = col![].gap(4.0);
    let conv: Vec<Conversation> = k.m.conversations().into_iter().filter(|c| c.peer.contains(&find)).collect();
    let air: Vec<&Heard> = k.m.on_the_air().into_iter().filter(|h| h.call.contains(&find)).take(12).collect();
    if !conv.is_empty() {
        list = list.push(caption("Conversations").dim());
        for c in conv {
            let unread = if c.unread > 0 { format!("  ({})", c.unread) } else { String::new() };
            let head = format!("{}{unread}   {}", c.peer, fmt::clock(c.last_at));
            let sub = format!("{}{}", if c.last_out { "You: " } else { "" }, c.last_text);
            list = list.push(
                col![button(head).nav(peer == c.peer).fill_w().act(select(c.peer.clone())), caption(sub).dim().no_wrap()].gap(0.0),
            );
        }
    }
    if !air.is_empty() {
        list = list.push(caption("On the air now").dim());
        for h in air {
            let q = Quality::of(h.snr);
            let head = format!("{}   {}", h.call, q.label());
            let sub = format!("heard {}", fmt::time_ago(h.at, now_ms()));
            list = list.push(col![button(head).nav(peer == h.call).fill_w().act(select(h.call.clone())), caption(sub).dim()].gap(0.0));
        }
    }
    if k.m.conversations().is_empty() && k.m.on_the_air().is_empty() {
        list = list.push(caption("Nobody yet. Stations appear here when KAHF hears them.").dim().fill_w());
    }
    col![input(k.ui.find.clone()).hint("Find a callsign").act(|k: &mut Kahf, v: String| k.ui.find = v), scroll(list.pad(2.0)).fill()]
        .gap(8.0)
        .pad(10.0)
        .w(280.0)
        .fill_h()
        .into()
}

fn thread(k: &Kahf) -> V {
    let peer = callsign::normalise(&k.ui.peer);
    let problem = callsign::problem(&k.ui.peer);
    let ok = !peer.is_empty() && problem.is_none();
    let linked = matches!(&k.m.link, LinkState::Linked { peer: p } if *p == peer);
    let heard = k.m.heard.iter().find(|h| h.call == peer);

    let mut top = row![
        text("To").dim(),
        input(k.ui.peer.clone()).hint("Callsign, like KA1ABC").act(|k: &mut Kahf, v: String| k.ui.peer = v.to_uppercase()).w(220.0),
    ]
    .gap(10.0)
    .align_center()
    .pad(10.0)
    .fill_w();
    if ok {
        let snr = if linked { k.m.metrics.as_ref().and_then(|m| m.own_snr) } else { heard.and_then(|h| h.snr) };
        let q = Quality::of(snr);
        let label = match (linked, snr) {
            (true, Some(s)) => format!("{}  {s} dB", q.label()),
            _ => q.label().to_string(),
        };
        top = top.push(text(label).caption().accent());
        if linked {
            if let Some(m) = &k.m.metrics {
                top = top.push(caption(format!("{} · {} bit/s", m.mode, m.net_bps)).dim());
            }
            top = top.push(button("Hang up").act(|k: &mut Kahf| k.hang_up()));
        } else {
            let p = peer.clone();
            top = top.push(button("Connect now").enabled(!k.m.settings.listen_only).act(move |k: &mut Kahf| k.call_station(&p)));
        }
    }

    let mut bubbles = col![].gap(10.0).pad(14.0).fill_w();
    if !ok {
        bubbles = bubbles.push(start_panel(k));
    } else {
        let thread = k.m.thread(&peer);
        if thread.is_empty() {
            bubbles = bubbles.push(text(format!("No messages with {peer} yet.")).dim());
            bubbles = bubbles.push(caption(route(k, &peer, linked, heard.is_some())).dim().fill_w());
        }
        for m in thread {
            bubbles = bubbles.push(bubble(k, m));
        }
    }
    col![top, scroll(bubbles).fill(), compose(k, &peer, ok, linked, heard.is_some())].gap(0.0).fill().into()
}

fn route(k: &Kahf, peer: &str, linked: bool, heard: bool) -> String {
    let _ = peer;
    if linked {
        "Direct link".into()
    } else if heard {
        "Heard directly. KAHF will contact them when you send.".into()
    } else if let Some(r) = k.m.settings.relay_hints.first() {
        format!("Not heard. KAHF will try a relay ({r}).")
    } else {
        "Not heard yet. KAHF will try them directly.".into()
    }
}

fn start_panel(k: &Kahf) -> V {
    let mut c = col![
        heading("Start a conversation"),
        text("Pick someone on the left, or type their callsign above. Then write below. KAHF handles the radio.").dim().fill_w(),
    ]
    .gap(10.0);
    for s in k.m.steps() {
        c = c.push(
            row![
                text(if s.done { "✓" } else { "○" }).accent_if(s.done),
                col![text(s.title).label(), caption(s.why).dim()].gap(0.0),
            ]
            .gap(10.0),
        );
    }
    c = c.push(
        row![
            button("60-second tour").act(|k: &mut Kahf| {
                k.ui.tour_step = 0;
                k.m.tour_open = true;
            }),
            button("How KAHF works").ghost().act(|k: &mut Kahf| {
                k.ui.help_topic = "how".into();
                k.m.help_open = true;
            }),
        ]
        .gap(10.0),
    );
    c.into()
}

fn bubble(k: &Kahf, m: &Msg) -> V {
    let out = m.dir == Dir::Out;
    let mut c = col![].gap(6.0);
    if let Some(a) = &m.attachment {
        if a.kind == AttachmentKind::Voice {
            let playing = k.ui.playing.as_ref().filter(|p| p.0 == m.id);
            let secs = a.seconds.unwrap_or(0.0);
            let frac = playing.map(|(_, t, total)| (t.elapsed().as_secs_f64() / total.max(0.001)).min(1.0) as f32).unwrap_or(0.0);
            let (id, path) = (m.id.clone(), a.path.clone());
            let clock = match playing {
                Some((_, t, _)) => format!("{} / {}", mmss(t.elapsed().as_secs_f64()), mmss(secs)),
                None => mmss(secs),
            };
            c = c.push(
                row![
                    button(if playing.is_some() { "Pause" } else { "Play" }).act(move |k: &mut Kahf| k.toggle_play(&id, path.clone(), secs)),
                    wave(&m.id, frac, 30),
                    caption(clock).dim(),
                ]
                .gap(10.0)
                .align_center(),
            );
        } else {
            let pct = a.done.map(|d| (d as f32 / a.size.max(1) as f32).min(1.0)).unwrap_or(0.0);
            let mut size = fmt::size_words(a.size);
            if out && !matches!(m.state, Some(OutState::Sent) | Some(OutState::Failed)) && pct > 0.0 {
                size = format!("{size} · {}%", (pct * 100.0).round());
            }
            let mut f = row![col![text(a.name.clone()).label(), caption(size).dim()].gap(0.0).fill_w()].gap(10.0).align_center();
            if !out {
                if let Some(path) = a.path.clone() {
                    f = f.push(button("Open").act(move |k: &mut Kahf| k.open_path(path.clone())));
                }
            }
            c = c.push(f);
            if out && m.state == Some(OutState::Sending) {
                c = c.push(progress(0.0..=1.0, pct));
            }
        }
    }
    if !m.text.is_empty() {
        c = c.push(text(m.text.clone()).fill_w());
    }
    let mut meta = vec![];
    if m.kind != MessageKind::Chat {
        meta.push(m.kind.as_str().to_string());
    }
    meta.push(fmt::clock(m.at));
    if out {
        meta.push(state_text(k, m).to_string());
    }
    let tag = caption(meta.join(" · ")).dim();
    c = c.push(if m.state == Some(OutState::Failed) { text(meta.join(" · ")).caption().danger() } else { tag });
    if m.state == Some(OutState::Failed) {
        c = c.push(text(m.note.clone().unwrap_or_else(|| "The other station did not answer.".into())).caption().fill_w());
        let (id, id2, peer, body, kind, has_att) = (m.id.clone(), m.id.clone(), m.peer.clone(), m.text.clone(), m.kind, m.attachment.is_some());
        c = c.push(
            row![
                button("Try again").act(move |k: &mut Kahf| {
                    k.discard(&id);
                    if !has_att {
                        k.send_message(&peer, &body, kind);
                    }
                }),
                button("Delete").ghost().act(move |k: &mut Kahf| k.discard(&id2)),
            ]
            .gap(8.0),
        );
    }
    let card = c.pad(10.0).round(10.0).w(460.0);
    let card: V = if out { card.bg(alloy_ui::rgb(0x0c6b5f).with_alpha(0.12)).into() } else { card.outline().into() };
    if out {
        row![space(), card].fill_w().into()
    } else {
        row![card, space()].fill_w().into()
    }
}

fn compose(k: &Kahf, peer: &str, ok: bool, linked: bool, heard: bool) -> V {
    let mut c = col![].gap(8.0).pad(10.0).fill_w();
    if k.m.settings.listen_only {
        c = c.push(note("Listen-only is on, so KAHF cannot send. Turn it off in Settings, under Sending.", true));
    }
    if k.m.recording {
        let secs = k.ui.rec_start.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
        let to = peer.to_string();
        c = c.push(
            row![
                text("●").danger(),
                text(mmss(secs)),
                level_bar(k.m.record_level, 160.0),
                caption(format!("about {} bytes so far", (secs * 88.0).round())).dim(),
                space(),
                button("Cancel").ghost().act(|k: &mut Kahf| k.cancel_recording()),
                button("Send voice note").primary().act(move |k: &mut Kahf| k.finish_recording(to.clone())),
            ]
            .gap(12.0)
            .align_center(),
        );
    } else {
        let p1 = peer.to_string();
        let p2 = peer.to_string();
        let p3 = peer.to_string();
        let can = ok && !k.m.settings.listen_only && !k.ui.draft.trim().is_empty();
        let send = move |k: &mut Kahf| {
            let (text, kind) = (k.ui.draft.clone(), k.ui.kind);
            if !text.trim().is_empty() {
                k.send_message(&p3, &text, kind);
                k.ui.draft.clear();
            }
        };
        c = c.push(
            row![
                button("Attach").ghost().enabled(ok).act(move |k: &mut Kahf| {
                    let to = p1.clone();
                    k.pick_files(move |k, files| k.ask_send_files(&to, files));
                }),
                input(k.ui.draft.clone())
                    .hint(if ok { format!("Message {peer}") } else { "Choose who to write to".into() })
                    .act(|k: &mut Kahf, v: String| k.ui.draft = v)
                    .submit(send.clone())
                    .fill_w(),
                button("Record").ghost().enabled(ok).act(move |k: &mut Kahf| {
                    if !callsign::valid(&p2) {
                        k.warn("Choose who to send the voice note to first.");
                    } else {
                        k.start_recording();
                    }
                }),
                button("Send").primary().enabled(can).act(send),
            ]
            .gap(8.0)
            .align_center(),
        );
        let sel = KINDS.iter().position(|(m, _, _)| *m == k.ui.kind).unwrap_or(0);
        c = c.push(
            row![
                caption("Send as").dim(),
                tabs(KINDS.iter().map(|(_, n, _)| *n), sel).act(|k: &mut Kahf, i: usize| k.ui.kind = KINDS[i].0),
                caption(KINDS[sel].2).dim(),
                space(),
                caption(route(k, peer, linked, heard)).dim(),
            ]
            .gap(10.0)
            .align_center(),
        );
    }
    c.into()
}
