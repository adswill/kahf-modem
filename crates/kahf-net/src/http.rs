//! HTTPS transport for the relay server.

use crate::client::{Response, Transport};
use std::io::Read;
use std::time::Duration;

/// Largest answer body read, in bytes. The server's answers are far smaller.
const MAX_BODY: u64 = 64 * 1024;

/// Blocking HTTPS transport with connect and read timeouts.
pub struct HttpTransport {
    agent: ureq::Agent,
}

impl HttpTransport {
    pub fn new(timeout: Duration) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(timeout)
            .timeout(timeout)
            .user_agent(concat!("kahf-modem/", env!("CARGO_PKG_VERSION")))
            .build();
        Self { agent }
    }
}

impl Default for HttpTransport {
    fn default() -> Self {
        Self::new(Duration::from_secs(15))
    }
}

impl Transport for HttpTransport {
    fn post_json(&self, url: &str, body: &str) -> Result<Response, String> {
        let answer = self
            .agent
            .post(url)
            .set("Content-Type", "application/json")
            .send_string(body);
        let response = match answer {
            Ok(r) => r,
            Err(ureq::Error::Status(_, r)) => r,
            Err(ureq::Error::Transport(t)) => return Err(t.to_string()),
        };
        let status = response.status();
        let mut text = String::new();
        response
            .into_reader()
            .take(MAX_BODY)
            .read_to_string(&mut text)
            .map_err(|e| format!("reading the answer failed: {e}"))?;
        Ok(Response { status, body: text })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use std::thread;

    /// One-shot local HTTP server that answers with `status` and `body`.
    fn serve_once(status: &'static str, body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let reply = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            s.write_all(reply.as_bytes()).unwrap();
        });
        format!("http://{addr}")
    }

    #[test]
    fn error_statuses_come_back_as_responses() {
        let url = serve_once("409 Conflict", r#"{"error":"taken"}"#);
        let r = HttpTransport::default()
            .post_json(&format!("{url}/register"), "{}")
            .unwrap();
        assert_eq!(
            r,
            Response {
                status: 409,
                body: r#"{"error":"taken"}"#.into()
            }
        );
    }

    #[test]
    fn no_server_is_an_offline_error() {
        // Bind and drop to get a port with nothing listening.
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let t = HttpTransport::new(Duration::from_secs(2));
        assert!(t
            .post_json(&format!("http://127.0.0.1:{port}/register"), "{}")
            .is_err());
    }
}
