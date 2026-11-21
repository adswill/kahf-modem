//! The window: the front panel on top, the sections on the left, the page, the status bar.

pub mod chats;
pub mod hardware;
pub mod logbook;
pub mod mail;
pub mod map;
pub mod kit;
pub mod radio;
pub mod relay;
pub mod setup;
pub mod stations;
pub mod widgets;
pub mod welcome;

use alloy_ui::prelude::*;
use alloy_ui::{stack, Anchor, Layer};

use crate::app::Kahf;
use crate::model::{Page, Tone};
use crate::util::{bands, fmt};
use kit::*;

pub fn view(k: &Kahf) -> impl View<A> {
    let page = k.m.page;
    let body: V = if page.full_screen() {
        match page {
            Page::Welcome => welcome::view(k),
            _ => setup::view(k),
        }
    } else {
        shell(k)
    };
    body
}

fn shell(k: &Kahf) -> V {
    let mut main = col![].fill();
    if let Some(group) = k.m.page.group() {
        main = main.push(subbar(k, group));
    }
    main = main.push(page(k));
    let mut c = col![strip(k)].gap(0.0);
    if !k.m.engine_error.is_empty() {
        c = c.push(
            row![
                text(k.m.engine_error.clone()).caption().fill_w(),
                button("Try again").act(|k: &mut Kahf| k.start_engine(|_, _| {})),
                button("Open Radio").act(|k: &mut Kahf| k.go(Page::Radio)),
            ]
            .gap(10.0)
            .align_center()
            .pad(8.0)
            .fill_w(),
        );
    }
    c = c.push(row![nav(k), main].gap(0.0).fill());
    c = c.push(status_bar(k));
    c.fill().into()
}

fn page(k: &Kahf) -> V {
    match k.m.page {
        Page::Radio => radio::view(k),
        Page::Chats => chats::view(k),
        Page::Mail => mail::view(k),
        Page::Stations => stations::view(k),
        Page::Relay => relay::view(k),
        Page::Logbook => logbook::view(k),
        _ => col![].into(),
    }
}

/// The front panel: always on screen, so the person always knows what the radio is doing.
fn strip(k: &Kahf) -> V {
    let khz = k.m.settings.freq_khz;
    let channels = bands::channels();
    let current = bands::index_of(khz);
    let status = k.m.status();
    let tone = |t: Tone| match t {
        Tone::Good => 1,
        Tone::Warn => 2,
        _ => 0,
    };
    let _ = tone;
    let mut r = row![
        heading("KAHF").accent(),
        text(fmt::mhz(khz)).heading().no_wrap(),
        tabs(channels.iter().map(|c| c.name), current.unwrap_or(0)).act(move |k: &mut Kahf, i: usize| {
            k.set_frequency(bands::channels()[i].khz);
        }),
        widgets::signal_meter(k.m.signal),
        lit("RX", k.m.receiving, false),
        lit("TX", k.m.transmitting, true),
        col![text(status.title.clone()).label(), caption(status.detail.clone()).dim().no_wrap()].gap(0.0),
        space(),
    ]
    .gap(14.0)
    .align_center()
    .pad_xy(16.0, 10.0)
    .fill_w();
    if k.m.relay.running {
        r = r.push(button(format!("Relay {}", k.m.relay.forwarded_today)).act(|k: &mut Kahf| k.go(Page::Relay)));
    }
    r = r.push(button("Help").ghost().act(|k: &mut Kahf| k.m.help_open = true));
    r = r.push(button("Settings").ghost().act(|k: &mut Kahf| k.m.open_settings(crate::model::SettingsTab::Station)));
    r.into()
}

fn nav(k: &Kahf) -> V {
    let item = |label: &str, pages: &'static [Page], badge: usize| -> V {
        let here = pages.contains(&k.m.page);
        let target = if here { k.m.page } else { pages[0] };
        let label = if badge > 0 { format!("{label}  ({badge})") } else { label.to_string() };
        button(label).nav(here).fill_w().act(move |k: &mut Kahf| k.go(target)).into()
    };
    col![
        item("Messages", &[Page::Chats, Page::Mail], k.m.unread_chats() + k.m.unread_mail()),
        item("Stations", &[Page::Stations, Page::Relay, Page::Logbook], 0),
        item("Radio", &[Page::Radio], 0),
    ]
    .gap(4.0)
    .pad(8.0)
    .w(160.0)
    .fill_h()
    .into()
}

/// The small switch under a main tab: two views of one place.
fn subbar(k: &Kahf, group: &'static [Page]) -> V {
    let mut r = row![].gap(6.0).pad_xy(12.0, 8.0);
    for p in group {
        let badge = match p {
            Page::Chats => k.m.unread_chats(),
            Page::Mail => k.m.unread_mail(),
            _ => 0,
        };
        let label = if badge > 0 { format!("{}  ({badge})", p.label()) } else { p.label().to_string() };
        let p = *p;
        r = r.push(button(label).nav(k.m.page == p).act(move |k: &mut Kahf| k.go(p)));
    }
    r.fill_w().into()
}

fn status_bar(k: &Kahf) -> V {
    row![
        caption(k.m.status().title).dim().fill_w(),
        caption(format!("Station {}", if k.m.settings.callsign.is_empty() { "listening only" } else { &k.m.settings.callsign })).dim(),
        caption(fmt::mhz(k.m.settings.freq_khz)).dim(),
    ]
    .gap(18.0)
    .pad_xy(14.0, 4.0)
    .fill_w()
    .into()
}

/// A panel over the page, with the page dimmed behind it. Clicking outside closes it.
pub fn modal(page: V, content: V, close: impl Fn(&mut Kahf) + Send + Sync + 'static) -> V {
    let scrim = button("").ghost().fill_w().act(close);
    let _ = scrim;
    stack([Layer::from(page), Layer::from(content.card().pad(18.0)).anchor(Anchor::Center)]).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_ui::testing::Harness;

    fn app(page: Page) -> Harness<impl alloy_ui::App> {
        let dir = std::env::temp_dir().join(format!("kahf-ui-{}-{page:?}", std::process::id()));
        let mut k = Kahf::for_tests(dir);
        k.m.settings.callsign = "KA1ABC".into();
        k.m.settings.setup_done = true;
        k.m.page = page;
        Harness::new(sketch(k, |k: &Kahf| view(k)), 1360.0, 800.0)
    }

    #[test]
    fn welcome_offers_setup_and_looking_around() {
        let mut h = app(Page::Welcome);
        h.expect("Welcome to KAHF").expect("Set up KAHF").expect("Just look around");
        if let Ok(dir) = std::env::var("KAHF_SHOTS") {
            h.save_png(format!("{dir}/welcome.png"));
        }
    }

    #[test]
    fn the_radio_screen_shows_the_channel_and_pickers() {
        let mut h = app(Page::Radio);
        h.expect("The channel").expect("How the radio is heard").expect("Radio control");
        if let Ok(dir) = std::env::var("KAHF_SHOTS") {
            h.save_png(format!("{dir}/radio.png"));
        }
    }
}
