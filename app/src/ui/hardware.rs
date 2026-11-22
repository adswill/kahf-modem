//! The pickers that setup, the Radio screen and Settings share: how the radio's sound reaches
//! the computer, how the radio is controlled, and where the station is.

use alloy_ui::prelude::*;

use super::kit::*;
use crate::app::*;
use crate::types::*;
use crate::util::geo;

/// Which copy of the choices a picker edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Where {
    /// The Radio screen: applied together with a restart.
    Radio,
    /// Setup: a draft that becomes real at the end.
    Setup,
    /// Settings: saved at once.
    Settings,
}

fn hw(k: &Kahf, w: Where) -> &Hw {
    match w {
        Where::Setup => &k.ui.setup.hw,
        _ => &k.ui.hw,
    }
}

fn hw_mut(k: &mut Kahf, w: Where) -> &mut Hw {
    match w {
        Where::Setup => &mut k.ui.setup.hw,
        _ => &mut k.ui.hw,
    }
}

/// How the radio's sound reaches the computer: a sound card, an SDR receiver, or nothing yet.
pub fn audio_picker(k: &Kahf, w: Where) -> V {
    let h = hw(k, w);
    let kind = h.kind;
    let mut c = col![choice(
        kind == SoundKind::Sound,
        "Sound card or radio interface",
        "The radio's audio goes into this computer's sound input. The usual way.",
        move |k| hw_mut(k, w).kind = SoundKind::Sound,
    )]
    .gap(8.0);
    if kind == SoundKind::Sound {
        let (ain, aout) = (h.audio_in.clone(), h.audio_out.clone());
        let (din, dout) = (ain.clone(), aout.clone());
        c = c.push(
            row![
                lab(
                    "From the radio",
                    device_pick(k, "audio_in", &k.ui.devices, &ain, move |k, d| hw_mut(k, w)
                        .audio_in = d)
                )
                .fill_w(),
                lab(
                    "To the radio",
                    device_pick(k, "audio_out", &k.ui.devices, &aout, move |k, d| hw_mut(k, w)
                        .audio_out =
                        d)
                )
                .fill_w(),
            ]
            .gap(12.0),
        );
        c = c.push(
            row![
                test_button(k, "audio_in", "Test input", move |k, key| k
                    .test_audio_in(key, din.clone())),
                test_button(k, "audio_out", "Test output", move |k, key| k
                    .test_audio_out(key, dout.clone())),
                test_button(k, "level", "Check the level (3 s)", {
                    let d = ain.clone();
                    move |k, key| k.check_level(key, d.clone())
                }),
            ]
            .gap(12.0),
        );
        c = c.push(
            para("The input test listens for a moment. The output test plays silence. The level check listens for three seconds with the band open. None of them transmits.", pw(w)),
        );
    }
    c = c.push(choice(
        kind == SoundKind::Sdr,
        "An SDR receiver",
        "HackRF, RTL-SDR and similar USB radios that connect straight to the computer.",
        move |k| hw_mut(k, w).kind = SoundKind::Sdr,
    ));
    if kind == SoundKind::Sdr {
        if k.ui.sdrs.is_empty() {
            c = c.push(
                caption("None found. Plug it in, then reopen this.")
                    .dim()
                    .fill_w(),
            );
        } else {
            let sel = k.ui.sdrs.iter().position(|d| *d == h.sdr);
            let items = k.ui.sdrs.clone();
            c = c.push(lab(
                "Receiver",
                dropdown(k, "sdr", k.ui.sdrs.clone(), sel, move |k: &mut Kahf, i: usize| {
                    hw_mut(k, w).sdr = items[i].clone()
                }),
            ));
        }
    }
    c = c.push(choice(
        kind == SoundKind::None,
        "No radio yet",
        "Skip this. Nothing is heard until you connect one on the Radio screen.",
        move |k| hw_mut(k, w).kind = SoundKind::None,
    ));
    c.into()
}

/// Choose the radio by name (Hamlib), control it directly, or point at a running rigctld.
pub fn rig_picker(k: &Kahf, w: Where) -> V {
    let h = hw(k, w);
    let mut c = col![].gap(10.0);
    match k.ui.rig_available {
        Some(false) => {
            c = c.push(note(
                "Radio control needs Hamlib. It is a free program that talks to more than 300 radios.\nMac: brew install hamlib\nLinux: sudo apt install libhamlib-utils\nWindows: hamlib.github.io",
                true,
            ));
            c = c.push(button("I installed it, check again").act(|k: &mut Kahf| k.check_hamlib()));
        }
        Some(true) => c = c.push(hamlib_choice(k, w)),
        None => c = c.push(caption("Looking for Hamlib…").dim().fill_w()),
    }

    // Direct control: KAHF talks to the radio's serial port itself, without Hamlib.
    c = c.push(heading("Control the radio directly (no Hamlib needed)").sub());
    c = c.push(para("For the radios listed here KAHF talks to the serial port itself, and can key the transmitter by command or by a port line.", pw(w)));
    let mut models = vec!["Not used".to_string()];
    models.extend(k.ui.direct_rigs.iter().map(|m| m.label.clone()));
    let sel = if h.d_model.is_empty() {
        Some(0)
    } else {
        k.ui.direct_rigs
            .iter()
            .position(|m| m.id == h.d_model)
            .map(|i| i + 1)
    };
    let rigs = k.ui.direct_rigs.clone();
    let ports = port_pick(k, "d_port", &h.d_port, move |k, p| hw_mut(k, w).d_port = p);
    let ptt_sel = PTTS.iter().position(|(id, _)| *id == h.d_ptt);
    c = c.push(
        row![
            lab(
                "Radio",
                dropdown(k, "rig_model", models, sel, move |k: &mut Kahf, i: usize| {
                    let h = hw_mut(k, w);
                    if i == 0 {
                        h.d_model.clear();
                    } else {
                        h.d_model = rigs[i - 1].id.clone();
                        h.d_baud = rigs[i - 1].baud.to_string();
                    }
                })
            )
            .fill_w(),
            lab("Serial port", ports).fill_w(),
        ]
        .gap(12.0),
    );
    c = c.push(
        row![
            lab_hint(
                "Speed (baud)",
                input(h.d_baud.clone())
                    .hint("0")
                    .act(move |k: &mut Kahf, v: String| hw_mut(k, w).d_baud = v),
                "0 uses the radio's usual speed."
            )
            .fill_w(),
            lab(
                "Keying (PTT)",
                dropdown(
                    k,
                    "ptt",
                    PTTS.iter().map(|(_, l)| l.to_string()).collect::<Vec<_>>(),
                    ptt_sel,
                    move |k: &mut Kahf, i: usize| hw_mut(k, w).d_ptt = PTTS[i].0.to_string()
                ),
            )
            .fill_w(),
        ]
        .gap(12.0),
    );
    let spec = h.spec();
    if !spec.is_empty() {
        c = c.push(test_button(k, "rig_spec", "Test the radio", move |k, key| {
            k.test_rig_spec(key, spec.clone())
        }));
        c = c.push(
            caption("The test reads the radio's frequency. It never transmits.")
                .dim()
                .fill_w(),
        );
    }

    c = c.push(heading("Already running a radio control program?").sub());
    c = c.push(lab_hint(
        "rigctld address",
        input(h.rigctld.clone())
            .hint("127.0.0.1:4532")
            .act(move |k: &mut Kahf, v: String| hw_mut(k, w).rigctld = v),
        "Host and port of a rigctld you started yourself. Leave empty if KAHF starts it for you.",
    ));
    let addr = h.rigctld.clone();
    c = c.push(test_button(k, "rigctld", "Test the address", move |k, key| {
        k.test_rig_address(key, addr.clone())
    }));
    c.into()
}

fn port_pick(
    k: &Kahf,
    key: &'static str,
    current: &str,
    set: impl Fn(&mut Kahf, String) + Send + Sync + 'static,
) -> V {
    let mut names = vec!["Choose…".to_string()];
    names.extend(k.ui.rig_ports.iter().cloned());
    let sel = if current.is_empty() {
        Some(0)
    } else {
        k.ui.rig_ports.iter().position(|p| p == current).map(|i| i + 1)
    };
    let ports = k.ui.rig_ports.clone();
    dropdown(k, key, names, sel, move |k: &mut Kahf, i: usize| {
        set(
            k,
            if i == 0 {
                String::new()
            } else {
                ports[i - 1].clone()
            },
        )
    })
}

fn hamlib_choice(k: &Kahf, w: Where) -> V {
    let h = hw(k, w);
    let mut c = col![].gap(8.0);
    match h.rig.model {
        None => {
            c = c.push(lab_hint(
                "Search for your radio",
                input(k.ui.rig_query.clone())
                    .hint("For example IC-7300, FT-991, K3")
                    .act(|k: &mut Kahf, v: String| k.ui.rig_query = v),
                &format!("{} radios known. Type part of the name.", k.ui.rig_models.len()),
            ));
            let q = k.ui.rig_query.trim().to_lowercase();
            if !q.is_empty() {
                let words: Vec<&str> = q.split_whitespace().collect();
                let found: Vec<&RigModel> =
                    k.ui.rig_models
                        .iter()
                        .filter(|m| {
                            let name = format!("{} {}", m.maker, m.model).to_lowercase();
                            words.iter().all(|w| name.contains(w))
                        })
                        .take(8)
                        .collect();
                if found.is_empty() {
                    c = c.push(
                        caption(format!(
                            "No radio matches \"{}\". Try fewer letters.",
                            k.ui.rig_query.trim()
                        ))
                        .dim()
                        .fill_w(),
                    );
                }
                for m in found {
                    let (id, name) = (m.id, format!("{} {}", m.maker, m.model));
                    let label = if m.status != "Stable" {
                        format!("{name}  ({})", m.status)
                    } else {
                        name.clone()
                    };
                    c = c.push(button(label).fill_w().act(move |k: &mut Kahf| {
                        let h = hw_mut(k, w);
                        h.rig.model = Some(id);
                        h.rig.name = name.clone();
                        k.ui.rig_query.clear();
                        k.ui.tests.remove("rig_model");
                    }));
                }
            }
        }
        Some(_) => {
            c = c.push(
                row![
                    text(h.rig.name.clone()).label().fill_w(),
                    button("Change").ghost().act(move |k: &mut Kahf| {
                        let h = hw_mut(k, w);
                        h.rig.model = None;
                        h.rig.name.clear();
                        k.ui.tests.remove("rig_model");
                        k.spawn(kahf_app_core::rig_stop, |_, _| {});
                    }),
                ]
                .align_center(),
            );
            let bauds: Vec<String> = BAUDS.iter().map(|b| b.to_string()).collect();
            let sel = BAUDS.iter().position(|b| *b == h.rig.baud);
            c = c.push(
                row![
                    lab_hint(
                        "Serial port",
                        port_pick(k, "rig_port", &h.rig.port, move |k, p| hw_mut(k, w).rig.port = p),
                        "The USB cable from the radio to the computer."
                    )
                    .fill_w(),
                    lab_hint(
                        "Speed (baud)",
                        dropdown(k, "baud", bauds, sel, move |k: &mut Kahf, i: usize| hw_mut(
                            k, w
                        )
                        .rig
                        .baud =
                            BAUDS[i]),
                        "Must match the radio's menu setting."
                    )
                    .fill_w(),
                ]
                .gap(12.0),
            );
            let choice = h.rig.clone();
            let enabled = !h.rig.port.is_empty();
            let running = matches!(k.ui.tests.get("rig_model"), Some(TestState::Running));
            if enabled {
                c = c.push(test_button(
                    k,
                    "rig_model",
                    "Test the connection",
                    move |k, key| k.test_rig_model(key, choice.clone()),
                ));
            } else {
                c = c.push(
                    button(if running {
                        "Testing…"
                    } else {
                        "Test the connection"
                    })
                    .enabled(false),
                );
            }
            if matches!(
                k.ui.tests.get("rig_model"),
                Some(TestState::Done { ok: false, .. })
            ) {
                c = c.push(para("Check that the radio is on, the cable is in, and its USB or CAT speed matches. On an Icom the CI-V address must match too.", pw(w)));
            }
        }
    }
    c.into()
}

fn loc_mut(k: &mut Kahf, w: Where) -> (&mut Location, &mut PosText) {
    match w {
        Where::Setup => (&mut k.ui.setup.draft.location, &mut k.ui.setup.pos),
        _ => (&mut k.m.settings.location, &mut k.ui.pos),
    }
}

fn loc(k: &Kahf, w: Where) -> (&Location, &PosText) {
    match w {
        Where::Setup => (&k.ui.setup.draft.location, &k.ui.setup.pos),
        _ => (&k.m.settings.location, &k.ui.pos),
    }
}

/// After an edit: Settings saves at once (only a valid position, or none).
fn commit_location(k: &mut Kahf, w: Where) {
    let (l, p) = loc_mut(k, w);
    let (lat, lon) = (p.lat(), p.lon());
    let ok = geo::valid_lat(lat) && geo::valid_lon(lon);
    l.lat = if ok { lat } else { None };
    l.lon = if ok { lon } else { None };
    if w == Where::Settings {
        k.save_settings();
    }
}

/// Choose whether to share a location, and where. Used by setup and by Settings.
pub fn location_editor(k: &Kahf, w: Where) -> V {
    let (l, p) = loc(k, w);
    let mode = l.mode;
    let mut c = col![].gap(8.0);
    let options = [
        (
            LocationMode::Off,
            "Do not share my location",
            "Nobody sees where you are. KAHF works the same.",
        ),
        (
            LocationMode::Area,
            "Share a circle around me",
            "Others see a circle on the map. Your exact position stays private.",
        ),
        (
            LocationMode::Precise,
            "Share my exact position",
            "Shown to about 200 metres. Good for relays that want to be found.",
        ),
    ];
    for (m, t, s) in options {
        c = c.push(choice(mode == m, t, s, move |k| {
            loc_mut(k, w).0.mode = m;
            commit_location(k, w);
        }));
    }
    let (lat, lon) = (p.lat(), p.lon());
    let pos_ok = geo::valid_lat(lat) && geo::valid_lon(lon);
    if mode != LocationMode::Off {
        let locator_ok = geo::locator_to_lat_lon(&p.locator).is_some();
        let mut b = col![].gap(8.0);
        b = b.push(lab_hint(
            "Grid square",
            row![
                input(p.locator.clone())
                    .hint("For example JN45rm")
                    .act(move |k: &mut Kahf, v: String| loc_mut(k, w).1.locator = v)
                    .fill_w(),
                button("Use").enabled(locator_ok).act(move |k: &mut Kahf| {
                    let (_, p) = loc_mut(k, w);
                    if let Some((la, lo)) = geo::locator_to_lat_lon(&p.locator) {
                        p.lat = format!("{la:.3}");
                        p.lon = format!("{lo:.3}");
                        commit_location(k, w);
                    }
                }),
            ]
            .gap(8.0),
            "A short code radio amateurs use for a place.",
        ));
        b = b.push(
            row![
                lab(
                    "Latitude",
                    input(p.lat.clone())
                        .hint("51.507")
                        .act(move |k: &mut Kahf, v: String| {
                            loc_mut(k, w).1.lat = v;
                            commit_location(k, w);
                        })
                )
                .fill_w(),
                lab(
                    "Longitude",
                    input(p.lon.clone())
                        .hint("-0.128")
                        .act(move |k: &mut Kahf, v: String| {
                            loc_mut(k, w).1.lon = v;
                            commit_location(k, w);
                        })
                )
                .fill_w(),
            ]
            .gap(12.0),
        );
        if !p.lat.is_empty() || !p.lon.is_empty() {
            b = b.push(if pos_ok {
                text(format!(
                    "Grid square {}",
                    geo::lat_lon_to_locator(lat.unwrap_or(0.0), lon.unwrap_or(0.0))
                ))
                .caption()
                .success()
            } else {
                text("Latitude is -90 to 90 and longitude is -180 to 180.")
                    .caption()
                    .danger()
            });
        }
        if mode == LocationMode::Area {
            b = b.push(lab(
                &format!("Circle size: {} km", l.radius_km),
                slider(10.0..=100.0, l.radius_km as f32)
                    .step(10.0)
                    .act(move |k: &mut Kahf, v: f32| {
                        loc_mut(k, w).0.radius_km = v.round() as u32;
                        commit_location(k, w);
                    }),
            ));
        }
        c = c.push(b.pad(10.0).round(8.0).outline().fill_w());
    }
    c = c.push(note(
        geo::sharing_summary(mode.as_str(), lat, lon, l.radius_km),
        false,
    ));
    if mode != LocationMode::Off {
        c = c.push(note(
            "Today the public map lists relays, each by the one-degree square it is in. Other stations appear once they can announce themselves over the air. Your choice is saved now and used as soon as that works.",
            true,
        ));
    }
    c.into()
}

/// Paragraph width in characters: the Radio screen's side panel is narrower than the other screens.
fn pw(w: Where) -> usize {
    if w == Where::Radio {
        60
    } else {
        100
    }
}
