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
    let mut side = col![
        heading("Channel").sub(),
        row![
            tabs(bands::channels().iter().map(|c| c.name), current.unwrap_or(0))
                .act(|k: &mut Kahf, i: usize| k.set_frequency(bands::channels()[i].khz)),
            text(fmt::mhz(s.freq_khz)),
        ]
        .gap(10.0)
        .align_center(),
        caption("KAHF uses 17 metres only (18.111 to 18.120 MHz), in three channels, each 2.7 kHz wide. Tune your radio to the frequency shown. A relay works on all three.")
            .dim(),
        heading("How the radio is heard").sub(),
        audio_picker(k, Where::Radio),
        heading("Radio control").sub(),
        caption("Optional. KAHF can read and set your radio's frequency through Hamlib, so you never touch the dial. Sound still comes through the sound card.").dim(),
        rig_picker(k, Where::Radio),
    ]
    .gap(10.0);
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
