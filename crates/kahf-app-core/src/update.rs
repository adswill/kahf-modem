//! Update check. Runs only when the operator asks, never in the background.

/// Where the newest release is announced.
const LATEST_URL: &str = "https://api.github.com/repos/adswill/kahf-modem/releases/latest";

/// The newest published release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Latest {
    pub version: String,
    pub url: String,
}

/// What a check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    UpToDate,
    Newer(Latest),
}

/// `1.2.3` or `v1.2.3` as numbers. Anything after a `-` or `+` is ignored.
fn parse_version(text: &str) -> Option<[u64; 3]> {
    let core = text.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let mut out = [0u64; 3];
    for slot in &mut out {
        *slot = parts.next()?.parse().ok()?;
    }
    parts.next().is_none().then_some(out)
}

/// Reads the release JSON and compares it with the running version.
pub fn judge(body: &str, current: &str) -> Result<Outcome, String> {
    let json: serde_json::Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let tag = json["tag_name"].as_str().ok_or("no tag_name in the answer")?;
    let url = json["html_url"].as_str().unwrap_or_default();
    let latest = parse_version(tag).ok_or_else(|| format!("unreadable version {tag}"))?;
    let now = parse_version(current).ok_or_else(|| format!("unreadable version {current}"))?;
    Ok(if latest > now {
        Outcome::Newer(Latest {
            version: tag.trim_start_matches('v').to_string(),
            url: url.to_string(),
        })
    } else {
        Outcome::UpToDate
    })
}

/// Asks the release server. Blocking: call it off the UI thread.
pub fn check(current: &str) -> Result<Outcome, String> {
    let body = ureq::get(LATEST_URL)
        .set("User-Agent", "kahf-update-check")
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .map_err(|e| e.to_string())?
        .into_string()
        .map_err(|e| e.to_string())?;
    judge(&body, current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_prefixed_versions() {
        assert_eq!(parse_version("1.2.3"), Some([1, 2, 3]));
        assert_eq!(parse_version("v1.0.10"), Some([1, 0, 10]));
        assert_eq!(parse_version("1.0.0-rc1"), Some([1, 0, 0]));
        assert_eq!(parse_version("1.0"), None);
        assert_eq!(parse_version("1.0.0.1"), None);
        assert_eq!(parse_version("x"), None);
    }

    #[test]
    fn newer_release_is_reported() {
        let body = r#"{"tag_name":"v1.0.10","html_url":"https://example.org/r"}"#;
        let got = judge(body, "1.0.9").unwrap();
        assert_eq!(
            got,
            Outcome::Newer(Latest {
                version: "1.0.10".into(),
                url: "https://example.org/r".into()
            })
        );
    }

    #[test]
    fn same_or_older_is_up_to_date() {
        let body = r#"{"tag_name":"v1.0.0","html_url":""}"#;
        assert_eq!(judge(body, "1.0.0").unwrap(), Outcome::UpToDate);
        assert_eq!(judge(body, "2.0.0").unwrap(), Outcome::UpToDate);
    }

    #[test]
    fn bad_answers_are_errors() {
        assert!(judge("not json", "1.0.0").is_err());
        assert!(judge("{}", "1.0.0").is_err());
        assert!(judge(r#"{"tag_name":"nightly"}"#, "1.0.0").is_err());
    }
}
