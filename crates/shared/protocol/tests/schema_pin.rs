//! Schema pinning (net-protocol 4.1). One fully filled sample of every message is encoded with postcard and
//! compared with `tests/schema-v{PROTOCOL_VERSION}.bin`. A changed byte means the wire format changed:
//! raise `PROTOCOL_VERSION` and write the new file (`SCHEMA_PIN_WRITE=1 cargo test -p protocol --test
//! schema_pin`). A field added by another track therefore cannot reach the network without a new version.
//!
//! File layout: for every sample in `samples()` order, `u32 LE` length and the postcard bytes. Samples are
//! only ever appended, like the wire format itself; the sample name list is checked too.
//!
//! Every sample also goes through postcard and JSON round trips.

use protocol::*;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fmt::Debug;

struct Pins {
    names: Vec<&'static str>,
    blob: Vec<u8>,
}

impl Pins {
    fn add<T: Serialize + DeserializeOwned + PartialEq + Debug>(
        &mut self,
        name: &'static str,
        v: &T,
    ) {
        let bytes = Postcard.encode(v).expect("encode");
        assert_eq!(
            &Postcard.decode::<T>(&bytes).expect("decode"),
            v,
            "{name}: postcard round trip"
        );
        let json = serde_json::to_vec(v).expect("json encode");
        assert_eq!(
            &serde_json::from_slice::<T>(&json).expect("json decode"),
            v,
            "{name}: json round trip"
        );
        self.names.push(name);
        self.blob.extend((bytes.len() as u32).to_le_bytes());
        self.blob.extend(bytes);
    }
}

fn plot() -> PlotView {
    PlotView {
        flags: PlotFlags(PlotFlags::REVEALED | PlotFlags::VISIBLE | PlotFlags::WORKED),
        plot_type: PlotType::Hill,
        terrain: 3,
        feature: 4,
        resource: 5,
        improvement: 6,
        route: 1,
        owner: 2,
        continent_art: 3,
        rivers: 0b101001,
        yields: [1, 2, 3, 4, 5, 6],
        building: 7,
        resource_qty: 8,
    }
}

fn basis_plot() -> PlotBasisView {
    PlotBasisView {
        plot_type: PlotType::Mountain,
        terrain: 4,
        continent_art: 2,
        feature: 11,
        flags: PlotFlags(PlotFlags::IS_LAKE | PlotFlags::IS_COASTAL | PlotFlags::NATURAL_WONDER),
        rivers: 0b110,
        river_flow: [1, 2, 3],
    }
}

fn basis() -> MapBasis {
    MapBasis {
        game: 3,
        basis_hash: 0x0123_4567_89AB_CDEF,
        width: 2,
        height: 1,
        wrap_x: true,
        wrap_y: true,
        terrain_seed: 77,
        plots: vec![basis_plot(), basis_plot()],
    }
}

fn unit() -> UnitView {
    UnitView {
        id: 100,
        owner: 1,
        unit_type: 3,
        x: 4,
        y: 5,
        hp: 90,
        max_hp: 100,
        moves_left: 60,
        max_moves: 120,
        strength: 8,
        ranged: 7,
        range: 2,
        domain: 1,
        activity: 2,
        level: 3,
        experience: 40,
        flags: UnitFlags(UnitFlags::CAN_MOVE | UnitFlags::NEEDS_ORDERS | UnitFlags::GARRISONED),
        name: "Warrior".into(),
        facing: 4,
        cargo_of: 55,
        building: 9,
    }
}

fn city() -> CityView {
    CityView {
        id: 7,
        owner: 1,
        x: 6,
        y: 7,
        name: "Roma".into(),
        population: 8,
        flags: CityFlags(CityFlags::CAPITAL | CityFlags::CAN_RANGE_ATTACK),
        hp: 200,
        max_hp: 250,
        defense: 1500,
        production_order: 2,
        production_item: 12,
        production_turns: 5,
        production_stored: 33,
        production_needed: 90,
        growth_turns: 4,
        yields: [1, 2, 3, 4, 5, 6],
        buildings: vec![3, 9],
        founder: 2,
    }
}

fn player() -> PlayerView {
    PlayerView {
        id: 1,
        team: 1,
        civ_type: 5,
        leader_type: 6,
        flags: PlayerFlags(PlayerFlags::ALIVE | PlayerFlags::MET),
        color_primary: 0xFF00_00FF,
        color_secondary: 0x00FF_00FF,
        name: "Caesar".into(),
        civ_name: "Rome".into(),
        era: 2,
        art_techs: 0x8000_0000_0000_0005,
    }
}

fn top_bar() -> TopBar {
    TopBar {
        gold: -5,
        gold_per_turn: 6,
        science_per_turn: 7,
        research_tech: 8,
        research_turns: 9,
        research_progress: 10,
        research_cost: 11,
        culture: 12,
        culture_per_turn: 13,
        next_policy_cost: 14,
        happiness: -3,
        golden_age_turns: 15,
        faith: 16,
        faith_per_turn: 17,
        era: 2,
        golden_age_progress: 18,
        golden_age_threshold: 19,
    }
}

fn notification() -> NotificationView {
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

fn peace_offer() -> PeaceOffer {
    PeaceOffer {
        from: 1,
        to: 2,
        turn: 9,
    }
}

fn trade_route() -> TradeRouteView {
    TradeRouteView {
        owner: 1,
        from: (1, 2),
        to: (-3, 4),
        domain: 1,
    }
}

fn world() -> WorldView {
    WorldView {
        turn: 10,
        year: -3000,
        human: 1,
        human_team: 1,
        status: TurnStatus::AiTurn { active: 2 },
        blocker: Some(Blocker {
            kind: 3,
            target: BlockerTarget::Plot { x: 4, y: 5 },
        }),
        map: MapView {
            width: 2,
            height: 1,
            wrap_x: true,
            wrap_y: true,
            plots: vec![plot(), plot()],
            terrain_seed: 0xDEAD_BEEF,
        },
        units: vec![unit()],
        cities: vec![city()],
        players: vec![player()],
        top_bar: top_bar(),
        notifications: vec![notification()],
        peace_offers: vec![peace_offer()],
        trade_routes: vec![trade_route()],
    }
}

fn view_diff() -> ViewDiff {
    ViewDiff {
        header: Some(ViewHeader {
            turn: 11,
            year: -2960,
            human: 1,
            human_team: 1,
            status: TurnStatus::GameOver { winner: Some(1) },
            blocker: Some(Blocker {
                kind: 1,
                target: BlockerTarget::Unit(4),
            }),
        }),
        plots: vec![(3, plot())],
        units: SectionDiff {
            upsert: vec![unit()],
            remove: vec![(1, 5), (2, -1)],
        },
        cities: SectionDiff {
            upsert: vec![city()],
            remove: vec![(0, 7)],
        },
        players: SectionDiff {
            upsert: vec![player()],
            remove: vec![3],
        },
        top_bar: Some(top_bar()),
        notifications: SectionDiff {
            upsert: vec![notification()],
            remove: vec![9],
        },
        peace_offers: Some(vec![peace_offer()]),
        basis: vec![(2, basis_plot())],
        trade_routes: Some(vec![trade_route()]),
    }
}

fn preview() -> CombatPreview {
    CombatPreview {
        attacker_damage: 30,
        defender_damage: 45,
        attacker_strength: 1200,
        defender_strength: 900,
        defender_hp: 100,
        defender_max_hp: 120,
        flags: CombatPreview::KILLS | CombatPreview::CITY,
    }
}

fn target(kind: TargetKind) -> TargetView {
    TargetView {
        x: -2,
        y: 9,
        kind,
        flags: TargetView::CITY,
        preview: Some(preview()),
    }
}

fn reach() -> Vec<ReachNode> {
    vec![
        ReachNode {
            x: 5,
            y: 6,
            parent: 0,
            turn: 1,
            moves_left: 60,
            flags: ReachFlags(ReachFlags::STOP),
        },
        ReachNode {
            x: 6,
            y: 6,
            parent: 0,
            turn: 2,
            moves_left: 30,
            flags: ReachFlags(ReachFlags::ATTACK | ReachFlags::CAPTURE),
        },
    ]
}

fn action() -> UnitActionView {
    UnitActionView {
        action: 3,
        enabled: true,
        reason: "why".into(),
    }
}

fn build() -> CityBuildView {
    CityBuildView {
        order_type: 1,
        item: 2,
        turns: 3,
        production_cost: 4,
        gold_cost: 5,
        flags: CityBuildFlags(CityBuildFlags::ENABLED | CityBuildFlags::CAN_PURCHASE),
        faith_cost: 6,
    }
}

fn settings() -> GameSettings {
    GameSettings {
        name: "Game".into(),
        world_size: "WORLDSIZE_HUGE".into(),
        map_script: "Lekmap".into(),
        map_options: vec![("sea".into(), 2)],
        speed: "GAMESPEED_STANDARD".into(),
        start_era: "ERA_ANCIENT".into(),
        handicap: "HANDICAP_PRINCE".into(),
        minor_civs: 12,
        turn_mode: TurnMode::Simultaneous,
        timer: TimerMode::Original {
            row: "TURNTIMER_STANDARD".into(),
        },
        game_options: vec![("GAMEOPTION_X".into(), 1)],
        seed: Some(11),
        map_seed: Some(12),
        auto_choices: AutoChoices {
            research: false,
            policy: true,
            ideology: false,
            religion: true,
            promotion: false,
            great_person: true,
            captured_city: false,
            league: true,
            archaeology: false,
            goody_hut: true,
            steal_tech: false,
        },
        diplo_auto_decline: false,
        mods: vec![ModPick {
            id: "example-mod".into(),
            req: "^1.2".into(),
        }],
    }
}

fn token() -> Token {
    Token([
        0x10, 0x21, 0x32, 0x43, 0x54, 0x65, 0x76, 0x87, 0x98, 0xA9, 0xBA, 0xCB, 0xDC, 0xED, 0xFE,
        0x0F,
    ])
}

fn claim() -> BasisClaim {
    BasisClaim {
        game: 3,
        hash: 0x0123_4567_89AB_CDEF,
    }
}

fn room_state() -> RoomState {
    RoomState {
        id: 4,
        name: "Room".into(),
        host: "alice".into(),
        status: GameStatus::Starting,
        settings: settings(),
        slots: vec![
            RoomSlot {
                index: 0,
                occupant: Occupant::Human {
                    nick: "alice".into(),
                    ready: true,
                },
                civ: CivPick::Civ("CIVILIZATION_ROME".into()),
                reserved_for: Some("alice".into()),
            },
            RoomSlot {
                index: 1,
                occupant: Occupant::Ai,
                civ: CivPick::Random,
                reserved_for: None,
            },
        ],
        from_save: Some("save-1".into()),
        mod_plan: ModPlan::Ok(vec![mod_view()]),
    }
}

fn lobby_state() -> LobbyState {
    LobbyState {
        rooms: vec![RoomSummary {
            id: 4,
            name: "Room".into(),
            host: "alice".into(),
            humans: 1,
            slots: 8,
            world_size: "WORLDSIZE_HUGE".into(),
            map_script: "Lekmap".into(),
            status: GameStatus::Lobby,
        }],
        games: vec![GameSummary {
            id: 9,
            name: "Game".into(),
            turn: 120,
            world_size: "WORLDSIZE_DUEL".into(),
            host: "alice".into(),
            status: GameStatus::Hibernated,
            seats: vec![SeatSummary {
                player: 2,
                civ_name: "Rome".into(),
                control: SeatControl::Ai {
                    kicked: Some("bob".into()),
                },
                online: true,
                takeable: true,
            }],
        }],
        saves: vec![SaveSummary {
            id: "save-1".into(),
            name: "Save".into(),
            turn: 55,
            saved_at_ms: 1_700_000_000_000,
            seats: vec![
                (0, "Rome".into(), Some("alice".into())),
                (1, "Egypt".into(), None),
            ],
            game: 9,
            host: "alice".into(),
        }],
        room: Some(room_state()),
        online: vec![
            OnlinePlayer {
                nick: "alice".into(),
                place: PlayerPlace::Room {
                    room: 4,
                    name: "Room".into(),
                    host: true,
                    ready: false,
                },
            },
            OnlinePlayer {
                nick: "bob".into(),
                place: PlayerPlace::Game {
                    game: 9,
                    name: "Game".into(),
                    turn: 120,
                },
            },
            OnlinePlayer {
                nick: "carol".into(),
                place: PlayerPlace::Lobby,
            },
        ],
    }
}

fn choice(id: &str) -> LobbyChoice {
    LobbyChoice {
        type_name: id.into(),
        name: format!("name of {id}"),
    }
}

fn choices() -> LobbyChoices {
    LobbyChoices {
        hash: 0xFEED_FACE,
        civs: vec![CivChoice {
            id: "CIVILIZATION_ROME".into(),
            name: "Rome".into(),
            leader: "Caesar".into(),
            traits: vec!["Trait: text".into()],
            unique: vec!["Unit: Legion".into()],
        }],
        handicaps: vec![choice("HANDICAP_PRINCE")],
        world_sizes: vec![WorldSizeChoice {
            id: "WORLDSIZE_HUGE".into(),
            name: "Huge".into(),
            width: 128,
            height: 80,
            default_players: 8,
            max_players: 12,
            default_minor: 16,
            max_minor: 24,
        }],
        map_scripts: vec![choice("Lekmap")],
        speeds: vec![choice("GAMESPEED_STANDARD")],
        eras: vec![choice("ERA_ANCIENT")],
        timers: vec![choice("TURNTIMER_STANDARD")],
        options: vec![OptionChoice {
            key: "GAMEOPTION_X".into(),
            name: "X".into(),
            default: 1,
            values: vec![(0, "off".into()), (1, "on".into())],
            help: "help".into(),
            group: OptionGroup::Ai,
        }],
        victories: vec![choice("VICTORY_TIME")],
        map_options: vec![MapScriptOptions {
            script: "Lekmap".into(),
            options: vec![MapOptionChoice {
                id: "1".into(),
                name: "World age".into(),
                help: "h".into(),
                default: 2,
                values: vec![(1, "3 billion".into()), (2, "4 billion".into())],
                read_only: true,
            }],
        }],
        max_minor_civs: 41,
        mods: vec![InstalledModView {
            id: "example-mod".into(),
            version: "1.2.3".into(),
            title: "Example".into(),
            sync: ModSync::Gameplay,
            client_required: true,
        }],
    }
}

fn entry(id: i16) -> CatalogEntry {
    CatalogEntry {
        id,
        type_name: format!("TYPE_{id}"),
        name: format!("Name {id}"),
        art: ArtRefs {
            art_define: "a".into(),
            icon_atlas: "b".into(),
            icon_index: 1,
            flag_atlas: "c".into(),
            flag_index: 2,
            art_style: "d".into(),
            strategic_view: "e".into(),
        },
    }
}

fn catalog() -> Catalog {
    let l = || vec![entry(1)];
    Catalog {
        units: l(),
        buildings: l(),
        projects: l(),
        techs: l(),
        policies: l(),
        terrains: l(),
        features: l(),
        resources: l(),
        improvements: l(),
        routes: l(),
        civs: l(),
        leaders: l(),
        eras: l(),
        handicaps: l(),
        promotions: l(),
        art_techs: vec![3, 90],
    }
}

fn journal(kind: JournalKind) -> JournalEntry {
    JournalEntry {
        turn: 5,
        kind,
        text: "text".into(),
        at: Some((-1, 7)),
        simplification: Some(2),
    }
}

fn setup() -> Setup {
    Setup {
        seed: Some(1),
        map_seed: Some(2),
        human_civ: "CIVILIZATION_ROME".into(),
        ai_count: 3,
        world_size: "WORLDSIZE_DUEL".into(),
        map_script: "m".into(),
        map_options: vec![("k".into(), 1)],
        handicap: "h".into(),
        game_speed: "s".into(),
        start_era: "e".into(),
        minor_civs: "4".into(),
        game_options: vec![("o".into(), 2)],
        auto_choices: AutoChoices::default(),
        diplo_auto_decline: false,
        autoplay_human: true,
        observer: true,
    }
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

fn commands() -> Vec<Command> {
    vec![
        Command::UnitMoveTo {
            unit: 1,
            x: 2,
            y: 3,
        },
        Command::UnitRangeAttack {
            unit: 1,
            x: 2,
            y: 3,
        },
        Command::UnitFoundCity { unit: 1 },
        Command::UnitFortify { unit: 1 },
        Command::UnitSleep { unit: 1 },
        Command::UnitSkip { unit: 1 },
        Command::UnitHeal { unit: 1 },
        Command::UnitAutomateBuild { unit: 1 },
        Command::UnitAutomateExplore { unit: 1 },
        Command::UnitCancel { unit: 1 },
        Command::UnitDisband { unit: 1 },
        Command::UnitWake { unit: 1 },
        Command::CitySetProduction {
            city: 2,
            order: 1,
            item: 3,
        },
        Command::CityPurchase {
            city: 2,
            order: 1,
            item: 3,
            yield_: 2,
        },
        Command::CityRangeAttack {
            city: 2,
            x: 4,
            y: 5,
        },
        Command::DeclareWar { team: 3 },
        Command::EndTurn { force: true },
        Command::CancelEndTurn,
        Command::ProposePeace { to: 3 },
        Command::AnswerPeace {
            from: 3,
            accept: true,
        },
    ]
}

fn queries() -> Vec<Query> {
    vec![
        Query::UnitActions { unit: 1 },
        Query::CityBuilds { city: 2 },
        Query::Path {
            unit: 1,
            x: 2,
            y: 3,
        },
        Query::Reachable { unit: 1 },
        Query::RangeTargets {
            from: Shooter::City(2),
        },
        Query::Units,
        Query::Cities,
        Query::TopBar,
        Query::Notifications,
        Query::Reach {
            unit: 1,
            horizon: 2,
        },
        Query::CombatPreview {
            attacker: Shooter::Unit(3),
            x: 4,
            y: 5,
        },
    ]
}

fn answers() -> Vec<QueryAnswer> {
    vec![
        QueryAnswer::UnitActions(vec![action()]),
        QueryAnswer::CityBuilds(vec![build()]),
        QueryAnswer::Path(vec![PathNode {
            x: 1,
            y: 2,
            turn: 3,
            flags: ReachFlags(ReachFlags::EMBARK | ReachFlags::UNREACHABLE),
        }]),
        QueryAnswer::Reachable(vec![(1, 2)]),
        QueryAnswer::RangeTargets(vec![(3, 4)]),
        QueryAnswer::Error {
            reason: "bad".into(),
        },
        QueryAnswer::Units(vec![unit()]),
        QueryAnswer::Cities(vec![city()]),
        QueryAnswer::TopBar(top_bar()),
        QueryAnswer::Notifications(vec![notification()]),
        QueryAnswer::Reach(reach()),
        QueryAnswer::CombatPreview(Some(preview())),
    ]
}

fn contexts() -> Vec<ContextMsg> {
    vec![
        ContextMsg::Unit(UnitContext {
            unit: 12,
            view_seq: 99,
            actions: vec![action()],
            reach: reach(),
            targets: vec![target(TargetKind::Ranged)],
        }),
        ContextMsg::City(CityContext {
            city: 4,
            view_seq: 100,
            builds: vec![build()],
            targets: vec![target(TargetKind::Melee)],
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
    let mut msgs = vec![
        GameMsg::Keyframe(Keyframe {
            view: world(),
            hash: 1 << 63,
            basis_hash: 5,
        }),
        GameMsg::Diff(ViewDelta {
            base: 41,
            diff: view_diff(),
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
                    code: RejectCode::TurnEnded,
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
        GameMsg::Journal(vec![journal(JournalKind::Diplomacy)]),
        GameMsg::TurnState(TurnState {
            turn: 12,
            phase: TurnPhase::GameOver { winner: Some(2) },
            mode: TurnMode::Hybrid,
            seats: vec![SeatTurn {
                player: 1,
                finished: true,
                may_act: true,
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
                    nick: "alice".into(),
                },
                conn: Conn::Lost { since_ms: 12 },
                host: true,
            },
            SeatView {
                player: 1,
                control: SeatControl::Closed,
                conn: Conn::Online,
                host: false,
            },
        ]),
        GameMsg::Context(contexts().remove(0)),
        GameMsg::Chat(vec![ChatLine {
            from: "alice".into(),
            from_player: Some(0),
            to: ChatTo::Player(1),
            text: "hi".into(),
            server_ms: 9,
        }]),
        GameMsg::MapBasis(basis()),
    ];
    msgs.extend(contexts().into_iter().skip(1).map(GameMsg::Context));
    msgs
}

fn samples() -> Pins {
    let mut p = Pins {
        names: Vec::new(),
        blob: Vec::new(),
    };

    // ClientMsg 0..=12
    p.add(
        "ClientMsg::Hello",
        &ClientMsg::Hello {
            protocol: PROTOCOL_VERSION,
            build: "dev-1".into(),
            nick: "alice".into(),
        },
    );
    p.add(
        "ClientMsg::Ping",
        &ClientMsg::Ping {
            seq: 7,
            client_ms: 1_700_000_000_123,
        },
    );
    p.add("ClientMsg::NewGame", &ClientMsg::NewGame(setup()));
    p.add("ClientMsg::Continue", &ClientMsg::Continue);
    p.add(
        "ClientMsg::Command",
        &ClientMsg::Command(Command::UnitMoveTo {
            unit: 1,
            x: 2,
            y: 3,
        }),
    );
    p.add(
        "ClientMsg::Query",
        &ClientMsg::Query {
            id: 77,
            q: Query::Reachable { unit: 5 },
        },
    );
    p.add(
        "ClientMsg::Login",
        &ClientMsg::Login(Login {
            password: "pw".into(),
            token: Some(token()),
            resume: Some(Resume {
                stream: 5,
                last_seq: 99,
                basis: Some(claim()),
            }),
        }),
    );
    p.add("ClientMsg::Lobby", &ClientMsg::Lobby(LobbyCmd::Ready(true)));
    p.add(
        "ClientMsg::Cmd",
        &ClientMsg::Cmd(Cmd {
            id: 17,
            turn: 120,
            cmd: Command::CancelEndTurn,
        }),
    );
    p.add(
        "ClientMsg::Focus",
        &ClientMsg::Focus(Focus {
            units: vec![4, 5, 6, 7],
            city: Some(2),
        }),
    );
    p.add(
        "ClientMsg::NeedKeyframe",
        &ClientMsg::NeedKeyframe {
            reason: KeyframeReason::BasisMismatch,
        },
    );
    p.add(
        "ClientMsg::Chat",
        &ClientMsg::Chat(ChatSend {
            to: ChatTo::Player(2),
            text: "hello".into(),
        }),
    );
    p.add(
        "ClientMsg::GameCtl",
        &ClientMsg::GameCtl(GameCtl::Kick { player: 3 }),
    );

    // ServerMsg 0..=20
    p.add(
        "ServerMsg::Welcome",
        &ServerMsg::Welcome {
            session: 42,
            build: "dev-1".into(),
        },
    );
    p.add(
        "ServerMsg::Reject",
        &ServerMsg::Reject(RejectReason::Unsupported {
            kind: "NewGame".into(),
        }),
    );
    p.add(
        "ServerMsg::Pong",
        &ServerMsg::Pong {
            seq: 7,
            client_ms: 1_700_000_000_123,
            server_ms: u64::MAX,
        },
    );
    p.add(
        "ServerMsg::Lobby",
        &ServerMsg::Lobby(LobbyInfo {
            civs: vec![choice("CIVILIZATION_ROME")],
            handicaps: vec![choice("HANDICAP_PRINCE")],
            save: Some(save_meta()),
        }),
    );
    p.add(
        "ServerMsg::GameStarted",
        &ServerMsg::GameStarted { catalog: catalog() },
    );
    p.add("ServerMsg::World", &ServerMsg::World(world()));
    p.add(
        "ServerMsg::TurnProgress",
        &ServerMsg::TurnProgress { active: 2 },
    );
    p.add(
        "ServerMsg::Journal",
        &ServerMsg::Journal(vec![journal(JournalKind::Combat)]),
    );
    p.add(
        "ServerMsg::QueryResult",
        &ServerMsg::QueryResult {
            id: 1,
            result: QueryAnswer::Reach(reach()),
        },
    );
    p.add(
        "ServerMsg::CommandRejected",
        &ServerMsg::CommandRejected {
            reason: "no".into(),
        },
    );
    p.add("ServerMsg::Saved", &ServerMsg::Saved(save_meta()));
    p.add(
        "ServerMsg::Fatal",
        &ServerMsg::Fatal {
            message: "boom".into(),
        },
    );
    p.add(
        "ServerMsg::SaveError",
        &ServerMsg::SaveError {
            message: "slot".into(),
        },
    );
    p.add(
        "ServerMsg::LoggedIn",
        &ServerMsg::LoggedIn(Session {
            token: Some(token()),
            nick: "alice".into(),
        }),
    );
    p.add(
        "ServerMsg::LoggedIn (cookie login)",
        &ServerMsg::LoggedIn(Session {
            token: None,
            nick: "alice".into(),
        }),
    );
    p.add("ServerMsg::Choices", &ServerMsg::Choices(choices()));
    p.add(
        "ServerMsg::LobbyState",
        &ServerMsg::LobbyState(lobby_state()),
    );
    p.add(
        "ServerMsg::Attached",
        &ServerMsg::Attached(Attached {
            game: 9,
            player: 2,
            stream: 5,
            mode: AttachMode::Replay,
            catalog: CatalogRef {
                hash: 0xCAFE_F00D,
                len: 123_456,
            },
            last_cmd_id: 31,
        }),
    );
    p.add(
        "ServerMsg::Game",
        &ServerMsg::Game(GameBatch {
            seq: u64::MAX,
            msgs: game_msgs(),
        }),
    );
    p.add(
        "ServerMsg::Detached",
        &ServerMsg::Detached {
            reason: DetachReason::GameStopped {
                why: "crash".into(),
            },
        },
    );
    p.add(
        "ServerMsg::LobbyError",
        &ServerMsg::LobbyError {
            text: "only the host".into(),
        },
    );
    p.add(
        "ServerMsg::GameCtlRejected",
        &ServerMsg::GameCtlRejected {
            ctl: GameCtl::Pause(true),
            reason: GameCtlReason::NotHost,
        },
    );

    // Payload types that no message variant spells out on its own.
    for r in [
        RejectReason::VersionMismatch { server: 3 },
        RejectReason::BadNick,
        RejectReason::BadPassword,
        RejectReason::Full,
        RejectReason::NickTaken,
    ]
    .iter()
    {
        p.add("RejectReason", r);
    }
    for c in &commands() {
        p.add("Command", c);
    }
    for q in &queries() {
        p.add("Query", q);
    }
    for a in &answers() {
        p.add("QueryAnswer", a);
    }
    for c in &contexts() {
        p.add("ContextMsg", c);
    }
    for m in &game_msgs() {
        p.add("GameMsg", m);
    }
    for c in [
        LobbyCmd::CreateRoom {
            settings: settings(),
        },
        LobbyCmd::JoinRoom { room: 5 },
        LobbyCmd::LeaveRoom,
        LobbyCmd::Settings(settings()),
        LobbyCmd::Slot {
            index: 3,
            occupant: SlotOccupant::Closed,
            civ: CivPick::Civ("CIVILIZATION_EGYPT".into()),
        },
        LobbyCmd::PickCiv {
            civ: CivPick::Random,
        },
        LobbyCmd::TakeSlot { index: 2 },
        LobbyCmd::Ready(true),
        LobbyCmd::Start,
        LobbyCmd::LoadSave {
            save: "save-1".into(),
        },
        LobbyCmd::JoinGame {
            game: 9,
            player: 2,
            basis: Some(claim()),
        },
        LobbyCmd::LeaveSeat,
    ] {
        p.add("LobbyCmd", &c);
    }
    for o in [SlotOccupant::Open, SlotOccupant::Ai, SlotOccupant::Closed] {
        p.add("SlotOccupant", &o);
    }
    for o in [
        Occupant::Open,
        Occupant::Human {
            nick: "alice".into(),
            ready: true,
        },
        Occupant::Ai,
        Occupant::Closed,
    ] {
        p.add("Occupant", &o);
    }
    for s in [
        GameStatus::Lobby,
        GameStatus::Starting,
        GameStatus::Running,
        GameStatus::Hibernated,
        GameStatus::Ended,
    ] {
        p.add("GameStatus", &s);
    }
    for t in [
        TimerMode::Off,
        TimerMode::Original {
            row: "TURNTIMER_STANDARD".into(),
        },
        TimerMode::Fixed { seconds: 90 },
    ] {
        p.add("TimerMode", &t);
    }
    p.add(
        "GameSetup",
        &GameSetup {
            settings: settings(),
            slots: vec![
                SlotSetup {
                    player: 0,
                    civ: CivPick::Random,
                    control: SlotControl::Human {
                        nick: "alice".into(),
                    },
                },
                SlotSetup {
                    player: 1,
                    civ: CivPick::Civ("CIVILIZATION_ROME".into()),
                    control: SlotControl::Ai,
                },
            ],
        },
    );
    for r in [
        GameCtl::Kick { player: 3 },
        GameCtl::Pause(false),
        GameCtl::Leave,
        GameCtl::Save,
    ] {
        p.add("GameCtl", &r);
    }
    for r in [
        GameCtlReason::NotHost,
        GameCtlReason::NoSuchPlayer,
        GameCtlReason::NotHuman,
        GameCtlReason::SelfKick,
        GameCtlReason::LastHuman,
        GameCtlReason::NoGame,
    ] {
        p.add("GameCtlReason", &r);
    }
    for r in [
        DetachReason::Kicked,
        DetachReason::GameOver,
        DetachReason::GameStopped { why: "w".into() },
        DetachReason::Replaced,
        DetachReason::Left,
    ] {
        p.add("DetachReason", &r);
    }
    for r in [
        KeyframeReason::HashMismatch,
        KeyframeReason::BaseMismatch,
        KeyframeReason::ApplyError,
        KeyframeReason::Decode,
        KeyframeReason::BasisMismatch,
    ] {
        p.add("KeyframeReason", &r);
    }
    for k in [
        JournalKind::Notification,
        JournalKind::AutoChoice,
        JournalKind::DiploAutoDeclined,
        JournalKind::Info,
        JournalKind::War,
        JournalKind::LuaError,
        JournalKind::Combat,
        JournalKind::System,
        JournalKind::Session,
        JournalKind::Diplomacy,
    ] {
        p.add("JournalKind", &journal(k));
    }
    p.add("Setup", &setup());

    // IPC gateway <-> worker (net-protocol 4.14), appended after everything else.
    let game_setup = GameSetup {
        settings: settings(),
        slots: vec![SlotSetup {
            player: 1,
            civ: CivPick::Random,
            control: SlotControl::Human {
                nick: "alice".into(),
            },
        }],
    };
    let cfg = WorkerConfig {
        stats: true,
        audit_fog: true,
        query_noise: 3,
        record: true,
        fixture_test: Some(FixtureTest {
            ai_step_delay_ms: 5,
            leak_fog: true,
            change_basis_at_turn: Some(4),
            flip_owner: Some(FlipOwner {
                at_turn: 5,
                plot: 7,
                owner: -1,
            }),
        }),
        ..WorkerConfig::default()
    };
    let init = |core, start| WorkerInit {
        ipc: IPC_VERSION,
        game: 9,
        core,
        start,
        save_dir: "saves/9".into(),
        seed: u64::MAX,
        cfg: cfg.clone(),
    };
    p.add(
        "ToWorker::Init",
        &ToWorker::Init(init(
            CoreKind::Fixture {
                huge: true,
                wrap_x: false,
            },
            Start::New(game_setup.clone()),
        )),
    );
    p.add(
        "ToWorker::Init(Load)",
        &ToWorker::Init(init(
            CoreKind::Real {
                gamedata_dir: "gd".into(),
            },
            Start::Load {
                file: "f.c5sv".into(),
                seats: game_setup.slots.clone(),
            },
        )),
    );
    p.add(
        "ToWorker::Init(Describe)",
        &ToWorker::Init(init(
            CoreKind::Real {
                gamedata_dir: "gd".into(),
            },
            Start::Describe,
        )),
    );
    p.add(
        "ToWorker::Cmd",
        &ToWorker::Cmd {
            player: 1,
            cmd: Cmd {
                id: 5,
                turn: 3,
                cmd: Command::CancelEndTurn,
            },
        },
    );
    p.add(
        "ToWorker::Query",
        &ToWorker::Query {
            player: 1,
            qid: 1 << 40,
            q: Query::Reach {
                unit: 1,
                horizon: 2,
            },
        },
    );
    p.add(
        "ToWorker::Focus",
        &ToWorker::Focus {
            player: 2,
            focus: Focus {
                units: vec![1, 2],
                city: Some(3),
            },
        },
    );
    p.add(
        "ToWorker::WantFull",
        &ToWorker::WantFull {
            player: 2,
            basis: true,
        },
    );
    for change in [SeatChange::Kick, SeatChange::Claim { nick: "bob".into() }] {
        p.add("ToWorker::Seat", &ToWorker::Seat { player: 3, change });
    }
    p.add(
        "ToWorker::ForceEndTurn",
        &ToWorker::ForceEndTurn {
            players: vec![0, 1],
        },
    );
    for reason in [SaveReason::Shutdown, SaveReason::Admin] {
        p.add("ToWorker::SaveNow", &ToWorker::SaveNow { reason });
    }
    p.add("ToWorker::Shutdown", &ToWorker::Shutdown { save: true });

    let turn_info = TurnInfo {
        turn: 12,
        phase: TurnPhase::Humans,
        mode: TurnMode::Hybrid,
        seats: vec![SeatTurnInfo {
            player: 1,
            finished: false,
            may_act: true,
            simultaneous: true,
        }],
        turn_len_s: 90,
        timer_base_s: 20,
        human_wars: vec![HumanWar { from: 0, to: 1 }],
    };
    let stats = StepStats {
        kind: StepKind::AiPhase,
        apply_us: 1,
        pump_us: 2,
        views_us: 3,
        diff_us: 4,
        ctx_us: 5,
        save_us: 6,
        ai_us: 7,
        diff_bytes: vec![(0, 100), (1, 200)],
        fog_violations: 2,
        digest: Some(u64::MAX),
    };
    let worker_msgs = vec![
        WorkerMsg::Ready(WorkerReady {
            build_id: "gc-1+FIX".into(),
            width: 2,
            height: 1,
            wrap_x: true,
            wrap_y: false,
            terrain_seed: 77,
            seats: game_setup.slots.clone(),
            turn: 0,
        }),
        WorkerMsg::Catalog {
            hash: 1 << 63,
            bytes: vec![1, 2, 3],
        },
        WorkerMsg::Choices(choices()),
        WorkerMsg::Basis {
            players: vec![0, 1],
            basis: basis(),
        },
        WorkerMsg::View {
            player: 0,
            update: ViewUpdate::Full {
                rev: 1,
                view: world(),
                hash: 9,
            },
        },
        WorkerMsg::View {
            player: 1,
            update: ViewUpdate::Delta {
                base_rev: 1,
                rev: 2,
                diff: view_diff(),
                hash: 5,
                basis_hash: Some(6),
            },
        },
        WorkerMsg::Results {
            player: 0,
            results: vec![CommandResult {
                id: 1,
                outcome: CommandOutcome::Applied,
            }],
        },
        WorkerMsg::Journal {
            player: 0,
            entries: vec![journal(JournalKind::System)],
        },
        WorkerMsg::Context {
            player: 0,
            rev: 2,
            ctx: ContextMsg::Dropped { unit: 4 },
        },
        WorkerMsg::QueryResult {
            qid: 7,
            answer: QueryAnswer::CombatPreview(None),
        },
        WorkerMsg::Turn(turn_info),
        WorkerMsg::SeatDone {
            player: 1,
            change: SeatChange::Kick,
            error: Some("e".into()),
        },
        WorkerMsg::Saved(SaveRecord {
            meta: save_meta(),
            file: "ring/00000003-t0012.c5sv".into(),
            kind: SaveKind::Keep,
            applied_upto: vec![(0, 41), (1, 7)],
        }),
        WorkerMsg::SaveFailed { reason: "r".into() },
        WorkerMsg::Heartbeat {
            phase: TurnPhase::Ai { active: 2 },
            queued: 3,
        },
        WorkerMsg::Stats(stats),
        WorkerMsg::Fatal {
            message: "m".into(),
            code: 5,
        },
    ];
    for m in &worker_msgs {
        p.add("WorkerMsg", m);
    }
    p.add("WorkerFrame", &WorkerFrame { msgs: worker_msgs });
    p.add("SaveKind", &SaveKind::Ring);
    for k in [
        StepKind::Orders,
        StepKind::AiPhase,
        StepKind::TurnChange,
        StepKind::Seat,
        StepKind::Other,
    ] {
        p.add("StepKind", &k);
    }
    replica_samples(&mut p);
    account_samples(&mut p);
    mod_samples(&mut p);
    p
}

/// v12 additions: the account store.
fn account_samples(p: &mut Pins) {
    p.add(
        "ClientMsg::Account",
        &ClientMsg::Account(AccountSave {
            set: vec![
                ("GridOn".into(), "1".into()),
                ("Volume.0".into(), "0.5".into()),
            ],
            remove: vec!["YieldOn".into()],
        }),
    );
    p.add(
        "ServerMsg::Account",
        &ServerMsg::Account(AccountData {
            version: ACCOUNT_VERSION,
            prefs: vec![("GridOn".into(), "1".into())],
            last_room: Some(settings()),
        }),
    );
    p.add(
        "ServerMsg::Account(empty)",
        &ServerMsg::Account(AccountData::default()),
    );
}

/// v4 additions, appended after every v3 sample.
fn replica_samples(p: &mut Pins) {
    let blob = GcBlobRef {
        hash: 1 << 63,
        size: 26_000_000,
    };
    let base = GcBase {
        epoch: 2,
        turn: 158,
        jseq: 40,
        raw_hash: 0x0123_4567_89AB_CDEF,
        snapshot: blob,
    };
    let turn = GcTurn {
        epoch: 2,
        turn: 159,
        jseq: 41,
        raw_hash: 77,
        snapshot: blob,
        patch: Some(GcPatchRef {
            from_raw_hash: 0x0123_4567_89AB_CDEF,
            blob: GcBlobRef {
                hash: 9,
                size: 225_000,
            },
        }),
    };
    let journal = GcJournal {
        epoch: 2,
        first_jseq: 41,
        batches: vec![vec![1, 2, 3], vec![]],
    };
    p.add("GameMsg::GcBase", &GameMsg::GcBase(base));
    p.add("GameMsg::GcJournal", &GameMsg::GcJournal(journal.clone()));
    p.add("GameMsg::GcTurn", &GameMsg::GcTurn(turn));
    p.add(
        "GameMsg::GcTurn(none)",
        &GameMsg::GcTurn(GcTurn {
            patch: None,
            ..turn
        }),
    );
    p.add("ClientMsg::GcNet", &ClientMsg::GcNet(vec![9, 8, 7]));
    for reason in [
        GcResyncReason::Diverged,
        GcResyncReason::Rejected,
        GcResyncReason::BadBlob,
        GcResyncReason::Gap,
    ] {
        p.add(
            "ClientMsg::GcResync",
            &ClientMsg::GcResync {
                epoch: 2,
                have_raw_hash: Some(5),
                at_jseq: 44,
                reason,
            },
        );
    }
    p.add(
        "ClientMsg::GcResync(none)",
        &ClientMsg::GcResync {
            epoch: 0,
            have_raw_hash: None,
            at_jseq: 0,
            reason: GcResyncReason::Gap,
        },
    );
    p.add(
        "ToWorker::Net",
        &ToWorker::Net {
            player: 1,
            msgs: vec![1, 2],
        },
    );
    p.add(
        "ToWorker::ReplicaSnapshot",
        &ToWorker::ReplicaSnapshot { player: 2 },
    );
    p.add(
        "WorkerMsg::ReplicaJournal",
        &WorkerMsg::ReplicaJournal {
            epoch: 2,
            first_jseq: 41,
            batches: journal.batches,
        },
    );
    p.add(
        "WorkerMsg::ReplicaBlob",
        &WorkerMsg::ReplicaBlob {
            hash: 3,
            bytes: vec![1, 2, 3],
        },
    );
    p.add(
        "WorkerMsg::ReplicaBase",
        &WorkerMsg::ReplicaBase {
            epoch: 2,
            turn: 158,
            jseq: 40,
            raw_hash: 5,
            snapshot: blob,
            for_player: Some(3),
        },
    );
    p.add(
        "WorkerMsg::ReplicaTurn",
        &WorkerMsg::ReplicaTurn {
            epoch: 2,
            turn: 159,
            jseq: 41,
            raw_hash: 77,
            snapshot: blob,
            patch: turn.patch,
        },
    );
    p.add(
        "WorkerMsg::NetRejected",
        &WorkerMsg::NetRejected {
            player: 1,
            reason: "not your turn".into(),
        },
    );
    p.add(
        "WorkerMsg::RecJournal",
        &WorkerMsg::RecJournal {
            phase: RecPhase::Ai,
            turn: 126,
            bytes: vec![1, 2, 3],
        },
    );
    p.add(
        "WorkerMsg::RecMark",
        &WorkerMsg::RecMark {
            turn: 126,
            what: "a step in turn 126, Ai".into(),
            begin: true,
        },
    );
}

fn pin_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(format!("schema-v{PROTOCOL_VERSION}.bin"))
}

#[test]
fn schema_is_pinned() {
    let now = samples();
    let path = pin_path();
    if std::env::var_os("SCHEMA_PIN_WRITE").is_some() {
        std::fs::write(&path, &now.blob).expect("write schema pin");
        return;
    }
    let pinned = std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "no schema pin {}: {e}. PROTOCOL_VERSION is {PROTOCOL_VERSION}; write the file for the new version \
             with SCHEMA_PIN_WRITE=1 cargo test -p protocol --test schema_pin",
            path.display()
        )
    });
    if pinned != now.blob {
        let first = (0..now.names.len()).find(|&i| record(&pinned, i) != record(&now.blob, i));
        panic!(
            "the wire format changed (first differing sample: {:?}). Raise PROTOCOL_VERSION (now {PROTOCOL_VERSION}) \
             and write tests/schema-v<new>.bin with SCHEMA_PIN_WRITE=1 cargo test -p protocol --test schema_pin",
            first.map(|i| now.names[i])
        );
    }
}

/// The `index`-th record (length prefix included) of a pin blob made by `samples()`; `None` past the end.
fn record(blob: &[u8], index: usize) -> Option<&[u8]> {
    let mut at = 0usize;
    for i in 0..=index {
        let head = blob.get(at..at + 4)?;
        let len = u32::from_le_bytes([head[0], head[1], head[2], head[3]]) as usize;
        let end = at.checked_add(4 + len)?;
        if i == index {
            return blob.get(at..end);
        }
        at = end;
    }
    None
}

fn mod_view() -> ModView {
    ModView {
        id: "example-mod".into(),
        version: "1.2.3".into(),
        load_idx: 3,
        sync: ModSync::Cosmetic,
        client_required: false,
        available: true,
        pack: Some("0f".repeat(32)),
    }
}

/// v13 additions: the mod envelope.
fn mod_samples(p: &mut Pins) {
    p.add(
        "ClientMsg::ModHello",
        &ClientMsg::ModHello {
            api: (1, 2, 3),
            built_in: vec![BuiltMod {
                id: "example-mod".into(),
                version: "1.2.3".into(),
                code_hash: [7; 16],
            }],
        },
    );
    p.add(
        "ClientMsg::Mod",
        &ClientMsg::Mod(ModC2S {
            mod_idx: 2,
            ch: 5,
            payload: vec![1, 2, 3],
        }),
    );
    let set = ModSetView {
        modset_hash: 0x0123_4567_89AB_CDEF,
        mods: vec![
            mod_view(),
            ModView {
                id: "other".into(),
                version: "0.1.0".into(),
                load_idx: 4,
                sync: ModSync::Client,
                client_required: true,
                available: false,
                pack: None,
            },
        ],
    };
    p.add("ModSetView", &set);
    p.add(
        "GameBatch(Mods)",
        &GameBatch {
            seq: 1,
            msgs: vec![GameMsg::Mods(set)],
        },
    );
    p.add(
        "GameMsg::Mod",
        &GameMsg::Mod(ModS2C {
            mod_idx: 2,
            ch: 5,
            from: Some(1),
            payload: vec![9, 8, 7],
        }),
    );
    p.add(
        "GameMsg::Mod(no from)",
        &GameMsg::Mod(ModS2C {
            mod_idx: 0,
            ch: 0,
            from: None,
            payload: Vec::new(),
        }),
    );
    for s in [ModSync::Gameplay, ModSync::Cosmetic, ModSync::Client] {
        p.add("ModSync", &s);
    }
    let problem = || ModProblem {
        text: "mod 'example-mod' 1.2.3 is required".into(),
    };
    p.add("ModPlan::Err", &ModPlan::Err(vec![problem()]));
    p.add(
        "RejectReason::ModMismatch",
        &RejectReason::ModMismatch(vec![problem()]),
    );
    p.add(
        "ServerMsg::Reject(ModMismatch)",
        &ServerMsg::Reject(RejectReason::ModMismatch(vec![problem()])),
    );
    // IPC v8: the mods block between the gateway and the worker.
    let section = ModsSection {
        lock: "mods.lock text".into(),
        states: vec![
            ModState {
                id: "example-mod".into(),
                state: vec![1, 2, 3],
            },
            ModState {
                id: "other".into(),
                state: Vec::new(),
            },
        ],
    };
    p.add("ToWorker::Mods", &ToWorker::Mods(section.clone()));
    p.add("WorkerMsg::ModsLoaded", &WorkerMsg::ModsLoaded(section));
    p.add("ToWorker::WantOwners", &ToWorker::WantOwners);
    p.add(
        "WorkerMsg::Owners::Full",
        &WorkerMsg::Owners(OwnersUpdate::Full {
            rev: 2,
            turn: 7,
            owners: vec![-1, 0, 3, -1],
        }),
    );
    p.add(
        "WorkerMsg::Owners::Delta",
        &WorkerMsg::Owners(OwnersUpdate::Delta {
            base_rev: 2,
            rev: 3,
            changes: vec![(1, 4), (3, -1)],
        }),
    );
}
