//! Urgent (priority) alert with a two-step operator confirmation.
//!
//! An urgent alert is only ever encoded once the operator has passed both
//! steps, in order:
//!   1. `acknowledge_step1` after reading `STEP1_TEXT`.
//!   2. `confirm_step2` with the exact word `URGENT`, after reading `STEP2_TEXT`.
//!
//! The draft expires `DRAFT_TTL_S` seconds after it was created if step 2 has
//! not been given. Nothing in this module transmits; `encode` only builds bytes.

/// Explanation shown for step 1. Part of the API so every interface shows the same words.
pub const STEP1_TEXT: &str = "An urgent message jumps the queue. It interrupts other traffic when the channel is free, and it is delivered to the recipient ahead of normal traffic.";

/// Explanation shown for step 2. Part of the API so every interface shows the same words.
pub const STEP2_TEXT: &str = "Urgent is only for genuine emergencies or safety of life. Misuse may break amateur radio rules and is the operator's responsibility. To continue, type the word URGENT.";

/// The word the operator must type to pass step 2 (exact match).
pub const CONFIRM_WORD: &str = "URGENT";

/// Time, in seconds, a draft stays valid while waiting for step 2.
pub const DRAFT_TTL_S: u64 = 120;

const ENCODE_VERSION: u8 = 1;

/// A pending urgent message. Created fresh for every urgent alert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlertDraft {
    pub text: String,
    pub to: String,
    pub created_unix_s: u64,
}

/// Confirmation progress of an urgent alert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Waiting for step 1 to be acknowledged.
    One,
    /// Step 1 acknowledged; waiting for step 2 (the typed word).
    Two,
    /// Both steps passed; the alert may be encoded.
    Ready,
}

/// An urgent alert and its confirmation state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrgentAlert {
    draft: AlertDraft,
    step: Step,
}

impl UrgentAlert {
    /// Start a new urgent alert. The 2-minute clock starts at `now_unix_s`.
    pub fn new(text: &str, to: &str, now_unix_s: u64) -> Self {
        Self {
            draft: AlertDraft {
                text: text.to_string(),
                to: to.to_string(),
                created_unix_s: now_unix_s,
            },
            step: Step::One,
        }
    }

    /// Current confirmation step.
    pub fn step(&self) -> Step {
        self.step
    }

    /// The underlying draft.
    pub fn draft(&self) -> &AlertDraft {
        &self.draft
    }

    fn check_not_expired(&self, now_unix_s: u64) -> Result<(), String> {
        let elapsed = now_unix_s.saturating_sub(self.draft.created_unix_s);
        if elapsed >= DRAFT_TTL_S {
            Err(format!(
                "urgent draft expired after {DRAFT_TTL_S} seconds; start a new alert"
            ))
        } else {
            Ok(())
        }
    }

    /// Pass step 1. Only valid in `Step::One` and before the draft expires.
    pub fn acknowledge_step1(&mut self, now_unix_s: u64) -> Result<(), String> {
        self.check_not_expired(now_unix_s)?;
        match self.step {
            Step::One => {
                self.step = Step::Two;
                Ok(())
            }
            Step::Two => Err("step 1 is already acknowledged".to_string()),
            Step::Ready => Err("urgent alert is already confirmed".to_string()),
        }
    }

    /// Pass step 2. Requires step 1 to be acknowledged first, and `typed`
    /// must be exactly `URGENT`. A wrong word keeps the alert at step 2 so the
    /// operator can retry until the draft expires.
    pub fn confirm_step2(&mut self, typed: &str, now_unix_s: u64) -> Result<(), String> {
        self.check_not_expired(now_unix_s)?;
        match self.step {
            Step::One => Err("acknowledge step 1 before step 2".to_string()),
            Step::Ready => Err("urgent alert is already confirmed".to_string()),
            Step::Two => {
                if typed == CONFIRM_WORD {
                    self.step = Step::Ready;
                    Ok(())
                } else {
                    Err("type the word URGENT exactly to continue".to_string())
                }
            }
        }
    }

    /// Encode the alert. Fails unless both steps have passed.
    ///
    /// Layout: version (1 byte), to length (u8) + to bytes, text length (u16 BE) + text bytes.
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        if self.step != Step::Ready {
            return Err("urgent alert is not confirmed; both steps are required".to_string());
        }
        let to = self.draft.to.as_bytes();
        let text = self.draft.text.as_bytes();
        if to.len() > u8::MAX as usize {
            return Err("recipient is longer than 255 bytes".to_string());
        }
        if text.len() > u16::MAX as usize {
            return Err("message is longer than 65535 bytes".to_string());
        }
        let mut out = Vec::with_capacity(4 + to.len() + text.len());
        out.push(ENCODE_VERSION);
        out.push(to.len() as u8);
        out.extend_from_slice(to);
        out.extend_from_slice(&(text.len() as u16).to_be_bytes());
        out.extend_from_slice(text);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: u64 = 1_000_000;

    #[test]
    fn encode_fails_before_step2() {
        let mut a = UrgentAlert::new("fire at the pier", "KA1ABC", T0);
        assert!(a.encode().is_err());
        a.acknowledge_step1(T0).unwrap();
        assert_eq!(a.step(), Step::Two);
        assert!(a.encode().is_err());
    }

    #[test]
    fn step2_wrong_word_fails() {
        let mut a = UrgentAlert::new("fire", "KA1ABC", T0);
        a.acknowledge_step1(T0).unwrap();
        assert!(a.confirm_step2("urgent", T0).is_err());
        assert!(a.confirm_step2("URGENT ", T0).is_err());
        assert!(a.confirm_step2("", T0).is_err());
        assert_eq!(a.step(), Step::Two);
        assert!(a.encode().is_err());
    }

    #[test]
    fn step2_before_step1_fails() {
        let mut a = UrgentAlert::new("fire", "KA1ABC", T0);
        assert!(a.confirm_step2("URGENT", T0).is_err());
        assert_eq!(a.step(), Step::One);
        assert!(a.encode().is_err());
    }

    #[test]
    fn expires_after_two_minutes() {
        let mut a = UrgentAlert::new("fire", "KA1ABC", T0);
        a.acknowledge_step1(T0 + 119).unwrap();
        a.confirm_step2("URGENT", T0 + 119).unwrap();
        assert_eq!(a.step(), Step::Ready);

        let mut b = UrgentAlert::new("fire", "KA1ABC", T0);
        b.acknowledge_step1(T0).unwrap();
        assert!(b.confirm_step2("URGENT", T0 + 120).is_err());
        assert_eq!(b.step(), Step::Two);
        assert!(b.encode().is_err());

        let mut c = UrgentAlert::new("fire", "KA1ABC", T0);
        assert!(c.acknowledge_step1(T0 + 120).is_err());
        assert_eq!(c.step(), Step::One);
    }

    #[test]
    fn full_path_succeeds() {
        let mut a = UrgentAlert::new("fire at the pier", "KA1ABC", T0);
        a.acknowledge_step1(T0 + 5).unwrap();
        a.confirm_step2("URGENT", T0 + 10).unwrap();
        assert_eq!(a.step(), Step::Ready);
        let bytes = a.encode().unwrap();
        assert_eq!(bytes[0], ENCODE_VERSION);
        assert_eq!(bytes[1] as usize, "KA1ABC".len());
        assert!(bytes.ends_with(b"fire at the pier"));
        // A second confirmation is refused.
        assert!(a.confirm_step2("URGENT", T0 + 11).is_err());
    }

    #[test]
    fn step_texts_are_non_empty() {
        assert!(!STEP1_TEXT.is_empty());
        assert!(!STEP2_TEXT.is_empty());
    }
}
