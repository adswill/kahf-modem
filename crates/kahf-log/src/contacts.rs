//! Address book of stations: plain text, one contact per line.
//!
//! Each line holds five tab-separated fields: call, name, qth, notes and
//! last_heard (empty when unknown). Backslash, tab, newline and carriage
//! return inside a field are escaped as `\\`, `\t`, `\n` and `\r`, so every
//! contact stays on one line whatever its notes contain.

use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

use crate::valid_call;

#[derive(Debug, Clone, PartialEq)]
pub struct Contact {
    pub call: String,
    pub name: String,
    pub qth: String,
    pub notes: String,
    /// YYYYMMDD of the last QSO or heard date, if any.
    pub last_heard: Option<String>,
}

/// Load all contacts. A missing file is an empty list.
pub fn load(path: &Path) -> Result<Vec<Contact>, String> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    let mut list = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != 5 {
            return Err(format!(
                "line {}: expected 5 fields, got {}",
                idx + 1,
                fields.len()
            ));
        }
        let call = unescape(fields[0]);
        if !valid_call(&call) {
            return Err(format!("line {}: invalid callsign: {call:?}", idx + 1));
        }
        let last_heard = unescape(fields[4]);
        list.push(Contact {
            call,
            name: unescape(fields[1]),
            qth: unescape(fields[2]),
            notes: unescape(fields[3]),
            last_heard: if last_heard.is_empty() {
                None
            } else {
                Some(last_heard)
            },
        });
    }
    Ok(list)
}

/// Write all contacts, replacing the file atomically (temp file, then rename).
pub fn save(path: &Path, list: &[Contact]) -> Result<(), String> {
    let mut out = String::new();
    for c in list {
        if !valid_call(&c.call) {
            return Err(format!("invalid callsign: {:?}", c.call));
        }
        let fields = [
            escape(&c.call),
            escape(&c.name),
            escape(&c.qth),
            escape(&c.notes),
            escape(c.last_heard.as_deref().unwrap_or("")),
        ];
        out.push_str(&fields.join("\t"));
        out.push('\n');
    }

    let tmp = {
        let mut s = path.as_os_str().to_owned();
        s.push(".tmp");
        std::path::PathBuf::from(s)
    };
    let write = || -> std::io::Result<()> {
        let mut file = File::create(&tmp)?;
        file.write_all(out.as_bytes())?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    };
    write().map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("save {}: {e}", path.display())
    })
}

/// Insert a contact, or replace the existing one with the same callsign
/// (case insensitive).
pub fn upsert(list: &mut Vec<Contact>, c: Contact) {
    match list.iter_mut().find(|e| e.call.eq_ignore_ascii_case(&c.call)) {
        Some(existing) => *existing = c,
        None => list.push(c),
    }
}

/// Contacts whose callsign or name contains `query` (case insensitive).
pub fn find<'a>(list: &'a [Contact], query: &str) -> Vec<&'a Contact> {
    let q = query.to_lowercase();
    list.iter()
        .filter(|c| c.call.to_lowercase().contains(&q) || c.name.to_lowercase().contains(&q))
        .collect()
}

/// Set `last_heard` for the contact with this callsign (case insensitive).
/// Does nothing when no such contact exists.
pub fn touch(list: &mut [Contact], call: &str, date: &str) {
    if let Some(c) = list.iter_mut().find(|e| e.call.eq_ignore_ascii_case(call)) {
        c.last_heard = Some(date.to_string());
    }
}

/// Fills the QTH of a contact from a heard grid locator, only when the QTH is empty, so a
/// locator or place typed by hand is never overwritten. Adds the contact if it is new.
/// Returns true if the list changed.
pub fn fill_grid(list: &mut Vec<Contact>, call: &str, grid: &str) -> bool {
    match list.iter_mut().find(|e| e.call.eq_ignore_ascii_case(call)) {
        Some(c) if c.qth.trim().is_empty() => {
            c.qth = grid.to_string();
            true
        }
        Some(_) => false,
        None => {
            list.push(Contact {
                call: call.to_string(),
                name: String::new(),
                qth: grid.to_string(),
                notes: String::new(),
                last_heard: None,
            });
            true
        }
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(ch),
        }
    }
    out
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn fill_grid_never_overwrites_hand_entry() {
        let mut list = Vec::new();
        assert!(fill_grid(&mut list, "G4ABC", "IO91WM"));
        assert_eq!(list[0].qth, "IO91WM");
        assert!(!fill_grid(&mut list, "g4abc", "JN58"), "already set");
        list[0].qth = "  ".into();
        assert!(fill_grid(&mut list, "G4ABC", "JN58"), "blank counts as empty");
        assert_eq!(list.len(), 1);
    }

    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn temp_path(tag: &str) -> std::path::PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("kahf-contacts-{}-{tag}-{n}.txt", std::process::id()));
        let _ = fs::remove_file(&p);
        p
    }

    fn contact(call: &str, name: &str) -> Contact {
        Contact {
            call: call.into(),
            name: name.into(),
            qth: String::new(),
            notes: String::new(),
            last_heard: None,
        }
    }

    #[test]
    fn round_trip_with_commas_tabs_and_newlines() {
        let path = temp_path("rt");
        let a = Contact {
            notes: "rig: FT-991, \"best\"\tkeeps\nline two\\back\r".into(),
            qth: "Dubai, UAE".into(),
            last_heard: Some("20261117".into()),
            ..contact("BB1BBB", "Jörg")
        };
        let b = contact("AB/9", "");
        save(&path, &[a.clone(), b.clone()]).unwrap();
        assert_eq!(load(&path).unwrap(), vec![a, b]);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn upsert_replaces_by_callsign_case_insensitive() {
        let mut list = vec![contact("BB1BBB", "Old")];
        upsert(&mut list, contact("bb1bbb", "New"));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "New");
        upsert(&mut list, contact("CC2CCC", "Other"));
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn find_is_case_insensitive_on_call_and_name() {
        let list = vec![contact("BB1BBB", "Jörg"), contact("CC2CCC", "Anna")];
        assert_eq!(find(&list, "bb1").len(), 1);
        assert_eq!(find(&list, "ANNA")[0].call, "CC2CCC");
        assert_eq!(find(&list, "rg")[0].call, "BB1BBB");
        assert!(find(&list, "zzz").is_empty());
    }

    #[test]
    fn bad_callsign_rejected() {
        let path = temp_path("badcall");
        assert!(save(&path, &[contact("AB", "Short")]).is_err());
        assert!(save(&path, &[contact("BAD CALL", "Space")]).is_err());
        assert!(!path.exists(), "invalid save must not create the file");

        fs::write(&path, "BB-1BB\t\t\t\t\n").unwrap();
        assert!(load(&path).is_err());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn missing_file_loads_as_empty() {
        let path = temp_path("missing");
        assert!(load(&path).unwrap().is_empty());
    }

    #[test]
    fn touch_sets_last_heard() {
        let mut list = vec![contact("BB1BBB", "Jörg")];
        touch(&mut list, "bb1bbb", "20261118");
        assert_eq!(list[0].last_heard.as_deref(), Some("20261118"));
        touch(&mut list, "NOPE1", "20261119");
        assert_eq!(list.len(), 1);
    }
}
