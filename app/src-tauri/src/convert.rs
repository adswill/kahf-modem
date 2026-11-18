//! Engine events to the JSON the window understands (see `ui/src/lib/types.ts`).

use kahf_core::link::LinkState;
use kahf_daemon::{Event, TxState};
use serde_json::{json, Value};

/// A message the window sent that is still in the engine's outbox.
#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    pub ui_id: String,
    pub to: String,
    pub engine_id: u64,
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

/// The window events for one engine event. Zero, one or several.
pub fn event_json(ev: &Event, pending: &[Pending]) -> Vec<Value> {
    match ev {
        Event::Link(state) => {
            let link = match state {
                LinkState::Idle | LinkState::Disconnecting => json!({ "state": "idle" }),
                // The engine does not report which try it is on, so none is shown.
                LinkState::Connecting { peer } => json!({ "state": "calling", "peer": peer, "attempt": 0, "of": 0 }),
                LinkState::Connected { peer } => json!({ "state": "linked", "peer": peer }),
            };
            vec![json!({ "type": "link", "link": link })]
        }
        Event::Message { from, text } => vec![
            json!({ "type": "heard", "heard": { "call": from, "snr": Value::Null, "at": now_ms() } }),
            json!({ "type": "message", "from": from, "text": text, "at": now_ms() }),
        ],
        Event::Mail { from, subject, .. } => vec![json!({
            "type": "message", "from": from, "at": now_ms(),
            "text": format!("Mail: {subject} (saved in your inbox folder)")
        })],
        Event::FileProgress { name, done, total, .. } => {
            if done >= total {
                vec![json!({ "type": "log", "text": format!("File {name} transferred.") })]
            } else {
                Vec::new()
            }
        }
        Event::Spectrum(bins) => vec![json!({ "type": "spectrum", "bins": thin_spectrum(bins) })],
        Event::Metrics(m) => vec![json!({
            "type": "metrics",
            "metrics": {
                "mode": m.mode, "auto": m.auto, "netBps": m.net_bps,
                "ownSnr": m.own_snr_db.map(round1), "peerSnr": m.peer_snr_db.map(round1),
                "ferPct": round1(m.fer_pct),
                "sent": m.frames_sent, "received": m.frames_received, "retransmits": m.retransmits
            }
        })],
        Event::Heard { call, snr_db } => vec![json!({
            "type": "heard",
            "heard": { "call": call, "snr": snr_db.map(|s| s.round() as i64), "at": now_ms() }
        })],
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

fn round1(v: f32) -> f64 {
    (v as f64 * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use kahf_daemon::LinkStatus;

    fn pend(id: &str, to: &str) -> Pending {
        Pending { ui_id: id.into(), to: to.into(), engine_id: 1 }
    }

    #[test]
    fn link_states_map_to_plain_names() {
        let c = event_json(&Event::Link(LinkState::Connecting { peer: "KA1ABC".into() }), &[]);
        assert_eq!(c[0]["link"]["state"], "calling");
        assert_eq!(c[0]["link"]["peer"], "KA1ABC");
        assert_eq!(event_json(&Event::Link(LinkState::Idle), &[])[0]["link"]["state"], "idle");
        let l = event_json(&Event::Link(LinkState::Connected { peer: "G4HAM".into() }), &[]);
        assert_eq!(l[0]["link"]["state"], "linked");
    }

    #[test]
    fn a_message_also_counts_as_hearing_the_sender() {
        let out = event_json(&Event::Message { from: "DL5XYZ".into(), text: "hi".into() }, &[]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["type"], "heard");
        assert_eq!(out[1]["text"], "hi");
    }

    #[test]
    fn spectrum_is_thinned_to_half_with_the_stronger_bin() {
        let t = thin_spectrum(&[-90.0, -80.04, -70.0, -100.0]);
        assert_eq!(t, vec![-80.0, -70.0]);
    }

    #[test]
    fn unreached_station_fails_the_messages_waiting_for_it() {
        let pending = [pend("a", "KA1ABC"), pend("b", "G4HAM"), pend("c", "ka1abc")];
        let out = event_json(&Event::Log("Could not reach KA1ABC on air. The station did not respond.".into()), &pending);
        let failed: Vec<&str> = out.iter().filter(|v| v["type"] == "failed").map(|v| v["id"].as_str().unwrap()).collect();
        assert_eq!(failed, vec!["a", "c"]);
        assert!(out[1]["reason"].as_str().unwrap().contains("did not answer"));
    }

    #[test]
    fn metrics_carry_the_mode_and_snr() {
        let st = LinkStatus {
            mode: "KAMI-32", auto: true, net_bps: 284, own_snr_db: Some(7.26), peer_snr_db: None,
            fer_pct: 0.0, frames_sent: 3, frames_received: 2, retransmits: 0,
        };
        let m = &event_json(&Event::Metrics(st), &[])[0]["metrics"];
        assert_eq!(m["mode"], "KAMI-32");
        assert_eq!(m["ownSnr"], 7.3);
        assert!(m["peerSnr"].is_null());
    }

    #[test]
    fn heard_station_carries_a_rounded_snr() {
        let h = &event_json(&Event::Heard { call: "VK3FIX".into(), snr_db: Some(-3.6) }, &[])[0]["heard"];
        assert_eq!(h["call"], "VK3FIX");
        assert_eq!(h["snr"], -4);
    }
}
