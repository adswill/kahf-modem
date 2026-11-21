//! The Radio screen: the channel and the spectrum, how the radio is heard, and radio control.

use alloy_ui::prelude::*;

use super::hardware::{audio_picker, rig_picker, Where};
use super::kit::*;
use super::widgets::spectrum;
use crate::app::*;
use crate::util::geo::{self, Tone};
use crate::util::{bands, fmt};

pub fn view(k: &Kahf) -> V {
    let s = &k.m.settings;
    let mut left = col![].gap(12.0).fill();
    if let Some((tone, t)) = geo::reach_notice(s.location.lat, s.location.lon, now_unix()) {
        left = left.push(note(t, tone == Tone::Warn));
    }
    if k.m.tx_gate.state == "off" && !k.m.tx_gate.reason.is_empty() {
        left = left.push(note(k.m.tx_gate.reason.clone(), true));
    } else if k.m.tx_gate.state == "bench" {
        left = left.push(note(k.m.tx_gate.reason.clone(), false));
    }
    left = left.push(
        row![
            heading("The channel"),
            caption(fmt::mhz(s.freq_khz)).dim(),
            space(),
            lit("TX", k.m.transmitting, true),
            lit("RX", k.m.receiving && !k.m.transmitting, false),
        ]
        .gap(10.0)
        .align_center(),
    );
    left = left.push(spectrum(
        k.m.spectrum.clone(),
        k.m.waterfall.iter().cloned().collect(),
        330.0,
    ));
    left = left.push(caption("Brighter is stronger. The scale is the sound from your radio, in kilohertz. The shaded band is where KAHF listens.").dim());

    let current = bands::index_of(s.freq_khz);
    let lanes = bands::channels();
    let mut side = col![
        heading("Lane").sub(),
        row![
            text(current.map_or("not a KAHF lane".to_string(), |i| lanes[i].name.clone())).heading(),
            text(format!("dial {}", fmt::mhz(s.freq_khz))),
        ]
        .gap(10.0)
        .align_center(),
        caption("KAHF uses 17 metres only (18.111 to 18.120 MHz), in twelve narrow lanes, each 480 Hz wide. Set your radio to upper sideband and to the dial shown for the lane. C1 is where stations call each other; R lanes belong to relays. Relays take care of the rest, so start with C1.")
            .dim(),
    ]
    .gap(10.0);
    for half in lanes.chunks(6) {
        let mut r = row![].gap(6.0);
        for ch in half {
            let khz = ch.khz;
            r = r.push(
                button(ch.name.clone())
                    .nav(current == Some(ch.index))
                    .act(move |k: &mut Kahf| k.set_frequency(khz)),
            );
        }
        side = side.push(r);
    }
    if let Some(i) = current {
        side = side.push(caption(format!("{}: {}.", lanes[i].name, lanes[i].what)).dim());
    }
    side = side.push(col![
        heading("How the radio is heard").sub(),
        audio_picker(k, Where::Radio),
        heading("Radio control").sub(),
        caption("Optional. KAHF can read and set your radio's frequency through Hamlib, so you never touch the dial. Sound still comes through the sound card.").dim(),
        rig_picker(k, Where::Radio),
    ].gap(10.0));
    if k.hardware_dirty() {
        side = side.push(
            row![
                text("Changes are not applied yet.").caption(),
                button(if k.ui.applying {
                    "Restarting…"
                } else {
                    "Apply and restart the radio"
                })
                .primary()
                .enabled(!k.ui.applying)
                .act(|k: &mut Kahf| k.apply_hardware()),
            ]
            .gap(10.0)
            .align_center(),
        );
    }
    row![col![left].pad(16.0).fill(), scroll(side.pad(16.0)).w(420.0)]
        .gap(0.0)
        .fill()
        .into()
}
