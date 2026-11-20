//! Payload framing: CRC-16, scrambler, length field. Bits are 0/1 in Vec<u8>.
//!
//! Frame layout (MSB first, always `K` bits, before scrambling):
//!   [0..8)     length in bytes (0..=36)
//!   [8..296)   data bytes, zero-padded to the 36-byte maximum
//!   [296..304) modulation id: 0 QPSK, 1 16QAM, 2 64QAM (the receiver tries each)
//!   [304..320) CRC-16/CCITT over the length byte, the data bytes and the id
//!   [320..324) zero padding up to K
//! The whole vector is then XOR-scrambled with a 15-bit LFSR.

use crate::ldpc::K;

/// Maximum data bytes carried by one frame.
pub const MAX_DATA: usize = 36;
/// Width of the length field in bits.
const LEN_BITS: usize = 8;
/// Bit offset of the modulation id, after the fixed-size zero-padded data field.
const ID_POS: usize = LEN_BITS + MAX_DATA * 8;
/// Width of the modulation id in bits.
const ID_BITS: usize = 8;
/// Bit offset of the CRC field.
const CRC_POS: usize = ID_POS + ID_BITS;
/// Width of the CRC field in bits.
const CRC_BITS: usize = 16;

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

/// XOR the bits in place with the LFSR keystream (x^15 + x^14 + 1, seed 0x7FFF).
/// Applying it twice restores the original bits, so it serves both directions.
fn scramble(bits: &mut [u8]) {
    let mut state: u16 = 0x7FFF;
    for b in bits.iter_mut() {
        let fb = ((state >> 14) ^ (state >> 13)) & 1;
        state = ((state << 1) | fb) & 0x7FFF;
        *b = (*b & 1) ^ (fb as u8);
    }
}

/// Write the low `n` bits of `val` into `out[pos..pos + n]`, MSB first.
fn put_bits(out: &mut [u8], pos: usize, val: u32, n: usize) {
    for i in 0..n {
        out[pos + i] = ((val >> (n - 1 - i)) & 1) as u8;
    }
}

/// Read `n` bits from `bits[pos..pos + n]` as an MSB-first integer.
fn get_bits(bits: &[u8], pos: usize, n: usize) -> u32 {
    (0..n).fold(0, |acc, i| (acc << 1) | (bits[pos + i] & 1) as u32)
}

/// Add length + CRC-16 + scrambling. Output fits the LDPC info size (see ldpc::K).
///
/// Panics if `data` is longer than 36 bytes.
pub fn build_payload(data: &[u8]) -> Vec<u8> {
    build_payload_as(data, 0)
}

/// As build_payload, with the modulation id the frame is sent with (0 QPSK, 1 16QAM, 2 64QAM).
///
/// Panics if `data` is longer than 36 bytes.
pub fn build_payload_as(data: &[u8], id: u8) -> Vec<u8> {
    assert!(
        data.len() <= MAX_DATA,
        "frame payload too large: {} bytes (max {} per frame)",
        data.len(),
        MAX_DATA
    );

    let mut covered = Vec::with_capacity(2 + data.len());
    covered.push(data.len() as u8);
    covered.extend_from_slice(data);
    covered.push(id);
    let crc = crc16(&covered);

    let mut bits = vec![0u8; K];
    put_bits(&mut bits, 0, covered[0] as u32, LEN_BITS);
    for (i, &b) in data.iter().enumerate() {
        put_bits(&mut bits, LEN_BITS + 8 * i, b as u32, 8);
    }
    put_bits(&mut bits, ID_POS, id as u32, ID_BITS);
    put_bits(&mut bits, CRC_POS, crc as u32, CRC_BITS);
    scramble(&mut bits);
    bits
}

/// Reverse of build_payload. None if the length is out of range, the CRC fails,
/// or the input is not exactly `K` bits.
pub fn parse_payload(bits: &[u8]) -> Option<Vec<u8>> {
    parse_payload_id(bits).map(|(data, _)| data)
}

/// Reverse of build_payload_as: the data and the modulation id. None under the
/// same conditions as parse_payload.
pub fn parse_payload_id(bits: &[u8]) -> Option<(Vec<u8>, u8)> {
    if bits.len() != K {
        return None;
    }
    let mut clear = bits.to_vec();
    scramble(&mut clear);

    let len = get_bits(&clear, 0, LEN_BITS) as usize;
    if len > MAX_DATA {
        return None;
    }

    let id = get_bits(&clear, ID_POS, ID_BITS) as u8;
    let mut covered = Vec::with_capacity(2 + len);
    covered.push(len as u8);
    for i in 0..len {
        covered.push(get_bits(&clear, LEN_BITS + 8 * i, 8) as u8);
    }
    covered.push(id);

    let rx_crc = get_bits(&clear, CRC_POS, CRC_BITS) as u16;
    if crc16(&covered) != rx_crc {
        return None;
    }
    Some((covered[1..covered.len() - 1].to_vec(), id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(data: &[u8]) {
        let bits = build_payload(data);
        assert_eq!(bits.len(), K);
        assert!(bits.iter().all(|&b| b <= 1));
        assert_eq!(parse_payload(&bits).as_deref(), Some(data));
    }

    #[test]
    fn roundtrip_empty() {
        roundtrip(&[]);
    }

    #[test]
    fn roundtrip_one_byte() {
        roundtrip(&[0xA5]);
    }

    #[test]
    fn roundtrip_max_36_bytes() {
        let data: Vec<u8> = (0..36u8).map(|i| i.wrapping_mul(7).wrapping_add(3)).collect();
        roundtrip(&data);
    }

    #[test]
    fn crc_catches_single_bit_flip() {
        let data = b"kahf frame";
        let good = build_payload(data);
        // Covered bits: the length byte, the data bytes and the CRC field.
        // Padding bits sit outside the CRC and are deliberately not checked.
        let covered: Vec<usize> = (0..LEN_BITS + 8 * data.len())
            .chain(CRC_POS..CRC_POS + CRC_BITS)
            .collect();
        for i in covered {
            let mut bits = good.clone();
            // Flipping a scrambled bit flips the same clear bit after descrambling.
            bits[i] ^= 1;
            assert_eq!(parse_payload(&bits), None, "flip at bit {} was not caught", i);
        }
    }

    #[test]
    fn rejects_wrong_length_input() {
        let bits = build_payload(b"x");
        assert_eq!(parse_payload(&bits[..K - 1]), None);
    }

    #[test]
    #[should_panic(expected = "frame payload too large")]
    fn oversize_panics() {
        build_payload(&[0u8; 37]);
    }
}
