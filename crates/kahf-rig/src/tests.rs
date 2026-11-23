use super::*;
use crate::sim::{Proto, SimPort};

fn rig(
    model: &str,
    proto: Proto,
    ptt: PttMethod,
    start: u64,
) -> (Box<dyn Rig>, std::sync::Arc<std::sync::Mutex<sim::SimState>>) {
    let spec = RigSpec {
        model: model.into(),
        device: "sim".into(),
        baud: None,
        ptt,
        ptt_device: None,
    };
    let (p, st) = SimPort::new(proto, start);
    (with_port(&spec, Box::new(p)).unwrap(), st)
}

#[test]
fn spec_parsing() {
    let s = parse_spec("ic7300:/dev/ttyUSB0:115200:cat").unwrap();
    assert_eq!(
        (s.baud, s.ptt, s.device.as_str()),
        (Some(115200), PttMethod::Cat, "/dev/ttyUSB0")
    );
    let s = parse_spec("ft991a:COM3:rts").unwrap();
    assert_eq!((s.device.as_str(), s.baud, s.ptt), ("COM3", None, PttMethod::Rts));
    assert_eq!(
        parse_spec("flrig:127.0.0.1:12345").unwrap().device,
        "127.0.0.1:12345"
    );
    assert!(parse_spec("nonsense:/dev/x").is_err());
    assert!(parse_spec("ic7300").is_err());
    assert!(parse_spec("icom-a2:/dev/x").is_ok());
}

#[test]
fn bcd_round_trip() {
    assert_eq!(civ::bcd_freq(14_074_000), [0x00, 0x40, 0x07, 0x14, 0x00]);
    assert_eq!(civ::parse_bcd_freq(&civ::bcd_freq(18_111_000)), Some(18_111_000));
}

#[test]
fn icom_tunes_reads_and_keys() {
    let (mut r, st) = rig("ic7300", Proto::Civ(0x94), PttMethod::Cat, 7_000_000);
    r.set_freq_hz(18_109_650).unwrap();
    assert_eq!(r.freq_hz().unwrap(), 18_109_650);
    r.set_mode(Mode::UsbData).unwrap();
    r.set_ptt(true).unwrap();
    assert!(st.lock().unwrap().transmitting);
    r.set_ptt(false).unwrap();
    assert!(!st.lock().unwrap().transmitting);
}

#[test]
fn icom_wrong_address_times_out_politely() {
    let spec = RigSpec {
        model: "ic7300".into(),
        device: "sim".into(),
        baud: None,
        ptt: PttMethod::Cat,
        ptt_device: None,
    };
    let (p, _) = SimPort::new(Proto::Civ(0xA4), 0);
    let mut r = with_port(&spec, Box::new(p)).unwrap();
    assert!(r.freq_hz().unwrap_err().contains("did not"));
}

#[test]
fn text_families_tune_and_key() {
    for (m, digits) in [
        ("ft991a", 9),
        ("ft710", 9),
        ("ts590", 11),
        ("ts480", 11),
        ("trusdx", 11),
        ("qmx", 11),
    ] {
        let (mut r, st) = rig(m, Proto::Text(digits), PttMethod::Cat, 0);
        r.set_freq_hz(18_109_650).unwrap();
        assert_eq!(r.freq_hz().unwrap(), 18_109_650, "{m}");
        r.set_mode(Mode::UsbData).unwrap();
        r.set_ptt(true).unwrap();
        assert!(st.lock().unwrap().transmitting, "{m} keyed");
        r.set_ptt(false).unwrap();
        assert!(!st.lock().unwrap().transmitting, "{m} released");
    }
}

#[test]
fn yaesu_uses_data_usb_mode() {
    let (mut r, st) = rig("ft991a", Proto::Text(9), PttMethod::Cat, 0);
    r.set_mode(Mode::UsbData).unwrap();
    assert_eq!(st.lock().unwrap().last_mode, "MD0C;");
}

#[test]
fn rts_and_dtr_ptt_use_the_lines_not_cat() {
    for (m, line) in [(PttMethod::Rts, "RTS"), (PttMethod::Dtr, "DTR")] {
        let (mut r, st) = rig("ic7300", Proto::Civ(0x94), m, 0);
        r.set_ptt(true).unwrap();
        r.set_ptt(false).unwrap();
        let log = st.lock().unwrap().log.clone();
        assert_eq!(log, vec![format!("{line} true"), format!("{line} false")]);
    }
}

#[test]
fn vox_sends_nothing() {
    let (mut r, st) = rig("ft991a", Proto::Text(9), PttMethod::Vox, 0);
    r.set_ptt(true).unwrap();
    assert!(st.lock().unwrap().log.is_empty());
}

#[test]
fn qsy_slots_and_17m_guard() {
    assert_eq!(qsy::relay_slot_count(), 3);
    assert_eq!(qsy::relay_slot_centre_hz(0), Some(18_112_350));
    let (mut r, st) = rig("ic7300", Proto::Civ(0x94), PttMethod::Cat, 14_074_000);
    assert_eq!(qsy::qsy_to_relay_slot(r.as_mut(), 1).unwrap(), 18_115_050 - 1500);
    assert_eq!(st.lock().unwrap().freq_hz, 18_113_550);
    assert!(!st.lock().unwrap().transmitting);
    assert!(qsy::qsy_to_relay_slot(r.as_mut(), 3).is_err());
    assert!(qsy::qsy_to_channel(r.as_mut(), 14_074_000)
        .unwrap_err()
        .contains("17 m"));
    assert_eq!(
        st.lock().unwrap().freq_hz,
        18_113_550,
        "refused QSY leaves the dial alone"
    );
}

#[test]
fn flrig_against_fake_server() {
    use std::io::{Read, Write};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let t = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for _ in 0..4 {
            let (mut s, _) = l.accept().unwrap();
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap();
            let req = String::from_utf8_lossy(&buf[..n]).into_owned();
            let val = if req.contains("get_vfo") {
                "<double>18113550.0</double>"
            } else {
                "<string>IC-7300</string>"
            };
            let body = format!("<?xml version=\"1.0\"?><methodResponse><params><param><value>{val}</value></param></params></methodResponse>");
            write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            seen.push(req);
        }
        seen
    });
    let mut r = flrig::FlrigClient::connect(&addr, PttMethod::Cat).unwrap();
    r.set_ptt(true).unwrap();
    r.set_freq_hz(18_113_550).unwrap();
    assert_eq!(r.freq_hz().unwrap(), 18_113_550);
    let seen = t.join().unwrap();
    assert!(seen[1].contains("rig.set_ptt") && seen[1].contains("<i4>1</i4>"));
    assert!(seen[2].contains("<double>18113550.0</double>"));
}

#[test]
fn flrig_fault_is_an_error() {
    let e = flrig::parse_response(
        "<methodResponse><fault><value><string>bad</string></value></fault></methodResponse>",
    )
    .unwrap_err();
    assert!(e.contains("bad"));
}
