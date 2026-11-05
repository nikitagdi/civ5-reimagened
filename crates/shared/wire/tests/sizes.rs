//! Frame size measurement on synthetic views (net-protocol 4.16, NP-D9, NP-D22; BUILD AC-N3.5).
//!
//! Run: `cargo xtask test -p wire --release --test sizes -- --nocapture`.
//!
//! The views are protocol v3 records (`WorldView`, `MapBasis`, `Keyframe`, `GameBatch`, the IPC
//! `WorkerFrame`) filled with patchy synthetic data; the hashes are the real ones (`ViewHash`,
//! `basis_hash_of`). Every number is printed. The thresholds of net-protocol 4.16 are asserted at the end,
//! after the whole table is printed, and only for the keyframe without the basis: Duel 15 KiB, Huge
//! (128x80, and the older 82x74) 80 KiB. The basis and the cold join are printed, no threshold (NP-D22).
//! A miss is a question for the orchestrator (zstd (C) on the server or a dictionary).

use protocol::{
    Blocker, BlockerTarget, CityFlags, CityView, Codec, GameBatch, GameMsg, Keyframe, MapBasis,
    MapView, PlayerFlags, PlayerView, PlotBasisView, PlotFlags, PlotType, PlotView, Postcard,
    TopBar, TurnMode, TurnPhase, TurnState, TurnStatus, UnitFlags, UnitView, ViewHash, ViewUpdate,
    WorkerFrame, WorkerMsg, WorldView, basis_hash_of,
};
use wire::Encoder;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

/// Smooth-ish value noise in 0..100 on a coarse grid, so terrain forms patches like a real map.
fn patch(seed: u64, x: i32, y: i32, cell: i32) -> u64 {
    let h = |gx: i32, gy: i32| -> u64 {
        let mut v = seed ^ ((gx as u64) << 32) ^ (gy as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        v = (v ^ (v >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        v = (v ^ (v >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        (v ^ (v >> 31)) % 100
    };
    h(x.div_euclid(cell), y.div_euclid(cell))
}

struct Relief {
    plot_type: PlotType,
    terrain: i16,
    feature: i16,
    resource: i16,
    rivers: u8,
    coastal: bool,
    lake: bool,
    wonder: bool,
    continent_art: u8,
}

/// One plot of the static map: a land blob in the middle, sea around, hills and mountains in patches.
fn relief(rng: &mut Rng, w: i32, h: i32, x: i32, y: i32) -> Relief {
    let dx = (x - w / 2) as f64 / (w as f64 * 0.42);
    let dy = (y - h / 2) as f64 / (h as f64 * 0.42);
    let d = (dx * dx + dy * dy).sqrt() + (patch(7, x, y, 6) as f64 - 50.0) / 220.0;
    let mut r = Relief {
        plot_type: PlotType::Water,
        terrain: 5,
        feature: -1,
        resource: -1,
        rivers: 0,
        coastal: false,
        lake: false,
        wonder: false,
        continent_art: 1,
    };
    if d > 1.0 {
        r.coastal = d < 1.12;
        r.terrain = if d < 1.12 { 6 } else { 5 };
        return r;
    }
    let p = patch(3, x, y, 5);
    r.terrain = match p {
        0..=29 => 0,  // grass
        30..=54 => 1, // plains
        55..=69 => 2, // desert
        70..=84 => 3, // tundra
        _ => 4,       // snow
    };
    let q = patch(11, x, y, 4);
    r.plot_type = match q {
        0..=14 => PlotType::Hill,
        15..=21 => PlotType::Mountain,
        _ => PlotType::Land,
    };
    if r.plot_type == PlotType::Land {
        if rng.chance(22) {
            r.feature = (7 + rng.below(3)) as i16; // forest, jungle, marsh
        }
        if rng.chance(2) {
            r.wonder = true;
        }
    }
    if rng.chance(14) {
        r.resource = rng.below(40) as i16;
    }
    if rng.chance(12) {
        r.rivers = (1 << rng.below(6)) as u8 | (1 << rng.below(6)) as u8;
    }
    r.coastal = d > 0.93;
    r.lake = rng.chance(1);
    r.continent_art = if x < w / 2 { 1 } else { 2 };
    r
}

struct Synth {
    view: WorldView,
    basis: MapBasis,
}

/// `revealed_pct` of the plots are revealed (a disc growing from the capital), a third of them visible now.
fn synth(w: i32, h: i32, revealed_pct: u64, units: usize, cities: usize, seed: u64) -> Synth {
    let mut rng = Rng(seed);
    let n = (w * h) as usize;
    let (cx, cy) = (w / 2, h / 2);
    // Reveal order: by distance from the centre, so any percent is a contiguous disc.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&i| {
        let (x, y) = ((i as i32) % w, (i as i32) / w);
        let (dx, dy) = (x - cx, (y - cy) * 2);
        dx * dx + dy * dy
    });
    let revealed_n = n * revealed_pct as usize / 100;
    let mut is_revealed = vec![false; n];
    for &i in order.iter().take(revealed_n) {
        is_revealed[i] = true;
    }
    let mut plots = Vec::with_capacity(n);
    let mut basis_plots = Vec::with_capacity(n);
    for (i, &revealed) in is_revealed.iter().enumerate() {
        let (x, y) = ((i as i32) % w, (i as i32) / w);
        let r = relief(&mut rng, w, h, x, y);
        let mut bflags = 0;
        if r.lake {
            bflags |= PlotFlags::IS_LAKE;
        }
        if r.coastal {
            bflags |= PlotFlags::IS_COASTAL;
        }
        if r.wonder {
            bflags |= PlotFlags::NATURAL_WONDER;
        }
        basis_plots.push(PlotBasisView {
            plot_type: r.plot_type,
            terrain: r.terrain,
            continent_art: r.continent_art,
            feature: if r.wonder { 20 } else { -1 },
            flags: PlotFlags(bflags),
            rivers: r.rivers,
            river_flow: if r.rivers != 0 {
                [rng.below(6) as i8, -1, rng.below(6) as i8]
            } else {
                [-1, -1, -1]
            },
        });
        if !revealed {
            plots.push(PlotView::default());
            continue;
        }
        let owned = r.plot_type != PlotType::Water && patch(5, x, y, 9) < 40;
        let mut flags = PlotFlags::REVEALED | bflags;
        if rng.chance(33) {
            flags |= PlotFlags::VISIBLE;
        }
        let land = r.plot_type != PlotType::Water;
        plots.push(PlotView {
            flags: PlotFlags(flags),
            plot_type: r.plot_type,
            terrain: r.terrain,
            feature: r.feature,
            resource: r.resource,
            improvement: if owned && rng.chance(25) {
                rng.below(12) as i16
            } else {
                -1
            },
            route: if owned && rng.chance(15) { 0 } else { -1 },
            owner: if owned {
                (patch(5, x, y, 9) % 8) as i8
            } else {
                -1
            },
            continent_art: r.continent_art,
            rivers: r.rivers,
            yields: if land {
                [
                    rng.below(4) as i8,
                    rng.below(3) as i8,
                    rng.below(3) as i8,
                    0,
                    0,
                    0,
                ]
            } else {
                [1, 0, rng.below(2) as i8, 0, 0, 0]
            },
            building: if owned && rng.chance(1) { 3 } else { -1 },
            resource_qty: if r.resource >= 0 {
                1 + rng.below(6) as i8
            } else {
                0
            },
        });
    }

    let names = [
        "Warrior",
        "Archer",
        "Settler",
        "Worker",
        "Horseman",
        "Swordsman",
    ];
    let unit_views: Vec<UnitView> = (0..units)
        .map(|k| {
            let ty = (k % names.len()) as i16;
            UnitView {
                id: k as i32,
                owner: (k % 8) as i8,
                unit_type: ty,
                x: rng.below(w as u64) as i16,
                y: rng.below(h as u64) as i16,
                hp: 100,
                max_hp: 100,
                moves_left: 120,
                max_moves: 120,
                strength: 8 + ty,
                ranged: 0,
                range: 0,
                domain: 0,
                activity: 0,
                level: 1,
                experience: rng.below(30) as i16,
                flags: UnitFlags(UnitFlags::CAN_MOVE | UnitFlags::NEEDS_ORDERS),
                name: names[ty as usize].into(),
                facing: rng.below(6) as i8,
                cargo_of: -1,
                building: -1,
            }
        })
        .collect();
    let city_views: Vec<CityView> = (0..cities)
        .map(|k| CityView {
            id: k as i32,
            owner: (k % 8) as i8,
            x: rng.below(w as u64) as i16,
            y: rng.below(h as u64) as i16,
            name: format!("City of Number {k}"),
            population: 1 + rng.below(12) as i16,
            flags: CityFlags(if k < 8 { CityFlags::CAPITAL } else { 0 }),
            hp: 200,
            max_hp: 200,
            defense: 600,
            production_order: 1,
            production_item: rng.below(60) as i16,
            production_turns: rng.below(30) as i16,
            production_stored: rng.below(300) as i32,
            production_needed: 300,
            growth_turns: rng.below(20) as i16,
            yields: [
                rng.below(20) as i16,
                rng.below(15) as i16,
                rng.below(15) as i16,
                rng.below(10) as i16,
                rng.below(5) as i16,
                0,
            ],
            buildings: (0..rng.below(5)).map(|b| (b * 7) as i16).collect(),
            founder: (k % 8) as i8,
        })
        .collect();
    let players: Vec<PlayerView> = (0..8)
        .map(|k| PlayerView {
            id: k,
            team: k,
            civ_type: k as i16,
            leader_type: k as i16,
            flags: PlayerFlags(PlayerFlags::ALIVE | PlayerFlags::MET),
            color_primary: 0xFF00_00FF ^ (k as u32 * 0x0011_2233),
            color_secondary: 0xFFFF_FFFF,
            name: format!("Leader {k}"),
            civ_name: format!("Civilization {k}"),
            era: 2,
            art_techs: 1,
        })
        .collect();

    let view = WorldView {
        turn: 120,
        year: -500,
        human: 0,
        human_team: 0,
        status: TurnStatus::HumanTurn,
        blocker: Some(Blocker {
            kind: 3,
            target: BlockerTarget::Unit(4),
        }),
        map: MapView {
            width: w as u16,
            height: h as u16,
            wrap_x: true,
            wrap_y: false,
            plots,
            terrain_seed: 12345,
        },
        units: unit_views,
        cities: city_views,
        players,
        top_bar: TopBar {
            gold: 321,
            gold_per_turn: 12,
            science_per_turn: 40,
            ..TopBar::default()
        },
        notifications: Vec::new(),
        peace_offers: Vec::new(),
        trade_routes: Vec::new(),
    };
    let mut basis = MapBasis {
        game: 1,
        basis_hash: 0,
        width: w as u16,
        height: h as u16,
        wrap_x: true,
        wrap_y: false,
        terrain_seed: 12345,
        plots: basis_plots,
    };
    basis.basis_hash = basis_hash_of(&basis);
    Synth { view, basis }
}

/// Body of the cold-join batch (net-protocol 4.4): `MapBasis`, `Keyframe` and a `TurnState`, as one
/// `GameBatch`, with the real hashes.
fn cold_body(s: &Synth) -> Vec<u8> {
    let batch = GameBatch {
        seq: 1,
        msgs: vec![
            GameMsg::MapBasis(s.basis.clone()),
            GameMsg::Keyframe(Keyframe {
                view: s.view.clone(),
                hash: ViewHash::of(&s.view).value(),
                basis_hash: s.basis.basis_hash,
            }),
            GameMsg::TurnState(TurnState {
                turn: s.view.turn,
                phase: TurnPhase::Humans,
                mode: TurnMode::Hybrid,
                seats: Vec::new(),
                timers: Vec::new(),
                paused: false,
            }),
        ],
    };
    Postcard.encode(&batch).unwrap()
}

/// What the worker writes to its stdout for a seat on a cold start: `Basis` and `Full` (uncompressed, the
/// IPC is a local pipe). Printed for the record.
fn ipc_start_body(s: &Synth) -> Vec<u8> {
    let frame = WorkerFrame {
        msgs: vec![
            WorkerMsg::Basis {
                players: vec![0],
                basis: s.basis.clone(),
            },
            WorkerMsg::View {
                player: 0,
                update: ViewUpdate::Full {
                    rev: 1,
                    view: s.view.clone(),
                    hash: ViewHash::of(&s.view).value(),
                },
            },
        ],
    };
    Postcard.encode(&frame).unwrap()
}

/// Share of revealed and of visible plots, percent.
fn shares(v: &WorldView) -> (usize, usize) {
    let n = v.map.plots.len().max(1);
    let rev = v
        .map
        .plots
        .iter()
        .filter(|p| p.flags.0 & PlotFlags::REVEALED != 0)
        .count();
    let vis = v
        .map
        .plots
        .iter()
        .filter(|p| p.flags.0 & PlotFlags::VISIBLE != 0)
        .count();
    (rev * 100 / n, vis * 100 / n)
}

struct Row {
    label: String,
    raw: usize,
    zipped: usize,
}

fn measure(label: &str, raw: &[u8]) -> Row {
    let framed = Encoder::default().frame(raw);
    Row {
        label: label.into(),
        raw: raw.len(),
        zipped: framed.len(),
    }
}

fn print(rows: &[Row]) {
    println!(
        "{:<46} {:>10} {:>10} {:>7}",
        "case", "raw B", "frame B", "ratio"
    );
    for r in rows {
        println!(
            "{:<46} {:>10} {:>10} {:>6.2}x",
            r.label,
            r.raw,
            r.zipped,
            r.raw as f64 / r.zipped as f64
        );
    }
}

/// Rows of one map size; `overs` collects the keyframes above `limit`.
fn run_map(
    name: &str,
    w: i32,
    h: i32,
    units: usize,
    cities: usize,
    limit: usize,
    overs: &mut Vec<String>,
) -> Vec<Row> {
    let mut rows = Vec::new();
    for pct in [30u64, 60, 100] {
        let s = synth(w, h, pct, units, cities, 0x1234_5678 + pct);
        let (rev, vis) = shares(&s.view);
        println!("{name}: {pct}% asked, {rev}% of plots revealed, {vis}% visible");
        let raw = Postcard.encode(&s.view).unwrap();
        let row = measure(&format!("{name} keyframe, no basis, {pct}% revealed"), &raw);
        if row.zipped > limit {
            overs.push(format!(
                "{}: {} B > limit {} B",
                row.label, row.zipped, limit
            ));
        }
        rows.push(row);
    }
    let s = synth(w, h, 100, units, cities, 0x9999);
    rows.push(measure(
        &format!("{name} basis (MapBasis)"),
        &Postcard.encode(&s.basis).unwrap(),
    ));
    for pct in [30u64, 100] {
        let s = synth(w, h, pct, units, cities, 0x1234_5678 + pct);
        rows.push(measure(
            &format!("{name} cold join (basis+keyframe), {pct}%"),
            &cold_body(&s),
        ));
        // IPC frame: the pipe does not compress; the raw size is the number.
        let ipc = ipc_start_body(&s);
        rows.push(Row {
            label: format!("{name} IPC start frame (Basis+Full), {pct}%, uncompressed"),
            raw: ipc.len(),
            zipped: ipc.len(),
        });
    }
    rows
}

#[test]
fn print_sizes() {
    let mut rows = Vec::new();
    let mut overs = Vec::new();
    rows.extend(run_map(
        "Huge 128x80 (10240)",
        128,
        80,
        600,
        150,
        80 * 1024,
        &mut overs,
    ));
    rows.extend(run_map(
        "Huge 82x74 (6068)",
        82,
        74,
        600,
        150,
        80 * 1024,
        &mut overs,
    ));
    rows.extend(run_map(
        "Duel 51x20 (1020)",
        51,
        20,
        30,
        8,
        15 * 1024,
        &mut overs,
    ));
    println!();
    print(&rows);
    // The compressor is data-sensitive: the spread over seeds, Huge 128x80, keyframe without the basis.
    for pct in [30u64, 60, 100] {
        let sizes: Vec<usize> = (0..6u64)
            .map(|seed| {
                let s = synth(128, 80, pct, 600, 150, 0xABCD_0000 + seed * 977 + pct);
                let raw = Postcard.encode(&s.view).unwrap();
                Encoder::default().frame(&raw).len()
            })
            .collect();
        let max = *sizes.iter().max().unwrap();
        println!(
            "Huge 128x80 keyframe {pct}% over 6 seeds: min {} max {max} B",
            sizes.iter().min().unwrap(),
        );
        if max > 80 * 1024 {
            overs.push(format!(
                "Huge 128x80 keyframe {pct}% (seed spread): {max} B > limit {} B",
                80 * 1024
            ));
        }
    }
    assert!(
        overs.is_empty(),
        "AC-N3.5 thresholds missed:
{}",
        overs.join(
            "
"
        )
    );
    // Round trip of one frame, so the measurement is of a valid frame.
    let s = synth(40, 24, 60, 30, 8, 1);
    let framed = Encoder::default().frame_msg(&s.view).unwrap();
    let back: WorldView = wire::decode(&framed, &wire::Limits::CLIENT).unwrap();
    assert_eq!(back, s.view);
}
