//! Wire codecs: postcard (binary, production) and JSON (debug, `json` feature).

use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("postcard: {0}")]
    Postcard(#[from] postcard::Error),
    #[error("trailing bytes after message: {0}")]
    TrailingBytes(usize),
    #[cfg(any(feature = "json", test))]
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

pub trait Codec {
    fn encode<T: Serialize>(&self, value: &T) -> Result<Vec<u8>, CodecError>;
    fn decode<T: DeserializeOwned>(&self, bytes: &[u8]) -> Result<T, CodecError>;
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Postcard;

impl Codec for Postcard {
    fn encode<T: Serialize>(&self, value: &T) -> Result<Vec<u8>, CodecError> {
        Ok(postcard::to_allocvec(value)?)
    }

    fn decode<T: DeserializeOwned>(&self, bytes: &[u8]) -> Result<T, CodecError> {
        let (value, rest) = postcard::take_from_bytes(bytes)?;
        if rest.is_empty() {
            Ok(value)
        } else {
            Err(CodecError::TrailingBytes(rest.len()))
        }
    }
}

#[cfg(any(feature = "json", test))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Json;

#[cfg(any(feature = "json", test))]
impl Codec for Json {
    fn encode<T: Serialize>(&self, value: &T) -> Result<Vec<u8>, CodecError> {
        Ok(serde_json::to_vec(value)?)
    }

    fn decode<T: DeserializeOwned>(&self, bytes: &[u8]) -> Result<T, CodecError> {
        Ok(serde_json::from_slice(bytes)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    fn client_msgs() -> Vec<ClientMsg> {
        vec![
            ClientMsg::Hello {
                protocol: PROTOCOL_VERSION,
                build: "dev-1234".into(),
                nick: "алиса".into(),
            },
            ClientMsg::Ping {
                seq: 7,
                client_ms: 1_700_000_000_123,
            },
            ClientMsg::NewGame(Setup::default()),
            ClientMsg::Continue,
            ClientMsg::Command(Command::CityPurchase {
                city: 9,
                order: 1,
                item: -1,
                yield_: 2,
            }),
            ClientMsg::Query {
                id: 77,
                q: Query::RangeTargets {
                    from: Shooter::City(4),
                },
            },
        ]
    }

    fn server_msgs() -> Vec<ServerMsg> {
        vec![
            ServerMsg::Welcome {
                session: 42,
                build: "dev-1234".into(),
            },
            ServerMsg::Reject(RejectReason::VersionMismatch { server: 3 }),
            ServerMsg::Reject(RejectReason::BadNick),
            ServerMsg::Pong {
                seq: 7,
                client_ms: 1_700_000_000_123,
                server_ms: u64::MAX,
            },
            ServerMsg::Lobby(LobbyInfo {
                civs: vec![LobbyChoice {
                    type_name: "CIVILIZATION_ROME".into(),
                    name: "Rome".into(),
                }],
                handicaps: vec![],
                save: Some(save_meta()),
            }),
            ServerMsg::GameStarted {
                catalog: Catalog {
                    units: vec![CatalogEntry {
                        id: 3,
                        type_name: "UNIT_WARRIOR".into(),
                        name: "Warrior".into(),
                        art: ArtRefs::default(),
                    }],
                    ..Catalog::default()
                },
            },
            ServerMsg::World(sample_world()),
            ServerMsg::TurnProgress { active: 2 },
            ServerMsg::Journal(vec![JournalEntry {
                turn: 5,
                kind: JournalKind::AutoChoice,
                text: "Research: Pottery".into(),
                at: Some((-1, 7)),
                simplification: Some(2),
            }]),
            ServerMsg::QueryResult {
                id: 1,
                result: QueryAnswer::Path(vec![PathNode {
                    x: 1,
                    y: 2,
                    turn: 3,
                    flags: ReachFlags(ReachFlags::EMBARK | ReachFlags::STOP),
                }]),
            },
            ServerMsg::CommandRejected {
                reason: "fixture".into(),
            },
            ServerMsg::Saved(save_meta()),
            ServerMsg::Fatal {
                message: "boom".into(),
            },
            ServerMsg::SaveError {
                message: "slot A: checksum".into(),
            },
        ]
    }

    fn save_meta() -> SaveMeta {
        SaveMeta {
            seq: 3,
            build_id: "gc-1".into(),
            turn: 12,
            civ_name: "Rome".into(),
            saved_at_ms: 1_700_000_000_000,
        }
    }

    fn sample_world() -> WorldView {
        let plot = |t: i16| PlotView {
            flags: PlotFlags(PlotFlags::REVEALED | PlotFlags::VISIBLE),
            plot_type: PlotType::Hill,
            terrain: t,
            feature: -1,
            resource: 4,
            improvement: -1,
            route: -1,
            owner: 1,
            continent_art: 2,
            rivers: 0b101001,
            yields: [2, 1, 0, 0, -1, 0],
            building: 7,
            resource_qty: 3,
        };
        WorldView {
            turn: 10,
            year: -3000,
            human: 0,
            human_team: 0,
            status: TurnStatus::AiTurn { active: 1 },
            blocker: Some(Blocker {
                kind: 3,
                target: BlockerTarget::Plot { x: 4, y: 5 },
            }),
            map: MapView {
                width: 2,
                height: 2,
                wrap_x: true,
                wrap_y: false,
                plots: vec![plot(0), plot(1), PlotView::default(), plot(2)],
                terrain_seed: 0xDEAD_BEEF,
            },
            units: vec![UnitView {
                id: 100,
                owner: 0,
                unit_type: 3,
                x: 1,
                y: 1,
                hp: 100,
                max_hp: 100,
                moves_left: 120,
                max_moves: 120,
                flags: UnitFlags(UnitFlags::CAN_MOVE | UnitFlags::NEEDS_ORDERS),
                name: "Warrior".into(),
                ..UnitView::default()
            }],
            cities: vec![CityView {
                id: 1,
                name: "Roma".into(),
                flags: CityFlags(CityFlags::CAPITAL),
                yields: [5, 4, 3, 2, 1, 0],
                ..CityView::default()
            }],
            players: vec![PlayerView {
                color_primary: 0xFF00_00FF,
                name: "Caesar".into(),
                ..PlayerView::default()
            }],
            top_bar: TopBar {
                gold: -5,
                ..TopBar::default()
            },
            notifications: vec![sample_notification()],
            peace_offers: vec![PeaceOffer {
                from: 1,
                to: 0,
                turn: 9,
            }],
            trade_routes: vec![TradeRouteView {
                owner: 1,
                from: (1, 2),
                to: (-3, 4),
                domain: 1,
            }],
        }
    }

    fn sample_notification() -> NotificationView {
        NotificationView {
            id: 4,
            kind: 5,
            at: Some((1, 2)),
            data1: -1,
            data2: 2,
            turn: 3,
            dismissed: true,
            message: "m".into(),
            summary: "s".into(),
        }
    }

    fn game_commands() -> Vec<GameCommand> {
        vec![
            GameCommand {
                kind: 12,
                player: 3,
                args: vec![-1, 0, i32::MAX, i32::MIN],
                text: Some("Москва".into()),
            },
            GameCommand {
                kind: 0,
                player: 0,
                args: vec![],
                text: None,
            },
        ]
    }

    fn round_trip<C: Codec>(codec: &C) {
        for msg in client_msgs() {
            let bytes = codec.encode(&msg).expect("encode ClientMsg");
            assert_eq!(
                codec.decode::<ClientMsg>(&bytes).expect("decode ClientMsg"),
                msg
            );
        }
        for msg in server_msgs() {
            let bytes = codec.encode(&msg).expect("encode ServerMsg");
            assert_eq!(
                codec.decode::<ServerMsg>(&bytes).expect("decode ServerMsg"),
                msg
            );
        }
        for cmd in game_commands() {
            let bytes = codec.encode(&cmd).expect("encode GameCommand");
            assert_eq!(
                codec
                    .decode::<GameCommand>(&bytes)
                    .expect("decode GameCommand"),
                cmd
            );
        }
    }

    #[test]
    fn round_trip_postcard() {
        round_trip(&Postcard);
    }

    #[test]
    fn round_trip_json() {
        round_trip(&Json);
    }

    /// Breaks if variants or fields are reordered (tag stability).
    #[test]
    fn tag_stability_client_msg() {
        let hello = ClientMsg::Hello {
            protocol: 1,
            build: "b".into(),
            nick: "n".into(),
        };
        assert_eq!(Postcard.encode(&hello).unwrap(), [0, 1, 1, b'b', 1, b'n']);

        let ping = ClientMsg::Ping {
            seq: 1,
            client_ms: 300,
        };
        assert_eq!(Postcard.encode(&ping).unwrap(), [1, 1, 0xAC, 0x02]);
    }

    #[test]
    fn tag_stability_server_msg() {
        let welcome = ServerMsg::Welcome {
            session: 5,
            build: String::new(),
        };
        assert_eq!(Postcard.encode(&welcome).unwrap(), [0, 5, 0]);

        let reject = ServerMsg::Reject(RejectReason::VersionMismatch { server: 2 });
        assert_eq!(Postcard.encode(&reject).unwrap(), [1, 0, 2]);

        let bad_nick = ServerMsg::Reject(RejectReason::BadNick);
        assert_eq!(Postcard.encode(&bad_nick).unwrap(), [1, 1]);

        let pong = ServerMsg::Pong {
            seq: 2,
            client_ms: 3,
            server_ms: 4,
        };
        assert_eq!(Postcard.encode(&pong).unwrap(), [2, 2, 3, 4]);
    }

    #[test]
    fn tag_stability_json() {
        let ping = ClientMsg::Ping {
            seq: 1,
            client_ms: 2,
        };
        assert_eq!(
            Json.encode(&ping).unwrap(),
            br#"{"Ping":{"seq":1,"client_ms":2}}"#
        );
        let bad_nick = ServerMsg::Reject(RejectReason::BadNick);
        assert_eq!(Json.encode(&bad_nick).unwrap(), br#"{"Reject":"BadNick"}"#);
    }

    #[test]
    fn postcard_rejects_trailing_bytes() {
        let mut bytes = Postcard
            .encode(&ServerMsg::Reject(RejectReason::BadNick))
            .unwrap();
        bytes.push(0);
        assert!(matches!(
            Postcard.decode::<ServerMsg>(&bytes),
            Err(CodecError::TrailingBytes(1))
        ));
    }

    #[test]
    fn postcard_rejects_unknown_variant() {
        assert!(matches!(
            Postcard.decode::<ServerMsg>(&[99]),
            Err(CodecError::Postcard(_))
        ));
        assert!(Postcard.decode::<ClientMsg>(&[]).is_err());
    }

    #[test]
    fn kinds() {
        let client: Vec<_> = client_msgs().iter().map(ClientMsg::kind).collect();
        assert_eq!(
            client,
            ["Hello", "Ping", "NewGame", "Continue", "Command", "Query"]
        );
        let server: Vec<_> = server_msgs().iter().map(ServerMsg::kind).collect();
        assert_eq!(
            server,
            [
                "Welcome",
                "Reject",
                "Reject",
                "Pong",
                "Lobby",
                "GameStarted",
                "World",
                "TurnProgress",
                "Journal",
                "QueryResult",
                "CommandRejected",
                "Saved",
                "Fatal",
                "SaveError"
            ]
        );
        assert_eq!(RejectReason::BadNick.kind(), "BadNick");
        assert!(
            RejectReason::VersionMismatch { server: 9 }
                .to_string()
                .contains('9')
        );
    }

    #[test]
    fn protocol_version_is_12() {
        assert_eq!(PROTOCOL_VERSION, 12);
    }

    /// New ClientMsg variants keep tags 2..=5 (after Hello = 0, Ping = 1).
    #[test]
    fn tag_stability_world_client_msg() {
        let mut setup = Setup::default();
        setup.human_civ.clear();
        setup.map_script.clear();
        setup.world_size.clear();
        setup.handicap.clear();
        setup.game_speed.clear();
        setup.start_era.clear();
        setup.minor_civs.clear();
        // tag, seed None, map_seed None, 7 empty strings/vecs around ai_count, 11 auto-choices.
        let bytes = Postcard.encode(&ClientMsg::NewGame(setup)).unwrap();
        assert_eq!(bytes[0], 2);
        assert_eq!(&bytes[1..4], [0, 0, 0]); // seed, map_seed, human_civ ""
        assert_eq!(bytes[4], 1); // ai_count

        assert_eq!(Postcard.encode(&ClientMsg::Continue).unwrap(), [3]);

        let cmd = |c| Postcard.encode(&ClientMsg::Command(c)).unwrap();
        assert_eq!(
            cmd(Command::UnitMoveTo {
                unit: 1,
                x: 2,
                y: -3
            }),
            [4, 0, 2, 4, 5]
        );
        assert_eq!(
            cmd(Command::UnitRangeAttack {
                unit: 1,
                x: 2,
                y: 3
            }),
            [4, 1, 2, 4, 6]
        );
        assert_eq!(cmd(Command::UnitFoundCity { unit: 1 }), [4, 2, 2]);
        assert_eq!(cmd(Command::UnitFortify { unit: 1 }), [4, 3, 2]);
        assert_eq!(cmd(Command::UnitSleep { unit: 1 }), [4, 4, 2]);
        assert_eq!(cmd(Command::UnitSkip { unit: 1 }), [4, 5, 2]);
        assert_eq!(cmd(Command::UnitHeal { unit: 1 }), [4, 6, 2]);
        assert_eq!(cmd(Command::UnitAutomateBuild { unit: 1 }), [4, 7, 2]);
        assert_eq!(cmd(Command::UnitAutomateExplore { unit: 1 }), [4, 8, 2]);
        assert_eq!(cmd(Command::UnitCancel { unit: 1 }), [4, 9, 2]);
        assert_eq!(cmd(Command::UnitDisband { unit: 1 }), [4, 10, 2]);
        assert_eq!(cmd(Command::UnitWake { unit: 1 }), [4, 11, 2]);
        assert_eq!(
            cmd(Command::CitySetProduction {
                city: 1,
                order: 2,
                item: 3
            }),
            [4, 12, 2, 2, 6]
        );
        assert_eq!(
            cmd(Command::CityPurchase {
                city: 1,
                order: 2,
                item: 3,
                yield_: 1
            }),
            [4, 13, 2, 2, 6, 1]
        );
        assert_eq!(
            cmd(Command::CityRangeAttack {
                city: 1,
                x: 2,
                y: 3
            }),
            [4, 14, 2, 4, 6]
        );
        assert_eq!(cmd(Command::DeclareWar { team: 3 }), [4, 15, 3]);
        assert_eq!(cmd(Command::EndTurn { force: true }), [4, 16, 1]);

        let q = |q| Postcard.encode(&ClientMsg::Query { id: 9, q }).unwrap();
        assert_eq!(q(Query::UnitActions { unit: 1 }), [5, 9, 0, 2]);
        assert_eq!(q(Query::CityBuilds { city: 1 }), [5, 9, 1, 2]);
        assert_eq!(
            q(Query::Path {
                unit: 1,
                x: 2,
                y: 3
            }),
            [5, 9, 2, 2, 4, 6]
        );
        assert_eq!(q(Query::Reachable { unit: 1 }), [5, 9, 3, 2]);
        assert_eq!(
            q(Query::RangeTargets {
                from: Shooter::Unit(1)
            }),
            [5, 9, 4, 0, 2]
        );
        assert_eq!(
            q(Query::RangeTargets {
                from: Shooter::City(1)
            }),
            [5, 9, 4, 1, 2]
        );
        assert_eq!(q(Query::Units), [5, 9, 5]);
        assert_eq!(q(Query::Cities), [5, 9, 6]);
        assert_eq!(q(Query::TopBar), [5, 9, 7]);
        assert_eq!(q(Query::Notifications), [5, 9, 8]);
    }

    /// New ServerMsg variants keep tags 3..=11 (after Welcome = 0, Reject = 1, Pong = 2); SaveError
    /// (G5) is 12.
    #[test]
    fn tag_stability_world_server_msg() {
        let enc = |m: ServerMsg| Postcard.encode(&m).unwrap();
        let lobby = enc(ServerMsg::Lobby(LobbyInfo {
            civs: vec![],
            handicaps: vec![],
            save: None,
        }));
        assert_eq!(lobby, [3, 0, 0, 0]);
        let started = enc(ServerMsg::GameStarted {
            catalog: Catalog::default(),
        });
        assert_eq!(started[0], 4);
        assert_eq!(started.len(), 1 + 16); // sixteen empty lists (15 tables and `art_techs`)
        let mut world = sample_world();
        world.turn = 1;
        let w = enc(ServerMsg::World(world));
        assert_eq!(&w[..3], [5, 2, 0xEF]); // tag, turn=1 zigzag, year -3000 zigzag low byte
        assert_eq!(enc(ServerMsg::TurnProgress { active: 2 }), [6, 2]);
        assert_eq!(enc(ServerMsg::Journal(vec![])), [7, 0]);
        assert_eq!(
            enc(ServerMsg::QueryResult {
                id: 1,
                result: QueryAnswer::Reachable(vec![])
            }),
            [8, 1, 3, 0]
        );
        assert_eq!(
            enc(ServerMsg::CommandRejected { reason: "x".into() }),
            [9, 1, b'x']
        );
        let saved = enc(ServerMsg::Saved(SaveMeta {
            seq: 1,
            build_id: String::new(),
            turn: 1,
            civ_name: String::new(),
            saved_at_ms: 1,
        }));
        assert_eq!(saved, [10, 1, 0, 2, 0, 1]);
        assert_eq!(
            enc(ServerMsg::Fatal {
                message: "m".into()
            }),
            [11, 1, b'm']
        );
        assert_eq!(
            enc(ServerMsg::SaveError {
                message: "s".into()
            }),
            [12, 1, b's']
        );
    }

    #[test]
    fn tag_stability_world_enums() {
        let enc = |v: &TurnStatus| Postcard.encode(v).unwrap();
        assert_eq!(enc(&TurnStatus::HumanTurn), [0]);
        assert_eq!(enc(&TurnStatus::AiTurn { active: -1 }), [1, 0xFF]);
        assert_eq!(enc(&TurnStatus::GameOver { winner: Some(2) }), [2, 1, 2]);
        assert_eq!(Postcard.encode(&PlotType::Water).unwrap(), [3]);
        assert_eq!(Postcard.encode(&JournalKind::LuaError).unwrap(), [5]);
        assert_eq!(Postcard.encode(&JournalKind::Combat).unwrap(), [6]);
        assert_eq!(
            Postcard
                .encode(&BlockerTarget::Plot { x: 1, y: 1 })
                .unwrap(),
            [3, 2, 2]
        );
        assert_eq!(
            Postcard.encode(&QueryAnswer::RangeTargets(vec![])).unwrap(),
            [4, 0]
        );
        assert_eq!(
            Postcard
                .encode(&QueryAnswer::Error { reason: "x".into() })
                .unwrap(),
            [5, 1, b'x']
        );
        let answer = |a: &QueryAnswer| Postcard.encode(a).unwrap();
        assert_eq!(answer(&QueryAnswer::Units(vec![])), [6, 0]);
        assert_eq!(answer(&QueryAnswer::Cities(vec![])), [7, 0]);
        assert_eq!(answer(&QueryAnswer::TopBar(TopBar::default()))[0], 8);
        assert_eq!(answer(&QueryAnswer::Notifications(vec![])), [9, 0]);
        let n = NotificationView {
            id: 1,
            kind: 2,
            at: None,
            data1: 0,
            data2: 0,
            turn: 3,
            dismissed: false,
            message: "m".into(),
            summary: String::new(),
        };
        assert_eq!(
            answer(&QueryAnswer::Notifications(vec![n.clone()])),
            [9, 1, 2, 2, 0, 0, 0, 6, 0, 1, b'm', 0]
        );
        let bytes = Postcard
            .encode(&QueryAnswer::Notifications(vec![n.clone()]))
            .unwrap();
        assert_eq!(
            Postcard.decode::<QueryAnswer>(&bytes).unwrap(),
            QueryAnswer::Notifications(vec![n])
        );
    }

    #[test]
    fn plot_flags_contains() {
        let f = PlotFlags(PlotFlags::REVEALED | PlotFlags::HAS_CITY);
        assert!(f.contains(PlotFlags::REVEALED));
        assert!(f.contains(PlotFlags::HAS_CITY));
        assert!(!f.contains(PlotFlags::VISIBLE));
    }

    // ------------------------------------------------------------ protocol v3: stream, context, orders

    fn sample_basis() -> MapBasis {
        MapBasis {
            game: 3,
            basis_hash: u64::MAX,
            width: 2,
            height: 2,
            wrap_x: true,
            wrap_y: false,
            terrain_seed: 77,
            plots: vec![
                PlotBasisView {
                    plot_type: PlotType::Mountain,
                    terrain: 4,
                    continent_art: 2,
                    feature: 11,
                    flags: PlotFlags(
                        PlotFlags::IS_LAKE | PlotFlags::IS_COASTAL | PlotFlags::NATURAL_WONDER,
                    ),
                    rivers: 0b110,
                    river_flow: [1, -1, 3],
                },
                PlotBasisView::default(),
                PlotBasisView::default(),
                PlotBasisView::default(),
            ],
        }
    }

    fn sample_diff() -> ViewDiff {
        let world = sample_world();
        ViewDiff {
            header: Some(ViewHeader {
                turn: 11,
                year: -2960,
                human: 1,
                human_team: 1,
                status: TurnStatus::GameOver { winner: None },
                blocker: None,
            }),
            plots: vec![(3, world.map.plots[0].clone())],
            units: SectionDiff {
                upsert: world.units.clone(),
                remove: vec![(1, 5), (2, -1)],
            },
            cities: SectionDiff {
                upsert: world.cities.clone(),
                remove: vec![(0, 7)],
            },
            players: SectionDiff {
                upsert: world.players.clone(),
                remove: vec![3],
            },
            top_bar: Some(world.top_bar.clone()),
            notifications: SectionDiff {
                upsert: vec![sample_notification()],
                remove: vec![9],
            },
            peace_offers: Some(world.peace_offers.clone()),
            basis: vec![(2, sample_basis().plots[0].clone())],
            trade_routes: Some(world.trade_routes.clone()),
        }
    }

    fn sample_target(kind: TargetKind) -> TargetView {
        TargetView {
            x: -2,
            y: 9,
            kind,
            flags: TargetView::CITY,
            preview: Some(CombatPreview {
                attacker_damage: 30,
                defender_damage: 45,
                attacker_strength: 1200,
                defender_strength: 900,
                defender_hp: 100,
                defender_max_hp: 100,
                flags: CombatPreview::KILLS | CombatPreview::CITY,
            }),
        }
    }

    fn sample_reach() -> Vec<ReachNode> {
        vec![
            ReachNode::default(),
            ReachNode {
                x: 5,
                y: 6,
                parent: 0,
                turn: 1,
                moves_left: 60,
                flags: ReachFlags(ReachFlags::ATTACK | ReachFlags::CAPTURE),
            },
        ]
    }

    fn context_msgs() -> Vec<ContextMsg> {
        vec![
            ContextMsg::Unit(UnitContext {
                unit: 12,
                view_seq: 99,
                actions: vec![UnitActionView {
                    action: 3,
                    enabled: true,
                    reason: String::new(),
                }],
                reach: sample_reach(),
                targets: vec![sample_target(TargetKind::Ranged)],
            }),
            ContextMsg::City(CityContext {
                city: 4,
                view_seq: 100,
                builds: vec![CityBuildView {
                    order_type: 1,
                    item: 2,
                    turns: 3,
                    production_cost: 4,
                    gold_cost: -1,
                    flags: CityBuildFlags(CityBuildFlags::ENABLED),
                    faith_cost: -1,
                }],
                targets: vec![sample_target(TargetKind::Melee)],
            }),
            ContextMsg::Valid {
                units: vec![1, 2, 3],
                city: Some(4),
                view_seq: 101,
            },
            ContextMsg::Dropped { unit: 8 },
        ]
    }

    fn game_msgs() -> Vec<GameMsg> {
        let world = sample_world();
        let mut msgs = vec![
            GameMsg::Keyframe(Keyframe {
                view: world,
                hash: 1 << 63,
                basis_hash: 5,
            }),
            GameMsg::Diff(ViewDelta {
                base: 41,
                diff: sample_diff(),
                hash: 6,
                basis_hash: Some(7),
            }),
            GameMsg::Results(vec![
                CommandResult {
                    id: 1,
                    outcome: CommandOutcome::Applied,
                },
                CommandResult {
                    id: 2,
                    outcome: CommandOutcome::Rejected {
                        code: RejectCode::Core,
                        reason: "no".into(),
                    },
                },
                CommandResult {
                    id: 3,
                    outcome: CommandOutcome::Queued,
                },
                CommandResult {
                    id: 4,
                    outcome: CommandOutcome::Lost,
                },
            ]),
            GameMsg::Journal(vec![JournalEntry {
                turn: 5,
                kind: JournalKind::Diplomacy,
                text: "peace".into(),
                at: None,
                simplification: None,
            }]),
            GameMsg::TurnState(TurnState {
                turn: 12,
                phase: TurnPhase::Ai { active: 3 },
                mode: TurnMode::Hybrid,
                seats: vec![SeatTurn {
                    player: 1,
                    finished: true,
                    may_act: false,
                    group: 2,
                }],
                timers: vec![TimerState {
                    group: 2,
                    deadline_ms: u64::MAX,
                    len_ms: 30_000,
                    left_ms_when_stopped: Some(1500),
                }],
                paused: true,
            }),
            GameMsg::Seats(vec![
                SeatView {
                    player: 0,
                    control: SeatControl::Human {
                        nick: "алиса".into(),
                    },
                    conn: Conn::Lost { since_ms: 12 },
                    host: true,
                },
                SeatView {
                    player: 1,
                    control: SeatControl::Ai {
                        kicked: Some("bob".into()),
                    },
                    conn: Conn::None,
                    host: false,
                },
            ]),
            GameMsg::Chat(vec![ChatLine {
                from: "алиса".into(),
                from_player: Some(0),
                to: ChatTo::Player(1),
                text: "hi".into(),
                server_ms: 9,
            }]),
            GameMsg::MapBasis(sample_basis()),
        ];
        msgs.extend(context_msgs().into_iter().map(GameMsg::Context));
        msgs
    }

    fn rt<C: Codec, T>(codec: &C, value: &T)
    where
        T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let bytes = codec.encode(value).expect("encode");
        assert_eq!(&codec.decode::<T>(&bytes).expect("decode"), value);
    }

    fn round_trip_v3<C: Codec>(codec: &C) {
        rt(
            codec,
            &GameBatch {
                seq: u64::MAX,
                msgs: game_msgs(),
            },
        );
        for m in game_msgs() {
            rt(codec, &m);
        }
        for m in context_msgs() {
            rt(codec, &m);
        }
        rt(codec, &sample_diff());
        rt(codec, &ViewDiff::default());
        rt(codec, &sample_basis());
        rt(codec, &SectionMask(0x1FF));
        rt(
            codec,
            &Focus {
                units: vec![4, 5, 6, 7],
                city: Some(2),
            },
        );
        rt(
            codec,
            &Cmd {
                id: 17,
                turn: 120,
                cmd: Command::AnswerPeace {
                    from: 2,
                    accept: true,
                },
            },
        );
        for cmd in [
            Command::CancelEndTurn,
            Command::ProposePeace { to: 3 },
            Command::AnswerPeace {
                from: -1,
                accept: false,
            },
        ] {
            rt(codec, &cmd);
        }
        for q in [
            Query::Reach {
                unit: 5,
                horizon: 3,
            },
            Query::CombatPreview {
                attacker: Shooter::City(2),
                x: 3,
                y: 4,
            },
        ] {
            rt(codec, &q);
        }
        rt(codec, &QueryAnswer::Reach(sample_reach()));
        rt(codec, &QueryAnswer::CombatPreview(None));
        rt(
            codec,
            &QueryAnswer::CombatPreview(sample_target(TargetKind::Melee).preview),
        );
        for kind in [
            JournalKind::System,
            JournalKind::Session,
            JournalKind::Diplomacy,
        ] {
            rt(codec, &kind);
        }
        for reason in [
            KeyframeReason::HashMismatch,
            KeyframeReason::BaseMismatch,
            KeyframeReason::ApplyError,
            KeyframeReason::Decode,
            KeyframeReason::BasisMismatch,
        ] {
            rt(codec, &reason);
        }
        rt(
            codec,
            &Catalog {
                art_techs: vec![3, 90],
                ..Catalog::default()
            },
        );
        rt(
            codec,
            &PlayerView {
                art_techs: u64::MAX,
                ..PlayerView::default()
            },
        );
        rt(
            codec,
            &UnitView {
                facing: 5,
                cargo_of: 77,
                building: 3,
                ..UnitView::default()
            },
        );
        rt(
            codec,
            &CityView {
                buildings: vec![1, 2, 3],
                founder: 4,
                ..CityView::default()
            },
        );
        rt(
            codec,
            &PathNode {
                x: 1,
                y: 2,
                turn: 3,
                flags: ReachFlags(ReachFlags::UNREACHABLE),
            },
        );
    }

    #[test]
    fn round_trip_v3_postcard() {
        round_trip_v3(&Postcard);
    }

    #[test]
    fn round_trip_v3_json() {
        round_trip_v3(&Json);
    }

    fn tag<T: serde::Serialize>(v: &T) -> Vec<u8> {
        Postcard.encode(v).unwrap()
    }

    #[test]
    fn tag_stability_v3_commands_and_queries() {
        let cmd = |c| Postcard.encode(&ClientMsg::Command(c)).unwrap();
        assert_eq!(cmd(Command::CancelEndTurn), [4, 17]);
        assert_eq!(cmd(Command::ProposePeace { to: 1 }), [4, 18, 1]);
        assert_eq!(
            cmd(Command::AnswerPeace {
                from: 2,
                accept: true
            }),
            [4, 19, 2, 1]
        );
        let q = |q| Postcard.encode(&ClientMsg::Query { id: 9, q }).unwrap();
        assert_eq!(
            q(Query::Reach {
                unit: 1,
                horizon: 2
            }),
            [5, 9, 9, 2, 2]
        );
        assert_eq!(
            q(Query::CombatPreview {
                attacker: Shooter::Unit(1),
                x: 2,
                y: 3
            }),
            [5, 9, 10, 0, 2, 4, 6]
        );
        assert_eq!(tag(&QueryAnswer::Reach(vec![])), [10, 0]);
        assert_eq!(tag(&QueryAnswer::CombatPreview(None)), [11, 0]);
        assert_eq!(tag(&JournalKind::System), [7]);
        assert_eq!(tag(&JournalKind::Session), [8]);
        assert_eq!(tag(&JournalKind::Diplomacy), [9]);
    }

    #[test]
    fn tag_stability_game_msg() {
        let first_bytes: Vec<u8> = game_msgs().iter().take(8).map(|m| tag(m)[0]).collect();
        assert_eq!(first_bytes, [0, 1, 2, 3, 4, 5, 7, 8]);
        // Context is tag 6, its variants 0..=3.
        let ctx: Vec<(u8, u8)> = context_msgs()
            .into_iter()
            .map(|c| {
                let b = tag(&GameMsg::Context(c));
                (b[0], b[1])
            })
            .collect();
        assert_eq!(ctx, [(6, 0), (6, 1), (6, 2), (6, 3)]);
        assert_eq!(tag(&GameMsg::Results(vec![])), [2, 0]);
        assert_eq!(tag(&GameMsg::Journal(vec![])), [3, 0]);
        assert_eq!(tag(&GameMsg::Seats(vec![])), [5, 0]);
        assert_eq!(tag(&GameMsg::Chat(vec![])), [7, 0]);
        assert_eq!(
            tag(&GameBatch {
                seq: 1,
                msgs: vec![]
            }),
            [1, 0]
        );
    }

    #[test]
    fn tag_stability_v3_enums() {
        let outcome = |o: CommandOutcome| tag(&o);
        assert_eq!(outcome(CommandOutcome::Applied), [0]);
        assert_eq!(
            outcome(CommandOutcome::Rejected {
                code: RejectCode::Expired,
                reason: "x".into()
            }),
            [1, 8, 1, b'x']
        );
        assert_eq!(outcome(CommandOutcome::Queued), [2]);
        assert_eq!(outcome(CommandOutcome::Lost), [3]);
        let codes = [
            RejectCode::Core,
            RejectCode::TurnChanged,
            RejectCode::TurnEnded,
            RejectCode::NotYourTurn,
            RejectCode::NoGame,
            RejectCode::Restarting,
            RejectCode::TooMany,
            RejectCode::Invalid,
            RejectCode::Expired,
            RejectCode::NotSeated,
        ];
        for (i, c) in codes.iter().enumerate() {
            assert_eq!(tag(c), [i as u8]);
        }
        let reasons = [
            KeyframeReason::HashMismatch,
            KeyframeReason::BaseMismatch,
            KeyframeReason::ApplyError,
            KeyframeReason::Decode,
            KeyframeReason::BasisMismatch,
        ];
        for (i, r) in reasons.iter().enumerate() {
            assert_eq!(tag(r), [i as u8]);
        }
        assert_eq!(tag(&TurnPhase::Starting), [0]);
        assert_eq!(tag(&TurnPhase::Ai { active: 1 }), [1, 1]);
        assert_eq!(tag(&TurnPhase::Humans), [2]);
        assert_eq!(tag(&TurnPhase::GameOver { winner: None }), [3, 0]);
        assert_eq!(tag(&TurnMode::Simultaneous), [0]);
        assert_eq!(tag(&TurnMode::Hybrid), [1]);
        assert_eq!(tag(&SeatControl::Human { nick: "n".into() }), [0, 1, b'n']);
        assert_eq!(tag(&SeatControl::Ai { kicked: None }), [1, 0]);
        assert_eq!(tag(&SeatControl::Closed), [2]);
        assert_eq!(tag(&Conn::Online), [0]);
        assert_eq!(tag(&Conn::Lost { since_ms: 5 }), [1, 5]);
        assert_eq!(tag(&Conn::None), [2]);
        assert_eq!(tag(&ChatTo::All), [0]);
        assert_eq!(tag(&ChatTo::Player(2)), [1, 2]);
        assert_eq!(tag(&TargetKind::Ranged), [0]);
        assert_eq!(tag(&TargetKind::Melee), [1]);
    }

    /// Records keep the v2 bytes in front; the new fields are appended (positional codec).
    #[test]
    fn v3_fields_are_appended() {
        // PathNode: x, y, turn, then flags; UNREACHABLE = bit 7 = varint 0x80 0x01.
        let node = PathNode {
            x: 1,
            y: 2,
            turn: 3,
            flags: ReachFlags(ReachFlags::UNREACHABLE),
        };
        assert_eq!(tag(&node), [2, 4, 6, 0x80, 0x01]);
        let plain = PathNode {
            flags: ReachFlags::default(),
            ..node
        };
        assert_eq!(tag(&plain), [2, 4, 6, 0]);
        // ReachNode: x, y, parent (u16), turn (u8), moves_left, flags.
        let r = ReachNode {
            x: 1,
            y: 2,
            parent: 3,
            turn: 4,
            moves_left: 5,
            flags: ReachFlags(ReachFlags::STOP),
        };
        assert_eq!(tag(&r), [2, 4, 3, 4, 10, 16]);
        // PlayerView::art_techs is the last field: a u64 varint.
        let p = tag(&PlayerView {
            art_techs: 1,
            ..PlayerView::default()
        });
        assert_eq!(*p.last().unwrap(), 1);
        // Catalog: `art_techs` is the 16th list, after `promotions`.
        let c = tag(&Catalog {
            art_techs: vec![5],
            ..Catalog::default()
        });
        assert_eq!(c.len(), 15 + 2);
        assert_eq!(&c[15..], [1, 10]);
        // MapView: terrain_seed is last.
        let m = tag(&MapView {
            terrain_seed: 3,
            ..MapView::default()
        });
        assert_eq!(*m.last().unwrap(), 3);
    }

    #[test]
    fn v3_flag_bits() {
        assert_eq!(PlotFlags::PILLAGED, 1 << 5);
        assert_eq!(PlotFlags::NATURAL_WONDER, 1 << 6);
        assert_eq!(PlotFlags::ROUTE_PILLAGED, 1 << 7);
        assert_eq!(PlotFlags::WORKED, 1 << 8);
        // No combat-ready bit: the original shows the ready pose only inside a fight the core reports (R-D7).
        assert_eq!(UnitFlags::GARRISONED, 1 << 8);
        let bits = [
            ReachFlags::ATTACK,
            ReachFlags::EMBARK,
            ReachFlags::DISEMBARK,
            ReachFlags::CITY,
            ReachFlags::STOP,
            ReachFlags::UNREVEALED,
            ReachFlags::CAPTURE,
            ReachFlags::UNREACHABLE,
        ];
        for (i, b) in bits.iter().enumerate() {
            assert_eq!(*b, 1 << i);
        }
    }

    /// The map basis is its own message: it has stream tag 8 and no field of `WorldView` carries it.
    #[test]
    fn map_basis_is_its_own_message() {
        let basis = tag(&sample_basis());
        let msg = tag(&GameMsg::MapBasis(sample_basis()));
        assert_eq!(msg[0], 8);
        assert_eq!(&msg[1..], &basis[..]);
    }

    // ------------------------------------------------------------ N1.7: login, lobby, session

    fn settings() -> GameSettings {
        GameSettings {
            name: "g".into(),
            world_size: "WORLDSIZE_DUEL".into(),
            map_script: "m".into(),
            map_options: vec![("k".into(), -1)],
            speed: "s".into(),
            start_era: "e".into(),
            handicap: "h".into(),
            minor_civs: 3,
            turn_mode: TurnMode::Hybrid,
            timer: TimerMode::Fixed { seconds: 90 },
            game_options: vec![],
            seed: Some(7),
            map_seed: None,
            auto_choices: AutoChoices::default(),
            diplo_auto_decline: true,
        }
    }

    #[test]
    fn tag_stability_v12_account() {
        assert_eq!(
            tag(&ClientMsg::Account(AccountSave {
                set: vec![("a".into(), "b".into())],
                remove: vec![],
            })),
            [15, 1, 1, b'a', 1, b'b', 0]
        );
        assert_eq!(
            tag(&ServerMsg::Account(AccountData::default())),
            [21, 0, 0, 0]
        );
    }

    #[test]
    fn tag_stability_v3_client_msg() {
        let token = Token([7; 16]);
        let login = ClientMsg::Login(Login {
            password: "pw".into(),
            token: Some(token),
            resume: None,
        });
        let mut want = vec![6, 2, b'p', b'w', 1];
        want.extend([7; 16]);
        want.push(0);
        assert_eq!(tag(&login), want);
        assert_eq!(tag(&ClientMsg::Lobby(LobbyCmd::LeaveRoom)), [7, 2]);
        assert_eq!(
            tag(&ClientMsg::Cmd(Cmd {
                id: 1,
                turn: 2,
                cmd: Command::CancelEndTurn
            })),
            [8, 1, 4, 17]
        );
        assert_eq!(
            tag(&ClientMsg::Focus(Focus {
                units: vec![3],
                city: None
            })),
            [9, 1, 6, 0]
        );
        assert_eq!(
            tag(&ClientMsg::NeedKeyframe {
                reason: KeyframeReason::BasisMismatch
            }),
            [10, 4]
        );
        assert_eq!(
            tag(&ClientMsg::Chat(ChatSend {
                to: ChatTo::All,
                text: "a".into()
            })),
            [11, 0, 1, b'a']
        );
        assert_eq!(tag(&ClientMsg::GameCtl(GameCtl::Leave)), [12, 2]);
        // v5: the host's save of the game.
        assert_eq!(tag(&ClientMsg::GameCtl(GameCtl::Save)), [12, 3]);
        assert_eq!(tag(&ClientMsg::GameCtl(GameCtl::Pause(true))), [12, 1, 1]);
        assert_eq!(
            tag(&ClientMsg::GameCtl(GameCtl::Kick { player: 3 })),
            [12, 0, 3]
        );
    }

    #[test]
    fn tag_stability_v4_replica() {
        use crate::replica::*;
        let blob = GcBlobRef { hash: 1, size: 2 };
        assert_eq!(tag(&ClientMsg::GcNet(vec![9])), [13, 1, 9]);
        assert_eq!(
            tag(&ClientMsg::GcResync {
                epoch: 1,
                have_raw_hash: None,
                at_jseq: 2,
                reason: GcResyncReason::Gap
            }),
            [14, 1, 0, 2, 3]
        );
        assert_eq!(tag(&GcResyncReason::Diverged), [0]);
        assert_eq!(tag(&GcResyncReason::Rejected), [1]);
        assert_eq!(tag(&GcResyncReason::BadBlob), [2]);
        let base = GcBase {
            epoch: 1,
            turn: 2,
            jseq: 3,
            raw_hash: 4,
            snapshot: blob,
        };
        assert_eq!(tag(&GameMsg::GcBase(base))[..5], [9, 1, 4, 3, 4]);
        assert_eq!(
            tag(&GameMsg::GcJournal(GcJournal {
                epoch: 1,
                first_jseq: 2,
                batches: vec![vec![7]]
            })),
            [10, 1, 2, 1, 1, 7]
        );
        let turn = GcTurn {
            epoch: 1,
            turn: 2,
            jseq: 3,
            raw_hash: 4,
            snapshot: blob,
            patch: None,
        };
        assert_eq!(tag(&GameMsg::GcTurn(turn))[0], 11);
        assert_eq!(*tag(&GameMsg::GcTurn(turn)).last().unwrap(), 0);
        // v7: the AI phase's turn events (turn 2: player 3 starts, player 3 ends).
        assert_eq!(
            tag(&GameMsg::AiTurn(AiTurn {
                turn: 2,
                events: vec![
                    AiTurnEvent {
                        player: 3,
                        ended: false
                    },
                    AiTurnEvent {
                        player: 3,
                        ended: true
                    }
                ]
            })),
            [12, 4, 2, 3, 0, 3, 1]
        );
    }

    #[test]
    fn tag_stability_v3_server_msg() {
        let session = Session {
            token: Some(Token([9; 16])),
            nick: "n".into(),
        };
        // v9: the token is an `Option` (1 = Some), `None` for the cookie login of the web page.
        let mut want = vec![13, 1];
        want.extend([9; 16]);
        want.extend([1, b'n']);
        assert_eq!(tag(&ServerMsg::LoggedIn(session)), want);
        let cookie = Session {
            token: None,
            nick: "n".into(),
        };
        assert_eq!(tag(&ServerMsg::LoggedIn(cookie)), [13, 0, 1, b'n']);
        assert_eq!(tag(&ServerMsg::Choices(LobbyChoices::default()))[0], 14);
        assert_eq!(
            tag(&ServerMsg::LobbyState(LobbyState::default())),
            // v11: `online` ends the state.
            [15, 0, 0, 0, 0, 0]
        );
        let attached = ServerMsg::Attached(Attached {
            game: 1,
            player: 2,
            stream: 3,
            mode: AttachMode::Replay,
            catalog: CatalogRef { hash: 4, len: 5 },
            last_cmd_id: 6,
        });
        assert_eq!(tag(&attached), [16, 1, 2, 3, 1, 4, 5, 6]);
        assert_eq!(
            tag(&ServerMsg::Game(GameBatch {
                seq: 1,
                msgs: vec![]
            })),
            [17, 1, 0]
        );
        assert_eq!(
            tag(&ServerMsg::Detached {
                reason: DetachReason::Replaced
            }),
            [18, 3]
        );
        assert_eq!(
            tag(&ServerMsg::LobbyError { text: "x".into() }),
            [19, 1, b'x']
        );
        assert_eq!(
            tag(&ServerMsg::GameCtlRejected {
                ctl: GameCtl::Leave,
                reason: GameCtlReason::LastHuman
            }),
            [20, 2, 4]
        );
        // Reject reasons: v2 tags stay, v3 ones are appended.
        assert_eq!(tag(&RejectReason::BadPassword), [2]);
        assert_eq!(
            tag(&RejectReason::Unsupported { kind: "k".into() }),
            [3, 1, b'k']
        );
        assert_eq!(tag(&RejectReason::Full), [4]);
        assert_eq!(tag(&RejectReason::NickTaken), [5]);
        assert_eq!(tag(&ServerMsg::Reject(RejectReason::NickTaken)), [1, 5]);
    }

    #[test]
    fn tag_stability_v3_session_and_lobby_enums() {
        assert_eq!(tag(&AttachMode::Fresh), [0]);
        assert_eq!(tag(&AttachMode::Replay), [1]);
        assert_eq!(tag(&DetachReason::Kicked), [0]);
        assert_eq!(tag(&DetachReason::GameOver), [1]);
        assert_eq!(
            tag(&DetachReason::GameStopped { why: "w".into() }),
            [2, 1, b'w']
        );
        assert_eq!(tag(&DetachReason::Replaced), [3]);
        assert_eq!(tag(&DetachReason::Left), [4]);
        let reasons = [
            GameCtlReason::NotHost,
            GameCtlReason::NoSuchPlayer,
            GameCtlReason::NotHuman,
            GameCtlReason::SelfKick,
            GameCtlReason::LastHuman,
            GameCtlReason::NoGame,
        ];
        for (i, r) in reasons.iter().enumerate() {
            assert_eq!(tag(r), [i as u8]);
        }
        let statuses = [
            GameStatus::Lobby,
            GameStatus::Starting,
            GameStatus::Running,
            GameStatus::Hibernated,
            GameStatus::Ended,
        ];
        for (i, s) in statuses.iter().enumerate() {
            assert_eq!(tag(s), [i as u8]);
        }
        assert_eq!(tag(&TimerMode::Off), [0]);
        assert_eq!(tag(&TimerMode::Original { row: "r".into() }), [1, 1, b'r']);
        assert_eq!(tag(&TimerMode::Fixed { seconds: 90 }), [2, 90]);
        assert_eq!(tag(&SlotOccupant::Open), [0]);
        assert_eq!(tag(&SlotOccupant::Ai), [1]);
        assert_eq!(tag(&SlotOccupant::Closed), [2]);
        assert_eq!(tag(&Occupant::Open), [0]);
        assert_eq!(
            tag(&Occupant::Human {
                nick: "n".into(),
                ready: true
            }),
            [1, 1, b'n', 1]
        );
        assert_eq!(tag(&Occupant::Ai), [2]);
        assert_eq!(tag(&Occupant::Closed), [3]);
        assert_eq!(tag(&CivPick::Random), [0]);
        assert_eq!(tag(&CivPick::Civ("c".into())), [1, 1, b'c']);
        assert_eq!(tag(&SlotControl::Human { nick: "n".into() }), [0, 1, b'n']);
        assert_eq!(tag(&SlotControl::Ai), [1]);
    }

    #[test]
    fn tag_stability_v3_lobby_cmd() {
        let g = settings();
        let first = |c: LobbyCmd| tag(&c)[0];
        assert_eq!(
            first(LobbyCmd::CreateRoom {
                settings: g.clone()
            }),
            0
        );
        assert_eq!(tag(&LobbyCmd::JoinRoom { room: 5 }), [1, 5]);
        assert_eq!(tag(&LobbyCmd::LeaveRoom), [2]);
        assert_eq!(first(LobbyCmd::Settings(g)), 3);
        assert_eq!(
            tag(&LobbyCmd::Slot {
                index: 1,
                occupant: SlotOccupant::Ai,
                civ: CivPick::Random
            }),
            [4, 1, 1, 0]
        );
        assert_eq!(
            tag(&LobbyCmd::PickCiv {
                civ: CivPick::Random
            }),
            [5, 0]
        );
        assert_eq!(tag(&LobbyCmd::TakeSlot { index: 2 }), [6, 2]);
        assert_eq!(tag(&LobbyCmd::Ready(true)), [7, 1]);
        assert_eq!(tag(&LobbyCmd::Start), [8]);
        assert_eq!(tag(&LobbyCmd::LoadSave { save: "s".into() }), [9, 1, b's']);
        assert_eq!(
            tag(&LobbyCmd::JoinGame {
                game: 1,
                player: 2,
                basis: Some(BasisClaim { game: 1, hash: 3 })
            }),
            [10, 1, 2, 1, 1, 3]
        );
        assert_eq!(tag(&LobbyCmd::LeaveSeat), [11]);
    }

    #[test]
    fn debug_hides_password_and_token() {
        let token = Token([0xAB; 16]);
        let login = Login {
            password: "hunter2-secret".into(),
            token: Some(token),
            resume: None,
        };
        let session = Session {
            token: Some(token),
            nick: "alice".into(),
        };
        let shown = format!(
            "{login:?} {token:?} {session:?} {:?} {:?}",
            ClientMsg::Login(login.clone()),
            ServerMsg::LoggedIn(session.clone())
        );
        assert!(!shown.contains("hunter2"), "{shown}");
        assert!(!shown.contains("171"), "{shown}");
        assert!(shown.contains("***"));
        assert!(shown.contains("alice"));
        assert_eq!(format!("{token:?}"), "Token(***)");
    }

    /// Full-size Duel map: size sanity check (spec: about 40 KB) and exact round trip.
    #[test]
    fn world_view_duel_round_trip() {
        let mut world = sample_world();
        world.map.width = 40;
        world.map.height = 24;
        world.map.plots = (0..40 * 24)
            .map(|i| PlotView {
                terrain: (i % 9) as i16,
                resource: -1,
                ..sample_world().map.plots[0].clone()
            })
            .collect();
        let bytes = Postcard.encode(&world).unwrap();
        assert!(bytes.len() < 40 * 1024, "{} bytes", bytes.len());
        assert_eq!(Postcard.decode::<WorldView>(&bytes).unwrap(), world);
    }
}
