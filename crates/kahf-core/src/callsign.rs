//! Callsign format check. Format only: no database lookup, and no blocking of
//! unusual but valid calls. Messages are plain sentences for the UI.

const MIN_LEN: usize = 3;
const MAX_LEN: usize = 10;
const SUFFIXES: [&str; 3] = ["P", "M", "QRP"];
const EXAMPLE: &str = "BB1BBB";

/// Checks a callsign and returns it normalised to upper case.
///
/// Leading and trailing spaces are trimmed. Letters and digits are allowed,
/// plus an optional `/P`, `/M` or `/QRP` suffix. At least one letter and one
/// digit are needed in the part before the suffix.
pub fn check(input: &str) -> Result<String, String> {
    let call = input.trim().to_ascii_uppercase();
    let len = call.chars().count();

    if len == 0 {
        return Err("Type your callsign.".to_string());
    }
    if len < MIN_LEN {
        return Err(format!("A callsign needs at least {MIN_LEN} characters."));
    }
    if len > MAX_LEN {
        return Err(format!("A callsign can be at most {MAX_LEN} characters."));
    }

    let (base, suffix) = match call.split_once('/') {
        Some((base, suffix)) => (base, Some(suffix)),
        None => (call.as_str(), None),
    };

    if let Some(suffix) = suffix {
        if !SUFFIXES.contains(&suffix) {
            return Err("The suffix must be /P, /M or /QRP.".to_string());
        }
    }

    if !base.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(
            "A callsign can only use letters and digits, plus an optional /P, /M or /QRP ending.".to_string(),
        );
    }

    let has_letter = base.chars().any(|c| c.is_ascii_alphabetic());
    let has_digit = base.chars().any(|c| c.is_ascii_digit());
    if !has_letter || !has_digit {
        return Err(format!(
            "A callsign needs at least one letter and one digit, for example {EXAMPLE}."
        ));
    }

    Ok(call)
}

/// A live hint for a field that is still being typed. Returns `None` when the
/// input is fine.
pub fn hint(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.ends_with('/') || trimmed.chars().count() < MIN_LEN {
        return Some("Keep typing".to_string());
    }
    check(input).err()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callsign_valid_examples_pass() {
        assert_eq!(check("BB1BBB").as_deref(), Ok("BB1BBB"));
        assert_eq!(check("K1ABC").as_deref(), Ok("K1ABC"));
        assert_eq!(check("G4XYZ/P").as_deref(), Ok("G4XYZ/P"));
        assert_eq!(check("W1AW/M").as_deref(), Ok("W1AW/M"));
        assert_eq!(check("K1ABC/QRP").as_deref(), Ok("K1ABC/QRP"));
    }

    #[test]
    fn callsign_lowercase_is_normalised() {
        assert_eq!(check("bb1bbb").as_deref(), Ok("BB1BBB"));
        assert_eq!(check("g4xyz/p").as_deref(), Ok("G4XYZ/P"));
    }

    #[test]
    fn callsign_surrounding_spaces_are_trimmed() {
        assert_eq!(check("  K1ABC \t").as_deref(), Ok("K1ABC"));
    }

    #[test]
    fn callsign_random_numbers_fail_with_clear_sentence() {
        let err = check("12345").unwrap_err();
        assert_eq!(
            err,
            "A callsign needs at least one letter and one digit, for example BB1BBB."
        );
        assert!(check("ABCDEF").unwrap_err().contains("one digit"));
    }

    #[test]
    fn callsign_very_long_input_fails() {
        let err = check("AB1CDEFGHIJKLMNOP").unwrap_err();
        assert_eq!(err, "A callsign can be at most 10 characters.");
    }

    #[test]
    fn callsign_too_short_fails() {
        assert_eq!(check("").unwrap_err(), "Type your callsign.");
        assert_eq!(
            check("K1").unwrap_err(),
            "A callsign needs at least 3 characters."
        );
    }

    #[test]
    fn callsign_bad_characters_and_suffix_fail() {
        assert!(check("K1 ABC").is_err());
        assert!(check("K1-ABC").is_err());
        assert_eq!(
            check("K1ABC/X").unwrap_err(),
            "The suffix must be /P, /M or /QRP."
        );
        assert_eq!(check("K1ABC/").unwrap_err(), "The suffix must be /P, /M or /QRP.");
    }

    #[test]
    fn callsign_hint_while_typing() {
        assert_eq!(hint("").as_deref(), Some("Keep typing"));
        assert_eq!(hint("K1").as_deref(), Some("Keep typing"));
        assert_eq!(hint("K1ABC/").as_deref(), Some("Keep typing"));
        assert_eq!(hint("BB1BBB"), None);
        assert_eq!(hint("G4XYZ/P"), None);
        assert!(hint("12345").is_some());
    }
}
