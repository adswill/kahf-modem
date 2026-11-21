use alloy_ui::prelude::*;

use super::kit::*;
use crate::app::Kahf;
use crate::model::Page;

pub fn view(_k: &Kahf) -> V {
    col![
        heading("Welcome to KAHF").display(),
        text("No internet needed").heading(),
        text("KAHF connects your computer to an amateur radio and sends what you write to other people's computers, even where there is no phone signal at all.")
            .dim(),
        col![
            step("You write", "a message, a voice note or a file, to someone's callsign."),
            step("Your radio sends it", "as a short burst of sound, through the air."),
            step("They receive it", "on their screen. If they are far, a relay passes it on."),
        ]
        .gap(10.0),
        titled(
            "You will need",
            col![
                text("An amateur radio licence to transmit. Listening needs none."),
                text("A radio for the shortwave bands, or an SDR receiver."),
                text("A cable or sound-card interface between the radio and this computer."),
            ]
            .gap(4.0)
        ),
        row![
            button("Just look around").act(|k: &mut Kahf| k.explore()),
            button("Set up KAHF").primary().act(|k: &mut Kahf| k.go(Page::Setup)),
        ]
        .gap(12.0),
    ]
    .gap(18.0)
    .pad(40.0)
    .w(720.0)
    .into()
}

fn step(title: &str, body: &str) -> V {
    row![text(title).label().w(190.0), text(body).dim().fill_w()]
        .gap(12.0)
        .into()
}
