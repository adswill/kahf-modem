//! Group chat. A group is addressed as `#NAME` in the `to` field of a session chat message, so no
//! new on-air format is needed: any station hears the message, and the ones that have joined
//! show it. Membership is advertised with plain-text control lines that ride in chat bodies.
//! Duplicates (the same message arriving direct and via a relay) are suppressed by id.
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub const MAX_GROUP_NAME: usize = 16;
/// Control-line prefix. A chat body starting with this is a group control message, not text.
pub const CTL_PREFIX: &str = "KAHFGRP1 ";

/// True when `to` addresses a group (`#NAME`).
pub fn is_group_addr(to: &str) -> bool {
    to.strip_prefix('#').is_some_and(valid_name)
}

fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= MAX_GROUP_NAME
        && n.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-')
}

/// Normalises user input ("net 17" is rejected, "net17" becomes "#NET17").
pub fn group_addr(name: &str) -> Result<String, String> {
    let n = name.trim().trim_start_matches('#').to_ascii_uppercase();
    if valid_name(&n) {
        Ok(format!("#{n}"))
    } else {
        Err(format!(
            "a group name is 1 to {MAX_GROUP_NAME} letters, digits or '-'"
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Control {
    Join {
        group: String,
        call: String,
    },
    Leave {
        group: String,
        call: String,
    },
    /// A member's full roster view, so a joiner learns who is in.
    Roster {
        group: String,
        members: Vec<String>,
    },
}

impl Control {
    pub fn encode(&self) -> String {
        match self {
            Control::Join { group, call } => format!("{CTL_PREFIX}JOIN {group} {call}"),
            Control::Leave { group, call } => format!("{CTL_PREFIX}LEAVE {group} {call}"),
            Control::Roster { group, members } => format!("{CTL_PREFIX}ROSTER {group} {}", members.join(",")),
        }
    }

    pub fn decode(body: &str) -> Option<Control> {
        let rest = body.strip_prefix(CTL_PREFIX)?;
        let mut it = rest.split(' ');
        let verb = it.next()?;
        let group = it.next()?.to_string();
        if !is_group_addr(&group) {
            return None;
        }
        let arg = it.next()?;
        if it.next().is_some() {
            return None;
        }
        match verb {
            "JOIN" => Some(Control::Join {
                group,
                call: arg.to_string(),
            }),
            "LEAVE" => Some(Control::Leave {
                group,
                call: arg.to_string(),
            }),
            "ROSTER" => Some(Control::Roster {
                group,
                members: arg
                    .split(',')
                    .filter(|c| !c.is_empty())
                    .map(String::from)
                    .collect(),
            }),
            _ => None,
        }
    }
}

/// Groups this station knows about, with the members it has learned.
#[derive(Clone, Debug, Default)]
pub struct GroupBook {
    me: String,
    /// Groups this station has joined.
    joined: BTreeSet<String>,
    /// Known members per group (includes us once joined).
    members: BTreeMap<String, BTreeSet<String>>,
}

impl GroupBook {
    pub fn new(my_call: &str) -> Self {
        Self {
            me: my_call.to_ascii_uppercase(),
            ..Default::default()
        }
    }

    pub fn join(&mut self, group: &str) -> Result<Control, String> {
        let g = group_addr(group)?;
        self.joined.insert(g.clone());
        self.members.entry(g.clone()).or_default().insert(self.me.clone());
        Ok(Control::Join {
            group: g,
            call: self.me.clone(),
        })
    }

    pub fn leave(&mut self, group: &str) -> Result<Control, String> {
        let g = group_addr(group)?;
        self.joined.remove(&g);
        if let Some(m) = self.members.get_mut(&g) {
            m.remove(&self.me);
        }
        Ok(Control::Leave {
            group: g,
            call: self.me.clone(),
        })
    }

    pub fn is_joined(&self, group: &str) -> bool {
        self.joined.contains(&group.to_ascii_uppercase())
    }

    pub fn joined(&self) -> Vec<&str> {
        self.joined.iter().map(String::as_str).collect()
    }

    pub fn members(&self, group: &str) -> Vec<&str> {
        self.members
            .get(&group.to_ascii_uppercase())
            .map(|m| m.iter().map(String::as_str).collect())
            .unwrap_or_default()
    }

    /// Roster line this station can send in reply to a join, or None if not joined.
    pub fn roster(&self, group: &str) -> Option<Control> {
        let g = group.to_ascii_uppercase();
        self.joined.contains(&g).then(|| Control::Roster {
            members: self.members(&g).into_iter().map(String::from).collect(),
            group: g,
        })
    }

    /// Applies a control line heard from `from` (must match the callsign it names, for join
    /// and leave, so nobody can enrol someone else). Returns a roster to send back when a new
    /// member joined a group we are in.
    pub fn apply(&mut self, c: &Control, from: &str) -> Option<Control> {
        let from = from.to_ascii_uppercase();
        match c {
            Control::Join { group, call } if *call == from => {
                let new = self.members.entry(group.clone()).or_default().insert(from);
                if new && self.joined.contains(group) {
                    return self.roster(group);
                }
                None
            }
            Control::Leave { group, call } if *call == from => {
                if let Some(m) = self.members.get_mut(group) {
                    m.remove(&from);
                }
                None
            }
            Control::Roster { group, members }
                if self.joined.contains(group) || self.members.contains_key(group) =>
            {
                let m = self.members.entry(group.clone()).or_default();
                m.extend(members.iter().map(|c| c.to_ascii_uppercase()));
                None
            }
            _ => None,
        }
    }

    /// Stations a group message must reach (every known member except the sender), for
    /// mailbox fan-out to members that are off air.
    pub fn fanout(&self, group: &str, sender: &str) -> Vec<String> {
        let sender = sender.to_ascii_uppercase();
        self.members(group)
            .into_iter()
            .filter(|c| *c != sender)
            .map(String::from)
            .collect()
    }

    /// Should a chat addressed to `to` be shown to this station?
    pub fn for_me(&self, to: &str) -> bool {
        let to = to.to_ascii_uppercase();
        to == self.me || self.joined.contains(&to)
    }
}

/// Remembers recent message ids so a copy arriving by two paths shows once. An id is
/// forgotten after `ttl_s`, so the same short text sent again later (for example "OK") is shown.
#[derive(Clone, Debug)]
pub struct Dedup {
    cap: usize,
    ttl_s: u64,
    seen: VecDeque<(u64, u64)>,
}

impl Dedup {
    pub fn new(cap: usize, ttl_s: u64) -> Self {
        Self {
            cap: cap.max(1),
            ttl_s,
            seen: VecDeque::new(),
        }
    }

    /// True the first time `id` is seen within the window, false for every repeat.
    pub fn first_time(&mut self, id: u64, now: u64) -> bool {
        while self
            .seen
            .front()
            .is_some_and(|&(_, at)| now.saturating_sub(at) > self.ttl_s)
        {
            self.seen.pop_front();
        }
        if self.seen.iter().any(|&(i, _)| i == id) {
            return false;
        }
        if self.seen.len() >= self.cap {
            self.seen.pop_front();
        }
        self.seen.push_back((id, now));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        assert!(
            is_group_addr("#NET17")
                && !is_group_addr("G4ABC")
                && !is_group_addr("#")
                && !is_group_addr("#net")
        );
        assert_eq!(group_addr("net-17").unwrap(), "#NET-17");
        assert!(group_addr("bad name").is_err());
        assert!(group_addr(&"A".repeat(17)).is_err());
    }

    #[test]
    fn control_round_trip() {
        for c in [
            Control::Join {
                group: "#X1".into(),
                call: "G4ABC".into(),
            },
            Control::Leave {
                group: "#X1".into(),
                call: "G4ABC".into(),
            },
            Control::Roster {
                group: "#X1".into(),
                members: vec!["A1".into(), "B2".into()],
            },
        ] {
            assert_eq!(Control::decode(&c.encode()), Some(c));
        }
        assert_eq!(Control::decode("hello"), None);
        assert_eq!(Control::decode("KAHFGRP1 JOIN notagroup A1"), None);
        assert_eq!(Control::decode("KAHFGRP1 JOIN #X1 A1 extra"), None);
    }

    #[test]
    fn membership_flow() {
        let mut a = GroupBook::new("A1");
        let mut b = GroupBook::new("B1");
        let join_a = a.join("#net").unwrap();
        let join_b = b.join("net").unwrap();
        // A hears B join and replies with a roster; B learns about A from it.
        let roster = a.apply(&join_b, "B1").expect("roster back");
        assert!(b.apply(&roster, "A1").is_none());
        assert_eq!(a.members("#NET"), ["A1", "B1"]);
        assert_eq!(b.members("#NET"), ["A1", "B1"]);
        // A spoofed join is ignored.
        assert!(a
            .apply(
                &Control::Join {
                    group: "#NET".into(),
                    call: "Z9".into()
                },
                "B1"
            )
            .is_none());
        assert!(!a.members("#NET").contains(&"Z9"));
        assert_eq!(a.fanout("#NET", "A1"), ["B1"]);
        assert!(a.for_me("#net") && a.for_me("a1") && !a.for_me("#OTHER"));
        a.apply(
            &Control::Leave {
                group: "#NET".into(),
                call: "B1".into(),
            },
            "B1",
        );
        assert_eq!(a.members("#NET"), ["A1"]);
        let _ = join_a;
    }

    #[test]
    fn dedup_window() {
        let mut d = Dedup::new(2, 100);
        assert!(d.first_time(1, 0) && !d.first_time(1, 1) && d.first_time(2, 2) && d.first_time(3, 3));
        assert!(d.first_time(1, 4), "1 pushed out by the cap");
        assert!(d.first_time(1, 200), "aged out by time");
    }
}
