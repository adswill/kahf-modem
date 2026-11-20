//! flrig over XML-RPC (default 127.0.0.1:12345). flrig then owns the serial port and the
//! radio model, so this covers every radio flrig supports.
use crate::{Mode, PttMethod, Rig};
use kahf_core::io::RigControl;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

pub const DEFAULT_ADDR: &str = "127.0.0.1:12345";

pub struct FlrigClient {
    addr: String,
    ptt: PttMethod,
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The value inside the first `<value>` of a methodResponse, or the fault text.
pub fn parse_response(body: &str) -> Result<String, String> {
    if body.contains("<fault>") {
        let msg = between(body, "<string>", "</string>").unwrap_or("fault");
        return Err(format!("flrig: {msg}"));
    }
    let v = between(body, "<value>", "</value>").ok_or("flrig sent no value")?;
    // Strip a type tag such as <double>..</double>.
    let inner = match v.find('>') {
        Some(i) if v.starts_with('<') => {
            let rest = &v[i + 1..];
            rest.rfind("</").map_or(rest, |j| &rest[..j])
        }
        _ => v,
    };
    Ok(inner.trim().to_string())
}

fn between<'a>(s: &'a str, a: &str, b: &str) -> Option<&'a str> {
    let i = s.find(a)? + a.len();
    let j = s[i..].find(b)? + i;
    Some(&s[i..j])
}

impl FlrigClient {
    pub fn new(addr: &str, ptt: PttMethod) -> Self {
        FlrigClient {
            addr: addr.into(),
            ptt,
        }
    }

    /// Connects once to prove flrig is there, and returns the radio's name.
    pub fn connect(addr: &str, ptt: PttMethod) -> Result<Self, String> {
        let c = Self::new(addr, ptt);
        c.call("rig.get_xcvr", "")?;
        Ok(c)
    }

    fn call(&self, method: &str, params: &str) -> Result<String, String> {
        let body = format!(
            "<?xml version=\"1.0\"?><methodCall><methodName>{method}</methodName><params>{params}</params></methodCall>"
        );
        let sock = self
            .addr
            .to_socket_addrs()
            .map_err(|e| format!("flrig address {}: {e}", self.addr))?
            .next()
            .ok_or("flrig address did not resolve")?;
        let mut s = TcpStream::connect_timeout(&sock, Duration::from_secs(2)).map_err(|e| {
            format!(
                "flrig connect {}: {e} (is flrig running with its XML-RPC server on?)",
                self.addr
            )
        })?;
        s.set_read_timeout(Some(Duration::from_secs(3))).ok();
        s.set_write_timeout(Some(Duration::from_secs(2))).ok();
        let req = format!(
            "POST /RPC2 HTTP/1.1\r\nHost: {}\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.addr,
            body.len()
        );
        s.write_all(req.as_bytes())
            .map_err(|e| format!("flrig write: {e}"))?;
        let mut resp = String::new();
        s.read_to_string(&mut resp)
            .map_err(|e| format!("flrig read: {e}"))?;
        let (head, payload) = resp.split_once("\r\n\r\n").ok_or("flrig sent a bad reply")?;
        if !head.starts_with("HTTP/1.1 200") && !head.starts_with("HTTP/1.0 200") {
            return Err(format!("flrig HTTP error: {}", head.lines().next().unwrap_or("")));
        }
        parse_response(payload)
    }
}

impl RigControl for FlrigClient {
    fn set_ptt(&mut self, on: bool) -> Result<(), String> {
        if self.ptt != PttMethod::Cat {
            return Err("flrig keys through CAT only; use ptt=cat, or let the rig use VOX".into());
        }
        self.call(
            "rig.set_ptt",
            &format!("<param><value><i4>{}</i4></value></param>", on as i32),
        )
        .map(|_| ())
    }

    fn set_freq_hz(&mut self, hz: u64) -> Result<(), String> {
        self.call(
            "rig.set_vfo",
            &format!("<param><value><double>{hz}.0</double></value></param>"),
        )
        .map(|_| ())
    }
}

impl Rig for FlrigClient {
    fn name(&self) -> String {
        self.call("rig.get_xcvr", "").unwrap_or_else(|_| "flrig".into())
    }

    fn freq_hz(&mut self) -> Result<u64, String> {
        let v = self.call("rig.get_vfo", "")?;
        v.parse::<f64>()
            .map(|f| f as u64)
            .map_err(|_| format!("bad flrig frequency {v:?}"))
    }

    fn set_mode(&mut self, mode: Mode) -> Result<(), String> {
        // flrig names data modes differently per radio; try the common names in turn.
        let names: &[&str] = match mode {
            Mode::Usb => &["USB"],
            Mode::UsbData => &["USB-D", "DATA-U", "PKT-U", "USB"],
            Mode::Lsb => &["LSB"],
            Mode::LsbData => &["LSB-D", "DATA-L", "PKT-L", "LSB"],
        };
        let mut last = String::new();
        for n in names {
            match self.call(
                "rig.set_mode",
                &format!("<param><value><string>{}</string></value></param>", xml_escape(n)),
            ) {
                Ok(_) => return Ok(()),
                Err(e) => last = e,
            }
        }
        Err(last)
    }
}
