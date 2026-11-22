//! Outbox: a queue of outgoing items (message, mail, file id) with priority,
//! retry backoff and a simple length-prefixed byte format for saving to disk.

const MAGIC: &[u8] = b"OBX1";
const BASE_BACKOFF_MS: u64 = 5_000;
const MAX_BACKOFF_MS: u64 = 600_000;
const MAX_ATTEMPTS: u32 = 8;

/// Lower value sorts first: `Urgent` is sent before `Normal`, `Normal` before `Low`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Urgent = 0,
    Normal = 1,
    Low = 2,
}

impl Priority {
    fn from_u8(b: u8) -> Option<Self> {
        match b {
            0 => Some(Priority::Urgent),
            1 => Some(Priority::Normal),
            2 => Some(Priority::Low),
            _ => None,
        }
    }
}

/// One outgoing item: a kind tag (e.g. "message", "mail", "file") and its payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutItem {
    pub kind: String,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    id: u64,
    priority: Priority,
    item: OutItem,
    attempts: u32,
    next_try_ms: u64,
}

/// Queue of outgoing items. Entries are kept in insertion (id) order, so FIFO
/// within a priority level is simply id order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outbox {
    next_id: u64,
    entries: Vec<Entry>,
}

impl Default for Outbox {
    fn default() -> Self {
        Self::new()
    }
}

impl Outbox {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            entries: Vec::new(),
        }
    }

    /// Adds an item that is due immediately and returns its id.
    pub fn push(&mut self, item: OutItem, priority: Priority) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push(Entry {
            id,
            priority,
            item,
            attempts: 0,
            next_try_ms: 0,
        });
        id
    }

    /// Returns the highest-priority item that is due at `now_ms` (FIFO within a
    /// priority), with its id. The item stays queued until `done` or `failed`.
    pub fn next(&mut self, now_ms: u64) -> Option<(u64, OutItem)> {
        self.due(now_ms).first().map(|(id, item)| (*id, (*item).clone()))
    }

    /// Every item due at `now_ms`, in sending order: priority first, then FIFO.
    /// Lets a caller skip items it cannot send yet (for example, ones for another station).
    pub fn due(&self, now_ms: u64) -> Vec<(u64, &OutItem)> {
        let mut due: Vec<&Entry> = self.entries.iter().filter(|e| e.next_try_ms <= now_ms).collect();
        due.sort_by_key(|e| (e.priority, e.id));
        due.into_iter().map(|e| (e.id, &e.item)).collect()
    }

    /// As `next`, but only items for which `want(id, item)` is true.
    pub fn next_matching(
        &mut self,
        now_ms: u64,
        want: impl Fn(u64, &OutItem) -> bool,
    ) -> Option<(u64, OutItem)> {
        self.entries
            .iter()
            .filter(|e| e.next_try_ms <= now_ms && want(e.id, &e.item))
            .min_by_key(|e| (e.priority, e.id))
            .map(|e| (e.id, e.item.clone()))
    }

    /// The queued item with this id.
    pub fn get(&self, id: u64) -> Option<&OutItem> {
        self.entries.iter().find(|e| e.id == id).map(|e| &e.item)
    }

    /// Records a failed send. The item is retried after an exponential wait
    /// (5 s, 10 s, 20 s, ... capped at 10 min). After 8 failed attempts it is dropped.
    pub fn failed(&mut self, id: u64, now_ms: u64) {
        let Some(pos) = self.entries.iter().position(|e| e.id == id) else {
            return;
        };
        let e = &mut self.entries[pos];
        e.attempts += 1;
        if e.attempts >= MAX_ATTEMPTS {
            self.entries.remove(pos);
            return;
        }
        e.next_try_ms = now_ms.saturating_add(backoff_ms(e.attempts));
    }

    /// Removes an item that was sent successfully.
    pub fn done(&mut self, id: u64) {
        self.entries.retain(|e| e.id != id);
    }

    /// Removes an item that is still queued. Returns false if no such item exists.
    pub fn cancel(&mut self, id: u64) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        self.entries.len() != before
    }

    /// Lists (id, priority, attempts) for every queued item, in insertion order.
    pub fn pending(&self) -> Vec<(u64, Priority, u32)> {
        self.entries
            .iter()
            .map(|e| (e.id, e.priority, e.attempts))
            .collect()
    }

    /// Serialises the whole queue (including retry state) for saving to disk.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&self.next_id.to_le_bytes());
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for e in &self.entries {
            out.extend_from_slice(&e.id.to_le_bytes());
            out.push(e.priority as u8);
            out.extend_from_slice(&e.attempts.to_le_bytes());
            out.extend_from_slice(&e.next_try_ms.to_le_bytes());
            put_bytes(&mut out, e.item.kind.as_bytes());
            put_bytes(&mut out, &e.item.payload);
        }
        out
    }

    /// Loads a queue written by `to_bytes`. Returns None for any malformed input.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let mut r = Reader { buf: bytes };
        if r.take(MAGIC.len())? != MAGIC {
            return None;
        }
        let next_id = r.u64()?;
        let count = r.u32()?;
        let mut entries: Vec<Entry> = Vec::new();
        for _ in 0..count {
            let id = r.u64()?;
            let priority = Priority::from_u8(r.u8()?)?;
            let attempts = r.u32()?;
            let next_try_ms = r.u64()?;
            let kind = String::from_utf8(r.bytes()?.to_vec()).ok()?;
            let payload = r.bytes()?.to_vec();
            // Ids must be increasing and below next_id; attempts must be below the give-up limit.
            if id == 0 || id >= next_id || attempts >= MAX_ATTEMPTS {
                return None;
            }
            if entries.last().is_some_and(|last| last.id >= id) {
                return None;
            }
            entries.push(Entry {
                id,
                priority,
                item: OutItem { kind, payload },
                attempts,
                next_try_ms,
            });
        }
        if !r.buf.is_empty() {
            return None;
        }
        Some(Self { next_id, entries })
    }
}

/// Wait after the `attempts`-th failure: 5 s doubled each time, capped at 10 min.
fn backoff_ms(attempts: u32) -> u64 {
    let shift = attempts.saturating_sub(1).min(32);
    BASE_BACKOFF_MS
        .checked_shl(shift)
        .unwrap_or(u64::MAX)
        .min(MAX_BACKOFF_MS)
}

fn put_bytes(out: &mut Vec<u8>, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
}

struct Reader<'a> {
    buf: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.buf.len() < n {
            return None;
        }
        let (head, tail) = self.buf.split_at(n);
        self.buf = tail;
        Some(head)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: &str, payload: &[u8]) -> OutItem {
        OutItem {
            kind: kind.to_string(),
            payload: payload.to_vec(),
        }
    }

    #[test]
    fn due_lists_every_due_item_in_sending_order() {
        let mut q = Outbox::new();
        let a = q.push(item("message", b"a"), Priority::Normal);
        let b = q.push(item("message", b"b"), Priority::Urgent);
        let c = q.push(item("message", b"c"), Priority::Normal);
        q.failed(c, 0);
        let ids: Vec<u64> = q.due(0).iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec![b, a], "waiting item left out, urgent first");
        assert_eq!(q.next(0).map(|(id, _)| id), Some(b));
    }

    #[test]
    fn priority_order_then_fifo() {
        let mut q = Outbox::new();
        let a = q.push(item("mail", b"a"), Priority::Low);
        let b = q.push(item("message", b"b"), Priority::Normal);
        let c = q.push(item("file", b"c"), Priority::Urgent);
        let d = q.push(item("message", b"d"), Priority::Normal);

        let mut order = Vec::new();
        while let Some((id, _)) = q.next(0) {
            order.push(id);
            q.done(id);
        }
        assert_eq!(order, vec![c, b, d, a]);
        assert!(q.pending().is_empty());
    }

    #[test]
    fn not_due_items_are_skipped() {
        let mut q = Outbox::new();
        let a = q.push(item("message", b"a"), Priority::Urgent);
        let b = q.push(item("message", b"b"), Priority::Low);
        q.failed(a, 0);
        // a is waiting, so b (lower priority) is returned.
        assert_eq!(q.next(0).map(|(id, _)| id), Some(b));
        q.done(b);
        assert_eq!(q.next(0), None);
        assert_eq!(q.next(5_000).map(|(id, _)| id), Some(a));
    }

    #[test]
    fn backoff_doubles() {
        assert_eq!(backoff_ms(1), 5_000);
        assert_eq!(backoff_ms(2), 10_000);
        assert_eq!(backoff_ms(3), 20_000);
        assert_eq!(backoff_ms(7), 320_000);
        assert_eq!(backoff_ms(8), MAX_BACKOFF_MS);
        assert_eq!(backoff_ms(40), MAX_BACKOFF_MS);

        let mut q = Outbox::new();
        let id = q.push(item("message", b"x"), Priority::Normal);
        q.failed(id, 0);
        assert_eq!(q.next(4_999), None);
        assert!(q.next(5_000).is_some());
        q.failed(id, 5_000);
        assert_eq!(q.next(14_999), None);
        assert!(q.next(15_000).is_some());
    }

    #[test]
    fn gives_up_after_eight_attempts() {
        let mut q = Outbox::new();
        let id = q.push(item("message", b"x"), Priority::Normal);
        for n in 1..MAX_ATTEMPTS {
            q.failed(id, 0);
            assert_eq!(q.pending(), vec![(id, Priority::Normal, n)]);
        }
        q.failed(id, 0);
        assert!(q.pending().is_empty());
        assert_eq!(q.next(u64::MAX), None);
    }

    #[test]
    fn cancel_removes_item() {
        let mut q = Outbox::new();
        let a = q.push(item("message", b"a"), Priority::Normal);
        let b = q.push(item("mail", b"b"), Priority::Low);
        assert!(q.cancel(a));
        assert!(!q.cancel(a));
        assert!(!q.cancel(999));
        assert_eq!(q.pending(), vec![(b, Priority::Low, 0)]);
    }

    #[test]
    fn save_and_load_round_trip() {
        let mut q = Outbox::new();
        q.push(item("message", b"hello"), Priority::Urgent);
        let mail = q.push(item("mail", &[0u8, 1, 2, 255]), Priority::Normal);
        q.push(item("file", b""), Priority::Low);
        q.failed(mail, 1_000);

        let bytes = q.to_bytes();
        let mut loaded = Outbox::from_bytes(&bytes).expect("valid bytes");
        assert_eq!(loaded, q);
        assert_eq!(loaded.pending(), q.pending());
        assert_eq!(loaded.next(0).map(|(_, i)| i.kind), Some("message".to_string()));

        // Ids continue after a reload, so they never collide.
        let new_id = loaded.push(item("file", b"z"), Priority::Low);
        assert_eq!(new_id, 4);
    }

    #[test]
    fn bad_bytes_return_none() {
        assert!(Outbox::from_bytes(b"").is_none());
        assert!(Outbox::from_bytes(b"junk data here").is_none());

        let mut q = Outbox::new();
        q.push(item("message", b"abc"), Priority::Normal);
        let bytes = q.to_bytes();
        for n in 0..bytes.len() {
            assert!(Outbox::from_bytes(&bytes[..n]).is_none(), "prefix {n}");
        }
        let mut extra = bytes.clone();
        extra.push(0);
        assert!(Outbox::from_bytes(&extra).is_none());

        let mut bad_priority = bytes.clone();
        // Priority byte sits after magic(4) + next_id(8) + count(4) + id(8).
        bad_priority[24] = 9;
        assert!(Outbox::from_bytes(&bad_priority).is_none());
    }
}
