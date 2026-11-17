//! Contact logbook: plain CSV storage plus ADIF 3 export.
//!
//! The file is CSV with a fixed header line. Fields containing a comma,
//! quote or line break are quoted, with embedded quotes doubled.

pub mod contacts;
pub mod occupancy;
pub mod planner;
pub mod relay_sim;

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

/// CSV header written as the first line of a new log file.
const CSV_HEADER: &str = "call,date,time,band,mode,freq_mhz,rst_sent,rst_rcvd,name,comment";

/// ADIF header emitted at the top of every export.
const ADIF_HEADER: &str = "ADIF export by kahf-log\n<ADIF_VER:5>3.1.4\n<PROGRAMID:4>kahf\n<EOH>\n";

#[derive(Debug, Clone, PartialEq)]
pub struct Qso {
    pub call: String,
    /// YYYYMMDD
    pub date: String,
    /// HHMMSS
    pub time: String,
    pub band: String,
    pub mode: String,
    pub freq_mhz: Option<f64>,
    pub rst_sent: String,
    pub rst_rcvd: String,
    pub name: String,
    pub comment: String,
}

/// A callsign is 3 to 12 characters of ASCII letters, digits or `/`.
pub fn valid_call(call: &str) -> bool {
    let n = call.chars().count();
    (3..=12).contains(&n) && call.chars().all(|c| c.is_ascii_alphanumeric() || c == '/')
}

/// Append one contact to the log, creating the file (with header) if missing.
pub fn add(path: &Path, qso: &Qso) -> Result<(), String> {
    if !valid_call(&qso.call) {
        return Err(format!("invalid callsign: {:?}", qso.call));
    }
    let needs_header = match fs::metadata(path) {
        Ok(m) => m.len() == 0,
        Err(_) => true,
    };
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("open {}: {e}", path.display()))?;

    let mut out = String::new();
    if needs_header {
        out.push_str(CSV_HEADER);
        out.push('\n');
    }
    let freq = qso.freq_mhz.map(|f| format!("{f}")).unwrap_or_default();
    let fields = [
        qso.call.as_str(),
        qso.date.as_str(),
        qso.time.as_str(),
        qso.band.as_str(),
        qso.mode.as_str(),
        freq.as_str(),
        qso.rst_sent.as_str(),
        qso.rst_rcvd.as_str(),
        qso.name.as_str(),
        qso.comment.as_str(),
    ];
    let line: Vec<String> = fields.iter().map(|f| csv_escape(f)).collect();
    out.push_str(&line.join(","));
    out.push('\n');

    file.write_all(out.as_bytes())
        .map_err(|e| format!("write {}: {e}", path.display()))
}

/// Load all contacts. A missing file is an empty log.
pub fn load(path: &Path) -> Result<Vec<Qso>, String> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    let mut qsos = Vec::new();
    for (idx, record) in parse_csv(&text).into_iter().enumerate() {
        if idx == 0 && record.join(",") == CSV_HEADER {
            continue;
        }
        if record.len() != 10 {
            return Err(format!(
                "record {}: expected 10 fields, got {}",
                idx + 1,
                record.len()
            ));
        }
        let freq_mhz = if record[5].is_empty() {
            None
        } else {
            Some(
                record[5]
                    .parse::<f64>()
                    .map_err(|e| format!("record {}: bad freq {:?}: {e}", idx + 1, record[5]))?,
            )
        };
        let mut f = record.into_iter();
        qsos.push(Qso {
            call: f.next().unwrap(),
            date: f.next().unwrap(),
            time: f.next().unwrap(),
            band: f.next().unwrap(),
            mode: f.next().unwrap(),
            freq_mhz: {
                f.next();
                freq_mhz
            },
            rst_sent: f.next().unwrap(),
            rst_rcvd: f.next().unwrap(),
            name: f.next().unwrap(),
            comment: f.next().unwrap(),
        });
    }
    Ok(qsos)
}

/// Render contacts as an ADIF 3 document. Field lengths are byte counts.
pub fn export_adif(qsos: &[Qso]) -> String {
    let mut out = String::from(ADIF_HEADER);
    for q in qsos {
        let freq = q.freq_mhz.map(|f| format!("{f:.6}")).unwrap_or_default();
        let fields: [(&str, &str); 10] = [
            ("CALL", &q.call),
            ("QSO_DATE", &q.date),
            ("TIME_ON", &q.time),
            ("BAND", &q.band),
            ("MODE", &q.mode),
            ("FREQ", &freq),
            ("RST_SENT", &q.rst_sent),
            ("RST_RCVD", &q.rst_rcvd),
            ("NAME", &q.name),
            ("COMMENT", &q.comment),
        ];
        for (tag, value) in fields {
            if value.is_empty() {
                continue;
            }
            out.push_str(&format!("<{tag}:{}>{value} ", value.len()));
        }
        out.push_str("<EOR>\n");
    }
    out
}

fn csv_escape(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Split CSV text into records, honouring quoted fields that span lines.
/// Blank lines are skipped.
fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut records = Vec::new();
    let mut record: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    let mut pending = false; // true once the current record has any content

    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' => {
                in_quotes = true;
                pending = true;
            }
            ',' => {
                record.push(std::mem::take(&mut field));
                pending = true;
            }
            '\r' => {}
            '\n' => {
                if pending || !field.is_empty() {
                    record.push(std::mem::take(&mut field));
                    records.push(std::mem::take(&mut record));
                }
                pending = false;
            }
            _ => {
                field.push(c);
                pending = true;
            }
        }
    }
    if pending || !field.is_empty() {
        record.push(field);
        records.push(record);
    }
    records
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn temp_path(tag: &str) -> std::path::PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("kahf-log-{}-{tag}-{n}.csv", std::process::id()));
        let _ = fs::remove_file(&p);
        p
    }

    fn sample(call: &str) -> Qso {
        Qso {
            call: call.into(),
            date: "20261117".into(),
            time: "120000".into(),
            band: "20M".into(),
            mode: "KAHF".into(),
            freq_mhz: Some(14.1),
            rst_sent: "59".into(),
            rst_rcvd: "57".into(),
            name: "Jörg".into(),
            comment: "hello, \"world\"\nline two".into(),
        }
    }

    #[test]
    fn round_trip() {
        let path = temp_path("rt");
        let a = sample("BB1BBB");
        let b = Qso {
            freq_mhz: None,
            name: String::new(),
            comment: String::new(),
            ..sample("AB/9")
        };
        add(&path, &a).unwrap();
        add(&path, &b).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded, vec![a, b]);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn load_missing_file_is_empty() {
        let path = temp_path("missing");
        assert!(load(&path).unwrap().is_empty());
    }

    #[test]
    fn adif_lengths_count_bytes_for_multibyte() {
        let adif = export_adif(&[sample("BB1BBB")]);
        // "Jörg" is 4 characters but 5 bytes.
        assert!(adif.contains("<NAME:5>Jörg "), "{adif}");
        assert!(adif.contains("<CALL:6>BB1BBB <QSO_DATE:8>20261117 <TIME_ON:6>120000 <BAND:3>20M <MODE:4>KAHF <FREQ:9>14.100000 "));
        assert!(adif.ends_with("<EOR>\n"));
    }

    #[test]
    fn adif_multibyte_value_length_matches_content() {
        let q = Qso {
            name: "日本語".into(),
            ..sample("BB1BBB")
        };
        let adif = export_adif(&[q]);
        // 3 characters, 9 bytes.
        assert!(adif.contains("<NAME:9>日本語 "), "{adif}");
    }

    #[test]
    fn invalid_call_rejected() {
        let path = temp_path("badcall");
        for bad in ["AB", "BAD CALL", "TOOLONG12345X", "BB-1BB", "", "BB1BBB\n"] {
            assert!(add(&path, &sample(bad)).is_err(), "accepted {bad:?}");
        }
        assert!(!path.exists(), "invalid add must not create the file");
    }

    #[test]
    fn valid_call_bounds() {
        assert!(valid_call("ABC"));
        assert!(valid_call("A2/BB1BBB1"));
        assert!(valid_call("ABCDEFGHIJKL"));
        assert!(!valid_call("ABCDEFGHIJKLM"));
    }

    #[test]
    fn export_empty_log_is_header_only() {
        assert_eq!(export_adif(&[]), ADIF_HEADER);
    }
}
pub mod routing;
pub mod sync;
