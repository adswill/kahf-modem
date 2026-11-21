//! Callsigns: what is accepted, and what to tell the person when it is not.

/// Upper case, no spaces around.
pub fn normalise(text: &str) -> String {
    text.trim().to_uppercase()
}

/// Letters and digits, 3 to 10 characters, with at least one digit and one letter.
pub fn valid(call: &str) -> bool {
    let ok_len = (3..=10).contains(&call.len());
    ok_len
        && call.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        && call.chars().any(|c| c.is_ascii_digit())
        && call.chars().any(|c| c.is_ascii_uppercase())
}

/// What is wrong with a callsign being typed, in a sentence, or `None` when nothing is (or the
/// field is empty).
pub fn problem(text: &str) -> Option<&'static str> {
    let c = normalise(text);
    if c.is_empty() {
        return None;
    }
    if !c
        .chars()
        .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '/')
    {
        return Some("Use letters and numbers only.");
    }
    if c.len() < 3 {
        return Some("A callsign has at least 3 characters.");
    }
    if c.len() > 10 {
        return Some("That is too long for a callsign.");
    }
    if !c.chars().any(|ch| ch.is_ascii_digit()) {
        return Some("A callsign has at least one number, like KA1ABC.");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_callsigns() {
        assert!(valid("KA1ABC"));
        assert!(valid("9A1AA"));
        assert!(!valid("ABC"));
        assert!(!valid("123"));
        assert!(!valid("KA1/ABC"));
        assert!(!valid("KA1ABCDEFGH"));
    }

    #[test]
    fn problems_are_explained() {
        assert_eq!(problem(""), None);
        assert_eq!(problem("ka1abc"), None);
        assert!(problem("K!").is_some());
        assert!(problem("KA").unwrap().contains("3"));
        assert!(problem("KAABC").unwrap().contains("number"));
        assert!(problem("KA1ABCDEFGHIJ").unwrap().contains("long"));
    }
}
