//! Journal entries from core events: notifications, auto-choices and
//! auto-declines with their SIMP number, info popups, wars, Lua errors, and the core's own message
//! log (`GC_EVENT_MESSAGE`: combat results and info lines).

use std::collections::BTreeMap;

use gamecore::{AutoChoiceKind, GameEvent, MessageType};
use protocol::{JournalEntry, JournalKind, PlayerFlags, PlayerId, PlayerView};

use crate::core::EventTarget;

/// How many of the newest journal entries the host remembers and saves with the autosave (the
/// client shows 300 lines, `JOURNAL_LINES` in its HUD).
pub const KEPT: usize = 300;

/// Appends `new` to `history` and drops the oldest entries beyond [`KEPT`].
pub fn remember(history: &mut Vec<JournalEntry>, new: &[JournalEntry]) {
    history.extend_from_slice(new);
    let extra = history.len().saturating_sub(KEPT);
    history.drain(..extra);
}

/// A line of the worker itself (game-host-mp 4.10): a seat that left or took a slot, a peace offer made,
/// declined or expired, a save that failed. The lines of the core are passed on as they are.
pub fn worker_entry(turn: i32, kind: JournalKind, text: impl Into<String>) -> JournalEntry {
    JournalEntry {
        turn,
        kind,
        text: text.into(),
        at: None,
        simplification: None,
    }
}

/// A `System` line of the worker: a lost save, a game continued from a save, an offer that expired.
pub fn system_entry(turn: i32, text: impl Into<String>) -> JournalEntry {
    worker_entry(turn, JournalKind::System, text)
}

/// Journal lines on their way to the seats, by seat (game-host-mp 4.10, GH-R6): an event of the core is
/// for one player or for all, and only human seats have a journal.
#[derive(Debug, Default)]
pub struct Pending {
    lines: BTreeMap<PlayerId, Vec<JournalEntry>>,
}

impl Pending {
    /// Files `entry` into the journal of its addressee among `seats`; the text and the cell stay as the
    /// core wrote them.
    pub fn route(
        &mut self,
        to: EventTarget,
        seats: impl IntoIterator<Item = PlayerId>,
        entry: &JournalEntry,
    ) {
        for p in seats {
            if to == EventTarget::All || to == EventTarget::Player(p) {
                self.lines.entry(p).or_default().push(entry.clone());
            }
        }
    }

    /// Takes everything filed so far.
    pub fn take(&mut self) -> BTreeMap<PlayerId, Vec<JournalEntry>> {
        std::mem::take(&mut self.lines)
    }
}

/// SIMP number of auto-choices for the human.
pub const SIMP_AUTO_CHOICE: u16 = 2;
/// SIMP number of diplomacy offers declined for the human.
pub const SIMP_DIPLO_AUTO_DECLINE: u16 = 3;

/// The register entry of an auto-choice kind. Every kind of `AUTO_CHOICE` (0-18) is the AI's
/// decision for the human (SIMP-002); the match is exhaustive so that a new kind is placed here
/// on purpose, not by default.
pub fn auto_choice_simp(kind: AutoChoiceKind) -> u16 {
    use AutoChoiceKind as K;
    match kind {
        // SIMPLIFIED(SIMP-002): research, policies, ideology, religion, promotions, great people,
        // captured cities, World Congress, archaeology, ruins and stolen techs are chosen by the AI
        // for the human; original: the player picks each in its popup.
        K::Research
        | K::FreeTech
        | K::Policy
        | K::PolicyBranch
        | K::Ideology
        | K::Pantheon
        | K::Religion
        | K::Enhancement
        | K::Reformation
        | K::Promotion
        | K::FreeGreatPerson
        | K::FaithGreatPerson
        | K::MayaGreatPerson
        | K::CapturedCity
        | K::LeagueProposals
        | K::LeagueVotes
        | K::Archaeology
        | K::GoodyHut
        | K::TechSteal
        // A kind appended to the ABI after this host was built is still an auto-choice.
        | K::Unknown(_) => SIMP_AUTO_CHOICE,
    }
}

fn at(x: i32, y: i32) -> Option<(i16, i16)> {
    Some((i16::try_from(x).ok()?, i16::try_from(y).ok()?)).filter(|&(x, y)| x >= 0 && y >= 0)
}

/// The journal of a batch of events. `turn` is the game turn when the batch began; a
/// `TurnStarted` inside it moves the later entries to the new turn. War and peace are journaled
/// only when `human_team` is one side: the core reports every team (barbarians, city-states,
/// players the human has not met), and the others are news the player would not have. The other
/// side is named by its civilization from `players`; the war with the barbarians is always on and
/// is not journaled.
///
/// The portable core sends its combat lines as `COMBAT_MESSAGE` with the plot of the fight
/// (Lekmod HYG-0012; golden passes the text only). A message without a plot takes `fight_at`,
/// the plot the human attacked since the last batch (when it was exactly one), as the place for
/// the camera click: the batch of an attack order holds that attack's lines.
pub fn entries(
    events: &[GameEvent],
    mut turn: i32,
    human_team: Option<i32>,
    players: &[PlayerView],
    fight_at: Option<(i16, i16)>,
) -> Vec<JournalEntry> {
    let mut out = Vec::new();
    for ev in events {
        match ev {
            GameEvent::TurnStarted { turn: t } => turn = *t,
            GameEvent::WarState {
                team_a,
                team_b,
                at_war,
            } => {
                let Some(h) = human_team.filter(|h| h == team_a || h == team_b) else {
                    continue;
                };
                let other = if *team_a == h { *team_b } else { *team_a };
                let Some(name) = team_name(players, other) else {
                    continue;
                };
                out.push(JournalEntry {
                    turn,
                    kind: JournalKind::War,
                    text: if *at_war {
                        format!("You are at war with {name}")
                    } else {
                        format!("You made peace with {name}")
                    },
                    at: None,
                    simplification: None,
                });
                continue;
            }
            _ => {}
        }
        out.extend(entry(ev, turn).map(|mut e| {
            if matches!(ev, GameEvent::Message { .. }) && e.at.is_none() {
                e.at = fight_at;
            }
            e
        }));
    }
    out
}

/// The civilization of `team` for the journal; `None` for the barbarians. A team without a known
/// player stays a number.
fn team_name(players: &[PlayerView], team: i32) -> Option<String> {
    let mut members = players.iter().filter(|p| i32::from(p.team) == team);
    match members.next() {
        Some(p) if p.flags.contains(PlayerFlags::BARBARIAN) => None,
        Some(p) if !p.civ_name.is_empty() => Some(p.civ_name.clone()),
        _ => Some(format!("team {team}")),
    }
}

/// The entry for an event, or `None` for events that are not for the player's journal.
pub fn entry(ev: &GameEvent, turn: i32) -> Option<JournalEntry> {
    let (kind, text, at, simplification) = match ev {
        GameEvent::NotificationAdded {
            x,
            y,
            message,
            summary,
            ..
        } => (
            JournalKind::Notification,
            if message.is_empty() { summary } else { message }.clone(),
            at(*x, *y),
            None,
        ),
        GameEvent::AutoChoice { what, .. } => (
            JournalKind::AutoChoice,
            what.clone(),
            None,
            ev.auto_choice_kind().map(auto_choice_simp),
        ),
        GameEvent::DiploAutoDeclined { text, .. } => (
            JournalKind::DiploAutoDeclined,
            text.clone(),
            None,
            Some(SIMP_DIPLO_AUTO_DECLINE),
        ),
        GameEvent::PopupInfo { text, .. } => (JournalKind::Info, text.clone(), None, None),
        // The core's message log: the fights, and the info lines of its own UI. Flying text
        // (DISPLAY_ONLY) and chat are not log lines.
        GameEvent::Message { at, kind, text, .. } => (
            match kind {
                MessageType::Combat => JournalKind::Combat,
                MessageType::Info
                | MessageType::MajorEvent
                | MessageType::MinorEvent
                | MessageType::Quest => JournalKind::Info,
                MessageType::DisplayOnly | MessageType::Chat => return None,
            },
            text.clone(),
            *at,
            None,
        ),
        // War and peace need the players to name the sides: [`entries`].
        GameEvent::LuaError { message } => (JournalKind::LuaError, message.clone(), None, None),
        _ => return None,
    };
    if text.is_empty() {
        return None;
    }
    Some(JournalEntry {
        turn,
        kind,
        text,
        at,
        simplification,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_the_journal_events_and_skips_the_rest() {
        let n = entry(
            &GameEvent::NotificationAdded {
                id: 1,
                kind: 2,
                x: 5,
                y: -1,
                data1: 0,
                data2: 0,
                player: 0,
                message: String::new(),
                summary: "Summary".into(),
            },
            7,
        )
        .unwrap();
        assert_eq!(
            (n.turn, n.kind, n.text.as_str(), n.at),
            (7, JournalKind::Notification, "Summary", None)
        );
        let a = entry(
            &GameEvent::AutoChoice {
                choice_kind: 1,
                player: 0,
                item: 3,
                what: "Research Pottery".into(),
            },
            7,
        )
        .unwrap();
        assert_eq!(a.simplification, Some(SIMP_AUTO_CHOICE));
        let d = entry(
            &GameEvent::DiploAutoDeclined {
                ai_player: 1,
                kind: 0,
                human: 0,
                text: "No deal".into(),
            },
            7,
        )
        .unwrap();
        assert_eq!(d.simplification, Some(SIMP_DIPLO_AUTO_DECLINE));
        let located = entry(
            &GameEvent::NotificationAdded {
                id: 1,
                kind: 2,
                x: 5,
                y: 6,
                data1: 0,
                data2: 0,
                player: 0,
                message: "Hi".into(),
                summary: String::new(),
            },
            1,
        )
        .unwrap();
        assert_eq!(located.at, Some((5, 6)));
        // Every auto-choice kind of the ABI (0-18), and one appended later, carries its SIMP.
        for kind in 0..=19 {
            let e = entry(
                &GameEvent::AutoChoice {
                    choice_kind: kind,
                    player: 0,
                    item: 1,
                    what: "Chosen".into(),
                },
                2,
            )
            .unwrap();
            assert_eq!(
                (e.kind, e.simplification),
                (JournalKind::AutoChoice, Some(SIMP_AUTO_CHOICE)),
                "kind {kind}"
            );
        }
        assert!(entry(&GameEvent::TurnStarted { turn: 3 }, 3).is_none());
        assert!(entry(&GameEvent::NotificationRemoved { id: 1, player: 0 }, 3).is_none());
        assert!(
            entry(
                &GameEvent::LuaError {
                    message: String::new()
                },
                3
            )
            .is_none()
        );
    }

    #[test]
    fn a_turn_start_inside_the_batch_dates_the_later_entries() {
        let lua = |m: &str| GameEvent::LuaError { message: m.into() };
        let war = |a, b| GameEvent::WarState {
            team_a: a,
            team_b: b,
            at_war: true,
        };
        let player = |id: i8, team: i8, civ: &str, flags: u16| PlayerView {
            id,
            team,
            civ_name: civ.into(),
            flags: PlayerFlags(flags),
            ..PlayerView::default()
        };
        let players = [
            player(0, 0, "Rome", PlayerFlags::ALIVE),
            player(1, 1, "Egypt", PlayerFlags::ALIVE),
            player(
                63,
                63,
                "Barbarians",
                PlayerFlags::ALIVE | PlayerFlags::BARBARIAN,
            ),
        ];
        let peace = GameEvent::WarState {
            team_a: 1,
            team_b: 0,
            at_war: false,
        };
        let got = entries(
            &[
                lua("a"),
                GameEvent::TurnStarted { turn: 8 },
                lua("b"),
                war(60, 63),
                war(0, 63),
                war(0, 1),
                war(2, 0),
                peace,
            ],
            7,
            Some(0),
            &players,
            None,
        );
        let turns: Vec<_> = got
            .iter()
            .map(|e| (e.turn, e.kind, e.text.as_str()))
            .collect();
        let war_kind = JournalKind::War;
        assert_eq!(
            turns,
            [
                (7, JournalKind::LuaError, "a"),
                (8, JournalKind::LuaError, "b"),
                (8, war_kind, "You are at war with Egypt"),
                (8, war_kind, "You are at war with team 2"),
                (8, war_kind, "You made peace with Egypt")
            ],
            "teams by civilization, the barbarians left out"
        );
        assert!(entries(&[war(0, 1)], 1, None, &players, None).is_empty());
        assert!(entry(&war(0, 1), 1).is_none(), "war needs the players");
    }

    fn message(kind: MessageType, at: Option<(i16, i16)>, text: &str) -> GameEvent {
        GameEvent::Message {
            player: 0,
            at,
            kind,
            source: gamecore::MessageSource::Message,
            force: false,
            text: text.into(),
        }
    }

    #[test]
    fn core_messages_map_to_journal_kinds() {
        use MessageType as M;
        let kinds = |t| entry(&message(t, Some((3, 4)), "Text"), 5).map(|e| (e.kind, e.at, e.turn));
        assert_eq!(
            kinds(M::Combat),
            Some((JournalKind::Combat, Some((3, 4)), 5))
        );
        for t in [M::Info, M::MajorEvent, M::MinorEvent, M::Quest] {
            assert_eq!(
                kinds(t),
                Some((JournalKind::Info, Some((3, 4)), 5)),
                "{t:?}"
            );
        }
        // Flying text and chat are not log lines.
        assert_eq!(kinds(M::DisplayOnly), None);
        assert_eq!(kinds(M::Chat), None);
        // Markup stays (the client strips it, like for notifications); an empty text is nothing.
        let raw = "[COLOR_RED]Your Archer[ENDCOLOR] hit";
        assert_eq!(entry(&message(M::Combat, None, raw), 1).unwrap().text, raw);
        assert!(entry(&message(M::Combat, None, ""), 1).is_none());
    }

    #[test]
    fn messages_without_a_plot_take_the_plot_the_human_attacked() {
        use MessageType as M;
        let events = [
            message(M::Combat, None, "no plot"),
            message(M::Combat, Some((1, 2)), "own plot"),
            message(M::Info, None, "info"),
            GameEvent::LuaError {
                message: "lua".into(),
            },
            GameEvent::TurnStarted { turn: 9 },
            message(M::Combat, None, "later"),
        ];
        let lines = |fight_at| {
            entries(&events, 8, Some(0), &[], fight_at)
                .into_iter()
                .map(|e| (e.turn, e.kind, e.text, e.at))
                .collect::<Vec<_>>()
        };
        let c = JournalKind::Combat;
        assert_eq!(
            lines(Some((7, 7))),
            [
                (8, c, "no plot".into(), Some((7, 7))),
                (8, c, "own plot".into(), Some((1, 2))),
                (8, JournalKind::Info, "info".into(), Some((7, 7))),
                (8, JournalKind::LuaError, "lua".into(), None),
                (9, c, "later".into(), Some((7, 7))),
            ]
        );
        assert_eq!(lines(None)[0].3, None, "none known, none given");
    }

    #[test]
    fn history_keeps_the_newest_entries() {
        let e = |turn| JournalEntry {
            turn,
            kind: JournalKind::Notification,
            text: String::new(),
            at: None,
            simplification: None,
        };
        let mut h = Vec::new();
        remember(&mut h, &(0..KEPT as i32 - 1).map(e).collect::<Vec<_>>());
        remember(&mut h, &[e(1000), e(1001), e(1002)]);
        assert_eq!(h.len(), KEPT);
        assert_eq!((h[0].turn, h[KEPT - 1].turn), (2, 1002));
    }
}
