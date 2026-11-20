//! `kahf-winlink`: bridge a mailbox to Winlink.
//!
//! ```text
//! kahf-winlink gateway --call K1ABC [--serve K1ABC,W1AW] --root DIR [--interval 300] [--once]
//! kahf-winlink cms     --call K1ABC [--serve ...] --root DIR            (one sync, then exit)
//! kahf-winlink listen  --call K1ABC --root DIR [--bind 0.0.0.0:8774] [--p2p-password PW]
//! kahf-winlink call    HOST:PORT --call K1ABC --root DIR [--p2p-password PW]
//! kahf-winlink send    --root DIR --from K1ABC --to N0CALL --subject S [--body FILE|-] [--attach FILE]...
//! kahf-winlink list    --root DIR --call K1ABC [--folder in|out|sent|archive]
//! ```
//!
//! The Winlink account password is read from `WINLINK_PASSWORD` or `--password-file`, never
//! from the command line. `--root` is a Pat-style mailbox directory.
use kahf_winlink::b2f::{Config, Stats};
use kahf_winlink::message::{File, Message};
use kahf_winlink::net;
use kahf_winlink::spool::{Folder, Policy, Spool};
use std::env;
use std::fs;
use std::io::Read;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const USAGE: &str = "usage:
  kahf-winlink gateway --call CALL [--serve CALL,CALL..] --root DIR [--interval SECS] [--once] [--cms HOST:PORT]
  kahf-winlink listen  --call CALL --root DIR [--bind ADDR:PORT] [--p2p-password PW]
  kahf-winlink call    HOST:PORT --call CALL --root DIR [--p2p-password PW]
  kahf-winlink send    --root DIR --from CALL --to ADDR[,ADDR..] --subject TEXT [--body FILE|-] [--attach FILE]..
  kahf-winlink list    --root DIR --call CALL [--folder in|out|sent|archive]
The Winlink password comes from WINLINK_PASSWORD or --password-file FILE.";

struct Args {
    positional: Vec<String>,
    opts: Vec<(String, String)>,
}

impl Args {
    fn parse(raw: Vec<String>) -> Result<Args, String> {
        const FLAGS: [&str; 1] = ["once"];
        let mut a = Args {
            positional: Vec::new(),
            opts: Vec::new(),
        };
        let mut it = raw.into_iter();
        while let Some(x) = it.next() {
            if let Some(name) = x.strip_prefix("--") {
                if FLAGS.contains(&name) {
                    a.opts.push((name.into(), String::new()));
                } else {
                    let v = it.next().ok_or_else(|| format!("--{name} needs a value"))?;
                    a.opts.push((name.into(), v));
                }
            } else {
                a.positional.push(x);
            }
        }
        Ok(a)
    }
    fn get(&self, k: &str) -> Option<&str> {
        self.opts
            .iter()
            .rev()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.as_str())
    }
    fn all(&self, k: &str) -> Vec<&str> {
        self.opts
            .iter()
            .filter(|(n, _)| n == k)
            .map(|(_, v)| v.as_str())
            .collect()
    }
    fn need(&self, k: &str) -> Result<&str, String> {
        self.get(k).ok_or_else(|| format!("--{k} is required"))
    }
    fn has(&self, k: &str) -> bool {
        self.opts.iter().any(|(n, _)| n == k)
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn password(a: &Args) -> Result<Option<String>, String> {
    if let Some(f) = a.get("password-file") {
        let s = fs::read_to_string(f).map_err(|e| format!("cannot read {f}: {e}"))?;
        return Ok(Some(s.trim().to_string()));
    }
    Ok(env::var("WINLINK_PASSWORD").ok().filter(|s| !s.is_empty()))
}

fn served(a: &Args, call: &str) -> Vec<String> {
    let mut v = vec![call.to_ascii_uppercase()];
    for s in a
        .get("serve")
        .unwrap_or("")
        .split(',')
        .map(|s| s.trim().to_ascii_uppercase())
    {
        if !s.is_empty() && !v.contains(&s) {
            v.push(s);
        }
    }
    v
}

fn config(a: &Args, call: &str, served: &[String]) -> Result<Config, String> {
    kahf_winlink::callsign_ok(&call.to_ascii_uppercase())?;
    let mut c = Config::client(call, "wl2k");
    c.password = password(a)?;
    c.aux = served.iter().skip(1).map(|s| (s.clone(), None)).collect();
    Ok(c)
}

fn report(what: &str, r: Result<Stats, kahf_winlink::b2f::Error>) -> bool {
    match r {
        Ok(s) => {
            println!("{what}: sent {}, received {}", s.sent.len(), s.received.len());
            true
        }
        Err(e) => {
            eprintln!("{what}: {e}");
            false
        }
    }
}

/// The wait before the next sync: the interval, doubled after each failure, at most one hour
/// (and never less than the interval).
fn backoff(every: Duration, failures: u32) -> Duration {
    (every * 2u32.pow(failures))
        .min(Duration::from_secs(3600))
        .max(every)
}

fn run(raw: Vec<String>) -> Result<bool, String> {
    let cmd = raw.first().cloned().ok_or("no command")?;
    let a = Args::parse(raw[1..].to_vec())?;
    let io = |e: std::io::Error| e.to_string();
    match cmd.as_str() {
        "gateway" | "cms" => {
            let call = a.need("call")?;
            let srv = served(&a, call);
            let cfg = config(&a, call, &srv)?;
            if cfg.password.is_none() {
                return Err(
                    "the Winlink account password is needed: set WINLINK_PASSWORD or --password-file".into(),
                );
            }
            let root = PathBuf::from(a.need("root")?);
            let addr = a.get("cms").unwrap_or(net::CMS_ADDR).to_string();
            let every = Duration::from_secs(
                a.get("interval")
                    .map_or(Ok(300), |s| s.parse().map_err(|_| "bad --interval"))?,
            );
            let mut failures = 0u32;
            loop {
                let mut spool = Spool::open(&root, &srv, Policy::All).map_err(io)?;
                let result = net::sync_cms(&addr, &cfg, &mut spool);
                // A refused login is not retried: repeating it only risks locking the account.
                if let Err(kahf_winlink::b2f::Error::Remote(why)) = &result {
                    if why.to_ascii_lowercase().contains("secure login failed") {
                        return Err(format!(
                            "the CMS refused the login ({why}); check the Winlink password"
                        ));
                    }
                }
                let ok = report("CMS sync", result);
                if cmd == "cms" || a.has("once") {
                    return Ok(ok);
                }
                failures = if ok { 0 } else { (failures + 1).min(6) };
                std::thread::sleep(backoff(every, failures));
            }
        }
        "listen" => {
            let call = a.need("call")?;
            let cfg = config(&a, call, &[call.to_ascii_uppercase()])?;
            let root = PathBuf::from(a.need("root")?);
            let bind = a
                .get("bind")
                .map_or_else(|| format!("0.0.0.0:{}", net::P2P_PORT), str::to_string);
            let l = TcpListener::bind(&bind).map_err(io)?;
            println!("listening for Winlink peer-to-peer calls on {bind}");
            loop {
                let (caller, r) = net::accept_peer(&l, &cfg, a.get("p2p-password"), &root).map_err(io)?;
                report(&format!("call from {caller}"), r);
            }
        }
        "call" => {
            let addr = a.positional.first().ok_or("call needs HOST:PORT")?;
            let call = a.need("call")?;
            let srv = vec![call.to_ascii_uppercase()];
            let mut cfg = config(&a, call, &srv)?;
            cfg.target = a.get("target").unwrap_or("PEER").to_string();
            let root = PathBuf::from(a.need("root")?);
            let mut spool = Spool::open(&root, &srv, Policy::ForPeer).map_err(io)?;
            Ok(report(
                &format!("call to {addr}"),
                net::call_peer(addr, &cfg, a.get("p2p-password").unwrap_or(""), &mut spool),
            ))
        }
        "send" => {
            let from = a.need("from")?.to_ascii_uppercase();
            let to: Vec<&str> = a.need("to")?.split(',').collect();
            let mut m = Message::new(&from, &to, a.need("subject")?, "", now());
            if let Some(b) = a.get("body") {
                let mut s = String::new();
                if b == "-" {
                    std::io::stdin().read_to_string(&mut s).map_err(io)?;
                } else {
                    s = fs::read_to_string(b).map_err(|e| format!("cannot read {b}: {e}"))?;
                }
                m.body = s;
            }
            for f in a.all("attach") {
                let data = fs::read(f).map_err(|e| format!("cannot read {f}: {e}"))?;
                let name = PathBuf::from(f)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                m.files.push(File { name, data });
            }
            let spool = Spool::open(
                &PathBuf::from(a.need("root")?),
                std::slice::from_ref(&from),
                Policy::All,
            )
            .map_err(io)?;
            spool.queue(&from, &m).map_err(io)?;
            println!("queued {} in {from}/out", m.mid);
            Ok(true)
        }
        "list" => {
            let call = a.need("call")?.to_ascii_uppercase();
            let folder = match a.get("folder").unwrap_or("in") {
                "in" => Folder::In,
                "out" => Folder::Out,
                "sent" => Folder::Sent,
                "archive" => Folder::Archive,
                other => return Err(format!("unknown folder {other:?}")),
            };
            let spool = Spool::open(
                &PathBuf::from(a.need("root")?),
                std::slice::from_ref(&call),
                Policy::All,
            )
            .map_err(io)?;
            for m in spool.list(&call, folder) {
                let to: Vec<String> = m.to.iter().map(|t| t.to_string()).collect();
                println!(
                    "{}  {}  {} -> {}  {}  ({} attachments)",
                    m.mid,
                    kahf_winlink::message::format_date(m.date),
                    m.from,
                    to.join(","),
                    m.subject,
                    m.files.len()
                );
            }
            Ok(true)
        }
        _ => Err(format!("unknown command {cmd:?}")),
    }
}

fn main() {
    match run(env::args().skip(1).collect()) {
        Ok(true) => {}
        Ok(false) => process::exit(1),
        Err(e) => {
            eprintln!("error: {e}\n{USAGE}");
            process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_and_caps() {
        let e = Duration::from_secs(300);
        assert_eq!(backoff(e, 0), e);
        assert_eq!(backoff(e, 1), Duration::from_secs(600));
        assert_eq!(backoff(e, 6), Duration::from_secs(3600));
        // An interval longer than the cap is kept as it is.
        assert_eq!(backoff(Duration::from_secs(7200), 3), Duration::from_secs(7200));
    }
}
