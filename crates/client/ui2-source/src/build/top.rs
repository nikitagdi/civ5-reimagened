//! The roots `top` (the top bar) and `research` (the current research, the research queue and the choices).

use replica::TopBarFlags;
use replica::{PlayerResource, PolicyBranchState, ReadError, ResourceUsage, TechState, TechStatus};
use ui2_model::r#gen::{GoldenAge, Mood, PolicyBranch, Research, Resource, Tech, Top};
use ui2_model::{Key, Keyed, KeyedList};

use crate::CountingRead;
use crate::build::fraction;
use crate::catalog::Catalog;

/// `EndTurnBlockingTypes::ENDTURN_BLOCKING_RESEARCH`: the turn waits for a research choice.
pub const BLOCKING_RESEARCH: i8 = 1;

/// Turns until the next policy: `ceil((cost - culture) / rate)`, `None` when culture does not grow.
#[must_use]
pub fn policy_turns(culture: i32, cost: i32, rate: i32) -> Option<i16> {
    if rate <= 0 {
        return None;
    }
    let left = (i64::from(cost) - i64::from(culture)).max(0);
    let rate = i64::from(rate);
    let turns = (left + rate - 1) / rate;
    Some(i16::try_from(turns).unwrap_or(i16::MAX))
}

/// The mood of the empire from the top bar's flags.
#[must_use]
pub fn mood(flags: TopBarFlags) -> Mood {
    if flags.very_unhappy() {
        Mood::VeryUnhappy
    } else if flags.unhappy() {
        Mood::Unhappy
    } else {
        Mood::Content
    }
}

/// A keyed list from rows in display order; a row whose key came before is dropped (the core's ids are unique,
/// so this never happens with a sound reply).
fn keyed<T: Keyed>(rows: impl IntoIterator<Item = T>) -> KeyedList<T> {
    let mut list = KeyedList::new();
    for r in rows {
        // A duplicate key is the only error: the first row with the key stays.
        let _ = list.try_push(r);
    }
    list
}

/// The strategic resources of the top bar, by id, with their names and icons.
#[must_use]
pub fn strategic_resources(rows: &[PlayerResource], catalog: &Catalog) -> KeyedList<Resource> {
    keyed(
        rows.iter()
            .filter(|r| r.usage == ResourceUsage::Strategic)
            .map(|r| {
                let item = catalog.resource(r.resource);
                Resource {
                    id: Key::from_i32(i32::from(r.resource)),
                    name: item.name,
                    icon: item.icon,
                    available: r.available,
                    used: r.used,
                    total: r.total,
                }
            }),
    )
}

/// The policy branches of the top bar, by id, with their names.
#[must_use]
pub fn policy_branches(rows: &[PolicyBranchState], catalog: &Catalog) -> KeyedList<PolicyBranch> {
    let small = |v: u8| i8::try_from(v).unwrap_or(i8::MAX);
    keyed(rows.iter().map(|b| PolicyBranch {
        id: b.branch,
        name: catalog.branch(b.branch).name,
        unlocked: b.unlocked,
        owned: small(b.owned),
        total: small(b.total),
    }))
}

/// A UI v2 read whose `Unsupported` (a core without the query) means "leave the field at its default".
fn optional<T>(r: Result<T, ReadError>) -> Result<Option<T>, ReadError> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(ReadError::Unsupported) => Ok(None),
        Err(e) => Err(e),
    }
}

/// `top` from `Q_TOP_BAR`, `Q_PLAYER_RESOURCES` (the strategic resources) and `Q_POLICY_BRANCHES`. A core without
/// the two lists leaves them empty; a core without the tourism field leaves `tourism` unknown.
pub(crate) fn top(read: &mut CountingRead<'_, '_>, catalog: &Catalog) -> Result<Top, ReadError> {
    let t = read.top_bar()?.clone();
    let resources = optional(read.player_resources())?
        .map(|rows| strategic_resources(&rows, catalog))
        .unwrap_or_default();
    let policies = optional(read.policy_branches())?
        .map(|rows| policy_branches(&rows, catalog))
        .unwrap_or_default();
    Ok(Top {
        science_rate: t.science_per_turn,
        gold: t.gold,
        gold_rate: t.gold_per_turn,
        happiness: i32::from(t.happiness),
        mood: mood(t.flags),
        golden_age: GoldenAge {
            active: t.flags.golden_age(),
            turns: t.golden_age_turns,
            progress: t.golden_age_progress,
            threshold: t.golden_age_threshold,
        },
        culture: t.culture,
        culture_rate: t.culture_per_turn,
        policy_cost: t.next_policy_cost,
        policy_turns: policy_turns(t.culture, t.next_policy_cost, t.culture_per_turn),
        faith: t.faith,
        faith_rate: t.faith_per_turn,
        tourism: t.tourism,
        resources,
        policies,
    })
}

/// A technology row of the model.
fn tech(t: &TechState, catalog: &Catalog) -> Tech {
    let item = catalog.tech(t.tech);
    Tech {
        id: Key::from_i32(i32::from(t.tech)),
        name: item.name,
        icon: item.icon,
        turns: t.turns,
        progress: t.progress,
        cost: t.cost,
        fraction: fraction(t.progress, t.cost),
    }
}

/// What `Q_TECHS` gives the research root: the next research (queue place 1), the queue behind the current
/// research in order, and the technologies that can be researched now by turns (never last), then by id.
#[must_use]
pub fn research_lists(
    techs: &[TechState],
    catalog: &Catalog,
) -> (Option<Tech>, KeyedList<Tech>, KeyedList<Tech>) {
    let mut queued: Vec<&TechState> = techs
        .iter()
        .filter(|t| t.queue.is_some_and(|q| q >= 1))
        .collect();
    queued.sort_by_key(|t| (t.queue, t.tech));
    let next = techs
        .iter()
        .find(|t| t.queue == Some(1))
        .map(|t| tech(t, catalog));
    let mut options: Vec<&TechState> = techs
        .iter()
        .filter(|t| t.status == TechStatus::CanResearch)
        .collect();
    options.sort_by_key(|t| (t.turns.is_none(), t.turns, t.tech));
    (
        next,
        keyed(queued.into_iter().map(|t| tech(t, catalog))),
        keyed(options.into_iter().map(|t| tech(t, catalog))),
    )
}

/// `research` from `Q_TOP_BAR` (the current research), `Q_GAME` (the research blocker) and `Q_TECHS` (the next
/// research, the queue, the choices). A core without `Q_TECHS` leaves those empty and `known = false`.
pub(crate) fn research(
    read: &mut CountingRead<'_, '_>,
    catalog: &Catalog,
) -> Result<Research, ReadError> {
    let blocked = read.game_info()?.blocker == BLOCKING_RESEARCH;
    let t = read.top_bar()?;
    let current = t.research_tech.map(|id| {
        let item = catalog.tech(id);
        Tech {
            id: Key::from_i32(i32::from(id)),
            name: item.name,
            icon: item.icon,
            turns: (t.research_turns >= 0).then_some(t.research_turns),
            progress: t.research_progress,
            cost: t.research_cost,
            fraction: fraction(t.research_progress, t.research_cost),
        }
    });
    let (known, (next, queue, options)) = match optional(read.techs())? {
        Some(techs) => (true, research_lists(&techs, catalog)),
        None => (false, (None, KeyedList::new(), KeyedList::new())),
    };
    Ok(Research {
        current,
        next,
        queue,
        options,
        known,
        blocked,
    })
}

#[cfg(test)]
mod tests {
    use super::policy_turns;

    #[test]
    fn policy_turns_round_up_and_need_growth() {
        assert_eq!(policy_turns(200, 360, 31), Some(6));
        assert_eq!(policy_turns(200, 386, 31), Some(6));
        assert_eq!(policy_turns(200, 387, 31), Some(7));
        assert_eq!(policy_turns(400, 360, 31), Some(0));
        assert_eq!(policy_turns(0, 100, 0), None);
        assert_eq!(policy_turns(0, 100, -3), None);
        assert_eq!(policy_turns(0, i32::MAX, 1), Some(i16::MAX));
    }
}
