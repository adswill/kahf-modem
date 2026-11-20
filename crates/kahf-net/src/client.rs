//! One signed request to the relay server and its answer.

use crate::wire::{canonical_json, Area, ChannelGrid};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use kahf_core::auth::StationKey;
use kahf_core::relay::{Direction, Slot};
use serde_json::{json, Map, Value};
use std::fmt;

/// Most slot requests the server takes in one call (`MAX_SCHEDULE_REQUESTS`).
pub const MAX_SCHEDULE_REQUESTS: usize = 20;

/// Largest latency budget the server takes, in seconds (`MAX_LATENCY_BUDGET_S`).
pub const MAX_LATENCY_BUDGET_S: u32 = 3600;

/// An HTTP answer: status code and body text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

/// Sends a JSON body with POST. An `Err` means the server was not reached
/// (no network, DNS, timeout); any HTTP status, 4xx and 5xx included, is an `Ok`.
pub trait Transport {
    fn post_json(&self, url: &str, body: &str) -> Result<Response, String>;
}

/// Why a request failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The server could not be reached. Worth retrying.
    Offline(String),
    /// The server answered with an error status and its `error` text.
    Server { status: u16, message: String },
    /// The server answered 200 with a body this client does not understand.
    BadResponse(String),
    /// The request was refused before it was sent.
    BadInput(String),
}

impl Error {
    /// True when the same request may succeed later without any change:
    /// the server was offline, overloaded or failed.
    pub fn is_retryable(&self) -> bool {
        match self {
            Error::Offline(_) => true,
            Error::Server { status, .. } => *status >= 500 || *status == 408 || *status == 429,
            Error::BadResponse(_) | Error::BadInput(_) => false,
        }
    }

    /// True when the server does not know this relay (or its registration expired).
    pub fn is_not_registered(&self) -> bool {
        matches!(self, Error::Server { status: 404, .. })
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Offline(why) => write!(f, "relay server not reachable: {why}"),
            Error::Server { status, message } => write!(f, "relay server refused ({status}): {message}"),
            Error::BadResponse(why) => write!(
                f,
                "relay server sent an answer this app does not understand: {why}"
            ),
            Error::BadInput(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for Error {}

/// A successful registration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registration {
    pub callsign: String,
    pub area: String,
    /// The registration lapses after this many seconds unless renewed.
    pub expires_after_s: u64,
}

/// One slot to ask for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlotRequest {
    /// Centre frequency in kHz; must be on the client's channel grid.
    pub channel_khz: f32,
    pub duration_s: u32,
    pub direction: Direction,
    /// Planned before the other requests.
    pub urgent: bool,
    /// Latest acceptable start, in seconds from the server's time.
    pub latency_budget_s: Option<u32>,
}

/// A request the server could not place.
#[derive(Clone, Debug, PartialEq)]
pub struct Unplaced {
    /// Position of the request in the list that was sent.
    pub index: usize,
    pub reason: String,
}

/// The server's plan for this relay.
#[derive(Clone, Debug, PartialEq)]
pub struct Schedule {
    pub relay: String,
    /// Sorted by start time.
    pub slots: Vec<Slot>,
    pub unplaced: Vec<Unplaced>,
}

impl Schedule {
    /// First second after the last slot, or None when there are no slots.
    pub fn end_unix(&self) -> Option<u64> {
        self.slots.iter().map(Slot::end_unix).max()
    }
}

/// Client for one relay server.
pub struct Client<T> {
    transport: T,
    base_url: String,
    grid: ChannelGrid,
}

impl<T: Transport> Client<T> {
    /// `base_url` is the server root, such as [`crate::DEFAULT_BASE_URL`].
    pub fn new(transport: T, base_url: &str) -> Self {
        Self {
            transport,
            base_url: base_url.trim_end_matches('/').to_string(),
            grid: ChannelGrid::RELAY_17M,
        }
    }

    /// Use another channel grid. It must match the server's.
    pub fn with_grid(mut self, grid: ChannelGrid) -> Self {
        self.grid = grid;
        self
    }

    pub fn grid(&self) -> ChannelGrid {
        self.grid
    }

    /// Register (or renew) this relay: `POST /register`.
    pub fn register(&self, key: &StationKey, callsign: &str, area: &Area) -> Result<Registration, Error> {
        let fields = json!({ "area": area.as_str(), "callsign": callsign });
        let v = self.post_signed("/register", key, fields)?;
        Ok(Registration {
            callsign: str_field(&v, "callsign")?.to_string(),
            area: str_field(&v, "area")?.to_string(),
            expires_after_s: v
                .get("expires_after_s")
                .and_then(Value::as_u64)
                .ok_or_else(|| bad("expires_after_s is missing"))?,
        })
    }

    /// Ask for slots: `POST /schedule`. The server replaces this relay's earlier plan.
    pub fn schedule(
        &self,
        key: &StationKey,
        relay: &str,
        requests: &[SlotRequest],
    ) -> Result<Schedule, Error> {
        if requests.is_empty() || requests.len() > MAX_SCHEDULE_REQUESTS {
            return Err(Error::BadInput(format!(
                "a schedule call needs 1 to {MAX_SCHEDULE_REQUESTS} slot requests, not {}",
                requests.len()
            )));
        }
        let wire = requests
            .iter()
            .map(|q| self.request_json(q))
            .collect::<Result<Vec<_>, _>>()?;
        let v = self.post_signed("/schedule", key, json!({ "relay": relay, "requests": wire }))?;

        let mut slots = v
            .get("slots")
            .and_then(Value::as_array)
            .ok_or_else(|| bad("slots is missing"))?
            .iter()
            .map(|s| self.slot_from_json(s))
            .collect::<Result<Vec<_>, _>>()?;
        slots.sort_by_key(|s| s.start_unix);
        let unplaced = match v.get("unplaced").and_then(Value::as_array) {
            Some(items) => items.iter().map(unplaced_from_json).collect::<Result<_, _>>()?,
            None => Vec::new(),
        };
        Ok(Schedule {
            relay: str_field(&v, "relay")?.to_string(),
            slots,
            unplaced,
        })
    }

    fn request_json(&self, q: &SlotRequest) -> Result<Value, Error> {
        let channel = self.grid.channel_of(q.channel_khz).ok_or_else(|| {
            Error::BadInput(format!(
                "{:.2} kHz is not a relay channel on the server's grid",
                q.channel_khz
            ))
        })?;
        if q.duration_s == 0 {
            return Err(Error::BadInput("a slot must last at least one second".into()));
        }
        let mut m = Map::new();
        m.insert("channel".into(), channel.into());
        m.insert("direction".into(), direction_str(q.direction).into());
        m.insert("duration".into(), q.duration_s.into());
        if q.urgent {
            m.insert("urgent".into(), true.into());
        }
        if let Some(budget) = q.latency_budget_s {
            if budget > MAX_LATENCY_BUDGET_S {
                return Err(Error::BadInput(format!(
                    "latency budget {budget} s is over the server's limit of {MAX_LATENCY_BUDGET_S} s"
                )));
            }
            m.insert("latency_budget_s".into(), budget.into());
        }
        Ok(Value::Object(m))
    }

    fn slot_from_json(&self, s: &Value) -> Result<Slot, Error> {
        let channel = s
            .get("channel")
            .and_then(Value::as_u64)
            .ok_or_else(|| bad("slot channel"))?;
        let channel_khz = u32::try_from(channel)
            .ok()
            .and_then(|c| self.grid.centre_khz(c))
            .ok_or_else(|| bad(&format!("slot channel {channel} is not on this client's grid")))?;
        let duration = s
            .get("duration")
            .and_then(Value::as_u64)
            .ok_or_else(|| bad("slot duration"))?;
        Ok(Slot {
            channel_khz,
            start_unix: s
                .get("start")
                .and_then(Value::as_u64)
                .ok_or_else(|| bad("slot start"))?,
            duration_s: u32::try_from(duration).map_err(|_| bad("slot duration"))?,
            direction: parse_direction(s.get("direction"))?,
        })
    }

    /// Sign `fields` (canonical JSON, no domain prefix), add the key and the
    /// signature, send, and return the parsed 200 body.
    fn post_signed(&self, path: &str, key: &StationKey, fields: Value) -> Result<Value, Error> {
        let signature = key.sign_raw(canonical_json(&fields).as_bytes());
        let Value::Object(mut body) = fields else {
            unreachable!("signed fields are always an object")
        };
        body.insert("public_key".into(), BASE64.encode(key.to_public()).into());
        body.insert("signature".into(), BASE64.encode(signature).into());
        let body = canonical_json(&Value::Object(body));

        let url = format!("{}{path}", self.base_url);
        let r = self.transport.post_json(&url, &body).map_err(Error::Offline)?;
        let parsed: Option<Value> = serde_json::from_str(&r.body).ok();
        if r.status != 200 {
            let message = parsed
                .as_ref()
                .and_then(|v| v.get("error"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| r.body.chars().take(200).collect());
            return Err(Error::Server {
                status: r.status,
                message,
            });
        }
        parsed.ok_or_else(|| bad("body is not JSON"))
    }
}

fn direction_str(d: Direction) -> &'static str {
    match d {
        Direction::Receive => "rx",
        Direction::Transmit => "tx",
    }
}

fn parse_direction(v: Option<&Value>) -> Result<Direction, Error> {
    match v.and_then(Value::as_str) {
        Some("rx") => Ok(Direction::Receive),
        Some("tx") => Ok(Direction::Transmit),
        _ => Err(bad("slot direction")),
    }
}

fn unplaced_from_json(u: &Value) -> Result<Unplaced, Error> {
    Ok(Unplaced {
        index: u
            .get("index")
            .and_then(Value::as_u64)
            .ok_or_else(|| bad("unplaced index"))? as usize,
        reason: str_field(u, "reason")?.to_string(),
    })
}

fn str_field<'a>(v: &'a Value, key: &str) -> Result<&'a str, Error> {
    v.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| bad(&format!("{key} is missing")))
}

fn bad(why: &str) -> Error {
    Error::BadResponse(why.to_string())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    /// Answers from a queue and records every request.
    #[derive(Default)]
    pub(crate) struct FakeTransport {
        pub answers: RefCell<VecDeque<Result<Response, String>>>,
        pub sent: RefCell<Vec<(String, String)>>,
    }

    impl FakeTransport {
        pub fn answer(&self, status: u16, body: Value) {
            self.answers.borrow_mut().push_back(Ok(Response {
                status,
                body: body.to_string(),
            }));
        }

        pub fn offline(&self) {
            self.answers
                .borrow_mut()
                .push_back(Err("connection refused".into()));
        }
    }

    impl Transport for &FakeTransport {
        fn post_json(&self, url: &str, body: &str) -> Result<Response, String> {
            self.sent.borrow_mut().push((url.to_string(), body.to_string()));
            self.answers.borrow_mut().pop_front().expect("no answer queued")
        }
    }

    /// Plain Ed25519 check over canonical JSON, as the server does it.
    pub(crate) fn verify(body: &str) -> bool {
        use ed25519_dalek::{Signature, Verifier, VerifyingKey};
        let mut v: Map<String, Value> = serde_json::from_str(body).unwrap();
        let key = BASE64
            .decode(v.remove("public_key").unwrap().as_str().unwrap())
            .unwrap();
        let sig = BASE64
            .decode(v.remove("signature").unwrap().as_str().unwrap())
            .unwrap();
        let key = VerifyingKey::from_bytes(&key.try_into().unwrap()).unwrap();
        let sig = Signature::from_bytes(&sig.try_into().unwrap());
        key.verify(canonical_json(&Value::Object(v)).as_bytes(), &sig)
            .is_ok()
    }

    pub(crate) fn key() -> StationKey {
        StationKey::from_bytes(&[7; 32])
    }

    fn area() -> Area {
        Area::parse("N54E25").unwrap()
    }

    fn ch(n: u32) -> f32 {
        ChannelGrid::RELAY_17M.centre_khz(n).unwrap()
    }

    #[test]
    fn register_sends_a_signed_body_and_reads_the_answer() {
        let t = FakeTransport::default();
        t.answer(
            200,
            json!({"ok": true, "callsign": "LY1ABC", "area": "N54E25", "expires_after_s": 172800}),
        );
        let c = Client::new(&t, "https://kahf.dev/");
        let r = c.register(&key(), "LY1ABC", &area()).unwrap();
        assert_eq!(
            r,
            Registration {
                callsign: "LY1ABC".into(),
                area: "N54E25".into(),
                expires_after_s: 172_800
            }
        );

        let (url, body) = t.sent.borrow()[0].clone();
        assert_eq!(url, "https://kahf.dev/register");
        let v: Value = serde_json::from_str(&body).unwrap();
        let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            ["area", "callsign", "public_key", "signature"],
            "exactly the fields the server allows"
        );
        assert!(verify(&body), "signature must check out over canonical JSON");
    }

    /// The same body the server's test suite verifies (server/test/client-fixture.test.mjs).
    #[test]
    fn register_body_matches_the_shared_fixture() {
        let t = FakeTransport::default();
        t.answer(
            200,
            json!({"ok": true, "callsign": "LY1ABC", "area": "N54E25", "expires_after_s": 172800}),
        );
        Client::new(&t, "https://kahf.dev")
            .register(&key(), "LY1ABC", &area())
            .unwrap();
        let fixture: Value =
            serde_json::from_str(include_str!("../../../server/test/fixtures/rust-client.json")).unwrap();
        assert_eq!(t.sent.borrow()[0].1, fixture["register"].as_str().unwrap());

        t.answer(200, json!({"relay": "LY1ABC", "slots": [], "unplaced": []}));
        let req = SlotRequest {
            channel_khz: ch(1),
            duration_s: 60,
            direction: Direction::Transmit,
            urgent: true,
            latency_budget_s: Some(600),
        };
        Client::new(&t, "https://kahf.dev")
            .schedule(&key(), "LY1ABC", &[req])
            .unwrap();
        assert_eq!(t.sent.borrow()[1].1, fixture["schedule"].as_str().unwrap());
    }

    #[test]
    fn schedule_maps_channels_and_reads_slots() {
        let t = FakeTransport::default();
        t.answer(
            200,
            json!({
                "relay": "LY1ABC",
                "generated_at": "2026-11-20T12:00:00.000Z",
                "slots": [
                    {"channel": 2, "start": 1_700_000_060u64, "duration": 60, "direction": "tx"},
                    {"channel": 0, "start": 1_700_000_000u64, "duration": 60, "direction": "rx"}
                ],
                "unplaced": [{"index": 2, "relay": "LY1ABC", "channel": 1, "direction": "rx", "reason": "no free time inside the latency budget"}]
            }),
        );
        let c = Client::new(&t, "https://kahf.dev");
        let reqs = [
            SlotRequest {
                channel_khz: ch(0),
                duration_s: 60,
                direction: Direction::Receive,
                urgent: false,
                latency_budget_s: None,
            },
            SlotRequest {
                channel_khz: ch(2),
                duration_s: 60,
                direction: Direction::Transmit,
                urgent: true,
                latency_budget_s: Some(600),
            },
            SlotRequest {
                channel_khz: ch(1),
                duration_s: 30,
                direction: Direction::Receive,
                urgent: false,
                latency_budget_s: Some(0),
            },
        ];
        let s = c.schedule(&key(), "LY1ABC", &reqs).unwrap();

        assert_eq!(s.slots.len(), 2);
        assert_eq!(s.slots[0].start_unix, 1_700_000_000, "sorted by start");
        assert_eq!(s.slots[0].direction, Direction::Receive);
        assert!((s.slots[0].channel_khz - ch(0)).abs() < 1e-3);
        assert!((s.slots[1].channel_khz - ch(2)).abs() < 1e-3);
        assert_eq!(s.end_unix(), Some(1_700_000_120));
        assert_eq!(
            s.unplaced,
            vec![Unplaced {
                index: 2,
                reason: "no free time inside the latency budget".into()
            }]
        );

        let body = t.sent.borrow()[0].1.clone();
        assert!(verify(&body));
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            v["requests"],
            json!([
                {"channel": 0, "direction": "rx", "duration": 60},
                {"channel": 2, "direction": "tx", "duration": 60, "urgent": true, "latency_budget_s": 600},
                {"channel": 1, "direction": "rx", "duration": 30, "latency_budget_s": 0}
            ])
        );
    }

    #[test]
    fn schedule_refuses_bad_requests_before_sending() {
        let t = FakeTransport::default();
        let c = Client::new(&t, "https://kahf.dev");
        let ok = SlotRequest {
            channel_khz: ch(0),
            duration_s: 60,
            direction: Direction::Receive,
            urgent: false,
            latency_budget_s: None,
        };
        let off_grid = SlotRequest {
            channel_khz: 14_074.0,
            ..ok
        };
        let zero = SlotRequest { duration_s: 0, ..ok };
        let budget = SlotRequest {
            latency_budget_s: Some(3601),
            ..ok
        };
        for reqs in [vec![], vec![ok; 21], vec![off_grid], vec![zero], vec![budget]] {
            assert!(matches!(
                c.schedule(&key(), "LY1ABC", &reqs),
                Err(Error::BadInput(_))
            ));
        }
        assert!(t.sent.borrow().is_empty());
    }

    #[test]
    fn errors_are_classified_for_retry() {
        let t = FakeTransport::default();
        let c = Client::new(&t, "https://kahf.dev");
        t.offline();
        t.answer(503, json!({"error": "overloaded"}));
        t.answer(409, json!({"error": "callsign is held by another key"}));
        t.answer(502, Value::String("<html>bad gateway</html>".into()));
        t.answer(200, Value::String("not an object".into()));

        let e = c.register(&key(), "LY1ABC", &area()).unwrap_err();
        assert!(matches!(e, Error::Offline(_)) && e.is_retryable());
        let e = c.register(&key(), "LY1ABC", &area()).unwrap_err();
        assert_eq!(
            e,
            Error::Server {
                status: 503,
                message: "overloaded".into()
            }
        );
        assert!(e.is_retryable());
        let e = c.register(&key(), "LY1ABC", &area()).unwrap_err();
        assert!(!e.is_retryable());
        assert!(e.to_string().contains("held by another key"));
        let e = c.register(&key(), "LY1ABC", &area()).unwrap_err();
        assert!(matches!(&e, Error::Server { status: 502, message } if message.contains("bad gateway")));
        let e = c.register(&key(), "LY1ABC", &area()).unwrap_err();
        assert!(matches!(e, Error::BadResponse(_)) && !e.is_retryable());
    }

    #[test]
    fn schedule_rejects_slots_off_the_grid() {
        let t = FakeTransport::default();
        t.answer(200, json!({"relay": "LY1ABC", "slots": [{"channel": 9, "start": 1, "duration": 60, "direction": "rx"}]}));
        let c = Client::new(&t, "https://kahf.dev");
        let ok = SlotRequest {
            channel_khz: ch(0),
            duration_s: 60,
            direction: Direction::Receive,
            urgent: false,
            latency_budget_s: None,
        };
        assert!(matches!(
            c.schedule(&key(), "LY1ABC", &[ok]),
            Err(Error::BadResponse(_))
        ));
    }
}
