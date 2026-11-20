//! When to register, when to fetch the next schedule, and how to back off.

use crate::client::{Client, Error, Registration, Schedule, SlotRequest, Transport};
use crate::wire::Area;
use kahf_core::auth::StationKey;

/// Settings for [`RelaySync`]. Times are seconds.
#[derive(Clone, Debug, PartialEq)]
pub struct SyncConfig {
    pub callsign: String,
    pub area: Area,
    /// Sent with every schedule call.
    pub requests: Vec<SlotRequest>,
    /// Renew the registration this often. The server keeps it for 48 hours.
    pub register_every_s: u64,
    /// Ask for the next schedule this long before the current one ends.
    pub schedule_lead_s: u64,
    /// Wait this long after a schedule with no slots.
    pub empty_retry_s: u64,
    /// Shortest wait between two schedule calls, so a schedule that is
    /// already nearly over (or a station clock that runs ahead) cannot make
    /// the client ask again and again.
    pub min_refetch_s: u64,
    /// First wait after a failure; it doubles with each failure in a row.
    pub backoff_min_s: u64,
    /// Longest wait after failures. Errors that a retry cannot fix (a refused
    /// callsign, for example) wait this long.
    pub backoff_max_s: u64,
}

impl SyncConfig {
    pub fn new(callsign: &str, area: Area, requests: Vec<SlotRequest>) -> Self {
        Self {
            callsign: callsign.to_string(),
            area,
            requests,
            register_every_s: 12 * 3600,
            schedule_lead_s: 60,
            empty_retry_s: 300,
            min_refetch_s: 30,
            backoff_min_s: 5,
            backoff_max_s: 900,
        }
    }
}

/// What one call to [`RelaySync::step`] did.
#[derive(Debug, PartialEq)]
pub enum Step {
    /// Nothing to do before [`RelaySync::next_attempt_unix`].
    Waiting,
    Registered(Registration),
    /// A new schedule to hand to the relay.
    Schedule(Schedule),
    /// The request failed; the next attempt is after a back-off.
    Failed(Error),
}

/// Keeps a relay registered and its schedule fresh. It holds no clock and
/// no thread: the caller passes the time and decides how to wait.
#[derive(Debug)]
pub struct RelaySync {
    cfg: SyncConfig,
    registered_at: Option<u64>,
    /// End of the last schedule received (0 before the first).
    schedule_until: u64,
    next_attempt: u64,
    failures: u32,
    /// The last failure was "not registered". A registration that follows it
    /// keeps the failure count, so a server that keeps forgetting the relay
    /// is not asked twice every few seconds.
    after_not_registered: bool,
}

impl RelaySync {
    pub fn new(cfg: SyncConfig) -> Self {
        Self {
            cfg,
            registered_at: None,
            schedule_until: 0,
            next_attempt: 0,
            failures: 0,
            after_not_registered: false,
        }
    }

    /// Unix time of the next request.
    pub fn next_attempt_unix(&self) -> u64 {
        self.next_attempt
    }

    /// Failed requests in a row.
    pub fn failures(&self) -> u32 {
        self.failures
    }

    /// True after a registration that has not been renewed or found expired since.
    pub fn is_registered(&self) -> bool {
        self.registered_at.is_some()
    }

    /// Replace the slot requests (for example after the relay's channels
    /// changed). The current schedule is dropped and a new one is asked for
    /// on the next step.
    pub fn set_requests(&mut self, requests: Vec<SlotRequest>) {
        self.cfg.requests = requests;
        self.schedule_until = 0;
        self.next_attempt = 0;
    }

    /// The relay refused the last schedule. Ask for a new one after a back-off
    /// instead of waiting for the refused one to run out.
    pub fn schedule_rejected(&mut self, now_unix: u64) {
        self.schedule_until = 0;
        self.failures = self.failures.saturating_add(1);
        self.next_attempt = now_unix + self.backoff();
    }

    /// Make at most one request, if one is due at `now_unix`.
    pub fn step<T: Transport>(&mut self, client: &Client<T>, key: &StationKey, now_unix: u64) -> Step {
        if now_unix < self.next_attempt {
            return Step::Waiting;
        }
        let register_due = self
            .registered_at
            .is_none_or(|at| now_unix >= at.saturating_add(self.cfg.register_every_s));
        if register_due {
            return match client.register(key, &self.cfg.callsign, &self.cfg.area) {
                Ok(r) => {
                    self.registered_at = Some(now_unix);
                    if !self.after_not_registered {
                        self.failures = 0;
                    }
                    // Ask for slots straight away.
                    self.next_attempt = now_unix;
                    Step::Registered(r)
                }
                Err(e) => self.fail(e, now_unix),
            };
        }
        if now_unix.saturating_add(self.cfg.schedule_lead_s) < self.schedule_until {
            self.next_attempt = self.next_schedule_time(now_unix);
            return Step::Waiting;
        }
        match client.schedule(key, &self.cfg.callsign, &self.cfg.requests) {
            Ok(s) => {
                self.failures = 0;
                self.after_not_registered = false;
                self.schedule_until = s.end_unix().unwrap_or(0);
                self.next_attempt = match s.end_unix() {
                    Some(_) => self
                        .next_schedule_time(now_unix)
                        .max(now_unix + self.cfg.min_refetch_s),
                    None => now_unix + self.cfg.empty_retry_s,
                };
                Step::Schedule(s)
            }
            Err(e) => {
                if e.is_not_registered() {
                    // Expired on the server: register again on the next attempt.
                    self.registered_at = None;
                }
                self.fail(e, now_unix)
            }
        }
    }

    /// The earlier of "lead seconds before the schedule ends" and the next renewal.
    fn next_schedule_time(&self, now_unix: u64) -> u64 {
        let fetch = self.schedule_until.saturating_sub(self.cfg.schedule_lead_s);
        let renew = self
            .registered_at
            .map_or(now_unix, |at| at.saturating_add(self.cfg.register_every_s));
        fetch.min(renew).max(now_unix)
    }

    fn fail(&mut self, e: Error, now_unix: u64) -> Step {
        self.failures = self.failures.saturating_add(1);
        self.after_not_registered = e.is_not_registered();
        let wait = if e.is_retryable() || e.is_not_registered() {
            self.backoff()
        } else {
            self.cfg.backoff_max_s
        };
        self.next_attempt = now_unix + wait;
        Step::Failed(e)
    }

    /// Doubling wait for the current failure count, capped.
    fn backoff(&self) -> u64 {
        let doublings = self.failures.saturating_sub(1).min(20);
        self.cfg
            .backoff_min_s
            .saturating_mul(1u64 << doublings)
            .min(self.cfg.backoff_max_s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::tests::{key, FakeTransport};
    use crate::wire::ChannelGrid;
    use kahf_core::relay::Direction;
    use serde_json::json;

    const T0: u64 = 1_700_000_000;

    fn cfg() -> SyncConfig {
        let req = SlotRequest {
            channel_khz: ChannelGrid::RELAY_17M.centre_khz(0).unwrap(),
            duration_s: 60,
            direction: Direction::Receive,
            urgent: false,
            latency_budget_s: None,
        };
        SyncConfig::new("LY1ABC", Area::parse("N54E25").unwrap(), vec![req])
    }

    fn registered(t: &FakeTransport) {
        t.answer(
            200,
            json!({"ok": true, "callsign": "LY1ABC", "area": "N54E25", "expires_after_s": 172800}),
        );
    }

    fn schedule(t: &FakeTransport, start: u64, duration: u64) {
        t.answer(
            200,
            json!({"relay": "LY1ABC", "slots": [{"channel": 0, "start": start, "duration": duration, "direction": "rx"}], "unplaced": []}),
        );
    }

    fn paths(t: &FakeTransport) -> Vec<String> {
        t.sent
            .borrow()
            .iter()
            .map(|(u, _)| u.rsplit('/').next().unwrap().to_string())
            .collect()
    }

    #[test]
    fn registers_then_fetches_then_waits_for_the_schedule_to_run_out() {
        let t = FakeTransport::default();
        let c = Client::new(&t, "https://kahf.dev");
        let mut s = RelaySync::new(cfg());
        registered(&t);
        schedule(&t, T0, 600);

        assert!(matches!(s.step(&c, &key(), T0), Step::Registered(_)));
        assert!(s.is_registered());
        assert!(matches!(s.step(&c, &key(), T0), Step::Schedule(sc) if sc.slots.len() == 1));
        assert_eq!(
            s.next_attempt_unix(),
            T0 + 600 - 60,
            "next fetch is one lead before the end"
        );
        assert_eq!(s.step(&c, &key(), T0 + 100), Step::Waiting);
        assert_eq!(paths(&t), ["register", "schedule"]);

        schedule(&t, T0 + 600, 600);
        assert!(matches!(s.step(&c, &key(), T0 + 540), Step::Schedule(_)));
        assert_eq!(paths(&t), ["register", "schedule", "schedule"]);
    }

    #[test]
    fn offline_backs_off_exponentially_up_to_the_cap() {
        let t = FakeTransport::default();
        let c = Client::new(&t, "https://kahf.dev");
        let mut s = RelaySync::new(cfg());
        let mut now = T0;
        let mut waits = Vec::new();
        for _ in 0..10 {
            t.offline();
            assert!(matches!(s.step(&c, &key(), now), Step::Failed(Error::Offline(_))));
            waits.push(s.next_attempt_unix() - now);
            assert_eq!(
                s.step(&c, &key(), s.next_attempt_unix() - 1),
                Step::Waiting,
                "no request before the wait ends"
            );
            now = s.next_attempt_unix();
        }
        assert_eq!(waits, [5, 10, 20, 40, 80, 160, 320, 640, 900, 900]);

        // Back online: the back-off resets after a schedule.
        registered(&t);
        schedule(&t, now, 600);
        assert!(matches!(s.step(&c, &key(), now), Step::Registered(_)));
        assert!(matches!(s.step(&c, &key(), now), Step::Schedule(_)));
        assert_eq!(s.failures(), 0);
    }

    #[test]
    fn expired_registration_registers_again() {
        let t = FakeTransport::default();
        let c = Client::new(&t, "https://kahf.dev");
        let mut s = RelaySync::new(cfg());
        registered(&t);
        schedule(&t, T0, 120);
        s.step(&c, &key(), T0);
        s.step(&c, &key(), T0);

        t.answer(404, json!({"error": "relay is not registered"}));
        assert!(matches!(s.step(&c, &key(), T0 + 60), Step::Failed(e) if e.is_not_registered()));
        assert!(!s.is_registered());
        let next = s.next_attempt_unix();
        assert_eq!(next, T0 + 65);
        registered(&t);
        schedule(&t, next, 120);
        assert!(matches!(s.step(&c, &key(), next), Step::Registered(_)));
        assert!(matches!(s.step(&c, &key(), next), Step::Schedule(_)));
        assert_eq!(
            paths(&t),
            ["register", "schedule", "schedule", "register", "schedule"]
        );
    }

    #[test]
    fn refused_registration_waits_the_longest_back_off() {
        let t = FakeTransport::default();
        let c = Client::new(&t, "https://kahf.dev");
        let mut s = RelaySync::new(cfg());
        t.answer(409, json!({"error": "callsign is registered with another key"}));
        assert!(matches!(
            s.step(&c, &key(), T0),
            Step::Failed(Error::Server { status: 409, .. })
        ));
        assert_eq!(s.next_attempt_unix(), T0 + 900);
    }

    #[test]
    fn renews_the_registration_before_it_lapses() {
        let t = FakeTransport::default();
        let c = Client::new(&t, "https://kahf.dev");
        let mut cfg = cfg();
        cfg.register_every_s = 1000;
        let mut s = RelaySync::new(cfg);
        registered(&t);
        schedule(&t, T0, 5000);
        s.step(&c, &key(), T0);
        s.step(&c, &key(), T0);
        assert_eq!(
            s.next_attempt_unix(),
            T0 + 1000,
            "renewal comes before the schedule runs out"
        );
        registered(&t);
        assert!(matches!(s.step(&c, &key(), T0 + 1000), Step::Registered(_)));
        assert_eq!(
            s.step(&c, &key(), T0 + 1000),
            Step::Waiting,
            "schedule still has time left"
        );
    }

    #[test]
    fn empty_schedule_retries_later() {
        let t = FakeTransport::default();
        let c = Client::new(&t, "https://kahf.dev");
        let mut s = RelaySync::new(cfg());
        registered(&t);
        t.answer(
            200,
            json!({"relay": "LY1ABC", "slots": [], "unplaced": [{"index": 0, "reason": "full"}]}),
        );
        s.step(&c, &key(), T0);
        assert!(matches!(s.step(&c, &key(), T0), Step::Schedule(sc) if sc.slots.is_empty()));
        assert_eq!(s.next_attempt_unix(), T0 + 300);
    }

    #[test]
    fn a_schedule_that_is_nearly_over_does_not_cause_a_tight_loop() {
        let t = FakeTransport::default();
        let c = Client::new(&t, "https://kahf.dev");
        let mut s = RelaySync::new(cfg());
        registered(&t);
        schedule(&t, T0, 60);
        s.step(&c, &key(), T0);
        assert!(matches!(s.step(&c, &key(), T0), Step::Schedule(_)));
        assert_eq!(s.next_attempt_unix(), T0 + 30, "waits the minimum gap");
        assert_eq!(s.step(&c, &key(), T0), Step::Waiting);
    }

    #[test]
    fn a_rejected_schedule_is_asked_for_again_after_a_back_off() {
        let t = FakeTransport::default();
        let c = Client::new(&t, "https://kahf.dev");
        let mut s = RelaySync::new(cfg());
        registered(&t);
        schedule(&t, T0, 3600);
        s.step(&c, &key(), T0);
        s.step(&c, &key(), T0);
        s.schedule_rejected(T0);
        assert_eq!(s.next_attempt_unix(), T0 + 5);
        schedule(&t, T0 + 5, 3600);
        assert!(matches!(s.step(&c, &key(), T0 + 5), Step::Schedule(_)));
    }

    #[test]
    fn new_requests_drop_the_current_schedule() {
        let t = FakeTransport::default();
        let c = Client::new(&t, "https://kahf.dev");
        let mut s = RelaySync::new(cfg());
        registered(&t);
        schedule(&t, T0, 3600);
        s.step(&c, &key(), T0);
        s.step(&c, &key(), T0);
        s.set_requests(cfg().requests);
        schedule(&t, T0 + 1, 3600);
        assert!(matches!(s.step(&c, &key(), T0 + 1), Step::Schedule(_)));
    }

    #[test]
    fn registering_after_an_outage_resets_the_back_off() {
        let t = FakeTransport::default();
        let c = Client::new(&t, "https://kahf.dev");
        let mut s = RelaySync::new(cfg());
        for _ in 0..5 {
            t.offline();
            s.step(&c, &key(), s.next_attempt_unix());
        }
        let now = s.next_attempt_unix();
        registered(&t);
        t.answer(503, json!({"error": "busy"}));
        s.step(&c, &key(), now);
        s.step(&c, &key(), now);
        assert_eq!(
            s.next_attempt_unix(),
            now + 5,
            "first failure after a success waits the minimum"
        );
    }
}
