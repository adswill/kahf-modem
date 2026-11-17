//! Technical detail: the band at the top of every screen and the side panels that show
//! what was transmitted. Everything here is derived from the sample data until the
//! engine is connected, so the numbers are consistent but not measured.

use iced::widget::{canvas, column, container, image, row, space, stack, Column, Row};
use iced::{Color, ContentFit, Fill, Length};

use crate::app::{App, Busy, Message, Section};
use crate::icons::{LevelBar, Sparkline};
use crate::sample::{self, Chat, FileRow, FileState, Hop, Kind as MsgKind, Mail};
use crate::spectrum::{self, SpectrumPlot, WaterfallMarker};
use crate::strings as s;
use crate::theme::{self, Kind};
use crate::view::{btn, chip, freq_readout, hline, label_caps, muted, num, small, strip, tip, vline, El};

/// Payload bytes carried by one frame.
pub const FRAME_BYTES: u32 = 48;
/// Frames in one burst.
pub const BURST_FRAMES: u32 = 16;
/// Header bytes added to every message.
const HEADER_BYTES: u32 = 24;
/// Signature bytes added to a signed message.
const SIGNATURE_BYTES: u32 = 64;
/// One burst plus its gap on air, in seconds.
const BURST_SECONDS: u32 = 15;
const TX_DEMO_FRAMES: u32 = 14;
const RX_PERIOD_TICKS: u64 = 375;
const RX_ACTIVE_TICKS: u64 = 225;

// ----------------------------------------------------------------- numbers

/// CRC-32 (IEEE) of some bytes, used as the checksum shown for transfers.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for b in data {
        crc ^= *b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

pub fn frames_for(bytes: u32) -> u32 {
    bytes.div_ceil(FRAME_BYTES)
}

pub fn message_bytes(text_len: usize, signed: bool) -> u32 {
    text_len as u32 + HEADER_BYTES + if signed { SIGNATURE_BYTES } else { 0 }
}

type MonitorRow = (&'static str, String, Option<Color>, String, Option<Color>);

pub struct Burst {
    pub n: u32,
    pub at: String,
    pub channel: String,
    pub frames: u32,
    pub bytes: u32,
    pub result: String,
    pub color: Color,
}

pub struct Transfer {
    pub bytes: u32,
    pub frames: u32,
    pub bursts_total: u32,
    pub bursts_done: u32,
    pub checksum: u32,
    pub rate_bps: u32,
    pub retries: u32,
    pub window: Vec<Burst>,
}

fn is_retry(key: u32, n: u32) -> bool {
    n.wrapping_mul(7).wrapping_add(key).is_multiple_of(11)
}

/// Bursts of a transfer, from its size and progress (0..1). `key` seeds which bursts were retried.
pub fn transfer(bytes: u32, progress: f32, state: FileState, key: u32, channel: &str) -> Transfer {
    let frames = frames_for(bytes);
    let total = frames.div_ceil(BURST_FRAMES).max(1);
    let done = ((progress * total as f32) as u32).min(total);
    let mut retries = 0;
    for n in 1..=done {
        if is_retry(key, n) {
            retries += 1;
        }
    }
    let from = done.saturating_sub(5).min(total.saturating_sub(9)) + 1;
    let to = (from + 8).min(total);
    let mut window = Vec::new();
    for n in from..=to {
        let first = (n - 1) * BURST_FRAMES;
        let f = BURST_FRAMES.min(frames - first);
        let b = (f * FRAME_BYTES).min(bytes - first * FRAME_BYTES);
        let (result, color) = if n <= done {
            if is_retry(key, n) {
                ("Acked, retried".to_string(), theme::WARNING)
            } else {
                ("Acked".to_string(), theme::ACCENT)
            }
        } else if n == done + 1 {
            match state {
                FileState::Active => ("In progress".to_string(), theme::INFO),
                FileState::Paused => ("Paused".to_string(), theme::WARNING),
                FileState::Cancelled => ("Cancelled".to_string(), theme::DANGER),
                _ => ("Queued".to_string(), theme::MUTED),
            }
        } else {
            ("Queued".to_string(), theme::FAINT)
        };
        window.push(Burst {
            n,
            at: format!("+{} s", (n - 1) * BURST_SECONDS),
            channel: channel.to_string(),
            frames: f,
            bytes: b,
            result,
            color,
        });
    }
    let rate_bps = if state == FileState::Active || state == FileState::Done {
        BURST_FRAMES * FRAME_BYTES / BURST_SECONDS
    } else {
        0
    };
    Transfer { bytes, frames, bursts_total: total, bursts_done: done, checksum: crc32(&key.to_le_bytes()), rate_bps, retries, window }
}

pub fn mail_transfer(m: &Mail) -> Transfer {
    let bytes = message_bytes(m.body.len() + m.subject.len(), m.signed);
    let mut data = m.subject.as_bytes().to_vec();
    data.extend_from_slice(m.body.as_bytes());
    let mut tr = transfer(bytes, 1.0, FileState::Done, crc32(&data), "7 078.000 kHz");
    tr.checksum = crc32(&data);
    tr
}

pub fn file_transfer(f: &FileRow) -> Transfer {
    let bytes = f.bytes.min(u32::MAX as u64) as u32;
    let key = crc32(f.name.as_bytes());
    let mut tr = transfer(bytes, f.progress, f.state, key, "7 078.000 kHz");
    tr.checksum = crc32(&[f.name.as_bytes(), &bytes.to_le_bytes()].concat());
    if f.id.is_some() {
        tr.retries = f.retries;
    }
    tr
}

/// Transmissions of one chat message, one per hop that sent it.
pub fn message_bursts(chat: &Chat, hops: &[Hop]) -> Vec<Burst> {
    let signed = matches!(chat.kind, MsgKind::Signed);
    let bytes = message_bytes(chat.text.len(), signed);
    let frames = frames_for(bytes);
    let mut out = Vec::new();
    for (i, pair) in hops.windows(2).enumerate() {
        let (from, to) = (&pair[0], &pair[1]);
        let (result, color) = if to.heard {
            (format!("Heard by {}", to.who), theme::ACCENT)
        } else {
            ("No acknowledgement".to_string(), theme::WARNING)
        };
        out.push(Burst {
            n: i as u32 + 1,
            at: from.time.clone(),
            channel: from.channel.to_string(),
            frames,
            bytes,
            result,
            color,
        });
    }
    if let Some(last) = hops.last().filter(|h| !h.heard) {
        out.push(Burst {
            n: out.len() as u32 + 1,
            at: "queued".into(),
            channel: last.channel.to_string(),
            frames,
            bytes,
            result: "Retry 1 waiting".into(),
            color: theme::MUTED,
        });
    }
    out
}

// ------------------------------------------------------------ small widgets

fn kv_inline<'a>(label: &'static str, value: String) -> El<'a> {
    column![label_caps(label), num(value).size(12)].spacing(2).into()
}

fn dcell<'a>(content: impl Into<String>, w: Length, color: Option<Color>) -> El<'a> {
    let mut x = num(content.into()).size(12).width(w);
    if let Some(c) = color {
        x = x.color(c);
    }
    x.into()
}

fn dheader<'a>(cols: &[(&str, Length)]) -> El<'a> {
    let mut r = Row::new().spacing(6);
    for (label, w) in cols {
        r = r.push(label_caps(label).size(10).width(*w));
    }
    container(r).padding([2, 6]).width(Fill).style(theme::header_row).into()
}

fn drow<'a>(cells: Vec<El<'a>>) -> El<'a> {
    column![container(Row::with_children(cells).spacing(6)).padding([3, 8]).width(Fill), hline()].into()
}

/// A line of the form "Label    value" with monospace value.
fn kv<'a>(label: &'static str, value: impl Into<String>, color: Option<Color>) -> El<'a> {
    let mut v = num(value.into()).size(12).width(Fill);
    if let Some(c) = color {
        v = v.color(c);
    }
    container(row![small(label).size(11).width(110), v].spacing(6)).padding([3, 8]).into()
}

fn section<'a>(title: &'static str, body: impl Into<El<'a>>) -> El<'a> {
    column![
        container(label_caps(title)).padding([8, 8]),
        body.into(),
    ]
    .into()
}

/// The right-hand technical panel: a hairline on the left, scrolling content.
pub fn side_panel<'a>(title: &'static str, content: Column<'a, Message>) -> El<'a> {
    row![
        vline(),
        column![
            container(label_caps(title).color(theme::TEXT)).padding([8, 8]).width(Fill).style(theme::header_row),
            crate::view::scroll(content),
        ]
        .width(Fill),
    ]
    .width(440)
    .height(Fill)
    .into()
}

fn burst_table<'a>(bursts: &[Burst]) -> El<'a> {
    let cols = [
        ("#", Length::Fixed(26.0)),
        ("At", Length::Fixed(64.0)),
        ("Channel", Length::Fixed(66.0)),
        ("Frm", Length::Fixed(30.0)),
        ("Bytes", Length::Fixed(40.0)),
        ("Result", Fill),
    ];
    let mut col = column![dheader(&cols)];
    for b in bursts {
        col = col.push(drow(vec![
            dcell(b.n.to_string(), cols[0].1, Some(theme::MUTED)),
            dcell(b.at.clone(), cols[1].1, None),
            dcell(b.channel.replace(" kHz", "").replace(' ', ""), cols[2].1, None),
            dcell(b.frames.to_string(), cols[3].1, None),
            dcell(b.bytes.to_string(), cols[4].1, None),
            dcell(b.result.clone(), cols[5].1, Some(b.color)),
        ]));
    }
    col.into()
}

pub fn hop_table<'a>(hops: &[Hop]) -> El<'a> {
    let cols = [
        ("Hop", Length::Fixed(28.0)),
        ("Call", Length::Fixed(60.0)),
        ("Role", Length::Fixed(72.0)),
        ("Time", Length::Fixed(60.0)),
        ("Heard", Fill),
    ];
    let mut col = column![dheader(&cols)];
    for (i, h) in hops.iter().enumerate() {
        let (label, color) =
            if h.heard { (s::HOP_HEARD, theme::ACCENT) } else { (s::HOP_NOT_HEARD, theme::WARNING) };
        col = col.push(drow(vec![
            dcell((i + 1).to_string(), cols[0].1, Some(theme::MUTED)),
            dcell(h.who.clone(), cols[1].1, None),
            dcell(h.role, cols[2].1, Some(theme::MUTED)),
            dcell(if h.time.is_empty() { "--:--:--".to_string() } else { h.time.clone() }, cols[3].1, None),
            dcell(label, cols[4].1, Some(color)),
        ]));
    }
    col.into()
}

fn transfer_panel<'a>(tr: &Transfer, state_label: &str) -> Vec<El<'a>> {
    let pct = tr.bursts_done as f32 / tr.bursts_total as f32 * 100.0;
    let remaining = (tr.bursts_total - tr.bursts_done) * BURST_FRAMES * FRAME_BYTES;
    let eta = if tr.rate_bps > 0 && remaining > 0 {
        let secs = remaining / tr.rate_bps;
        format!("{}:{:02}", secs / 60, secs % 60)
    } else {
        "--".to_string()
    };
    let rate = if tr.rate_bps > 0 { format!("{} B/s", tr.rate_bps) } else { "0 B/s".to_string() };
    vec![
        section(
            "Transfer",
            column![
                kv("State", state_label, None),
                kv("Size", format!("{} bytes, {} frames", tr.bytes, tr.frames), None),
                kv("Bursts", format!("{} of {} done  ({pct:.0}%)", tr.bursts_done, tr.bursts_total), None),
                kv("Rate", rate, None),
                kv("Time left", eta, None),
                kv("Retries", tr.retries.to_string(), if tr.retries > 0 { Some(theme::WARNING) } else { None }),
                kv("CRC-32", format!("{:08X}", tr.checksum), None),
            ],
        ),
        section(
            "Bursts",
            column![
                burst_table(&tr.window),
                container(small(format!("Showing {} of {}", tr.window.len(), tr.bursts_total)).size(11)).padding([4, 8]),
            ],
        ),
    ]
}

// ------------------------------------------------------------------ the band

impl App {
    /// Receive burst of the simulated input: (frames so far, active).
    fn rx_burst(&self) -> (u32, bool) {
        let phase = self.ticks % RX_PERIOD_TICKS;
        if phase < RX_ACTIVE_TICKS {
            (1 + (phase / 12) as u32, true)
        } else {
            (0, false)
        }
    }

    /// Transmit burst: (frames sent, total frames, active).
    fn tx_burst(&self) -> (u32, u32, bool) {
        match self.busy {
            Some((Busy::Transmitting, until)) => {
                let left = until.saturating_duration_since(self.clock).as_millis() as u64;
                let elapsed = 2600u64.saturating_sub(left.min(2600));
                ((1 + (elapsed / 180) as u32).min(TX_DEMO_FRAMES), TX_DEMO_FRAMES, true)
            }
            _ => (0, TX_DEMO_FRAMES, false),
        }
    }

    fn nearest_channel(&self) -> String {
        let hz = self.tuned_hz();
        spectrum::channels()
            .into_iter()
            .min_by(|a, b| (a.0 - hz).abs().total_cmp(&(b.0 - hz).abs()))
            .map(|(_, n)| format!("Ch {n}"))
            .unwrap_or_default()
    }

    fn monitor_table(&self) -> El<'_> {
        let (rx_frames, rx_on) = self.rx_burst();
        let (tx_sent, tx_total, tx_on) = self.tx_burst();
        let w_l = Length::Fixed(64.0);
        let w_c = Length::Fixed(110.0);
        let (rx_state, rx_color) = if rx_on { ("RECEIVING", theme::ACCENT) } else { ("IDLE", theme::FAINT) };
        let (tx_state, tx_color) = if tx_on {
            ("ON AIR", theme::DANGER)
        } else if self.listen_only {
            ("BLOCKED", theme::FAINT)
        } else {
            ("READY", theme::WARNING)
        };
        let dbfs = self.sim.peak_db;
        let rows: Vec<MonitorRow> = vec![
            ("State", rx_state.into(), Some(rx_color), tx_state.into(), Some(tx_color)),
            (
                "Frames",
                if rx_on { rx_frames.to_string() } else { "0".into() },
                None,
                if tx_on { format!("{tx_sent} of {tx_total}") } else { "0".into() },
                None,
            ),
            (
                "Bytes",
                (rx_frames * FRAME_BYTES).to_string(),
                None,
                (if tx_on { tx_sent * FRAME_BYTES } else { 0 }).to_string(),
                None,
            ),
            ("Channel", self.nearest_channel(), None, self.nearest_channel(), None),
            ("SNR", format!("{:.0} dB", self.snr_now()), None, "n/a".into(), Some(theme::FAINT)),
            ("Level", format!("{dbfs:.0} dBFS"), None, format!("{:.0} %", self.level), None),
        ];
        let mut col = column![container(
            Row::new()
                .push(label_caps("").width(w_l))
                .push(label_caps("Receive").width(w_c))
                .push(label_caps("Transmit").width(w_c))
                .spacing(6),
        )
        .padding([4, 8])
        .width(Fill)
        .style(theme::header_row)];
        for (label, rx, rxc, tx, txc) in rows {
            col = col.push(drow(vec![
                small(label).size(11).width(w_l).into(),
                dcell(rx, w_c, rxc),
                dcell(tx, w_c, txc),
            ]));
        }
        col.into()
    }

    /// The band shown on every screen: frequency and level, spectrum and waterfall, monitor.
    /// Collapses to the single header row.
    pub fn tech_band(&self) -> El<'_> {
        let full = self.section == Section::Operate;
        let offset = self.tuned_hz() - spectrum::centre_hz();
        let header = row![
            freq_readout(self.tuned_hz(), 30.0),
            column![
                label_caps(s::INPUT_LEVEL_SHORT),
                canvas(LevelBar { level: self.level_now, peak: self.level_peak }).width(110).height(12),
                num(format!("{:.0} dBFS", self.sim.peak_db)).size(11).color(theme::MUTED),
            ]
            .spacing(2),
            column![
                label_caps(s::BAND_LABEL),
                iced::widget::pick_list(spectrum::Band::ALL, Some(self.band), Message::BandChosen).style(theme::dropdown),
            ]
            .spacing(2),
            kv_inline(s::SEGMENT_LABEL, spectrum::segment_text(self.tuned_hz())),
            kv_inline(s::MODE_LABEL, s::MODE_VALUE.to_string()),
            kv_inline(s::WIDTH_LABEL, format!("{:.1} kHz", spectrum::BANDWIDTH_HZ / 1000.0)),
            kv_inline(s::OFFSET_LABEL, format!("{offset:+.0} Hz")),
            kv_inline("Span", format!("{:.0} kHz", spectrum::SPAN_HZ / 1000.0)),
            space().width(Fill),
            tip(chip(s::SIMULATED_INPUT, theme::AMBER), s::SIMULATED_HINT),
            tip(
                btn(if self.band_open { "Hide spectrum" } else { "Show spectrum" }, Kind::Secondary)
                    .on_press(Message::ToggleBand),
                "Collapse or expand the spectrum and waterfall. The choice is remembered.",
            ),
        ]
        .spacing(18)
        .align_y(iced::Center);

        let mut out = column![container(header).padding([3, 8])];
        if self.band_open {
            let (sh, wh) = if full { (170.0, 120.0) } else { (78.0, 58.0) };
            let plot = canvas(SpectrumPlot {
                data: &self.bins,
                peak_bin: self.sim.peak_bin,
                tune: self.tune,
                grid: &self.grid,
            })
            .width(Fill)
            .height(sh);
            let waterfall = stack![
                image(self.wf_handle.clone()).width(Fill).height(wh).content_fit(ContentFit::Fill),
                canvas(WaterfallMarker { tune: self.tune, scale: full }).width(Fill).height(wh),
            ];
            let mut monitor = column![strip(s::MONITOR_HEADING), self.monitor_table()].spacing(0);
            if full {
                monitor = monitor.push(strip(s::SNR_HISTORY_HEADING));
                monitor = monitor.push(
                    container(
                        column![
                            row![space().width(Fill), num(format!("{:.0} dB", self.snr_now())).size(12)],
                            canvas(Sparkline { data: &self.snr_hist }).width(Fill).height(34),
                        ]
                        .spacing(2),
                    )
                    .padding(6),
                );
            }
            out = out.push(hline()).push(
                row![
                    column![plot, hline(), waterfall].width(Fill),
                    vline(),
                    container(monitor).width(300),
                ]
                .height(Length::Shrink),
            );
        }
        container(out).width(Fill).style(theme::bar).into()
    }

    // ----------------------------------------------------------- side panels

    pub fn messages_tech(&self) -> El<'_> {
        let chats = self.conv_chats();
        let index = self.msg_sel.unwrap_or(chats.len().saturating_sub(1));
        let (Some(chat), Some(hops)) = (chats.get(index), self.selected_route()) else {
            return side_panel("Technical detail", column![container(muted(s::HOP_NONE_HINT)).padding(10)]);
        };
        let bursts = message_bursts(chat, &hops);
        let delivered = hops.last().is_some_and(|h| h.heard);
        let (sig, sig_color) = match chat.kind {
            MsgKind::Signed => ("Signed, signature valid", theme::ACCENT),
            MsgKind::Private => ("Not signed, sealed to recipient", theme::MUTED),
            _ => ("Not signed", theme::MUTED),
        };
        let delivery = if delivered {
            let when = hops.last().map(|h| h.time.clone()).unwrap_or_default();
            (format!("{} {when}", if chat.mine { "Delivered" } else { "Received" }), theme::ACCENT)
        } else {
            ("Not delivered, retry waiting".to_string(), theme::WARNING)
        };
        let bytes = message_bytes(chat.text.len(), matches!(chat.kind, MsgKind::Signed));
        side_panel(
            "Technical detail",
            column![
                section(
                    "Message",
                    column![
                        kv("Sent at", chat.time.clone(), None),
                        kv("Kind", chat.kind.label(), None),
                        kv("Size", format!("{bytes} bytes, {} frames", frames_for(bytes)), None),
                        kv("Signature", sig, Some(sig_color)),
                        kv("Delivery", delivery.0, Some(delivery.1)),
                    ],
                ),
                section("Transmissions", burst_table(&bursts)),
                section("Hop path", hop_table(&hops)),
            ],
        )
    }

    pub fn mail_tech(&self) -> El<'_> {
        if self.mail_composing {
            let body = self.mail_body.text();
            let bytes = message_bytes(body.len() + self.mail_subject.len(), self.mail_signed);
            return side_panel(
                "Technical detail",
                column![section(
                    "Draft",
                    column![
                        kv("Size", format!("{bytes} bytes"), None),
                        kv("Frames", frames_for(bytes).to_string(), None),
                        kv("Bursts", frames_for(bytes).div_ceil(BURST_FRAMES).max(1).to_string(), None),
                        kv("Signature", if self.mail_signed { "Will be signed" } else { "Not signed" }, None),
                        kv("Air time", format!("about {} s", frames_for(bytes).div_ceil(BURST_FRAMES).max(1) * BURST_SECONDS), None),
                    ],
                )],
            );
        }
        let Some(m) = self.mail_sel.and_then(|i| sample::MAILS.get(i)).filter(|_| self.sample) else {
            return side_panel("Technical detail", column![container(muted("Choose a message to see its bursts.")).padding(10)]);
        };
        let tr = mail_transfer(m);
        let mut items = transfer_panel(&tr, "Received in full");
        items.push(section(
            "Signature",
            kv("State", if m.signed { "Signed, signature valid" } else { "Not signed" }, Some(if m.signed { theme::ACCENT } else { theme::MUTED })),
        ));
        side_panel("Technical detail", Column::with_children(items))
    }

    pub fn map_tech(&self) -> El<'_> {
        let chats = self.conv_chats();
        let index = self.msg_sel.unwrap_or(chats.len().saturating_sub(1));
        let (Some(chat), Some(hops)) = (chats.get(index), self.selected_route()) else {
            return side_panel("Hop path", column![container(muted(s::HOP_NONE_HINT)).padding(10)]);
        };
        side_panel(
            "Hop path",
            column![
                section(
                    "Selected message",
                    column![
                        kv("From", chat.from.clone(), None),
                        kv("Time", chat.time.clone(), None),
                        kv("Text", chat.text.clone(), None),
                    ],
                ),
                section("Hops", hop_table(&hops)),
            ],
        )
    }

    pub fn files_tech(&self) -> El<'_> {
        let index = self
            .file_sel
            .filter(|i| *i < self.files.len())
            .or_else(|| self.files.iter().position(|f| f.state == FileState::Active))
            .unwrap_or(0);
        let Some(f) = self.files.get(index) else {
            return side_panel("Technical detail", column![container(muted("No file selected.")).padding(10)]);
        };
        let tr = file_transfer(f);
        let state = match f.state {
            FileState::Active => if f.sending { "Sending" } else { "Receiving" },
            FileState::Paused => "Paused",
            FileState::Done => "Complete",
            FileState::Cancelled => "Cancelled",
            FileState::Waiting => "Waiting",
        };
        let mut items = vec![section(
            "File",
            column![kv("Name", f.name.clone(), None), kv("To", if f.to.is_empty() { "-".to_string() } else { f.to.clone() }, None)],
        )];
        items.extend(transfer_panel(&tr, state));
        side_panel("Technical detail", Column::with_children(items))
    }
}

/// Dense table pieces reused by the screens.
pub mod grid {
    use super::*;

    pub fn cell<'a>(content: impl Into<String>, w: Length, color: Option<Color>) -> El<'a> {
        dcell(content, w, color)
    }
}

#[cfg(test)]
mod shots {
    use crate::app::{App, Section};

    /// Renders the four main screens offscreen (no window) into ~/kahf-work/shots/.
    /// Run with: ICED_TEST_BACKEND=tiny-skia cargo test -p kahf-ui shots -- --ignored
    #[test]
    #[ignore]
    fn render_screens() {
        let dir = std::path::PathBuf::from(std::env::var("HOME").unwrap()).join("kahf-work/shots");
        std::fs::create_dir_all(&dir).unwrap();
        for (sec, name) in [
            (Section::Operate, "operate"),
            (Section::Messages, "messages"),
            (Section::Mail, "mail"),
            (Section::Files, "files"),
            (Section::Map, "map"),
            (Section::Settings, "settings"),
            (Section::Operate, "setup"),
            (Section::Operate, "relay-start"),
            (Section::Operate, "bandplan"),
            (Section::Operate, "bandplan-in"),
            (Section::Operate, "20m"),
        ] {
            let mut app = App::init();
            app.section = sec;
            let _ = app.update(crate::app::Message::ShowSample(true));
            app.setup_open = name == "setup";
            if name == "20m" {
                let _ = app.update(crate::app::Message::BandChosen(crate::spectrum::Band::M20));
            }
            if name == "bandplan" {
                app.tune = 0.93;
            }
            if name == "relay-start" {
                let _ = app.update(crate::app::Message::RoleChosen(crate::app::Role::Relay));
                app.modal_alpha = 1.0;
            }
            app.attach_open = name == "operate";
            let settings = iced::Settings { default_font: crate::theme::UI_FONT, ..Default::default() };
            let mut sim = iced_test::Simulator::with_size(settings, iced::Size::new(1512.0, 940.0), app.view());
            let snap = sim.snapshot(&app.theme()).unwrap();
            let path = dir.join(format!("{name}.png"));
            let _ = std::fs::remove_file(dir.join(format!("{name}-tiny-skia.png")));
            let _ = std::fs::remove_file(&path);
            snap.matches_image(&path).unwrap();
        }
    }
}
