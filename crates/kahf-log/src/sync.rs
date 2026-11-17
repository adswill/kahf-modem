//! Logbook sync between a station's own stations (for example a home station
//! and its remote relay).
//!
//! A batch carries up to [`BATCH_MAX`] QSO records starting at a sequence
//! index. Layout (multi-byte integers are little-endian):
//!
//! ```text
//! [count: u8] [from: u16] [checksum: u16] records...
//! record: [call_len: u8] [call] [date: u16] [time: u24] [band_len: u8] [band]
//!         [mode_len: u8] [mode] [freq_flag: u8] [freq: f64 if flag is 1]
//! ```
//!
//! `date` is packed as year-2000 (7 bits), month (4 bits), day (5 bits).
//! `time` is seconds since midnight. The checksum is CRC-16/CCITT-FALSE over
//! the record bytes. Only the fields above travel; RST, name and comment are
//! not synced. A date or time that cannot be encoded is stored as zero.

use crate::Qso;

/// Maximum number of records in one batch.
pub const BATCH_MAX: usize = 6;

const HEADER_LEN: usize = 5;

/// Build a batch of up to [`BATCH_MAX`] entries starting at index `from`.
/// `from` past the end of `qsos` gives an empty batch.
pub fn make_batch(qsos: &[Qso], from: usize) -> Vec<u8> {
    let start = from.min(qsos.len());
    let end = (start + BATCH_MAX).min(qsos.len());
    let batch = &qsos[start..end];

    let mut records = Vec::new();
    for q in batch {
        encode_record(q, &mut records);
    }

    let mut out = Vec::with_capacity(HEADER_LEN + records.len());
    out.push(batch.len() as u8);
    out.extend_from_slice(&(start as u16).to_le_bytes());
    out.extend_from_slice(&crc16(&records).to_le_bytes());
    out.extend_from_slice(&records);
    out
}

/// Decode a batch and append every entry not already in `existing`.
/// An entry matches when call, date, time and band are all equal.
/// Returns how many entries were added. Nothing is changed on error.
pub fn merge_batch(existing: &mut Vec<Qso>, batch: &[u8]) -> Result<usize, String> {
    if batch.len() < HEADER_LEN {
        return Err("batch too short".into());
    }
    let count = batch[0] as usize;
    let sent = u16::from_le_bytes([batch[3], batch[4]]);
    let records = &batch[HEADER_LEN..];

    if crc16(records) != sent {
        return Err("checksum mismatch".into());
    }
    if count > BATCH_MAX {
        return Err(format!("batch count {count} exceeds {BATCH_MAX}"));
    }

    let mut cur = Cursor { buf: records, pos: 0 };
    let mut decoded = Vec::with_capacity(count);
    for _ in 0..count {
        decoded.push(decode_record(&mut cur)?);
    }
    if cur.pos != records.len() {
        return Err("trailing bytes after records".into());
    }

    let mut added = 0;
    for q in decoded {
        if !existing.iter().any(|e| same_entry(e, &q)) {
            existing.push(q);
            added += 1;
        }
    }
    Ok(added)
}

fn same_entry(a: &Qso, b: &Qso) -> bool {
    a.call == b.call && a.date == b.date && a.time == b.time && a.band == b.band
}

fn encode_record(q: &Qso, out: &mut Vec<u8>) {
    put_str(out, &q.call);
    out.extend_from_slice(&pack_date(&q.date).to_le_bytes());
    out.extend_from_slice(&pack_time(&q.time).to_le_bytes()[..3]);
    put_str(out, &q.band);
    put_str(out, &q.mode);
    match q.freq_mhz {
        Some(f) => {
            out.push(1);
            out.extend_from_slice(&f.to_le_bytes());
        }
        None => out.push(0),
    }
}

fn decode_record(cur: &mut Cursor) -> Result<Qso, String> {
    let call = cur.str()?;
    let date = unpack_date(u16::from_le_bytes(cur.take(2)?.try_into().unwrap()));
    let t = cur.take(3)?;
    let time = unpack_time(u32::from_le_bytes([t[0], t[1], t[2], 0]));
    let band = cur.str()?;
    let mode = cur.str()?;
    let freq_mhz = match cur.take(1)?[0] {
        0 => None,
        1 => Some(f64::from_le_bytes(cur.take(8)?.try_into().unwrap())),
        _ => return Err("bad frequency flag".into()),
    };
    Ok(Qso {
        call,
        date,
        time,
        band,
        mode,
        freq_mhz,
        rst_sent: String::new(),
        rst_rcvd: String::new(),
        name: String::new(),
        comment: String::new(),
    })
}

/// Length-prefixed text. Values longer than 255 bytes are cut.
fn put_str(out: &mut Vec<u8>, s: &str) {
    let bytes = &s.as_bytes()[..s.len().min(255)];
    out.push(bytes.len() as u8);
    out.extend_from_slice(bytes);
}

fn pack_date(s: &str) -> u16 {
    if s.len() != 8 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return 0;
    }
    let y: u16 = s[0..4].parse().unwrap_or(0);
    let m: u16 = s[4..6].parse().unwrap_or(0);
    let d: u16 = s[6..8].parse().unwrap_or(0);
    if !(2000..=2127).contains(&y) || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return 0;
    }
    ((y - 2000) << 9) | (m << 5) | d
}

fn unpack_date(v: u16) -> String {
    format!("{:04}{:02}{:02}", (v >> 9) + 2000, (v >> 5) & 0xF, v & 0x1F)
}

fn pack_time(s: &str) -> u32 {
    if s.len() != 6 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return 0;
    }
    let h: u32 = s[0..2].parse().unwrap_or(99);
    let m: u32 = s[2..4].parse().unwrap_or(99);
    let sec: u32 = s[4..6].parse().unwrap_or(99);
    if h > 23 || m > 59 || sec > 59 {
        return 0;
    }
    h * 3600 + m * 60 + sec
}

fn unpack_time(v: u32) -> String {
    format!("{:02}{:02}{:02}", v / 3600, (v / 60) % 60, v % 60)
}

/// CRC-16/CCITT-FALSE (poly 0x1021, init 0xFFFF).
fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
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

struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.buf.len() - self.pos < n {
            return Err("truncated batch".into());
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn str(&mut self) -> Result<String, String> {
        let n = self.take(1)?[0] as usize;
        let bytes = self.take(n)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| "invalid text in batch".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(call: &str, time: &str, band: &str, freq: Option<f64>) -> Qso {
        Qso {
            call: call.into(),
            date: "20261117".into(),
            time: time.into(),
            band: band.into(),
            mode: "CW".into(),
            freq_mhz: freq,
            rst_sent: "599".into(),
            rst_rcvd: "579".into(),
            name: "Name".into(),
            comment: "note".into(),
        }
    }

    /// What survives the wire: RST, name and comment are dropped.
    fn on_wire(q: &Qso) -> Qso {
        Qso {
            rst_sent: String::new(),
            rst_rcvd: String::new(),
            name: String::new(),
            comment: String::new(),
            ..q.clone()
        }
    }

    fn six() -> Vec<Qso> {
        vec![
            entry("AB1C", "120000", "20M", Some(14.074)),
            entry("AB2D", "120500", "20M", None),
            entry("AB3E", "121000", "40M", Some(7.030)),
            entry("AB4F", "121500", "20M", None),
            entry("AB5G", "122000", "80M", Some(3.560)),
            entry("AB6H", "235959", "10M", Some(28.074)),
        ]
    }

    #[test]
    fn sync_batch_round_trip() {
        let qsos = six();
        let batch = make_batch(&qsos, 0);
        let mut dest = Vec::new();
        assert_eq!(merge_batch(&mut dest, &batch), Ok(6));
        let expected: Vec<Qso> = qsos.iter().map(on_wire).collect();
        assert_eq!(dest, expected);
    }

    #[test]
    fn sync_merge_twice_adds_nothing() {
        let qsos = six();
        let batch = make_batch(&qsos, 0);
        let mut dest = Vec::new();
        assert_eq!(merge_batch(&mut dest, &batch), Ok(6));
        assert_eq!(merge_batch(&mut dest, &batch), Ok(0));
        assert_eq!(dest.len(), 6);
    }

    #[test]
    fn sync_corrupted_byte_fails_checksum() {
        let batch = make_batch(&six(), 0);
        let mut bad = batch.clone();
        bad[HEADER_LEN + 2] ^= 0x01;
        let mut dest = Vec::new();
        let err = merge_batch(&mut dest, &bad).unwrap_err();
        assert!(err.contains("checksum"), "{err}");
        assert!(dest.is_empty(), "failed merge must not change the log");
    }

    #[test]
    fn sync_batch_size_for_six_short_calls() {
        // Per record: call 5 + date 2 + time 3 + band 4 + mode 3 + freq flag 1 = 18.
        let short: Vec<Qso> = (0..6)
            .map(|i| entry(&format!("AB{i}C"), "120000", "20M", None))
            .collect();
        let batch = make_batch(&short, 0);
        println!(
            "sync batch size, 6 short-call entries without frequency: {} bytes",
            batch.len()
        );
        assert_eq!(batch.len(), HEADER_LEN + 6 * 18);

        let with_freq = make_batch(&six(), 0);
        println!(
            "sync batch size, 6 entries with frequency: {} bytes",
            with_freq.len()
        );
    }

    #[test]
    fn sync_from_index_and_end_of_log() {
        let qsos = six();
        let tail = make_batch(&qsos, 4);
        let mut dest = Vec::new();
        assert_eq!(merge_batch(&mut dest, &tail), Ok(2));
        assert_eq!(dest[0].call, "AB5G");

        let past = make_batch(&qsos, 10);
        assert_eq!(past[0], 0);
        assert_eq!(merge_batch(&mut dest, &past), Ok(0));
    }

    #[test]
    fn sync_truncated_batch_is_an_error() {
        let batch = make_batch(&six(), 0);
        let mut dest = Vec::new();
        assert!(merge_batch(&mut dest, &batch[..HEADER_LEN]).is_err());
        assert!(merge_batch(&mut dest, &[1, 0]).is_err());
        assert!(dest.is_empty());
    }
}
