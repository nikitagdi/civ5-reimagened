//! Random views for the stream tests: the generators of `protocol/tests/view_props.rs` (net-protocol 9),
//! copied because an integration test of another crate cannot reach them. Own xorshift, no dependencies.

#![allow(dead_code)]

use protocol::*;

pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    pub fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
    pub fn i16(&mut self) -> i16 {
        self.next() as i16
    }
    pub fn small(&mut self, n: i64) -> i16 {
        (self.below(n as u64) as i64 - 1) as i16
    }
}

pub fn plot_type(r: &mut Rng) -> PlotType {
    match r.below(4) {
        0 => PlotType::Land,
        1 => PlotType::Hill,
        2 => PlotType::Mountain,
        _ => PlotType::Water,
    }
}

pub fn gen_plot(r: &mut Rng) -> PlotView {
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

pub fn gen_basis_plot(r: &mut Rng) -> PlotBasisView {
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

pub fn gen_unit(r: &mut Rng, owner: PlayerId, id: i32) -> UnitView {
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

pub fn gen_city(r: &mut Rng, owner: PlayerId, id: i32) -> CityView {
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

pub fn gen_player(r: &mut Rng, id: PlayerId) -> PlayerView {
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

pub fn gen_notification(r: &mut Rng, id: i32) -> NotificationView {
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

pub fn gen_top_bar(r: &mut Rng) -> TopBar {
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

pub fn gen_status(r: &mut Rng) -> TurnStatus {
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

pub fn gen_blocker(r: &mut Rng) -> Option<Blocker> {
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

pub fn gen_peace(r: &mut Rng) -> Vec<PeaceOffer> {
    (0..r.below(3))
        .map(|_| PeaceOffer {
            from: r.small(4) as i8,
            to: r.small(4) as i8,
            turn: r.small(50) as i32,
        })
        .collect()
}

pub fn gen_routes(r: &mut Rng) -> Vec<TradeRouteView> {
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
pub fn gen_view(r: &mut Rng, w: u16, h: u16) -> WorldView {
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
pub fn mutate(r: &mut Rng, v: &WorldView) -> WorldView {
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

pub fn basis_of(r: &mut Rng, w: u16, h: u16, seed: u32) -> BasisState {
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
