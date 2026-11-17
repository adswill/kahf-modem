//! Small pure helpers for amateur exchanges: message templates, delivery/read receipts,
//! and resume requests for interrupted transfers. No I/O; everything is deterministic.

/// Built-in exchange templates as (name, body). Plain ASCII text, no emojis.
pub const TEMPLATES: &[(&str, &str)] = &[
    (
        "signal_report",
        "{call} de {my_call}, your signal is RST {rst}, QTH {qth}. Over.",
    ),
    (
        "check_in",
        "{my_call} checking in on {band}, listening for traffic. Over.",
    ),
    ("closing", "Thanks for the QSO {call}. 73 from {my_call}. Out."),
];

/// Replace each `{name}` in `template` with the matching value from `fields`.
/// Placeholders without a matching field are left as written. Substituted values are
/// not scanned again, so a value containing `{x}` stays literal.
pub fn render(template: &str, fields: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                let name = &after[..close];
                match fields.iter().find(|(k, _)| *k == name) {
                    Some((_, value)) => out.push_str(value),
                    None => {
                        out.push('{');
                        out.push_str(name);
                        out.push('}');
                    }
                }
                rest = &after[close + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Receipt sent back over the link. Encodes to 3 bytes: a tag and a 16-bit message id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Receipt {
    Delivered { message_id: u32 },
    Read { message_id: u32 },
}

const TAG_DELIVERED: u8 = 0x01;
const TAG_READ: u8 = 0x02;

/// Encode a receipt into 3 bytes. Panics if `message_id` exceeds 16 bits, because the
/// wire field is 16 bits and truncating silently would corrupt the receipt.
pub fn encode_receipt(receipt: &Receipt) -> Vec<u8> {
    let (tag, message_id) = match *receipt {
        Receipt::Delivered { message_id } => (TAG_DELIVERED, message_id),
        Receipt::Read { message_id } => (TAG_READ, message_id),
    };
    assert!(message_id <= 0xFFFF, "receipt message id exceeds 16 bits");
    let id = message_id as u16;
    vec![tag, (id >> 8) as u8, id as u8]
}

/// Decode a 3-byte receipt. Returns None for wrong length or unknown tag.
pub fn decode_receipt(bytes: &[u8]) -> Option<Receipt> {
    if bytes.len() != 3 {
        return None;
    }
    let message_id = u16::from_be_bytes([bytes[1], bytes[2]]) as u32;
    match bytes[0] {
        TAG_DELIVERED => Some(Receipt::Delivered { message_id }),
        TAG_READ => Some(Receipt::Read { message_id }),
        _ => None,
    }
}

/// Largest resume request that fits one 36-byte frame.
pub const REQUEST_MAX: usize = 36;
const MODE_BITMAP: u8 = 0;
const MODE_INDEXES: u8 = 1;
/// Request header: mode (1) + message id (4, big endian) + total chunks (2, big endian).
const REQUEST_HEADER: usize = 7;
/// Missing indexes listed in fallback mode. Each index is 2 bytes, so only
/// (36 - 7) / 2 = 14 fit in one frame. The requested 30 would need 60 bytes.
const FALLBACK_INDEXES: usize = (REQUEST_MAX - REQUEST_HEADER) / 2;

/// Receive-side state of a chunked transfer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferState {
    pub message_id: u32,
    pub total_chunks: u16,
    pub received: Vec<bool>,
}

impl TransferState {
    /// Indexes of chunks not yet received, ascending. Chunks past the end of `received` count as missing.
    pub fn missing(&self) -> Vec<u16> {
        (0..self.total_chunks as usize)
            .filter(|&i| !self.received.get(i).copied().unwrap_or(false))
            .map(|i| i as u16)
            .collect()
    }

    /// Resume request, at most 36 bytes. Uses a bitmap of all chunks (bit set = missing,
    /// MSB first) when it fits. Otherwise lists the first missing indexes that fit.
    pub fn encode_request(&self) -> Vec<u8> {
        let header = |mode: u8| {
            let mut out = vec![mode];
            out.extend_from_slice(&self.message_id.to_be_bytes());
            out.extend_from_slice(&self.total_chunks.to_be_bytes());
            out
        };
        let bitmap_len = (self.total_chunks as usize).div_ceil(8);
        if REQUEST_HEADER + bitmap_len <= REQUEST_MAX {
            let mut out = header(MODE_BITMAP);
            let mut bits = vec![0u8; bitmap_len];
            for index in self.missing() {
                let i = index as usize;
                bits[i / 8] |= 0x80 >> (i % 8);
            }
            out.extend_from_slice(&bits);
            out
        } else {
            let mut out = header(MODE_INDEXES);
            for index in self.missing().into_iter().take(FALLBACK_INDEXES) {
                out.extend_from_slice(&index.to_be_bytes());
            }
            out
        }
    }
}

/// Decode a resume request into the list of missing chunk indexes. Returns None if the
/// bytes are malformed (bad length, unknown mode, or index outside the transfer).
pub fn decode_request(bytes: &[u8]) -> Option<Vec<u16>> {
    if bytes.len() < REQUEST_HEADER || bytes.len() > REQUEST_MAX {
        return None;
    }
    let total = u16::from_be_bytes([bytes[5], bytes[6]]) as usize;
    let body = &bytes[REQUEST_HEADER..];
    match bytes[0] {
        MODE_BITMAP => {
            if body.len() != total.div_ceil(8) {
                return None;
            }
            Some(
                (0..total)
                    .filter(|&i| body[i / 8] & (0x80 >> (i % 8)) != 0)
                    .map(|i| i as u16)
                    .collect(),
            )
        }
        MODE_INDEXES => {
            if !body.len().is_multiple_of(2) {
                return None;
            }
            let indexes: Vec<u16> = body
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect();
            if indexes.iter().any(|&i| i as usize >= total) {
                return None;
            }
            Some(indexes)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extras_render_substitutes_fields() {
        let out = render(
            "{call} de {my_call}, RST {rst}",
            &[("call", "DL1ABC"), ("my_call", "OK2XYZ"), ("rst", "599")],
        );
        assert_eq!(out, "DL1ABC de OK2XYZ, RST 599");
    }

    #[test]
    fn extras_render_keeps_unknown_and_unmatched_braces() {
        assert_eq!(render("{a} and {b}", &[("a", "x")]), "x and {b}");
        assert_eq!(render("open { brace", &[]), "open { brace");
        assert_eq!(render("{v}", &[("v", "{w}")]), "{w}");
    }

    #[test]
    fn extras_builtin_templates_render_without_placeholders_left() {
        let fields = [
            ("call", "DL1ABC"),
            ("my_call", "OK2XYZ"),
            ("rst", "59"),
            ("qth", "Berlin"),
            ("band", "20m"),
        ];
        for (name, body) in TEMPLATES {
            let out = render(body, &fields);
            assert!(!out.contains('{'), "template {name} left a placeholder: {out}");
            assert!(out.is_ascii(), "template {name} is not plain ASCII");
        }
    }

    #[test]
    fn extras_receipt_round_trip() {
        for receipt in [
            Receipt::Delivered { message_id: 0 },
            Receipt::Delivered { message_id: 0xABCD },
            Receipt::Read { message_id: 0xFFFF },
        ] {
            let bytes = encode_receipt(&receipt);
            assert_eq!(bytes.len(), 3);
            assert_eq!(decode_receipt(&bytes), Some(receipt));
        }
    }

    #[test]
    fn extras_receipt_rejects_bad_input() {
        assert_eq!(decode_receipt(&[0x01, 0x00]), None);
        assert_eq!(decode_receipt(&[0x09, 0x00, 0x01]), None);
    }

    #[test]
    fn extras_resume_bitmap_100_chunks_10_missing() {
        let missing_set = [0u16, 7, 8, 33, 50, 64, 65, 90, 98, 99];
        let received: Vec<bool> = (0..100u16).map(|i| !missing_set.contains(&i)).collect();
        let state = TransferState {
            message_id: 0x1234_5678,
            total_chunks: 100,
            received,
        };
        assert_eq!(state.missing(), missing_set.to_vec());

        let request = state.encode_request();
        assert_eq!(request.len(), REQUEST_HEADER + 13);
        assert_eq!(decode_request(&request), Some(missing_set.to_vec()));
    }

    #[test]
    fn extras_resume_request_fits_36_bytes() {
        // 100 chunks: bitmap mode.
        let state = TransferState {
            message_id: 1,
            total_chunks: 100,
            received: vec![false; 100],
        };
        assert!(state.encode_request().len() <= REQUEST_MAX);

        // Worst case: 65535 chunks, all missing. Bitmap is too big, so index fallback.
        let big = TransferState {
            message_id: 2,
            total_chunks: u16::MAX,
            received: vec![false; u16::MAX as usize],
        };
        let request = big.encode_request();
        assert!(request.len() <= REQUEST_MAX);
        assert_eq!(request.len(), REQUEST_HEADER + 2 * FALLBACK_INDEXES);
        let decoded = decode_request(&request).expect("fallback request decodes");
        assert_eq!(decoded.len(), FALLBACK_INDEXES);
        assert_eq!(decoded, big.missing()[..FALLBACK_INDEXES].to_vec());
    }

    #[test]
    fn extras_resume_rejects_bad_input() {
        assert_eq!(decode_request(&[0u8; 6]), None);
        assert_eq!(decode_request(&[9, 0, 0, 0, 1, 0, 8, 0]), None);
        // Index mode naming chunk 5 of a 3-chunk transfer.
        assert_eq!(decode_request(&[1, 0, 0, 0, 1, 0, 3, 0, 5]), None);
    }
}
