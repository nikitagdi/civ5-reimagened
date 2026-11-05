//! The unit panel of the selected unit (play-ui §3.3, AC-5.1, AC-5.6-AC-5.9): the actions of its
//! `UnitContext` (net-client 4.6), and the order of each action button. What is possible is the
//! server's answer (`enabled`, `reason`), never computed here. A stale context is shown with its
//! buttons grey ("waiting for the server"), a missing one as "loading"; without a connection the
//! panel is grey ("No connection", NC-D5).
//!
//! SIMPLIFIED(SIMP-001): text buttons for the unit commands of the ABI; original: the UnitPanel Lua
//! screen with the action icons of `UnitActions` / `InterfaceModes`.

use client_core::Ctx;
use interim_ui::screens::unit_panel::{ActionButton, UnitPanelView};
use protocol::{
    Catalog, Command, Shooter, TurnStatus, UnitActionView, UnitContext, UnitFlags, UnitView,
    WorldView,
};

use crate::play::Mode;

/// gc-abi-v1 unit command numbers (`GC_CMD_UNIT_*`, gamecore_abi.h) the panel treats specially.
pub const ACTION_MOVE: i16 = 1;
pub const ACTION_RANGE_ATTACK: i16 = 2;
pub const ACTION_DISBAND: i16 = 11;

/// A unit command's button text and key. The keys are the game's (`Missions`, `InterfaceModes`,
/// `Automates`, `Commands` hotkeys of the database) where the client binds them; Sleep is on S here
/// (play-ui §3.2; the game has it on F with Fortify). `None` for a command without a button: the
/// move is a right click.
pub fn action_info(action: i16) -> Option<(&'static str, Option<&'static str>)> {
    Some(match action {
        ACTION_RANGE_ATTACK => ("Ranged attack", Some("B")),
        3 => ("Found city", Some("B")),
        4 => ("Fortify", Some("F")),
        5 => ("Sleep", Some("S")),
        6 => ("Skip turn", Some("Space")),
        7 => ("Heal", Some("H")),
        8 => ("Automate", Some("A")),
        9 => ("Explore", Some("E")),
        10 => ("Cancel orders", Some("Backspace")),
        ACTION_DISBAND => ("Disband", None),
        12 => ("Wake", None),
        _ => return None,
    })
}

/// The order of an action button that is a plain command (not the ranged mode, not the move).
pub fn command_for(action: i16, unit: i32) -> Option<Command> {
    Some(match action {
        3 => Command::UnitFoundCity { unit },
        4 => Command::UnitFortify { unit },
        5 => Command::UnitSleep { unit },
        6 => Command::UnitSkip { unit },
        7 => Command::UnitHeal { unit },
        8 => Command::UnitAutomateBuild { unit },
        9 => Command::UnitAutomateExplore { unit },
        10 => Command::UnitCancel { unit },
        ACTION_DISBAND => Command::UnitDisband { unit },
        12 => Command::UnitWake { unit },
        _ => return None,
    })
}

/// Why the buttons are grey, when they are (the first that applies).
const NO_CONNECTION: &str = "No connection";
pub const RIVALS_MOVING: &str = "Rivals are moving";
const WAITING: &str = "Waiting for the server";

#[derive(Debug, Default)]
pub struct UnitPanel {
    unit: Option<i32>,
    actions: Option<Vec<UnitActionView>>,
    /// The context is older than the view: shown, not usable.
    stale: bool,
    /// The line is up and a game is attached (`ClientCore::orders_enabled`).
    orders_enabled: bool,
    /// Why the phase of the turn takes no orders: "Rivals are moving", "Waiting for Bob" (hybrid).
    blocked: Option<String>,
}

impl UnitPanel {
    /// Takes the context of `selected` as it is now; call every frame. A new selection forgets the old
    /// actions at once.
    pub fn sync(&mut self, selected: Option<i32>, ctx: Ctx<'_, UnitContext>, orders_enabled: bool) {
        if selected != self.unit {
            self.unit = selected;
            self.actions = None;
        }
        self.orders_enabled = orders_enabled;
        self.stale = ctx.is_stale();
        match ctx.get() {
            Some(c) if selected.is_some() => {
                if self.actions.as_ref() != Some(&c.actions) {
                    self.actions = Some(c.actions.clone());
                }
            }
            _ => self.actions = None,
        }
    }

    /// The reason the phase of the turn gives for grey buttons (`hud::order_block`); `None`: none.
    pub fn set_blocked(&mut self, why: Option<String>) {
        self.blocked = why;
    }

    /// The panel of the selected unit; `None` without one (or when it is gone).
    pub fn view(
        &self,
        world: &WorldView,
        catalog: Option<&Catalog>,
        mode: Mode,
    ) -> Option<UnitPanelView> {
        let id = self.unit?;
        let u = world
            .units
            .iter()
            .find(|u| u.id == id && u.owner == world.human)?;
        let aiming = mode == Mode::RangeAttack(Shooter::Unit(id));
        let human_turn = world.status == TurnStatus::HumanTurn;
        let actions = self
            .actions
            .iter()
            .flatten()
            .filter_map(|a| {
                let (label, hotkey) = action_info(a.action)?;
                let (enabled, reason) = if !self.orders_enabled {
                    (false, NO_CONNECTION.to_owned())
                } else if let Some(why) = &self.blocked {
                    (false, why.clone())
                } else if !human_turn {
                    (false, RIVALS_MOVING.to_owned())
                } else if self.stale {
                    (false, WAITING.to_owned())
                } else {
                    (a.enabled, a.reason.clone())
                };
                Some(ActionButton {
                    action: a.action,
                    label: label.to_owned(),
                    hotkey: hotkey.map(str::to_owned),
                    enabled,
                    reason,
                    active: aiming && a.action == ACTION_RANGE_ATTACK,
                })
            })
            .collect();
        let type_name = catalog
            .and_then(|c| c.units.iter().find(|e| e.id == u.unit_type))
            .map(|e| e.name.clone())
            .filter(|n| *n != u.name);
        Some(UnitPanelView {
            name: u.name.clone(),
            subtitle: type_name,
            stats: stats(u),
            health: if u.max_hp > 0 {
                f32::from(u.hp) / f32::from(u.max_hp)
            } else {
                1.0
            },
            status: status(u),
            actions,
            loading: self.actions.is_none(),
            aiming: aiming
                .then(|| "Ranged attack: click a red plot. Esc or right click cancels.".to_owned()),
        })
    }
}

/// Moves in 1/60 (the core's `MOVE_DENOMINATOR`) as the original shows them.
fn moves(m: i16) -> String {
    if m % 60 == 0 {
        (m / 60).to_string()
    } else {
        format!("{:.1}", f32::from(m) / 60.0)
    }
}

fn stats(u: &UnitView) -> Vec<(String, String)> {
    let mut out = vec![(
        "Moves".to_owned(),
        format!("{}/{}", moves(u.moves_left), moves(u.max_moves)),
    )];
    if u.strength > 0 {
        out.push(("Strength".to_owned(), u.strength.to_string()));
    }
    if u.ranged > 0 {
        out.push((
            "Ranged".to_owned(),
            format!("{}, range {}", u.ranged, u.range),
        ));
    }
    out.push(("HP".to_owned(), format!("{}/{}", u.hp, u.max_hp)));
    out
}

fn status(u: &UnitView) -> Vec<String> {
    let mut out: Vec<String> = [
        (UnitFlags::FORTIFIED, "Fortified"),
        (UnitFlags::SLEEPING, "Sleeping"),
        (UnitFlags::AUTOMATED, "Automated"),
        (UnitFlags::EMBARKED, "Embarked"),
        (UnitFlags::GARRISONED, "Garrisoned"),
    ]
    .iter()
    .filter(|(f, _)| u.flags.contains(*f))
    .map(|(_, s)| (*s).to_owned())
    .collect();
    if u.level > 1 || u.experience > 0 {
        out.push(format!("Level {}, {} XP", u.level.max(1), u.experience));
    }
    if u.flags.contains(UnitFlags::CAN_PROMOTE) {
        // SIMPLIFIED(SIMP-002): the promotion is picked by the AI for the human (journaled).
        out.push("Promotion due (chosen automatically)".to_owned());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> WorldView {
        crate::test_world::world()
    }

    fn action(action: i16, enabled: bool, reason: &str) -> UnitActionView {
        UnitActionView {
            action,
            enabled,
            reason: reason.into(),
        }
    }

    fn ctx(unit: i32, actions: Vec<UnitActionView>) -> UnitContext {
        UnitContext {
            unit,
            view_seq: 5,
            actions,
            ..UnitContext::default()
        }
    }

    #[test]
    fn a_new_selection_forgets_the_old_actions_at_once() {
        let c = ctx(4, vec![action(4, true, "")]);
        let mut p = UnitPanel::default();
        p.sync(Some(4), Ctx::Fresh(&c), true);
        assert!(p.actions.is_some());
        // Selected another unit whose context is not in yet: nothing of the first one is shown.
        p.sync(Some(5), Ctx::Missing, true);
        assert!(p.actions.is_none());
        p.sync(None, Ctx::Fresh(&c), true);
        assert!(p.actions.is_none(), "no selection, no panel");
    }

    #[test]
    fn the_view_shows_the_unit_and_the_contexts_actions() {
        let w = world();
        let c = ctx(
            4,
            vec![
                action(1, true, ""),
                action(2, true, ""),
                action(4, false, "Already fortified"),
                action(6, true, ""),
            ],
        );
        let mut p = UnitPanel::default();
        // Archer 4: 6/10 HP, ranged 7, range 2. No context yet: loading.
        p.sync(Some(4), Ctx::Missing, true);
        assert!(p.view(&w, None, Mode::Normal).unwrap().loading);
        p.sync(Some(4), Ctx::Fresh(&c), true);
        let v = p.view(&w, None, Mode::Normal).unwrap();
        assert_eq!(v.name, "Archer");
        assert!(!v.loading && v.aiming.is_none());
        assert_eq!(
            v.stats,
            [
                ("Moves".to_owned(), "2/2".to_owned()),
                ("Strength".to_owned(), "5".to_owned()),
                ("Ranged".to_owned(), "7, range 2".to_owned()),
                ("HP".to_owned(), "6/10".to_owned()),
            ]
        );
        assert!((v.health - 0.6).abs() < 1e-6);
        let buttons: Vec<_> = v
            .actions
            .iter()
            .map(|b| (b.action, b.label.as_str(), b.enabled, b.reason.as_str()))
            .collect();
        assert_eq!(
            buttons,
            [
                (2, "Ranged attack", true, ""),
                (4, "Fortify", false, "Already fortified"),
                (6, "Skip turn", true, ""),
            ],
            "the move has no button; the rest is the server's answer"
        );
        // Aiming: the button is on and the panel says how to aim.
        let v = p
            .view(&w, None, Mode::RangeAttack(Shooter::Unit(4)))
            .unwrap();
        assert!(v.aiming.is_some() && v.actions[0].active);
        // The rivals' turn disables everything.
        let mut ai = w.clone();
        ai.status = TurnStatus::AiTurn { active: 1 };
        let v = p.view(&ai, None, Mode::Normal).unwrap();
        assert!(
            v.actions
                .iter()
                .all(|b| !b.enabled && b.reason == RIVALS_MOVING)
        );
        // A foreign or missing unit has no panel.
        p.sync(Some(9), Ctx::Missing, true);
        assert!(p.view(&w, None, Mode::Normal).is_none());
    }

    #[test]
    fn a_stale_context_is_shown_grey_and_no_connection_greys_the_panel() {
        let w = world();
        let c = ctx(4, vec![action(4, true, ""), action(6, true, "")]);
        let mut p = UnitPanel::default();
        p.sync(Some(4), Ctx::Stale(&c), true);
        let v = p.view(&w, None, Mode::Normal).unwrap();
        assert!(!v.loading, "stale is shown, not loading");
        assert_eq!(v.actions.len(), 2);
        assert!(v.actions.iter().all(|b| !b.enabled && b.reason == WAITING));
        // Fresh again: the buttons are the server's.
        p.sync(Some(4), Ctx::Fresh(&c), true);
        assert!(
            p.view(&w, None, Mode::Normal)
                .unwrap()
                .actions
                .iter()
                .all(|b| b.enabled)
        );
        // The line is down: grey with the reason, whatever the context says.
        p.sync(Some(4), Ctx::Fresh(&c), false);
        let v = p.view(&w, None, Mode::Normal).unwrap();
        assert!(
            v.actions
                .iter()
                .all(|b| !b.enabled && b.reason == NO_CONNECTION)
        );
    }

    #[test]
    fn the_warrior_is_fortified_with_experience() {
        let w = world();
        let warrior = w.units.iter().find(|u| u.id == 3).unwrap();
        assert_eq!(
            status(warrior),
            ["Fortified", "Garrisoned", "Level 2, 30 XP"]
        );
        assert_eq!(moves(90), "1.5");
        assert_eq!(moves(120), "2");
    }

    #[test]
    fn plain_actions_map_to_commands() {
        assert_eq!(command_for(3, 7), Some(Command::UnitFoundCity { unit: 7 }));
        assert_eq!(
            command_for(9, 7),
            Some(Command::UnitAutomateExplore { unit: 7 })
        );
        assert_eq!(command_for(ACTION_RANGE_ATTACK, 7), None);
        assert_eq!(command_for(ACTION_MOVE, 7), None);
        assert!(action_info(ACTION_MOVE).is_none());
        for a in 2..=12 {
            assert!(action_info(a).is_some(), "{a}");
        }
    }
}
