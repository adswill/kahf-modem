//! Window chrome (top bar, rail, status bar, modals) and shared widget helpers.

use iced::widget::text::IntoFragment;
use iced::widget::{
    button, center, column, container, mouse_area, opaque, pick_list, row, scrollable, space, stack, text,
    text_input, toggler, tooltip, Button, Row, Text,
};
use crate::icons::{self, Icon};
use iced::{Color, Element, Fill, Font, Length};

use crate::app::{App, Busy, Confirm, Message, Section, ToastKind, PALETTE_INPUT};
use crate::sample;
use crate::spectrum;
use crate::strings as s;
use crate::theme::{self, Kind};

pub type El<'a> = Element<'a, Message>;

// ------------------------------------------------------------------- clock

fn utc_parts() -> (u64, u64, u64) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    ((secs / 3600) % 24, (secs / 60) % 60, secs % 60)
}

pub fn utc_clock() -> String {
    let (h, m, sec) = utc_parts();
    format!("{h:02}:{m:02}:{sec:02}")
}

pub fn utc_hm() -> String {
    let (h, m, _) = utc_parts();
    format!("{h:02}:{m:02}")
}

// ----------------------------------------------------------------- helpers

pub fn t<'a>(content: impl IntoFragment<'a>) -> Text<'a> {
    text(content).size(12)
}

pub fn muted<'a>(content: impl IntoFragment<'a>) -> Text<'a> {
    t(content).color(theme::MUTED)
}

pub fn small<'a>(content: impl IntoFragment<'a>) -> Text<'a> {
    text(content).size(11).color(theme::MUTED)
}

pub fn mono<'a>(content: impl IntoFragment<'a>) -> Text<'a> {
    t(content).size(13).font(Font::MONOSPACE)
}

/// Numbers in tables: monospace, so the digits line up (tabular).
pub fn num<'a>(content: impl IntoFragment<'a>) -> Text<'a> {
    text(content).size(13).font(Font::MONOSPACE)
}

/// Page heading: a short upper-case label for a header strip.
pub fn heading<'a>(content: impl IntoFragment<'a>) -> Text<'a> {
    let fragment = content.into_fragment();
    text(fragment.to_uppercase()).size(12).color(theme::TEXT).font(theme::SEMIBOLD)
}

/// The one section label style: small caps look (upper case, 11 px, semibold, muted).
pub fn label_caps<'a>(content: &str) -> Text<'a> {
    text(content.to_uppercase()).size(10).color(theme::MUTED).font(theme::SEMIBOLD)
}

pub fn subheading<'a>(content: impl IntoFragment<'a>) -> Text<'a> {
    let fragment = content.into_fragment();
    label_caps(fragment.as_ref())
}

pub fn hline<'a>() -> El<'a> {
    container(space()).width(Fill).height(1).style(theme::hairline).into()
}

pub fn vline<'a>() -> El<'a> {
    container(space()).width(1).height(Fill).style(theme::hairline).into()
}

/// Raised surface: flat fill and a 1 px border, square.
pub fn raised<'a>(content: impl Into<El<'a>>) -> container::Container<'a, Message> {
    container(content.into()).style(theme::raised)
}

pub fn panel_box<'a>(content: impl Into<El<'a>>) -> container::Container<'a, Message> {
    container(content).padding(8).style(theme::panel)
}

/// A thin dark header strip with a short upper-case label and a 1 px border.
pub fn strip<'a>(title: &str) -> El<'a> {
    container(label_caps(title).color(theme::TEXT))
        .padding([3, 8])
        .width(Fill)
        .style(theme::header_row)
        .into()
}

/// A panel with a header strip.
pub fn section_box<'a>(title: &'static str, body: impl Into<El<'a>>) -> El<'a> {
    container(column![strip(title), container(body.into()).padding(8).width(Fill)].width(Fill))
        .width(Fill)
        .style(theme::panel)
        .into()
}

pub fn field<'a>(label: &'static str, widget: impl Into<El<'a>>) -> El<'a> {
    column![muted(label).size(12), widget.into()].spacing(4).into()
}

pub fn key_value<'a>(label: &'static str, value: impl IntoFragment<'a>) -> El<'a> {
    row![small(label).width(Length::FillPortion(2)), num(value).size(12).width(Length::FillPortion(3))]
        .spacing(6)
        .into()
}

/// Status chip: a dot plus a word.
pub fn chip<'a>(label: impl IntoFragment<'a>, color: Color) -> El<'a> {
    chip_pulse(label, color, None)
}

/// Status chip whose dot breathes while `pulse` is `Some`.
pub fn chip_pulse<'a>(label: impl IntoFragment<'a>, color: Color, pulse: Option<f32>) -> El<'a> {
    container(
        row![icons::dot(color, pulse), text(label).size(11).font(Font::MONOSPACE)]
            .spacing(2)
            .align_y(iced::Center),
    )
    .padding(iced::Padding { top: 1.0, right: 6.0, bottom: 1.0, left: 2.0 })
    .style(theme::chip(color))
    .into()
}

/// Small count pill for unread items.
pub fn badge<'a>(count: u32) -> El<'a> {
    container(text(count.to_string()).size(10).font(iced::Font::MONOSPACE).color(theme::BACKGROUND))
        .padding([0, 4])
        .style(|_: &iced::Theme| container::Style {
            background: Some(iced::Background::Color(theme::AMBER)),
            border: iced::Border { radius: 0.0.into(), ..Default::default() },
            ..Default::default()
        })
        .into()
}

pub fn notice<'a>(content: impl IntoFragment<'a>, color: Color) -> El<'a> {
    container(t(content).size(12))
        .padding([4, 8])
        .width(Fill)
        .style(theme::notice(color))
        .into()
}

/// Empty state: a small line drawing above the text.
pub fn empty_state<'a>(icon: Icon, title: &'static str, hint: &'static str) -> El<'a> {
    center(
        column![
            icons::icon(icon, 56.0, theme::with_alpha(theme::MUTED, 0.7)),
            space().height(6),
            t(title).size(16).font(theme::SEMIBOLD),
            muted(hint),
        ]
        .spacing(6)
        .align_x(iced::Center),
    )
    .into()
}

/// Button with the variants of `theme::Kind`. Primary and secondary get the inner top highlight.
pub fn btn<'a>(label: impl IntoFragment<'a>, kind: Kind) -> Button<'a, Message> {
    btn_content(text(label).size(12).into(), kind)
}

/// Button with a drawn icon before the label.
pub fn btn_icon<'a>(glyph: Icon, label: impl IntoFragment<'a>, kind: Kind) -> Button<'a, Message> {
    let color = if matches!(kind, Kind::Primary | Kind::Danger) { theme::BACKGROUND } else { theme::MUTED };
    btn_content(
        row![icons::icon(glyph, 13.0, color), text(label).size(12)]
            .spacing(5)
            .align_y(iced::Center)
            .into(),
        kind,
    )
}

fn btn_content<'a>(label: El<'a>, kind: Kind) -> Button<'a, Message> {
    let content: El<'a> = container(label).padding([3, 8]).into();
    button(content).padding(0).style(theme::btn(kind))
}

/// Segmented control: one pill of mutually exclusive options.
pub fn segmented<'a, T: Copy + PartialEq + 'a>(
    options: &[(T, &'static str)],
    selected: T,
    on_pick: impl Fn(T) -> Message,
) -> El<'a> {
    let mut r = Row::new();
    let n = options.len();
    for (i, (value, label)) in options.iter().enumerate() {
        let on = *value == selected;
        r = r.push(
            button(text(*label).size(12))
                .padding([3, 8])
                .style(theme::segment(on, i == 0, i + 1 == n))
                .on_press(on_pick(*value)),
        );
    }
    r.spacing(-1.0).into()
}

pub fn cell<'a>(content: impl IntoFragment<'a>, width: Length) -> El<'a> {
    t(content).width(width).into()
}

/// A numeric table cell (tabular digits).
pub fn num_cell<'a>(content: impl IntoFragment<'a>, width: Length) -> El<'a> {
    num(content).size(12).width(width).into()
}

pub fn table_header<'a>(cols: &[(&'static str, Length)]) -> El<'a> {
    let mut r = Row::new().spacing(6);
    for (label, width) in cols {
        let cell = label_caps(label).width(*width);
        r = r.push(match header_tip(label) {
            Some(why) => tip(cell, why),
            None => cell.into(),
        });
    }
    container(r).padding([3, 6]).width(Fill).style(theme::header_row).into()
}

/// One table row. With `on_press` it becomes clickable; the selected row gets a 2 px accent bar.
pub fn table_row<'a>(cells: Vec<El<'a>>, selected: bool, on_press: Option<Message>) -> El<'a> {
    let inner = container(Row::with_children(cells).spacing(6).align_y(iced::Center))
        .padding([2, 6])
        .width(Fill);
    let bar = container(space()).width(2).height(Fill).style(theme::row_bar(selected));
    container(
        row![
            bar,
            button(inner)
                .padding(0)
                .width(Fill)
                .style(theme::row_button)
                .on_press_maybe(on_press),
        ]
        .height(Length::Shrink),
    )
    .style(theme::row_fill(selected))
    .into()
}

pub const W_CALL: Length = Length::Fixed(110.0);
pub const W_NAME: Length = Length::Fixed(150.0);
pub const W_WHEN: Length = Length::Fixed(120.0);
pub const W_TIME: Length = Length::Fixed(90.0);
pub const W_SMALL: Length = Length::Fixed(80.0);
pub const W_ROLE: Length = Length::Fixed(150.0);

/// Large frequency readout: monospace digits, leading zeros dimmed, separators smaller.
pub fn freq_readout<'a>(hz: f64, size: f32) -> El<'a> {
    let total = (hz.max(0.0) + 0.5) as u64;
    let (khz, rest) = (total / 1000, total % 1000);
    let digits = format!("{khz:05}");
    let (head, tail) = digits.split_at(2);
    let mut r = Row::new().align_y(iced::Alignment::End);
    let mut leading = true;
    let push_digits = |r: Row<'a, Message>, part: &str, leading: &mut bool| {
        let mut r = r;
        for ch in part.chars() {
            if ch != '0' {
                *leading = false;
            }
            let c = if *leading { theme::FAINT } else { theme::TEXT };
            r = r.push(text(ch.to_string()).size(size).font(Font::MONOSPACE).color(c));
        }
        r
    };
    r = push_digits(r, head, &mut leading);
    r = r.push(text(" ").size(size * 0.5).font(Font::MONOSPACE));
    r = push_digits(r, tail, &mut leading);
    r = r.push(text(".").size(size * 0.56).font(Font::MONOSPACE).color(theme::MUTED));
    leading = false;
    r = push_digits(r, &format!("{rest:03}"), &mut leading);
    r = r.push(space().width(6)).push(text("kHz").size(size * 0.45).color(theme::MUTED));
    r.into()
}

/// Hover explanation in plain words.
pub fn tip_at<'a>(
    content: impl Into<El<'a>>,
    explanation: &'static str,
    position: tooltip::Position,
) -> El<'a> {
    tooltip(
        content,
        raised(container(t(explanation).size(11)).padding([4, 6]).max_width(300)).style(theme::popover),
        position,
    )
    .into()
}

pub fn tip<'a>(content: impl Into<El<'a>>, explanation: &'static str) -> El<'a> {
    tip_at(content, explanation, tooltip::Position::Bottom)
}

fn header_tip(label: &str) -> Option<&'static str> {
    Some(match label {
        s::COL_LAST_HEARD => s::TIP_LAST_HEARD,
        s::COL_TIMES => s::TIP_TIMES,
        s::COL_ROLE => s::TIP_ROLE,
        s::COL_SENT => s::TIP_SENT,
        s::COL_RECEIVED => s::TIP_RECEIVED,
        s::COL_PROGRESS => s::TIP_PROGRESS,
        s::COL_STATE => s::TIP_STATE,
        s::COL_CHANNEL => s::TIP_CHANNEL,
        s::COL_DIRECTION => s::TIP_DIRECTION,
        s::COL_HEARD => s::TIP_HEARD,
        _ => return None,
    })
}

/// One screen: a single heading (with optional actions), the sample banner, then the body.
pub fn page<'a>(
    title: &'static str,
    actions: Option<El<'a>>,
    banner: bool,
    body: impl Into<El<'a>>,
) -> El<'a> {
    let mut head = row![heading(title)].spacing(10).align_y(iced::Center);
    if banner {
        head = head.push(tip(chip("Sample data", theme::INFO), s::SAMPLE_BANNER));
    }
    head = head.push(space().width(Fill));
    if let Some(a) = actions {
        head = head.push(a);
    }
    column![
        container(head).padding([2, 8]).width(Fill).style(theme::header_row),
        container(body.into()).padding(6).width(Fill).height(Fill),
    ]
    .height(Fill)
    .into()
}

pub fn scroll<'a>(content: impl Into<El<'a>>) -> El<'a> {
    scrollable(content).width(Fill).height(Fill).into()
}

fn section_icon(sec: Section) -> Icon {
    match sec {
        Section::Operate => Icon::Operate,
        Section::Stations => Icon::Stations,
        Section::Messages => Icon::Messages,
        Section::Mail => Icon::Mail,
        Section::Files => Icon::Files,
        Section::Relay => Icon::Relay,
        Section::Logbook => Icon::Logbook,
        Section::Map => Icon::Map,
        Section::Settings => Icon::Settings,
    }
}

fn status_item<'a>(label: &'static str, why: &'static str) -> El<'a> {
    tip_at(
        row![icons::dot(theme::FAINT, None), small(label)].spacing(0).align_y(iced::Center),
        why,
        tooltip::Position::Top,
    )
}

// -------------------------------------------------------------------- view

impl App {
    pub fn view(&self) -> El<'_> {
        let body = match self.section {
            Section::Operate => self.screen_operate(),
            Section::Stations => self.screen_stations(),
            Section::Messages => self.screen_messages(),
            Section::Mail => self.screen_mail(),
            Section::Files => self.screen_files(),
            Section::Relay => self.screen_relay(),
            Section::Logbook => self.screen_logbook(),
            Section::Map => self.screen_map(),
            Section::Settings => self.screen_settings(),
        };
        let mut main = column![self.tech_band(), hline(), container(body).width(Fill).height(Fill)];
        if let Some(toast) = &self.toast {
            let color = match toast.kind {
                ToastKind::Info => theme::INFO,
                ToastKind::Warning => theme::WARNING,
            };
            main = main.push(container(notice(toast.text.clone(), color)).padding(4));
        }
        let base = container(
            column![
                self.top_bar(),
                hline(),
                row![self.rail(), vline(), main.width(Fill)].height(Fill),
                hline(),
                self.status_bar(),
            ],
        )
        .width(Fill)
        .height(Fill)
        .style(theme::window);

        if let Some(modal) = self.modal() {
            let scrim = mouse_area(container(space()).width(Fill).height(Fill).style(theme::scrim(self.modal_alpha)))
                .on_press(Message::CancelConfirm);
            stack![base, opaque(scrim), modal].into()
        } else {
            base.into()
        }
    }

    /// Control bar: source, centre frequency, record and stop, mode, state chips, Settings.
    fn top_bar(&self) -> El<'_> {
        let call = if self.my_call().is_empty() {
            tip(chip(s::CALLSIGN_UNSET, theme::WARNING), s::TIP_CALL_UNSET)
        } else {
            tip(chip(self.my_call().to_string(), theme::ACCENT), s::TIP_CALL_SET)
        };
        let connecting = self.is_busy(Busy::Connecting);
        let link = match &self.link {
            Some(c) if connecting => {
                tip(chip_pulse(s::link_with(c), theme::WARNING, self.pulse_for(true)), s::TIP_LINK_UP)
            }
            Some(c) => tip(chip(s::link_with(c), theme::ACCENT), s::TIP_LINK_UP),
            None => tip(chip(s::LINK_IDLE, theme::FAINT), s::TIP_LINK_IDLE),
        };
        let transmitting = self.is_busy(Busy::Transmitting);
        let tx = if transmitting {
            tip(chip_pulse(s::TX_ON_AIR, theme::DANGER, self.pulse_for(true)), s::TIP_TX_READY)
        } else if self.listen_only {
            tip(chip(s::TX_BLOCKED, theme::FAINT), s::TIP_TX_BLOCKED)
        } else {
            tip(chip(s::TX_IDLE, theme::WARNING), s::TIP_TX_READY)
        };
        let commands = tip(
            button(row![t(s::COMMANDS_BUTTON), small(s::COMMANDS_HINT)].spacing(6).align_y(iced::Center))
                .padding([3, 8])
                .style(theme::btn(Kind::Secondary))
                .on_press(Message::OpenPalette),
            s::TIP_COMMANDS,
        );
        let source = tip(
            row![
                label_caps(s::SOURCE_LABEL),
                pick_list(self.sdr_options(), Some(self.sdr_selected()), Message::SdrChosen)
                    .text_size(12)
                    .padding([2, 6])
                    .style(theme::dropdown)
                    .width(170),
            ]
            .spacing(6)
            .align_y(iced::Center),
            s::TIP_SOURCE,
        );
        let centre = tip(
            row![label_caps(s::CENTRE_LABEL), num(spectrum::khz_text(spectrum::CENTRE_HZ)).color(theme::TEXT)]
                .spacing(6)
                .align_y(iced::Center),
            s::TIP_CENTRE,
        );
        let rec = tip(
            btn(
                if self.recording { s::RECORDING } else { s::RECORD },
                if self.recording { Kind::Danger } else { Kind::Secondary },
            )
            .on_press(Message::ToggleRecord),
            s::TIP_RECORD,
        );
        let stop = tip(
            btn(s::STOP, Kind::Secondary).on_press_maybe(self.recording.then_some(Message::ToggleRecord)),
            s::TIP_STOP,
        );
        let mode = tip(
            row![
                label_caps(s::MODE_LABEL),
                pick_list([s::MODE_VALUE], Some(s::MODE_VALUE), |_| Message::Tick(self.clock))
                    .text_size(12)
                    .padding([2, 6])
                    .style(theme::dropdown),
            ]
            .spacing(6)
            .align_y(iced::Center),
            s::TIP_MODE,
        );
        container(
            row![
                text(s::APP_NAME).size(14).font(theme::SEMIBOLD),
                vline(),
                source,
                centre,
                row![rec, stop].spacing(2),
                mode,
                vline(),
                call,
                link,
                tx,
                space().width(Fill),
                tip(
                    toggler(self.listen_only)
                        .label(s::LISTEN_ONLY_HINT)
                        .size(14)
                        .text_size(12)
                        .spacing(6)
                        .style(theme::switch)
                        .on_toggle(Message::ToggleListen),
                    s::TIP_LISTEN,
                ),
                tip(row![muted(s::UTC), mono(utc_clock())].spacing(4).align_y(iced::Center), s::TIP_UTC),
                commands,
                tip(
                    btn(s::SECTION_SETTINGS, Kind::Secondary).on_press(Message::Go(Section::Settings)),
                    Section::Settings.tip(),
                ),
            ]
            .spacing(10)
            .align_y(iced::Center),
        )
        .padding([4, 8])
        .width(Fill)
        .style(theme::bar)
        .into()
    }

    /// Left column: the sections, then a channel row per marked channel.
    fn rail(&self) -> El<'_> {
        let mut nav = column![strip(s::NAV_HEADING)];
        for (i, sec) in Section::ALL.iter().copied().enumerate() {
            let active = self.section == sec;
            let unread = self.section_unread(sec);
            let tint = if active { theme::AMBER } else { theme::MUTED };
            let mut r = row![icons::icon(section_icon(sec), 14.0, tint), t(sec.label())]
                .spacing(8)
                .align_y(iced::Center);
            if unread > 0 {
                r = r.push(badge(unread));
            }
            r = r.push(space().width(Fill)).push(small(format!("^{}", i + 1)).size(10));
            let b = button(r)
                .width(Fill)
                .padding([4, 8])
                .style(theme::nav_button(active))
                .on_press(Message::Go(sec));
            let bar = container(space()).width(2).height(Fill).style(theme::row_bar(active));
            nav = nav.push(tip_at(
                row![bar, b].height(Length::Shrink),
                sec.tip(),
                tooltip::Position::Right,
            ));
        }
        let hz_now = self.tuned_hz();
        let mut chans = column![strip(s::CHANNELS_HEADING)];
        for (hz, name) in spectrum::MARKS {
            let frac = ((hz - (spectrum::CENTRE_HZ - spectrum::SPAN_HZ / 2.0)) / spectrum::SPAN_HZ) as f32;
            let at = ((frac * self.bins.len() as f32) as usize).min(self.bins.len().saturating_sub(1));
            let lo = at.saturating_sub(6);
            let db = self.bins[lo..(at + 7).min(self.bins.len())].iter().copied().fold(spectrum::DB_MIN, f32::max);
            let busy = db > spectrum::DB_MIN + 28.0;
            let tuned = (hz_now - hz).abs() < spectrum::BANDWIDTH_HZ;
            let (state, color) = if tuned && busy {
                (s::CH_RECEIVING, theme::ACCENT)
            } else if tuned {
                (s::CH_WAITING, theme::AMBER)
            } else if busy {
                (s::CH_ACTIVITY, theme::INFO)
            } else {
                (s::CH_IDLE, theme::FAINT)
            };
            chans = chans.push(
                container(
                    row![
                        icons::dot(color, None),
                        text(name).size(12).width(34),
                        num(spectrum::khz_text(hz)).size(12).width(Fill),
                        num(format!("{db:.0}")).size(12).color(color),
                    ]
                    .spacing(4)
                    .align_y(iced::Center),
                )
                .padding([3, 6])
                .style(theme::row_fill(tuned)),
            );
            chans = chans.push(container(small(state).size(10)).padding(iced::Padding { top: 0.0, right: 6.0, bottom: 3.0, left: 20.0 }));
            chans = chans.push(hline());
        }
        container(column![nav, hline(), chans])
            .width(200)
            .height(Fill)
            .style(theme::bar)
            .into()
    }

    /// State, source, channel rate, CPU, counts.
    fn status_bar(&self) -> El<'_> {
        let (state, color) = if self.is_busy(Busy::Transmitting) {
            (s::TX_ON_AIR, theme::DANGER)
        } else if self.sample {
            (s::STATUS_SIMULATED, theme::AMBER)
        } else {
            (s::STATUS_STOPPED, theme::FAINT)
        };
        let source = if self.sdr.is_empty() { s::STATUS_SOUND_CARD.to_string() } else { self.sdr.clone() };
        let cpu = 3 + (self.ticks / 7 % 5) as u32;
        let cell = |label: &'static str, v: String| -> El<'_> {
            row![small(label), num(v).size(12)].spacing(4).align_y(iced::Center).into()
        };
        container(
            row![
                row![icons::dot(color, None), text(state).size(11).font(Font::MONOSPACE).color(color)].align_y(iced::Center),
                cell(s::STATUS_SOURCE, source),
                cell(s::STATUS_RATE, s::CHANNEL_RATE.to_string()),
                cell(s::STATUS_CPU, format!("{cpu} %")),
                tip_at(small(s::STATUS_ENGINE), s::TIP_STATUS_ENGINE, tooltip::Position::Top),
                tip_at(small(s::STATUS_RIG), s::TIP_STATUS_RIG, tooltip::Position::Top),
                tip_at(small(s::STATUS_AUDIO), s::TIP_STATUS_AUDIO, tooltip::Position::Top),
                tip_at(small(s::STATUS_UPDATES), s::TIP_STATUS_UPDATES, tooltip::Position::Top),
                space().width(Fill),
                cell(s::STATUS_STATIONS, sample::STATIONS.len().to_string()),
                cell(s::STATUS_MESSAGES, self.thread.len().to_string()),
                tip_at(
                    row![icons::icon(Icon::Send, 11.0, theme::MUTED), small(s::outbox(self.outbox))]
                        .spacing(4)
                        .align_y(iced::Center),
                    s::TIP_OUTBOX,
                    tooltip::Position::Top
                ),
            ]
            .spacing(14)
            .align_y(iced::Center),
        )
        .padding([2, 8])
        .width(Fill)
        .style(theme::header_row)
        .into()
    }

    fn modal(&self) -> Option<El<'_>> {
        if let Some(p) = &self.palette {
            return Some(self.palette_modal(&p.query, p.selected));
        }
        match self.confirm? {
            Confirm::Transmit(action) => {
                let rows = column![
                    key_value(s::CONFIRM_TX_ACTION, action.label()),
                    key_value(s::CONFIRM_TX_FREQ, format!("{} kHz", spectrum::khz_text(self.tuned_hz()))),
                    key_value(s::CONFIRM_TX_CALL, self.my_call().to_string()),
                ]
                .spacing(6);
                Some(modal_box(
                    self.modal_alpha,
                    s::CONFIRM_TX_TITLE,
                    s::CONFIRM_TX_BODY,
                    Some(rows.into()),
                    btn(s::CONFIRM_CANCEL, Kind::Secondary).on_press(Message::CancelConfirm),
                    btn(s::CONFIRM_TX_BUTTON, Kind::Danger).on_press(Message::ConfirmTx),
                ))
            }
            Confirm::Disconnect => Some(modal_box(
                self.modal_alpha,
                s::CONFIRM_DISCONNECT_TITLE,
                s::CONFIRM_DISCONNECT_BODY,
                None,
                btn(s::CONFIRM_DISCONNECT_KEEP, Kind::Secondary).on_press(Message::CancelConfirm),
                btn(s::DISCONNECT, Kind::Danger).on_press(Message::ConfirmDisconnect),
            )),
        }
    }

    fn palette_modal(&self, query: &str, selected: usize) -> El<'_> {
        let hits = self.filtered_commands(query);
        let mut list = column![].spacing(2);
        if hits.is_empty() {
            list = list.push(container(muted(s::PALETTE_EMPTY)).padding(8));
        }
        for (i, cmd) in hits.into_iter().enumerate() {
            list = list.push(
                button(t(cmd.label))
                    .width(Fill)
                    .padding([7, 10])
                    .style(theme::nav_button(i == selected))
                    .on_press(Message::PaletteRunAt(i)),
            );
        }
        let panel = container(
            column![
                text_input(s::PALETTE_PLACEHOLDER, query)
                    .id(PALETTE_INPUT)
                    .on_input(Message::PaletteQuery)
                    .on_submit(Message::PaletteRun)
                    .padding(10)
                    .style(theme::input),
                scrollable(list).height(Length::Shrink),
                small(s::CONFIRM_ESC_HINT),
            ]
            .spacing(10),
        )
        .width(560)
        .max_height(440)
        .padding(14)
        .style(theme::modal(self.modal_alpha));
        column![space().height(90), container(opaque(panel)).center_x(Fill)].into()
    }
}

fn modal_box<'a>(
    alpha: f32,
    title: &'static str,
    body: &'static str,
    details: Option<El<'a>>,
    secondary: Button<'a, Message>,
    primary: Button<'a, Message>,
) -> El<'a> {
    let mut col = column![text(title).size(18).font(theme::SEMIBOLD), muted(body)].spacing(10);
    if let Some(d) = details {
        col = col.push(d);
    }
    col = col.push(row![space().width(Fill), secondary, primary].spacing(8));
    col = col.push(small(s::CONFIRM_ESC_HINT));
    center(opaque(container(col).width(440).padding(18).style(theme::modal(alpha)))).into()
}
