//! A gateway sync against a stand-in for the Winlink CMS: telnet login, secure-login
//! challenge, mail for two kahf callsigns coming down, kahf mail going up.

use kahf_winlink::b2f::{self, Answer, Config, Mailbox, Offer};
use kahf_winlink::bridge::{self, KahfMail};
use kahf_winlink::message::{Address, Message};
use kahf_winlink::net;
use kahf_winlink::spool::{Folder, Policy, Spool};
use std::io::{Read, Write};
use std::net::TcpListener;

#[derive(Default)]
struct FakeCms {
    waiting: Vec<Message>,
    uploaded: Vec<Message>,
    fw_seen: Vec<Address>,
}

impl Mailbox for FakeCms {
    fn outbound(&mut self, fw: &[Address]) -> Vec<Message> {
        self.fw_seen = fw.to_vec();
        self.waiting
            .iter()
            .filter(|m| m.to.iter().any(|t| fw.contains(t)))
            .cloned()
            .collect()
    }
    fn answer(&mut self, _o: &Offer) -> Answer {
        Answer::Accept
    }
    fn deliver(&mut self, m: Message) -> Result<(), String> {
        self.uploaded.push(m);
        Ok(())
    }
    fn sent(&mut self, mid: &str, _r: bool) {
        self.waiting.retain(|m| m.mid != mid);
    }
    fn password_for(&mut self, call: &str) -> Option<String> {
        (call == "K1ABC").then(|| "account-secret".to_string())
    }
}

fn read_line(s: &mut impl Read) -> String {
    let mut v = Vec::new();
    let mut b = [0u8; 1];
    while s.read(&mut b).unwrap() == 1 && b[0] != b'\r' {
        v.push(b[0]);
    }
    String::from_utf8(v).unwrap()
}

#[test]
fn gateway_sync_moves_mail_both_ways_for_every_served_callsign() {
    let root = std::env::temp_dir().join(format!("kahf-winlink-gw-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let served = vec!["K1ABC".to_string(), "W1AW".to_string()];
    let mut spool = Spool::open(&root, &served, Policy::All).unwrap();

    // Outgoing: a kahf mail from W1AW to an internet address, queued by the bridge.
    let out = bridge::to_winlink(
        &KahfMail {
            from: "W1AW".into(),
            to: "friend@example.com".into(),
            subject: "from the air".into(),
            body: "73".into(),
            files: vec![],
        },
        1_800_000_000,
    )
    .unwrap();
    spool.queue("W1AW", &out).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let cms = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        s.write_all(b"Callsign :\r").unwrap();
        assert_eq!(read_line(&mut s), "K1ABC");
        s.write_all(b"Password :\r").unwrap();
        assert_eq!(read_line(&mut s), net::CMS_TELNET_PASSWORD);
        let mut cfg = Config::client("wl2k", "K1ABC");
        cfg.master = true;
        cfg.require_login = true;
        let mut cms = FakeCms::default();
        cms.waiting.push(Message::new(
            "N0CALL",
            &["K1ABC"],
            "for k1abc",
            "hello",
            1_800_000_100,
        ));
        cms.waiting.push(Message::new(
            "bob@example.com",
            &["W1AW"],
            "for w1aw",
            "hi there",
            1_800_000_200,
        ));
        cms.waiting.push(Message::new(
            "N0CALL",
            &["ZZ9ZZZ"],
            "for someone else",
            "no",
            1_800_000_300,
        ));
        let r = b2f::run(s, &cfg, &mut cms);
        (r, cms)
    });

    let mut cfg = Config::client("K1ABC", "wl2k");
    cfg.password = Some("account-secret".into());
    cfg.aux = vec![("W1AW".into(), None)];
    let stats = net::sync_cms(&addr, &cfg, &mut spool).unwrap();
    let (cms_result, cms) = cms.join().unwrap();
    cms_result.unwrap();

    assert_eq!(stats.received.len(), 2);
    assert_eq!(stats.sent.len(), 1);
    assert!(cms.fw_seen.contains(&Address::call("W1AW")));
    assert_eq!(cms.uploaded.len(), 1);
    assert_eq!(cms.uploaded[0].to[0].to_string(), "SMTP:friend@example.com");

    let k = spool.list("K1ABC", Folder::In);
    let w = spool.list("W1AW", Folder::In);
    assert_eq!((k.len(), w.len()), (1, 1));
    assert_eq!(k[0].subject, "for k1abc");
    // The mail from an internet sender reaches the kahf side with its address intact.
    let kahf = bridge::from_winlink(&w[0], &served);
    assert_eq!(kahf[0].from, "SMTP:bob@example.com");
    assert_eq!(kahf[0].to, "W1AW");
    assert_eq!(spool.list("W1AW", Folder::Sent).len(), 1);
    assert!(spool.list("W1AW", Folder::Out).is_empty());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn wrong_account_password_is_refused_and_nothing_moves() {
    let root = std::env::temp_dir().join(format!("kahf-winlink-gw-bad-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut spool = Spool::open(&root, &["K1ABC".to_string()], Policy::All).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let cms = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        s.write_all(b"Callsign :\rPassword :\r").unwrap();
        read_line(&mut s);
        read_line(&mut s);
        let mut cfg = Config::client("wl2k", "K1ABC");
        cfg.master = true;
        cfg.require_login = true;
        let mut cms = FakeCms::default();
        cms.waiting
            .push(Message::new("N0CALL", &["K1ABC"], "secret", "x", 0));
        b2f::run(s, &cfg, &mut cms).is_err()
    });
    let mut cfg = Config::client("K1ABC", "wl2k");
    cfg.password = Some("not-the-password".into());
    assert!(net::sync_cms(&addr, &cfg, &mut spool).is_err());
    assert!(cms.join().unwrap());
    assert!(spool.list("K1ABC", Folder::In).is_empty());
    let _ = std::fs::remove_dir_all(root);
}
