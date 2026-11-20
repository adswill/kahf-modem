//! One function per section of the rail.

use iced::keyboard::{key::Named, Key};
use iced::widget::text_editor::{Binding, KeyPress};
use iced::widget::{
    button, center, checkbox, column, container, pick_list, progress_bar, radio, row, slider, space,
    text_editor as editor, text_input, toggler, Column,
};
use iced::{Fill, Length};

use crate::app::{App, FileCmd, Location, Message, Role, TxAction, KINDS, PROFILES};
use crate::icons::{self, Icon};
use crate::live;
use crate::sample::{self, Chat, FileState};
use crate::strings as s;
use crate::tech::grid;
use crate::theme::{self, Kind};
use crate::view::*;

/// Ctrl+Enter is handled by the window (it sends); everything else is default.
fn composer_keys(press: KeyPress) -> Option<Binding<Message>> {
    let ctrl = press.modifiers.command() || press.modifiers.control();
    if ctrl && press.key == Key::Named(Named::Enter) {
        return None;
    }
    Binding::from_key_press(press)
}

fn kind_tip(kind: sample::Kind) -> &'static str {
    match kind {
        sample::Kind::Chat | sample::Kind::File => "",
        sample::Kind::Urgent => s::TIP_KIND_URGENT,
        sample::Kind::Signed => s::TIP_KIND_SIGNED,
        sample::Kind::Private => s::TIP_KIND_PRIVATE,
    }
}

fn chat_cells<'a>(c: &Chat) -> Vec<El<'a>> {
    let who = if c.mine { theme::ACCENT } else { theme::INFO };
    let kind_color = if c.kind == sample::Kind::Urgent {
        theme::DANGER
    } else {
        theme::MUTED
    };
    vec![
        grid::cell(c.time.clone(), Length::Fixed(52.0), Some(theme::MUTED)),
        grid::cell(c.from.clone(), Length::Fixed(64.0), Some(who)),
        t(c.text.clone()).size(13).width(Fill).into(),
        tip(
            grid::cell(
                if c.kind == sample::Kind::Chat {
                    String::new()
                } else {
                    c.kind.label().to_string()
                },
                Length::Fixed(64.0),
                Some(kind_color),
            ),
            kind_tip(c.kind),
        ),
    ]
}

/// Messages as table rows. With `selectable` they can be picked to show their route.
fn chat_list<'a>(chats: &[Chat], selectable: Option<usize>) -> El<'a> {
    let cols = [
        ("Time", Length::Fixed(52.0)),
        ("From", Length::Fixed(64.0)),
        ("Message", Fill),
        ("Kind", Length::Fixed(64.0)),
    ];
    let mut col = Column::new();
    for (i, c) in chats.iter().enumerate() {
        let msg = selectable.map(|_| Message::SelectMsg(i));
        let row = table_row(chat_cells(c), selectable == Some(i), msg);
        col = col.push(if selectable.is_some() {
            tip(row, s::TIP_PICK_MESSAGE)
        } else {
            row
        });
    }
    column![table_header(&cols), scroll(col)].width(Fill).into()
}

fn setting<'a>(title: &'static str, desc: &'static str, control: impl Into<El<'a>>) -> El<'a> {
    column![
        t(title).font(theme::SEMIBOLD),
        muted(desc).size(12),
        control.into()
    ]
    .spacing(4)
    .into()
}

fn file_state_label(state: FileState) -> &'static str {
    match state {
        FileState::Active => s::STATE_ACTIVE,
        FileState::Paused => s::STATE_PAUSED,
        FileState::Done => s::STATE_DONE,
        FileState::Cancelled => s::STATE_CANCELLED,
        FileState::Waiting => s::STATE_WAITING,
    }
}

impl App {
    // ---------------------------------------------------------------- Operate

    pub fn screen_operate(&self) -> El<'_> {
        let right = column![
            container(self.conversation_panel()).height(Fill),
            hline(),
            self.heard_panel(),
        ]
        .width(Fill)
        .height(Fill);
        if self.panel_shown {
            row![container(self.right_panel()).width(330), vline(), right].into()
        } else {
            right.into()
        }
    }

    /// Stations heard: call, last heard, SNR.
    fn heard_panel(&self) -> El<'_> {
        // Live contacts carry no SNR or last message; their name and note take those places.
        let cols = if self.sample {
            [
                (s::COL_CALLSIGN, W_CALL),
                (s::COL_LAST_HEARD, W_WHEN),
                (s::SNR_LABEL, W_SMALL),
                (s::COL_LAST_MESSAGE, Fill),
            ]
        } else {
            [
                (s::COL_CALLSIGN, W_CALL),
                (s::COL_LAST_HEARD, W_WHEN),
                (s::COL_NAME, W_NAME),
                (s::COL_NOTE, Fill),
            ]
        };
        let mut rows = Column::new();
        if self.sample {
            for (i, st) in sample::STATIONS.iter().enumerate() {
                let snr = 8 + st.call.bytes().map(|b| b as u32).sum::<u32>() % 24;
                let color = if snr >= 20 {
                    theme::ACCENT
                } else if snr >= 12 {
                    theme::AMBER
                } else {
                    theme::MUTED
                };
                rows = rows.push(table_row(
                    vec![
                        mono(st.call).size(12).color(theme::INFO).width(cols[0].1).into(),
                        grid::cell(st.last_heard, cols[1].1, Some(theme::MUTED)),
                        grid::cell(format!("{snr} dB"), cols[2].1, Some(color)),
                        grid::cell(st.last_msg, cols[3].1, None),
                    ],
                    self.station_sel == Some(i),
                    Some(Message::SelectStation(i)),
                ));
            }
        } else if self.live.stations.is_empty() {
            rows = rows.push(container(muted(s::STATIONS_EMPTY_HINT)).padding(6));
        } else {
            for (i, c) in self.live.stations.iter().enumerate() {
                let heard = c.last_heard.as_deref().map(live::date).unwrap_or_default();
                rows = rows.push(table_row(
                    vec![
                        mono(c.call.clone())
                            .size(12)
                            .color(theme::INFO)
                            .width(cols[0].1)
                            .into(),
                        grid::cell(heard, cols[1].1, Some(theme::MUTED)),
                        grid::cell(c.name.clone(), cols[2].1, None),
                        grid::cell(c.notes.clone(), cols[3].1, None),
                    ],
                    self.station_sel == Some(i),
                    Some(Message::SelectStation(i)),
                ));
            }
        }
        container(
            column![
                strip(s::STATIONS_HEARD_HEADING),
                table_header(&cols),
                scroll(rows)
            ]
            .width(Fill),
        )
        .height(150)
        .into()
    }

    fn conversation_panel(&self) -> El<'_> {
        let thread: El<'_> = if self.thread.is_empty() {
            empty_state(Icon::Messages, s::THREAD_EMPTY_TITLE, s::THREAD_EMPTY_HINT)
        } else {
            chat_list(&self.thread, None)
        };
        let send = tip(
            btn(format!("{}  {}", s::SEND, s::SEND_HINT), Kind::Primary)
                .on_press_maybe((!self.listen_only).then_some(Message::RequestTx(TxAction::Send))),
            if self.listen_only {
                s::TIP_SEND_BLOCKED
            } else {
                s::TIP_SEND
            },
        );
        let attach = tip(
            btn(s::ATTACH, Kind::Secondary).on_press(Message::AttachOpen(true)),
            s::TIP_ATTACH,
        );
        let kinds: Vec<(sample::Kind, &'static str)> = KINDS.iter().map(|k| (*k, k.label())).collect();
        let mut composer = column![row![
            tip(
                row![
                    label_caps(s::KIND_LABEL),
                    segmented(&kinds, self.kind, Message::KindPicked)
                ]
                .spacing(8)
                .align_y(iced::Center),
                s::TIP_KIND
            ),
            attach,
            space().width(Fill),
            num(s::char_count(self.composer_len()))
                .size(11)
                .color(theme::MUTED),
        ]
        .spacing(8)
        .align_y(iced::Center),]
        .spacing(4);
        if self.attach_open {
            composer = composer.push(self.attach_panel());
        }
        composer = composer.push(
            row![
                editor(&self.composer)
                    .placeholder(s::COMPOSER_PLACEHOLDER)
                    .on_action(Message::Composer)
                    .key_binding(composer_keys)
                    .height(52)
                    .style(theme::editor),
                container(send).align_y(iced::Bottom).height(52),
            ]
            .spacing(6),
        );
        let head = container(
            row![
                heading(s::CONVERSATION_HEADING),
                space().width(Fill),
                tip(
                    btn(
                        if self.panel_shown {
                            s::PANEL_HIDE
                        } else {
                            s::PANEL_SHOW
                        },
                        Kind::Secondary
                    )
                    .on_press(Message::TogglePanel),
                    s::TIP_PANEL
                ),
            ]
            .align_y(iced::Center),
        )
        .padding([2, 8])
        .width(Fill)
        .style(theme::header_row);
        column![head, thread, container(composer).padding(6)]
            .width(Fill)
            .height(Fill)
            .into()
    }

    /// Link readout: state line, mode line, SNR bar, key-value grid, then connect and transmit.
    fn right_panel(&self) -> El<'_> {
        let linked = self.link.is_some();
        let status = self.status_line();
        let (state_text, state_color) = match (&self.link, &status) {
            (Some(c), _) => (s::link_with(c), theme::ACCENT),
            (None, Some((_, true))) => (s::LINK_CONNECTING.to_string(), theme::AMBER),
            (None, _) => (s::NO_LINK_TEXT.to_string(), theme::MUTED),
        };
        let snr = self.snr_now();
        let mut link = column![
            iced::widget::text(state_text)
                .size(12)
                .font(iced::Font::MONOSPACE)
                .color(state_color),
            iced::widget::text(format!("{} {}", s::MODE_VALUE, self.profile))
                .size(22)
                .font(iced::Font::MONOSPACE),
            row![
                label_caps(s::SNR_LABEL),
                iced::widget::canvas(crate::icons::LevelBar {
                    level: snr / 60.0,
                    peak: snr / 60.0
                })
                .width(Fill)
                .height(10),
                num(format!("{snr:.0} dB")).size(12),
            ]
            .spacing(6)
            .align_y(iced::Center),
            key_value(s::STATE_LABEL, if linked { s::LINK_UP } else { s::NO_LINK_TEXT }),
            key_value(s::CONNECTION_LABEL, self.rig.to_string()),
            key_value(
                s::DEVICE_LABEL,
                if self.audio_in.is_empty() {
                    s::DEVICE_SYSTEM.to_string()
                } else {
                    self.audio_in.clone()
                }
            ),
            tip(key_value(s::SPEED_LABEL, s::NOT_REPORTED), s::TIP_SPEED),
        ]
        .spacing(5);
        if let Some((line, waiting)) = status {
            link = link.push(notice(line, if waiting { theme::AMBER } else { theme::WARNING }));
        }
        if !self.engine_note.is_empty() {
            link = link.push(key_value(s::ENGINE_LAST, self.engine_note.clone()));
        }
        link = link.push(tip(
            field(
                s::TARGET_LABEL,
                text_input(s::TARGET_PLACEHOLDER, &self.target)
                    .on_input(Message::Target)
                    .padding(4)
                    .style(theme::input),
            ),
            s::TIP_TARGET,
        ));
        link = link.push(
            row![
                if linked {
                    tip(
                        btn(s::DISCONNECT, Kind::Secondary).on_press(Message::RequestDisconnect),
                        s::TIP_DISCONNECT,
                    )
                } else {
                    tip(
                        btn(s::CONNECT_ELLIPSIS, Kind::Primary).on_press_maybe(
                            (!self.listen_only).then_some(Message::RequestTx(TxAction::Connect)),
                        ),
                        if self.listen_only {
                            s::TIP_CONNECT_BLOCKED
                        } else {
                            s::TIP_CONNECT
                        },
                    )
                },
                tip(
                    btn(s::FILES_SEND_BUTTON, Kind::Secondary).on_press(Message::AttachOpen(true)),
                    s::TIP_ATTACH
                ),
            ]
            .spacing(6),
        );

        let transmit: El<'_> = if self.listen_only {
            column![
                notice(s::LISTEN_ONLY_WHY, theme::WARNING),
                tip(
                    btn(s::LISTEN_ONLY_TURN_OFF, Kind::Secondary).on_press(Message::ToggleListen(false)),
                    s::TIP_TURN_OFF_LISTEN
                ),
            ]
            .spacing(8)
            .into()
        } else {
            column![
                tip(
                    field(
                        s::PROFILE_LABEL,
                        pick_list(PROFILES, Some(self.profile), Message::Profile)
                            .style(theme::dropdown)
                            .width(Fill),
                    ),
                    s::TIP_PROFILE
                ),
                tip(
                    field(
                        s::LEVEL_LABEL,
                        row![
                            slider(0.0..=100.0, self.level, Message::Level)
                                .step(1.0f32)
                                .width(Fill),
                            small(s::level_pct(self.level)).width(40),
                        ]
                        .spacing(8)
                        .align_y(iced::Center),
                    ),
                    s::TIP_LEVEL
                ),
                tip(
                    btn(s::SEND_BEACON, Kind::Secondary).on_press(Message::RequestTx(TxAction::Beacon)),
                    s::TIP_BEACON
                ),
            ]
            .spacing(10)
            .into()
        };

        scroll(
            column![
                section_box(s::LINK_HEADING, link),
                section_box(s::TRANSMIT_HEADING, transmit),
                section_box(
                    s::RIG_HEADING,
                    column![
                        key_value(s::SET_RIG_CONTROL, self.rig.to_string()),
                        key_value(
                            s::RIG_ADDR_LABEL,
                            if self.rig_addr.is_empty() {
                                "-".to_string()
                            } else {
                                self.rig_addr.clone()
                            }
                        ),
                    ]
                    .spacing(5),
                ),
                section_box(s::AUDIO_HEADING, self.volume_controls()),
                section_box(
                    s::HOPS_HEADING,
                    t(self.last_sent.clone().unwrap_or_else(|| s::NO_HOPS.to_string())),
                ),
            ]
            .spacing(10),
        )
    }

    // --------------------------------------------------------------- Stations

    pub fn screen_stations(&self) -> El<'_> {
        if !self.sample {
            return self.live_stations();
        }
        self.sample_stations()
    }

    /// Stations from the address book the engine keeps.
    fn live_stations(&self) -> El<'_> {
        if self.live.stations.is_empty() {
            return page(
                s::STATIONS_TITLE,
                None,
                false,
                empty_state(Icon::Antenna, s::STATIONS_EMPTY_TITLE, s::STATIONS_EMPTY_HINT),
            );
        }
        let cols = [
            (s::COL_CALLSIGN, W_CALL),
            (s::COL_NAME, W_NAME),
            (s::COL_LAST_HEARD, W_WHEN),
            (s::COL_QTH, W_NAME),
            (s::COL_NOTE, Fill),
        ];
        let q = self.station_filter.trim().to_lowercase();
        let mut rows = Column::new();
        for (i, c) in self.live.stations_matching(&q) {
            let heard = c.last_heard.as_deref().map(live::date).unwrap_or_default();
            rows = rows.push(table_row(
                vec![
                    mono(c.call.clone()).width(cols[0].1).into(),
                    cell(c.name.clone(), cols[1].1),
                    num_cell(heard, cols[2].1),
                    cell(c.qth.clone(), cols[3].1),
                    cell(c.notes.clone(), cols[4].1),
                ],
                self.station_sel == Some(i),
                Some(Message::SelectStation(i)),
            ));
        }
        let mut body = column![
            text_input(s::STATIONS_FILTER, &self.station_filter)
                .on_input(Message::StationFilter)
                .padding(8)
                .style(theme::input)
                .width(360),
            table_header(&cols),
        ]
        .spacing(8);
        if self.live.stations_matching(&q).next().is_none() {
            body = body.push(muted(s::STATIONS_NO_MATCH));
        }
        page(s::STATIONS_TITLE, None, false, body.push(scroll(rows)))
    }

    fn sample_stations(&self) -> El<'_> {
        let cols = [
            (s::COL_CALLSIGN, W_CALL),
            (s::COL_NAME, W_NAME),
            (s::COL_LAST_HEARD, W_WHEN),
            (s::COL_TIMES, W_TIME),
            (s::COL_LAST_MESSAGE, Fill),
            (s::COL_ROLE, W_ROLE),
        ];
        let q = self.station_filter.trim().to_lowercase();
        let mut rows = Column::new();
        let mut shown = 0;
        for (i, st) in sample::STATIONS.iter().enumerate() {
            if !q.is_empty() && !st.call.to_lowercase().contains(&q) && !st.name.to_lowercase().contains(&q) {
                continue;
            }
            shown += 1;
            rows = rows.push(table_row(
                vec![
                    mono(st.call).width(cols[0].1).into(),
                    cell(st.name, cols[1].1),
                    cell(st.last_heard, cols[2].1),
                    num_cell(st.times.to_string(), cols[3].1),
                    cell(st.last_msg, cols[4].1),
                    cell(st.role, cols[5].1),
                ],
                self.station_sel == Some(i),
                Some(Message::SelectStation(i)),
            ));
        }
        let mut body = column![
            text_input(s::STATIONS_FILTER, &self.station_filter)
                .on_input(Message::StationFilter)
                .padding(8)
                .style(theme::input)
                .width(360),
            table_header(&cols),
        ]
        .spacing(8);
        if shown == 0 {
            body = body.push(muted(s::STATIONS_NO_MATCH));
        }
        page(s::STATIONS_TITLE, None, true, body.push(scroll(rows)))
    }

    // --------------------------------------------------------------- Messages

    pub fn screen_messages(&self) -> El<'_> {
        if !self.sample {
            return page(
                s::SECTION_MESSAGES,
                None,
                false,
                empty_state(Icon::Wave, s::MESSAGES_EMPTY_TITLE, s::MESSAGES_EMPTY_HINT),
            );
        }
        let cols = [
            ("Call", Length::Fixed(64.0)),
            ("Last message", Fill),
            ("When", Length::Fixed(72.0)),
            ("New", Length::Fixed(28.0)),
        ];
        let mut list = Column::new();
        for (i, c) in sample::CONVERSATIONS.iter().enumerate() {
            let unread = self.conv_unread.get(i).copied().unwrap_or(0);
            list = list.push(table_row(
                vec![
                    grid::cell(c.call, cols[0].1, Some(theme::INFO)),
                    t(c.preview).size(13).width(cols[1].1).into(),
                    grid::cell(c.when, cols[2].1, Some(theme::MUTED)),
                    grid::cell(
                        if unread > 0 {
                            unread.to_string()
                        } else {
                            String::new()
                        },
                        cols[3].1,
                        Some(theme::ACCENT),
                    ),
                ],
                self.conv_sel == i,
                Some(Message::SelectConversation(i)),
            ));
        }
        let chats = self.conv_chats();
        let selected = self.msg_sel.unwrap_or(chats.len().saturating_sub(1));
        let left = column![table_header(&cols), scroll(list)].width(400);
        let body = row![
            left,
            vline(),
            chat_list(&chats, Some(selected)),
            self.messages_tech()
        ]
        .spacing(0)
        .height(Fill);
        page(s::SECTION_MESSAGES, None, true, body)
    }

    // ------------------------------------------------------------------- Mail

    pub fn screen_mail(&self) -> El<'_> {
        let cols = [
            ("From", Length::Fixed(64.0)),
            ("Subject", Fill),
            ("Sig", Length::Fixed(30.0)),
            ("When", Length::Fixed(90.0)),
        ];
        let mut list = Column::new();
        for (i, m) in sample::MAILS.iter().enumerate().filter(|_| self.sample) {
            let unread = self.mail_unread.get(i).copied().unwrap_or(false);
            let subject = if unread {
                t(m.subject).size(13).color(theme::ACCENT)
            } else {
                t(m.subject).size(13)
            };
            list = list.push(table_row(
                vec![
                    grid::cell(m.from, cols[0].1, Some(theme::INFO)),
                    subject.width(cols[1].1).into(),
                    grid::cell(if m.signed { "yes" } else { "no" }, cols[2].1, Some(theme::MUTED)),
                    grid::cell(m.when, cols[3].1, Some(theme::MUTED)),
                ],
                self.mail_sel == Some(i),
                Some(Message::SelectMail(i)),
            ));
        }
        let left: El<'_> = if self.sample {
            column![table_header(&cols), scroll(list)].into()
        } else {
            empty_state(Icon::Mail, s::MAIL_EMPTY_TITLE, s::MAIL_EMPTY_HINT)
        };

        let right: El<'_> = if self.mail_composing {
            container(self.mail_form()).padding([0, 12]).into()
        } else if let Some(m) = self.mail_sel.and_then(|i| sample::MAILS.get(i)) {
            container(
                column![
                    t(m.subject).font(theme::SEMIBOLD),
                    num(format!("{}   {}", m.from, m.when)).color(theme::MUTED),
                    hline(),
                    scroll(t(m.body)),
                ]
                .spacing(8),
            )
            .padding([0, 12])
            .into()
        } else {
            center(muted(s::MAIL_PICK_HINT)).into()
        };
        let body = row![
            container(left).width(430).height(Fill),
            vline(),
            container(right).width(Fill).height(Fill),
            self.mail_tech(),
        ]
        .height(Fill);
        page(
            s::MAIL_TITLE,
            Some(tip(
                btn(s::MAIL_COMPOSE, Kind::Primary).on_press(Message::MailCompose),
                s::TIP_COMPOSE,
            )),
            self.sample,
            body,
        )
    }

    fn mail_form(&self) -> El<'_> {
        column![
            tip(
                btn(s::MAIL_BACK, Kind::Ghost).on_press(Message::MailBack),
                s::TIP_MAIL_BACK
            ),
            field(
                s::MAIL_TO,
                text_input(s::TARGET_PLACEHOLDER, &self.mail_to)
                    .on_input(Message::MailTo)
                    .padding(6)
                    .style(theme::input),
            ),
            field(
                s::MAIL_SUBJECT,
                text_input("", &self.mail_subject)
                    .on_input(Message::MailSubject)
                    .padding(6)
                    .style(theme::input),
            ),
            editor(&self.mail_body)
                .placeholder(s::MAIL_BODY_PLACEHOLDER)
                .on_action(Message::MailBody)
                .height(Fill)
                .style(theme::editor),
            tip(
                checkbox(self.mail_signed)
                    .label(s::MAIL_SIGNED)
                    .on_toggle(Message::MailSigned),
                s::TIP_KIND_SIGNED
            ),
            row![
                tip(
                    btn_icon(Icon::Paperclip, s::MAIL_ATTACH, Kind::Secondary),
                    s::TIP_ATTACH
                ),
                muted(s::MAIL_ATTACH_NONE).size(12),
                space().width(Fill),
                tip(
                    btn_icon(Icon::Send, s::MAIL_QUEUE, Kind::Primary).on_press(Message::MailQueue),
                    s::TIP_QUEUE
                ),
            ]
            .spacing(10)
            .align_y(iced::Center),
        ]
        .spacing(10)
        .into()
    }

    // ------------------------------------------------------------------ Files

    pub fn screen_files(&self) -> El<'_> {
        if self.files.is_empty() {
            return page(
                s::FILES_TITLE,
                Some(tip(
                    btn(s::FILES_SEND_BUTTON, Kind::Secondary).on_press(Message::AttachOpen(true)),
                    s::TIP_ATTACH,
                )),
                false,
                empty_state(Icon::Files, s::FILES_EMPTY_TITLE, s::FILES_EMPTY_HINT),
            );
        }
        let cols = [
            (s::COL_FILE, Fill),
            (s::COL_DIRECTION, Length::Fixed(80.0)),
            (s::COL_SIZE, Length::Fixed(70.0)),
            (s::COL_PROGRESS, Length::Fixed(140.0)),
            (s::COL_STATE, Length::Fixed(80.0)),
            (s::COL_RETRIES, Length::Fixed(60.0)),
            ("", Length::Fixed(150.0)),
        ];
        let shown = self
            .file_sel
            .filter(|i| *i < self.files.len())
            .or_else(|| self.files.iter().position(|f| f.state == FileState::Active))
            .unwrap_or(0);
        let mut rows = Column::new();
        for (i, f) in self.files.iter().enumerate() {
            let mut actions = row![].spacing(4);
            if matches!(f.state, FileState::Waiting | FileState::Paused) {
                let blocked = f.sending && self.listen_only;
                actions = actions.push(tip(
                    btn(s::FILE_START, Kind::Secondary)
                        .on_press_maybe((!blocked).then_some(Message::File(i, FileCmd::Start))),
                    if blocked {
                        s::TIP_START_BLOCKED
                    } else {
                        s::TIP_START
                    },
                ));
            }
            if f.state == FileState::Active {
                actions = actions.push(tip(
                    btn(s::FILE_PAUSE, Kind::Secondary).on_press(Message::File(i, FileCmd::Pause)),
                    s::TIP_PAUSE,
                ));
            }
            if !matches!(f.state, FileState::Done | FileState::Cancelled) {
                actions = actions.push(tip(
                    btn(s::FILE_CANCEL, Kind::Ghost).on_press(Message::File(i, FileCmd::Cancel)),
                    s::TIP_FILE_CANCEL,
                ));
            }
            let name = button(t(f.name.clone()).size(13))
                .padding(0)
                .width(cols[0].1)
                .style(theme::row_button)
                .on_press(Message::SelectFile(i));
            rows = rows.push(table_row(
                vec![
                    name.into(),
                    grid::cell(
                        if f.sending {
                            s::FILE_SENDING
                        } else {
                            s::FILE_RECEIVING
                        },
                        cols[1].1,
                        Some(theme::MUTED),
                    ),
                    grid::cell(format!("{} kB", f.bytes / 1024), cols[2].1, None),
                    row![
                        progress_bar(0.0..=1.0, f.progress)
                            .girth(5)
                            .length(80)
                            .style(theme::progress),
                        num(s::percent(f.progress)).size(12).color(theme::MUTED)
                    ]
                    .spacing(8)
                    .align_y(iced::Center)
                    .width(cols[3].1)
                    .into(),
                    grid::cell(file_state_label(f.state), cols[4].1, None),
                    grid::cell(
                        f.retries.to_string(),
                        cols[5].1,
                        if f.retries > 0 {
                            Some(theme::AMBER)
                        } else {
                            Some(theme::MUTED)
                        },
                    ),
                    actions.width(cols[6].1).into(),
                ],
                shown == i,
                None,
            ));
        }
        page(
            s::FILES_TITLE,
            Some(tip(
                btn(s::FILES_SEND_BUTTON, Kind::Secondary).on_press(Message::AttachOpen(true)),
                s::TIP_ATTACH,
            )),
            self.sample,
            row![
                column![table_header(&cols), scroll(rows)].width(Fill),
                self.files_tech()
            ]
            .height(Fill),
        )
    }

    // ------------------------------------------------------------------ Relay

    pub fn screen_relay(&self) -> El<'_> {
        if !self.role.is_relay() {
            return page(
                s::RELAY_TITLE,
                None,
                false,
                center(
                    column![
                        icons::icon(Icon::Relay, 56.0, theme::with_alpha(theme::MUTED, 0.7)),
                        t(s::RELAY_OFF_TITLE).size(16).font(theme::SEMIBOLD),
                        muted(s::RELAY_OFF_HINT),
                        tip(
                            btn(s::RELAY_OPEN_SETTINGS, Kind::Secondary)
                                .on_press(Message::Go(crate::app::Section::Settings)),
                            s::TIP_OPEN_SETTINGS
                        ),
                    ]
                    .spacing(8)
                    .align_x(iced::Center),
                ),
            );
        }
        if !self.sample {
            return self.live_relay();
        }
        let sched_cols = [
            (s::COL_CHANNEL, Length::Fixed(160.0)),
            (s::COL_TIME, W_TIME),
            (s::COL_DURATION, W_TIME),
            (s::COL_DIR, Fill),
        ];
        let mut sched = column![table_header(&sched_cols)];
        for sl in sample::SLOTS {
            sched = sched.push(table_row(
                vec![
                    mono(sl.channel).width(sched_cols[0].1).into(),
                    num_cell(sl.time, sched_cols[1].1),
                    num_cell(sl.duration, sched_cols[2].1),
                    cell(sl.direction, sched_cols[3].1),
                ],
                false,
                None,
            ));
        }
        sched = sched.push(
            row![
                tip(
                    btn(s::RELAY_ACCEPT, Kind::Primary).on_press(Message::RelayAccept),
                    s::TIP_ACCEPT
                ),
                tip(
                    btn(s::RELAY_DECLINE, Kind::Secondary).on_press(Message::RelayDecline),
                    s::TIP_DECLINE
                ),
            ]
            .spacing(8),
        );
        let hop_cols = [
            (s::COL_TIME, W_TIME),
            (s::COL_FROM_TO, Fill),
            (s::COL_STATE, W_ROLE),
        ];
        let mut hops = column![table_header(&hop_cols)];
        for (time, route, state) in sample::HOPS {
            hops = hops.push(table_row(
                vec![
                    num_cell(*time, hop_cols[0].1),
                    mono(*route).width(hop_cols[1].1).into(),
                    cell(*state, hop_cols[2].1),
                ],
                false,
                None,
            ));
        }
        let stats = row![
            column![
                label_caps(s::RELAY_SENT_TODAY),
                heading(sample::RELAY_TODAY.to_string()).font(iced::Font::MONOSPACE)
            ]
            .spacing(4),
            column![
                label_caps(s::RELAY_SENT_TOTAL),
                heading(sample::RELAY_TOTAL.to_string()).font(iced::Font::MONOSPACE)
            ]
            .spacing(4),
        ]
        .spacing(40);
        page(
            s::RELAY_TITLE,
            None,
            true,
            scroll(
                column![
                    section_box(s::RELAY_SCHEDULE, sched.spacing(8)),
                    section_box(s::RELAY_HOPS, hops),
                    section_box(s::RELAY_STATS, stats),
                ]
                .spacing(12),
            ),
        )
    }

    /// Relay schedule, forwards, outbox and counters from the engine.
    fn live_relay(&self) -> El<'_> {
        let lv = &self.live;
        let sched_cols = [
            (s::COL_CHANNEL, Length::Fixed(160.0)),
            (s::COL_TIME, W_TIME),
            (s::COL_DURATION, W_TIME),
            (s::COL_DIR, Fill),
        ];
        let sched: El<'_> = if lv.slots.is_empty() {
            muted(s::RELAY_NO_SLOTS).into()
        } else {
            let mut c = column![table_header(&sched_cols)];
            for sl in &lv.slots {
                c = c.push(table_row(
                    vec![
                        mono(live::channel(sl.channel_khz)).width(sched_cols[0].1).into(),
                        num_cell(live::utc_hm(sl.start_unix), sched_cols[1].1),
                        num_cell(live::duration(sl.duration_s), sched_cols[2].1),
                        cell(live::direction(sl.direction), sched_cols[3].1),
                    ],
                    false,
                    None,
                ));
            }
            c.into()
        };
        let hop_cols = [
            (s::COL_TIME, W_TIME),
            (s::COL_RELAY, W_CALL),
            (s::COL_CHANNEL, Length::Fixed(160.0)),
            (s::COL_STATE, Fill),
        ];
        let hops: El<'_> = if lv.hops.is_empty() {
            muted(s::RELAY_NO_HOPS).into()
        } else {
            let mut c = column![table_header(&hop_cols)];
            for h in &lv.hops {
                c = c.push(table_row(
                    vec![
                        num_cell(live::utc_hm(h.at_unix), hop_cols[0].1),
                        mono(h.relay.clone()).width(hop_cols[1].1).into(),
                        mono(live::channel(h.channel_khz)).width(hop_cols[2].1).into(),
                        grid::cell(
                            live::hop_state(h),
                            hop_cols[3].1,
                            Some(if h.heard_by_next {
                                theme::ACCENT
                            } else {
                                theme::WARNING
                            }),
                        ),
                    ],
                    false,
                    None,
                ));
            }
            c.into()
        };
        let outbox: El<'_> = if lv.outbox.is_empty() {
            muted(s::RELAY_OUTBOX_EMPTY).into()
        } else {
            let mut c = column![table_header(&[(s::COL_ITEM, Fill)])];
            for (_, what) in &lv.outbox {
                c = c.push(table_row(vec![cell(what.clone(), Fill)], false, None));
            }
            c.into()
        };
        let stat = |label: &'static str, n: u64| {
            column![
                label_caps(label),
                heading(n.to_string()).font(iced::Font::MONOSPACE)
            ]
            .spacing(4)
        };
        let stats = row![
            stat(s::RELAY_SENT_TODAY, lv.stats.sent_today),
            stat(s::RELAY_SENT_TOTAL, lv.stats.sent_total),
            stat(s::RELAY_OUTBOX, lv.outbox.len() as u64),
        ]
        .spacing(40);
        let server: El<'_> = match (&lv.server, self.server_note) {
            (Some(st), _) => {
                let mut c = column![
                    key_value(
                        s::SERVER_STATUS,
                        if st.registered {
                            s::SERVER_REGISTERED
                        } else {
                            s::SERVER_NOT_REGISTERED
                        }
                    ),
                    key_value(
                        s::SERVER_SCHEDULE,
                        match st.last_schedule_unix {
                            Some(at) => format!("{} slots, received at {}", st.slots, live::utc_hm(at)),
                            None => s::SERVER_NO_SCHEDULE.to_string(),
                        }
                    ),
                    key_value(s::SERVER_NEXT, live::utc_hm(st.next_attempt_unix)),
                ]
                .spacing(5);
                if let Some(e) = &st.last_error {
                    c = c.push(key_value(s::SERVER_ERROR, e.clone()));
                }
                // Label and value columns stay close together on a wide screen.
                container(c).max_width(520).into()
            }
            (None, Some(note)) => muted(note).into(),
            (None, None) => muted(s::SERVER_NOT_REGISTERED).into(),
        };
        page(
            s::RELAY_TITLE,
            None,
            false,
            scroll(
                column![
                    section_box(s::RELAY_STATS, stats),
                    section_box(s::RELAY_SERVER, server),
                    section_box(s::RELAY_SCHEDULE, sched),
                    section_box(s::RELAY_OUTBOX, outbox),
                    section_box(s::RELAY_HOPS, hops),
                ]
                .spacing(12),
            ),
        )
    }

    // ---------------------------------------------------------------- Logbook

    pub fn screen_logbook(&self) -> El<'_> {
        let export = Some(tip(
            btn(s::LOG_EXPORT, Kind::Secondary).on_press(Message::LogExport),
            s::TIP_EXPORT,
        ));
        if !self.sample && self.live.qsos.is_empty() {
            return page(
                s::LOG_TITLE,
                export,
                false,
                empty_state(Icon::Logbook, s::LOG_EMPTY_TITLE, s::LOG_EMPTY_HINT),
            );
        }
        let cols = [
            (s::COL_DATE, W_WHEN),
            (s::COL_TIME, W_TIME),
            (s::COL_CALLSIGN, W_CALL),
            (s::COL_BAND, W_SMALL),
            (s::COL_SENT, W_SMALL),
            (s::COL_RECEIVED, W_TIME),
            (s::COL_NOTE, Fill),
        ];
        let q = self.log_search.trim().to_lowercase();
        let mut rows = Column::new();
        let mut shown = 0;
        // (date, time, call, band, sent, received, note), from the sample or the logbook file.
        let entries: Vec<[String; 7]> = if self.sample {
            sample::QSOS
                .iter()
                .map(|q| [q.date, q.time, q.call, q.band, q.sent, q.received, q.note].map(String::from))
                .collect()
        } else {
            self.live
                .qsos
                .iter()
                .map(|q| {
                    [
                        live::date(&q.date),
                        live::time(&q.time),
                        q.call.clone(),
                        q.band.clone(),
                        q.rst_sent.clone(),
                        q.rst_rcvd.clone(),
                        q.comment.clone(),
                    ]
                })
                .collect()
        };
        for [date, time, call, band, sent, received, note] in entries {
            if !q.is_empty() && !call.to_lowercase().contains(&q) && !note.to_lowercase().contains(&q) {
                continue;
            }
            shown += 1;
            rows = rows.push(table_row(
                vec![
                    num_cell(date, cols[0].1),
                    num_cell(time, cols[1].1),
                    mono(call).width(cols[2].1).into(),
                    cell(band, cols[3].1),
                    num_cell(sent, cols[4].1),
                    num_cell(received, cols[5].1),
                    cell(note, cols[6].1),
                ],
                false,
                None,
            ));
        }
        let mut body = column![
            text_input(s::LOG_SEARCH, &self.log_search)
                .on_input(Message::LogSearch)
                .padding(8)
                .style(theme::input)
                .width(360),
            table_header(&cols),
        ]
        .spacing(8);
        if shown == 0 {
            body = body.push(muted(s::LOG_NO_MATCH));
        }
        page(s::LOG_TITLE, export, self.sample, body.push(scroll(rows)))
    }

    // -------------------------------------------------------------------- Map

    pub fn screen_map(&self) -> El<'_> {
        let left = column![
            t(s::MAP_HEADLINE).font(theme::SEMIBOLD).size(16),
            t(s::MAP_BODY),
            muted(s::MAP_NO_MAP_HERE),
            tip(
                btn(s::MAP_BUTTON, Kind::Primary).on_press(Message::OpenMap),
                s::TIP_MAP_OPEN
            ),
            num(s::MAP_URL).color(theme::MUTED),
            muted(s::MAP_PRIVACY).size(12),
            section_box(
                s::MAP_SUMMARY,
                column![
                    key_value(s::MAP_LOC_LABEL, self.location.label()),
                    key_value(s::MAP_HEARD_LABEL, self.stations_heard().to_string()),
                    key_value(
                        s::MAP_LINK_LABEL,
                        self.link.clone().unwrap_or_else(|| s::NO_LINK_TEXT.to_string())
                    ),
                ]
                .spacing(5),
            ),
        ]
        .spacing(12)
        .max_width(520);
        page(
            s::MAP_TITLE,
            None,
            false,
            row![
                container(left).width(Fill).height(Fill).padding([8, 4]),
                self.map_tech()
            ]
            .height(Fill),
        )
    }

    // --------------------------------------------------------------- Settings

    pub fn screen_settings(&self) -> El<'_> {
        let station = column![
            muted(s::SET_STATION_DESC),
            setting(
                s::SET_CALLSIGN,
                s::SET_CALLSIGN_DESC,
                text_input("", &self.callsign)
                    .on_input(Message::Callsign)
                    .padding(8)
                    .style(theme::input)
                    .width(220),
            ),
            setting(
                s::SET_LOCATOR,
                if self.locator.is_empty() || live::locator_position(&self.locator).is_some() {
                    s::SET_LOCATOR_DESC
                } else {
                    s::SET_LOCATOR_BAD
                },
                text_input("KP20", &self.locator)
                    .on_input(Message::Locator)
                    .padding(8)
                    .style(theme::input)
                    .width(220),
            ),
            setting(
                s::SET_FINGERPRINT,
                s::SET_FINGERPRINT_DESC,
                mono(s::SET_FINGERPRINT_NONE)
            ),
            setting(
                s::SET_ROLE,
                s::SET_ROLE_DESC,
                segmented(
                    &[
                        (Role::Station, s::ROLE_STATION),
                        (Role::Relay, s::ROLE_RELAY),
                        (Role::Both, s::ROLE_BOTH)
                    ],
                    self.role,
                    Message::RoleChosen
                ),
            ),
        ]
        .spacing(12);

        let audio = column![muted(s::SET_AUDIO_DESC), self.step_controls(3)].spacing(10);

        let rig = column![
            muted(s::SET_RIG_DESC),
            field(s::SET_RIG_CONTROL, self.step_controls(1))
        ]
        .spacing(10);

        let sdr = column![
            muted(s::SET_SDR_DESC),
            field(s::SET_SDR_DEVICE, self.step_controls(2))
        ]
        .spacing(10);

        let data = column![
            muted(s::SET_DATA_DESC),
            key_value(s::SET_DATA_PATH, s::DATA_NOT_CHOSEN)
        ]
        .spacing(10);

        let mut location = column![muted(s::SET_LOCATION_DESC)].spacing(8);
        for l in Location::ALL {
            location = location.push(
                column![
                    radio(l.label(), l, Some(self.location), Message::LocationChosen),
                    muted(l.description()).size(12),
                ]
                .spacing(2),
            );
        }
        if self.location == Location::Area {
            location = location.push(field(
                s::LOC_RADIUS,
                row![
                    slider(5.0..=100.0, self.radius_km, Message::Radius)
                        .step(5.0f32)
                        .width(220),
                    t(s::km(self.radius_km)),
                ]
                .spacing(10)
                .align_y(iced::Center),
            ));
        }

        let relay = column![
            muted(s::SET_RELAY_DESC),
            toggler(self.internet_relay)
                .label(s::SET_RELAY_TOGGLE)
                .style(theme::switch)
                .on_toggle(Message::InternetRelay),
        ]
        .spacing(10);

        let listen = column![
            muted(s::SET_LISTEN_DESC),
            toggler(self.listen_only)
                .label(s::SET_LISTEN)
                .style(theme::switch)
                .on_toggle(Message::ToggleListen),
        ]
        .spacing(10);

        let appearance = column![
            muted(s::SET_APPEARANCE_DESC),
            toggler(self.sample)
                .label(s::SET_SAMPLE)
                .style(theme::switch)
                .on_toggle(Message::ShowSample),
            muted(s::SET_SAMPLE_DESC).size(12),
            toggler(self.panel_shown)
                .label(s::SET_PANEL)
                .style(theme::switch)
                .on_toggle(|_| Message::TogglePanel),
        ]
        .spacing(10);

        let updates = column![
            muted(s::SET_UPDATES_DESC),
            key_value(s::SET_VERSION, env!("CARGO_PKG_VERSION")),
            key_value(s::STATUS_UPDATES, self.update_status.as_str()),
            tip(
                btn(s::SET_CHECK, Kind::Secondary).on_press(Message::CheckUpdates),
                s::TIP_CHECK
            ),
        ]
        .spacing(10);

        page(
            s::SETTINGS_TITLE,
            None,
            false,
            scroll(
                column![
                    section_box(s::SETUP_SETTINGS_HEADING, self.setup_section()),
                    section_box(s::SET_STATION, station),
                    section_box(s::SET_AUDIO, audio),
                    section_box(s::SET_RIG, rig),
                    section_box(s::SET_SDR, sdr),
                    section_box(s::SET_DATA, data),
                    section_box(s::SET_LOCATION, location),
                    section_box(s::SET_RELAY, relay),
                    section_box(s::SET_LISTEN, listen),
                    section_box(s::SET_APPEARANCE, appearance),
                    section_box(s::SET_UPDATES, updates),
                ]
                .spacing(12)
                .max_width(720),
            ),
        )
    }
}
