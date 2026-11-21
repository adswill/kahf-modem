//! The logbook: every contact, and sending it to other services.

use alloy_ui::prelude::*;

use super::kit::*;
use crate::app::*;
use crate::types::*;

fn matches(q: &Qso, needle: &str) -> bool {
    needle.is_empty() || [&q.call, &q.name, &q.comment, &q.band, &q.date].iter().any(|v| v.to_lowercase().contains(needle))
}

pub fn view(k: &Kahf) -> V {
    let needle = k.ui.book_filter.trim().to_lowercase();
    let rows: Vec<Qso> = k.m.book.qsos.iter().filter(|q| matches(q, &needle)).cloned().collect();
    let total = k.m.book.qsos.len();
    let mut c = col![row![
        heading("Logbook"),
        caption(total.to_string()).dim(),
        space(),
        edit(&k.ui.book_filter, "Find a callsign or note", |k, v| k.ui.book_filter = v).w(240.0),
        button("Open the data folder").act(|k: &mut Kahf| {
            if let Err(e) = kahf_app_core::open_data_dir(&k.shared()) {
                k.warn(e);
            }
        }),
        button("Export as ADIF").primary().enabled(total > 0).act(|k: &mut Kahf| k.export_logbook()),
    ]
    .gap(10.0)
    .align_center()]
    .gap(12.0);
    c = c.push(
        caption("Every contact KAHF logs is kept in a plain text file in your data folder. Export writes the same list as an ADIF file (logbook.adi) that other logging programs and the online logbooks can read.")
            .dim()
            .fill_w(),
    );
    if let Some(e) = &k.m.book.error {
        c = c.push(note(format!("A data file could not be read: {e}"), true));
    }
    if total == 0 {
        c = c.push(text("Nothing logged yet. Contacts appear here, newest first, as soon as KAHF logs them.").dim());
    } else if rows.is_empty() {
        c = c.push(text(format!("No contact matches \"{}\".", k.ui.book_filter.trim())).dim());
    } else {
        c = c.push(
            table(&rows)
                .col("Date", |q: &Qso| q.date.clone())
                .col("Time (UTC)", |q: &Qso| q.time.clone())
                .col("Station", |q: &Qso| q.call.clone())
                .col("Name", |q: &Qso| q.name.clone())
                .col("Band", |q: &Qso| q.band.clone())
                .col("Frequency", |q: &Qso| q.freq_mhz.map(|f| format!("{f:.5} MHz")).unwrap_or_else(|| "-".into()))
                .col("Sent", |q: &Qso| if q.rst_sent.is_empty() { "-".into() } else { q.rst_sent.clone() })
                .col("Received", |q: &Qso| if q.rst_rcvd.is_empty() { "-".into() } else { q.rst_rcvd.clone() })
                .col("Note", |q: &Qso| q.comment.clone()),
        );
    }
    c = c.push(uploads(k, total));
    scroll(c.pad(16.0)).fill().into()
}

fn secret(value: &str, set: impl Fn(&mut Kahf, String) + Send + Sync + 'static) -> V {
    input(value.to_string()).secure().act(move |k: &mut Kahf, v: String| set(k, v)).into()
}

fn uploads(k: &Kahf, total: usize) -> V {
    let open = k.ui.uploads_open;
    let mut c = col![button(if open { "Hide: send the logbook to other services" } else { "Send the logbook to other services" })
        .ghost()
        .act(|k: &mut Kahf| k.ui.uploads_open = !k.ui.uploads_open)]
    .gap(10.0);
    if !open {
        return c.into();
    }
    let e = &k.m.settings.export_services;
    let busy = k.ui.upload_busy;
    let go = |label: &str, ok: bool, service: &'static str| -> V {
        button(if busy { "Sending…" } else { label }).enabled(!busy && ok && total > 0).act(move |k: &mut Kahf| {
            k.save_settings();
            k.upload_logbook(service);
        }).into()
    };
    c = c.push(caption("Nothing is sent until you press a button. The keys and passwords below are kept on this computer, in the settings file, as plain text.").dim().fill_w());
    c = c.push(titled(
        "QRZ Logbook",
        col![lab("Logbook API key", secret(&e.qrz_key, |k, v| k.m.settings.export_services.qrz_key = v)), go("Send to QRZ", !e.qrz_key.is_empty(), "qrz")].gap(8.0),
    ));
    c = c.push(titled(
        "Club Log",
        col![
            row![
                lab("Email", edit(&e.clublog_email, "", |k, v| k.m.settings.export_services.clublog_email = v)).fill_w(),
                lab("Password", secret(&e.clublog_password, |k, v| k.m.settings.export_services.clublog_password = v)).fill_w(),
                lab("Application key", secret(&e.clublog_app_key, |k, v| k.m.settings.export_services.clublog_app_key = v)).fill_w(),
            ]
            .gap(10.0),
            go("Send to Club Log", !e.clublog_email.is_empty() && !e.clublog_password.is_empty() && !e.clublog_app_key.is_empty(), "clublog"),
        ]
        .gap(8.0),
    ));
    c = c.push(titled(
        "Logbook of the World (needs ARRL TQSL)",
        col![
            row![
                lab("Path of TQSL", edit(&e.tqsl_path, "/Applications/TQSL.app/Contents/MacOS/tqsl", |k, v| k.m.settings.export_services.tqsl_path = v)).fill_w(),
                lab("Station location name", edit(&e.lotw_location, "", |k, v| k.m.settings.export_services.lotw_location = v)).fill_w(),
            ]
            .gap(10.0),
            go("Send with TQSL", !e.tqsl_path.is_empty() && !e.lotw_location.is_empty(), "lotw"),
        ]
        .gap(8.0),
    ));
    c = c.push(titled(
        "N1MM Logger+ (on this network)",
        col![lab("Address", edit(&e.n1mm_addr, "127.0.0.1:12060", |k, v| k.m.settings.export_services.n1mm_addr = v)), go("Send to N1MM", !e.n1mm_addr.is_empty(), "n1mm")].gap(8.0),
    ));
    c.into()
}
