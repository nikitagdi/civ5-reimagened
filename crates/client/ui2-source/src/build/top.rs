//! The roots `top` (the top bar) and `research` (the current research).

use replica::{ReadError, TopBarFlags};
use ui2_model::r#gen::{GoldenAge, Mood, Research, Tech, Top};
use ui2_model::{Key, KeyedList};

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

/// `top` from `Q_TOP_BAR`.
pub(crate) fn top(read: &mut CountingRead<'_, '_>) -> Result<Top, ReadError> {
    let t = read.top_bar()?;
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
        // Tourism, the strategic resources and the policy branches need read queries the core does not serve
        // yet; until then they stay at the schema's default (unknown, empty).
        tourism: None,
        resources: KeyedList::default(),
        policies: KeyedList::default(),
    })
}

/// `research` from `Q_TOP_BAR` (the current research) and `Q_GAME` (the research blocker).
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
    Ok(Research {
        current,
        // The queue, the next research and the options need the core's technology query, which it does not
        // serve yet: `known = false` says so to the skin.
        next: None,
        queue: KeyedList::default(),
        options: KeyedList::default(),
        known: false,
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
