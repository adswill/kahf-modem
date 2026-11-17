//! Session layer: chat, mail and file transfer carried as link chunks.
//!
//! Chunk layout (at most 36 bytes on the wire):
//! - byte 0: message id, the first byte of SHA-256 over the message stream
//! - byte 1: chunk index, 0-based
//! - byte 2: total chunks minus one (so 1..=256 chunks are possible)
//! - bytes 3..n-2: data, at most 31 bytes
//! - last 2 bytes: CRC-16/CCITT-FALSE over everything before it, big endian
//!
//! Header size note: the brief asked for a 1-byte header with 33 data bytes. One byte cannot
//! carry message id, index, total and kind together, so the smallest workable header is 3 bytes.
//! That leaves 31 data bytes per chunk, and a message is limited to 256 chunks (about 7.9 KB).
//!
//! Message stream (reassembled, then split into chunks): one meta byte, then the body.
//! - meta bit 0: body is zstd-compressed (level 9); set only when compression made it smaller
//! - meta bits 1..2: kind (0 chat, 1 mail, 2 file)
//!
//! Body fields: strings are a u16 length plus UTF-8 bytes. A file body adds a 32-byte SHA-256
//! of its data, then a u32 length and the data. decode checks the CRC of every chunk and the
//! SHA-256 of every file.
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Chat { from: String, to: String, text: String },
    Mail { from: String, to: String, subject: String, body: String, attachments: Vec<String> },
    File { from: String, to: String, name: String, data: Vec<u8> },
}

/// Maximum bytes of one chunk on the link.
pub const CHUNK_MAX: usize = 36;
const HEADER: usize = 3;
const CRC_LEN: usize = 2;
/// Data bytes per chunk (36 - 3 header - 2 CRC).
pub const CHUNK_DATA: usize = CHUNK_MAX - HEADER - CRC_LEN;
/// Most chunks one message may use (the total field is one byte, minus one).
pub const MAX_CHUNKS: usize = 256;
const ZSTD_LEVEL: i32 = 9;
const KIND_CHAT: u8 = 0;
const KIND_MAIL: u8 = 1;
const KIND_FILE: u8 = 2;

/// Serialise a message into link payload chunks (each <= 36 bytes). Compresses with zstd.
/// Panics if a string exceeds its length field or the message needs more than 256 chunks.
pub fn encode(msg: &Message) -> Vec<Vec<u8>> {
    encode_with(msg, true)
}

/// As `encode`, with compression switched on or off.
pub fn encode_with(msg: &Message, compress: bool) -> Vec<Vec<u8>> {
    let kind = match msg {
        Message::Chat { .. } => KIND_CHAT,
        Message::Mail { .. } => KIND_MAIL,
        Message::File { .. } => KIND_FILE,
    };
    let body = serialise(msg);
    let compressed = if compress {
        zstd::encode_all(&body[..], ZSTD_LEVEL).ok().filter(|z| z.len() < body.len())
    } else {
        None
    };
    let (flag, payload) = match compressed {
        Some(z) => (1u8, z),
        None => (0u8, body),
    };
    let mut stream = Vec::with_capacity(payload.len() + 1);
    stream.push(flag | (kind << 1));
    stream.extend_from_slice(&payload);
    to_chunks(&stream)
}

/// Reassemble chunks; returns Some(message) once complete and verified.
/// Chunks may come in any order and may repeat. Chunks with a bad CRC are ignored.
/// If chunks from several messages are mixed, the first complete, verified message wins.
pub fn decode(chunks: &[Vec<u8>]) -> Option<Message> {
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
    assert!(n <= MAX_CHUNKS, "session: message needs {n} chunks, limit is {MAX_CHUNKS}");
    stream
        .chunks(CHUNK_DATA)
        .enumerate()
        .map(|(i, data)| {
            let mut c = Vec::with_capacity(CHUNK_MAX);
            c.push(id);
            c.push(i as u8);
            c.push((n - 1) as u8);
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
    let index = covered[1] as usize;
    let total = covered[2] as usize + 1;
    if index >= total {
        return None;
    }
    Some(Parsed { id: covered[0], index, total, data: &covered[HEADER..] })
}

fn decode_group(id: u8, valid: &[Parsed<'_>]) -> Option<Message> {
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
    let (&meta, rest) = stream.split_first()?;
    let body = if meta & 1 == 1 { zstd::decode_all(rest).ok()? } else { rest.to_vec() };
    parse_body((meta >> 1) & 3, &body)
}

fn serialise(msg: &Message) -> Vec<u8> {
    let mut out = Vec::new();
    match msg {
        Message::Chat { from, to, text } => {
            put_str(&mut out, from);
            put_str(&mut out, to);
            put_str(&mut out, text);
        }
        Message::Mail { from, to, subject, body, attachments } => {
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
        KIND_CHAT => Message::Chat { from: r.string()?, to: r.string()?, text: r.string()? },
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
            Message::Mail { from, to, subject, body: text, attachments }
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
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
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
        Message::Chat { from: "KA1ABC".into(), to: "KA2XYZ".into(), text: "hello over the link".into() }
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

    #[test]
    fn session_oversize_file_panics() {
        let msg = file(8000);
        let result = std::panic::catch_unwind(|| encode_with(&msg, false));
        assert!(result.is_err());
    }
}
