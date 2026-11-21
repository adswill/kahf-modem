//! The relay's link to the kahf.dev relay mesh (server/README.md, "The relay mesh").
//!
//! A relay is online. It tells the server which stations it hears (as 24 bit hashes), hands a message
//! for a station it cannot reach itself to the mesh, which routes it to the relay that hears that
//! station (the message only passes through the server's memory; nothing is stored), and picks up what
//! is waiting for it. Every call is signed with the relay's station key over `mesh1|CALL|ts|kind`.
//!
//! HTTP goes through the system `curl` (present on every supported system) so the engine needs
//! no TLS stack of its own. Nothing here runs unless relay mode is on and a server is set.

use std::collections::HashMap;
use std::io::Write;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
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
#[cfg_attr(not(test), allow(dead_code))]
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

/// The project's public signing key (base64). The matching private seed is the server secret `PROJECT_KEY`
/// (server/README.md); it signs the relay plans. Changing the key means a new app version.
pub const PROJECT_PUBLIC_KEY_B64: &str = "F52kG5qSH4UDBdMsflIWctLs49hpyaYFQV94IcmU2Fs=";

/// The project public key as bytes.
pub fn project_public_key() -> [u8; 32] {
    b64_decode(PROJECT_PUBLIC_KEY_B64).and_then(|v| <[u8; 32]>::try_from(v).ok()).unwrap_or([0; 32])
}

fn now_s() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Backbone {
    pub fn new(server: &str, relay: &str, key: StationKey) -> Self {
        Backbone { server: server.trim_end_matches('/').to_string(), relay: relay.to_uppercase(), key }
    }

    fn signed_body(&self, kind: &str, extra: &str, mut fields: Map<String, Value>) -> Value {
        let ts = now_s();
        let canonical = if extra.is_empty() {
            format!("mesh1|{}|{ts}|{kind}", self.relay)
        } else {
            format!("mesh1|{}|{ts}|{kind}|{extra}", self.relay)
        };
        let sig = self.key.sign_raw(canonical.as_bytes());
        fields.insert("relay".into(), json!(self.relay));
        fields.insert("ts".into(), json!(ts));
        fields.insert("public_key".into(), json!(b64_encode(&self.key.to_public())));
        fields.insert("signature".into(), json!(b64_encode(&sig)));
        Value::Object(fields)
    }

    /// Sends frames; returns the server's replies (`routed`, `unknown`, `dup`, `found`).
    fn post_frames(&self, frames: Vec<Value>) -> Result<Vec<Value>, String> {
        let frames_json = Value::Array(frames).to_string();
        let mut f = Map::new();
        f.insert("frames_json".into(), json!(frames_json));
        let body = self.signed_body("frames", &frames_json, f);
        let v = self.post_body("/mesh/frames", &body)?;
        Ok(v["replies"].as_array().cloned().unwrap_or_default())
    }

    /// What the mesh holds for this relay (at most 60 s old).
    fn post_poll(&self) -> Result<Vec<Value>, String> {
        let body = self.signed_body("poll", "", Map::new());
        let v = self.post_body("/mesh/poll", &body)?;
        Ok(v["frames"].as_array().cloned().unwrap_or_default())
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

    /// Stations this relay hears now: `(callsign, snr, heard at unix seconds)`. The server learns
    /// only the 24 bit hash of each callsign.
    pub fn heard(&self, list: &[(String, Option<i32>, u64)]) -> Result<(), String> {
        if list.is_empty() {
            return Ok(());
        }
        let frames: Vec<Value> = list
            .iter()
            .take(40)
            .map(|(c, snr, _)| json!({ "t": "heard", "dest": kahf_core::bundle::h24(c), "snr": snr.unwrap_or(0), "lane": 0 }))
            .collect();
        self.post_frames(frames).map(|_| ())
    }

    /// Stations that told this relay they are here, by hash.
    pub fn heard_hashes(&self, list: &[(u32, i32)]) -> Result<(), String> {
        if list.is_empty() {
            return Ok(());
        }
        let frames: Vec<Value> = list.iter().take(40).map(|(h, snr)| json!({ "t": "heard", "dest": h, "snr": snr, "lane": 0 })).collect();
        self.post_frames(frames).map(|_| ())
    }

    /// Hands a message to the mesh. `Ok(Routed)` when a relay that hears the destination was found,
    /// `Ok(Unrouted)` when nobody does yet (the caller sends it on the radio), `Ok(Duplicate)` when
    /// another relay already handed over the same id.
    pub fn submit(&self, m: &Queued) -> Result<Submitted, String> {
        let replies = self.post_frames(vec![submit_frame(m)])?;
        Ok(match replies.first().map(|r| r["t"].as_str().unwrap_or("")) {
            Some("dup") => Submitted::Duplicate,
            Some("routed") => Submitted::Routed,
            _ => Submitted::Unrouted,
        })
    }

    /// What is waiting for this relay: messages for stations it hears, and receipts for messages
    /// its stations sent. The stations list is not needed: the mesh knows from `heard`.
    pub fn pull(&self) -> Result<Pulled, String> {
        Ok(parse_frames(&self.post_poll()?))
    }

    pub fn receipt(&self, id: &str, delivered: bool, reason: &str) -> Result<(), String> {
        self.post_frames(vec![json!({ "t": "receipt", "id": id, "status": if delivered { "delivered" } else { "failed" }, "reason": reason })])
            .map(|_| ())
    }

    /// How many messages queued here, so the planner can give the relay air time.
    pub fn queue(&self, items: usize, bytes: usize) -> Result<(), String> {
        self.post_frames(vec![json!({ "t": "queue", "items": items, "bytes": bytes, "urgency": 0 })]).map(|_| ())
    }
}

/// The mesh frame for a message: ids, size, and the message itself as an opaque blob that only passes
/// through the server's memory to the delivering relay.
fn submit_frame(m: &Queued) -> Value {
    let blob = json!({
        "origin": m.origin, "dest": m.dest, "kind": m.kind, "subject": m.subject,
        "body": b64_encode(&m.body), "urgent": m.urgent,
    })
    .to_string();
    json!({ "t": "submit", "id": m.id, "dest": kahf_core::bundle::h24(&m.dest), "size": m.body.len(), "blob": b64_encode(blob.as_bytes()) })
}

/// What `Backbone::submit` found out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Submitted {
    Routed,
    Unrouted,
    Duplicate,
}

/// Turns the frames the mesh pushed into messages and receipts. A `deliver` without its blob is
/// dropped (the sender's relay asks again).
pub fn parse_frames(frames: &[Value]) -> Pulled {
    // Numbers (the 24 bit destination hash) read as their digits.
    let text = |m: &Value, k: &str| match &m[k] {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    };
    let mut out = Pulled::default();
    for f in frames {
        match f["t"].as_str() {
            Some("deliver") => {
                let Some(blob) = f["blob"].as_str().and_then(b64_decode) else { continue };
                let Ok(v) = serde_json::from_slice::<Value>(&blob) else { continue };
                let Some(body) = v["body"].as_str().and_then(b64_decode) else { continue };
                out.messages.push(Queued {
                    id: text(f, "id"),
                    origin: text(&v, "origin"),
                    dest: text(&v, "dest"),
                    kind: text(&v, "kind"),
                    subject: text(&v, "subject"),
                    body,
                    urgent: v["urgent"].as_bool().unwrap_or(false),
                });
            }
            Some("receipt") => out.receipts.push(Receipt {
                id: text(f, "id"),
                dest: text(f, "dest"),
                status: text(f, "status"),
                reason: text(f, "reason"),
            }),
            _ => {}
        }
    }
    out
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

/// Percent-encodes a query value (base64 uses + / =).
fn pct(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

impl Backbone {
    /// The signed WebSocket address: `ws(s)://host/mesh/ws?relay=..&ts=..&key=..&sig=..`.
    pub fn ws_url(&self) -> String {
        let ts = now_s();
        let sig = self.key.sign_raw(format!("mesh1|{}|{ts}|ws", self.relay).as_bytes());
        let base = if let Some(rest) = self.server.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = self.server.strip_prefix("http://") {
            format!("ws://{rest}")
        } else {
            self.server.clone()
        };
        format!("{base}/mesh/ws?relay={}&ts={ts}&key={}&sig={}", pct(&self.relay), pct(&b64_encode(&self.key.to_public())), pct(&b64_encode(&sig)))
    }
}

type Socket = tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>;

/// The push link: one blocking WebSocket with short read waits. The server pushes deliveries,
/// receipts and plans the moment they exist.
struct Ws {
    socket: Socket,
}

impl Ws {
    fn open(url: &str) -> Result<Ws, String> {
        let (socket, _) = tungstenite::connect(url).map_err(|e| format!("no WebSocket: {e}"))?;
        let ws = Ws { socket };
        // Small frames must not wait for each other (Nagle): a push is only useful if it leaves at once.
        if let tungstenite::stream::MaybeTlsStream::Plain(s) = ws.socket.get_ref() {
            let _ = s.set_nodelay(true);
        }
        ws.set_wait(Duration::from_millis(4));
        Ok(ws)
    }

    fn set_wait(&self, d: Duration) {
        use tungstenite::stream::MaybeTlsStream;
        match self.socket.get_ref() {
            MaybeTlsStream::Plain(s) => {
                let _ = s.set_read_timeout(Some(d));
            }
            MaybeTlsStream::Rustls(s) => {
                let _ = s.get_ref().set_read_timeout(Some(d));
            }
            _ => {}
        }
    }

    fn send(&mut self, v: &Value) -> Result<(), String> {
        self.socket.send(tungstenite::Message::text(v.to_string())).map_err(|e| e.to_string())
    }

    /// The next frame, `Ok(None)` when nothing arrived within the wait, `Err` when the socket is gone.
    fn recv(&mut self) -> Result<Option<Value>, String> {
        match self.socket.read() {
            Ok(tungstenite::Message::Text(t)) => Ok(serde_json::from_str(t.as_str()).ok()),
            Ok(tungstenite::Message::Close(_)) => Err("closed".into()),
            Ok(_) => Ok(None),
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// Work for the server thread.
pub enum BbJob {
    Heard(Vec<(String, Option<i32>, u64)>),
    /// Stations that said they are here, by 24 bit hash and SNR: all the mesh ever learns of them.
    HeardHash(Vec<(u32, i32)>),
    Submit(Queued, u64, u64),
    Pull,
    Receipt(String, bool, String),
    Queue(usize, usize),
}

/// What the server thread reports back.
pub enum BbResult {
    Pulled(Pulled),
    /// Nobody in the mesh hears the destination yet: the relay sends it on the radio itself.
    Unrouted(Queued),
    /// A signed plan the server pushed: epoch, plan text, signature (base64). Checked by the engine.
    Plan(u64, String, String),
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
            let mut ws: Option<Ws> = None;
            let mut stopped = false;
            let mut next_connect = Instant::now();
            let mut backoff = Duration::from_secs(2);
            let mut last_beat = Instant::now();
            // Messages sent over the socket whose answer (routed, unknown, dup) has not come yet.
            let mut awaiting: HashMap<String, Queued> = HashMap::new();
            let push = |out: &Arc<Mutex<Vec<BbResult>>>, r: BbResult| {
                if let Ok(mut o) = out.lock() {
                    o.push(r);
                }
            };
            loop {
                let job = match rx.recv_timeout(Duration::from_millis(1)) {
                    Ok(j) => Some(j),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                };
                // Keep the socket up (the server pushes through it); fall back to HTTPS while it is down.
                if ws.is_none() && !stopped && Instant::now() >= next_connect {
                    match Ws::open(&bb.ws_url()) {
                        Ok(w) => {
                            ws = Some(w);
                            backoff = Duration::from_secs(2);
                            last_beat = Instant::now();
                            push(&out, BbResult::Log("Push link to the network is up.".into()));
                        }
                        Err(_) => {
                            next_connect = Instant::now() + backoff;
                            backoff = (backoff * 2).min(Duration::from_secs(30));
                        }
                    }
                }
                if let Some(w) = ws.as_mut() {
                    if last_beat.elapsed() >= Duration::from_secs(25) {
                        last_beat = Instant::now();
                        let _ = w.send(&json!({ "t": "beat" }));
                    }
                    loop {
                        match w.recv() {
                            Ok(Some(f)) => match f["t"].as_str() {
                                Some("deliver") | Some("receipt") => {
                                    let p = parse_frames(std::slice::from_ref(&f));
                                    if !p.messages.is_empty() || !p.receipts.is_empty() {
                                        push(&out, BbResult::Pulled(p));
                                    }
                                }
                                Some("plan") => push(
                                    &out,
                                    BbResult::Plan(f["epoch"].as_u64().unwrap_or(0), f["plan"].as_str().unwrap_or("").into(), f["sig"].as_str().unwrap_or("").into()),
                                ),
                                Some("unknown") => {
                                    if let Some(q) = f["id"].as_str().and_then(|id| awaiting.remove(id)) {
                                        push(&out, BbResult::Unrouted(q));
                                    }
                                }
                                Some("routed") | Some("dup") => {
                                    if let Some(id) = f["id"].as_str() {
                                        awaiting.remove(id);
                                    }
                                }
                                Some("kicked") => {
                                    stopped = true;
                                    ws = None;
                                    push(&out, BbResult::Log("Another copy of this relay connected to the network; this one stopped using it.".into()));
                                    break;
                                }
                                _ => {}
                            },
                            Ok(None) => break,
                            Err(_) => {
                                ws = None;
                                next_connect = Instant::now() + backoff;
                                // What was waiting for an answer goes on the radio.
                                for (_, q) in awaiting.drain() {
                                    push(&out, BbResult::Unrouted(q));
                                }
                                break;
                            }
                        }
                    }
                }
                let Some(job) = job else { continue };
                // Over the socket when it is up.
                if let Some(w) = ws.as_mut() {
                    let sent = match &job {
                        BbJob::Heard(list) => {
                            let frames: Vec<Value> = list
                                .iter()
                                .take(40)
                                .map(|(c, snr, _)| json!({ "t": "heard", "dest": kahf_core::bundle::h24(c), "snr": snr.unwrap_or(0), "lane": 0 }))
                                .collect();
                            frames.iter().try_for_each(|f| w.send(f)).is_ok()
                        }
                        BbJob::HeardHash(list) => list
                            .iter()
                            .take(40)
                            .try_for_each(|(h, snr)| w.send(&json!({ "t": "heard", "dest": h, "snr": snr, "lane": 0 })))
                            .is_ok(),
                        BbJob::Queue(items, bytes) => w.send(&json!({ "t": "queue", "items": items, "bytes": bytes, "urgency": 0 })).is_ok(),
                        BbJob::Submit(q, _, _) => {
                            let ok = w.send(&submit_frame(q)).is_ok();
                            if ok {
                                awaiting.insert(q.id.clone(), q.clone());
                            }
                            ok
                        }
                        BbJob::Receipt(id, ok, why) => {
                            w.send(&json!({ "t": "receipt", "id": id, "status": if *ok { "delivered" } else { "failed" }, "reason": why })).is_ok()
                        }
                        // The server pushes: nothing to fetch while the socket is up.
                        BbJob::Pull => true,
                    };
                    if sent {
                        continue;
                    }
                    ws = None;
                    next_connect = Instant::now() + backoff;
                }
                match job {
                    BbJob::Heard(list) => report(&out, &mut last_error, bb.heard(&list), "reporting stations"),
                    BbJob::Submit(q, _created, _ttl) => {
                        let mut r = Err(String::new());
                        for attempt in 0..3 {
                            r = bb.submit(&q);
                            if r.is_ok() {
                                break;
                            }
                            if attempt < 2 {
                                std::thread::sleep(Duration::from_secs(5));
                            }
                        }
                        match r {
                            Ok(Submitted::Unrouted) => push(&out, BbResult::Unrouted(q)),
                            Ok(_) => last_error.clear(),
                            // The server cannot be reached: the relay keeps the message for the radio.
                            Err(e) => {
                                report(&out, &mut last_error, Err(e), "handing a message over");
                                push(&out, BbResult::Unrouted(q));
                            }
                        }
                    }
                    BbJob::HeardHash(list) => report(&out, &mut last_error, bb.heard_hashes(&list), "reporting stations"),
                    BbJob::Queue(items, bytes) => report(&out, &mut last_error, bb.queue(items, bytes), "reporting the queue"),
                    BbJob::Pull => match bb.pull() {
                        Ok(p) => {
                            if !p.messages.is_empty() || !p.receipts.is_empty() {
                                push(&out, BbResult::Pulled(p));
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

    /// The signed body of `/register`, as the app makes it (canonical JSON of the fields).
    fn register_body(b: &Backbone, area: &str) -> Value {
        let mut f = Map::new();
        f.insert("area".into(), json!(area));
        f.insert("callsign".into(), json!(b.relay));
        let sig = b.key.sign_raw(canonical(&Value::Object(f.clone())).as_bytes());
        f.insert("public_key".into(), json!(b64_encode(&b.key.to_public())));
        f.insert("signature".into(), json!(b64_encode(&sig)));
        Value::Object(f)
    }

    /// Two relays through a local copy of the real server code (HTTPS path). Needs node; run by hand:
    /// `KAHF_BB_URL=http://127.0.0.1:8787 cargo test -p kahf-daemon backbone_round_trip -- --ignored`
    /// with `node server/tools/serve-local.mjs 8787` running.
    #[test]
    #[ignore]
    fn backbone_round_trip_through_a_local_server() {
        let url = std::env::var("KAHF_BB_URL").unwrap_or_else(|_| "http://127.0.0.1:8787".into());
        let r1 = Backbone::new(&url, "R1AAA", StationKey::generate());
        let r2 = Backbone::new(&url, "R2BBB", StationKey::generate());
        for (r, area) in [(&r1, "N51W1"), (&r2, "N50E8")] {
            r.post_body("/register", &register_body(r, area)).expect("register");
        }
        // Both relays call once so the mesh counts them online; R2 hears Dave, R1 takes Alice's message for him.
        r2.heard(&[("D1AVE".into(), Some(9), now_s())]).expect("heard 2");
        r1.heard(&[("K1ALF".into(), Some(12), now_s())]).expect("heard 1");
        let q = Queued {
            id: message_id("K1ALF", b"hello Dave", now_s()),
            origin: "K1ALF".into(),
            dest: "D1AVE".into(),
            kind: "chat".into(),
            subject: String::new(),
            body: b"hello Dave".to_vec(),
            urgent: false,
        };
        assert_eq!(r1.submit(&q).expect("submit"), Submitted::Routed);
        assert_eq!(r2.submit(&q).expect("submit again"), Submitted::Duplicate);
        let got = r2.pull().expect("pull");
        assert_eq!(got.messages.len(), 1);
        assert_eq!(got.messages[0].body, b"hello Dave");
        assert_eq!(got.messages[0].origin, "K1ALF");
        r2.receipt(&q.id, true, "").expect("receipt");
        let back = r1.pull().expect("pull receipts");
        assert_eq!(back.receipts.len(), 1);
        assert_eq!(back.receipts[0].status, "delivered");
        assert_eq!(back.receipts[0].dest, kahf_core::bundle::h24("D1AVE").to_string(), "the receipt names the destination by hash");
        // A destination nobody hears is reported as unrouted, so the relay can use the radio.
        let lost = Queued { id: "K1ALF-9".into(), dest: "NOBODY1".into(), ..q.clone() };
        assert_eq!(r1.submit(&lost).expect("submit unknown"), Submitted::Unrouted);
    }

    /// The push link over a local copy of the server: a message handed in by one relay reaches the other
    /// within 100 ms (loopback: this shows there is no poll wait, not internet latency). Run by hand:
    /// `KAHF_BB_URL=http://127.0.0.1:8787 cargo test -p kahf-daemon push_link_latency -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn push_link_latency_between_two_relays_on_loopback() {
        let url = std::env::var("KAHF_BB_URL").unwrap_or_else(|_| "http://127.0.0.1:8787".into());
        let mk = |call: &str, area: &str| {
            let b = Backbone::new(&url, call, StationKey::generate());
            b.post_body("/register", &register_body(&b, area)).expect("register");
            b
        };
        let (a, b) = (mk("P1AAA", "N51W1"), mk("P2BBB", "N50E8"));
        let (ha, hb) = (BbHandle::spawn(a), BbHandle::spawn(b));
        // Wait until both sockets are up.
        let mut up = 0;
        for _ in 0..100 {
            for h in [&ha, &hb] {
                up += h.take_results().iter().filter(|r| matches!(r, BbResult::Log(t) if t.contains("Push link"))).count();
            }
            if up >= 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(up, 2, "both push links should be up");
        hb.send(BbJob::Heard(vec![("D1AVE".into(), Some(9), now_s())]));
        std::thread::sleep(Duration::from_millis(300));
        let q = Queued {
            id: message_id("K1ALF", b"hi", now_s()),
            origin: "K1ALF".into(),
            dest: "D1AVE".into(),
            kind: "chat".into(),
            subject: String::new(),
            body: b"hi".to_vec(),
            urgent: false,
        };
        let t0 = Instant::now();
        ha.send(BbJob::Submit(q.clone(), now_s(), 3600));
        let mut got = None;
        let mut ms = 0;
        while t0.elapsed() < Duration::from_secs(2) && got.is_none() {
            for r in hb.take_results() {
                if let BbResult::Pulled(p) = r {
                    got = p.messages.into_iter().next();
                    ms = t0.elapsed().as_millis();
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        eprintln!("relay to relay over loopback: {ms} ms");
        assert_eq!(got.expect("the message did not arrive").body, b"hi");
        assert!(ms < 100, "took {ms} ms");
        // And the receipt comes back to the sender the same way.
        let t1 = Instant::now();
        hb.send(BbJob::Receipt(q.id.clone(), true, String::new()));
        let mut receipt = false;
        while t1.elapsed() < Duration::from_secs(2) && !receipt {
            receipt = ha.take_results().iter().any(|r| matches!(r, BbResult::Pulled(p) if p.receipts.iter().any(|x| x.id == q.id)));
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(receipt, "no receipt came back");
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
    fn a_call_carries_its_key_and_a_signature_over_the_mesh_text() {
        let key = StationKey::generate();
        let public = key.to_public();
        let b = Backbone::new("https://kahf.dev/", "dl0net", key);
        let body = b.signed_body("poll", "", Map::new());
        assert_eq!(body["relay"], "DL0NET");
        assert_eq!(body["public_key"].as_str().unwrap().len(), 44);
        let sig = b64_decode(body["signature"].as_str().unwrap()).unwrap();
        assert_eq!(sig.len(), 64);
        let text = format!("mesh1|DL0NET|{}|poll", body["ts"].as_u64().unwrap());
        assert!(kahf_core::auth::verify_raw(&public, text.as_bytes(), &sig.try_into().unwrap()), "the signature does not verify");
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
    fn pushed_frames_are_read_into_messages_and_receipts() {
        let blob = json!({ "origin": "K1ABC", "dest": "DL1XYZ", "kind": "chat", "subject": "", "body": b64_encode(b"hi"), "urgent": false }).to_string();
        let frames = vec![
            json!({ "t": "deliver", "id": "K1ABC-5", "dest": 1, "size": 2, "from": "R1AAA", "blob": b64_encode(blob.as_bytes()) }),
            json!({ "t": "deliver", "id": "K1ABC-6", "dest": 1, "size": 2, "from": "R1AAA" }),
            json!({ "t": "receipt", "id": "K1ABC-4", "dest": "5", "status": "delivered", "reason": "" }),
            json!({ "t": "plan", "epoch": 5, "plan": "P1|5|1|", "sig": "x" }),
        ];
        let p = parse_frames(&frames);
        assert_eq!(p.messages.len(), 1, "a deliver without its blob is dropped");
        assert_eq!(p.messages[0].body, b"hi");
        assert_eq!(p.messages[0].origin, "K1ABC");
        assert_eq!(p.messages[0].dest, "DL1XYZ");
        assert_eq!(p.receipts[0].status, "delivered");
    }
}
