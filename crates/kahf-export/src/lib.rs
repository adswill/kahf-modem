//! Sending finished contacts to other ham software: N1MM Logger+ and any logger that
//! listens for its UDP broadcast, QRZ.com Logbook, Club Log and Logbook of The World.
//!
//! Everything here is explicit: a contact is sent only when the caller asks. Request
//! building is separate from the network call so it can be tested without either.

use kahf_log::Qso;
use std::net::UdpSocket;
use std::process::Command;
use std::time::Duration;

/// One QSO as an ADIF record with the operator's own callsign and grid.
pub fn adif_record(q: &Qso, my_call: &str, my_grid: &str) -> String {
    let mut out = String::new();
    let freq = q.freq_mhz.map(|f| format!("{f:.6}")).unwrap_or_default();
    let fields: [(&str, &str); 12] = [
        ("STATION_CALLSIGN", my_call),
        ("MY_GRIDSQUARE", my_grid),
        ("CALL", &q.call),
        ("QSO_DATE", &q.date),
        ("TIME_ON", &q.time),
        ("BAND", &q.band),
        ("MODE", &q.mode),
        ("FREQ", &freq),
        ("RST_SENT", &q.rst_sent),
        ("RST_RCVD", &q.rst_rcvd),
        ("NAME", &q.name),
        ("COMMENT", &q.comment),
    ];
    for (tag, v) in fields {
        if !v.is_empty() {
            out.push_str(&format!("<{tag}:{}>{v} ", v.len()));
        }
    }
    out.push_str("<EOR>");
    out
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// N1MM Logger+ "contactinfo" XML datagram.
pub fn n1mm_contactinfo(q: &Qso, my_call: &str, my_grid: &str) -> String {
    let mhz = q.freq_mhz.unwrap_or(0.0);
    let band = mhz.floor() as u32;
    let rx10hz = (mhz * 100_000.0).round() as u64;
    let ts = if q.date.len() == 8 && q.time.len() >= 6 {
        format!(
            "{}-{}-{} {}:{}:{}",
            &q.date[..4],
            &q.date[4..6],
            &q.date[6..8],
            &q.time[..2],
            &q.time[2..4],
            &q.time[4..6]
        )
    } else {
        String::new()
    };
    let e = xml_escape;
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><contactinfo><app>KAHF</app><contestname></contestname>\
<contestnr>1</contestnr><timestamp>{ts}</timestamp><mycall>{}</mycall><band>{band}</band>\
<rxfreq>{rx10hz}</rxfreq><txfreq>{rx10hz}</txfreq><operator>{}</operator><mode>USB</mode><call>{}</call>\
<snt>{}</snt><rcv>{}</rcv><gridsquare></gridsquare><mygrid>{}</mygrid><name>{}</name><comment>{}</comment>\
<points>1</points><radionr>1</radionr><isrunqso>0</isrunqso></contactinfo>",
        e(my_call),
        e(my_call),
        e(&q.call),
        e(&q.rst_sent),
        e(&q.rst_rcvd),
        e(my_grid),
        e(&q.name),
        e(&q.comment)
    )
}

/// Default N1MM listening address.
pub const N1MM_ADDR: &str = "127.0.0.1:12060";

/// Sends one contact to N1MM (or a compatible logger) by UDP.
pub fn send_n1mm(addr: &str, q: &Qso, my_call: &str, my_grid: &str) -> Result<(), String> {
    let s = UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("udp: {e}"))?;
    s.send_to(n1mm_contactinfo(q, my_call, my_grid).as_bytes(), addr)
        .map_err(|e| format!("send to {addr}: {e}"))?;
    Ok(())
}

/// Result of an upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Upload {
    Accepted,
    /// The service already had this contact.
    Duplicate,
}

pub const QRZ_URL: &str = "https://logbook.qrz.com/api";
pub const CLUBLOG_URL: &str = "https://clublog.org/realtime.php";

/// Reads QRZ's `RESULT=OK&LOGID=1` style reply.
pub fn parse_qrz_reply(body: &str) -> Result<Upload, String> {
    let get = |k: &str| {
        body.split('&')
            .find_map(|p| p.strip_prefix(k).and_then(|r| r.strip_prefix('=')))
    };
    match get("RESULT") {
        Some("OK") => Ok(Upload::Accepted),
        Some("REPLACE") => Ok(Upload::Duplicate),
        Some("AUTH") => Err("QRZ rejected the logbook API key".into()),
        _ => Err(format!("QRZ: {}", get("REASON").unwrap_or("unexpected reply"))),
    }
}

fn post_form(url: &str, form: &[(&str, &str)]) -> Result<(u16, String), String> {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(15)).build();
    match agent.post(url).send_form(form) {
        Ok(r) => Ok((r.status(), r.into_string().unwrap_or_default())),
        Err(ureq::Error::Status(c, r)) => Ok((c, r.into_string().unwrap_or_default())),
        Err(e) => Err(format!("could not reach {url}: {e}")),
    }
}

/// Uploads one contact to the QRZ.com Logbook with the operator's logbook API key.
pub fn upload_qrz(url: &str, api_key: &str, adif: &str) -> Result<Upload, String> {
    let (_, body) = post_form(url, &[("KEY", api_key), ("ACTION", "INSERT"), ("ADIF", adif)])?;
    parse_qrz_reply(&body)
}

/// Uploads one contact to Club Log (real-time API). `app_key` is the application key
/// Club Log issues to a program; the operator's own login goes alongside it.
pub fn upload_clublog(
    url: &str,
    email: &str,
    password: &str,
    callsign: &str,
    app_key: &str,
    adif: &str,
) -> Result<Upload, String> {
    let (code, body) = post_form(
        url,
        &[
            ("email", email),
            ("password", password),
            ("callsign", callsign),
            ("api", app_key),
            ("adif", adif),
        ],
    )?;
    match code {
        200 => Ok(Upload::Accepted),
        400 if body.to_ascii_lowercase().contains("dupe") => Ok(Upload::Duplicate),
        403 => Err("Club Log rejected the login or key".into()),
        c => Err(format!("Club Log answered {c}: {}", body.trim())),
    }
}

/// Command line that has ARRL's TQSL sign and upload an ADIF file to LoTW. LoTW needs
/// the operator's own certificate, which only TQSL can use, so KAHF hands the file over.
pub fn lotw_command(tqsl: &str, adif_path: &str, station_location: &str) -> Command {
    let mut c = Command::new(tqsl);
    c.args(["-d", "-u", "-q", "-l", station_location, adif_path]);
    c
}

/// Runs TQSL. Exit code 0 is success, 9 means every contact was a duplicate.
pub fn upload_lotw(tqsl: &str, adif_path: &str, station_location: &str) -> Result<Upload, String> {
    let st = lotw_command(tqsl, adif_path, station_location)
        .status()
        .map_err(|e| format!("could not run {tqsl}: {e} (install ARRL TQSL)"))?;
    match st.code() {
        Some(0) => Ok(Upload::Accepted),
        Some(9) => Ok(Upload::Duplicate),
        c => Err(format!("TQSL failed with code {c:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn qso() -> Qso {
        Qso {
            call: "OZ1ABC".into(),
            date: "20261120".into(),
            time: "153000".into(),
            band: "17M".into(),
            mode: "USB".into(),
            freq_mhz: Some(18.1131),
            rst_sent: "59".into(),
            rst_rcvd: "57".into(),
            name: "A&B".into(),
            comment: String::new(),
        }
    }

    /// One-shot HTTP server: returns the request text it got.
    fn serve(status: &str, body: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/x", l.local_addr().unwrap());
        let status = status.to_string();
        let t = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            // Headers and body can arrive in separate reads; take both.
            s.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            while let Ok(n) = s.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            s.set_read_timeout(None).unwrap();
            write!(
                s,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            String::from_utf8_lossy(&buf).into_owned()
        });
        (url, t)
    }

    #[test]
    fn adif_record_has_station_fields() {
        let r = adif_record(&qso(), "N0CALL", "JO65");
        assert!(r.contains("<STATION_CALLSIGN:6>N0CALL"));
        assert!(r.contains("<CALL:6>OZ1ABC") && r.ends_with("<EOR>"));
    }

    #[test]
    fn n1mm_xml_is_escaped_and_scaled() {
        let x = n1mm_contactinfo(&qso(), "N0CALL", "JO65");
        assert!(x.contains("<band>18</band>") && x.contains("<rxfreq>1811310</rxfreq>"));
        assert!(x.contains("<name>A&amp;B</name>") && x.contains("2026-11-20 15:30:00"));
    }

    #[test]
    fn n1mm_goes_out_by_udp() {
        let l = UdpSocket::bind("127.0.0.1:0").unwrap();
        l.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        send_n1mm(&l.local_addr().unwrap().to_string(), &qso(), "N0CALL", "JO65").unwrap();
        let mut b = [0u8; 4096];
        let n = l.recv(&mut b).unwrap();
        assert!(String::from_utf8_lossy(&b[..n]).starts_with("<?xml"));
    }

    #[test]
    fn qrz_replies() {
        assert_eq!(parse_qrz_reply("RESULT=OK&LOGID=5"), Ok(Upload::Accepted));
        assert_eq!(parse_qrz_reply("RESULT=REPLACE&LOGID=5"), Ok(Upload::Duplicate));
        assert!(parse_qrz_reply("RESULT=FAIL&REASON=bad")
            .unwrap_err()
            .contains("bad"));
        assert!(parse_qrz_reply("RESULT=AUTH").unwrap_err().contains("key"));
    }

    #[test]
    fn qrz_upload_posts_the_key_and_adif() {
        let (url, t) = serve("200 OK", "RESULT=OK&LOGID=1");
        assert_eq!(upload_qrz(&url, "KEY1", "<CALL:3>ABC<EOR>"), Ok(Upload::Accepted));
        let req = t.join().unwrap();
        assert!(req.contains("KEY=KEY1") && req.contains("ACTION=INSERT") && req.contains("ADIF=%3CCALL"));
    }

    #[test]
    fn clublog_codes() {
        let (url, t) = serve("200 OK", "OK");
        assert_eq!(
            upload_clublog(&url, "a@b.c", "pw", "N0CALL", "app", "<EOR>"),
            Ok(Upload::Accepted)
        );
        assert!(t.join().unwrap().contains("callsign=N0CALL"));
        let (url, _t) = serve("403 Forbidden", "no");
        assert!(upload_clublog(&url, "a", "b", "c", "d", "e").is_err());
    }

    #[test]
    fn lotw_command_line() {
        let c = lotw_command("tqsl", "/tmp/a.adi", "Home");
        let a: Vec<_> = c.get_args().map(|s| s.to_string_lossy().into_owned()).collect();
        assert_eq!(a, ["-d", "-u", "-q", "-l", "Home", "/tmp/a.adi"]);
    }
}
