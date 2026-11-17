//! Session layer: chat, mail and file transfer carried as link chunks.
//!
//! Chunk layout (at most 34 bytes on the wire, the link's DATA body size):
//! - byte 0: message id, the first byte of SHA-256 over the message stream
//! - bytes 1..=2: chunk index, 0-based, u16 big endian
//! - bytes 3..=4: total chunks minus one, u16 big endian (so 1..=65536 chunks are possible)
//! - bytes 5..n-2: data, at most 27 bytes
//! - last 2 bytes: CRC-16/CCITT-FALSE over everything before it, big endian
//!
//! Header size note: the first version used 1-byte counters (3-byte header, 31 data bytes), which
//! capped a message at 256 chunks (about 7.9 KB). Two-byte counters raise the header to 5 bytes
//! and the cap to 65536 chunks (about 1.9 MB), so larger files and mail with attachments fit in
//! one message. Each chunk carries 27 data bytes.
//!
//! The link splits any payload longer than its DATA body (34 bytes) into separate frames and
//! delivers the pieces separately, so a session chunk must fit in one link body. The first
//! 36-byte version lost every full chunk on the link, which is why an 18-byte message never
//! arrived while an 8-byte one (one short chunk) did.
//!
//! Message stream (reassembled, then split into chunks): one meta byte, then the body.
//! - meta bit 0: body is zstd-compressed (level 9); set only when compression made it smaller
//! - meta bits 1..2: kind (0 chat, 1 mail, 2 file)
//! - meta bit 3: private to the addressee (see `encode_signed`)
//! - meta bit 4: signed; an `auth` block follows the meta byte, before the (possibly compressed) body
//!
//! Body fields: strings are a u16 length plus UTF-8 bytes. A file body adds a 32-byte SHA-256
//! of its data, then a u32 length and the data. decode checks the CRC of every chunk and the
//! SHA-256 of every file.
//!
//! Signatures: an optional Ed25519 signature (`auth` module) covers the sender callsign and the
//! uncompressed body. Signing does not hide anything; `Signed::verified` only says the content
//! was signed by its sender. Private is a routing rule for the app's inbox, NOT encryption:
//! the content is sent in the clear and any receiver can read it. The private bit is not
//! covered by the signature.
use crate::auth::{self, StationKey};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Chat {
        from: String,
        to: String,
        text: String,
    },
    Mail {
        from: String,
        to: String,
        subject: String,
        body: String,
        attachments: Vec<String>,
    },
    File {
        from: String,
        to: String,
        name: String,
        data: Vec<u8>,
    },
}

/// Maximum bytes of one chunk on the link: one link DATA body, so the link never splits it.
pub const CHUNK_MAX: usize = crate::link::MAX_CHUNK;
const HEADER: usize = 5;
const CRC_LEN: usize = 2;
/// Data bytes per chunk (34 - 5 header - 2 CRC).
pub const CHUNK_DATA: usize = CHUNK_MAX - HEADER - CRC_LEN;
/// Most chunks one message may use (the total field is two bytes, minus one).
pub const MAX_CHUNKS: usize = 65536;
/// Largest message stream in bytes (meta byte plus body), about 1.9 MB.
pub const MAX_MESSAGE: usize = MAX_CHUNKS * CHUNK_DATA;
const ZSTD_LEVEL: i32 = 9;
const KIND_CHAT: u8 = 0;
const KIND_MAIL: u8 = 1;
const KIND_FILE: u8 = 2;
/// Meta bit 3: the message is private to its addressee (app routing rule, not encryption).
const META_PRIVATE: u8 = 0x08;
/// Meta bit 4: an auth block follows the meta byte.
const META_SIGNED: u8 = 0x10;
/// Auth block bytes after the callsign: 32-byte Ed25519 public key and 64-byte signature.
const AUTH_FIXED: usize = 32 + 64;

/// Serialise a message into link payload chunks (each <= CHUNK_MAX bytes). Compresses with zstd.
/// Panics if a string exceeds its length field or the message needs more than 65536 chunks.
pub fn encode(msg: &Message) -> Vec<Vec<u8>> {
    encode_with(msg, true)
}

/// As `encode`, with compression switched on or off.
pub fn encode_with(msg: &Message, compress: bool) -> Vec<Vec<u8>> {
    encode_inner(msg, compress, None, false)
}

/// Serialise a message with an optional signature and an optional private flag.
///
/// With `key`, the stream carries an `auth` block signing the sender (`from`) and the body.
/// With `private`, the app delivers the message only to the addressee's inbox. This is a
/// routing rule, NOT encryption: the content is sent in the clear and stays readable on air.
/// Panics if the sender is longer than 255 bytes and `key` is given.
pub fn encode_signed(msg: &Message, key: Option<&StationKey>, private: bool) -> Vec<Vec<u8>> {
    encode_inner(msg, true, key, private)
}

fn encode_inner(msg: &Message, compress: bool, key: Option<&StationKey>, private: bool) -> Vec<Vec<u8>> {
    let kind = match msg {
        Message::Chat { .. } => KIND_CHAT,
        Message::Mail { .. } => KIND_MAIL,
        Message::File { .. } => KIND_FILE,
    };
    let body = serialise(msg);
    let auth_block = key.map(|k| auth::sign(k, sender(msg), &body));
    let compressed = if compress {
        zstd::encode_all(&body[..], ZSTD_LEVEL)
            .ok()
            .filter(|z| z.len() < body.len())
    } else {
        None
    };
    let (flag, payload) = match compressed {
        Some(z) => (1u8, z),
        None => (0u8, body),
    };
    let mut meta = flag | (kind << 1);
    if private {
        meta |= META_PRIVATE;
    }
    if auth_block.is_some() {
        meta |= META_SIGNED;
    }
    let mut stream = Vec::with_capacity(payload.len() + 1);
    stream.push(meta);
    if let Some(block) = &auth_block {
        stream.extend_from_slice(block);
    }
    stream.extend_from_slice(&payload);
    to_chunks(&stream)
}

/// A decoded message with its signature and routing status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signed {
    pub message: Message,
    /// Callsign that signed the message. Set only when `verified` is true.
    pub signer: Option<String>,
    /// True only when a signature checks out against this exact content and the signer is
    /// the message's `from` station. Unsigned and altered messages are false.
    pub verified: bool,
    /// Private to the addressee: the app delivers it only to that station's inbox.
    /// A routing rule, not encryption; the content is readable on air.
    pub private: bool,
}

/// Reassemble chunks; returns Some(message) once complete and verified.
/// Chunks may come in any order and may repeat. Chunks with a bad CRC are ignored.
/// If chunks from several messages are mixed, the first complete, verified message wins.
/// Signatures are not checked here; use `decode_signed` for that. Works on unsigned and signed messages.
pub fn decode(chunks: &[Vec<u8>]) -> Option<Message> {
    decode_full(chunks).map(|d| d.message)
}

/// As `decode`, and also reports the signature status and the private flag.
pub fn decode_signed(chunks: &[Vec<u8>]) -> Option<Signed> {
    let d = decode_full(chunks)?;
    let signer = d
        .auth
        .as_ref()
        .and_then(|block| verify_signature(&d.message, block));
    Some(Signed {
        verified: signer.is_some(),
        signer,
        message: d.message,
        private: d.private,
    })
}

/// Check an `auth` block against a message's content. Returns the signer callsign only when
/// the signature covers this exact message and the signer is the message's `from` station.
/// Any change to the body, the sender or the signature returns None.
pub fn verify_signature(msg: &Message, block: &[u8]) -> Option<String> {
    let body = serialise(msg);
    let (callsign, _public) = auth::verify(block, &body).ok()?;
    (callsign == sender(msg)).then_some(callsign)
}

fn sender(msg: &Message) -> &str {
    match msg {
        Message::Chat { from, .. } | Message::Mail { from, .. } | Message::File { from, .. } => from,
    }
}

/// A reassembled message before the signature check.
struct Decoded {
    message: Message,
    auth: Option<Vec<u8>>,
    private: bool,
}

fn decode_full(chunks: &[Vec<u8>]) -> Option<Decoded> {
    let valid: Vec<Parsed<'_>> = chunks.iter().filter_map(|c| parse_chunk(c)).collect();
    let mut ids: Vec<u8> = valid.iter().map(|c| c.id).collect();
    ids.sort_unstable();
    ids.dedup();
    ids.into_iter().find_map(|id| decode_group(id, &valid))
}

/// One chunk that passed its CRC: id, index, total and data.
struct Parsed<'a> {
    id: u8,
    index: usize,
    total: usize,
    data: &'a [u8],
}

fn to_chunks(stream: &[u8]) -> Vec<Vec<u8>> {
    let id = Sha256::digest(stream).as_slice()[0];
    let n = stream.len().div_ceil(CHUNK_DATA);
    assert!(
        n <= MAX_CHUNKS,
        "session: message needs {n} chunks, limit is {MAX_CHUNKS}"
    );
    stream
        .chunks(CHUNK_DATA)
        .enumerate()
        .map(|(i, data)| {
            let mut c = Vec::with_capacity(CHUNK_MAX);
            c.push(id);
            c.extend_from_slice(&(i as u16).to_be_bytes());
            c.extend_from_slice(&((n - 1) as u16).to_be_bytes());
            c.extend_from_slice(data);
            let crc = crc16(&c);
            c.extend_from_slice(&crc.to_be_bytes());
            c
        })
        .collect()
}

fn parse_chunk(c: &[u8]) -> Option<Parsed<'_>> {
    if c.len() <= HEADER + CRC_LEN || c.len() > CHUNK_MAX {
        return None;
    }
    let (covered, crc) = c.split_at(c.len() - CRC_LEN);
    if crc16(covered) != u16::from_be_bytes([crc[0], crc[1]]) {
        return None;
    }
    let index = u16::from_be_bytes([covered[1], covered[2]]) as usize;
    let total = u16::from_be_bytes([covered[3], covered[4]]) as usize + 1;
    if index >= total {
        return None;
    }
    Some(Parsed {
        id: covered[0],
        index,
        total,
        data: &covered[HEADER..],
    })
}

fn decode_group(id: u8, valid: &[Parsed<'_>]) -> Option<Decoded> {
    let mine: Vec<&Parsed<'_>> = valid.iter().filter(|c| c.id == id).collect();
    let total = mine.first()?.total;
    if mine.iter().any(|c| c.total != total) {
        return None;
    }
    let mut parts: Vec<Option<&[u8]>> = vec![None; total];
    for c in &mine {
        if parts[c.index].is_none() {
            parts[c.index] = Some(c.data);
        }
    }
    let mut stream = Vec::new();
    for p in parts {
        stream.extend_from_slice(p?);
    }
    let (&meta, mut rest) = stream.split_first()?;
    let auth = if meta & META_SIGNED != 0 {
        // The auth block is self-delimiting: its first byte is the callsign length.
        let n = *rest.first()? as usize;
        let len = 1 + n + AUTH_FIXED;
        let block = rest.get(..len)?.to_vec();
        rest = &rest[len..];
        Some(block)
    } else {
        None
    };
    let body = if meta & 1 == 1 {
        zstd::decode_all(rest).ok()?
    } else {
        rest.to_vec()
    };
    let message = parse_body((meta >> 1) & 3, &body)?;
    Some(Decoded {
        message,
        auth,
        private: meta & META_PRIVATE != 0,
    })
}

fn serialise(msg: &Message) -> Vec<u8> {
    let mut out = Vec::new();
    match msg {
        Message::Chat { from, to, text } => {
            put_str(&mut out, from);
            put_str(&mut out, to);
            put_str(&mut out, text);
        }
        Message::Mail {
            from,
            to,
            subject,
            body,
            attachments,
        } => {
            put_str(&mut out, from);
            put_str(&mut out, to);
            put_str(&mut out, subject);
            put_str(&mut out, body);
            let count = u16::try_from(attachments.len()).expect("session: too many attachments");
            out.extend_from_slice(&count.to_le_bytes());
            for a in attachments {
                put_str(&mut out, a);
            }
        }
        Message::File { from, to, name, data } => {
            put_str(&mut out, from);
            put_str(&mut out, to);
            put_str(&mut out, name);
            out.extend_from_slice(Sha256::digest(data).as_slice());
            let len = u32::try_from(data.len()).expect("session: file too large");
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(data);
        }
    }
    out
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    let len = u16::try_from(s.len()).expect("session: string longer than 65535 bytes");
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let s = self.b.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }
    fn u16(&mut self) -> Option<usize> {
        let b = self.take(2)?;
        Some(u16::from_le_bytes([b[0], b[1]]) as usize)
    }
    fn u32(&mut self) -> Option<usize> {
        let b = self.take(4)?;
        Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    }
    fn string(&mut self) -> Option<String> {
        let n = self.u16()?;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
}

fn parse_body(kind: u8, body: &[u8]) -> Option<Message> {
    let mut r = Reader { b: body, pos: 0 };
    let msg = match kind {
        KIND_CHAT => Message::Chat {
            from: r.string()?,
            to: r.string()?,
            text: r.string()?,
        },
        KIND_MAIL => {
            let from = r.string()?;
            let to = r.string()?;
            let subject = r.string()?;
            let text = r.string()?;
            let n = r.u16()?;
            let mut attachments = Vec::new();
            for _ in 0..n {
                attachments.push(r.string()?);
            }
            Message::Mail {
                from,
                to,
                subject,
                body: text,
                attachments,
            }
        }
        KIND_FILE => {
            let from = r.string()?;
            let to = r.string()?;
            let name = r.string()?;
            let want = r.take(32)?;
            let len = r.u32()?;
            let data = r.take(len)?.to_vec();
            if Sha256::digest(&data).as_slice() != want {
                return None;
            }
            Message::File { from, to, name, data }
        }
        _ => return None,
    };
    (r.pos == body.len()).then_some(msg)
}

/// CRC-16/CCITT-FALSE: poly 0x1021, init 0xFFFF, no reflection, no final XOR.
fn crc16(bytes: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in bytes {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(s: &mut u32) -> u32 {
        *s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        *s
    }

    fn random_bytes(n: usize, seed: u32) -> Vec<u8> {
        let mut s = seed;
        (0..n).map(|_| (lcg(&mut s) >> 24) as u8).collect()
    }

    fn shuffle<T>(v: &mut [T], seed: u32) {
        let mut s = seed;
        for i in (1..v.len()).rev() {
            let j = lcg(&mut s) as usize % (i + 1);
            v.swap(i, j);
        }
    }

    fn check_sizes(chunks: &[Vec<u8>]) {
        assert!(chunks.iter().all(|c| c.len() <= CHUNK_MAX));
        assert!(chunks.len() <= MAX_CHUNKS);
    }

    fn chat() -> Message {
        Message::Chat {
            from: "KA1ABC".into(),
            to: "KA2XYZ".into(),
            text: "hello over the link".into(),
        }
    }

    fn mail() -> Message {
        Message::Mail {
            from: "a@kahf".into(),
            to: "b@kahf".into(),
            subject: "Field report".into(),
            body: "Signal was strong at noon. Photos attached.\n".repeat(4),
            attachments: vec!["photo1.jpg".into(), "log.txt".into()],
        }
    }

    fn file(n: usize) -> Message {
        Message::File {
            from: "a".into(),
            to: "b".into(),
            name: "random.bin".into(),
            data: random_bytes(n, 7),
        }
    }

    #[test]
    fn session_chat_round_trip() {
        let msg = chat();
        let chunks = encode(&msg);
        check_sizes(&chunks);
        assert_eq!(decode(&chunks), Some(msg));
    }

    #[test]
    fn session_chat_uncompressed_round_trip() {
        let msg = chat();
        let chunks = encode_with(&msg, false);
        check_sizes(&chunks);
        assert_eq!(decode(&chunks), Some(msg));
    }

    #[test]
    fn session_mail_two_attachments_round_trip() {
        let msg = mail();
        let chunks = encode(&msg);
        check_sizes(&chunks);
        assert_eq!(decode(&chunks), Some(msg));
    }

    #[test]
    fn session_file_5kb_shuffled_round_trip() {
        let msg = file(5000);
        let mut chunks = encode(&msg);
        check_sizes(&chunks);
        shuffle(&mut chunks, 99);
        assert_eq!(decode(&chunks), Some(msg));
    }

    #[test]
    fn session_duplicate_chunks_allowed() {
        let msg = mail();
        let mut chunks = encode(&msg);
        let copy = chunks.clone();
        chunks.extend(copy);
        assert_eq!(decode(&chunks), Some(msg));
    }

    #[test]
    fn session_missing_chunk_returns_none() {
        let mut chunks = encode(&file(500));
        chunks.remove(2);
        assert_eq!(decode(&chunks), None);
    }

    #[test]
    fn session_corrupted_chunk_returns_none() {
        let mut chunks = encode(&file(500));
        // Flip one data bit: the CRC no longer matches, so the chunk is dropped.
        chunks[1][5] ^= 0x01;
        assert_eq!(decode(&chunks), None);
    }

    #[test]
    fn session_file_hash_mismatch_returns_none() {
        // Build a file stream whose data was altered after hashing, then re-chunk it with valid CRCs.
        let msg = file(300);
        let mut body = serialise(&msg);
        let last = body.len() - 1;
        body[last] ^= 0x01;
        let mut stream = vec![KIND_FILE << 1];
        stream.extend_from_slice(&body);
        assert_eq!(decode(&to_chunks(&stream)), None);
    }

    /// Bytes of a file message's fields other than its data (the stream adds one meta byte).
    fn file_overhead() -> usize {
        serialise(&file(0)).len()
    }

    /// A file whose uncompressed stream is exactly `stream_len` bytes.
    fn file_with_stream_len(stream_len: usize) -> Message {
        file(stream_len - 1 - file_overhead())
    }

    #[test]
    fn session_file_100kb_shuffled_round_trip() {
        let msg = file(100 * 1024);
        let mut chunks = encode(&msg);
        assert!(chunks.len() > 256, "must use more than the old 256-chunk cap");
        check_sizes(&chunks);
        shuffle(&mut chunks, 4242);
        assert_eq!(decode(&chunks), Some(msg));
    }

    #[test]
    fn session_file_7_9kb_round_trip() {
        // 256 chunks of 31 bytes was the old cap (7936 bytes); this is exactly that size.
        let msg = file_with_stream_len(256 * 31);
        let chunks = encode(&msg);
        check_sizes(&chunks);
        assert_eq!(chunks.len(), (256 * 31usize).div_ceil(CHUNK_DATA));
        assert_eq!(decode(&chunks), Some(msg));
    }

    #[test]
    fn session_file_7_9kb_plus_one_byte_round_trip() {
        let msg = file_with_stream_len(256 * 31 + 1);
        let mut chunks = encode(&msg);
        check_sizes(&chunks);
        shuffle(&mut chunks, 31);
        assert_eq!(decode(&chunks), Some(msg));
    }

    #[test]
    fn session_max_size_message_round_trip() {
        let msg = file_with_stream_len(MAX_MESSAGE);
        let chunks = encode_with(&msg, false);
        assert_eq!(chunks.len(), MAX_CHUNKS);
        check_sizes(&chunks);
        assert_eq!(decode(&chunks), Some(msg));
    }

    #[test]
    fn session_corrupted_chunk_in_large_file_returns_none() {
        let mut chunks = encode(&file(100 * 1024));
        // Flip one data bit in the middle of the message: the CRC fails, the chunk is dropped.
        chunks[1000][10] ^= 0x04;
        assert_eq!(decode(&chunks), None);
    }

    /// Carry chunks across a loopback link pair (A sends, B receives) and return what B's
    /// link delivered to the session layer, in order.
    fn over_link(chunks: &[Vec<u8>]) -> Vec<Vec<u8>> {
        use crate::link::{Action, Link};
        use std::collections::VecDeque;

        fn step(
            acts: Vec<Action>,
            sender_is_a: bool,
            wire: &mut VecDeque<(bool, Vec<u8>)>,
            rx: &mut Vec<Vec<u8>>,
        ) {
            for act in acts {
                match act {
                    Action::Transmit(p) => wire.push_back((sender_is_a, p)),
                    Action::Deliver(p) if !sender_is_a => rx.push(p),
                    _ => {}
                }
            }
        }

        let mut a = Link::new("AA1AAA");
        let mut b = Link::new("BB1BBB");
        let mut wire = VecDeque::new();
        let mut rx = Vec::new();
        step(a.connect("BB1BBB"), true, &mut wire, &mut rx);
        while let Some((from_a, p)) = wire.pop_front() {
            let acts = if from_a {
                b.on_receive(&p)
            } else {
                a.on_receive(&p)
            };
            step(acts, !from_a, &mut wire, &mut rx);
        }
        assert!(matches!(a.state, crate::link::LinkState::Connected { .. }));
        for c in chunks {
            step(a.send(c), true, &mut wire, &mut rx);
            while let Some((from_a, p)) = wire.pop_front() {
                let acts = if from_a {
                    b.on_receive(&p)
                } else {
                    a.on_receive(&p)
                };
                step(acts, !from_a, &mut wire, &mut rx);
            }
        }
        rx
    }

    /// 18 bytes of text: the stream is 37 bytes (meta, three strings), so the first chunk
    /// is full. A full chunk must reach the link as one frame and arrive intact.
    #[test]
    fn session_18_byte_text_survives_link_round_trip() {
        let msg = Message::Chat {
            from: "AA1AAA".into(),
            to: "BB1BBB".into(),
            text: "sent while offline".into(),
        };
        assert_eq!("sent while offline".len(), 18);
        let chunks = encode(&msg);
        assert_eq!(decode(&chunks), Some(msg.clone()));
        let received = over_link(&chunks);
        assert_eq!(received, chunks, "link must deliver each chunk whole");
        assert_eq!(decode(&received), Some(msg));
    }

    #[test]
    fn session_over_max_message_panics() {
        let msg = file_with_stream_len(MAX_MESSAGE + 1);
        let result = std::panic::catch_unwind(|| encode_with(&msg, false));
        assert!(result.is_err());
    }

    fn signing_key() -> StationKey {
        StationKey::from_bytes(&[7u8; 32])
    }

    /// Build a stream by hand (meta, optional auth block, body) so tests can tamper with it
    /// while keeping every chunk CRC valid.
    fn raw_stream(meta: u8, auth_block: &[u8], body: &[u8]) -> Vec<Vec<u8>> {
        let mut stream = vec![meta];
        stream.extend_from_slice(auth_block);
        stream.extend_from_slice(body);
        to_chunks(&stream)
    }

    #[test]
    fn session_signed_message_verifies() {
        for msg in [chat(), mail()] {
            let chunks = encode_signed(&msg, Some(&signing_key()), false);
            check_sizes(&chunks);
            let got = decode_signed(&chunks).unwrap();
            assert_eq!(got.message, msg);
            assert!(got.verified);
            assert_eq!(got.signer.as_deref(), Some(sender(&msg)));
            assert!(!got.private);
            // Plain decode still returns the message for signed streams.
            assert_eq!(decode(&chunks), Some(msg));
        }
    }

    #[test]
    fn session_signed_changed_body_fails() {
        let msg = chat();
        let auth_block = auth::sign(&signing_key(), "KA1ABC", &serialise(&msg));
        let mut tampered = msg.clone();
        if let Message::Chat { text, .. } = &mut tampered {
            *text = "hello over the lint".into();
        }
        let chunks = raw_stream(META_SIGNED | (KIND_CHAT << 1), &auth_block, &serialise(&tampered));
        let got = decode_signed(&chunks).unwrap();
        assert_eq!(got.message, tampered);
        assert!(!got.verified);
        assert_eq!(got.signer, None);
        assert_eq!(verify_signature(&tampered, &auth_block), None);
        assert_eq!(verify_signature(&msg, &auth_block), Some("KA1ABC".into()));
    }

    #[test]
    fn session_signature_from_other_station_fails() {
        // Valid signature by KA9ZZZ over a message sent from KA1ABC: not the sender, so unverified.
        let msg = chat();
        let auth_block = auth::sign(&signing_key(), "KA9ZZZ", &serialise(&msg));
        let chunks = raw_stream(META_SIGNED | (KIND_CHAT << 1), &auth_block, &serialise(&msg));
        let got = decode_signed(&chunks).unwrap();
        assert!(!got.verified);
        assert_eq!(got.signer, None);
    }

    #[test]
    fn session_unsigned_message_decodes_as_before() {
        let msg = chat();
        let chunks = encode(&msg);
        assert_eq!(decode(&chunks), Some(msg.clone()));
        let got = decode_signed(&chunks).unwrap();
        assert_eq!(
            got,
            Signed {
                message: msg,
                signer: None,
                verified: false,
                private: false,
            }
        );
    }

    #[test]
    fn session_private_flag_round_trips() {
        let msg = chat();
        let plain = decode_signed(&encode_signed(&msg, None, true)).unwrap();
        assert!(plain.private);
        assert!(!plain.verified);
        assert_eq!(plain.message, msg);

        let both = decode_signed(&encode_signed(&msg, Some(&signing_key()), true)).unwrap();
        assert!(both.private);
        assert!(both.verified);
        assert_eq!(both.message, msg);

        assert!(!decode_signed(&encode(&msg)).unwrap().private);
    }
}
