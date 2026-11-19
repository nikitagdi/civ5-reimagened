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
    /// Fourth and fifth integers of an array popup (captured city: Great Works taken, player that
    /// may be liberated or -1); `None` when the core event had none.
    pub data4: Option<i32>,
    pub data5: Option<i32>,
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
    /// For a move's war question: what makes it stale (`Popups::watch`).
    pub(crate) watch: Option<MoveWatch>,
}

/// The core's war question for a move (`BUTTONPOPUP_DECLAREWARMOVE`) belongs to one order: the turn it was asked in and,
/// when the core named the unit, where that unit stood. A seat whose client never answers it (the classic client) must
/// not keep it, so it lapses when the turn changes or the unit moved or is gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MoveWatch {
    pub turn: i32,
    /// (unit id, x, y) at the question.
    pub unit: Option<(i32, i32, i32)>,
}

pub(crate) const DECLARE_WAR_MOVE: &str = "BUTTONPOPUP_DECLAREWARMOVE";

/// The unit a move's war question names, when the core named one.
fn question_unit(popup: &Popup) -> Option<i32> {
    (popup.kind == DECLARE_WAR_MOVE)
        .then_some(popup.data4)
        .flatten()
        .filter(|unit| *unit >= 0)
}

/// Two decisions ask the same thing: the kind and every integer and flag of the payload agree (the text is a rendering
/// of them).
fn same_question(a: &Popup, b: &Popup) -> bool {
    a.kind == b.kind
        && (a.data1, a.data2, a.data3, a.data4, a.data5)
            == (b.data1, b.data2, b.data3, b.data4, b.data5)
        && (a.flag, a.option1, a.option2) == (b.flag, b.option1, b.option2)
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
        if let Some(unit) = question_unit(&popup) {
            // A newer order of the same unit asks again: the older question is moot (one per unit, no pile-up).
            queue.retain(|entry| question_unit(&entry.popup) != Some(unit));
        }
        if head.decision && popup.kind != DECLARE_WAR_MOVE {
            // The same question again (same kind and payload): one answer settles both, the newer one stays. A move's
            // war question belongs to its order and follows its own rule above.
            queue.retain(|entry| !(entry.head.decision && same_question(&entry.popup, &popup)));
        }
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
                // SIMPLIFIED(SIMP-411): more than 64 distinct live decisions exceed the soft bound so that none is lost
                // (a repeated question merges and a lapsed head is dropped, so only distinct live ones pile up).
                tracing::error!(player, "popup queue decision-only overflow retained");
            }
        }
        self.next = id;
        head.id = id;
        queue.push_back(Entry {
            head,
            popup,
            watch: None,
        });
        Ok(id)
    }

    /// Marks a queued war question of a move with what makes it stale.
    pub(crate) fn watch(&mut self, player: PlayerId, id: i32, watch: MoveWatch) {
        if let Some(entry) = self
            .seats
            .get_mut(&player)
            .and_then(|queue| queue.iter_mut().find(|entry| entry.head.id == id))
        {
            entry.watch = Some(watch);
        }
    }

    /// The watched questions: (seat, popup id, watch).
    pub(crate) fn watched(&self) -> Vec<(PlayerId, i32, MoveWatch)> {
        self.seats
            .iter()
            .flat_map(|(player, queue)| {
                queue
                    .iter()
                    .filter_map(move |entry| entry.watch.map(|w| (*player, entry.head.id, w)))
            })
            .collect()
    }

    /// Drops one entry wherever it is in the seat's queue.
    pub(crate) fn remove(&mut self, player: PlayerId, id: i32) {
        if let Some(queue) = self.seats.get_mut(&player) {
            queue.retain(|entry| entry.head.id != id);
        }
    }

    pub fn head(&self, player: PlayerId) -> Option<&Entry> {
        self.seats.get(&player)?.front()
    }

    /// A captured city stays read-only until its authenticated choice is submitted.
    pub fn capture_pending(&self, player: PlayerId, city: i32) -> bool {
        self.seats.get(&player).is_some_and(|queue| {
            queue.iter().any(|entry| {
                entry.popup.kind == "BUTTONPOPUP_CITY_CAPTURED" && entry.popup.data1 == city
            })
        })
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
        let distinct = |n| Popup {
            data1: n,
            ..Popup::default()
        };
        for n in 0..64 {
            q.push(1, distinct(n), head(true)).unwrap();
        }
        assert!(q.push(1, Popup::default(), head(false)).is_err());
        q.push(1, distinct(64), head(true)).unwrap();
        assert_eq!(q.len(1), 65);
    }

    #[test]
    fn the_same_decision_asked_again_is_kept_once_and_the_newest_stays() {
        let mut q = Popups::default();
        let city = |n| Popup {
            kind: "BUTTONPOPUP_CHOOSEPRODUCTION".into(),
            data1: n,
            ..Popup::default()
        };
        let old = q.push(0, city(7), head(true)).unwrap();
        let other = q.push(0, city(8), head(true)).unwrap();
        let info = q.push(0, city(7), head(false)).unwrap();
        let again = q.push(0, city(7), head(true)).unwrap();
        let ids: Vec<i32> = q.seats[&0].iter().map(|e| e.head.id).collect();
        assert_eq!(ids, vec![other, info, again], "the older {old} went");
    }

    #[test]
    fn one_move_question_per_unit_and_watched_entries_can_lapse() {
        let mut q = Popups::default();
        let question = |unit| Popup {
            kind: DECLARE_WAR_MOVE.into(),
            data4: Some(unit),
            ..Popup::default()
        };
        let info = q.push(0, Popup::default(), head(false)).unwrap();
        let first = q.push(0, question(9), head(true)).unwrap();
        let other = q.push(0, question(10), head(true)).unwrap();
        let again = q.push(0, question(9), head(true)).unwrap();
        // The unit's newer order replaced its older question; other units and other popups stay.
        let ids: Vec<i32> = q.seats[&0].iter().map(|e| e.head.id).collect();
        assert_eq!(ids, vec![info, other, again]);
        assert!(!ids.contains(&first));
        // Questions without a named unit (-1) are never merged.
        q.push(1, question(-1), head(true)).unwrap();
        q.push(1, question(-1), head(true)).unwrap();
        assert_eq!(q.len(1), 2);
        let watch = MoveWatch {
            turn: 3,
            unit: Some((10, 4, 5)),
        };
        q.watch(0, other, watch);
        assert_eq!(q.watched(), vec![(0, other, watch)]);
        q.remove(0, other);
        assert!(q.watched().is_empty());
        assert_eq!(q.len(0), 2);
        assert_eq!(q.head(0).unwrap().head.id, info);
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
