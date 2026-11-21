//! Stations: the map, who was heard on the radio, and the address book.

use alloy_ui::prelude::*;

use super::kit::*;
use super::map;
use crate::app::*;
use crate::model::Page;
use crate::util::fmt::{self, Quality};
use crate::types::*;

fn message_button(k: &Kahf, call: String) -> V {
    button("Message").enabled(!k.m.settings.listen_only).act(move |k: &mut Kahf| {
        k.ui.peer = call.clone();
        k.go(Page::Chats);
    }).into()
}

pub fn view(k: &Kahf) -> V {
    let sharing = k.m.settings.location.mode != LocationMode::Off && k.m.settings.location.lat.is_some();
    let mut c = col![
        row![
            heading("Stations"),
            space(),
            button("Refresh from kahf.dev").act(|k: &mut Kahf| k.refresh_relay_directory(true)),
            button("Open the website map").ghost().act(|k: &mut Kahf| k.open_url("https://kahf.dev/map")),
        ]
        .gap(10.0)
        .align_center(),
        caption("A station is one person's KAHF setup, named by its callsign. The map shows relays, and stations that chose to share their location, from kahf.dev.")
            .dim()
            .fill_w(),
        map::view(k, map::pins(k), 340.0),
    ]
    .gap(10.0);
    if !sharing {
        c = c.push(
            row![
                caption("Your own position is not shown because you are not sharing a location.").dim(),
                button("Change that").ghost().act(|k: &mut Kahf| k.m.open_settings(crate::model::SettingsTab::Location)),
            ]
            .gap(8.0)
            .align_center(),
        );
    }
    c = c.push(row![heading("Heard on the radio").sub(), caption(k.m.heard.len().to_string()).dim()].gap(10.0));
    if k.m.heard.is_empty() {
        c = c.push(
            text("Nobody yet. Stations appear when they transmit, so this can take a few minutes, or longer when the band is quiet. If nothing ever appears, check the picture on the Radio screen.")
                .dim()
                .fill_w(),
        );
    } else {
        for h in &k.m.heard {
            c = c.push(heard_row(k, h));
        }
    }
    c = c.push(row![heading("Address book").sub(), caption(k.m.book.stations.len().to_string()).dim()].gap(10.0));
    if k.m.book.stations.is_empty() {
        c = c.push(text("No saved stations yet.").dim());
    } else {
        for s in &k.m.book.stations {
            c = c.push(
                row![
                    text(s.call.clone()).label().w(110.0),
                    text(s.name.clone()).w(200.0),
                    text(if s.qth.is_empty() { "-".to_string() } else { s.qth.clone() }).w(90.0),
                    caption(if s.last_heard.is_empty() { "never".to_string() } else { s.last_heard.clone() }).dim().fill_w(),
                    message_button(k, s.call.clone()),
                ]
                .gap(10.0)
                .align_center(),
            );
        }
    }
    scroll(c.pad(16.0)).fill().into()
}

fn heard_row(k: &Kahf, h: &Heard) -> V {
    let q = Quality::of(h.snr);
    let signal: String = if h.relay == Some(true) && h.snr.is_none() && h.hops.is_none() {
        format!("Relay · {}", if h.online == Some(true) { "online, reaches other areas" } else { "radio only" })
    } else if let (Some(hops), None) = (h.hops, h.snr) {
        format!("Announced · {}", if hops == 0 { "heard directly".to_string() } else { format!("through {hops} relay{}", if hops == 1 { "" } else { "s" }) })
    } else {
        match h.snr {
            Some(s) => format!("{} · {s} dB", q.label()),
            None => q.label().to_string(),
        }
    };
    row![
        text(h.call.clone()).label().w(110.0),
        text(signal).w(320.0),
        caption(fmt::time_ago(h.at, now_ms())).dim().fill_w(),
        message_button(k, h.call.clone()),
    ]
    .gap(10.0)
    .align_center()
    .into()
}
