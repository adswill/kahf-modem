//! TCP transports for B2F: the Winlink CMS over telnet, and peer-to-peer telnet links that
//! Pat and Winlink Express both speak (one side listens, the other calls).
//!
//! Winlink's telnet front door is a plain text login before the B2F greeting: the called side
//! asks `Callsign :` and `Password :`. Towards the CMS the telnet password is the fixed
//! string `CMSTelnet`; the account password is proven later by the secure-login challenge.
//! Nothing on these links is encrypted, and the CMS and the radio side both rely on callsigns
//! and the account password for authentication, as amateur rules require.

use crate::b2f::{self, Config, Error, Stats};
use crate::spool::{Policy, Spool};
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::time::Duration;

/// The CMS telnet gateway.
pub const CMS_ADDR: &str = "server.winlink.org:8772";
/// Fixed telnet password for CMS logins.
pub const CMS_TELNET_PASSWORD: &str = "CMSTelnet";
/// Callsign the CMS answers to in B2F.
pub const CMS_TARGET: &str = "wl2k";
/// Port Pat and Winlink Express use for telnet peer-to-peer.
pub const P2P_PORT: u16 = 8774;

const IO_TIMEOUT: Duration = Duration::from_secs(120);

fn dial(addr: &str) -> io::Result<TcpStream> {
    let mut last = io::Error::new(io::ErrorKind::NotFound, "address did not resolve");
    for a in addr.to_socket_addrs()? {
        match TcpStream::connect_timeout(&a, Duration::from_secs(20)) {
            Ok(s) => {
                s.set_read_timeout(Some(IO_TIMEOUT))?;
                s.set_write_timeout(Some(IO_TIMEOUT))?;
                s.set_nodelay(true)?;
                return Ok(s);
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Reads one CR or LF ended line straight off the socket, one byte at a time, so no bytes
/// are held back from the B2F session that follows.
fn raw_line<R: Read>(r: &mut R) -> io::Result<String> {
    let mut v = Vec::new();
    let mut b = [0u8; 1];
    loop {
        if r.read(&mut b)? == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        match b[0] {
            b'\r' | b'\n' if v.is_empty() => continue,
            b'\r' | b'\n' => return Ok(String::from_utf8_lossy(&v).trim().to_string()),
            c => v.push(c),
        }
        if v.len() > 512 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "login line too long"));
        }
    }
}

/// Answers the `Callsign :` and `Password :` prompts of a called station.
fn telnet_login(s: &mut TcpStream, call: &str, password: &str) -> Result<(), Error> {
    for _ in 0..40 {
        let line = raw_line(s)?.to_ascii_lowercase();
        if line.starts_with("callsign") {
            s.write_all(format!("{call}\r").as_bytes())?;
        } else if line.starts_with("password") {
            s.write_all(format!("{password}\r").as_bytes())?;
            return Ok(());
        }
    }
    Err(Error::Protocol("no login prompt from the remote".into()))
}

/// Exchanges mail with the Winlink CMS for every callsign `spool` serves. `cfg.mycall` logs
/// in, any other served callsign is added as an auxiliary address, and `cfg.password` is the
/// Winlink account password.
pub fn sync_cms(addr: &str, cfg: &Config, spool: &mut Spool) -> Result<Stats, Error> {
    let mut s = dial(addr)?;
    telnet_login(&mut s, &cfg.mycall, CMS_TELNET_PASSWORD)?;
    let mut cfg = cfg.clone();
    cfg.target = CMS_TARGET.into();
    cfg.master = false;
    b2f::run(s, &cfg, spool)
}

/// Calls another station's telnet P2P port (Pat, Winlink Express or another kahf).
pub fn call_peer(addr: &str, cfg: &Config, p2p_password: &str, spool: &mut Spool) -> Result<Stats, Error> {
    let mut s = dial(addr)?;
    telnet_login(&mut s, &cfg.mycall, p2p_password)?;
    let mut cfg = cfg.clone();
    cfg.master = false;
    b2f::run(s, &cfg, spool)
}

/// Accepts one P2P call on `listener` and runs the session. `password` is the telnet password
/// callers must give; `None` accepts any. Returns the caller's callsign and the result.
pub fn accept_peer(
    listener: &TcpListener,
    cfg: &Config,
    password: Option<&str>,
    root: &std::path::Path,
) -> io::Result<(String, Result<Stats, Error>)> {
    let (mut s, _) = listener.accept()?;
    s.set_read_timeout(Some(IO_TIMEOUT))?;
    s.set_write_timeout(Some(IO_TIMEOUT))?;
    s.set_nodelay(true)?;
    s.write_all(b"Callsign :\r")?;
    let caller = raw_line(&mut s)?.to_ascii_uppercase();
    s.write_all(b"Password :\r")?;
    let given = raw_line(&mut s)?;
    if password.is_some_and(|p| p != given) || crate::callsign_ok(&caller).is_err() {
        let _ = s.write_all(b"*** Login failed\r\n");
        return Ok((caller, Err(Error::Remote("login failed".into()))));
    }
    let mut cfg = cfg.clone();
    cfg.master = true;
    cfg.target = caller.clone();
    let mut spool = Spool::open(root, &[cfg.mycall.clone()], Policy::ForPeer)?;
    Ok((caller, b2f::run(s, &cfg, &mut spool)))
}
