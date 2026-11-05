//! Properties of the view operations (net-protocol 4.6, 9): `diff`, `apply`, `UndoLog`, the hashes and the
//! map basis, on 10 000 random pairs of views. Own xorshift generator, no test dependencies.

use protocol::*;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
    fn i16(&mut self) -> i16 {
        self.next() as i16
    }
    fn small(&mut self, n: i64) -> i16 {
        (self.below(n as u64) as i64 - 1) as i16
    }
}

fn plot_type(r: &mut Rng) -> PlotType {
    match r.below(4) {
        0 => PlotType::Land,
        1 => PlotType::Hill,
        2 => PlotType::Mountain,
        _ => PlotType::Water,
    }
}

fn gen_plot(r: &mut Rng) -> PlotView {
    if r.chance(40) {
        return PlotView::default();
    }
    PlotView {
        flags: PlotFlags(r.next() as u16 & 0x1ff),
        plot_type: plot_type(r),
        terrain: r.small(8),
        feature: r.small(6),
        resource: r.small(10),
        improvement: r.small(10),
        route: r.small(3),
        owner: r.small(5) as i8,
        continent_art: r.below(4) as u8,
        rivers: r.below(64) as u8,
        yields: [r.small(5) as i8; 6],
        building: r.small(4),
        resource_qty: r.small(5) as i8,
    }
}

fn gen_basis_plot(r: &mut Rng) -> PlotBasisView {
    PlotBasisView {
        plot_type: plot_type(r),
        terrain: r.small(8),
        continent_art: r.below(4) as u8,
        feature: r.small(6),
        flags: PlotFlags(r.next() as u16 & 0x58),
        rivers: r.below(64) as u8,
        river_flow: [r.small(6) as i8, r.small(6) as i8, r.small(6) as i8],
    }
}

fn gen_unit(r: &mut Rng, owner: PlayerId, id: i32) -> UnitView {
    UnitView {
        id,
        owner,
        unit_type: r.small(20),
        x: r.small(8),
        y: r.small(8),
        hp: r.small(100),
        max_hp: 100,
        moves_left: r.i16() % 120,
        max_moves: 120,
        strength: r.small(30),
        ranged: r.small(30),
        range: r.small(3) as i8,
        domain: r.small(3) as i8,
        activity: r.small(8) as i8,
        level: r.small(4) as i8,
        experience: r.small(50),
        flags: UnitFlags(r.next() as u16 & 0x1ff),
        name: if r.chance(30) {
            String::new()
        } else {
            format!("unit {}", r.below(100))
        },
        facing: r.small(7) as i8,
        cargo_of: r.small(5) as i32,
        building: r.small(4),
    }
}

fn gen_city(r: &mut Rng, owner: PlayerId, id: i32) -> CityView {
    CityView {
        id,
        owner,
        x: r.small(8),
        y: r.small(8),
        name: format!("city {}", r.below(50)),
        population: r.small(20),
        flags: CityFlags(r.next() as u16 & 0x1f),
        hp: r.small(200),
        max_hp: 200,
        defense: r.small(500),
        production_order: r.small(4) as i8,
        production_item: r.small(30),
        production_turns: r.small(30),
        production_stored: r.small(500) as i32,
        production_needed: r.small(500) as i32,
        growth_turns: r.small(30),
        yields: [r.small(10); 6],
        buildings: (0..r.below(4)).map(|_| r.small(30)).collect(),
        founder: r.small(5) as i8,
    }
}

fn gen_player(r: &mut Rng, id: PlayerId) -> PlayerView {
    PlayerView {
        id,
        team: id,
        civ_type: r.small(10),
        leader_type: r.small(10),
        flags: PlayerFlags(r.next() as u16 & 0x3f),
        color_primary: r.next() as u32,
        color_secondary: r.next() as u32,
        name: format!("p{}", r.below(10)),
        civ_name: format!("civ{}", r.below(10)),
        era: r.small(5),
        art_techs: r.below(3),
    }
}

fn gen_notification(r: &mut Rng, id: i32) -> NotificationView {
    NotificationView {
        id,
        kind: r.below(20) as u32,
        at: if r.chance(50) {
            None
        } else {
            Some((r.small(8), r.small(8)))
        },
        data1: r.small(5) as i32,
        data2: r.small(5) as i32,
        turn: r.small(50),
        dismissed: r.chance(30),
        message: format!("m{}", r.below(10)),
        summary: String::new(),
    }
}

fn gen_top_bar(r: &mut Rng) -> TopBar {
    TopBar {
        gold: r.small(1000) as i32,
        gold_per_turn: r.small(50) as i32,
        science_per_turn: r.small(50) as i32,
        research_tech: r.small(30),
        research_turns: r.small(30),
        research_progress: r.small(500) as i32,
        research_cost: r.small(500) as i32,
        culture: r.small(500) as i32,
        culture_per_turn: r.small(20) as i32,
        next_policy_cost: r.small(500) as i32,
        happiness: r.small(20),
        golden_age_turns: r.small(10),
        faith: r.small(100) as i32,
        faith_per_turn: r.small(10) as i32,
        era: r.small(5),
        golden_age_progress: r.small(100) as i32,
        golden_age_threshold: r.small(100) as i32,
    }
}

fn gen_status(r: &mut Rng) -> TurnStatus {
    match r.below(3) {
        0 => TurnStatus::HumanTurn,
        1 => TurnStatus::AiTurn {
            active: r.small(5) as i8,
        },
        _ => TurnStatus::GameOver {
            winner: if r.chance(50) {
                None
            } else {
                Some(r.small(4) as i8)
            },
        },
    }
}

fn gen_blocker(r: &mut Rng) -> Option<Blocker> {
    if r.chance(60) {
        return None;
    }
    Some(Blocker {
        kind: r.small(10) as i32,
        target: match r.below(4) {
            0 => BlockerTarget::None,
            1 => BlockerTarget::Unit(r.small(10) as i32),
            2 => BlockerTarget::City(r.small(10) as i32),
            _ => BlockerTarget::Plot {
                x: r.small(8),
                y: r.small(8),
            },
        },
    })
}

fn gen_peace(r: &mut Rng) -> Vec<PeaceOffer> {
    (0..r.below(3))
        .map(|_| PeaceOffer {
            from: r.small(4) as i8,
            to: r.small(4) as i8,
            turn: r.small(50) as i32,
        })
        .collect()
}

fn gen_routes(r: &mut Rng) -> Vec<TradeRouteView> {
    (0..r.below(3))
        .map(|_| TradeRouteView {
            owner: r.small(4) as i8,
            from: (r.small(8), r.small(8)),
            to: (r.small(8), r.small(8)),
            domain: r.small(3) as i8,
        })
        .collect()
}

/// A random keyframe-ordered view of the given size.
fn gen_view(r: &mut Rng, w: u16, h: u16) -> WorldView {
    let mut units = Vec::new();
    for owner in 0..4i8 {
        let mut id = 0;
        for _ in 0..r.below(5) {
            id += 1 + r.below(3) as i32;
            units.push(gen_unit(r, owner, id));
        }
    }
    let mut cities = Vec::new();
    for owner in 0..3i8 {
        let mut id = 0;
        for _ in 0..r.below(3) {
            id += 1 + r.below(3) as i32;
            cities.push(gen_city(r, owner, id));
        }
    }
    let mut notifications = Vec::new();
    let mut id = 0;
    for _ in 0..r.below(5) {
        id += 1 + r.below(3) as i32;
        notifications.push(gen_notification(r, id));
    }
    WorldView {
        turn: r.small(300) as i32,
        year: r.small(300) as i32,
        human: r.small(4) as i8,
        human_team: r.small(4) as i8,
        status: gen_status(r),
        blocker: gen_blocker(r),
        map: MapView {
            width: w,
            height: h,
            wrap_x: r.chance(50),
            wrap_y: false,
            plots: (0..usize::from(w) * usize::from(h))
                .map(|_| gen_plot(r))
                .collect(),
            terrain_seed: 7,
        },
        units,
        cities,
        players: (0..r.below(5) as i8).map(|i| gen_player(r, i)).collect(),
        top_bar: gen_top_bar(r),
        notifications,
        peace_offers: gen_peace(r),
        trade_routes: gen_routes(r),
    }
}

/// A small random change of a view, like one step of the game.
fn mutate(r: &mut Rng, v: &WorldView) -> WorldView {
    let mut n = v.clone();
    if r.chance(30) {
        n.turn += 1;
        n.status = gen_status(r);
        n.blocker = gen_blocker(r);
    }
    for _ in 0..r.below(6) {
        let i = r.below(n.map.plots.len() as u64) as usize;
        n.map.plots[i] = gen_plot(r);
    }
    if r.chance(60) && !n.units.is_empty() {
        let i = r.below(n.units.len() as u64) as usize;
        let (o, id) = (n.units[i].owner, n.units[i].id);
        n.units[i] = gen_unit(r, o, id);
    }
    if r.chance(30) && !n.units.is_empty() {
        let i = r.below(n.units.len() as u64) as usize;
        n.units.remove(i);
    }
    if r.chance(30) {
        let owner = r.below(4) as i8;
        let id = 100 + r.below(50) as i32;
        if !n.units.iter().any(|u| (u.owner, u.id) == (owner, id)) {
            n.units.push(gen_unit(r, owner, id));
            n.units.sort_by_key(|u| (u.owner, u.id));
        }
    }
    if r.chance(30) && !n.cities.is_empty() {
        let i = r.below(n.cities.len() as u64) as usize;
        let (o, id) = (n.cities[i].owner, n.cities[i].id);
        n.cities[i] = gen_city(r, o, id);
    }
    if r.chance(10) && !n.cities.is_empty() {
        n.cities.remove(0);
    }
    if r.chance(20) {
        n.top_bar = gen_top_bar(r);
    }
    if r.chance(20) && !n.players.is_empty() {
        let i = r.below(n.players.len() as u64) as usize;
        let id = n.players[i].id;
        n.players[i] = gen_player(r, id);
    }
    if r.chance(20) {
        let id = 1000 + r.below(20) as i32;
        if !n.notifications.iter().any(|x| x.id == id) {
            n.notifications.push(gen_notification(r, id));
        }
    }
    if r.chance(20) && !n.notifications.is_empty() {
        n.notifications.remove(0);
    }
    if r.chance(15) {
        n.peace_offers = gen_peace(r);
    }
    if r.chance(15) {
        n.trade_routes = gen_routes(r);
    }
    n
}

fn basis_of(r: &mut Rng, w: u16, h: u16, seed: u32) -> BasisState {
    let mut m = MapBasis {
        game: 1,
        basis_hash: 0,
        width: w,
        height: h,
        wrap_x: true,
        wrap_y: false,
        terrain_seed: seed,
        plots: (0..usize::from(w) * usize::from(h))
            .map(|_| gen_basis_plot(r))
            .collect(),
    };
    m.basis_hash = basis_hash_of(&m);
    BasisState::new(m).expect("a basis with the right hash")
}

const PAIRS: u32 = 10_000;

#[test]
fn properties_on_random_pairs() {
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    for pair in 0..PAIRS {
        let (w, h) = (3 + r.below(5) as u16, 3 + r.below(5) as u16);
        let a = gen_view(&mut r, w, h);
        // Half of the pairs are one game step apart, half are unrelated views of the same map size.
        let mut b = if pair % 2 == 0 {
            mutate(&mut r, &a)
        } else {
            gen_view(&mut r, w, h)
        };
        // The map header never changes inside a game and is not part of a diff.
        b.map.wrap_x = a.map.wrap_x;
        assert_eq!(check(&a), Ok(()));
        assert_eq!(check(&b), Ok(()));

        let basis_a = basis_of(&mut r, w, h, 7);
        let d = diff(&a, &b, None);
        assert!(d.basis.is_empty());

        // apply(a, diff(a, b)) == b, and the hash after equals the full hash of b.
        let hash_a = ViewHash::of(&a);
        let mut v = a.clone();
        let mut hash = hash_a.clone();
        let mut basis = basis_a.clone();
        apply(&mut v, &mut hash, &mut basis, &d).expect("a diff of two valid views");
        assert_eq!(v, b, "pair {pair}");
        assert_eq!(hash, ViewHash::of(&b), "pair {pair}");
        assert_eq!(hash.value(), ViewHash::of(&b).value(), "pair {pair}");
        assert_eq!(
            basis, basis_a,
            "a diff without basis leaves the basis alone"
        );
        assert_eq!(check(&v), Ok(()));

        // diff(a, a) is empty.
        let same = diff(&a, &a, None);
        assert!(same.is_empty() && same.touched() == SectionMask(0));

        // Dirty plots: covering the changes (plus junk) gives the same diff.
        let mut dirty: Vec<u32> = (0..a.map.plots.len() as u32)
            .filter(|&i| a.map.plots[i as usize] != b.map.plots[i as usize] || r.chance(10))
            .collect();
        dirty.push(u32::MAX);
        if let Some(&first) = dirty.first() {
            dirty.push(first);
        }
        dirty.reverse();
        assert_eq!(diff(&a, &b, Some(&dirty)), d, "pair {pair}");
        // Dirty plots that miss a change leave that change out, and nothing else.
        let none = diff(&a, &b, Some(&[]));
        assert!(none.plots.is_empty());
        assert_eq!(
            (none.units, none.top_bar),
            (d.units.clone(), d.top_bar.clone())
        );

        // Undo: capture + apply + restore returns everything byte for byte.
        let mut dd = d.clone();
        let mut basis_b = basis_a.map().clone();
        let mut fresh: Vec<(u32, PlotBasisView)> = Vec::new();
        for i in 0..basis_b.plots.len() as u32 {
            if r.chance(20) {
                fresh.push((i, gen_basis_plot(&mut r)));
            }
        }
        dd.basis = diff_basis(basis_a.map(), &fresh);
        for (i, rec) in &dd.basis {
            basis_b.plots[*i as usize] = rec.clone();
        }
        let mut v2 = a.clone();
        let mut hash2 = hash_a.clone();
        let mut basis2 = basis_a.clone();
        let undo = UndoLog::capture(&v2, &hash2, &basis2, &dd);
        apply(&mut v2, &mut hash2, &mut basis2, &dd).expect("valid diff with basis");
        assert_eq!(v2, b);
        assert_eq!(hash2, ViewHash::of(&b));
        // The basis hash after basis diffs equals the full count over the rebuilt basis.
        assert_eq!(basis2.map().plots, basis_b.plots, "pair {pair}");
        assert_eq!(basis2.hash(), basis_hash_of(&basis_b), "pair {pair}");
        assert_eq!(basis2.map().basis_hash, basis2.hash());
        assert_eq!(dd.touched().contains(8), !dd.basis.is_empty());
        undo.restore(&mut v2, &mut hash2, &mut basis2);
        assert_eq!(v2, a, "pair {pair}");
        assert_eq!(hash2, hash_a);
        assert_eq!(basis2, basis_a);
        assert_eq!(basis2.hash(), basis_a.map().basis_hash);
    }
}

#[test]
fn bad_diffs_are_refused_and_change_nothing() {
    let mut r = Rng(0xDEAD_BEEF_1234_5678);
    let mut tested = 0;
    for _ in 0..2000 {
        let a = gen_view(&mut r, 5, 4);
        let b = gen_view(&mut r, 5, 4);
        let d = diff(&a, &b, None);
        let hash_a = ViewHash::of(&a);
        let basis_a = basis_of(&mut r, 5, 4, 7);
        let mut bad: Vec<(ViewDiff, ApplyError)> = Vec::new();

        if d.units.upsert.len() >= 2 {
            let mut x = d.clone();
            x.units.upsert.swap(0, 1);
            bad.push((x, ApplyError::Unsorted { section: 2 }));
        }
        if let Some(first) = d.units.upsert.first() {
            let mut x = d.clone();
            x.units.upsert.insert(0, first.clone());
            bad.push((x, ApplyError::Unsorted { section: 2 }));
        }
        if d.plots.len() >= 2 {
            let mut x = d.clone();
            x.plots.swap(0, 1);
            bad.push((x, ApplyError::Unsorted { section: 1 }));
        }
        {
            let mut x = d.clone();
            x.plots.push((20, PlotView::default()));
            bad.push((
                x,
                ApplyError::IndexOutOfRange {
                    section: 1,
                    index: 20,
                },
            ));
        }
        {
            let mut x = d.clone();
            x.basis.push((20, PlotBasisView::default()));
            bad.push((
                x,
                ApplyError::IndexOutOfRange {
                    section: 8,
                    index: 20,
                },
            ));
        }
        {
            let mut x = d.clone();
            x.cities.remove = vec![(100, 100)];
            bad.push((x, ApplyError::UnknownKey { section: 3 }));
        }
        if let Some(u) = a.units.first() {
            let mut x = d.clone();
            x.units = SectionDiff {
                upsert: vec![u.clone()],
                remove: vec![(u.owner, u.id)],
            };
            bad.push((x, ApplyError::KeyInBoth { section: 2 }));
        }
        for (x, want) in bad {
            // The valid parts of the diff come first in the section order, so a refused diff must still
            // leave the view exactly as it was.
            let mut v = a.clone();
            let mut hash = hash_a.clone();
            let mut basis = basis_a.clone();
            assert_eq!(apply(&mut v, &mut hash, &mut basis, &x), Err(want));
            assert_eq!(v, a);
            assert_eq!(hash, hash_a);
            assert_eq!(basis, basis_a);
            tested += 1;
        }
    }
    assert!(tested > 5000, "only {tested} bad diffs were generated");
}

#[test]
fn hash_mismatch_is_rolled_back_with_the_view() {
    let mut r = Rng(42);
    let a = gen_view(&mut r, 6, 5);
    let b = mutate(&mut r, &a);
    let mut d = diff(&a, &b, None);
    let basis_a = basis_of(&mut r, 6, 5, 7);
    let fresh = vec![(3, gen_basis_plot(&mut r))];
    d.basis = diff_basis(basis_a.map(), &fresh);
    assert!(!d.basis.is_empty());

    let (mut v, mut hash, mut basis) = (a.clone(), ViewHash::of(&a), basis_a.clone());
    let undo = UndoLog::capture(&v, &hash, &basis, &d);
    apply(&mut v, &mut hash, &mut basis, &d).unwrap();
    // The caller compares the message hashes; here the "message" claims other values.
    assert_ne!(hash.value(), ViewHash::of(&a).value());
    assert_ne!(basis.hash(), basis_a.hash());
    undo.restore(&mut v, &mut hash, &mut basis);
    assert_eq!(
        (v, hash, basis.clone()),
        (a.clone(), ViewHash::of(&a), basis_a.clone())
    );
    assert_eq!(basis.hash(), basis_a.hash());
}

#[test]
fn touched_names_the_sections() {
    let mut d = ViewDiff::default();
    assert_eq!(d.touched(), SectionMask(0));
    d.header = Some(header());
    d.plots.push((0, PlotView::default()));
    d.units.remove.push((0, 1));
    d.cities.upsert.push(CityView::default());
    d.players.remove.push(0);
    d.top_bar = Some(TopBar::default());
    d.notifications.remove.push(1);
    d.peace_offers = Some(Vec::new());
    d.basis.push((0, PlotBasisView::default()));
    d.trade_routes = Some(Vec::new());
    assert_eq!(d.touched(), SectionMask(0b11_1111_1111));
    for n in 0..10 {
        assert!(d.touched().contains(n));
    }
    assert!(!d.touched().contains(10));
}

fn header() -> ViewHeader {
    ViewHeader {
        turn: 1,
        year: -4000,
        human: 0,
        human_team: 0,
        status: TurnStatus::HumanTurn,
        blocker: None,
    }
}

#[test]
fn basis_checks() {
    let mut r = Rng(7);
    let good = basis_of(&mut r, 4, 3, 9).map().clone();
    assert!(BasisState::new(good.clone()).is_ok());

    let mut short = good.clone();
    short.plots.pop();
    assert_eq!(
        check_basis(&short),
        Err(ApplyError::PlotCount {
            expected: 12,
            actual: 11
        })
    );

    let mut wrong = good.clone();
    wrong.basis_hash ^= 1;
    assert!(matches!(
        BasisState::new(wrong),
        Err(ApplyError::BasisHash { .. })
    ));

    // `game` is not in the hash.
    let mut other_game = good.clone();
    other_game.game = 99;
    assert_eq!(basis_hash_of(&other_game), good.basis_hash);

    let view = gen_view(&mut r, 4, 3);
    assert_eq!(check_pair(&view, &good), Err(ApplyError::MapHeader)); // seed 7 against 9
    let mut matching = view.clone();
    matching.map.terrain_seed = good.terrain_seed;
    matching.map.wrap_x = good.wrap_x;
    matching.map.wrap_y = good.wrap_y;
    assert_eq!(check_pair(&matching, &good), Ok(()));
    matching.map.terrain_seed += 1;
    assert_eq!(check_pair(&matching, &good), Err(ApplyError::MapHeader));
}

#[test]
fn view_hash_does_not_depend_on_the_basis() {
    let mut r = Rng(11);
    let v = gen_view(&mut r, 4, 4);
    let one = ViewHash::of(&v);
    let mut b = basis_of(&mut r, 4, 4, 7);
    let mut hash = one.clone();
    let mut view = v.clone();
    // A diff that only changes a basis record: the view hash stays, the basis hash moves.
    let fresh = vec![(
        0,
        PlotBasisView {
            terrain: 123,
            ..b.map().plots[0].clone()
        },
    )];
    let d = ViewDiff {
        basis: diff_basis(b.map(), &fresh),
        ..ViewDiff::default()
    };
    let before = b.hash();
    apply(&mut view, &mut hash, &mut b, &d).unwrap();
    assert_eq!(hash.value(), one.value());
    assert_ne!(b.hash(), before);
}

#[test]
fn check_rejects_broken_keyframes() {
    let mut r = Rng(5);
    let mut v = gen_view(&mut r, 4, 4);
    v.units = vec![gen_unit(&mut r, 1, 5), gen_unit(&mut r, 0, 9)];
    assert_eq!(check(&v), Err(ApplyError::Unsorted { section: 2 }));
    v.units = vec![gen_unit(&mut r, 1, 5), gen_unit(&mut r, 1, 5)];
    assert_eq!(check(&v), Err(ApplyError::Unsorted { section: 2 }));
    v.units.clear();
    v.map.plots.pop();
    assert_eq!(
        check(&v),
        Err(ApplyError::PlotCount {
            expected: 16,
            actual: 15
        })
    );
}

/// Speed (net-protocol 9): `apply` with the hash recount of a 50 plot + 5 unit diff to a Huge view.
#[test]
fn apply_speed_on_huge() {
    let mut r = Rng(99);
    let (w, h) = (128u16, 80u16);
    let mut a = gen_view(&mut r, w, h);
    a.units.clear();
    for i in 0..600 {
        a.units.push(gen_unit(&mut r, (i % 8) as i8, i / 8 + 1));
    }
    a.units.sort_by_key(|u| (u.owner, u.id));
    let mut b = a.clone();
    for k in 0..50 {
        let i = (k * 197) % b.map.plots.len();
        b.map.plots[i] = gen_plot(&mut r);
        b.map.plots[i].owner = 3;
    }
    for k in 0..5 {
        let (o, id) = (b.units[k * 100].owner, b.units[k * 100].id);
        b.units[k * 100] = gen_unit(&mut r, o, id);
    }
    let fwd = diff(&a, &b, None);
    let back = diff(&b, &a, None);
    assert!(fwd.plots.len() >= 40 && fwd.units.upsert.len() == 5);

    let basis = basis_of(&mut r, w, h, 7);
    let mut view = a.clone();
    let mut hash = ViewHash::of(&a);
    let mut basis = basis;
    let rounds = 200;
    let start = std::time::Instant::now();
    for _ in 0..rounds {
        apply(&mut view, &mut hash, &mut basis, &fwd).unwrap();
        apply(&mut view, &mut hash, &mut basis, &back).unwrap();
    }
    let per_apply_ms = start.elapsed().as_secs_f64() * 1000.0 / f64::from(rounds * 2);
    assert_eq!(view, a);
    assert_eq!(hash, ViewHash::of(&a));
    println!("apply of 50 plots + 5 units to Huge: {per_apply_ms:.4} ms (limit 0.2 ms, release)");
    if !cfg!(debug_assertions) {
        if per_apply_ms > 0.2 {
            println!("WARNING: apply is slower than 0.2 ms");
        }
        // Soft check: ten times the budget fails (a loaded machine must not break the build).
        assert!(per_apply_ms < 2.0, "apply takes {per_apply_ms} ms");
    }
}
