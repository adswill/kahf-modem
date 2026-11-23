//! First-run setup: who you are, the radio, the place, ready. The lane starts on C1 (where
//! stations call each other); it can be changed later on the Radio screen.

use alloy_ui::prelude::*;

use super::hardware::{audio_picker, location_editor, rig_picker, Where};
use super::kit::*;
use super::widgets::spectrum;
use crate::app::*;
use crate::model::Page;
use crate::types::LocationMode;
use crate::util::{callsign, fmt, geo};

fn can_next(k: &Kahf) -> bool {
    let s = &k.ui.setup;
    match s.step {
        0 => {
            !s.have_licence
                || (callsign::valid(&callsign::normalise(&s.draft.callsign))
                    && callsign::problem(&s.draft.callsign).is_none())
        }
        2 => {
            s.draft.location.mode == LocationMode::Off
                || (geo::valid_lat(s.pos.lat()) && geo::valid_lon(s.pos.lon()))
        }
        _ => true,
    }
}

/// Why "Continue" is greyed, in words. Empty when it is not.
fn why_not_next(k: &Kahf) -> &'static str {
    match (can_next(k), k.ui.setup.step) {
        (true, _) => "",
        (false, 0) => "Type your callsign to continue, or choose listening only.",
        (false, _) => "Enter a valid position, or turn the map off.",
    }
}

pub fn view(k: &Kahf) -> V {
    let st = &k.ui.setup;
    let step = st.step;
    let last = step == SETUP_STEPS.len() - 1;
    let mut steps = row![].gap(14.0);
    for (i, s) in SETUP_STEPS.iter().enumerate() {
        let label = format!("{}  {s}", i + 1);
        steps = steps.push(text(label).caption().accent_if(i == step).dim_if(i > step));
    }
    let body: V = match step {
        0 => who(k),
        1 => radio(k),
        2 => place(k),
        _ => ready(k),
    };
    let cancel_to = if k.m.settings.setup_done {
        Page::Chats
    } else {
        Page::Welcome
    };
    col![
        row![
            heading("KAHF").accent(),
            text("Set up").dim(),
            space(),
            button("Cancel").ghost().act(move |k: &mut Kahf| k.go(cancel_to))
        ]
        .gap(12.0)
        .align_center(),
        steps,
        scroll(col![body].pad(4.0)).fill(),
        text(why_not_next(k)).caption().dim(),
        row![
            button("Back").enabled(step > 0).act(|k: &mut Kahf| {
                k.ui.setup.step = k.ui.setup.step.saturating_sub(1);
            }),
            space(),
            button(if last { "Start listening" } else { "Continue" })
                .primary()
                .enabled(can_next(k))
                .act(move |k: &mut Kahf| {
                    if k.ui.setup.step + 1 < SETUP_STEPS.len() {
                        k.ui.setup.step += 1;
                    } else {
                        k.finish_setup();
                    }
                }),
        ]
        .align_center(),
    ]
    .gap(14.0)
    .pad(24.0)
    .fill()
    .into()
}

fn who(k: &Kahf) -> V {
    let st = &k.ui.setup;
    let call = callsign::normalise(&st.draft.callsign);
    let msg = callsign::problem(&st.draft.callsign);
    let ok = callsign::valid(&call) && msg.is_none();
    let mut c = col![
        heading("Who are you on the air?"),
        para(
            "Every radio amateur has a callsign, like KA1ABC. It is how other stations know it is you.",
            100
        ),
        choice(
            st.have_licence,
            "I have an amateur radio licence",
            "I can transmit and I know my callsign.",
            |k| k.ui.setup.have_licence = true
        ),
    ]
    .gap(10.0);
    if st.have_licence {
        c = c.push(lab(
            "Your callsign",
            input(st.draft.callsign.clone())
                .hint("For example KA1ABC")
                .act(|k: &mut Kahf, v: String| k.ui.setup.draft.callsign = v.to_uppercase()),
        ));
        c = c.push(match msg {
            Some(m) => text(m).caption().danger(),
            None if ok => text("Looks good.").caption().success(),
            None => text("Letters and numbers, with at least one number.")
                .caption()
                .dim(),
        });
        c = c.push(note("Only transmit if your licence allows it on the frequency you use. KAHF cannot check that for you.", true));
    }
    c = c.push(choice(
        !st.have_licence,
        "I only want to listen for now",
        "KAHF will not transmit. You can add a callsign later in Settings.",
        |k| k.ui.setup.have_licence = false,
    ));
    c.into()
}

fn radio(k: &Kahf) -> V {
    let st = &k.ui.setup;
    let mut c = col![
        heading("Connect your radio"),
        para("Two separate things: how the sound reaches the computer, and, optionally, letting KAHF control the radio's frequency.", 100),
        heading("1. Sound").sub(),
        audio_picker(k, Where::Setup),
    ]
    .gap(10.0);
    if st.hw.kind != SoundKind::None {
        c = c.push(
            row![
                button("Check that I can hear it").act(|k: &mut Kahf| {
                    k.ui.setup.previewing = true;
                    let mut s = k.m.settings.clone();
                    k.ui.setup.hw.apply_to(&mut s);
                    s.callsign = if k.ui.setup.have_licence {
                        callsign::normalise(&k.ui.setup.draft.callsign)
                    } else {
                        String::new()
                    };
                    s.listen_only = true;
                    k.preview_engine(s);
                }),
                caption("Moving lines below mean it works.").dim(),
            ]
            .gap(12.0)
            .align_center(),
        );
        if st.previewing {
            c = c.push(spectrum(
                k.m.spectrum.clone(),
                k.m.waterfall.iter().cloned().collect(),
                200.0,
            ));
        }
    }
    c = c.push(heading("2. Radio control (optional)").sub());
    c = c.push(rig_picker(k, Where::Setup));
    c.into()
}

fn place(k: &Kahf) -> V {
    col![
        heading("Do you want to be on the map?"),
        para("The public map at kahf.dev shows stations that choose to share where they are. It is off unless you turn it on.", 100),
        location_editor(k, Where::Setup),
    ]
    .gap(10.0)
    .into()
}

fn ready(k: &Kahf) -> V {
    let st = &k.ui.setup;
    let call = callsign::normalise(&st.draft.callsign);
    let row_of = |a: &str, b: String| -> V { row![text(a).dim().w(140.0), text(b).label()].gap(12.0).into() };
    col![
        heading("You are ready"),
        text("This is what KAHF will do. You can change all of it later.").dim(),
        row_of(
            "Callsign",
            if st.have_licence {
                call
            } else {
                "listening only".into()
            }
        ),
        row_of(
            "Sound",
            match st.hw.kind {
                SoundKind::Sound => "Sound card".into(),
                SoundKind::Sdr => "SDR receiver".into(),
                SoundKind::None => "none yet".into(),
            }
        ),
        row_of(
            "Radio control",
            if st.hw.rig.model.is_some() {
                st.hw.rig.name.clone()
            } else {
                "off".into()
            }
        ),
        row_of("Channel", format!("{}, 17 m", fmt::mhz(st.draft.freq_khz))),
        row_of(
            "Map",
            match st.draft.location.mode {
                LocationMode::Off => "not shared".into(),
                LocationMode::Area => "a circle".into(),
                LocationMode::Precise => "exact position".into(),
            }
        ),
    ]
    .gap(8.0)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_has_no_channel_step_and_starts_on_the_calling_lane() {
        assert_eq!(SETUP_STEPS, ["You", "Radio", "Place", "Ready"]);
        assert!((crate::util::bands::default_khz() - 18_113.44).abs() < 0.01);
    }
}
