//! The messaging facade the daemon drives. It owns the heard list, mailbox, delivery tracker,
//! group book, duplicate filter and beacon scheduler, and turns on-air events into decisions.
//! It never touches the radio: every method takes `now` and returns what the daemon should do.
//!
//! Typical flow:
//! - Outgoing: encode the session message into chunks, call [`Messenger::send`]. Transmit now if
//!   `plan.transmit` is true; recipients in `plan.held` are in the mailbox for later.
//! - Any received frame: [`Messenger::on_heard`] (also learns SNR and path). It returns the held
//!   messages to release if the station just came on air.
//! - Received chat: [`Messenger::on_chat`] filters duplicates and groups and says what to show
//!   and which receipt to send.
//! - Receipt received: [`Messenger::on_receipt`].
//! - Every few seconds: [`Messenger::tick`] returns expiries, resends, beacons and ID prompts.
use super::beacon::{Due, Scheduler, Settings as BeaconSettings};
use super::groups::{is_group_addr, Control, Dedup, GroupBook, CTL_PREFIX};
use super::heard::{HeardList, Observation, DEFAULT_CAP};
use super::mailbox::{Deposit, Held, Mailbox};
use super::receipts::{Status, Tracker};
use super::traffic::Class;
use super::{message_id, short_id};
use crate::extras::{encode_receipt, Receipt};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// A station heard within this window counts as on air for direct sending.
    pub on_air_window_s: u64,
    /// Silent for longer than this, then heard again: it "came on air" and its mailbox is released.
    pub returning_after_s: u64,
    /// Sent with no receipt for this long: resend (receipts are requested only for Priority and up).
    pub receipt_timeout_s: u64,
    /// Keep finished delivery tracks this long.
    pub track_keep_s: u64,
    /// Duplicate-filter memory.
    pub dedup_ttl_s: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            on_air_window_s: 1800,
            returning_after_s: 900,
            receipt_timeout_s: 600,
            track_keep_s: 7 * 86_400,
            dedup_ttl_s: 3600,
        }
    }
}

/// Result of [`Messenger::send`].
#[derive(Clone, Debug, PartialEq)]
pub struct SendPlan {
    pub id: u64,
    pub class: Class,
    /// Put the chunks on air (outbox) now: a recipient is on air, the message is a group
    /// broadcast, or the class floods.
    pub transmit: bool,
    /// Recipients stored in the mailbox until they come on air.
    pub held: Vec<String>,
    /// Recipients that were on air when sent.
    pub direct: Vec<String>,
    /// Mailbox refusals as (callsign, reason).
    pub rejected: Vec<(String, String)>,
}

/// A held message to send because its station was just heard.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub id: u64,
    pub to: String,
    pub from: String,
    pub class: Class,
    pub chunks: Vec<Vec<u8>>,
}

/// What to do with a received chat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Incoming {
    pub id: u64,
    /// Show in the conversation (addressed to us or to a group we joined, first copy).
    pub show: bool,
    pub duplicate: bool,
    /// Group the message belongs to, if any.
    pub group: Option<String>,
    /// Receipt bytes to send back to the sender (direct messages only).
    pub receipt: Option<Vec<u8>>,
    /// Plain text to send back as a chat to `reply_to` (a group roster after a join).
    pub control_reply: Option<(String, String)>,
}

/// Things the daemon should act on after [`Messenger::tick`].
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// A held message ran out of time; tell the sender.
    Expired {
        id: u64,
        to: String,
        from: String,
    },
    /// Sent but never acknowledged: send again.
    Resend {
        id: u64,
    },
    Beacon(Due),
    /// A callsign ID is due.
    Identify,
}

pub struct Messenger {
    me: String,
    pub settings: Settings,
    heard: HeardList,
    mailbox: Mailbox,
    tracker: Tracker,
    groups: GroupBook,
    dedup: Dedup,
    beacon: Scheduler,
    /// Sender class per sent id, for resend and release decisions.
    classes: Vec<(u64, Class)>,
}

impl Messenger {
    pub fn new(my_call: &str) -> Self {
        let settings = Settings::default();
        Self {
            me: my_call.to_ascii_uppercase(),
            heard: HeardList::new(DEFAULT_CAP),
            mailbox: Mailbox::new(),
            tracker: Tracker::new(),
            groups: GroupBook::new(my_call),
            dedup: Dedup::new(512, settings.dedup_ttl_s),
            beacon: Scheduler::new(BeaconSettings::default()),
            classes: Vec::new(),
            settings,
        }
    }

    /// Restores persisted state; a missing or damaged blob starts empty.
    pub fn restore(my_call: &str, heard_text: Option<&str>, mailbox_text: Option<&str>) -> Self {
        let mut m = Self::new(my_call);
        if let Some(h) = heard_text.and_then(|t| HeardList::from_text(t, DEFAULT_CAP)) {
            m.heard = h;
        }
        if let Some(b) = mailbox_text.and_then(Mailbox::from_text) {
            m.mailbox = b;
        }
        m
    }

    pub fn heard(&self) -> &HeardList {
        &self.heard
    }
    pub fn heard_mut(&mut self) -> &mut HeardList {
        &mut self.heard
    }
    pub fn mailbox(&self) -> &Mailbox {
        &self.mailbox
    }
    pub fn tracker(&self) -> &Tracker {
        &self.tracker
    }
    pub fn groups(&self) -> &GroupBook {
        &self.groups
    }
    pub fn groups_mut(&mut self) -> &mut GroupBook {
        &mut self.groups
    }
    pub fn beacon_mut(&mut self) -> &mut Scheduler {
        &mut self.beacon
    }

    /// Identity of a message as both ends compute it, from the chunks as they go on air.
    pub fn wire_id(from: &str, to: &str, chunks: &[Vec<u8>]) -> u64 {
        message_id(
            &from.to_ascii_uppercase(),
            &to.to_ascii_uppercase(),
            0,
            &chunks.concat(),
        )
    }

    /// Plans one outgoing message (already encoded into session chunks).
    pub fn send(&mut self, now: u64, to: &str, class: Class, chunks: Vec<Vec<u8>>) -> SendPlan {
        let to = to.to_ascii_uppercase();
        let id = Self::wire_id(&self.me, &to, &chunks);
        let group = is_group_addr(&to);
        let recipients = if group {
            self.groups.fanout(&to, &self.me)
        } else {
            vec![to.clone()]
        };
        self.tracker.track(id, &recipients, now);
        self.classes.retain(|(i, _)| *i != id);
        self.classes.push((id, class));

        let (mut direct, mut held, mut rejected) = (Vec::new(), Vec::new(), Vec::new());
        for r in recipients {
            if self.heard.on_air(&r, now, self.settings.on_air_window_s) {
                direct.push(r);
            } else {
                match self
                    .mailbox
                    .deposit(id, &self.me, &r, class.outbox_priority(), chunks.clone(), now)
                {
                    Deposit::Stored | Deposit::Duplicate => {
                        self.tracker.advance(id, Some(&r), Status::Held, now);
                        held.push(r);
                    }
                    Deposit::Rejected(why) => {
                        self.tracker.advance(id, Some(&r), Status::Failed, now);
                        rejected.push((r, why));
                    }
                }
            }
        }
        let transmit = group || !direct.is_empty() || class.flood();
        SendPlan {
            id,
            class,
            transmit,
            held,
            direct,
            rejected,
        }
    }

    /// The daemon reports the message is on air (link ACKed it or it went out).
    pub fn mark_sent(&mut self, now: u64, id: u64, to: Option<&str>) {
        self.tracker.advance(id, to, Status::Sent, now);
    }

    /// Any frame heard from `call`. Returns held messages to release when the station just
    /// came on air (or is new), best class first. Released items stay held until `acked`.
    pub fn on_heard(&mut self, now: u64, call: &str, obs: Observation) -> Vec<Release> {
        if call.eq_ignore_ascii_case(&self.me) {
            return Vec::new();
        }
        let returning = self.heard.hear(call, now, obs, self.settings.returning_after_s);
        if !returning {
            return Vec::new();
        }
        self.mailbox
            .release(call)
            .into_iter()
            .map(|h| Release {
                class: self.class_of(&h),
                id: h.id,
                to: h.to,
                from: h.from,
                chunks: h.chunks,
            })
            .collect()
    }

    fn class_of(&self, h: &Held) -> Class {
        self.classes.iter().find(|(i, _)| *i == h.id).map_or_else(
            || match h.priority {
                crate::outbox::Priority::Urgent => Class::Emergency,
                crate::outbox::Priority::Normal => Class::Routine,
                crate::outbox::Priority::Low => Class::Bulk,
            },
            |(_, c)| *c,
        )
    }

    /// A released message reached its station (link ACK): drop it from the mailbox.
    pub fn acked(&mut self, now: u64, id: u64, to: &str) {
        self.mailbox.ack(id, to);
        self.tracker.advance(id, Some(to), Status::Sent, now);
    }

    /// Position or status learned from a beacon payload.
    pub fn on_position(&mut self, call: &str, lat: f64, lon: f64) {
        self.heard.set_position(call, lat, lon);
    }

    /// A location beacon payload (position or grid) arrived. Updates the heard list and returns
    /// `(callsign, grid)` so the daemon can fill the contact's locator in the address book.
    pub fn on_beacon(&mut self, payload: &[u8]) -> Option<(String, String)> {
        let (call, grid, pos) = super::grid::learn(payload)?;
        if call.eq_ignore_ascii_case(&self.me) {
            return None;
        }
        match pos {
            Some((la, lo)) => self.heard.set_position(&call, la, lo),
            None => self.heard.set_grid(&call, &grid),
        }
        Some((call, grid))
    }

    pub fn on_status(&mut self, call: &str, status: crate::payload_presence::Status) {
        self.heard.set_status(call, status);
    }

    /// A chat arrived (`chunks` are the session chunks it was decoded from, for its id).
    pub fn on_chat(&mut self, now: u64, from: &str, to: &str, text: &str, chunks: &[Vec<u8>]) -> Incoming {
        let id = Self::wire_id(from, to, chunks);
        let to_u = to.to_ascii_uppercase();
        let group = is_group_addr(&to_u).then(|| to_u.clone());
        let first = self.dedup.first_time(id, now);
        let mut control_reply = None;
        let mut is_ctl = false;
        if text.starts_with(CTL_PREFIX) {
            is_ctl = true;
            if first {
                if let Some(c) = Control::decode(text) {
                    if let Some(reply) = self.groups.apply(&c, from) {
                        if let Control::Roster { group, .. } = &reply {
                            control_reply = Some((group.clone(), reply.encode()));
                        }
                    }
                }
            }
        }
        let mine = self.groups.for_me(&to_u);
        let show = first && mine && !is_ctl && !from.eq_ignore_ascii_case(&self.me);
        let direct = to_u == self.me;
        let receipt = (first && direct && !is_ctl).then(|| {
            encode_receipt(&Receipt::Delivered {
                message_id: short_id(id),
            })
        });
        Incoming {
            id,
            show,
            duplicate: !first,
            group,
            receipt,
            control_reply,
        }
    }

    /// A delivery or read receipt arrived from `from`.
    pub fn on_receipt(&mut self, now: u64, from: &str, bytes: &[u8]) -> Option<u64> {
        let r = crate::extras::decode_receipt(bytes)?;
        let id = self.tracker.on_receipt(&r, from, now)?;
        self.mailbox.ack(id, from);
        Some(id)
    }

    /// Periodic housekeeping. `pos` is the station's position for beacon decisions.
    pub fn tick(&mut self, now: u64, pos: Option<(f64, f64)>) -> Vec<Action> {
        let mut out = Vec::new();
        for h in self.mailbox.expire(now) {
            self.tracker.advance(h.id, Some(&h.to), Status::Expired, now);
            out.push(Action::Expired {
                id: h.id,
                to: h.to,
                from: h.from,
            });
        }
        for id in self.tracker.overdue(now, self.settings.receipt_timeout_s) {
            let wants = self
                .classes
                .iter()
                .find(|(i, _)| *i == id)
                .is_some_and(|(_, c)| c.receipt_required());
            if wants {
                out.push(Action::Resend { id });
            }
        }
        self.tracker.prune(now, self.settings.track_keep_s);
        let live: Vec<u64> = self.tracker.all().iter().map(|t| t.id).collect();
        self.classes.retain(|(i, _)| live.contains(i));
        if let Some(d) = self.beacon.due(now, pos) {
            out.push(Action::Beacon(d));
        }
        if self.beacon.id_due(now) {
            out.push(Action::Identify);
        }
        out
    }

    /// Text blobs to persist: (heard list, mailbox).
    pub fn snapshot(&self) -> (String, String) {
        (self.heard.to_text(), self.mailbox.to_text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs() -> Observation {
        Observation {
            snr_db: 6.0,
            channel_khz: 18111.0,
            via: None,
        }
    }
    fn chunks(tag: u8) -> Vec<Vec<u8>> {
        vec![vec![tag, 1, 2, 3], vec![tag, 9]]
    }

    #[test]
    fn offline_message_is_held_then_released_when_station_appears() {
        let mut a = Messenger::new("A1AA");
        let plan = a.send(100, "b2bb", Class::Routine, chunks(1));
        assert!(!plan.transmit);
        assert_eq!(plan.held, ["B2BB"]);
        assert_eq!(a.tracker().get(plan.id).unwrap().overall(), Status::Held);

        // Someone else being heard releases nothing.
        assert!(a.on_heard(200, "C3CC", obs()).is_empty());
        // B2BB comes on air: released.
        let rel = a.on_heard(300, "B2BB", obs());
        assert_eq!(rel.len(), 1);
        assert_eq!(
            (rel[0].id, rel[0].to.as_str(), rel[0].chunks.clone()),
            (plan.id, "B2BB", chunks(1))
        );
        // A second frame from the same station does not release again.
        assert!(a.on_heard(310, "B2BB", obs()).is_empty());
        // Until acked it stays held, and survives a restart.
        let (h, m) = a.snapshot();
        let mut a2 = Messenger::restore("A1AA", Some(&h), Some(&m));
        assert_eq!(a2.mailbox().len(), 1);
        a2.acked(400, plan.id, "B2BB");
        assert!(a2.mailbox().is_empty());
    }

    #[test]
    fn direct_send_receipt_round_trip() {
        let mut a = Messenger::new("A1AA");
        let mut b = Messenger::new("B2BB");
        a.on_heard(0, "B2BB", obs());
        let ch = chunks(7);
        let plan = a.send(10, "B2BB", Class::Priority, ch.clone());
        assert!(plan.transmit && plan.direct == ["B2BB"] && plan.held.is_empty());
        a.mark_sent(11, plan.id, None);
        let inc = b.on_chat(12, "A1AA", "B2BB", "hello", &ch);
        assert!(inc.show && !inc.duplicate);
        assert_eq!(inc.id, plan.id);
        let receipt = inc.receipt.unwrap();
        // Same frames arriving again via a relay are not shown or acknowledged twice.
        let dup = b.on_chat(13, "A1AA", "B2BB", "hello", &ch);
        assert!(!dup.show && dup.duplicate && dup.receipt.is_none());
        assert_eq!(a.on_receipt(20, "B2BB", &receipt), Some(plan.id));
        assert_eq!(a.tracker().get(plan.id).unwrap().overall(), Status::Delivered);
    }

    #[test]
    fn unacknowledged_priority_resends_but_routine_does_not() {
        let mut a = Messenger::new("A1AA");
        a.on_heard(0, "B2BB", obs());
        let p = a.send(1, "B2BB", Class::Priority, chunks(1));
        let r = a.send(1, "B2BB", Class::Routine, chunks(2));
        a.mark_sent(2, p.id, None);
        a.mark_sent(2, r.id, None);
        let acts = a.tick(2 + 601, None);
        assert_eq!(acts, vec![Action::Resend { id: p.id }]);
    }

    #[test]
    fn emergency_floods_even_when_nobody_is_on_air() {
        let mut a = Messenger::new("A1AA");
        let p = a.send(1, "B2BB", Class::Emergency, chunks(3));
        assert!(p.transmit && p.held == ["B2BB"]);
        assert_eq!(
            a.mailbox().for_station("B2BB")[0].priority,
            crate::outbox::Priority::Urgent
        );
    }

    #[test]
    fn group_chat_fanout_and_membership() {
        let mut a = Messenger::new("A1AA");
        let mut b = Messenger::new("B2BB");
        a.groups_mut().join("net").unwrap();
        let join = b.groups_mut().join("net").unwrap();
        // A hears B's join control chat and answers with a roster.
        let jc = chunks(4);
        let inc = a.on_chat(5, "B2BB", "#NET", &join.encode(), &jc);
        assert!(!inc.show, "control lines are not shown");
        let (grp, roster) = inc.control_reply.unwrap();
        assert_eq!(grp, "#NET");
        let inc_b = b.on_chat(6, "A1AA", "#NET", &roster, &chunks(5));
        assert!(!inc_b.show);
        assert_eq!(b.groups().members("#NET"), ["A1AA", "B2BB"]);

        // A posts to the group while B is off air: broadcast plus a mailbox copy for B.
        let plan = a.send(10, "#NET", Class::Routine, chunks(6));
        assert!(plan.transmit && plan.held == ["B2BB"]);
        // B hears it shown once; a repeat is a duplicate.
        let first = b.on_chat(11, "A1AA", "#NET", "hi all", &chunks(6));
        assert!(first.show && first.group.as_deref() == Some("#NET") && first.receipt.is_none());
        assert!(!b.on_chat(12, "A1AA", "#NET", "hi all", &chunks(6)).show);
        // A chat for a group B has not joined is ignored.
        assert!(!b.on_chat(13, "A1AA", "#OTHER", "x", &chunks(8)).show);
    }

    #[test]
    fn expiry_is_reported_and_tracked() {
        let mut a = Messenger::new("A1AA");
        let p = a.send(0, "B2BB", Class::Bulk, chunks(1));
        let acts = a.tick(super::super::mailbox::TTL_LOW_S + 1, None);
        assert_eq!(
            acts,
            vec![Action::Expired {
                id: p.id,
                to: "B2BB".into(),
                from: "A1AA".into()
            }]
        );
        assert_eq!(a.tracker().get(p.id).unwrap().overall(), Status::Failed);
    }

    #[test]
    fn beacons_teach_grid_to_heard_list() {
        let mut a = Messenger::new("A1AA");
        a.on_heard(0, "B2BB", obs());
        a.on_heard(0, "C3CC", obs());
        let gps = crate::msg::grid::beacon_payload("B2BB", Some((51.5, -0.12)), None).unwrap();
        assert_eq!(a.on_beacon(&gps), Some(("B2BB".into(), "IO91WM".into())));
        let manual = crate::msg::grid::beacon_payload("C3CC", None, Some("jn58")).unwrap();
        assert_eq!(a.on_beacon(&manual), Some(("C3CC".into(), "JN58".into())));
        assert!(
            a.heard().get("C3CC").unwrap().position.is_some(),
            "route planner gets a position"
        );
        assert_eq!(a.heard().get("B2BB").unwrap().grid.as_deref(), Some("IO91WM"));
        let own = crate::msg::grid::beacon_payload("A1AA", None, Some("IO91")).unwrap();
        assert_eq!(a.on_beacon(&own), None);
    }

    #[test]
    fn beacons_and_id_surface_in_tick() {
        let mut a = Messenger::new("A1AA");
        a.beacon_mut().set_enabled(true);
        let acts = a.tick(0, Some((51.0, 0.0)));
        assert!(acts.contains(&Action::Beacon(Due::Status(
            crate::payload_presence::Status::Listening
        ))));
        a.beacon_mut().note_tx(1);
        assert!(a.tick(2, None).contains(&Action::Identify));
    }
}
