//! Sharing with the public map at kahf.dev.
//!
//! Today the map lists relays, each by its one-degree area. A relay registers with a request
//! signed by the station key. Ordinary stations are not listed: the server's station report
//! needs the position of the station that was heard, which a receiver cannot know yet.

use std::io::Write;
use std::process::{Command, Stdio};

use kahf_core::auth::StationKey;
use serde_json::{json, Map, Value};

pub const SERVER: &str = "https://kahf.dev";

/// The one-degree square a position is in, such as "N51W1". Same rule as the server's `areaKey`.
pub fn area_key(lat: f64, lon: f64) -> String {
    let la = lat.floor() as i32;
    let lo = lon.floor() as i32;
    format!("{}{}{}{}", if la >= 0 { "N" } else { "S" }, la.abs(), if lo >= 0 { "E" } else { "W" }, lo.abs())
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

fn b64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// The signed body of POST /register for a relay in `area`.
pub fn register_body(key: &StationKey, callsign: &str, area: &str) -> Value {
    let mut fields = Map::new();
    fields.insert("area".into(), json!(area));
    fields.insert("callsign".into(), json!(callsign));
    let sig = key.sign_raw(canonical(&Value::Object(fields.clone())).as_bytes());
    fields.insert("public_key".into(), json!(b64(&key.to_public())));
    fields.insert("signature".into(), json!(b64(&sig)));
    Value::Object(fields)
}

/// POSTs JSON with the system's `curl` (every supported system has one), with a time limit.
/// Returns the HTTP status and the body.
pub fn post_json(url: &str, body: &Value) -> Result<(u16, String), String> {
    let mut child = Command::new("curl")
        .args(["-sS", "-m", "20", "-X", "POST", "-H", "content-type: application/json", "--data-binary", "@-", "-w", "\n%{http_code}", url])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Cannot reach the internet: curl did not start ({e})"))?;
    child
        .stdin
        .take()
        .ok_or("curl has no input")?
        .write_all(body.to_string().as_bytes())
        .map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr);
        return Err(format!("No connection to kahf.dev: {}", why.trim()));
    }
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let (body, code) = text.rsplit_once('\n').unwrap_or(("", &text));
    Ok((code.trim().parse().unwrap_or(0), body.to_string()))
}

/// Registers this station as a relay on the map. Plain words in the result either way.
pub fn register_relay(key: &StationKey, callsign: &str, lat: f64, lon: f64) -> Result<String, String> {
    let area = area_key(lat, lon);
    let body = register_body(key, callsign, &area);
    match post_json(&format!("{SERVER}/register"), &body)? {
        (200, _) => Ok(format!("Your relay is on the map, in the square {area}.")),
        (409, _) => Err("That callsign is already registered with a different key on the map.".into()),
        (code, text) => Err(format!("The map refused the registration ({code}): {}", text.trim())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn area_keys_follow_the_servers_rule() {
        assert_eq!(area_key(51.52, -0.125), "N51W1");
        assert_eq!(area_key(51.52, 0.4), "N51E0");
        assert_eq!(area_key(-33.87, 151.2), "S34E151");
        assert_eq!(area_key(0.3, -0.3), "N0W1");
        assert_eq!(area_key(40.7, -74.0), "N40W74");
    }

    #[test]
    fn canonical_json_matches_what_javascript_prints() {
        let v = json!({ "b": [1, 2.5, "x"], "a": { "z": null, "y": true }, "n": 14100.0, "m": 0.0, "k": 51.521 });
        assert_eq!(canonical(&v), r#"{"a":{"y":true,"z":null},"b":[1,2.5,"x"],"k":51.521,"m":0,"n":14100}"#);
        assert_eq!(canonical(&json!("a\"b")), r#""a\"b""#);
    }

    #[test]
    fn base64_matches_the_standard_alphabet() {
        assert_eq!(b64(b""), "");
        assert_eq!(b64(b"f"), "Zg==");
        assert_eq!(b64(b"fo"), "Zm8=");
        assert_eq!(b64(b"foo"), "Zm9v");
        assert_eq!(b64(&[0xfb, 0xff]), "+/8=");
    }

    #[test]
    fn registration_body_has_the_four_fields_and_a_valid_signature() {
        let key = StationKey::generate();
        let b = register_body(&key, "G4ABC", "N51W1");
        let o = b.as_object().unwrap();
        let mut keys: Vec<&str> = o.keys().map(|k| k.as_str()).collect();
        keys.sort();
        assert_eq!(keys, ["area", "callsign", "public_key", "signature"]);
        assert_eq!(o["public_key"].as_str().unwrap().len(), 44);
        assert_eq!(o["signature"].as_str().unwrap().len(), 88);
    }

    /// Prints a signed registration for the server's own code to check (see the app README).
    #[test]
    #[ignore]
    fn print_a_signed_registration() {
        let key = StationKey::generate();
        println!("BODY={}", register_body(&key, "G4ABC", &area_key(51.52, -0.125)));
    }
}
