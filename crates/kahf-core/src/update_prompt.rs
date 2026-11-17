//! Update prompt state machine.
//!
//! Pure logic only: no window code and no wall-clock reads. The interface draws
//! the state and the daemon reads `may_transmit` / `may_receive`. Every time is
//! passed in as milliseconds.

/// Shown on a required release before the user has decided.
pub const REQUIRED_TEXT: &str =
    "This update is required. KAHF cannot transmit or receive until it is installed.";

/// Shown in SkipConfirm after the user chose to skip a required release.
pub const REQUIRED_SKIP_TEXT: &str = "KAHF will not work until this update is installed. Your station cannot transmit or receive on this version. If you skip, the only thing you can do is look at this message.";

/// UpToDate closes itself after this long.
const UP_TO_DATE_CLOSE_MS: u64 = 5_000;

/// The accept button in SkipConfirm stays disabled for this long.
const SKIP_ACCEPT_DELAY_MS: u64 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Optional,
    Required,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub kind: Kind,
    pub notes: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// A version check is in flight.
    Checking,
    /// Already on the latest version; closes itself after 5 s.
    UpToDate { since_ms: u64 },
    /// A newer release is offered (Optional or Required, see `Release::kind`).
    Available(Release),
    /// User pressed Skip on a required release; accept unlocks after 10 s.
    SkipConfirm { release: Release, since_ms: u64 },
    /// Required release skipped and accepted: transmit and receive are blocked
    /// until `install_done` reports the required version or newer.
    RequiredLocked(Release),
    /// Nothing to show; the app keeps working.
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdatePrompt {
    state: State,
}

impl Default for UpdatePrompt {
    fn default() -> Self {
        Self::new()
    }
}

impl UpdatePrompt {
    /// Idle prompt with nothing to show.
    pub fn new() -> Self {
        Self { state: State::Done }
    }

    /// Prompt that has started a version check.
    pub fn checking() -> Self {
        Self {
            state: State::Checking,
        }
    }

    /// Feed the result of a version check. Ignored while locked or while the
    /// skip confirmation is open, because neither may be replaced by a check.
    pub fn on_check_result(&mut self, now_ms: u64, latest: Option<Release>, current: &str) {
        if matches!(self.state, State::RequiredLocked(_) | State::SkipConfirm { .. }) {
            return;
        }
        self.state = match latest {
            Some(release) => match is_newer(&release.version, current) {
                Ok(false) => State::UpToDate { since_ms: now_ms },
                // Cannot compare the versions: offer the release rather than hide it.
                _ => State::Available(release),
            },
            None => State::UpToDate { since_ms: now_ms },
        };
    }

    /// Advance time. UpToDate closes after 5 s.
    pub fn tick(&mut self, now_ms: u64) {
        if let State::UpToDate { since_ms } = self.state {
            if now_ms.saturating_sub(since_ms) >= UP_TO_DATE_CLOSE_MS {
                self.state = State::Done;
            }
        }
    }

    /// Skip pressed. Optional: back to Done. Required: open SkipConfirm.
    pub fn skip(&mut self, now_ms: u64) {
        let next = match &self.state {
            State::Available(release) if release.kind == Kind::Optional => State::Done,
            State::Available(release) => State::SkipConfirm {
                release: release.clone(),
                since_ms: now_ms,
            },
            _ => return,
        };
        self.state = next;
    }

    /// Back out of SkipConfirm to the required release.
    pub fn cancel_skip(&mut self) {
        let state = std::mem::replace(&mut self.state, State::Done);
        self.state = match state {
            State::SkipConfirm { release, .. } => State::Available(release),
            other => other,
        };
    }

    /// Accept the skip. Fails until 10 s after SkipConfirm was entered.
    pub fn confirm_skip(&mut self, now_ms: u64) -> Result<(), String> {
        let release = match &self.state {
            State::SkipConfirm { release, .. } => release.clone(),
            _ => return Err("no skip is waiting for confirmation".to_string()),
        };
        let left = self.remaining_ms(now_ms);
        if left > 0 {
            return Err(format!("accept unlocks in {} s", left.div_ceil(1_000)));
        }
        self.state = State::RequiredLocked(release);
        Ok(())
    }

    /// Update pressed. An Optional release moves to Done (the installer takes
    /// over). A Required release stays put until `install_done`.
    pub fn update(&mut self) {
        if matches!(&self.state, State::Available(r) if r.kind == Kind::Optional) {
            self.state = State::Done;
        }
    }

    /// Report that an install finished with version `installed`. Only accepted
    /// when a release is pending and `installed` is the required version or newer.
    pub fn install_done(&mut self, installed: &str) -> Result<(), String> {
        let required = match &self.state {
            State::Available(r) | State::RequiredLocked(r) => r.version.clone(),
            State::SkipConfirm { release, .. } => release.version.clone(),
            _ => return Err("no update is pending".to_string()),
        };
        if is_newer(&required, installed)? {
            return Err(format!(
                "installed version {installed} is older than required {required}"
            ));
        }
        self.state = State::Done;
        Ok(())
    }

    /// Close the prompt. Allowed for UpToDate, Checking, Done and Optional.
    /// A required release cannot be closed: it must be updated or skipped.
    pub fn close(&mut self) {
        let closable = match &self.state {
            State::Checking | State::UpToDate { .. } | State::Done => true,
            State::Available(release) => release.kind == Kind::Optional,
            State::SkipConfirm { .. } | State::RequiredLocked(_) => false,
        };
        if closable {
            self.state = State::Done;
        }
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    /// Seconds left before the SkipConfirm accept button enables (0 otherwise).
    pub fn seconds_until_accept(&self, now_ms: u64) -> u32 {
        match &self.state {
            State::SkipConfirm { .. } => self.remaining_ms(now_ms).div_ceil(1_000) as u32,
            _ => 0,
        }
    }

    /// False only while RequiredLocked.
    pub fn may_transmit(&self) -> bool {
        !self.is_locked()
    }

    /// False only while RequiredLocked.
    pub fn may_receive(&self) -> bool {
        !self.is_locked()
    }

    fn is_locked(&self) -> bool {
        matches!(self.state, State::RequiredLocked(_))
    }

    /// Milliseconds until accept unlocks; 0 when not in SkipConfirm or unlocked.
    fn remaining_ms(&self, now_ms: u64) -> u64 {
        match &self.state {
            State::SkipConfirm { since_ms, .. } => {
                SKIP_ACCEPT_DELAY_MS.saturating_sub(now_ms.saturating_sub(*since_ms))
            }
            _ => 0,
        }
    }
}

/// Parse a dotted version such as "3.10" into numeric parts.
fn parse_version(v: &str) -> Result<Vec<u64>, String> {
    v.split('.')
        .map(|part| part.parse::<u64>().map_err(|_| format!("bad version '{v}'")))
        .collect()
}

/// True when `a` is strictly newer than `b`. Parts compare as numbers, so
/// "3.10" is newer than "3.9", and missing parts count as 0 ("3.0" == "3").
fn is_newer(a: &str, b: &str) -> Result<bool, String> {
    let mut x = parse_version(a)?;
    let mut y = parse_version(b)?;
    let n = x.len().max(y.len());
    x.resize(n, 0);
    y.resize(n, 0);
    Ok(x > y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(version: &str, kind: Kind) -> Release {
        Release {
            version: version.to_string(),
            kind,
            notes: String::new(),
        }
    }

    #[test]
    fn up_to_date_closes_after_5_seconds() {
        let mut p = UpdatePrompt::checking();
        p.on_check_result(1_000, None, "3.9");
        assert_eq!(p.state(), &State::UpToDate { since_ms: 1_000 });
        p.tick(5_999);
        assert!(matches!(p.state(), State::UpToDate { .. }));
        p.tick(6_000);
        assert_eq!(p.state(), &State::Done);
    }

    #[test]
    fn up_to_date_can_be_closed_early() {
        let mut p = UpdatePrompt::checking();
        p.on_check_result(0, Some(rel("3.9", Kind::Optional)), "3.9");
        assert!(matches!(p.state(), State::UpToDate { .. }));
        p.close();
        assert_eq!(p.state(), &State::Done);
    }

    #[test]
    fn optional_skip_returns_to_done_and_keeps_working() {
        let mut p = UpdatePrompt::checking();
        p.on_check_result(0, Some(rel("3.10", Kind::Optional)), "3.9");
        assert!(matches!(p.state(), State::Available(_)));
        p.skip(100);
        assert_eq!(p.state(), &State::Done);
        assert!(p.may_transmit());
        assert!(p.may_receive());
    }

    #[test]
    fn required_skip_opens_confirm_and_locks_after_10_seconds() {
        let mut p = UpdatePrompt::new();
        p.on_check_result(0, Some(rel("4.0", Kind::Required)), "3.9");
        p.skip(1_000);
        assert!(matches!(p.state(), State::SkipConfirm { .. }));
        assert_eq!(p.seconds_until_accept(1_000), 10);
        assert!(p.confirm_skip(1_000 + 9_999).is_err());
        assert!(matches!(p.state(), State::SkipConfirm { .. }));
        assert_eq!(p.seconds_until_accept(1_000 + 9_999), 1);
        assert!(p.confirm_skip(1_000 + 10_000).is_ok());
        assert!(matches!(p.state(), State::RequiredLocked(_)));
        assert!(!p.may_transmit());
        assert!(!p.may_receive());
    }

    #[test]
    fn required_prompt_cannot_be_closed() {
        let mut p = UpdatePrompt::new();
        p.on_check_result(0, Some(rel("4.0", Kind::Required)), "3.9");
        p.close();
        assert!(matches!(p.state(), State::Available(_)));
    }

    #[test]
    fn cancel_skip_returns_to_required() {
        let mut p = UpdatePrompt::new();
        p.on_check_result(0, Some(rel("4.0", Kind::Required)), "3.9");
        p.skip(0);
        p.cancel_skip();
        assert_eq!(p.state(), &State::Available(rel("4.0", Kind::Required)));
        assert!(p.confirm_skip(20_000).is_err());
    }

    #[test]
    fn install_done_with_older_version_fails() {
        let mut p = UpdatePrompt::new();
        p.on_check_result(0, Some(rel("3.10", Kind::Required)), "3.9");
        p.skip(0);
        p.confirm_skip(10_000).unwrap();
        assert!(p.install_done("3.9").is_err());
        assert!(matches!(p.state(), State::RequiredLocked(_)));
        assert!(!p.may_transmit());
        assert!(p.install_done("3.10").is_ok());
        assert_eq!(p.state(), &State::Done);
        assert!(p.may_transmit());
    }

    #[test]
    fn install_done_accepts_newer_version() {
        let mut p = UpdatePrompt::new();
        p.on_check_result(0, Some(rel("3.10", Kind::Required)), "3.9");
        p.skip(0);
        p.confirm_skip(10_000).unwrap();
        assert!(p.install_done("3.11").is_ok());
        assert!(p.may_receive());
    }

    #[test]
    fn locked_state_ignores_new_check_results() {
        let mut p = UpdatePrompt::new();
        p.on_check_result(0, Some(rel("4.0", Kind::Required)), "3.9");
        p.skip(0);
        p.confirm_skip(10_000).unwrap();
        p.on_check_result(20_000, None, "4.0");
        assert!(matches!(p.state(), State::RequiredLocked(_)));
        assert!(!p.may_transmit());
    }

    #[test]
    fn update_on_optional_moves_to_done_and_required_waits_for_install() {
        let mut opt = UpdatePrompt::new();
        opt.on_check_result(0, Some(rel("3.10", Kind::Optional)), "3.9");
        opt.update();
        assert_eq!(opt.state(), &State::Done);

        let mut req = UpdatePrompt::new();
        req.on_check_result(0, Some(rel("3.10", Kind::Required)), "3.9");
        req.update();
        assert!(matches!(req.state(), State::Available(_)));
    }

    #[test]
    fn version_comparison_is_numeric() {
        assert_eq!(is_newer("3.10", "3.9"), Ok(true));
        assert_eq!(is_newer("3.9", "3.10"), Ok(false));
        assert_eq!(is_newer("3.0", "3"), Ok(false));
        assert!(is_newer("3.x", "3.9").is_err());

        let mut p = UpdatePrompt::new();
        p.on_check_result(0, Some(rel("3.10", Kind::Optional)), "3.9");
        assert!(matches!(p.state(), State::Available(_)));

        let mut q = UpdatePrompt::new();
        q.on_check_result(0, Some(rel("3.9", Kind::Optional)), "3.10");
        assert!(matches!(q.state(), State::UpToDate { .. }));
    }

    #[test]
    fn skip_text_is_exact() {
        assert_eq!(
            REQUIRED_SKIP_TEXT,
            "KAHF will not work until this update is installed. Your station cannot transmit or receive on this version. If you skip, the only thing you can do is look at this message."
        );
        assert_eq!(
            REQUIRED_TEXT,
            "This update is required. KAHF cannot transmit or receive until it is installed."
        );
    }
}
