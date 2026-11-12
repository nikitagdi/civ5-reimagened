//! The root `production`: the reference production window of Moscow (size 14, 38 production a turn, overflow 12;
//! Machu Picchu 180/420 in 6 turns; queue Machu Picchu, University, Musketman; options 9 units, 6 buildings, 4
//! wonders, 2 processes; Great Wall locked with the core's reason; University 560 gold with 312 in the treasury),
//! the comparison across the seat's cities only for a focused option, and no reads while the window is closed.

mod common;

use common::*;
use replica::testing::{FakeRead, catalog_entry};
use replica::{BuildFlags, CatalogTable, CityBuild, CityOrder, OrderType, ReadError};
use ui2_model::r#gen::{ItemKind, Opt, Pending, ProdNow, Production, QueueRow};
use ui2_model::{AssetRef, Key, KeyedList, Text};
use ui2_source::DEFAULT_FRAME_BUDGET;
use ui2_source::pending::option_target;

// `GC_BUILD_F_*`
const WONDER: u16 = 0x0001;
const CURRENT: u16 = 0x0002;
const CAN_BUY: u16 = 0x0004;
const ENABLED: u16 = 0x0008;

const SETTLER: i16 = 0;
const CROSSBOWMAN: i16 = 12;
const TREBUCHET: i16 = 13;
const TRIREME: i16 = 14;
const GREAT_SCIENTIST: i16 = 15;
const MISSIONARY: i16 = 16;
const MUSKETMAN: i16 = 17;
const TEMPLE: i16 = 5;
const CASTLE: i16 = 6;
const HARBOR: i16 = 7;
const BANK: i16 = 8;
const WORKSHOP: i16 = 9;
const GREAT_WALL: i16 = 41;
const HAGIA_SOPHIA: i16 = 42;
const LEANING_TOWER: i16 = 43;
const WEALTH: i16 = 0;
const RESEARCH: i16 = 1;

fn item(order: OrderType, id: i16) -> Key {
    Key::from_u32((order.abi().cast_unsigned() << 16) | u32::from(id.cast_unsigned()))
}

fn build(order: OrderType, item: i16, turns: Option<i16>, flags: u16, cost: i32) -> CityBuild {
    CityBuild {
        order,
        item,
        turns,
        flags: BuildFlags(flags),
        production_cost: cost,
        gold_cost: None,
        faith_cost: None,
        reason: None,
    }
}

/// Moscow's options at 38 production a turn, in the core's order (units, buildings and wonders, processes).
fn moscow_builds() -> Vec<CityBuild> {
    use OrderType::{Construct, Maintain, Train};
    let unit = |id, turns: i16| build(Train, id, Some(turns), ENABLED, i32::from(turns) * 38);
    let mut worker = unit(WORKER, 2);
    worker.flags = BuildFlags(ENABLED | CAN_BUY);
    worker.gold_cost = Some(120);
    let mut scientist = build(Train, GREAT_SCIENTIST, None, 0, 0);
    scientist.faith_cost = Some(1000);
    scientist.reason = Some("Purchase with faith only".into());
    let building =
        |id, turns: i16| build(Construct, id, Some(turns), ENABLED, i32::from(turns) * 38);
    let mut university = building(UNIVERSITY, 7);
    university.production_cost = 240;
    university.flags = BuildFlags(ENABLED | CAN_BUY);
    university.gold_cost = Some(560);
    let mut bank = build(Construct, BANK, Some(5), 0, 200);
    bank.reason = Some("Needs Banking".into());
    let mut machu = build(
        Construct,
        MACHU_PICCHU,
        Some(6),
        WONDER | ENABLED | CURRENT,
        420,
    );
    machu.gold_cost = None;
    let mut wall = build(Construct, GREAT_WALL, Some(8), WONDER, 300);
    wall.reason = Some("Built by Harun al-Rashid".into());
    let mut tower = build(Construct, LEANING_TOWER, Some(10), WONDER, 380);
    tower.reason = Some("Needs Physics".into());
    vec![
        unit(SETTLER, 4),
        worker,
        unit(KNIGHT, 5),
        unit(LONGSWORDSMAN, 5),
        unit(CROSSBOWMAN, 4),
        unit(TREBUCHET, 5),
        unit(TRIREME, 2),
        scientist,
        unit(MISSIONARY, 3),
        building(TEMPLE, 3),
        building(CASTLE, 5),
        building(HARBOR, 3),
        bank,
        building(WORKSHOP, 6),
        university,
        machu,
        wall,
        build(Construct, HAGIA_SOPHIA, Some(9), WONDER | ENABLED, 340),
        tower,
        build(Maintain, WEALTH, None, ENABLED, 0),
        build(Maintain, RESEARCH, None, ENABLED, 0),
    ]
}

fn reference() -> FakeRead {
    let mut f = game();
    f.top.gold = 312;
    f.top.faith = 140;
    f.builds.insert(MOSCOW, moscow_builds());
    let uni = |turns, flags| build(OrderType::Construct, UNIVERSITY, Some(turns), flags, 240);
    f.builds.insert(
        NOVGOROD,
        vec![
            build(
                OrderType::Train,
                LONGSWORDSMAN,
                Some(2),
                ENABLED | CURRENT,
                120,
            ),
            uni(12, ENABLED),
        ],
    );
    f.builds.insert(
        KAZAN,
        vec![
            build(
                OrderType::Construct,
                GRANARY,
                Some(3),
                ENABLED | CURRENT,
                60,
            ),
            uni(18, ENABLED),
        ],
    );
    let order = |slot, order, item, turns, needed| CityOrder {
        slot,
        order,
        item,
        turns: Some(turns),
        needed,
    };
    f.queues.insert(
        MOSCOW,
        vec![
            order(0, OrderType::Construct, MACHU_PICCHU, 6, 420),
            order(1, OrderType::Construct, UNIVERSITY, 13, 240),
            order(2, OrderType::Train, MUSKETMAN, 16, 160),
        ],
    );
    let units = f
        .catalogs
        .iter_mut()
        .find(|(t, _)| *t == CatalogTable::Units)
        .expect("units");
    units.1.push(catalog_entry(
        MUSKETMAN,
        "UNIT_MUSKETMAN",
        "Musketman",
        "UNIT_ATLAS_1",
        30,
    ));
    units.1.push(catalog_entry(
        SETTLER,
        "UNIT_SETTLER",
        "Settler",
        "UNIT_ATLAS_1",
        0,
    ));
    let buildings = f
        .catalogs
        .iter_mut()
        .find(|(t, _)| *t == CatalogTable::Buildings)
        .expect("buildings");
    buildings.1.push(catalog_entry(
        GREAT_WALL,
        "BUILDING_GREAT_WALL",
        "Great Wall",
        "BW_ATLAS_2",
        5,
    ));
    f
}

fn open(rig: &mut Rig, city: i32) {
    rig.ui.windows.production.open = true;
    rig.ui.windows.production.city = Some(Key::new(0, city.cast_unsigned()));
}

fn production(rig: &Rig) -> Option<Production> {
    rig.store
        .get::<Option<Production>>()
        .cloned()
        .expect("production is written")
}

#[test]
fn the_reference_window_of_moscow() {
    let mut f = reference();
    let mut rig = Rig::new(session());
    rig.start(&mut f);
    assert_eq!(production(&rig), None, "closed");

    open(&mut rig, MOSCOW);
    let stats = rig.frame(&mut f);
    assert_eq!(stats.roots_built, 1);
    assert_eq!(
        f.calls,
        ["city_builds", "city_queue"],
        "the cities and the top bar were read already; no comparison without a focus"
    );
    let p = production(&rig).expect("open");
    assert_eq!(p.city, Key::new(0, 1));
    assert_eq!((&*p.name, p.capital, p.size), ("Moscow", true, 14));
    assert_eq!(
        (p.rate, p.overflow, p.gold, p.faith),
        (38, Some(12), 312, 140)
    );
    assert_eq!(
        p.current,
        Some(ProdNow {
            item: item(OrderType::Construct, MACHU_PICCHU),
            name: Text::raw("Machu Picchu"),
            kind: ItemKind::Wonder,
            stored: 180,
            needed: 420,
            turns: Some(6),
            fraction: 0.429,
        })
    );

    assert!(p.queue_known);
    let q: Vec<(i32, Text, ItemKind, bool)> = p
        .queue
        .iter()
        .map(|r| (r.slot, r.name.clone(), r.kind, r.first))
        .collect();
    assert_eq!(
        q,
        [
            (0, Text::raw("Machu Picchu"), ItemKind::Wonder, true),
            (1, Text::raw("University"), ItemKind::Building, false),
            (2, Text::raw("Musketman"), ItemKind::Unit, false),
        ]
    );

    let counts: Vec<(ItemKind, i32)> = p.groups.iter().map(|g| (g.kind, g.count)).collect();
    assert_eq!(
        counts,
        [
            (ItemKind::Unit, 9),
            (ItemKind::Building, 6),
            (ItemKind::Wonder, 4),
            (ItemKind::Process, 2),
        ]
    );

    let opt = |kind: ItemKind, key: Key| -> Opt {
        p.groups
            .get(Key::from_enum(kind as u16))
            .and_then(|g| g.options.get(key))
            .cloned()
            .expect("an option")
    };
    let university = opt(ItemKind::Building, item(OrderType::Construct, UNIVERSITY));
    assert_eq!(
        university,
        Opt {
            item: item(OrderType::Construct, UNIVERSITY),
            order: 14,
            name: Text::raw("University"),
            icon: AssetRef::new("icon:BW_ATLAS_1:20"),
            turns: Some(7),
            cost: 240,
            enabled: true,
            current: false,
            queued: true,
            locked_reason: None,
            effect: None,
            buy_gold: Some(560),
            buy_gold_ok: false,
            buy_faith: None,
            buy_faith_ok: false,
            pending: false,
        }
    );
    let worker = opt(ItemKind::Unit, item(OrderType::Train, WORKER));
    assert_eq!(
        (worker.buy_gold, worker.buy_gold_ok, worker.queued),
        (Some(120), true, false)
    );
    let wall = opt(ItemKind::Wonder, item(OrderType::Construct, GREAT_WALL));
    assert_eq!(
        (wall.enabled, wall.locked_reason.as_deref()),
        (false, Some("Built by Harun al-Rashid")),
        "the core's own text"
    );
    let bank = opt(ItemKind::Building, item(OrderType::Construct, BANK));
    assert_eq!(bank.locked_reason.as_deref(), Some("Needs Banking"));
    let machu = opt(ItemKind::Wonder, item(OrderType::Construct, MACHU_PICCHU));
    assert!(machu.current && machu.queued && machu.enabled);
    let scientist = opt(ItemKind::Unit, item(OrderType::Train, GREAT_SCIENTIST));
    assert_eq!(
        (scientist.buy_faith, scientist.buy_faith_ok),
        (Some(1000), false)
    );
    let wealth = opt(ItemKind::Process, item(OrderType::Maintain, WEALTH));
    assert_eq!((wealth.name, wealth.turns), (Text::raw("Process 0"), None));
    assert_eq!(p.detail, None);
}

#[test]
fn the_focused_option_compares_the_seats_cities() {
    let mut f = reference();
    let mut rig = Rig::new(session());
    open(&mut rig, MOSCOW);
    rig.start(&mut f);

    rig.ui.production.focus = Some(item(OrderType::Construct, UNIVERSITY));
    let stats = rig.frame(&mut f);
    assert_eq!(stats.roots_built, 1);
    assert_eq!(
        f.calls,
        ["city_builds", "city_builds"],
        "Novgorod and Kazan; Moscow's options were read already"
    );
    let d = production(&rig).and_then(|p| p.detail).expect("a detail");
    assert_eq!(d.name, Text::raw("University"));
    assert_eq!(
        (d.kind, d.turns, d.cost),
        (ItemKind::Building, Some(7), 240)
    );
    assert_eq!(
        d.short_gold,
        Some(248),
        "560 gold costs, 312 in the treasury"
    );
    assert_eq!((d.help.clone(), d.maintenance), (None, None));
    assert!(d.requires.is_empty());
    let cmp: Vec<(&str, Option<i16>, f32)> = d
        .compare
        .iter()
        .map(|c| (&*c.name, c.turns, c.fraction))
        .collect();
    assert_eq!(
        cmp,
        [
            ("Moscow", Some(7), 0.389),
            ("Novgorod", Some(12), 0.667),
            ("Kazan", Some(18), 1.0),
        ]
    );

    // Another focus reads nothing more: every city's options are kept until the game changes.
    f.calls.clear();
    rig.ui.production.focus = Some(item(OrderType::Construct, MACHU_PICCHU));
    rig.frame(&mut f);
    assert!(f.calls.is_empty());
    let d = production(&rig).and_then(|p| p.detail).expect("a detail");
    assert_eq!(d.compare.len(), 1, "only Moscow builds the wonder");
    assert_eq!(d.short_gold, None, "a wonder cannot be bought");

    // An item the city does not offer: no detail.
    rig.ui.production.focus = Some(item(OrderType::Train, MUSKETMAN));
    rig.frame(&mut f);
    assert_eq!(production(&rig).and_then(|p| p.detail), None);
}

#[test]
fn a_closed_window_reads_no_options() {
    let mut f = reference();
    let mut rig = Rig::new(session());
    rig.frame(&mut f);
    for _ in 0..3 {
        rig.batch(&mut f);
    }
    assert_eq!(production(&rig), None);
    assert!(!f.calls.contains(&"city_builds") && !f.calls.contains(&"city_queue"));

    // Open, then close: the root goes back to none without a read.
    open(&mut rig, MOSCOW);
    rig.frame(&mut f);
    f.calls.clear();
    rig.ui.windows.production.open = false;
    rig.frame(&mut f);
    assert_eq!(production(&rig), None);
    assert!(f.calls.is_empty());
}

#[test]
fn a_foreign_or_unknown_city_has_no_window() {
    let mut f = reference();
    let mut rig = Rig::new(session());
    rig.ui.windows.production.open = true;
    rig.ui.windows.production.city = Some(Key::new(u32::from(HARUN.cast_unsigned()), 1));
    rig.frame(&mut f);
    assert_eq!(production(&rig), None);
    assert!(!f.calls.contains(&"city_builds"));
    open(&mut rig, 99);
    rig.frame(&mut f);
    assert_eq!(production(&rig), None);
}

#[test]
fn a_batch_rebuilds_the_open_window() {
    let mut f = reference();
    let mut rig = Rig::new(session());
    open(&mut rig, MOSCOW);
    rig.start(&mut f);
    f.top.gold = 600;
    f.cities[0].production_stored = 218;
    rig.batch(&mut f);
    let p = production(&rig).expect("open");
    assert_eq!(p.gold, 600);
    assert_eq!(p.current.map(|c| c.stored), Some(218));
    let uni = p
        .groups
        .iter()
        .flat_map(|g| g.options.iter())
        .find(|o| o.item == item(OrderType::Construct, UNIVERSITY))
        .cloned()
        .expect("university");
    assert!(uni.buy_gold_ok, "600 gold buys it now");
    assert_eq!(f.calls.iter().filter(|c| **c == "city_builds").count(), 1);
}

#[test]
fn a_core_without_the_queue_shows_the_head_only() {
    let mut f = reference();
    f.ui2_error = Some(ReadError::Unsupported);
    let mut rig = Rig::new(session());
    open(&mut rig, MOSCOW);
    rig.frame(&mut f);
    let p = production(&rig).expect("open");
    assert!(!p.queue_known);
    assert_eq!(
        p.queue,
        KeyedList::from_vec(vec![QueueRow {
            slot: 0,
            item: item(OrderType::Construct, MACHU_PICCHU),
            name: Text::raw("Machu Picchu"),
            kind: ItemKind::Wonder,
            turns: Some(6),
            first: true,
        }])
        .unwrap()
    );
    let queued: Vec<Key> = p
        .groups
        .iter()
        .flat_map(|g| g.options.iter())
        .filter(|o| o.queued)
        .map(|o| o.item)
        .collect();
    assert_eq!(queued, [item(OrderType::Construct, MACHU_PICCHU)]);
    f.calls.clear();
    rig.batch(&mut f);
    assert!(!f.calls.contains(&"city_queue"), "not asked again");
}

/// The reference game with seven more cities of the seat (ten in all), each able to build a University.
fn ten_cities() -> FakeRead {
    let mut f = reference();
    for id in 4..=10 {
        let x = i16::try_from(id).unwrap();
        f.cities.push(city(
            id,
            CATHERINE,
            &format!("City {id}"),
            4,
            x,
            40,
            C_VISIBLE,
        ));
        f.builds.insert(
            id,
            vec![build(
                OrderType::Construct,
                UNIVERSITY,
                Some(10 + x),
                ENABLED,
                240,
            )],
        );
    }
    f
}

fn compare_rows(rig: &Rig) -> Vec<(String, Option<i16>, f32)> {
    production(rig)
        .and_then(|p| p.detail)
        .map(|d| {
            d.compare
                .iter()
                .map(|c| (c.name.to_string(), c.turns, c.fraction))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn the_comparison_fills_one_city_a_frame() {
    // What one unlimited frame shows.
    let mut f = ten_cities();
    let mut whole = Rig::new(session());
    whole.start(&mut f);
    open(&mut whole, MOSCOW);
    whole.ui.production.focus = Some(item(OrderType::Construct, UNIVERSITY));
    whole.frame(&mut f);
    let expected = compare_rows(&whole);
    assert_eq!(expected.len(), 10);

    let mut f = ten_cities();
    let mut rig = Rig::new(session());
    rig.start(&mut f);
    rig.src.set_read_budget(DEFAULT_FRAME_BUDGET);
    open(&mut rig, MOSCOW);
    rig.ui.production.focus = Some(item(OrderType::Construct, UNIVERSITY));

    let mut frames = 0;
    let mut rows = 0;
    loop {
        f.calls.clear();
        let stats = rig.frame(&mut f);
        frames += 1;
        let builds = f.calls.iter().filter(|c| **c == "city_builds").count();
        assert!(builds <= 1, "one city's options a frame: {:?}", f.calls);
        assert_eq!(stats.roots_built, 1, "the window, whole, every frame");
        let p = production(&rig).expect("open");
        // The window's own city comes first and complete; the comparison grows by one city a frame.
        assert_eq!(p.groups.len(), 4);
        assert_eq!(p.queue.len(), 3);
        let now = compare_rows(&rig);
        assert_eq!(now.len(), rows + 1, "frame {frames}: {now:?}");
        rows = now.len();
        if stats.deferred == 0 {
            break;
        }
        assert!(frames < 20, "the comparison never ends");
    }
    assert!(frames >= 10, "{frames}");
    assert_eq!(
        compare_rows(&rig),
        expected,
        "the same as one unlimited frame"
    );

    // Done: nothing is read or written any more.
    rig.delta();
    for _ in 0..5 {
        f.calls.clear();
        let stats = rig.frame(&mut f);
        assert_eq!(stats, ui2_source::SourceStats::default());
        assert!(f.calls.is_empty());
    }
    assert!(rig.delta().is_none());

    // The game changes in the middle of a new sweep: the window is rebuilt from the new state, one city a frame.
    rig.batch(&mut f);
    rig.frame(&mut f);
    f.builds.get_mut(&10).unwrap()[0].turns = Some(3);
    let mut frames = 0;
    let mut stats = rig.batch(&mut f);
    while stats.deferred > 0 {
        stats = rig.frame(&mut f);
        frames += 1;
        assert!(frames < 25);
    }
    let last = compare_rows(&rig);
    assert_eq!(last.len(), 10);
    assert_eq!(last[9].1, Some(3));
}

#[test]
fn the_open_window_does_not_share_a_frame() {
    let mut f = reference();
    let mut rig = Rig::new(session());
    open(&mut rig, MOSCOW);
    rig.start(&mut f);
    rig.src.set_read_budget(DEFAULT_FRAME_BUDGET);

    // A batch: the other roots first; the window waits for the next frame and has it alone.
    f.calls.clear();
    let stats = rig.batch(&mut f);
    assert!(!f.calls.contains(&"city_builds"), "{:?}", f.calls);
    assert!(stats.spent_units <= DEFAULT_FRAME_BUDGET);
    assert_eq!(stats.deferred, 1, "the window");
    f.calls.clear();
    let stats = rig.frame(&mut f);
    assert_eq!(stats.roots_built, 1);
    assert_eq!(f.calls, ["city_builds", "city_queue"]);
    assert_eq!(stats.deferred, 0);

    // Batches every frame: the roots on screen are rebuilt in each, and the window is not starved: once it has
    // waited `STALE_FRAMES` frames it gets its reads with them.
    let mut window_frames = 0;
    for _ in 0..6 {
        f.calls.clear();
        rig.batch(&mut f);
        assert!(f.calls.contains(&"top_bar"), "{:?}", f.calls);
        if f.calls.contains(&"city_builds") {
            window_frames += 1;
        }
    }
    assert_eq!(
        window_frames,
        6 / (1 + usize::from(ui2_source::STALE_FRAMES))
    );
}

#[test]
fn an_order_waiting_for_its_answer_marks_the_option_without_reading() {
    let mut f = reference();
    let mut rig = Rig::new(session());
    open(&mut rig, MOSCOW);
    rig.start(&mut f);
    let uni = item(OrderType::Construct, UNIVERSITY);
    rig.ui
        .pending
        .try_push(Pending {
            seq: 3,
            intent: "city.buy".into(),
            target: option_target(Key::new(0, 1), uni).into(),
            since_ms: 0,
        })
        .unwrap();
    rig.frame(&mut f);
    assert!(f.calls.is_empty());
    let marked: Vec<Key> = production(&rig)
        .expect("open")
        .groups
        .iter()
        .flat_map(|g| g.options.iter())
        .filter(|o| o.pending)
        .map(|o| o.item)
        .collect();
    assert_eq!(marked, [uni]);
    let d = rig.delta().expect("a delta");
    assert_eq!(d.ops.len(), 1);
}
