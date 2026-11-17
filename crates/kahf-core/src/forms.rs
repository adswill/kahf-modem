//! Structured forms (for example a check-in or a situation report) sent as compact text.
//!
//! Wire format: the form name on the first line, then one `key=value` line per field
//! that has a value. Inside the name and the values, `\` `\n` and `\r` are written as
//! `\\`, `\n` and `\r`. Field keys must not contain `=`, newlines or backslashes.

/// A named template with fields.
#[derive(Debug, Clone, PartialEq)]
pub struct FormDef {
    pub name: String,
    pub fields: Vec<FieldDef>,
}

/// One field of a form.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldDef {
    pub key: String,
    pub label: String,
    pub kind: Kind,
    pub required: bool,
}

/// What a field accepts.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// Free text, at most `max_len` characters.
    Text { max_len: usize },
    /// A number from `min` to `max`, both included.
    Number { min: f64, max: f64 },
    /// One of the listed options (compared without case).
    Choice(Vec<String>),
    /// A calendar date written as YYYY-MM-DD.
    Date,
}

/// Checks `values` (key, value pairs) against `def`.
///
/// Returns every problem found, each as a plain-language sentence.
pub fn validate(def: &FormDef, values: &[(String, String)]) -> Result<(), Vec<String>> {
    let mut problems = Vec::new();

    for (key, _) in values {
        if !def.fields.iter().any(|f| &f.key == key) {
            problems.push(format!("Unknown field \"{}\".", key));
        }
    }

    for f in &def.fields {
        let raw = lookup(values, &f.key).trim();
        if raw.is_empty() {
            if f.required {
                problems.push(format!("{} is required.", f.label));
            }
            continue;
        }
        if let Some(problem) = check_value(f, raw) {
            problems.push(problem);
        }
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

/// Validates `values` and encodes them as the compact text form.
///
/// Fields with no value are left out. Values are trimmed, and choices are
/// written in the spelling used by the form definition.
pub fn encode(def: &FormDef, values: &[(String, String)]) -> Result<Vec<u8>, String> {
    validate(def, values).map_err(|problems| problems.join(" "))?;

    let mut lines = vec![escape(&def.name)];
    for f in &def.fields {
        let raw = lookup(values, &f.key).trim();
        if raw.is_empty() {
            continue;
        }
        lines.push(format!("{}={}", f.key, escape(&canonical(f, raw))));
    }
    Ok(lines.join("\n").into_bytes())
}

/// Reads the compact text form back into the form name and its key/value pairs.
///
/// Returns `None` if the bytes are not UTF-8, the name is empty, a line has no
/// `=`, or an escape sequence is not valid.
pub fn decode(bytes: &[u8]) -> Option<(String, Vec<(String, String)>)> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut lines = text.split('\n');

    let name = unescape(lines.next()?)?;
    if name.is_empty() {
        return None;
    }

    let mut values = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (key, value) = line.split_once('=')?;
        values.push((unescape(key)?, unescape(value)?));
    }
    Some((name, values))
}

/// The built-in "Check-in" form: callsign, location, status and traffic count.
pub fn checkin() -> FormDef {
    FormDef {
        name: "Check-in".to_string(),
        fields: vec![
            field("callsign", "Callsign", Kind::Text { max_len: 10 }, true),
            field("location", "Location", Kind::Text { max_len: 40 }, true),
            field(
                "status",
                "Status",
                Kind::Choice(vec![
                    "Listening".to_string(),
                    "Standby".to_string(),
                    "Leaving".to_string(),
                    "Net control".to_string(),
                ]),
                true,
            ),
            field("traffic", "Traffic count", Kind::Number { min: 0.0, max: 999.0 }, true),
        ],
    }
}

/// The built-in "Situation report" form: location, time, need, resources and contact.
pub fn situation_report() -> FormDef {
    FormDef {
        name: "Situation report".to_string(),
        fields: vec![
            field("location", "Location", Kind::Text { max_len: 40 }, true),
            field("time", "Time", Kind::Text { max_len: 16 }, true),
            field("need", "What you need", Kind::Text { max_len: 80 }, true),
            field("resources", "Resources available", Kind::Text { max_len: 80 }, false),
            field("contact", "Contact", Kind::Text { max_len: 40 }, false),
        ],
    }
}

/// All built-in forms.
pub fn builtin_forms() -> Vec<FormDef> {
    vec![checkin(), situation_report()]
}

fn field(key: &str, label: &str, kind: Kind, required: bool) -> FieldDef {
    FieldDef {
        key: key.to_string(),
        label: label.to_string(),
        kind,
        required,
    }
}

/// The value for `key`, or "" when it is not given.
fn lookup<'a>(values: &'a [(String, String)], key: &str) -> &'a str {
    values
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
        .unwrap_or("")
}

/// The choice spelling from the definition when the input matches it without case.
fn canonical(f: &FieldDef, value: &str) -> String {
    if let Kind::Choice(options) = &f.kind {
        if let Some(opt) = options.iter().find(|o| o.eq_ignore_ascii_case(value)) {
            return opt.clone();
        }
    }
    value.to_string()
}

/// Checks one non-empty, trimmed value. Returns a plain-language problem if it is wrong.
fn check_value(f: &FieldDef, value: &str) -> Option<String> {
    match &f.kind {
        Kind::Text { max_len } => {
            if value.chars().count() > *max_len {
                Some(format!("{} must be {} characters or fewer.", f.label, max_len))
            } else {
                None
            }
        }
        Kind::Number { min, max } => match value.parse::<f64>() {
            Ok(n) if n.is_finite() && n >= *min && n <= *max => None,
            _ => Some(format!(
                "{} must be a number between {} and {}.",
                f.label, min, max
            )),
        },
        Kind::Choice(options) => {
            if options.iter().any(|o| o.eq_ignore_ascii_case(value)) {
                None
            } else {
                Some(format!("{} must be one of: {}.", f.label, options.join(", ")))
            }
        }
        Kind::Date => {
            if is_date(value) {
                None
            } else {
                Some(format!("{} must be a date in the form YYYY-MM-DD.", f.label))
            }
        }
    }
}

/// True for YYYY-MM-DD with a real calendar day (leap years included).
fn is_date(value: &str) -> bool {
    if !value.is_ascii() || value.len() != 10 {
        return false;
    }
    let b = value.as_bytes();
    if b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let num = |s: &str| -> Option<u32> {
        if s.bytes().all(|c| c.is_ascii_digit()) {
            s.parse::<u32>().ok()
        } else {
            None
        }
    };
    let (y, m, d) = match (num(&value[0..4]), num(&value[5..7]), num(&value[8..10])) {
        (Some(y), Some(m), Some(d)) => (y, m, d),
        _ => return false,
    };
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        _ => return false,
    };
    (1..=days).contains(&d)
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out
}

fn unescape(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            '\\' => out.push('\\'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            _ => return None,
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn problems(def: &FormDef, list: &[(&str, &str)]) -> Vec<String> {
        validate(def, &pairs(list)).err().unwrap_or_default()
    }

    fn one_field(kind: Kind, required: bool) -> FormDef {
        FormDef {
            name: "Test".to_string(),
            fields: vec![field("f", "Thing", kind, required)],
        }
    }

    #[test]
    fn text_length_is_checked() {
        let def = one_field(Kind::Text { max_len: 5 }, true);
        assert!(validate(&def, &pairs(&[("f", "hello")])).is_ok());
        assert_eq!(
            problems(&def, &[("f", "hello!")]),
            vec!["Thing must be 5 characters or fewer."]
        );
    }

    #[test]
    fn number_range_is_checked() {
        let def = one_field(Kind::Number { min: 1.0, max: 5.0 }, true);
        assert!(validate(&def, &pairs(&[("f", "3")])).is_ok());
        assert!(validate(&def, &pairs(&[("f", "1")])).is_ok());
        let msg = "Thing must be a number between 1 and 5.";
        assert_eq!(problems(&def, &[("f", "6")]), vec![msg]);
        assert_eq!(problems(&def, &[("f", "0")]), vec![msg]);
        assert_eq!(problems(&def, &[("f", "abc")]), vec![msg]);
        assert_eq!(problems(&def, &[("f", "NaN")]), vec![msg]);
    }

    #[test]
    fn signal_report_example_message() {
        let def = FormDef {
            name: "Signal".to_string(),
            fields: vec![field("rst", "Signal report", Kind::Number { min: 1.0, max: 5.0 }, true)],
        };
        assert_eq!(
            problems(&def, &[("rst", "9")]),
            vec!["Signal report must be a number between 1 and 5."]
        );
    }

    #[test]
    fn choice_is_checked_and_canonicalised() {
        let def = one_field(Kind::Choice(vec!["Yes".into(), "No".into()]), true);
        assert!(validate(&def, &pairs(&[("f", "yes")])).is_ok());
        assert_eq!(
            problems(&def, &[("f", "maybe")]),
            vec!["Thing must be one of: Yes, No."]
        );
        let bytes = encode(&def, &pairs(&[("f", "no")])).unwrap();
        assert_eq!(bytes, b"Test\nf=No".to_vec());
    }

    #[test]
    fn date_is_checked() {
        let def = one_field(Kind::Date, true);
        assert!(validate(&def, &pairs(&[("f", "2026-11-17")])).is_ok());
        assert!(validate(&def, &pairs(&[("f", "2024-02-29")])).is_ok());
        let msg = vec!["Thing must be a date in the form YYYY-MM-DD."];
        assert_eq!(problems(&def, &[("f", "2026-02-29")]), msg);
        assert_eq!(problems(&def, &[("f", "2026-13-01")]), msg);
        assert_eq!(problems(&def, &[("f", "2026-04-31")]), msg);
        assert_eq!(problems(&def, &[("f", "17/11/2026")]), msg);
        assert_eq!(problems(&def, &[("f", "2026-1l-01")]), msg);
        assert_eq!(problems(&def, &[("f", "2026-11-1é")]), msg);
    }

    #[test]
    fn required_and_unknown_fields_are_reported() {
        let def = checkin();
        let found = problems(&def, &[("callsign", "K1ABC"), ("nope", "x")]);
        assert!(found.contains(&"Unknown field \"nope\".".to_string()));
        assert!(found.contains(&"Location is required.".to_string()));
        assert!(found.contains(&"Status is required.".to_string()));
        assert!(found.contains(&"Traffic count is required.".to_string()));
        assert!(!found.iter().any(|p| p.starts_with("Callsign")));
    }

    #[test]
    fn optional_fields_may_be_empty() {
        let def = situation_report();
        let values = pairs(&[
            ("location", "Dubai Marina"),
            ("time", "14:30 UTC"),
            ("need", "Water"),
        ]);
        assert!(validate(&def, &values).is_ok());
    }

    #[test]
    fn encode_decode_round_trip() {
        let def = situation_report();
        let values = pairs(&[
            ("location", "Line 3 = east\\west"),
            ("time", "14:30"),
            ("need", "Water\nand food"),
            ("resources", "  two trucks  "),
        ]);
        let bytes = encode(&def, &values).unwrap();
        let (name, back) = decode(&bytes).unwrap();
        assert_eq!(name, "Situation report");
        assert_eq!(
            back,
            pairs(&[
                ("location", "Line 3 = east\\west"),
                ("time", "14:30"),
                ("need", "Water\nand food"),
                ("resources", "two trucks"),
            ])
        );
    }

    #[test]
    fn encode_refuses_invalid_values() {
        let def = checkin();
        let err = encode(&def, &pairs(&[("callsign", "K1ABC")])).unwrap_err();
        assert!(err.contains("Location is required."));
    }

    #[test]
    fn decode_rejects_bad_input() {
        assert!(decode(&[0xff, 0xfe]).is_none());
        assert!(decode(b"").is_none());
        assert!(decode(b"Name\nno-equals-here").is_none());
        assert!(decode(b"Name\nk=bad\\x").is_none());
        assert!(decode(b"Name\nk=trailing\\").is_none());
        assert_eq!(decode(b"Name\n"), Some(("Name".to_string(), vec![])));
    }

    #[test]
    fn checkin_example_size() {
        let def = checkin();
        let bytes = encode(
            &def,
            &pairs(&[
                ("callsign", "K1ABC"),
                ("location", "Dubai"),
                ("status", "Listening"),
                ("traffic", "0"),
            ]),
        )
        .unwrap();
        assert_eq!(
            bytes,
            b"Check-in\ncallsign=K1ABC\nlocation=Dubai\nstatus=Listening\ntraffic=0".to_vec()
        );
        assert_eq!(bytes.len(), 65);
    }

    #[test]
    fn builtin_forms_have_names_and_fields() {
        let forms = builtin_forms();
        assert_eq!(forms[0].name, "Check-in");
        assert_eq!(forms[1].name, "Situation report");
        assert_eq!(forms[1].fields.len(), 5);
    }
}
