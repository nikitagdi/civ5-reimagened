//! Personalities of the autoplay bot: how a bot behaves besides the shared
//! policy. A persona adds acts before the policy plays the turn, or replaces the policy (`idle`).

use std::collections::BTreeSet;

use protocol::{ChatSend, ChatTo, Command, PlayerFlags, PlayerId, TurnState, WorldView};
use serde_json::Value;

/// What a persona does in a turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    Cmd(Command),
    Chat(ChatSend),
    /// Many orders at once, without waiting (it breaks the limits of the gateway on purpose).
    Burst(Vec<Command>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Persona {
    /// The shared policy and nothing else.
    Autoplay,
    /// Ends the turn at once, every turn.
    Idle,
    /// From `war_turn` declares war on the first human it has met (for the hybrid turn mode). It does not
    /// lead its army to the enemy (a simplification of the stand, not of the game).
    Warmonger { war_turn: i32, declared: bool },
    /// Answers every peace offer with yes and offers peace to every human it is at war with.
    Peaceful {
        answered: BTreeSet<(PlayerId, i32)>,
        offered: BTreeSet<(PlayerId, i32)>,
    },
    /// A chat line every `every` turns.
    Chatty { every: i32, last: i32 },
    /// Breaks the order limits of the gateway: `per_turn` orders in a burst at the start of every turn.
    Spammer { per_turn: u32, sent_turn: i32 },
}

impl Persona {
    /// `autoplay`, `idle`, `warmonger`, `peaceful`, `chatty`, `spammer`, with the numbers of `args`
    /// (`war_turn`, `every`, `per_turn`).
    pub fn parse(name: &str, args: &Value) -> Result<Persona, String> {
        let int = |k: &str, d: i64| args.get(k).and_then(Value::as_i64).unwrap_or(d);
        Ok(match name {
            "autoplay" => Persona::Autoplay,
            "idle" => Persona::Idle,
            "warmonger" => Persona::Warmonger {
                war_turn: int("war_turn", 5) as i32,
                declared: false,
            },
            "peaceful" => Persona::Peaceful {
                answered: BTreeSet::new(),
                offered: BTreeSet::new(),
            },
            "chatty" => Persona::Chatty {
                every: int("every", 3) as i32,
                last: 0,
            },
            "spammer" => Persona::Spammer {
                per_turn: int("per_turn", 100) as u32,
                sent_turn: 0,
            },
            other => return Err(format!("unknown persona {other}")),
        })
    }

    pub fn is_idle(&self) -> bool {
        matches!(self, Persona::Idle)
    }

    /// The next act of the persona in this turn, if any; the persona remembers it was done.
    pub fn plan(&mut self, view: &WorldView, ts: &TurnState, nick: &str) -> Option<Act> {
        let me = view.human;
        let turn = ts.turn;
        match self {
            Persona::Autoplay | Persona::Idle => None,
            Persona::Warmonger { war_turn, declared } => {
                if *declared || turn < *war_turn {
                    return None;
                }
                let target = view.players.iter().find(|p| {
                    p.id != me
                        && p.flags.contains(PlayerFlags::HUMAN)
                        && p.flags.contains(PlayerFlags::MET)
                        && p.flags.contains(PlayerFlags::ALIVE)
                        && !p.flags.contains(PlayerFlags::AT_WAR_WITH_VIEWER)
                })?;
                *declared = true;
                Some(Act::Cmd(Command::DeclareWar { team: target.team }))
            }
            Persona::Peaceful { answered, offered } => {
                for o in &view.peace_offers {
                    if o.to == me && answered.insert((o.from, turn)) {
                        return Some(Act::Cmd(Command::AnswerPeace {
                            from: o.from,
                            accept: true,
                        }));
                    }
                }
                let foe = view.players.iter().find(|p| {
                    p.id != me
                        && p.flags.contains(PlayerFlags::HUMAN)
                        && p.flags.contains(PlayerFlags::AT_WAR_WITH_VIEWER)
                        && !offered.contains(&(p.id, turn))
                })?;
                offered.insert((foe.id, turn));
                Some(Act::Cmd(Command::ProposePeace { to: foe.id }))
            }
            Persona::Chatty { every, last } => {
                if *every <= 0 || turn % *every != 0 || *last == turn {
                    return None;
                }
                *last = turn;
                Some(Act::Chat(ChatSend {
                    to: ChatTo::All,
                    text: format!("{nick}: turn {turn}"),
                }))
            }
            Persona::Spammer {
                per_turn,
                sent_turn,
            } => {
                if *sent_turn == turn {
                    return None;
                }
                *sent_turn = turn;
                let unit = view
                    .units
                    .iter()
                    .find(|u| u.owner == me)
                    .map_or(0, |u| u.id);
                Some(Act::Burst(
                    (0..*per_turn).map(|_| Command::UnitSkip { unit }).collect(),
                ))
            }
        }
    }
}
