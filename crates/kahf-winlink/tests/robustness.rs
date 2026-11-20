//! Hostile or damaged input must give errors, never panics or huge allocations.

use kahf_winlink::lzhuf;
use kahf_winlink::message::Message;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u8 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) as u8
    }
}

#[test]
fn decompress_survives_random_streams_with_valid_checksums() {
    let mut r = Rng(1);
    for round in 0..300 {
        let len = (r.next() as usize % 200) + 4;
        let mut body: Vec<u8> = (0..len).map(|_| r.next()).collect();
        // Keep the declared size small so the decoder does real work, not an instant refusal.
        body[0..4].copy_from_slice(&((r.next() as u32) * 7 % 3000).to_le_bytes());
        let mut stream = lzhuf::crc16(&body).to_le_bytes().to_vec();
        stream.extend_from_slice(&body);
        let _ = lzhuf::decompress(&stream); // must not panic
        let _ = round;
    }
}

#[test]
fn decompress_survives_bit_flips_of_a_real_stream() {
    let good =
        lzhuf::compress(&b"Mid: ABCDEFGH1234\r\nFrom: K1ABC\r\n\r\nhello hello hello hello".repeat(20));
    let mut r = Rng(2);
    for _ in 0..500 {
        let mut s = good.clone();
        let i = r.next() as usize % s.len();
        s[i] ^= 1 << (r.next() % 8);
        // Re-seal the checksum so the damage reaches the decoder itself.
        if s.len() > 6 {
            let crc = lzhuf::crc16(&s[2..]).to_le_bytes();
            s[0] = crc[0];
            s[1] = crc[1];
        }
        let _ = lzhuf::decompress(&s);
    }
}

#[test]
fn message_parse_survives_garbage_and_truncation() {
    let m = Message::new(
        "K1ABC",
        &["N0CALL"],
        "subject",
        &"body ".repeat(50),
        1_800_000_000,
    );
    let wire = m.to_bytes();
    for cut in 0..wire.len() {
        let _ = Message::parse(&wire[..cut]);
    }
    let mut r = Rng(3);
    for _ in 0..500 {
        let mut s = wire.clone();
        for _ in 0..3 {
            let i = r.next() as usize % s.len();
            s[i] = r.next();
        }
        let _ = Message::parse(&s);
    }
    let _ = Message::parse(b"Mid: X\r\nBody: 99999999999999999999\r\n\r\n");
    let _ = Message::parse(b"Mid: X\r\nBody: 18446744073709551615\r\nFile: 18446744073709551615 a\r\n\r\n");
}
