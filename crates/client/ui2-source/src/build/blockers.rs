//! `turn.blockers` and `turn.reminders`: everything that holds the end of turn, in the core's order, and what does not
//! hold it but the original game reminds of.
//!
//! The core decides the first blocker (`CvGame::updateTestEndTurn`): the player's notifications oldest first, the first
//! that blocks (`CvNotifications::GetEndTurnBlockedType`); without one, a unit that can be promoted (unless promotions
//! may be saved), then the first unit that needs orders (stacked when it cannot stay where it is). The list goes on the
//! same way past the first: every blocking notification, every promotable unit, every unit waiting for orders. The
//! core's own blocker (`Q_GAME`) always comes first, so the button and the list agree; with no blocker from the core
//! the list is empty.
//!
//! Each entry carries what its notification opens in the original (`CvNotifications::Activate`): the unit to select,
//! the city whose production to choose, or the window of the choice with the arguments its popup would get.

use std::sync::Arc;

use ui2_model::KeyedList;
use ui2_model::r#gen::{EndTurnBlock, Plot, TurnBlocker};

use crate::build::entity_key;
use crate::build::notes::type_hash;
use crate::build::turn::{blocking, waiting};
use crate::read::{GameInfo, Notification, Unit};

/// The `seq` of the first reminder: reminders and blockers share the `turn.blocker` intent.
pub const REMINDER_SEQ: i32 = 1000;

/// Entries kept of each list.
const MAX_ENTRIES: usize = 64;

/// The blocking notification types (`GetEndTurnBlockedType`) and what they block with. City range attacks block only
/// with automatic end of turn, which a network game's client does not have: they are reminders.
const BLOCKING: &[(&str, EndTurnBlock)] = &[
    ("NOTIFICATION_DIPLO_VOTE", EndTurnBlock::DiploVote),
    ("NOTIFICATION_PRODUCTION", EndTurnBlock::Production),
    ("NOTIFICATION_POLICY", EndTurnBlock::Policy),
    ("NOTIFICATION_FREE_POLICY", EndTurnBlock::FreePolicy),
    ("NOTIFICATION_TECH", EndTurnBlock::Research),
    ("NOTIFICATION_FREE_TECH", EndTurnBlock::FreeTech),
    ("NOTIFICATION_FREE_GREAT_PERSON", EndTurnBlock::FreeItems),
    ("NOTIFICATION_FOUND_PANTHEON", EndTurnBlock::FoundPantheon),
    ("NOTIFICATION_FOUND_RELIGION", EndTurnBlock::FoundReligion),
    (
        "NOTIFICATION_ENHANCE_RELIGION",
        EndTurnBlock::EnhanceReligion,
    ),
    ("NOTIFICATION_SPY_STOLE_TECH", EndTurnBlock::StealTech),
    ("NOTIFICATION_MAYA_LONG_COUNT", EndTurnBlock::MayaLongCount),
    (
        "NOTIFICATION_FAITH_GREAT_PERSON",
        EndTurnBlock::FaithGreatPerson,
    ),
    (
        "NOTIFICATION_ADD_REFORMATION_BELIEF",
        EndTurnBlock::Reformation,
    ),
    (
        "NOTIFICATION_LEAGUE_CALL_FOR_PROPOSALS",
        EndTurnBlock::LeagueProposals,
    ),
    ("NOTIFICATION_CHOOSE_ARCHAEOLOGY", EndTurnBlock::Archaeology),
    (
        "NOTIFICATION_LEAGUE_CALL_FOR_VOTES",
        EndTurnBlock::LeagueVotes,
    ),
    ("NOTIFICATION_CHOOSE_IDEOLOGY", EndTurnBlock::Ideology),
];

/// Notifications that do not block and that the original reminds of on the turn panel.
const REMINDING: &[(&str, EndTurnBlock)] = &[
    (
        "NOTIFICATION_CITY_RANGE_ATTACK",
        EndTurnBlock::CityRangeAttack,
    ),
    (
        "NOTIFICATION_PLAYER_DEAL_RECEIVED",
        EndTurnBlock::DealReceived,
    ),
];

fn kind_of(table: &[(&str, EndTurnBlock)], hash: u32) -> Option<EndTurnBlock> {
    table
        .iter()
        .find(|(name, _)| type_hash(name) == hash)
        .map(|(_, k)| *k)
}

/// The skin window that takes the choice and its fixed first argument (the mode its popup opens it in). A research
/// choice and a production choice go to the HUD's own panels (the original's small choose-tech and production popups),
/// which the route opens: they have no entry here.
#[must_use]
pub fn window_of(kind: EndTurnBlock) -> Option<(&'static str, Option<i32>)> {
    use EndTurnBlock as K;
    Some(match kind {
        K::Policy | K::FreePolicy => ("policies", None),
        K::FreeTech => ("techs", Some(2)),
        K::StealTech => ("techs", Some(3)),
        K::Ideology => ("ideology", None),
        K::FoundPantheon => ("pantheon", Some(0)),
        K::Reformation => ("pantheon", Some(1)),
        K::FoundReligion | K::EnhanceReligion => ("religion", None),
        K::DiploVote => ("mp_vote", Some(2)),
        K::LeagueProposals | K::LeagueVotes => ("congress", Some(0)),
        K::FreeItems => ("free_great_person", Some(0)),
        K::FaithGreatPerson => ("free_great_person", Some(1)),
        K::MayaLongCount => ("free_great_person", Some(2)),
        K::Archaeology => ("archaeology", None),
        K::DealReceived => ("deal", None),
        _ => return None,
    })
}

fn entry(kind: EndTurnBlock) -> TurnBlocker {
    let (window, a) = window_of(kind).map_or((None, None), |(w, a)| (Some(Arc::from(w)), a));
    TurnBlocker {
        kind,
        window,
        a,
        ..TurnBlocker::default()
    }
}

/// The entry of a notification, with what its activation opens. The city of a production, a religion or a city
/// attack is the seat's city on the notification's plot: the route finds it on the map (`map.cities`), so the turn
/// root does not read the cities.
fn note_entry(kind: EndTurnBlock, n: &Notification) -> TurnBlocker {
    let mut e = entry(kind);
    e.plot = n.plot().map(|(x, y)| Plot { x, y });
    let text = if n.summary.is_empty() {
        &n.message
    } else {
        &n.summary
    };
    e.text = (!text.is_empty()).then(|| Arc::from(text.as_str()));
    match kind {
        // `b` 1 = enhance the religion founded already.
        EndTurnBlock::FoundReligion | EndTurnBlock::EnhanceReligion => {
            e.b = Some(i32::from(kind == EndTurnBlock::EnhanceReligion));
        }
        // The player to steal from (`BUTTONPOPUP_CHOOSE_TECH_TO_STEAL` gets the notification's data).
        EndTurnBlock::StealTech => e.b = Some(n.data1),
        // The deal of the player the notification's x names (`OpenPlayerDealScreen`).
        EndTurnBlock::DealReceived => {
            e.a = Some(i32::from(n.x));
            e.plot = None;
        }
        _ => {}
    }
    e
}

fn unit_entry(kind: EndTurnBlock, u: &Unit) -> TurnBlocker {
    TurnBlocker {
        unit: Some(entity_key(u.owner, u.id)),
        plot: Some(Plot { x: u.x, y: u.y }),
        ..entry(kind)
    }
}

/// The core's own blocker when the list does not have it: its kind and target as `Q_GAME` gives them.
fn core_entry(info: &GameInfo, kind: EndTurnBlock, seat: i8) -> TurnBlocker {
    let mut e = entry(kind);
    e.unit = (info.blocker_unit >= 0).then(|| entity_key(seat, info.blocker_unit));
    e.city = (info.blocker_city >= 0).then(|| entity_key(seat, info.blocker_city));
    e.plot = (info.blocker_x >= 0 && info.blocker_y >= 0).then_some(Plot {
        x: info.blocker_x,
        y: info.blocker_y,
    });
    e
}

fn numbered(v: Vec<TurnBlocker>, from: i32) -> KeyedList<TurnBlocker> {
    let mut out = KeyedList::new();
    for (i, mut e) in v.into_iter().take(MAX_ENTRIES).enumerate() {
        e.seq = from + i32::try_from(i).unwrap_or(0);
        let _ = out.try_push(e);
    }
    out
}

/// The blockers and the reminders of the seat.
#[must_use]
pub fn build(
    info: &GameInfo,
    units: &[Unit],
    notes: &[Notification],
    seat: u8,
) -> (KeyedList<TurnBlocker>, KeyedList<TurnBlocker>) {
    let me = seat.cast_signed();
    let mut hard: Vec<TurnBlocker> = Vec::new();
    let mut soft: Vec<TurnBlocker> = Vec::new();
    for n in notes.iter().filter(|n| !n.dismissed) {
        if let Some(kind) = kind_of(BLOCKING, n.kind) {
            hard.push(note_entry(kind, n));
        } else if let Some(kind) = kind_of(REMINDING, n.kind) {
            soft.push(note_entry(kind, n));
        }
    }
    let own: Vec<&Unit> = units.iter().filter(|u| u.owner == me).collect();
    // Promotions block unless the game lets them be saved. The core shows which: with a promotable unit and no
    // blocking notification, it names a unit that needs orders (or nothing) only when promotions may be saved.
    // SIMPLIFIED(SIMP-540): the game option itself (GAMEOPTION_PROMOTION_SAVING) is not in the replica's records, so
    // while a notification blocks, promotions are listed as blockers after it; the core's next blocker settles it.
    let promotable: Vec<&Unit> = own
        .iter()
        .copied()
        .filter(|u| u.flags.can_promote())
        .collect();
    let saved = hard.is_empty()
        && !promotable.is_empty()
        && matches!(info.blocker, blocking::UNITS | blocking::STACKED_UNITS | -1);
    for u in &promotable {
        let e = unit_entry(EndTurnBlock::UnitPromotion, u);
        if saved {
            soft.push(e);
        } else {
            hard.push(e);
        }
    }
    for u in own.iter().filter(|u| waiting(u)) {
        let stacked = info.blocker == blocking::STACKED_UNITS && info.blocker_unit == u.id;
        let kind = if stacked {
            EndTurnBlock::StackedUnits
        } else {
            EndTurnBlock::Units
        };
        hard.push(unit_entry(kind, u));
    }
    match u16::try_from(info.blocker)
        .ok()
        .and_then(EndTurnBlock::from_u16)
    {
        // No blocker from the core: the turn can end, whatever the replica suggests.
        None => hard.clear(),
        Some(kind) => {
            let core_unit = (info.blocker_unit >= 0).then(|| entity_key(me, info.blocker_unit));
            let at = hard
                .iter()
                .position(|e| e.kind == kind && (core_unit.is_none() || e.unit == core_unit));
            match at {
                Some(0) => {}
                Some(i) => {
                    let e = hard.remove(i);
                    hard.insert(0, e);
                }
                None => hard.insert(0, core_entry(info, kind, me)),
            }
        }
    }
    (numbered(hard, 0), numbered(soft, REMINDER_SEQ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::read::{Activity, UnitFlags};

    fn note(name: &str, x: i16, y: i16, dismissed: bool) -> Notification {
        Notification {
            id: 0,
            kind: type_hash(name),
            x,
            y,
            data1: 0,
            data2: 0,
            turn: 0,
            dismissed,
            message: String::new(),
            summary: String::new(),
        }
    }

    fn unit(id: i32, waits: bool, promote: bool) -> Unit {
        Unit {
            id,
            owner: 0,
            unit_type: 0,
            x: 3,
            y: 4,
            hp: 100,
            max_hp: 100,
            moves_left: if waits { 60 } else { 0 },
            max_moves: 60,
            strength: 0,
            ranged: 0,
            range: 0,
            domain: 0,
            activity: Activity::Awake,
            level: 1,
            experience: 0,
            flags: UnitFlags(if promote { 0x0080 } else { 0 }),
            transport_id: None,
            build_type: None,
            facing: None,
            name: String::new(),
        }
    }

    fn info(blocker: i8, unit: i32) -> GameInfo {
        GameInfo {
            blocker,
            blocker_unit: unit,
            blocker_city: -1,
            blocker_x: -1,
            blocker_y: -1,
            ..GameInfo::default()
        }
    }

    fn kinds(l: &KeyedList<TurnBlocker>) -> Vec<EndTurnBlock> {
        l.iter().map(|e| e.kind).collect()
    }

    #[test]
    fn notifications_come_first_in_their_order_then_promotions_then_units() {
        use EndTurnBlock as K;
        let notes = [
            note("NOTIFICATION_PRODUCTION", 5, 6, false),
            note("NOTIFICATION_TECH", -1, -1, true),
            note("NOTIFICATION_FOUND_PANTHEON", -1, -1, false),
            note("NOTIFICATION_PLAYER_DEAL_RECEIVED", 2, -1, false),
            note("NOTIFICATION_CITY_GROWTH", 5, 6, false),
            note("NOTIFICATION_ENHANCE_RELIGION", 5, 6, false),
        ];
        let units = [
            unit(1, true, false),
            unit(2, false, true),
            unit(3, true, false),
        ];
        let (hard, soft) = build(&info(2, -1), &units, &notes, 0);
        assert_eq!(
            kinds(&hard),
            [
                K::Production,
                K::FoundPantheon,
                K::EnhanceReligion,
                K::UnitPromotion,
                K::Units,
                K::Units
            ],
            "a dismissed notification does not block, a city growth does not either"
        );
        let e: Vec<&TurnBlocker> = hard.iter().collect();
        assert_eq!(e[0].plot, Some(Plot { x: 5, y: 6 }));
        assert!(
            e[0].window.is_none(),
            "production goes to the city on the plot"
        );
        assert_eq!(
            (e[1].window.as_deref(), e[1].a),
            (Some("pantheon"), Some(0))
        );
        assert_eq!(
            (e[2].window.as_deref(), e[2].plot.as_ref(), e[2].b),
            (Some("religion"), Some(&Plot { x: 5, y: 6 }), Some(1))
        );
        assert_eq!(e[3].unit, Some(entity_key(0, 2)));
        assert_eq!(e[4].unit, Some(entity_key(0, 1)));
        assert_eq!((e[0].seq, e[5].seq), (0, 5));
        let r: Vec<&TurnBlocker> = soft.iter().collect();
        assert_eq!(kinds(&soft), [K::DealReceived]);
        assert_eq!(
            (r[0].seq, r[0].window.as_deref(), r[0].a),
            (REMINDER_SEQ, Some("deal"), Some(2))
        );
    }

    #[test]
    fn the_cores_blocker_leads_and_saved_promotions_only_remind() {
        use EndTurnBlock as K;
        // Promotions may be saved: the core names a unit that needs orders although one can be promoted.
        let units = [unit(1, true, false), unit(2, true, true)];
        let (hard, soft) = build(&info(blocking::STACKED_UNITS, 2), &units, &[], 0);
        assert_eq!(kinds(&hard), [K::StackedUnits, K::Units]);
        assert_eq!(hard.iter().next().unwrap().unit, Some(entity_key(0, 2)));
        assert_eq!(kinds(&soft), [K::UnitPromotion]);
        // A blocker the list cannot see (a minor quest) still comes first, with the core's target.
        let mut quest = info(5, -1);
        quest.blocker_x = 7;
        quest.blocker_y = 8;
        let (hard, _) = build(&quest, &units, &[], 0);
        assert_eq!(
            kinds(&hard),
            [K::MinorQuest, K::UnitPromotion, K::Units, K::Units]
        );
        assert_eq!(hard.iter().next().unwrap().plot, Some(Plot { x: 7, y: 8 }));
        // No blocker from the core: nothing blocks, a promotion only reminds.
        let (hard, soft) = build(&info(-1, -1), &units, &[], 0);
        assert!(hard.is_empty());
        assert_eq!(kinds(&soft), [K::UnitPromotion]);
    }

    #[test]
    fn another_players_units_do_not_count() {
        let mut theirs = unit(1, true, true);
        theirs.owner = 1;
        let (hard, soft) = build(&info(3, 5), &[theirs], &[], 0);
        assert_eq!(
            kinds(&hard),
            [EndTurnBlock::Units],
            "only the core's own entry"
        );
        assert_eq!(hard.iter().next().unwrap().unit, Some(entity_key(0, 5)));
        assert!(soft.is_empty());
    }
}
