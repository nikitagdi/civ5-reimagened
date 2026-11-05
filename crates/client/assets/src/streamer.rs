//! Blob request queue with priorities.

use std::collections::{BTreeMap, HashMap};

use crate::ContentHash;

/// Lower is served first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Priority {
    /// The `boot` group and the first frame of the map: served before everything else (AB-D10).
    Boot,
    Visible,
    Adjacent,
    Prefetch,
}

/// Priority, then FIFO. Performs no I/O; `client-app` pops and fetches.
#[derive(Debug, Default)]
pub struct Streamer {
    order: BTreeMap<(Priority, u64), ContentHash>,
    index: HashMap<ContentHash, (Priority, u64)>,
    seq: u64,
}

impl Streamer {
    /// Empty queue.
    pub fn new() -> Self {
        Self::default()
    }

    /// Re-requesting upgrades priority but never downgrades it.
    pub fn request(&mut self, hash: ContentHash, priority: Priority) {
        match self.index.get(&hash).copied() {
            Some((old, _)) if old <= priority => {}
            Some((old, seq)) => {
                self.order.remove(&(old, seq));
                self.order.insert((priority, seq), hash);
                self.index.insert(hash, (priority, seq));
            }
            None => {
                let seq = self.seq;
                self.seq += 1;
                self.order.insert((priority, seq), hash);
                self.index.insert(hash, (priority, seq));
            }
        }
    }

    /// `true` if it was queued.
    pub fn cancel(&mut self, hash: &ContentHash) -> bool {
        match self.index.remove(hash) {
            Some(key) => {
                self.order.remove(&key);
                true
            }
            None => false,
        }
    }

    // Named by the interface; not an iterator since the queue changes between calls.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<(ContentHash, Priority)> {
        let ((priority, _), hash) = self.order.pop_first()?;
        self.index.remove(&hash);
        Some((hash, priority))
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    pub fn contains(&self, hash: &ContentHash) -> bool {
        self.index.contains_key(hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(n: u8) -> ContentHash {
        ContentHash([n; 32])
    }

    #[test]
    fn orders_by_priority_then_fifo() {
        let mut s = Streamer::new();
        s.request(h(1), Priority::Prefetch);
        s.request(h(2), Priority::Adjacent);
        s.request(h(3), Priority::Visible);
        s.request(h(4), Priority::Visible);
        s.request(h(5), Priority::Adjacent);
        let order: Vec<u8> = std::iter::from_fn(|| s.next())
            .map(|(h, _)| h.0[0])
            .collect();
        assert_eq!(order, [3, 4, 2, 5, 1]);
        assert!(s.is_empty());
    }

    #[test]
    fn boot_is_served_before_visible() {
        let mut s = Streamer::new();
        s.request(h(1), Priority::Visible);
        s.request(h(2), Priority::Boot);
        s.request(h(3), Priority::Visible);
        // A later request for a visible blob upgrades it past the earlier visible ones.
        s.request(h(3), Priority::Boot);
        let order: Vec<(u8, Priority)> = std::iter::from_fn(|| s.next())
            .map(|(h, p)| (h.0[0], p))
            .collect();
        assert_eq!(
            order,
            [
                (2, Priority::Boot),
                (3, Priority::Boot),
                (1, Priority::Visible)
            ]
        );
    }

    #[test]
    fn upgrade_but_never_downgrade() {
        let mut s = Streamer::new();
        s.request(h(1), Priority::Prefetch);
        s.request(h(2), Priority::Adjacent);
        s.request(h(1), Priority::Visible);
        s.request(h(2), Priority::Prefetch);
        assert_eq!(s.len(), 2);
        assert_eq!(s.next(), Some((h(1), Priority::Visible)));
        assert_eq!(s.next(), Some((h(2), Priority::Adjacent)));
        assert_eq!(s.next(), None);
    }

    #[test]
    fn cancel_removes() {
        let mut s = Streamer::new();
        s.request(h(1), Priority::Visible);
        assert!(s.contains(&h(1)));
        assert!(s.cancel(&h(1)));
        assert!(!s.cancel(&h(1)));
        assert_eq!(s.next(), None);
    }
}
