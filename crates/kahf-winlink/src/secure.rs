//! Secure login: the challenge/response the Winlink CMS uses to check an account password.
//! The password never crosses the link; only an 8 digit token derived from it does. This is
//! authentication, not encryption, and is allowed on amateur bands.

use md5::{Digest, Md5};

// A fixed 64 byte value mixed into every response, as published by the Winlink client
// ecosystem (paclink-unix, Pat).
const SALT: [u8; 64] = [
    77, 197, 101, 206, 190, 249, 93, 200, 51, 243, 93, 237, 71, 94, 239, 138, 68, 108, 70, 185, 225, 137,
    217, 16, 51, 122, 193, 48, 194, 195, 198, 175, 172, 169, 70, 84, 61, 62, 104, 186, 114, 52, 61, 168, 66,
    129, 192, 208, 187, 249, 232, 193, 41, 113, 41, 45, 240, 16, 29, 228, 208, 228, 61, 20,
];

/// The 8 digit response to `challenge` for `password`.
pub fn response(challenge: &str, password: &str) -> String {
    let mut h = Md5::new();
    h.update(challenge.as_bytes());
    h.update(password.as_bytes());
    h.update(SALT);
    let sum = h.finalize();
    let mut pr = i32::from(sum[3] & 0x3f);
    for i in (0..3).rev() {
        pr = (pr << 8) | i32::from(sum[i]);
    }
    let s = format!("{pr:08}");
    s[s.len() - 8..].to_string()
}

/// A fresh 8 digit challenge.
pub fn new_challenge() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut h = Md5::new();
    h.update(nanos.to_le_bytes());
    h.update(std::process::id().to_le_bytes());
    let sum = h.finalize();
    let n = u32::from_le_bytes([sum[0], sum[1], sum[2], sum[3]]) % 100_000_000;
    format!("{n:08}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_reference_vector() {
        // From wl2k-go's secure_test.go.
        assert_eq!(response("23753528", "FOOBAR"), "72768415");
    }

    #[test]
    fn challenge_is_eight_digits() {
        let c = new_challenge();
        assert_eq!(c.len(), 8);
        assert!(c.bytes().all(|b| b.is_ascii_digit()));
    }
}
