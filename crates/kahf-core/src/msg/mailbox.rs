//! Offline mailbox: holds messages for stations that are not on air and releases them when the
//! station is heard again. Bounded per addressee and in total; emergency traffic is kept longest
//! and is never evicted by lower classes.
use super::{esc, hex, unesc, unhex};
use crate::outbox::Priority;

const MAGIC: &str = "KMB1";

pub const TTL_EMERGENCY_S: u64 = 7 * 86_400;
pub const TTL_NORMAL_S: u64 = 3 * 86_400;
pub const TTL_LOW_S: u64 = 86_400;
/// Most items held for one addressee.
pub const MAX_PER_STATION: usize = 50;
/// Most bytes held in total (session chunks only).
pub const MAX_TOTAL_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Held {
    pub id: u64,
    pub from: String,
    pub to: String,
    pub priority: Priority,
    pub deposited: u64,
    pub expires: u64,
    /// Session chunks exactly as they go on air (signature and private bit intact).
    pub chunks: Vec<Vec<u8>>,
    /// Release attempts so far (a release that was never acknowledged).
    pub attempts: u32,
}

impl Held {
    pub fn bytes(&self) -> usize {
        self.chunks.iter().map(Vec::len).sum()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Deposit {
    Stored,
    /// Already held (same id); nothing changed.
    Duplicate,
    /// Not stored, with the reason.
    Rejected(String),
}

#[derive(Clone, Debug, Default)]
pub struct Mailbox {
    items: Vec<Held>,
}

pub fn ttl_for(p: Priority) -> u64 {
    match p {
        Priority::Urgent => TTL_EMERGENCY_S,
        Priority::Normal => TTL_NORMAL_S,
        Priority::Low => TTL_LOW_S,
    }
}

fn pri_name(p: Priority) -> &'static str {
    match p {
        Priority::Urgent => "u",
        Priority::Normal => "n",
        Priority::Low => "l",
    }
}

impl Mailbox {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn total_bytes(&self) -> usize {
        self.items.iter().map(Held::bytes).sum()
    }

    /// True when message `id` is already held for `to`.
    pub fn contains(&self, id: u64, to: &str) -> bool {
        let to = to.to_ascii_uppercase();
        self.items.iter().any(|i| i.id == id && i.to == to)
    }

    /// Stores a message for `to`. When full, the oldest lowest-class item for that addressee
    /// (or overall, for the byte cap) is dropped to make room, but never one of a higher class.
    pub fn deposit(
        &mut self,
        id: u64,
        from: &str,
        to: &str,
        priority: Priority,
        chunks: Vec<Vec<u8>>,
        now: u64,
    ) -> Deposit {
        if self.contains(id, to) {
            return Deposit::Duplicate;
        }
        let item = Held {
            id,
            from: from.to_ascii_uppercase(),
            to: to.to_ascii_uppercase(),
            priority,
            deposited: now,
            expires: now.saturating_add(ttl_for(priority)),
            chunks,
            attempts: 0,
        };
        if item.bytes() > MAX_TOTAL_BYTES {
            return Deposit::Rejected("message larger than the whole mailbox".into());
        }
        while self.items.iter().filter(|i| i.to == item.to).count() >= MAX_PER_STATION {
            if !self.evict(|i| i.to == item.to, priority) {
                return Deposit::Rejected(format!("mailbox for {} is full", item.to));
            }
        }
        while self.total_bytes() + item.bytes() > MAX_TOTAL_BYTES {
            if !self.evict(|_| true, priority) {
                return Deposit::Rejected("mailbox is full".into());
            }
        }
        self.items.push(item);
        Deposit::Stored
    }

    /// Drops the oldest item matching `scope` whose class is not higher than `incoming`.
    fn evict(&mut self, scope: impl Fn(&Held) -> bool, incoming: Priority) -> bool {
        let victim = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| scope(i) && i.priority >= incoming)
            .max_by_key(|(_, i)| (i.priority, std::cmp::Reverse(i.deposited)))
            .map(|(n, _)| n);
        match victim {
            Some(n) => {
                self.items.remove(n);
                true
            }
            None => false,
        }
    }

    /// Items for `to` in release order: emergency first, then oldest first.
    pub fn for_station(&self, to: &str) -> Vec<&Held> {
        let to = to.to_ascii_uppercase();
        let mut v: Vec<&Held> = self.items.iter().filter(|i| i.to == to).collect();
        v.sort_by_key(|i| (i.priority, i.deposited, i.id));
        v
    }

    /// Items to release now for a station that just came on air; counts one attempt each. They
    /// stay held until `ack`, so a failed release is simply repeated next time.
    pub fn release(&mut self, to: &str) -> Vec<Held> {
        let to = to.to_ascii_uppercase();
        for i in self.items.iter_mut().filter(|i| i.to == to) {
            i.attempts += 1;
        }
        self.for_station(&to).into_iter().cloned().collect()
    }

    pub fn get(&self, id: u64, to: &str) -> Option<&Held> {
        let to = to.to_ascii_uppercase();
        self.items.iter().find(|i| i.id == id && i.to == to)
    }

    /// Removes an acknowledged (or cancelled) item.
    pub fn ack(&mut self, id: u64, to: &str) -> bool {
        let to = to.to_ascii_uppercase();
        let n = self.items.len();
        self.items.retain(|i| !(i.id == id && i.to == to));
        self.items.len() != n
    }

    /// Drops expired items and returns them so the sender can be told.
    pub fn expire(&mut self, now: u64) -> Vec<Held> {
        let (gone, keep): (Vec<Held>, Vec<Held>) = std::mem::take(&mut self.items)
            .into_iter()
            .partition(|i| i.expires <= now);
        self.items = keep;
        gone
    }

    /// Addressees that have something waiting, with their counts.
    pub fn waiting(&self) -> Vec<(String, usize)> {
        let mut m: Vec<(String, usize)> = Vec::new();
        for i in &self.items {
            match m.iter_mut().find(|(c, _)| *c == i.to) {
                Some((_, n)) => *n += 1,
                None => m.push((i.to.clone(), 1)),
            }
        }
        m.sort();
        m
    }

    pub fn to_text(&self) -> String {
        let mut s = format!("{MAGIC}\n");
        for i in &self.items {
            let chunks: Vec<String> = i.chunks.iter().map(|c| hex(c)).collect();
            s.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                i.id,
                esc(&i.from),
                esc(&i.to),
                pri_name(i.priority),
                i.deposited,
                i.expires,
                i.attempts,
                chunks.join(","),
            ));
        }
        s
    }

    pub fn from_text(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        if lines.next()? != MAGIC {
            return None;
        }
        let mut items = Vec::new();
        for line in lines.filter(|l| !l.is_empty()) {
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() != 8 {
                return None;
            }
            let priority = match f[3] {
                "u" => Priority::Urgent,
                "n" => Priority::Normal,
                "l" => Priority::Low,
                _ => return None,
            };
            let chunks = if f[7].is_empty() {
                Vec::new()
            } else {
                f[7].split(',').map(unhex).collect::<Option<Vec<_>>>()?
            };
            items.push(Held {
                id: f[0].parse().ok()?,
                from: unesc(f[1]),
                to: unesc(f[2]),
                priority,
                deposited: f[4].parse().ok()?,
                expires: f[5].parse().ok()?,
                attempts: f[6].parse().ok()?,
                chunks,
            });
        }
        Some(Self { items })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dep(m: &mut Mailbox, id: u64, to: &str, p: Priority, now: u64) -> Deposit {
        m.deposit(id, "A1", to, p, vec![vec![1, 2, 3], vec![4]], now)
    }

    #[test]
    fn deposit_dedup_release_ack() {
        let mut m = Mailbox::new();
        assert_eq!(dep(&mut m, 1, "b1", Priority::Normal, 10), Deposit::Stored);
        assert_eq!(dep(&mut m, 1, "B1", Priority::Normal, 11), Deposit::Duplicate);
        assert_eq!(
            dep(&mut m, 1, "C1", Priority::Normal, 11),
            Deposit::Stored,
            "same id, other station"
        );
        assert!(m.ack(1, "C1"));
        dep(&mut m, 2, "B1", Priority::Urgent, 20);
        dep(&mut m, 3, "C1", Priority::Normal, 5);
        assert_eq!(m.waiting(), vec![("B1".into(), 2), ("C1".into(), 1)]);
        let ids: Vec<u64> = m.release("B1").iter().map(|h| h.id).collect();
        assert_eq!(ids, vec![2, 1], "urgent first");
        assert_eq!(m.get(1, "B1").unwrap().attempts, 1);
        assert!(m.ack(2, "B1") && !m.ack(2, "B1"));
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn expiry_by_class() {
        let mut m = Mailbox::new();
        dep(&mut m, 1, "B1", Priority::Low, 0);
        dep(&mut m, 2, "B1", Priority::Normal, 0);
        dep(&mut m, 3, "B1", Priority::Urgent, 0);
        let gone: Vec<u64> = m.expire(TTL_LOW_S + 1).iter().map(|i| i.id).collect();
        assert_eq!(gone, [1]);
        let gone: Vec<u64> = m.expire(TTL_NORMAL_S + 1).iter().map(|i| i.id).collect();
        assert_eq!(gone, [2]);
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn per_station_cap_keeps_urgent() {
        let mut m = Mailbox::new();
        for i in 0..MAX_PER_STATION as u64 {
            let p = if i == 0 {
                Priority::Urgent
            } else {
                Priority::Normal
            };
            dep(&mut m, i, "B1", p, i);
        }
        // A new normal message evicts the oldest normal one, not the urgent one.
        assert_eq!(dep(&mut m, 1000, "B1", Priority::Normal, 999), Deposit::Stored);
        assert!(m.contains(0, "B1") && !m.contains(1, "B1") && m.contains(1000, "B1"));
        assert_eq!(m.for_station("B1").len(), MAX_PER_STATION);
        // A full box of urgent traffic rejects a normal message.
        let mut m = Mailbox::new();
        for i in 0..MAX_PER_STATION as u64 {
            dep(&mut m, i, "B1", Priority::Urgent, i);
        }
        assert!(matches!(
            dep(&mut m, 99, "B1", Priority::Normal, 99),
            Deposit::Rejected(_)
        ));
        assert_eq!(dep(&mut m, 100, "B1", Priority::Urgent, 100), Deposit::Stored);
    }

    #[test]
    fn save_round_trip() {
        let mut m = Mailbox::new();
        dep(&mut m, 7, "B1", Priority::Low, 3);
        m.deposit(8, "A1", "C1", Priority::Normal, vec![], 4);
        m.release("B1");
        let back = Mailbox::from_text(&m.to_text()).unwrap();
        assert_eq!(back.to_text(), m.to_text());
        assert_eq!(back.get(7, "B1").unwrap().chunks, vec![vec![1, 2, 3], vec![4]]);
        assert!(Mailbox::from_text("KMB1\nbad line\n").is_none());
    }
}
