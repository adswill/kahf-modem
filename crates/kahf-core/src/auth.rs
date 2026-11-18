//! Message signing for station authentication.
//!
//! This module signs messages so a receiver can check who sent them and that
//! the body was not changed. It is NOT encryption: amateur radio rules forbid
//! encryption. Nothing here hides the content of a message; the body is sent
//! in the clear and only its integrity and the sender's callsign are protected.
//!
//! Block layout (compact binary, no encryption):
//!
//! ```text
//! [1 byte callsign length N][N bytes callsign UTF-8][32 bytes Ed25519 public key][64 bytes signature]
//! ```
//!
//! The signature covers the callsign bytes and the body, so changing either
//! makes verification fail.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand_core::OsRng;
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

const PUBLIC_LEN: usize = 32;
const SIG_LEN: usize = 64;
const MAX_CALLSIGN: usize = 255;
const SECRET_LEN: usize = 32;
const DOMAIN: &[u8] = b"kahf-auth-v1";

/// File name of the station key inside the data folder.
pub const KEY_FILE: &str = "station.key";

/// Ed25519 signing key for a station. Signing only; never used to encrypt.
pub struct StationKey {
    key: SigningKey,
}

impl Clone for StationKey {
    fn clone(&self) -> Self {
        Self::from_bytes(&self.secret())
    }
}

impl StationKey {
    /// Generate a new random key from the OS random number generator.
    pub fn generate() -> Self {
        Self {
            key: SigningKey::generate(&mut OsRng),
        }
    }

    /// Build a key from its 32-byte secret seed.
    pub fn from_bytes(secret: &[u8; 32]) -> Self {
        Self {
            key: SigningKey::from_bytes(secret),
        }
    }

    /// The public verifying key bytes.
    pub fn to_public(&self) -> [u8; 32] {
        self.key.verifying_key().to_bytes()
    }

    /// Sign a message (domain-separated, over callsign and body) and return the 64-byte signature.
    pub fn sign(&self, callsign: &str, body: &[u8]) -> [u8; 64] {
        self.key.sign(&signed_data(callsign, body)).to_bytes()
    }

    /// Plain Ed25519 signature over `data`, with no domain prefix. The kahf.dev server checks
    /// its requests this way, over the canonical JSON of the request.
    pub fn sign_raw(&self, data: &[u8]) -> [u8; 64] {
        self.key.sign(data).to_bytes()
    }

    /// Fingerprint of this station's public key, for comparison in Settings.
    pub fn fingerprint(&self) -> String {
        fingerprint(&self.to_public())
    }

    fn secret(&self) -> [u8; SECRET_LEN] {
        self.key.to_bytes()
    }
}

/// Load the station key from the data folder `dir`. On first run (no key file
/// yet) a new key is generated and saved there. The file holds the raw 32-byte
/// secret and is created readable only by the owner on Unix.
pub fn load_or_create(dir: &Path) -> io::Result<StationKey> {
    let path = dir.join(KEY_FILE);
    match fs::read(&path) {
        Ok(bytes) => key_from_file_bytes(&bytes),
        Err(e) if e.kind() == io::ErrorKind::NotFound => create_key_file(dir, &path),
        Err(e) => Err(e),
    }
}

fn key_from_file_bytes(bytes: &[u8]) -> io::Result<StationKey> {
    let secret: [u8; SECRET_LEN] = bytes.try_into().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "station key file must be {SECRET_LEN} bytes, found {}",
                bytes.len()
            ),
        )
    })?;
    Ok(StationKey::from_bytes(&secret))
}

fn create_key_file(dir: &Path, path: &Path) -> io::Result<StationKey> {
    fs::create_dir_all(dir)?;
    let key = StationKey::generate();
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(mut file) => {
            file.write_all(&key.secret())?;
            file.sync_all()?;
            Ok(key)
        }
        // Another process created the file first: use its key.
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => key_from_file_bytes(&fs::read(path)?),
        Err(e) => Err(e),
    }
}

fn signed_data(callsign: &str, body: &[u8]) -> Vec<u8> {
    let mut data = Vec::with_capacity(DOMAIN.len() + 1 + callsign.len() + body.len());
    data.extend_from_slice(DOMAIN);
    data.push(callsign.len() as u8);
    data.extend_from_slice(callsign.as_bytes());
    data.extend_from_slice(body);
    data
}

/// Sign `body` as `callsign` and return the packed auth block:
/// callsign length, callsign, 32-byte public key, 64-byte signature.
///
/// Panics if the callsign is longer than 255 bytes.
pub fn sign(key: &StationKey, callsign: &str, body: &[u8]) -> Vec<u8> {
    assert!(callsign.len() <= MAX_CALLSIGN, "callsign longer than 255 bytes");
    let mut block = Vec::with_capacity(1 + callsign.len() + PUBLIC_LEN + SIG_LEN);
    block.push(callsign.len() as u8);
    block.extend_from_slice(callsign.as_bytes());
    block.extend_from_slice(&key.to_public());
    block.extend_from_slice(&key.sign(callsign, body));
    block
}

/// Check an auth block against `body`. Returns the signed callsign and the
/// sender's public key when the signature is valid.
pub fn verify(block: &[u8], body: &[u8]) -> Result<(String, [u8; 32]), String> {
    let n = *block.first().ok_or("auth block is empty")? as usize;
    if block.len() != 1 + n + PUBLIC_LEN + SIG_LEN {
        return Err(format!(
            "auth block length {} does not match callsign length {}",
            block.len(),
            n
        ));
    }
    let callsign_bytes = &block[1..1 + n];
    let callsign =
        std::str::from_utf8(callsign_bytes).map_err(|_| "callsign is not valid UTF-8".to_string())?;
    let mut public = [0u8; PUBLIC_LEN];
    public.copy_from_slice(&block[1 + n..1 + n + PUBLIC_LEN]);
    let sig_bytes: [u8; SIG_LEN] = block[1 + n + PUBLIC_LEN..]
        .try_into()
        .map_err(|_| "bad signature length".to_string())?;

    let vk = VerifyingKey::from_bytes(&public).map_err(|e| format!("bad public key: {e}"))?;
    let sig = Signature::from_bytes(&sig_bytes);
    vk.verify(&signed_data(callsign, body), &sig)
        .map_err(|_| "signature does not match callsign and body".to_string())?;
    Ok((callsign.to_string(), public))
}

/// Short readable fingerprint of a public key for manual comparison:
/// first 8 bytes of SHA-256 as hex, grouped in 4-hex-digit blocks.
pub fn fingerprint(public: &[u8; 32]) -> String {
    let digest = Sha256::digest(public);
    let hex: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    hex.as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> StationKey {
        StationKey::from_bytes(&[7u8; 32])
    }

    #[test]
    fn raw_signature_verifies_with_plain_ed25519_and_not_with_the_domain_prefix() {
        let key = StationKey::generate();
        let msg = br#"{"area":"N51W1","callsign":"G4ABC"}"#;
        let sig = Signature::from_bytes(&key.sign_raw(msg));
        let public = VerifyingKey::from_bytes(&key.to_public()).unwrap();
        assert!(public.verify(msg, &sig).is_ok(), "the server checks plain Ed25519");
        assert!(public.verify(&signed_data("G4ABC", msg), &sig).is_err());
    }

    #[test]
    fn sign_verify_round_trip() {
        let k = key();
        let block = sign(&k, "LY1ABC", b"hello");
        let (call, public) = verify(&block, b"hello").unwrap();
        assert_eq!(call, "LY1ABC");
        assert_eq!(public, k.to_public());
        assert_eq!(block.len(), 1 + 6 + 32 + 64);
    }

    #[test]
    fn changed_body_fails() {
        let block = sign(&key(), "LY1ABC", b"hello");
        assert!(verify(&block, b"hellO").is_err());
    }

    #[test]
    fn changed_callsign_fails() {
        let mut block = sign(&key(), "LY1ABC", b"hello");
        block[1] = b'M';
        assert!(verify(&block, b"hello").is_err());
    }

    #[test]
    fn wrong_length_fails() {
        let block = sign(&key(), "LY1ABC", b"hello");
        assert!(verify(&block[..block.len() - 1], b"hello").is_err());
        let mut longer = block.clone();
        longer.push(0);
        assert!(verify(&longer, b"hello").is_err());
        assert!(verify(&[], b"hello").is_err());
    }

    #[test]
    fn fingerprint_format() {
        let fp = fingerprint(&key().to_public());
        assert_eq!(fp.len(), 19); // 16 hex digits + 3 dashes
        assert_eq!(fp.split('-').count(), 4);
    }

    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("kahf-auth-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn key_is_created_once_and_reloaded() {
        let dir = scratch_dir("reload");
        let first = load_or_create(&dir).unwrap();
        let bytes = fs::read(dir.join(KEY_FILE)).unwrap();
        assert_eq!(bytes.len(), SECRET_LEN);
        let second = load_or_create(&dir).unwrap();
        assert_eq!(first.to_public(), second.to_public());
        assert_eq!(first.fingerprint(), fingerprint(&first.to_public()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_key_file_is_an_error() {
        let dir = scratch_dir("corrupt");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(KEY_FILE), b"short").unwrap();
        let err = load_or_create(&dir).err().expect("short key file must fail");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn different_folders_get_different_keys() {
        let a = scratch_dir("ka");
        let b = scratch_dir("kb");
        let ka = load_or_create(&a).unwrap();
        let kb = load_or_create(&b).unwrap();
        assert_ne!(ka.to_public(), kb.to_public());
        let _ = fs::remove_dir_all(&a);
        let _ = fs::remove_dir_all(&b);
    }
}
