//! Setup steps, the attach-file panel and the device lists.

use iced::widget::{center, column, container, opaque, pick_list, row, scrollable, slider, space, text_input, toggler};
use iced::{Fill, Length};

use crate::app::{App, Message, RigControl, SETUP_STEPS, TxAction};
use crate::icons;
use crate::strings as s;
use crate::theme::{self, Kind};
use crate::view::*;

const STEP_TITLES: [&str; SETUP_STEPS] = [s::STEP_CALL, s::STEP_RADIO, s::STEP_SDR, s::STEP_AUDIO, s::STEP_LISTEN, s::STEP_LOCATION];
const STEP_HINTS: [&str; SETUP_STEPS] =
    [s::STEP_CALL_HINT, s::STEP_RADIO_HINT, s::STEP_SDR_HINT, s::STEP_AUDIO_HINT, s::STEP_LISTEN_HINT, s::STEP_LOCATION_HINT];

impl App {
    fn detected_list(&self, items: &[String]) -> El<'_> {
        let mut col = column![
            row![
                label_caps(s::DETECTED),
                space().width(Fill),
                btn(s::RESCAN, Kind::Secondary).on_press(Message::Rescan),
            ]
            .align_y(iced::Center)
        ]
        .spacing(4);
        if items.is_empty() {
            col = col.push(muted(s::NOTHING_DETECTED));
        }
        for d in items {
            col = col.push(num(d.clone()).size(12));
        }
        col.into()
    }

    fn audio_pick(&self, label: &'static str, value: &str, on: fn(String) -> Message) -> El<'_> {
        let shown = if value.is_empty() { s::DEVICE_SYSTEM.to_string() } else { value.to_string() };
        field(label, pick_list(self.audio_options(), Some(shown), on).style(theme::dropdown).width(Fill))
    }

    /// Input level meter and the two volume sliders.
    pub fn volume_controls(&self) -> El<'_> {
        let slide = |label: &'static str, v: f32, on: fn(f32) -> Message| {
            field(
                label,
                row![slider(0.0..=100.0, v, on).step(1.0f32).width(Fill), small(s::level_pct(v)).width(40)]
                    .spacing(8)
                    .align_y(iced::Center),
            )
        };
        column![
            field(
                s::INPUT_LEVEL_LABEL,
                iced::widget::canvas(crate::icons::LevelBar { level: self.level_now, peak: self.level_peak })
                    .width(Fill)
                    .height(12),
            ),
            slide(s::VOL_IN_LABEL, self.vol_in, Message::VolIn),
            slide(s::VOL_OUT_LABEL, self.vol_out, Message::VolOut),
        ]
        .spacing(6)
        .into()
    }

    /// The controls of one setup step.
    pub fn step_controls(&self, i: usize) -> El<'_> {
        match i {
            0 => text_input("", &self.callsign)
                .on_input(Message::Callsign)
                .padding(5)
                .style(theme::input)
                .width(220)
                .into(),
            1 => {
                let mut col = column![
                    pick_list(RigControl::ALL, Some(self.rig), Message::RigChosen).style(theme::dropdown)
                ]
                .spacing(6);
                if self.rig == RigControl::Cat {
                    col = col.push(field(
                        s::RIG_ADDR_LABEL,
                        text_input(s::RIG_ADDR_PLACEHOLDER, &self.rig_addr)
                            .on_input(Message::RigAddr)
                            .padding(5)
                            .style(theme::input),
                    ));
                }
                col.into()
            }
            2 => {
                let mut col = column![
                    pick_list(self.sdr_options(), Some(self.sdr_selected()), Message::SdrChosen)
                        .style(theme::dropdown)
                        .width(Fill),
                ]
                .spacing(6);
                if self.sdr == s::SDR_RTL {
                    col = col.push(notice(s::SDR_RTL_NOTE, theme::AMBER));
                }
                col.push(self.detected_list(&self.detected_sdr)).into()
            }
            3 => column![
                self.audio_pick(s::SET_AUDIO_IN, &self.audio_in, Message::AudioIn),
                self.audio_pick(s::SET_AUDIO_OUT, &self.audio_out, Message::AudioOut),
                self.volume_controls(),
                self.detected_list(&self.detected_audio),
            ]
            .spacing(6)
            .into(),
            5 => {
                let mut col = column![].spacing(4);
                for l in crate::app::Location::ALL {
                    col = col.push(iced::widget::radio(l.label(), l, Some(self.location), Message::LocationChosen));
                }
                col.into()
            }
            _ => toggler(self.listen_only)
                .label(s::SET_LISTEN)
                .size(14)
                .text_size(12)
                .style(theme::switch)
                .on_toggle(Message::ToggleListen)
                .into(),
        }
    }

    fn step_chip(&self, i: usize) -> El<'_> {
        if self.setup_state()[i] {
            chip(s::SETUP_DONE_CHIP, theme::ACCENT)
        } else {
            chip(s::SETUP_ATTENTION_CHIP, theme::AMBER)
        }
    }

    /// Settings section: the five steps, each with its state and an Open button.
    pub fn setup_section(&self) -> El<'_> {
        let mut col = column![muted(s::SETUP_SETTINGS_DESC)].spacing(4);
        for (i, title) in STEP_TITLES.iter().enumerate() {
            col = col.push(
                row![
                    num((i + 1).to_string()).color(theme::MUTED).width(16),
                    t(*title).width(Fill),
                    self.step_chip(i),
                    btn(s::SETUP_REVISIT, Kind::Secondary).on_press(Message::OpenSetup(Some(i))),
                ]
                .spacing(8)
                .align_y(iced::Center),
            );
        }
        col.push(btn(s::SETUP_RUN_ALL, Kind::Primary).on_press(Message::OpenSetup(None))).into()
    }

    /// The setup panel: every step, or only the one that was opened from Settings.
    pub fn setup_modal(&self) -> El<'_> {
        let state = self.setup_state();
        let mut body = column![].spacing(8);
        for i in 0..SETUP_STEPS {
            if self.setup_focus.is_some_and(|f| f != i) {
                continue;
            }
            body = body.push(
                column![
                    container(
                        row![
                            num(format!("{}", i + 1)).color(theme::AMBER),
                            label_caps(STEP_TITLES[i]).color(theme::TEXT),
                            space().width(Fill),
                            icons::dot(if state[i] { theme::ACCENT } else { theme::AMBER }, None),
                        ]
                        .spacing(6)
                        .align_y(iced::Center),
                    )
                    .padding([3, 8])
                    .width(Fill)
                    .style(theme::header_row),
                    container(column![muted(STEP_HINTS[i]).size(11), self.step_controls(i)].spacing(6))
                        .padding(8)
                        .width(Fill),
                ]
                .width(Fill),
            );
        }
        let buttons = row![
            space().width(Fill),
            btn(s::SETUP_BACK, Kind::Secondary).on_press(Message::SetupClose),
            btn(s::SETUP_START, Kind::Primary).on_press(Message::SetupFinish),
        ]
        .spacing(6);
        let col = column![
            strip(s::SETUP_TITLE),
            container(
                column![
                    muted(s::SETUP_INTRO),
                    scrollable(body).height(Length::Fixed(730.0)),
                    buttons,
                ]
                .spacing(8),
            )
            .padding(10),
        ];
        center(opaque(container(col).width(520).max_height(880).style(theme::modal(self.modal_alpha)))).into()
    }

    /// Attach-file panel in the composer: the path, then Send file, which asks to confirm.
    pub fn attach_panel(&self) -> El<'_> {
        let size = if self.attach_bytes() > 0 { s::size_text(self.attach_bytes()) } else { String::new() };
        container(
            column![
                row![
                    label_caps(s::ATTACH_PATH_LABEL),
                    text_input(s::ATTACH_PATH_PLACEHOLDER, &self.attach_path)
                        .on_input(Message::AttachPath)
                        .padding(4)
                        .style(theme::input)
                        .width(Fill),
                    tip(
                        btn(s::ATTACH_SEND, Kind::Primary)
                            .on_press_maybe((!self.listen_only).then_some(Message::RequestTx(TxAction::SendFile))),
                        if self.listen_only { s::TIP_SEND_BLOCKED } else { s::TIP_ATTACH },
                    ),
                    btn(s::ATTACH_CANCEL, Kind::Secondary).on_press(Message::AttachOpen(false)),
                ]
                .spacing(6)
                .align_y(iced::Center),
                num(size).size(11).color(theme::MUTED),
            ]
            .spacing(3),
        )
        .padding(6)
        .width(Fill)
        .style(theme::raised)
        .into()
    }
}
