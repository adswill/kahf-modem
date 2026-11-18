//! The relay's link to the kahf.dev backbone (server/README.md, "HF backbone for relays").
//!
//! A relay is online. It tells the server which stations it hears, hands over messages for
//! stations it cannot reach itself, and picks up messages for the ones it can. Every request is
//! signed with the relay's station key, in the same canonical JSON form as `/register`.
//!
//! HTTP goes through the system `curl` (present on every supported system) so the engine needs
//! no TLS stack of its own. Nothing here runs unless relay mode is on and a server is set.

use std::io::Write;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::process::{Command, Stdio};

use kahf_core::auth::StationKey;
use serde_json::{json, Map, Value};

pub struct Backbone {
    pub server: String,
    pub relay: String,
    key: StationKey,
}

/// A message the server hands to this relay for a station it hears.
#[derive(Clone, Debug, PartialEq)]
pub struct Queued {
    pub id: String,
    pub origin: String,
    pub dest: String,
    pub kind: String,
    pub subject: String,
    pub body: Vec<u8>,
    pub urgent: bool,
}

/// What happened to a message this relay's stations sent into the network.
#[derive(Clone, Debug, PartialEq)]
pub struct Receipt {
    pub id: String,
    pub dest: String,
    pub status: String,
    pub reason: String,
}

#[derive(Default, Debug, PartialEq)]
pub struct Pulled {
    pub messages: Vec<Queued>,
    pub receipts: Vec<Receipt>,
}

/// JSON with sorted keys and no spaces, numbers as JavaScript prints them. The server verifies
/// signatures over exactly this text.
pub fn canonical(v: &Value) -> String {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let parts: Vec<String> =
                keys.iter().map(|k| format!("{}:{}", Value::String((*k).clone()), canonical(&m[*k]))).collect();
            format!("{{{}}}", parts.join(","))
        }
        Value::Array(a) => format!("[{}]", a.iter().map(canonical).collect::<Vec<_>>().join(",")),
        Value::Number(n) => match n.as_f64() {
            Some(f) if n.is_f64() && f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", f as i64),
            _ => n.to_string(),
        },
        other => other.to_string(),
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(bytes: &[u8]) -> String {
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = 0u32;
    let mut bits = 0;
    for ch in s.bytes() {
        if ch == b'=' {
            break;
        }
        let v = B64.iter().position(|&c| c == ch)? as u32;
        buf = buf << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

fn now_s() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Backbone {
    pub fn new(server: &str, relay: &str, key: StationKey) -> Self {
        Backbone { server: server.trim_end_matches('/').to_string(), relay: relay.to_uppercase(), key }
    }

    /// The signed body for `fields`: the fields plus the key and the signature over them.
    pub fn signed(&self, fields: Map<String, Value>) -> Value {
        let sig = self.key.sign_raw(canonical(&Value::Object(fields.clone())).as_bytes());
        let mut body = fields;
        body.insert("public_key".into(), json!(b64_encode(&self.key.to_public())));
        body.insert("signature".into(), json!(b64_encode(&sig)));
        Value::Object(body)
    }

    /// `ts` is part of /relay/heard and /msg/pull only; submit and receipt do not take it.
    fn fields(&self, extra: Value, ts: bool) -> Map<String, Value> {
        let mut m = match extra {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        m.insert("relay".into(), json!(self.relay));
        if ts {
            m.insert("ts".into(), json!(now_s()));
        }
        m
    }

    fn post(&self, path: &str, extra: Value) -> Result<Value, String> {
        let body = self.signed(self.fields(extra, matches!(path, "/relay/heard" | "/msg/pull")));
        self.post_body(path, &body)
    }

    fn post_body(&self, path: &str, body: &Value) -> Result<Value, String> {
        let url = format!("{}{}", self.server, path);
        let mut child = Command::new("curl")
            .args(["-sS", "-m", "20", "-X", "POST", "-H", "content-type: application/json", "--data-binary", "@-", "-w", "\n%{http_code}", &url])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("curl did not start: {e}"))?;
        child.stdin.take().ok_or("curl has no input")?.write_all(body.to_string().as_bytes()).map_err(|e| e.to_string())?;
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(format!("no connection to the server: {}", String::from_utf8_lossy(&out.stderr).trim()));
        }
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        let (payload, code) = text.rsplit_once('\n').unwrap_or(("", &text));
        let code: u16 = code.trim().parse().unwrap_or(0);
        let v: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
        if code == 200 {
            Ok(v)
        } else {
            let why = v["error"].as_str().unwrap_or(payload.trim()).to_string();
            Err(format!("the server said {code}: {why}"))
        }
    }

    /// Stations this relay hears now: `(callsign, snr, heard at unix seconds)`.
    pub fn heard(&self, list: &[(String, Option<i32>, u64)]) -> Result<(), String> {
        if list.is_empty() {
            return Ok(());
        }
        let rows: Vec<Value> = list
            .iter()
            .take(200)
            .map(|(c, snr, at)| match snr {
                Some(s) => json!({ "callsign": c, "snr": s, "at": at }),
                None => json!({ "callsign": c, "at": at }),
            })
            .collect();
        self.post("/relay/heard", json!({ "heard": rows })).map(|_| ())
    }

    /// Puts a message into the network. `Ok(true)` when the server took it, `Ok(false)` when
    /// another relay had already submitted the same id.
    pub fn submit(&self, m: &Queued, created: u64, ttl_s: u64, hops: u8) -> Result<bool, String> {
        let mut f = json!({
            "id": m.id, "origin": m.origin, "dest": m.dest, "kind": m.kind,
            "body": b64_encode(&m.body), "created": created, "ttl_s": ttl_s, "hops": hops,
        });
        if !m.subject.is_empty() {
            f["subject"] = json!(m.subject);
        }
        if m.urgent {
            f["urgent"] = json!(true);
        }
        let v = self.post("/msg/submit", f)?;
        Ok(!v["duplicate"].as_bool().unwrap_or(false))
    }

    /// Messages waiting for the stations heard now, and receipts for stations that sent some.
    pub fn pull(&self, callsigns: &[String]) -> Result<Pulled, String> {
        if callsigns.is_empty() {
            return Ok(Pulled::default());
        }
        let v = self.post("/msg/pull", json!({ "callsigns": callsigns.iter().take(100).collect::<Vec<_>>() }))?;
        Ok(parse_pull(&v))
    }

    pub fn receipt(&self, id: &str, delivered: bool, reason: &str) -> Result<(), String> {
        let mut f = json!({ "id": id, "status": if delivered { "delivered" } else { "failed" } });
        if !reason.is_empty() {
            f["reason"] = json!(reason);
        }
        self.post("/msg/receipt", f).map(|_| ())
    }
}

pub fn parse_pull(v: &Value) -> Pulled {
    let text = |m: &Value, k: &str| m[k].as_str().unwrap_or("").to_string();
    Pulled {
        messages: v["messages"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|m| {
                        Some(Queued {
                            id: text(m, "id"),
                            origin: text(m, "origin"),
                            dest: text(m, "dest"),
                            kind: text(m, "kind"),
                            subject: text(m, "subject"),
                            body: b64_decode(m["body"].as_str()?)?,
                            urgent: m["urgent"].as_bool().unwrap_or(false),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        receipts: v["receipts"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|r| Receipt { id: text(r, "id"), dest: text(r, "dest"), status: text(r, "status"), reason: text(r, "reason") })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// A server message id for a message heard on the air: the origin, and a number made from the
/// content and the minute. Two relays that hear the same burst make the same id, so the server
/// keeps only one copy.
pub fn message_id(origin: &str, stream: &[u8], now_s: u64) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in stream.iter().chain((now_s / 60).to_be_bytes().iter()) {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{}-{}", origin.to_uppercase(), h % 4_000_000_000)
}

/// Work for the server thread.
pub enum BbJob {
    Heard(Vec<(String, Option<i32>, u64)>),
    Submit(Queued, u64, u64),
    Pull(Vec<String>),
    Receipt(String, bool, String),
}

/// What the server thread reports back.
pub enum BbResult {
    Pulled(Pulled),
    Log(String),
}

/// The server thread. HTTP can take seconds, so it never runs on the radio's thread.
pub struct BbHandle {
    tx: Sender<BbJob>,
    results: Arc<Mutex<Vec<BbResult>>>,
}

impl BbHandle {
    pub fn spawn(bb: Backbone) -> Self {
        let (tx, rx) = channel::<BbJob>();
        let results: Arc<Mutex<Vec<BbResult>>> = Arc::new(Mutex::new(Vec::new()));
        let out = results.clone();
        std::thread::spawn(move || {
            let mut last_error = String::new();
            // One line per new problem, not one per try.
            fn report(out: &Arc<Mutex<Vec<BbResult>>>, last: &mut String, r: Result<(), String>, what: &str) {
                match r {
                    Err(e) if e != *last => {
                        *last = e.clone();
                        if let Ok(mut o) = out.lock() {
                            o.push(BbResult::Log(format!("Server: {what} failed: {e}")));
                        }
                    }
                    Err(_) => {}
                    Ok(()) => last.clear(),
                }
            }
            while let Ok(job) = rx.recv() {
                match job {
                    BbJob::Heard(list) => report(&out, &mut last_error, bb.heard(&list), "reporting stations"),
                    BbJob::Submit(q, created, ttl) => {
                        let mut r = Err(String::new());
                        for attempt in 0..3 {
                            r = bb.submit(&q, created, ttl, 0).map(|_| ());
                            if r.is_ok() {
                                break;
                            }
                            if attempt < 2 {
                                std::thread::sleep(Duration::from_secs(5));
                            }
                        }
                        report(&out, &mut last_error, r, "handing a message over");
                    }
                    BbJob::Pull(calls) => match bb.pull(&calls) {
                        Ok(p) => {
                            if !p.messages.is_empty() || !p.receipts.is_empty() {
                                if let Ok(mut o) = out.lock() {
                                    o.push(BbResult::Pulled(p));
                                }
                            }
                            last_error.clear();
                        }
                        Err(e) => report(&out, &mut last_error, Err(e), "collecting messages"),
                    },
                    BbJob::Receipt(id, ok, why) => report(&out, &mut last_error, bb.receipt(&id, ok, &why), "sending a receipt"),
                }
            }
        });
        BbHandle { tx, results }
    }

    pub fn send(&self, job: BbJob) {
        let _ = self.tx.send(job);
    }

    pub fn take_results(&self) -> Vec<BbResult> {
        self.results.lock().map(|mut r| std::mem::take(&mut *r)).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two relays through a local copy of the real server code. Needs node; run by hand:
    /// `KAHF_BB_URL=http://127.0.0.1:8787 cargo test -p kahf-daemon backbone_round_trip -- --ignored`
    /// with `node server/tools/serve-local.mjs 8787` running.
    #[test]
    #[ignore]
    fn backbone_round_trip_through_a_local_server() {
        let url = std::env::var("KAHF_BB_URL").unwrap_or_else(|_| "http://127.0.0.1:8787".into());
        let r1 = Backbone::new(&url, "R1AAA", StationKey::generate());
        let r2 = Backbone::new(&url, "R2BBB", StationKey::generate());
        for (r, area) in [(&r1, "N51W1"), (&r2, "N50E8")] {
            let mut f = Map::new();
            f.insert("area".into(), json!(area));
            f.insert("callsign".into(), json!(r.relay));
            let body = r.signed(f);
            r.post_body("/register", &body).expect("register");
        }
        // R1 hears Alice and takes her message for Dave, whom only R2 hears.
        r1.heard(&[("K1ALF".into(), Some(12), now_s())]).expect("heard 1");
        r2.heard(&[("D1AVE".into(), Some(9), now_s())]).expect("heard 2");
        let q = Queued {
            id: message_id("K1ALF", b"hello Dave", now_s()),
            origin: "K1ALF".into(),
            dest: "D1AVE".into(),
            kind: "chat".into(),
            subject: String::new(),
            body: b"hello Dave".to_vec(),
            urgent: false,
        };
        assert!(r1.submit(&q, now_s(), 3600, 0).expect("submit"), "first submit must be taken");
        assert!(!r2.submit(&q, now_s(), 3600, 0).expect("submit again"), "second submit must be a duplicate");
        let got = r2.pull(&["D1AVE".into()]).expect("pull");
        assert_eq!(got.messages.len(), 1);
        assert_eq!(got.messages[0].body, b"hello Dave");
        r2.receipt(&q.id, true, "").expect("receipt");
        let back = r1.pull(&["K1ALF".into()]).expect("pull receipts");
        assert_eq!(back.receipts.len(), 1);
        assert_eq!(back.receipts[0].status, "delivered");
        assert_eq!(back.receipts[0].dest, "D1AVE");
    }

    #[test]
    fn canonical_json_matches_what_the_server_checks() {
        let v = json!({ "b": [1, 2.5, "x"], "a": { "z": null, "y": true }, "n": 14100.0 });
        assert_eq!(canonical(&v), r#"{"a":{"y":true,"z":null},"b":[1,2.5,"x"],"n":14100}"#);
    }

    #[test]
    fn base64_round_trips() {
        for data in [&b""[..], b"f", b"fo", b"foo", b"foob", &[0xfb, 0xff, 0x00, 0x10]] {
            assert_eq!(b64_decode(&b64_encode(data)).unwrap(), data);
        }
        assert_eq!(b64_encode(b"foo"), "Zm9v");
    }

    #[test]
    fn a_request_carries_its_key_and_a_signature_over_the_fields() {
        let key = StationKey::generate();
        let b = Backbone::new("https://kahf.dev/", "dl0net", key);
        let signed = b.signed(b.fields(json!({ "callsigns": ["K1ABC"] }), true));
        assert_eq!(signed["relay"], "DL0NET");
        assert_eq!(signed["public_key"].as_str().unwrap().len(), 44);
        assert_eq!(signed["signature"].as_str().unwrap().len(), 88);
        assert_eq!(b.server, "https://kahf.dev");
    }

    #[test]
    fn the_same_burst_gets_the_same_id_at_every_relay() {
        let a = message_id("ka1alf", b"hello", 1_800_000_030);
        assert_eq!(a, message_id("KA1ALF", b"hello", 1_800_000_050));
        assert_ne!(a, message_id("KA1ALF", b"hello!", 1_800_000_030));
        let num: u64 = a.rsplit('-').next().unwrap().parse().unwrap();
        assert!(num < 4_000_000_000 && a.starts_with("KA1ALF-"));
    }

    #[test]
    fn a_pull_answer_is_read_into_messages_and_receipts() {
        let v = json!({
            "messages": [{ "id": "K1ABC-5", "origin": "K1ABC", "dest": "DL1XYZ", "kind": "chat", "body": b64_encode(b"hi"), "created": 1 }],
            "receipts": [{ "id": "K1ABC-4", "dest": "DL1XYZ", "status": "delivered", "at": 2 }]
        });
        let p = parse_pull(&v);
        assert_eq!(p.messages[0].body, b"hi");
        assert_eq!(p.messages[0].dest, "DL1XYZ");
        assert_eq!(p.receipts[0].status, "delivered");
    }
}
