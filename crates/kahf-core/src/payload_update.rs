//! Signed software and firmware updates, sent as file data.
//!
//! A publisher describes an update with a [`Manifest`]: product name, version
//! string, total length and the SHA-256 of the whole image. The encoded
//! manifest is signed with the publisher's Ed25519 key through [`crate::auth`].
//!
//! A receiver checks, in order: the signature over the manifest (must come
//! from the trusted publisher key), the length of the image, then its SHA-256.
//! Because the signed manifest carries the hash, any changed byte is caught.
//!
//! Nothing here encrypts. The image and manifest travel in the clear; only
//! integrity and the publisher's identity are protected.
//!
//! Manifest encoding (compact binary):
//!
//! ```text
//! [1 byte product length P][P bytes product UTF-8]
//! [1 byte version length V][V bytes version UTF-8]
//! [8 bytes length, little endian][32 bytes SHA-256 of the image]
//! ```

use sha2::{Digest, Sha256};

use crate::auth::{self, StationKey};

/// Name placed in the auth block for the publisher's signature.
const PUBLISHER_LABEL: &str = "kahf-publisher";
/// Longest product or version string the manifest can carry.
const MAX_FIELD: usize = 255;

const MSG_UNSIGNED: &str = "The update is not signed by the trusted publisher.";
const MSG_INCOMPLETE: &str = "The update file is incomplete.";
const MSG_TRANSIT: &str = "The update file was changed in transit.";

/// Description of one update image. Signed as its encoded form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub product: String,
    pub version: String,
    pub length: u64,
    pub sha256: [u8; 32],
}

/// Build a manifest for `image`, computing its length and SHA-256.
///
/// Returns an error if the product or version is longer than 255 bytes.
pub fn make_manifest(product: &str, version: &str, image: &[u8]) -> Result<Manifest, String> {
    if product.len() > MAX_FIELD {
        return Err("product name is longer than 255 bytes".to_string());
    }
    if version.len() > MAX_FIELD {
        return Err("version string is longer than 255 bytes".to_string());
    }
    Ok(Manifest {
        product: product.to_string(),
        version: version.to_string(),
        length: image.len() as u64,
        sha256: sha256(image),
    })
}

/// Encode a manifest to its compact binary form.
///
/// Panics if the product or version is longer than 255 bytes. Manifests from
/// `make_manifest` never hit this; `verify_image` checks lengths before encoding.
pub fn encode_manifest(m: &Manifest) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + m.product.len() + m.version.len() + 8 + 32);
    push_field(&mut out, &m.product);
    push_field(&mut out, &m.version);
    out.extend_from_slice(&m.length.to_le_bytes());
    out.extend_from_slice(&m.sha256);
    out
}

/// Sign the encoded manifest with the publisher's key and return the auth block.
pub fn sign_manifest(key: &StationKey, m: &Manifest) -> Vec<u8> {
    auth::sign(key, PUBLISHER_LABEL, &encode_manifest(m))
}

/// Verify an update: signature first, then length, then SHA-256 of the image.
///
/// `signature_block` is the block produced by [`sign_manifest`]. `trusted_key`
/// is the publisher's Ed25519 public key the receiver already holds.
pub fn verify_image(
    manifest: &Manifest,
    signature_block: &[u8],
    image: &[u8],
    trusted_key: &[u8; 32],
) -> Result<(), String> {
    if manifest.product.len() > MAX_FIELD || manifest.version.len() > MAX_FIELD {
        return Err(MSG_UNSIGNED.to_string());
    }

    let (_, signer) =
        auth::verify(signature_block, &encode_manifest(manifest)).map_err(|_| MSG_UNSIGNED.to_string())?;
    if signer != *trusted_key {
        return Err(MSG_UNSIGNED.to_string());
    }

    let received = image.len() as u64;
    if received < manifest.length {
        return Err(MSG_INCOMPLETE.to_string());
    }
    if received > manifest.length || sha256(image) != manifest.sha256 {
        return Err(MSG_TRANSIT.to_string());
    }
    Ok(())
}

fn push_field(out: &mut Vec<u8>, s: &str) {
    assert!(s.len() <= MAX_FIELD, "manifest field longer than 255 bytes");
    out.push(s.len() as u8);
    out.extend_from_slice(s.as_bytes());
}

fn sha256(data: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(data);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u8) -> StationKey {
        StationKey::from_bytes(&[seed; 32])
    }

    fn image() -> Vec<u8> {
        (0..4096u32).map(|i| (i % 251) as u8).collect()
    }

    fn signed(img: &[u8]) -> (Manifest, Vec<u8>, [u8; 32]) {
        let publisher = key(1);
        let m = make_manifest("kahf-fw", "1.0.0", img).unwrap();
        let block = sign_manifest(&publisher, &m);
        (m, block, publisher.to_public())
    }

    #[test]
    fn valid_image_verifies() {
        let img = image();
        let (m, block, trusted) = signed(&img);
        assert_eq!(verify_image(&m, &block, &img, &trusted), Ok(()));
    }

    #[test]
    fn changed_byte_fails_in_transit() {
        let img = image();
        let (m, block, trusted) = signed(&img);
        let mut bad = img.clone();
        bad[1000] ^= 0x01;
        assert_eq!(
            verify_image(&m, &block, &bad, &trusted),
            Err(MSG_TRANSIT.to_string())
        );
    }

    #[test]
    fn wrong_key_fails_as_untrusted_publisher() {
        let img = image();
        let (m, block, _) = signed(&img);
        let other = key(2).to_public();
        assert_eq!(
            verify_image(&m, &block, &img, &other),
            Err(MSG_UNSIGNED.to_string())
        );
    }

    #[test]
    fn altered_manifest_fails_signature() {
        let img = image();
        let (mut m, block, trusted) = signed(&img);
        m.version = "9.9.9".to_string();
        assert_eq!(
            verify_image(&m, &block, &img, &trusted),
            Err(MSG_UNSIGNED.to_string())
        );
    }

    #[test]
    fn truncated_image_fails_incomplete() {
        let img = image();
        let (m, block, trusted) = signed(&img);
        assert_eq!(
            verify_image(&m, &block, &img[..img.len() - 1], &trusted),
            Err(MSG_INCOMPLETE.to_string())
        );
    }

    #[test]
    fn manifest_is_small() {
        let m = make_manifest("kahf-fw", "1.0.0", &image()).unwrap();
        // 1+7 product, 1+5 version, 8 length, 32 hash
        assert_eq!(encode_manifest(&m).len(), 54);
    }

    #[test]
    fn oversized_fields_are_rejected() {
        let long = "x".repeat(256);
        assert!(make_manifest(&long, "1.0.0", &[]).is_err());
        assert!(make_manifest("kahf-fw", &long, &[]).is_err());
    }
}
