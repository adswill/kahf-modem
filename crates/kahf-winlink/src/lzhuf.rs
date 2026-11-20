//! LZHUF compression as used by the binary FBB protocols (B2F variant with a CRC16 in front).
//!
//! The stream is: CRC16 (2 bytes, little endian) over everything after it, the uncompressed
//! length (4 bytes, little endian), then the LZSS + adaptive Huffman bit stream. The algorithm
//! is Haruyasu Yoshizaki's LZHUF; this port follows the structure of the MIT-licensed
//! `wl2k-go` implementation (Copyright 2016 Martin Hebnes Pedersen, LA5NTA), which Winlink
//! Express and the CMS are known to interoperate with.

use crate::tables::{CRC16_TAB, D_CODE, D_LEN, P_CODE, P_LEN};
use std::fmt;

const N: usize = 2048; // ring buffer size
const F: usize = 60; // look-ahead size
const NIL: usize = N; // end of tree
const THRESHOLD: usize = 2;
const NUM_CHAR: usize = 256 - THRESHOLD + F;
const T: usize = NUM_CHAR * 2 - 1;
const R: usize = T - 1;
const MAX_FREQ: u32 = 0x8000;

/// Why a compressed stream could not be decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The stream is shorter than its header or ends early.
    Truncated,
    /// The CRC16 does not match the data.
    Checksum,
    /// The declared size is larger than this decoder accepts.
    TooLarge,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Error::Truncated => "compressed data is truncated",
            Error::Checksum => "compressed data failed its checksum",
            Error::TooLarge => "compressed data declares an oversized message",
        })
    }
}

impl std::error::Error for Error {}

/// Largest uncompressed size accepted when decoding (Winlink limits messages to about 120 kB;
/// this leaves room).
pub const MAX_DECODED: usize = 4 * 1024 * 1024;

fn crc_step(sum: u16, byte: u8) -> u16 {
    (sum << 8) ^ CRC16_TAB[(sum >> 8) as usize] ^ u16::from(byte)
}

/// CRC16 (XMODEM variant, two zero bytes appended) used in front of B2F compressed data.
pub fn crc16(data: &[u8]) -> u16 {
    let mut sum = 0u16;
    for &b in data.iter().chain([0u8, 0u8].iter()) {
        sum = crc_step(sum, b);
    }
    sum
}

struct Model {
    freq: [u32; T + 1],
    prnt: [usize; T + NUM_CHAR],
    son: [usize; T],
}

impl Model {
    fn new() -> Self {
        let mut m = Model {
            freq: [0; T + 1],
            prnt: [0; T + NUM_CHAR],
            son: [0; T],
        };
        for i in 0..NUM_CHAR {
            m.freq[i] = 1;
            m.son[i] = i + T;
            m.prnt[i + T] = i;
        }
        let (mut i, mut j) = (0, NUM_CHAR);
        while j <= R {
            m.freq[j] = m.freq[i] + m.freq[i + 1];
            m.son[j] = i;
            m.prnt[i] = j;
            m.prnt[i + 1] = j;
            i += 2;
            j += 1;
        }
        m.freq[T] = 0xffff;
        m.prnt[R] = 0;
        m
    }

    fn reconst(&mut self) {
        let mut j = 0;
        for i in 0..T {
            if self.son[i] >= T {
                self.freq[j] = self.freq[i].div_ceil(2);
                self.son[j] = self.son[i];
                j += 1;
            }
        }
        let (mut i, mut j) = (0, NUM_CHAR);
        while j < T {
            let k = i + 1;
            self.freq[j] = self.freq[i] + self.freq[k];
            let first = self.freq[j];
            let mut k = j;
            while first < self.freq[k - 1] {
                k -= 1;
            }
            let last = j - k;
            self.freq.copy_within(k..k + last, k + 1);
            self.freq[k] = first;
            self.son.copy_within(k..k + last, k + 1);
            self.son[k] = i;
            i += 2;
            j += 1;
        }
        for i in 0..T {
            let k = self.son[i];
            if k >= T {
                self.prnt[k] = i;
            } else {
                self.prnt[k + 1] = i;
                self.prnt[k] = i;
            }
        }
    }

    fn update(&mut self, c: usize) {
        if self.freq[R] == MAX_FREQ {
            self.reconst();
        }
        let mut c = self.prnt[c + T];
        loop {
            self.freq[c] += 1;
            let k = self.freq[c];
            if k <= self.freq[c + 1] || self.freq.len() <= c + 2 {
                c = self.prnt[c];
                if c == 0 {
                    break;
                }
                continue;
            }
            let mut l = c + 1;
            while k > self.freq[l + 1] {
                l += 1;
            }
            self.freq[c] = self.freq[l];
            self.freq[l] = k;

            let i = self.son[c];
            self.prnt[i] = l;
            if i < T {
                self.prnt[i + 1] = l;
            }
            let j = self.son[l];
            self.son[l] = i;
            self.prnt[j] = c;
            if j < T {
                self.prnt[j + 1] = c;
            }
            self.son[c] = j;

            c = self.prnt[l];
            if c == 0 {
                break;
            }
        }
    }
}

struct Encoder {
    model: Model,
    dad: [usize; N + 1],
    lson: [usize; N + 1],
    rson: [usize; N + 257],
    text: [u8; N + F - 1],
    match_len: usize,
    match_pos: usize,
    out: Vec<u8>,
    putbuf: u32,
    putlen: u32,
}

impl Encoder {
    fn new() -> Box<Self> {
        // dad and rson start as NIL (the roots and nodes are unused), as the reference InitTree sets them.
        Box::new(Encoder {
            model: Model::new(),
            dad: [NIL; N + 1],
            lson: [NIL; N + 1],
            rson: [NIL; N + 257],
            text: [b' '; N + F - 1],
            match_len: 0,
            match_pos: 0,
            out: Vec::new(),
            putbuf: 0,
            putlen: 0,
        })
    }

    fn delete_node(&mut self, p: usize) {
        if self.dad[p] == NIL {
            return;
        }
        let q = if self.rson[p] == NIL {
            self.lson[p]
        } else if self.lson[p] == NIL {
            self.rson[p]
        } else {
            let mut qq = self.lson[p];
            if self.rson[qq] != NIL {
                while self.rson[qq] != NIL {
                    qq = self.rson[qq];
                }
                self.rson[self.dad[qq]] = self.lson[qq];
                self.dad[self.lson[qq]] = self.dad[qq];
                self.lson[qq] = self.lson[p];
                self.dad[self.lson[p]] = qq;
            }
            self.rson[qq] = self.rson[p];
            self.dad[self.rson[p]] = qq;
            qq
        };
        self.dad[q] = self.dad[p];
        if self.rson[self.dad[p]] == p {
            self.rson[self.dad[p]] = q;
        } else {
            self.lson[self.dad[p]] = q;
        }
        self.dad[p] = NIL;
    }

    fn insert_node(&mut self, r: usize) {
        let mut cmp: i32 = 1;
        let mut p = N + 1 + self.text[r] as usize;
        self.rson[r] = NIL;
        self.lson[r] = NIL;
        self.match_len = 0;
        loop {
            if cmp >= 0 {
                if self.rson[p] != NIL {
                    p = self.rson[p];
                } else {
                    self.rson[p] = r;
                    self.dad[r] = p;
                    return;
                }
            } else if self.lson[p] != NIL {
                p = self.lson[p];
            } else {
                self.lson[p] = r;
                self.dad[r] = p;
                return;
            }
            let mut i = 1;
            while i < F {
                cmp = i32::from(self.text[r + i]) - i32::from(self.text[p + i]);
                if cmp != 0 {
                    break;
                }
                i += 1;
            }
            if i > THRESHOLD {
                if i > self.match_len {
                    self.match_pos = (r.wrapping_sub(p) & (N - 1)) - 1;
                    self.match_len = i;
                    if self.match_len >= F {
                        break;
                    }
                }
                if i == self.match_len {
                    let c = (r.wrapping_sub(p) & (N - 1)) - 1;
                    if c < self.match_pos {
                        self.match_pos = c;
                    }
                }
            }
        }
        self.dad[r] = self.dad[p];
        self.lson[r] = self.lson[p];
        self.rson[r] = self.rson[p];
        self.dad[self.lson[p]] = r;
        self.dad[self.rson[p]] = r;
        if self.rson[self.dad[p]] == p {
            self.rson[self.dad[p]] = r;
        } else {
            self.lson[self.dad[p]] = r;
        }
        self.dad[p] = NIL;
    }

    fn put_code(&mut self, l: u32, c: u32) {
        self.putbuf |= c >> self.putlen;
        self.putlen += l;
        if self.putlen < 8 {
            return;
        }
        self.out.push((self.putbuf >> 8) as u8);
        self.putlen -= 8;
        if self.putlen >= 8 {
            self.out.push(self.putbuf as u8);
            self.putlen -= 8;
            self.putbuf = (c << (l - self.putlen)) & 0xffff;
        } else {
            self.putbuf = (self.putbuf << 8) & 0xffff;
        }
    }

    fn encode_char(&mut self, c: usize) {
        let mut i: u32 = 0;
        let mut j: u32 = 0;
        let mut k = self.model.prnt[c + T];
        loop {
            i >>= 1;
            j += 1;
            if k & 1 != 0 {
                i += 0x8000;
            }
            k = self.model.prnt[k];
            if k == R {
                break;
            }
        }
        self.put_code(j, i);
        self.model.update(c);
    }

    fn encode_position(&mut self, c: usize) {
        let i = c >> 6;
        self.put_code(u32::from(P_LEN[i]), u32::from(P_CODE[i]) << 8);
        self.put_code(6, ((c & 0x3f) << 10) as u32);
    }
}

/// Compresses `data` into a B2F stream (CRC16, length, bit stream).
pub fn compress(data: &[u8]) -> Vec<u8> {
    let mut e = Encoder::new();
    let (mut r, mut s) = (N - F, 0usize);
    let mut len = 0usize;
    let mut last_match = 1usize;
    let mut idx = 0;
    // Pre-fill the look-ahead buffer.
    while len < F && idx < data.len() {
        e.text[r + len] = data[idx];
        idx += 1;
        len += 1;
        e.insert_node(r - len);
        last_match = 1;
    }
    let pre_filled = len == F;

    // One step of the main loop; `c` is None while draining the look-ahead at the end.
    macro_rules! advance {
        ($c:expr) => {{
            if let Some(c) = $c {
                e.text[s] = c;
                if s < F - 1 {
                    e.text[s + N] = c;
                }
                len += 1;
            }
            e.insert_node(r);
            last_match -= 1;
            if last_match == 0 {
                // encode()
                if len != 0 {
                    if e.match_len > len {
                        e.match_len = len;
                    }
                    if e.match_len <= THRESHOLD {
                        e.match_len = 1;
                        let ch = e.text[r] as usize;
                        e.encode_char(ch);
                    } else {
                        let sym = 255 - THRESHOLD + e.match_len;
                        e.encode_char(sym);
                        let pos = e.match_pos;
                        e.encode_position(pos);
                    }
                    last_match = e.match_len;
                }
            }
            e.delete_node(s);
            s = (s + 1) & (N - 1);
            r = (r + 1) & (N - 1);
            len -= 1;
        }};
    }

    if pre_filled {
        while idx < data.len() {
            advance!(Some(data[idx]));
            idx += 1;
        }
    }
    while len > 0 {
        advance!(None::<u8>);
    }
    // The final encode() has nothing left to flush; encodeEnd() writes the last partial byte.
    if e.putlen != 0 {
        let b = (e.putbuf >> 8) as u8;
        e.out.push(b);
    }

    let mut body = Vec::with_capacity(4 + e.out.len());
    body.extend_from_slice(&(data.len() as u32).to_le_bytes());
    body.extend_from_slice(&e.out);
    let mut out = Vec::with_capacity(2 + body.len());
    out.extend_from_slice(&crc16(&body).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    acc: u32,
    nbits: u32,
}

impl Bits<'_> {
    fn read(&mut self, n: u32) -> Option<u32> {
        while self.nbits < n {
            let b = *self.data.get(self.pos)?;
            self.pos += 1;
            self.acc = (self.acc << 8) | u32::from(b);
            self.nbits += 8;
        }
        let v = (self.acc >> (self.nbits - n)) & ((1 << n) - 1);
        self.nbits -= n;
        self.acc &= (1u32 << self.nbits) - 1;
        Some(v)
    }
}

/// Decompresses a B2F stream, checking its CRC16 and declared length.
pub fn decompress(stream: &[u8]) -> Result<Vec<u8>, Error> {
    if stream.len() < 6 {
        return Err(Error::Truncated);
    }
    let want = u16::from_le_bytes([stream[0], stream[1]]);
    let body = &stream[2..];
    if crc16(body) != want {
        return Err(Error::Checksum);
    }
    let size = u32::from_le_bytes([body[0], body[1], body[2], body[3]]) as usize;
    if size > MAX_DECODED {
        return Err(Error::TooLarge);
    }
    let mut bits = Bits {
        data: &body[4..],
        pos: 0,
        acc: 0,
        nbits: 0,
    };
    let mut model = Model::new();
    let mut text = [b' '; N];
    let mut r = N - F;
    let mut out = Vec::with_capacity(size);
    while out.len() < size {
        // decode_char
        let mut c = model.son[R];
        while c < T {
            c += bits.read(1).ok_or(Error::Truncated)? as usize;
            c = model.son[c];
        }
        c -= T;
        model.update(c);
        if c < 256 {
            out.push(c as u8);
            text[r] = c as u8;
            r = (r + 1) & (N - 1);
        } else {
            // decode_position
            let mut i = bits.read(8).ok_or(Error::Truncated)? as usize;
            let hi = (D_CODE[i] as usize) << 6;
            let mut j = u32::from(D_LEN[i]);
            j -= 2;
            while j > 0 {
                i = (i << 1) + bits.read(1).ok_or(Error::Truncated)? as usize;
                j -= 1;
            }
            let pos = hi | (i & 0x3f);
            let start = r.wrapping_sub(pos).wrapping_sub(1) & (N - 1);
            let count = c - 255 + THRESHOLD;
            for k in 0..count {
                let ch = text[(start + k) & (N - 1)];
                if out.len() < size {
                    out.push(ch);
                }
                text[r] = ch;
                r = (r + 1) & (N - 1);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(data: &[u8]) {
        let z = compress(data);
        assert_eq!(decompress(&z).unwrap(), data, "len {}", data.len());
    }

    #[test]
    fn empty_and_tiny() {
        round(b"");
        round(b"a");
        round(b"hello");
        round(&[0u8; 59]);
        round(&[7u8; 60]);
        round(&[7u8; 61]);
    }

    #[test]
    fn text_and_runs() {
        let text = "Mid: ABCDEFGH1234\r\nFrom: K1ABC\r\nSubject: test\r\n\r\nHello world. ".repeat(200);
        round(text.as_bytes());
        round(&vec![b' '; 5000]);
        round(&vec![0u8; 100_000]);
    }

    #[test]
    fn pseudo_random_binary() {
        let mut x = 0x1234_5678u32;
        let data: Vec<u8> = (0..30_000)
            .map(|_| {
                x = x.wrapping_mul(1664525).wrapping_add(1013904223);
                (x >> 24) as u8
            })
            .collect();
        round(&data);
    }

    #[test]
    fn detects_corruption() {
        let mut z = compress(b"some message body, some message body, some message body");
        let last = z.len() - 1;
        z[last] ^= 0x10;
        assert_eq!(decompress(&z), Err(Error::Checksum));
        assert_eq!(decompress(&z[..3]), Err(Error::Truncated));
    }
}
