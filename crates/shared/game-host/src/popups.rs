//! Seat-local popup storage. Connections never own queue entries.
use std::collections::{BTreeMap, HashSet, VecDeque};

use protocol::PlayerId;
use serde::{Deserialize, Serialize};

/// Full popup payload, also accepted by the worker's fixture injection hook.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Popup {
    pub kind: String,
    pub data1: i32,
    pub data2: i32,
    pub data3: i32,
    pub flag: i32,
    pub option1: bool,
    pub option2: bool,
    pub text: Option<String>,
}

/// Core event awaiting enum-name resolution in the screen interpreter.
#[derive(Clone, Debug)]
pub struct CorePopup {
    pub player: PlayerId,
    pub popup_type: i32,
    pub popup: Popup,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PopupHead {
    pub id: i32,
    pub screen: String,
    pub a: Option<i32>,
    pub b: Option<i32>,
    pub decision: bool,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub head: PopupHead,
    pub popup: Popup,
}

#[derive(Default)]
pub(crate) struct Popups {
    next: i32,
    seats: BTreeMap<PlayerId, VecDeque<Entry>>,
    unknown: HashSet<String>,
}

impl Popups {
    pub fn unknown(&mut self, kind: &str) {
        if self.unknown.insert(kind.to_owned()) {
            tracing::warn!(kind, "unmapped popup dropped");
        }
    }

    pub fn next_id(&self) -> Result<i32, String> {
        self.next
            .checked_add(1)
            .ok_or_else(|| "popup ids exhausted".into())
    }

    pub fn push(
        &mut self,
        player: PlayerId,
        popup: Popup,
        mut head: PopupHead,
    ) -> Result<i32, String> {
        let id = self.next.checked_add(1).ok_or("popup ids exhausted")?;
        let queue = self.seats.entry(player).or_default();
        if queue.len() >= 64 {
            if let Some(index) = queue.iter().position(|entry| !entry.head.decision) {
                queue.remove(index);
                tracing::warn!(
                    player,
                    "popup queue overflow dropped oldest informational entry"
                );
            } else if !head.decision {
                tracing::warn!(
                    player,
                    "popup queue full of decisions; informational arrival dropped"
                );
                return Err("popup queue full of decisions".into());
            } else {
                // SIMPLIFIED(SIMP-411): decision-only overflow exceeds the soft bound to preserve every decision.
                tracing::error!(player, "popup queue decision-only overflow retained");
            }
        }
        self.next = id;
        head.id = id;
        queue.push_back(Entry { head, popup });
        Ok(id)
    }

    pub fn head(&self, player: PlayerId) -> Option<&Entry> {
        self.seats.get(&player)?.front()
    }

    pub fn len(&self, player: PlayerId) -> usize {
        self.seats.get(&player).map_or(0, VecDeque::len)
    }

    pub fn validate_done(
        &self,
        player: PlayerId,
        id: i32,
        screen: Option<&str>,
    ) -> Result<(), String> {
        let entry = self.head(player).ok_or("no pending popup")?;
        if entry.head.id != id
            || screen.is_some_and(|s| s != entry.head.screen)
            || (entry.head.decision && screen.is_none())
        {
            return Err("popup dismissal does not match pending entry".into());
        }
        Ok(())
    }

    pub fn done(&mut self, player: PlayerId, id: i32, screen: Option<&str>) -> Result<(), String> {
        self.validate_done(player, id, screen)?;
        if let Some(queue) = self.seats.get_mut(&player) {
            queue.pop_front();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(decision: bool) -> PopupHead {
        PopupHead {
            id: 0,
            screen: "techs".into(),
            a: None,
            b: None,
            decision,
        }
    }

    #[test]
    fn ordered_bounded_and_decisions_are_preserved() {
        let mut q = Popups::default();
        let decision = q.push(0, Popup::default(), head(true)).unwrap();
        for _ in 0..64 {
            q.push(0, Popup::default(), head(false)).unwrap();
        }
        assert_eq!(q.len(0), 64);
        assert_eq!(q.head(0).unwrap().head.id, decision);
        assert!(q.done(0, decision, None).is_err());
        assert!(q.done(0, decision, Some("other")).is_err());
        q.done(0, decision, Some("techs")).unwrap();
        assert_eq!(q.head(0).unwrap().head.id, 3);
        for _ in 0..64 {
            q.push(1, Popup::default(), head(true)).unwrap();
        }
        assert!(q.push(1, Popup::default(), head(false)).is_err());
        q.push(1, Popup::default(), head(true)).unwrap();
        assert_eq!(q.len(1), 65);
    }

    #[test]
    fn seat_isolation_and_reconnect_do_not_consume_entries() {
        let mut q = Popups::default();
        let id = q.push(0, Popup::default(), head(false)).unwrap();
        assert_eq!(q.len(1), 0);
        assert!(q.done(1, id, None).is_err());
        // Reading again models a fresh HUD subscription after reconnect.
        assert_eq!(q.head(0).unwrap().head.id, id);
        assert_eq!(q.head(0).unwrap().head.id, id);
        q.done(0, id, None).unwrap();
        assert!(q.head(0).is_none());
        assert!(q.push(0, Popup::default(), head(false)).unwrap() > id);
    }

    #[test]
    fn informational_bound_keeps_arrival_order_and_unknowns_log_once() {
        let mut q = Popups::default();
        for _ in 0..65 {
            q.push(0, Popup::default(), head(false)).unwrap();
        }
        assert_eq!(q.len(0), 64);
        for id in 2..=65 {
            assert_eq!(q.head(0).unwrap().head.id, id);
            assert!(q.done(0, id + 1, None).is_err());
            q.done(0, id, None).unwrap();
        }
        q.unknown("UNKNOWN");
        q.unknown("UNKNOWN");
        assert_eq!(q.unknown.len(), 1);
    }
}
