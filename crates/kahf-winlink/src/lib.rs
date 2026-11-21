//! Winlink bridge for kahf.
//!
//! - [`lzhuf`], [`message`], [`secure`], [`b2f`]: the Winlink wire formats and the B2F
//!   forwarding protocol, interoperable with the CMS, Pat and Winlink Express.
//! - [`bridge`]: maps kahf mail to Winlink messages and back.
pub mod b2f;
pub mod bridge;
pub mod lzhuf;
pub mod message;
pub mod net;
pub mod secure;
pub mod spool;
mod tables;

/// A loose callsign check for network logins: 3 to 12 letters, digits and `/`, with at
/// least one digit and one letter. Real validation is the CMS's job.
pub fn callsign_ok(c: &str) -> Result<(), String> {
    let ok = (3..=12).contains(&c.len())
        && c.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'/' || b == b'-')
        && c.bytes().any(|b| b.is_ascii_digit())
        && c.bytes().any(|b| b.is_ascii_alphabetic());
    if ok {
        Ok(())
    } else {
        Err(format!("{c:?} is not a callsign"))
    }
}
