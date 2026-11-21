//! Local scripting API: one JSON object per line over TCP, loopback only.
//!
//! ```text
//! {"cmd":"status"}
//! {"cmd":"send","to":"OZ1ABC","text":"hello"}
//! {"cmd":"events","since":0}
//! {"cmd":"qsy","slot":1}
//! ```
//! Each request gets one JSON line back: `{"ok":true,...}` or `{"ok":false,"error":"..."}`.
//! The station itself stays on the main thread; connections hand requests over a channel.
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Status,
    Send { to: String, text: String },
    Events { since: u64 },
    Qsy { slot: usize },
}

pub fn parse_request(line: &str) -> Result<Request, String> {
    let v: Value = serde_json::from_str(line).map_err(|e| format!("not JSON: {e}"))?;
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    match v.get("cmd").and_then(Value::as_str) {
        Some("status") => Ok(Request::Status),
        Some("send") => Ok(Request::Send {
            to: s("to").ok_or("send needs \"to\"")?,
            text: s("text").ok_or("send needs \"text\"")?,
        }),
        Some("events") => Ok(Request::Events {
            since: v.get("since").and_then(Value::as_u64).unwrap_or(0),
        }),
        Some("qsy") => Ok(Request::Qsy {
            slot: v
                .get("slot")
                .and_then(Value::as_u64)
                .ok_or("qsy needs a numeric \"slot\"")? as usize,
        }),
        Some(o) => Err(format!("unknown cmd {o:?}")),
        None => Err("missing \"cmd\"".into()),
    }
}

/// What the main thread offers to the API.
pub trait Station {
    fn status(&mut self) -> Value;
    fn send(&mut self, to: &str, text: &str) -> Result<(), String>;
    fn events(&mut self, since: u64) -> Value;
    fn qsy(&mut self, slot: usize) -> Result<u64, String>;
}

/// Runs one request against the station and builds the reply line.
pub fn handle(st: &mut dyn Station, req: Request) -> Value {
    let r: Result<Value, String> = match req {
        Request::Status => Ok(st.status()),
        Request::Send { to, text } => st.send(&to, &text).map(|_| json!({})),
        Request::Events { since } => Ok(st.events(since)),
        Request::Qsy { slot } => st.qsy(slot).map(|d| json!({ "dial_hz": d })),
    };
    match r {
        Ok(mut v) => {
            if let Some(o) = v.as_object_mut() {
                o.insert("ok".into(), json!(true));
            }
            v
        }
        Err(e) => json!({ "ok": false, "error": e }),
    }
}

pub type Job = (Request, Sender<Value>);

/// Starts the listener. Returns the receiving end the main loop drains, and the bound address.
/// Refuses anything but a loopback address: the API can transmit.
pub fn serve(addr: &str) -> Result<(Receiver<Job>, SocketAddr), String> {
    let sa: SocketAddr = addr
        .parse()
        .map_err(|_| format!("bad API address {addr:?} (want 127.0.0.1:port)"))?;
    if !sa.ip().is_loopback() {
        return Err("the API only listens on loopback (127.0.0.1 or ::1)".into());
    }
    let l = TcpListener::bind(sa).map_err(|e| format!("API bind {sa}: {e}"))?;
    let bound = l.local_addr().map_err(|e| e.to_string())?;
    let (tx, rx) = mpsc::channel::<Job>();
    thread::spawn(move || {
        for conn in l.incoming().flatten() {
            let tx = tx.clone();
            thread::spawn(move || {
                let mut w = match conn.try_clone() {
                    Ok(w) => w,
                    Err(_) => return,
                };
                for line in BufReader::new(conn).lines().map_while(Result::ok) {
                    if line.trim().is_empty() {
                        continue;
                    }
                    let reply = match parse_request(&line) {
                        Ok(req) => {
                            let (rtx, rrx) = mpsc::channel();
                            if tx.send((req, rtx)).is_err() {
                                return;
                            }
                            rrx.recv()
                                .unwrap_or_else(|_| json!({"ok": false, "error": "node is stopping"}))
                        }
                        Err(e) => json!({ "ok": false, "error": e }),
                    };
                    if writeln!(w, "{reply}").is_err() {
                        return;
                    }
                }
            });
        }
    });
    Ok((rx, bound))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpStream;
    use std::time::Duration;

    #[derive(Default)]
    struct Fake {
        sent: Vec<(String, String)>,
    }

    impl Station for Fake {
        fn status(&mut self) -> Value {
            json!({"call": "N0CALL", "relay": false})
        }
        fn send(&mut self, to: &str, text: &str) -> Result<(), String> {
            if to == "BAD" {
                return Err("not a callsign".into());
            }
            self.sent.push((to.into(), text.into()));
            Ok(())
        }
        fn events(&mut self, since: u64) -> Value {
            json!({"next": since + 1, "events": []})
        }
        fn qsy(&mut self, slot: usize) -> Result<u64, String> {
            if slot > 2 {
                Err("no such slot".into())
            } else {
                Ok(18_113_550)
            }
        }
    }

    #[test]
    fn parsing() {
        assert_eq!(parse_request(r#"{"cmd":"status"}"#), Ok(Request::Status));
        assert_eq!(
            parse_request(r#"{"cmd":"send","to":"A","text":"b"}"#),
            Ok(Request::Send {
                to: "A".into(),
                text: "b".into()
            })
        );
        assert!(parse_request(r#"{"cmd":"send","to":"A"}"#).is_err());
        assert!(parse_request(r#"{"cmd":"nope"}"#).is_err());
        assert!(parse_request("garbage").is_err());
    }

    #[test]
    fn handle_wraps_results() {
        let mut f = Fake::default();
        assert_eq!(handle(&mut f, Request::Status)["ok"], true);
        let r = handle(
            &mut f,
            Request::Send {
                to: "BAD".into(),
                text: "x".into(),
            },
        );
        assert_eq!(
            (r["ok"].clone(), r["error"].clone()),
            (json!(false), json!("not a callsign"))
        );
        assert_eq!(handle(&mut f, Request::Qsy { slot: 1 })["dial_hz"], 18_113_550);
        assert_eq!(handle(&mut f, Request::Qsy { slot: 9 })["ok"], false);
    }

    #[test]
    fn refuses_non_loopback() {
        assert!(serve("0.0.0.0:0").is_err());
        assert!(serve("192.168.1.5:7373").is_err());
    }

    #[test]
    fn end_to_end_over_tcp() {
        let (rx, addr) = serve("127.0.0.1:0").unwrap();
        let t = thread::spawn(move || {
            let mut f = Fake::default();
            for (req, reply) in rx.iter().take(3) {
                reply.send(handle(&mut f, req)).unwrap();
            }
            f.sent
        });
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        s.write_all(b"{\"cmd\":\"status\"}\n{\"cmd\":\"send\",\"to\":\"OZ1ABC\",\"text\":\"hi\"}\nbroken\n{\"cmd\":\"events\"}\n").unwrap();
        let mut got = String::new();
        let mut buf = [0u8; 1024];
        while !(got.ends_with('\n') && got.lines().count() == 4) {
            let n = s.read(&mut buf).unwrap();
            got.push_str(&String::from_utf8_lossy(&buf[..n]));
        }
        let lines: Vec<Value> = got.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines[0]["call"], "N0CALL");
        assert_eq!(lines[1]["ok"], true);
        assert_eq!(lines[2]["ok"], false); // broken line answered without reaching the station
        assert_eq!(lines[3]["next"], 1);
        assert_eq!(t.join().unwrap(), vec![("OZ1ABC".to_string(), "hi".to_string())]);
    }
}
