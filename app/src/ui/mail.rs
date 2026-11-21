//! Mail: longer messages with a subject and attachments.

use alloy_ui::prelude::*;

use super::kit::*;
use crate::app::*;
use crate::model::Page;
use crate::types::*;
use crate::util::callsign;
use crate::util::fmt::{self, FILE_LIMIT_BYTES};

fn state_text(s: Option<OutState>) -> &'static str {
    match s {
        Some(OutState::Queued) => "waiting for the channel",
        Some(OutState::Calling) => "reaching them",
        Some(OutState::Sending) => "sending",
        Some(OutState::Sent) => "sent",
        Some(OutState::Failed) => "not delivered",
        None => "",
    }
}

fn new_mail(k: &mut Kahf, reply: Option<&MailItem>) {
    k.ui.mail_compose = true;
    k.ui.mail_open = None;
    k.ui.mail_files.clear();
    match reply {
        Some(r) => {
            k.ui.mail_to = r.peer.clone();
            k.ui.mail_subject = if r.subject.starts_with("Re: ") {
                r.subject.clone()
            } else {
                format!("Re: {}", r.subject)
            };
            let quoted: Vec<String> = r.body.lines().map(|l| format!("> {l}")).collect();
            k.ui.mail_body = format!("\n\n— {} wrote:\n{}", r.peer, quoted.join("\n"));
        }
        None => {
            k.ui.mail_to.clear();
            k.ui.mail_subject.clear();
            k.ui.mail_body.clear();
        }
    }
}

pub fn view(k: &Kahf) -> V {
    row![folders(k), listing(k), reader(k)].gap(0.0).fill().into()
}

fn folders(k: &Kahf) -> V {
    let inbox = if k.m.unread_mail() > 0 {
        format!("Inbox  ({})", k.m.unread_mail())
    } else {
        "Inbox".into()
    };
    col![
        button("Write mail")
            .primary()
            .fill_w()
            .act(|k: &mut Kahf| new_mail(k, None)),
        button(inbox)
            .nav(k.ui.mail_view == MailView::Inbox && !k.ui.mail_compose)
            .fill_w()
            .act(|k: &mut Kahf| {
                k.ui.mail_view = MailView::Inbox;
                k.ui.mail_compose = false;
            }),
        button("Sent")
            .nav(k.ui.mail_view == MailView::Sent && !k.ui.mail_compose)
            .fill_w()
            .act(|k: &mut Kahf| {
                k.ui.mail_view = MailView::Sent;
                k.ui.mail_compose = false;
            }),
        space(),
        caption("Mail is for longer messages with a subject and attachments. For quick lines, use Chats.")
            .dim()
            .fill_w(),
    ]
    .gap(6.0)
    .pad(10.0)
    .w(200.0)
    .fill_h()
    .into()
}

fn listing(k: &Kahf) -> V {
    let inbox = k.ui.mail_view == MailView::Inbox;
    let mut items: Vec<&MailItem> = k.m.mail.iter().filter(|m| (m.dir == Dir::In) == inbox).collect();
    items.sort_by_key(|m| std::cmp::Reverse(m.at));
    let mut c = col![caption(if inbox { "Inbox" } else { "Sent" }).dim()].gap(4.0);
    if items.is_empty() {
        c = c.push(
            caption(if inbox {
                "No mail yet. Mail from other stations arrives here by itself."
            } else {
                "Nothing sent yet."
            })
            .dim()
            .fill_w(),
        );
    }
    for m in items {
        let id = m.id.clone();
        let unread = m.dir == Dir::In && !m.read;
        let head = format!(
            "{}{}   {}",
            if unread { "● " } else { "" },
            m.peer,
            fmt::clock(m.at)
        );
        let snippet: String = m
            .body
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(70)
            .collect();
        c = c.push(
            col![
                button(head)
                    .nav(k.ui.mail_open.as_deref() == Some(&m.id))
                    .fill_w()
                    .act(move |k: &mut Kahf| {
                        k.ui.mail_open = Some(id.clone());
                        k.ui.mail_compose = false;
                        k.m.go(Page::Mail);
                    }),
                text(m.subject.clone()).label().no_wrap(),
                caption(snippet).dim().no_wrap(),
            ]
            .gap(0.0),
        );
    }
    scroll(c.pad(10.0)).w(320.0).fill_h().into()
}

fn reader(k: &Kahf) -> V {
    if k.ui.mail_compose {
        return compose(k);
    }
    let Some(m) =
        k.ui.mail_open
            .as_ref()
            .and_then(|id| k.m.mail.iter().find(|m| &m.id == id))
    else {
        return col![text("No message selected. Pick one on the left, or write a new one.").dim()]
            .pad(20.0)
            .fill()
            .into();
    };
    let mut meta = row![
        text(format!(
            "{} {}",
            if m.dir == Dir::In { "From" } else { "To" },
            m.peer
        )),
        caption(fmt::time_ago(m.at, now_ms())).dim(),
    ]
    .gap(10.0);
    if m.dir == Dir::Out {
        let t = text(state_text(m.state)).caption();
        meta = meta.push(match m.state {
            Some(OutState::Sent) => t.success(),
            Some(OutState::Failed) => t.danger(),
            _ => t.accent(),
        });
    }
    let mut c = col![
        heading(m.subject.clone()),
        meta,
        rule(),
        text(m.body.clone()).fill_w()
    ]
    .gap(10.0);
    if !m.files.is_empty() {
        c = c.push(caption("Attachments").dim());
        for f in &m.files {
            let mut r = row![
                text(f.name.clone()).fill_w(),
                caption(fmt::size_words(f.size)).dim()
            ]
            .gap(10.0)
            .align_center();
            if let Some(p) = f.path.clone() {
                r = r.push(button("Open").act(move |k: &mut Kahf| k.open_path(p.clone())));
            }
            c = c.push(r);
        }
    }
    if m.state == Some(OutState::Failed) {
        c = c.push(note(
            m.note
                .clone()
                .unwrap_or_else(|| "The other station did not answer.".into()),
            true,
        ));
    }
    if m.dir == Dir::In {
        let reply = m.clone();
        c = c.push(button("Reply").act(move |k: &mut Kahf| new_mail(k, Some(&reply))));
    }
    scroll(c.pad(20.0)).fill().into()
}

fn compose(k: &Kahf) -> V {
    let u = &k.ui;
    let problem = callsign::problem(&u.mail_to);
    let total: u64 = u.mail_files.iter().map(|f| f.size).sum();
    let ready = !callsign::normalise(&u.mail_to).is_empty()
        && problem.is_none()
        && (!u.mail_body.trim().is_empty() || !u.mail_files.is_empty())
        && total <= FILE_LIMIT_BYTES
        && !k.m.settings.listen_only;
    let mut c = col![
        heading("New mail"),
        lab(
            "To",
            edit(&u.mail_to, "Callsign, like KA1ABC", |k, v| k.ui.mail_to =
                v.to_uppercase())
        ),
    ]
    .gap(10.0);
    if let Some(p) = problem {
        c = c.push(text(p).caption().danger());
    }
    c = c.push(lab(
        "Subject",
        edit(&u.mail_subject, "", |k, v| k.ui.mail_subject = v),
    ));
    c = c.push(lab(
        "Message",
        editor(u.mail_body.clone())
            .act(|k: &mut Kahf, v: String| k.ui.mail_body = v)
            .h(220.0),
    ));
    c = c.push(button("Attach a file").act(|k: &mut Kahf| {
        k.pick_files(|k, files| k.ui.mail_files.extend(files));
    }));
    for (i, f) in u.mail_files.iter().enumerate() {
        c = c.push(
            row![
                text(f.name.clone()).fill_w(),
                caption(fmt::size_words(f.size)).dim(),
                button("Remove").ghost().act(move |k: &mut Kahf| {
                    if i < k.ui.mail_files.len() {
                        k.ui.mail_files.remove(i);
                    }
                })
            ]
            .gap(10.0)
            .align_center(),
        );
    }
    c = c.push(file_notice(k, &u.mail_files));
    let mut actions = row![
        button("Send mail").primary().enabled(ready).act(|k: &mut Kahf| {
            if k.send_mail() {
                k.ui.mail_compose = false;
                k.ui.mail_view = MailView::Sent;
            }
        }),
        button("Discard")
            .ghost()
            .act(|k: &mut Kahf| k.ui.mail_compose = false),
    ]
    .gap(10.0)
    .align_center();
    if k.m.settings.listen_only {
        actions = actions.push(
            text("Listen-only is on. Turn it off in Settings to send.")
                .caption()
                .danger(),
        );
    }
    scroll(c.push(actions).pad(20.0)).fill().into()
}
