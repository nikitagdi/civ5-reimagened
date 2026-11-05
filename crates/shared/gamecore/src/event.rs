//! Game events reported through `gc_host::game_event`.

use gamecore_sys::{RawEvent, ffi};

/// `choice_kind` of `GC_EVENT_AUTO_CHOICE` (SIMP-002; the ABI appends kinds only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoChoiceKind {
    /// 0: research. Item = `TechTypes`.
    Research,
    /// 1: free tech. Item = `TechTypes`.
    FreeTech,
    /// 2: policy or tenet. Item = `PolicyTypes`.
    Policy,
    /// 3: policy branch. Item = `PolicyBranchTypes`.
    PolicyBranch,
    /// 4: ideology. Item = its branch.
    Ideology,
    /// 5: pantheon. Item = `BeliefTypes`.
    Pantheon,
    /// 6: religion founded. Item = `ReligionTypes`.
    Religion,
    /// 7: religion enhanced. Item = `ReligionTypes`.
    Enhancement,
    /// 8: reformation belief. Item = `BeliefTypes`.
    Reformation,
    /// 9: promotion. Item = `PromotionTypes`.
    Promotion,
    /// 10: free great person. Item = `UnitTypes`.
    FreeGreatPerson,
    /// 11: great person bought with faith. Item = `UnitTypes`.
    FaithGreatPerson,
    /// 12: Maya Long Count great person. Item = `UnitTypes`.
    MayaGreatPerson,
    /// 13: captured city (annexed / puppeted / razed). Item = the city id.
    CapturedCity,
    /// 14: World Congress proposals made. Item = the id of the first new proposal.
    LeagueProposals,
    /// 15: World Congress votes. Item = the number of delegates cast.
    LeagueVotes,
    /// 16: archaeology (finished dig). Item = `ArchaeologyChoiceType`.
    Archaeology,
    /// 17: ruin reward (goody hut). Item = `GoodyTypes`.
    GoodyHut,
    /// 18: tech stolen by a spy. Item = `TechTypes`.
    TechSteal,
    /// A kind this host does not know.
    Unknown(i32),
}

impl AutoChoiceKind {
    /// Types a `choice_kind`; never fails.
    pub fn from_abi(v: i32) -> Self {
        match v {
            0 => AutoChoiceKind::Research,
            1 => AutoChoiceKind::FreeTech,
            2 => AutoChoiceKind::Policy,
            3 => AutoChoiceKind::PolicyBranch,
            4 => AutoChoiceKind::Ideology,
            5 => AutoChoiceKind::Pantheon,
            6 => AutoChoiceKind::Religion,
            7 => AutoChoiceKind::Enhancement,
            8 => AutoChoiceKind::Reformation,
            9 => AutoChoiceKind::Promotion,
            10 => AutoChoiceKind::FreeGreatPerson,
            11 => AutoChoiceKind::FaithGreatPerson,
            12 => AutoChoiceKind::MayaGreatPerson,
            13 => AutoChoiceKind::CapturedCity,
            14 => AutoChoiceKind::LeagueProposals,
            15 => AutoChoiceKind::LeagueVotes,
            16 => AutoChoiceKind::Archaeology,
            17 => AutoChoiceKind::GoodyHut,
            18 => AutoChoiceKind::TechSteal,
            other => AutoChoiceKind::Unknown(other),
        }
    }

    /// The `choice_kind` number.
    pub fn abi(self) -> i32 {
        match self {
            AutoChoiceKind::Research => 0,
            AutoChoiceKind::FreeTech => 1,
            AutoChoiceKind::Policy => 2,
            AutoChoiceKind::PolicyBranch => 3,
            AutoChoiceKind::Ideology => 4,
            AutoChoiceKind::Pantheon => 5,
            AutoChoiceKind::Religion => 6,
            AutoChoiceKind::Enhancement => 7,
            AutoChoiceKind::Reformation => 8,
            AutoChoiceKind::Promotion => 9,
            AutoChoiceKind::FreeGreatPerson => 10,
            AutoChoiceKind::FaithGreatPerson => 11,
            AutoChoiceKind::MayaGreatPerson => 12,
            AutoChoiceKind::CapturedCity => 13,
            AutoChoiceKind::LeagueProposals => 14,
            AutoChoiceKind::LeagueVotes => 15,
            AutoChoiceKind::Archaeology => 16,
            AutoChoiceKind::GoodyHut => 17,
            AutoChoiceKind::TechSteal => 18,
            AutoChoiceKind::Unknown(v) => v,
        }
    }
}

/// `message_type` of `GC_EVENT_MESSAGE`: the engine's `InterfaceMessageTypes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
    /// 0: `INTERFACEMESSAGE_TYPE_INFO`.
    Info,
    /// 1: `..._DISPLAY_ONLY` (flying text, not for the log).
    DisplayOnly,
    /// 2: `..._MAJOR_EVENT`.
    MajorEvent,
    /// 3: `..._MINOR_EVENT`.
    MinorEvent,
    /// 4: `..._CHAT`.
    Chat,
    /// 5: `..._COMBAT_MESSAGE`.
    Combat,
    /// 6: `..._QUEST`.
    Quest,
}

impl MessageType {
    /// Types a `message_type`; `None` for a number the ABI does not have.
    pub fn from_abi(v: i32) -> Option<Self> {
        Some(match v {
            0 => MessageType::Info,
            1 => MessageType::DisplayOnly,
            2 => MessageType::MajorEvent,
            3 => MessageType::MinorEvent,
            4 => MessageType::Chat,
            5 => MessageType::Combat,
            6 => MessageType::Quest,
            _ => return None,
        })
    }
}

/// Which `CvDLLInterface` call produced a `GC_EVENT_MESSAGE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageSource {
    /// 0: `AddMessage`.
    Message,
    /// 1: `AddUnitMessage`.
    Unit,
    /// 2: `AddPlotMessage`.
    Plot,
}

impl MessageSource {
    /// Types a `source`; `None` for a number the ABI does not have.
    pub fn from_abi(v: i32) -> Option<Self> {
        Some(match v {
            0 => MessageSource::Message,
            1 => MessageSource::Unit,
            2 => MessageSource::Plot,
            _ => return None,
        })
    }
}

/// Game event. New variants go at the end.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum GameEvent {
    /// `GC_EVENT_TURN_STARTED`: every game-turn change.
    TurnStarted { turn: i32 },
    /// `GC_EVENT_PLAYER_TURN_STARTED`.
    PlayerTurnStarted { player: i32 },
    /// `GC_EVENT_NOTIFICATION_ADDED`.
    NotificationAdded {
        id: i32,
        kind: i32,
        x: i32,
        y: i32,
        data1: i32,
        data2: i32,
        /// The human the notification is for (v2).
        player: i32,
        message: String,
        summary: String,
    },
    /// `GC_EVENT_NOTIFICATION_REMOVED`; `player` is the human it was for (v2).
    NotificationRemoved { id: i32, player: i32 },
    /// `GC_EVENT_AUTO_CHOICE`: what the core chose for the player (English text, SIMP-002).
    /// `choice_kind` is the raw number; [`GameEvent::auto_choice_kind`] types it.
    AutoChoice {
        choice_kind: i32,
        player: i32,
        item: i32,
        what: String,
    },
    /// `GC_EVENT_DIPLO_AUTO_DECLINED` (SIMP-003).
    DiploAutoDeclined {
        ai_player: i32,
        kind: i32,
        /// The human who was offered (v2).
        human: i32,
        text: String,
    },
    /// `GC_EVENT_POPUP_INFO`: ruins, natural wonder, golden age ...
    PopupInfo {
        popup_type: i32,
        data1: i32,
        data2: i32,
        data3: i32,
        /// The active player at the moment of the popup (v2).
        player: i32,
        text: String,
    },
    /// `GC_EVENT_GAME_OVER`: once.
    GameOver { winner_team: i32, victory_type: i32 },
    /// `GC_EVENT_LUA_ERROR`: message and place.
    LuaError { message: String },
    /// `GC_EVENT_WAR_STATE`.
    WarState {
        team_a: i32,
        team_b: i32,
        at_war: bool,
    },
    /// `GC_EVENT_MESSAGE`: a line of the core's own message log (combat results, info) for the
    /// human; `text` keeps the game's markup. `at` is `None` when the core had no plot (x or y
    /// below 0, or outside `i16`).
    Message {
        player: i32,
        at: Option<(i16, i16)>,
        kind: MessageType,
        source: MessageSource,
        force: bool,
        text: String,
    },
    /// `GC_EVENT_PLAYER_TURN_ENDED` (v2): the player sent the end of turn (`ended`) or took it
    /// back.
    PlayerTurnEnded { player: i32, ended: bool },
    /// `GC_EVENT_SLOT_CHANGED` (v2): a slot changed hands or lost or regained its connection.
    /// `slot_status` is the core's `SlotStatus` (see `Player::slot`).
    SlotChanged {
        player: i32,
        slot_status: i32,
        connected: bool,
        kicked: bool,
        /// The nick the slot has now; `None` when it has none.
        nick: Option<String>,
    },
    /// `GC_EVENT_HUMAN_PHASE_STARTED` (v2): the AI civs are done, the humans may act (the
    /// autosave point of the turn start).
    HumanPhaseStarted { turn: i32 },
    /// `GC_EVENT_HUMAN_WAR_DECLARED` (v2): a war between two teams with humans. `extends_timer`:
    /// a side of the declaration moves simultaneously now.
    HumanWarDeclared {
        from_team: i32,
        to_team: i32,
        extends_timer: bool,
    },
    /// `GC_EVENT_PLAYER_TURN_STATUS` (v2 addition): a player's `setTurnActive`; `started`: it became active, otherwise it
    /// became inactive. Sent for every player, AI and minors included; `human` as the core saw the player then.
    PlayerTurnStatus {
        started: bool,
        player: i32,
        human: bool,
    },
    /// An event this host does not know, or a known kind with an unexpected argument count.
    Unknown {
        kind: i32,
        args: Vec<i32>,
        text: Option<String>,
        text2: Option<String>,
    },
}

/// A plot from the event's `x`, `y`; `None` when either is negative (no plot) or too big.
fn plot_of(x: i32, y: i32) -> Option<(i16, i16)> {
    Some((i16::try_from(x).ok()?, i16::try_from(y).ok()?)).filter(|&(x, y)| x >= 0 && y >= 0)
}

impl GameEvent {
    /// The typed `choice_kind` of an `AutoChoice` event.
    pub fn auto_choice_kind(&self) -> Option<AutoChoiceKind> {
        match self {
            GameEvent::AutoChoice { choice_kind, .. } => {
                Some(AutoChoiceKind::from_abi(*choice_kind))
            }
            _ => None,
        }
    }

    /// Types a `gc_event`. Never fails: a kind or shape it does not know is `Unknown`.
    pub fn from_raw(ev: &RawEvent<'_>) -> GameEvent {
        let text = || ev.text.unwrap_or_default().to_owned();
        let text2 = || ev.text2.unwrap_or_default().to_owned();
        match (ev.kind, ev.args) {
            (ffi::GC_EVENT_TURN_STARTED, &[turn]) => GameEvent::TurnStarted { turn },
            (ffi::GC_EVENT_PLAYER_TURN_STARTED, &[player]) => {
                GameEvent::PlayerTurnStarted { player }
            }
            (ffi::GC_EVENT_NOTIFICATION_ADDED, &[id, kind, x, y, data1, data2, player]) => {
                GameEvent::NotificationAdded {
                    id,
                    kind,
                    x,
                    y,
                    data1,
                    data2,
                    player,
                    message: text(),
                    summary: text2(),
                }
            }
            (ffi::GC_EVENT_NOTIFICATION_REMOVED, &[id, player]) => {
                GameEvent::NotificationRemoved { id, player }
            }
            (ffi::GC_EVENT_AUTO_CHOICE, &[choice_kind, player, item]) => GameEvent::AutoChoice {
                choice_kind,
                player,
                item,
                what: text(),
            },
            (ffi::GC_EVENT_DIPLO_AUTO_DECLINED, &[ai_player, kind, human]) => {
                GameEvent::DiploAutoDeclined {
                    ai_player,
                    kind,
                    human,
                    text: text(),
                }
            }
            (ffi::GC_EVENT_POPUP_INFO, &[popup_type, data1, data2, data3, player]) => {
                GameEvent::PopupInfo {
                    popup_type,
                    data1,
                    data2,
                    data3,
                    player,
                    text: text(),
                }
            }
            (ffi::GC_EVENT_GAME_OVER, &[winner_team, victory_type]) => GameEvent::GameOver {
                winner_team,
                victory_type,
            },
            (ffi::GC_EVENT_LUA_ERROR, _) if ev.args.is_empty() => {
                GameEvent::LuaError { message: text() }
            }
            (ffi::GC_EVENT_WAR_STATE, &[team_a, team_b, at_war]) => GameEvent::WarState {
                team_a,
                team_b,
                at_war: at_war != 0,
            },
            (ffi::GC_EVENT_PLAYER_TURN_ENDED, &[player, ended]) if matches!(ended, 0 | 1) => {
                GameEvent::PlayerTurnEnded {
                    player,
                    ended: ended == 1,
                }
            }
            (ffi::GC_EVENT_SLOT_CHANGED, &[player, slot_status, connected, kicked]) => {
                GameEvent::SlotChanged {
                    player,
                    slot_status,
                    connected: connected != 0,
                    kicked: kicked != 0,
                    nick: ev.text.filter(|t| !t.is_empty()).map(str::to_owned),
                }
            }
            (ffi::GC_EVENT_HUMAN_PHASE_STARTED, &[turn]) => GameEvent::HumanPhaseStarted { turn },
            (ffi::GC_EVENT_HUMAN_WAR_DECLARED, &[from_team, to_team, extends_timer]) => {
                GameEvent::HumanWarDeclared {
                    from_team,
                    to_team,
                    extends_timer: extends_timer != 0,
                }
            }
            (ffi::GC_EVENT_PLAYER_TURN_STATUS, &[status, player, human])
                if matches!(status, 0 | 1) && matches!(human, 0 | 1) =>
            {
                GameEvent::PlayerTurnStatus {
                    started: status == 0,
                    player,
                    human: human == 1,
                }
            }
            // Strict: a number outside the ABI's tables or a flag other than 0 / 1 is not typed.
            (ffi::GC_EVENT_MESSAGE, &[player, x, y, kind, source, force])
                if matches!(force, 0 | 1)
                    && let (Some(kind), Some(source)) =
                        (MessageType::from_abi(kind), MessageSource::from_abi(source)) =>
            {
                GameEvent::Message {
                    player,
                    at: plot_of(x, y),
                    kind,
                    source,
                    force: force == 1,
                    text: text(),
                }
            }
            _ => GameEvent::Unknown {
                kind: ev.kind,
                args: ev.args.to_vec(),
                text: ev.text.map(str::to_owned),
                text2: ev.text2.map(str::to_owned),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev<'a>(kind: i32, args: &'a [i32], text: Option<&'a str>) -> RawEvent<'a> {
        RawEvent {
            kind,
            args,
            text,
            text2: None,
        }
    }

    #[test]
    fn known_events_are_typed() {
        assert_eq!(
            GameEvent::from_raw(&ev(1, &[5], None)),
            GameEvent::TurnStarted { turn: 5 }
        );
        assert_eq!(
            GameEvent::from_raw(&ev(2, &[0], None)),
            GameEvent::PlayerTurnStarted { player: 0 }
        );
        assert_eq!(
            GameEvent::from_raw(&RawEvent {
                kind: 3,
                args: &[1, 2, 3, 4, 5, 6, 7],
                text: Some("msg"),
                text2: Some("sum"),
            }),
            GameEvent::NotificationAdded {
                id: 1,
                kind: 2,
                x: 3,
                y: 4,
                data1: 5,
                data2: 6,
                player: 7,
                message: "msg".into(),
                summary: "sum".into()
            }
        );
        assert_eq!(
            GameEvent::from_raw(&ev(10, &[0, 1, 1], None)),
            GameEvent::WarState {
                team_a: 0,
                team_b: 1,
                at_war: true
            }
        );
        assert_eq!(
            GameEvent::from_raw(&ev(9, &[], Some("boom"))),
            GameEvent::LuaError {
                message: "boom".into()
            }
        );
        assert_eq!(
            GameEvent::from_raw(&ev(8, &[1, 2], None)),
            GameEvent::GameOver {
                winner_team: 1,
                victory_type: 2
            }
        );
    }

    #[test]
    fn every_event_kind_is_typed() {
        let t = |kind: i32, args: &[i32]| GameEvent::from_raw(&ev(kind, args, Some("x")));
        assert!(matches!(
            t(4, &[9, 1]),
            GameEvent::NotificationRemoved { id: 9, player: 1 }
        ));
        assert!(matches!(
            t(6, &[3, 1, 0]),
            GameEvent::DiploAutoDeclined {
                ai_player: 3,
                kind: 1,
                human: 0,
                ..
            }
        ));
        assert_eq!(
            t(7, &[1, 2, 3, 4, 5]),
            GameEvent::PopupInfo {
                popup_type: 1,
                data1: 2,
                data2: 3,
                data3: 4,
                player: 5,
                text: "x".into()
            }
        );
        assert_eq!(
            t(10, &[1, 2, 0]),
            GameEvent::WarState {
                team_a: 1,
                team_b: 2,
                at_war: false
            }
        );
        // A Lua error carries no arguments; with any it is not that event.
        assert!(matches!(t(9, &[1]), GameEvent::Unknown { kind: 9, .. }));
        // Wrong argument count for each kind with fixed arity.
        for (kind, n) in [
            (2, 1usize),
            (3, 7),
            (4, 2),
            (5, 3),
            (6, 3),
            (7, 5),
            (8, 2),
            (10, 3),
            (11, 6),
            (12, 2),
            (13, 4),
            (14, 1),
            (15, 3),
            (16, 3),
        ] {
            let ok = vec![0; n];
            assert!(
                !matches!(t(kind, &ok), GameEvent::Unknown { .. }),
                "kind {kind}"
            );
            let bad = vec![0; n + 1];
            assert!(
                matches!(t(kind, &bad), GameEvent::Unknown { .. }),
                "kind {kind}"
            );
        }
    }

    #[test]
    fn slot_and_turn_events_are_typed() {
        assert_eq!(
            GameEvent::from_raw(&ev(12, &[1, 1], None)),
            GameEvent::PlayerTurnEnded {
                player: 1,
                ended: true
            }
        );
        assert_eq!(
            GameEvent::from_raw(&ev(12, &[1, 0], None)),
            GameEvent::PlayerTurnEnded {
                player: 1,
                ended: false
            }
        );
        // `ended` is 0 or 1 only.
        assert!(matches!(
            GameEvent::from_raw(&ev(12, &[1, 2], None)),
            GameEvent::Unknown { kind: 12, .. }
        ));
        assert_eq!(
            GameEvent::from_raw(&ev(13, &[2, 3, 1, 0], Some("friend"))),
            GameEvent::SlotChanged {
                player: 2,
                slot_status: 3,
                connected: true,
                kicked: false,
                nick: Some("friend".into())
            }
        );
        assert_eq!(
            GameEvent::from_raw(&ev(13, &[2, 1, 0, 1], Some(""))),
            GameEvent::SlotChanged {
                player: 2,
                slot_status: 1,
                connected: false,
                kicked: true,
                nick: None
            }
        );
        assert_eq!(
            GameEvent::from_raw(&ev(14, &[12], None)),
            GameEvent::HumanPhaseStarted { turn: 12 }
        );
        assert_eq!(
            GameEvent::from_raw(&ev(15, &[0, 1, 1], None)),
            GameEvent::HumanWarDeclared {
                from_team: 0,
                to_team: 1,
                extends_timer: true
            }
        );
        assert_eq!(
            GameEvent::from_raw(&ev(16, &[1, 7, 0], None)),
            GameEvent::PlayerTurnStatus {
                started: false,
                player: 7,
                human: false
            }
        );
        assert!(matches!(
            GameEvent::from_raw(&ev(16, &[2, 7, 0], None)),
            GameEvent::Unknown { kind: 16, .. }
        ));
    }

    #[test]
    fn message_events_are_typed_strictly() {
        let msg =
            |args: &[i32]| GameEvent::from_raw(&ev(11, args, Some("[COLOR_RED]Hit[ENDCOLOR]")));
        assert_eq!(
            msg(&[0, 7, 9, 5, 1, 1]),
            GameEvent::Message {
                player: 0,
                at: Some((7, 9)),
                kind: MessageType::Combat,
                source: MessageSource::Unit,
                force: true,
                text: "[COLOR_RED]Hit[ENDCOLOR]".into()
            }
        );
        // No plot: -1 on either axis, or a value outside i16.
        for (x, y) in [(-1, -1), (3, -1), (-1, 3), (40_000, 1)] {
            assert!(
                matches!(
                    msg(&[0, x, y, 0, 0, 0]),
                    GameEvent::Message { at: None, .. }
                ),
                "{x},{y}"
            );
        }
        let kinds = [
            MessageType::Info,
            MessageType::DisplayOnly,
            MessageType::MajorEvent,
            MessageType::MinorEvent,
            MessageType::Chat,
            MessageType::Combat,
            MessageType::Quest,
        ];
        for (n, want) in kinds.into_iter().enumerate() {
            assert!(
                matches!(msg(&[0, 1, 1, n as i32, 2, 0]), GameEvent::Message { kind, source: MessageSource::Plot, .. } if kind == want),
                "type {n}"
            );
        }
        // A number outside the tables, or a flag that is not 0 / 1, is not a message.
        for bad in [
            [0, 1, 1, 7, 0, 0],
            [0, 1, 1, -1, 0, 0],
            [0, 1, 1, 0, 3, 0],
            [0, 1, 1, 0, -1, 0],
            [0, 1, 1, 0, 0, 2],
        ] {
            assert!(
                matches!(msg(&bad), GameEvent::Unknown { kind: 11, .. }),
                "{bad:?}"
            );
        }
        assert!(matches!(
            msg(&[0, 1, 1, 0, 0]),
            GameEvent::Unknown { kind: 11, .. }
        ));
        // Empty text stays a message with an empty text.
        assert!(matches!(
            GameEvent::from_raw(&ev(11, &[0, 1, 1, 0, 0, 0], None)),
            GameEvent::Message { text, .. } if text.is_empty()
        ));
    }

    #[test]
    fn auto_choice_kinds_0_to_18_and_unknown() {
        for v in 0..=18 {
            let k = AutoChoiceKind::from_abi(v);
            assert!(!matches!(k, AutoChoiceKind::Unknown(_)), "{v}");
            assert_eq!(k.abi(), v);
        }
        assert_eq!(AutoChoiceKind::from_abi(0), AutoChoiceKind::Research);
        assert_eq!(AutoChoiceKind::from_abi(13), AutoChoiceKind::CapturedCity);
        assert_eq!(AutoChoiceKind::from_abi(18), AutoChoiceKind::TechSteal);
        assert_eq!(AutoChoiceKind::from_abi(19), AutoChoiceKind::Unknown(19));
        assert_eq!(AutoChoiceKind::from_abi(-1).abi(), -1);
        let e = GameEvent::from_raw(&ev(5, &[9, 0, 12], Some("Promoted")));
        assert_eq!(
            e,
            GameEvent::AutoChoice {
                choice_kind: 9,
                player: 0,
                item: 12,
                what: "Promoted".into()
            }
        );
        assert_eq!(e.auto_choice_kind(), Some(AutoChoiceKind::Promotion));
        assert_eq!(GameEvent::TurnStarted { turn: 1 }.auto_choice_kind(), None);
    }

    #[test]
    fn unknown_kind_or_shape_is_unknown() {
        assert_eq!(
            GameEvent::from_raw(&ev(99, &[1], Some("t"))),
            GameEvent::Unknown {
                kind: 99,
                args: vec![1],
                text: Some("t".into()),
                text2: None
            }
        );
        // Known kind, wrong argument count.
        assert!(matches!(
            GameEvent::from_raw(&ev(1, &[1, 2], None)),
            GameEvent::Unknown { kind: 1, .. }
        ));
        assert!(matches!(
            GameEvent::from_raw(&ev(-3, &[], None)),
            GameEvent::Unknown { kind: -3, .. }
        ));
    }
}
