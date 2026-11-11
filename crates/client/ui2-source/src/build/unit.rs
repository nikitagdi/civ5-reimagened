//! The root `unit`: the unit selected in the UI (`ui.selection.unit`), its numbers, promotions and orders.

use std::sync::Arc;

use replica::{Activity, ReadError, Unit, UnitAction, UnitRow};
use ui2_model::r#gen::{Act, Pending, Promo, UnitActionKind, UnitActionRow, UnitActs};
use ui2_model::{Key, KeyedList};

use crate::CountingRead;
use crate::build::map::domain;
use crate::build::{entity_key, fraction};
use crate::catalog::Catalog;
use crate::pending;

/// The core's moves are in 1/60 of a move (`MOVE_DENOMINATOR`).
pub const MOVE_DENOMINATOR: i16 = 60;

/// The model's kind of a `Q_UNIT_ACTIONS` row: a unit command (`GC_CMD_UNIT_*`) or an extra row; `None` for a
/// value this source does not know.
#[must_use]
pub fn kind(a: &UnitAction) -> Option<UnitActionKind> {
    if let Some(row) = a.row() {
        return Some(match row {
            UnitRow::Alert => UnitActionKind::Alert,
            UnitRow::Pillage => UnitActionKind::Pillage,
            UnitRow::Upgrade => UnitActionKind::Upgrade,
        });
    }
    Some(match a.action {
        1 => UnitActionKind::Move,
        2 => UnitActionKind::RangeAttack,
        3 => UnitActionKind::FoundCity,
        4 => UnitActionKind::Fortify,
        5 => UnitActionKind::Sleep,
        6 => UnitActionKind::Skip,
        7 => UnitActionKind::Heal,
        8 => UnitActionKind::AutomateBuild,
        9 => UnitActionKind::AutomateExplore,
        10 => UnitActionKind::Cancel,
        11 => UnitActionKind::Disband,
        12 => UnitActionKind::Wake,
        _ => return None,
    })
}

/// The button of one kind.
fn act_mut(acts: &mut UnitActs, k: UnitActionKind) -> &mut Act {
    match k {
        UnitActionKind::Move => &mut acts.r#move,
        UnitActionKind::RangeAttack => &mut acts.range_attack,
        UnitActionKind::FoundCity => &mut acts.found_city,
        UnitActionKind::Fortify => &mut acts.fortify,
        UnitActionKind::Sleep => &mut acts.sleep,
        UnitActionKind::Skip => &mut acts.skip,
        UnitActionKind::Heal => &mut acts.heal,
        UnitActionKind::AutomateBuild => &mut acts.automate_build,
        UnitActionKind::AutomateExplore => &mut acts.automate_explore,
        UnitActionKind::Cancel => &mut acts.cancel,
        UnitActionKind::Disband => &mut acts.disband,
        UnitActionKind::Wake => &mut acts.wake,
        UnitActionKind::Alert => &mut acts.alert,
        UnitActionKind::Pillage => &mut acts.pillage,
        UnitActionKind::Upgrade => &mut acts.upgrade,
    }
}

/// The order rows in the core's order, and the same as one button per kind.
#[must_use]
pub fn actions(rows: &[UnitAction]) -> (KeyedList<UnitActionRow>, UnitActs) {
    let mut list = KeyedList::new();
    let mut acts = UnitActs::default();
    for a in rows {
        let Some(k) = kind(a) else {
            continue;
        };
        let reason: Option<Arc<str>> = a.reason.as_deref().map(Arc::from);
        let row = UnitActionRow {
            kind: k,
            enabled: a.enabled,
            reason: reason.clone(),
            // SIMPLIFIED(SIMP-250): alert, pillage and upgrade are shown with the core's availability, but there is
            // no order for them yet, so they cannot be given.
            supported: a.row().is_none(),
        };
        // One row per kind: a repeated kind keeps its first row.
        if list.try_push(row).is_ok() {
            *act_mut(&mut acts, k) = Act {
                shown: true,
                enabled: a.enabled,
                reason,
            };
        }
    }
    (list, acts)
}

/// `Activity` of the model.
#[must_use]
pub fn activity(a: Activity) -> ui2_model::r#gen::Activity {
    use ui2_model::r#gen::Activity as M;
    match a {
        Activity::Awake => M::Awake,
        Activity::Hold => M::Hold,
        Activity::Sleep => M::Sleep,
        Activity::Heal => M::Heal,
        Activity::Sentry => M::Sentry,
        Activity::Intercept => M::Intercept,
        Activity::Mission => M::Mission,
        Activity::Other(_) => M::Other,
    }
}

/// The unit a key names: `(owner << 32) | id`.
#[must_use]
pub fn find(units: &[Unit], key: Key) -> Option<&Unit> {
    units.iter().find(|u| entity_key(u.owner, u.id) == key)
}

/// The panel of one unit. `actions` and `promotions` are the reads of an own unit (empty for a foreign one).
#[must_use]
pub fn panel(
    u: &Unit,
    catalog: &Catalog,
    actions_rows: &[UnitAction],
    promotions: &[i16],
    pending_rows: &[Pending],
) -> ui2_model::r#gen::Unit {
    let key = entity_key(u.owner, u.id);
    let item = catalog.unit(u.unit_type);
    let (list, acts) = actions(actions_rows);
    let mut promos = KeyedList::new();
    for p in promotions {
        let it = catalog.promotion(*p);
        let _ = promos.try_push(Promo {
            id: *p,
            name: it.name,
            icon: it.icon,
        });
    }
    ui2_model::r#gen::Unit {
        key,
        name: Arc::from(u.name.as_str()),
        type_name: item.name,
        portrait: item.icon,
        level: i16::from(u.level),
        xp: u.experience,
        strength: u.strength,
        ranged: u.ranged,
        range: i16::from(u.range),
        moves: fraction(i32::from(u.moves_left), i32::from(MOVE_DENOMINATOR)),
        max_moves: u.max_moves / MOVE_DENOMINATOR,
        hp: u.hp,
        max_hp: u.max_hp,
        hp_fraction: fraction(i32::from(u.hp), i32::from(u.max_hp)),
        domain: domain(u.domain),
        activity: activity(u.activity),
        promotions: promos,
        promotion_ready: u.flags.can_promote(),
        actions: list,
        act: acts,
        pending: pending::waits(pending_rows, &pending::unit_target(key)),
    }
}

/// `unit` for the selected unit: `Q_UNITS` (kept), and for an own unit `Q_UNIT_ACTIONS` and `Q_UNIT_PROMOTIONS`.
/// No selection, or a unit that is gone: `None`, with no read.
pub(crate) fn unit(
    read: &mut CountingRead<'_, '_>,
    catalog: &Catalog,
    selected: Option<Key>,
    pending_rows: &[Pending],
) -> Result<Option<ui2_model::r#gen::Unit>, ReadError> {
    let Some(key) = selected else {
        return Ok(None);
    };
    let seat = read.seat();
    let units = read.units()?;
    let Some(u) = find(&units, key) else {
        return Ok(None);
    };
    let mine = u.owner.cast_unsigned() == seat;
    let (rows, promos) = if mine {
        let rows = read.unit_actions(u.id)?;
        // A core without `Q_UNIT_PROMOTIONS` shows no promotions.
        let promos = match read.unit_promotions(u.id) {
            Ok(p) => p,
            Err(ReadError::Unsupported) => Arc::new(Vec::new()),
            Err(e) => return Err(e),
        };
        (rows, promos)
    } else {
        (Arc::new(Vec::new()), Arc::new(Vec::new()))
    };
    Ok(Some(panel(u, catalog, &rows, &promos, pending_rows)))
}
