//! Engine events to the JSON the window understands (see `ui/src/lib/types.ts`).

use std::fs;
use std::path::{Path, PathBuf};

use kahf_core::link::LinkState;
use kahf_daemon::{Delivery, Event, TxState};
use serde_json::{json, Value};

/// A message the window sent that is still in the engine's outbox.
#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    pub ui_id: String,
    pub to: String,
    /// The outbox id. 0 while it is not known yet: a chat message gets no id back from the
    /// engine, so the poll loop adopts the first new outbox item for it.
    pub engine_id: u64,
    pub at: std::time::Instant,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 1024 audio bins (0 to 4 kHz) become 512, keeping the stronger of each pair, with one decimal.
pub fn thin_spectrum(bins: &[f32]) -> Vec<f64> {
    bins.chunks(2)
        .map(|c| (c.iter().copied().fold(f32::MIN, f32::max) as f64 * 10.0).round() / 10.0)
        .collect()
}

/// What the converter needs to know besides the event itself.
pub struct Ctx<'a> {
    /// Messages the window sent that are still in the outbox.
    pub pending: &'a [Pending],
    /// Where the engine saves received mail and files.
    pub inbox: &'a Path,
    /// The station this one is linked to, if any.
    pub peer: Option<&'a str>,
    /// Outbox ids this app queued, so their progress is never taken for an incoming file.
    pub own_ids: &'a [u64],
}

/// The body of a received mail, from the file the engine saved. Empty when it cannot be read.
fn mail_body(inbox: &Path, id: u64) -> (String, Vec<String>) {
    let Ok(text) = fs::read_to_string(inbox.join(format!("mail-{id}.txt"))) else {
        return (String::new(), Vec::new());
    };
    let rest = text.split_once("\n\n").map_or(text.as_str(), |(_, r)| r);
    let (body, files) = match rest.split_once("\nAttachments:\n") {
        Some((b, a)) => (
            b.trim_end().to_string(),
            a.lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect(),
        ),
        None => (rest.trim_end().to_string(), Vec::new()),
    };
    (body, files)
}

/// Where the engine saved an incoming file: `name` in the inbox, or `name (2)` and so on when the
/// name was taken. The newest of those is the one just received.
fn saved_path(inbox: &Path, name: &str) -> PathBuf {
    let first = inbox.join(name);
    let p = Path::new(name);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_string());
    let ext = p
        .extension()
        .map(|s| format!(".{}", s.to_string_lossy()))
        .unwrap_or_default();
    let modified = |f: &Path| f.metadata().and_then(|m| m.modified()).ok();
    let mut best = first.clone();
    for i in 2..200 {
        let c = inbox.join(format!("{stem} ({i}){ext}"));
        if !c.exists() {
            break;
        }
        if modified(&c) >= modified(&best) {
            best = c;
        }
    }
    best
}

/// The window events for one engine event. Zero, one or several.
pub fn event_json(ev: &Event, ctx: &Ctx) -> Vec<Value> {
    let pending = ctx.pending;
    match ev {
        Event::Link(state) => {
            let link = match state {
                LinkState::Idle | LinkState::Disconnecting => json!({ "state": "idle" }),
                // The engine does not report which try it is on, so none is shown.
                LinkState::Connecting { peer } => {
                    json!({ "state": "calling", "peer": peer, "attempt": 0, "of": 0 })
                }
                LinkState::Connected { peer } => json!({ "state": "linked", "peer": peer }),
            };
            vec![json!({ "type": "link", "link": link })]
        }
        Event::Message { from, text } => vec![
            json!({ "type": "heard", "heard": { "call": from, "snr": Value::Null, "at": now_ms() } }),
            json!({ "type": "message", "from": from, "text": text, "at": now_ms() }),
        ],
        Event::Mail { id, from, subject } => {
            let (body, files) = mail_body(ctx.inbox, *id);
            vec![
                json!({ "type": "heard", "heard": { "call": from, "snr": Value::Null, "at": now_ms() } }),
                json!({
                    "type": "mail", "from": from, "subject": subject, "body": body, "at": now_ms(),
                    "files": files.iter().map(|n| json!({ "name": n, "size": 0 })).collect::<Vec<_>>()
                }),
            ]
        }
        Event::FileProgress {
            id,
            name,
            done,
            total,
        } => match pending.iter().find(|p| p.engine_id == *id) {
            Some(p) => vec![json!({
                "type": "file", "id": p.ui_id, "name": name, "done": done, "total": total, "dir": "out"
            })],
            // Something this app sent whose entry is already gone: not an incoming file.
            None if ctx.own_ids.contains(id) => Vec::new(),
            // A file coming in: the engine reports it once, when it has been saved to the inbox.
            None if done >= total => {
                let path = saved_path(ctx.inbox, name);
                vec![json!({
                    "type": "file", "id": format!("in-{id}"), "name": name, "done": done, "total": total,
                    "dir": "in", "from": ctx.peer.unwrap_or(""),
                    "path": path.exists().then(|| path.to_string_lossy().to_string())
                })]
            }
            None => Vec::new(),
        },
        Event::Spectrum(bins) => vec![json!({ "type": "spectrum", "bins": thin_spectrum(bins) })],
        Event::Relay { call, online, .. } => {
            let what = if *online {
                format!("Relay {call} is on the air.")
            } else {
                format!("Relay {call} went off the air.")
            };
            vec![
                json!({ "type": "log", "text": what }),
                json!({ "type": "heard", "heard": { "call": call, "snr": Value::Null, "at": now_ms() } }),
            ]
        }
        Event::Heard { call, snr_db } => {
            vec![json!({ "type": "heard", "heard": { "call": call, "snr": snr_db, "at": now_ms() } })]
        }
        Event::Advert { call, .. } => {
            vec![json!({ "type": "heard", "heard": { "call": call, "snr": Value::Null, "at": now_ms() } })]
        }
        Event::MailReceived { .. } | Event::FileReceived { .. } => Vec::new(),
        Event::Metrics(_) => Vec::new(),
        Event::Delivery { id, state, note } => {
            let mut out = Vec::new();
            if !note.is_empty() {
                out.push(json!({ "type": "log", "text": note }));
            }
            // A relay or the station answered: the message left the outbox for good.
            if matches!(state, Delivery::AtRelay | Delivery::Delivered) {
                if let Some(p) = pending.iter().find(|p| p.engine_id == *id && *id != 0) {
                    out.push(json!({ "type": "sent", "id": p.ui_id }));
                }
            }
            out
        }
        Event::Tx { state } => {
            let (s, reason) = match state {
                TxState::Starting => ("starting", None),
                TxState::Sending => ("sending", None),
                TxState::Done => ("done", None),
                TxState::Failed(why) => ("failed", Some(why.clone())),
            };
            vec![json!({ "type": "tx", "state": s, "reason": reason })]
        }
        Event::Log(text) => {
            let mut out = vec![json!({ "type": "log", "text": text })];
            // "Could not reach XX1XX on air. ..." means the messages for that station did not go.
            if let Some(rest) = text.strip_prefix("Could not reach ") {
                let call = rest.split_whitespace().next().unwrap_or("");
                for p in pending.iter().filter(|p| p.to.eq_ignore_ascii_case(call)) {
                    out.push(json!({
                        "type": "failed", "id": p.ui_id,
                        "reason": format!("{call} did not answer. KAHF will try again when it hears them.")
                    }));
                }
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pend(id: &str, to: &str) -> Pending {
        Pending {
            ui_id: id.into(),
            to: to.into(),
            engine_id: 1,
            at: std::time::Instant::now(),
        }
    }

    fn ctx<'a>(pending: &'a [Pending], inbox: &'a Path) -> Ctx<'a> {
        Ctx {
            pending,
            inbox,
            peer: Some("G4HAM"),
            own_ids: &[],
        }
    }

    #[test]
    fn link_states_map_to_plain_names() {
        let p = Path::new("/nowhere");
        let c = event_json(
            &Event::Link(LinkState::Connecting {
                peer: "KA1ABC".into(),
            }),
            &ctx(&[], p),
        );
        assert_eq!(c[0]["link"]["state"], "calling");
        assert_eq!(c[0]["link"]["peer"], "KA1ABC");
        assert_eq!(
            event_json(&Event::Link(LinkState::Idle), &ctx(&[], p))[0]["link"]["state"],
            "idle"
        );
        let l = event_json(
            &Event::Link(LinkState::Connected { peer: "G4HAM".into() }),
            &ctx(&[], p),
        );
        assert_eq!(l[0]["link"]["state"], "linked");
    }

    #[test]
    fn a_message_also_counts_as_hearing_the_sender() {
        let out = event_json(
            &Event::Message {
                from: "DL5XYZ".into(),
                text: "hi".into(),
            },
            &ctx(&[], Path::new("/x")),
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["type"], "heard");
        assert_eq!(out[1]["text"], "hi");
    }

    #[test]
    fn a_received_mail_is_read_from_the_inbox() {
        let dir = std::env::temp_dir().join(format!("kahf-app-convert-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("mail-5.txt"),
            "From: G4HAM\nTo: XX0AB\nSubject: Plan\nSignature: none\n\nMeet at 9\n\nAttachments:\n  map.png\n",
        )
        .unwrap();
        let out = event_json(
            &Event::Mail {
                id: 5,
                from: "G4HAM".into(),
                subject: "Plan".into(),
            },
            &ctx(&[], &dir),
        );
        assert_eq!(out[1]["body"], "Meet at 9");
        assert_eq!(out[1]["files"][0]["name"], "map.png");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_received_file_comes_from_the_linked_station() {
        let out = event_json(
            &Event::FileProgress {
                id: 4,
                name: "a.txt".into(),
                done: 9,
                total: 9,
            },
            &ctx(&[], Path::new("/nowhere")),
        );
        assert_eq!(out[0]["dir"], "in");
        assert_eq!(out[0]["from"], "G4HAM");
        assert!(out[0]["path"].is_null());
    }

    #[test]
    fn a_received_note_points_at_the_file_the_engine_actually_saved() {
        let dir = std::env::temp_dir().join(format!("kahf-conv-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // The name was taken by an earlier note, so the engine saved this one as "clip-1 (2).kvm".
        fs::write(dir.join("clip-1.kvm"), b"old").unwrap();
        assert_eq!(saved_path(&dir, "clip-1.kvm"), dir.join("clip-1.kvm"));
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(dir.join("clip-1 (2).kvm"), b"new").unwrap();
        assert_eq!(saved_path(&dir, "clip-1.kvm"), dir.join("clip-1 (2).kvm"));
        let out = event_json(
            &Event::FileProgress {
                id: 8,
                name: "clip-1.kvm".into(),
                done: 6,
                total: 6,
            },
            &ctx(&[], &dir),
        );
        assert_eq!(
            out[0]["path"],
            dir.join("clip-1 (2).kvm").to_string_lossy().to_string()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn progress_of_something_this_app_sent_is_never_an_incoming_file() {
        let c = Ctx {
            pending: &[],
            inbox: Path::new("/x"),
            peer: Some("G4HAM"),
            own_ids: &[5],
        };
        let done = Event::FileProgress {
            id: 5,
            name: "x".into(),
            done: 9,
            total: 9,
        };
        assert!(event_json(&done, &c).is_empty());
    }

    #[test]
    fn sending_progress_belongs_to_the_waiting_message() {
        let p = [pend("ui-7", "KA1ABC")];
        let out = event_json(
            &Event::FileProgress {
                id: 1,
                name: "x".into(),
                done: 5,
                total: 10,
            },
            &ctx(&p, Path::new("/x")),
        );
        assert_eq!(out[0]["id"], "ui-7");
        assert_eq!(out[0]["dir"], "out");
        assert!(event_json(
            &Event::FileProgress {
                id: 9,
                name: "x".into(),
                done: 1,
                total: 2
            },
            &ctx(&[], Path::new("/x"))
        )
        .is_empty());
    }

    #[test]
    fn spectrum_is_thinned_to_half_with_the_stronger_bin() {
        let t = thin_spectrum(&[-90.0, -80.04, -70.0, -100.0]);
        assert_eq!(t, vec![-80.0, -70.0]);
    }

    #[test]
    fn unreached_station_fails_the_messages_waiting_for_it() {
        let pending = [pend("a", "KA1ABC"), pend("b", "G4HAM"), pend("c", "ka1abc")];
        let out = event_json(
            &Event::Log("Could not reach KA1ABC on air. The station did not respond.".into()),
            &ctx(&pending, Path::new("/x")),
        );
        let failed: Vec<&str> = out
            .iter()
            .filter(|v| v["type"] == "failed")
            .map(|v| v["id"].as_str().unwrap())
            .collect();
        assert_eq!(failed, vec!["a", "c"]);
    }

    #[test]
    fn relay_events_are_logged() {
        let out = event_json(
            &Event::Relay {
                call: "A1AAA".into(),
                online: true,
                time_s: 0,
                load: 0,
            },
            &ctx(&[], Path::new("/x")),
        );
        assert!(out[0]["text"].as_str().unwrap().contains("on the air"));
    }
}
