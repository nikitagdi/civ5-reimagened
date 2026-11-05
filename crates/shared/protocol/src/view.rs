//! View diff (net-protocol 4.6, 4.7): the data types of a diff, `diff`, `apply`, the view hash, the map
//! basis state and the undo log. Field order of the data types is the wire format.
//!
//! Hash recap (NP-D6, 4.6): a record hash is `xxh64(seed = section number, canonical bytes)`; the sum of a
//! section is the sum of its record hashes modulo 2^64. Sections: 1 plots, 2 units, 3 cities, 4 players,
//! 5 top bar, 6 notifications, 7 peace offers, 8 map basis (own `basis_hash`, not in the view hash),
//! 9 trade routes. Sections 5, 7 and 9 are one record each (the whole value, also when the list is empty).

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::hash::xxh64;
use crate::world::{
    Blocker, CityKey, CityView, MapBasis, NotificationView, PeaceOffer, PlayerId, PlayerView,
    PlotBasisView, PlotView, TeamId, TopBar, TradeRouteView, TurnStatus, UnitKey, UnitView,
    WorldView,
};

/// The scalar header of a `WorldView` (everything but the sections).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewHeader {
    pub turn: i32,
    pub year: i32,
    pub human: PlayerId,
    pub human_team: TeamId,
    pub status: TurnStatus,
    pub blocker: Option<Blocker>,
}

/// Changes of one keyed section: whole records to insert or replace, and keys to remove. Both lists are
/// sorted by key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionDiff<K, V> {
    pub upsert: Vec<V>,
    pub remove: Vec<K>,
}

impl<K, V> Default for SectionDiff<K, V> {
    fn default() -> Self {
        SectionDiff {
            upsert: Vec::new(),
            remove: Vec::new(),
        }
    }
}

/// Difference between two views of one seat. New sections go to the end.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewDiff {
    pub header: Option<ViewHeader>,
    /// Whole plot records by index `y * width + x`.
    pub plots: Vec<(u32, PlotView)>,
    pub units: SectionDiff<UnitKey, UnitView>,
    pub cities: SectionDiff<CityKey, CityView>,
    pub players: SectionDiff<PlayerId, PlayerView>,
    pub top_bar: Option<TopBar>,
    pub notifications: SectionDiff<i32, NotificationView>,
    /// The whole (small) list, when it changed.
    pub peace_offers: Option<Vec<PeaceOffer>>,
    /// Map basis records by plot index; applied to the basis, not to the view (hash section 8).
    pub basis: Vec<(u32, PlotBasisView)>,
    /// The whole trade route list, when it changed (hash section 9; a small list, like `peace_offers`).
    pub trade_routes: Option<Vec<TradeRouteView>>,
}

/// Bit n set: the diff touches section n (0 header, 1 plots, 2 units, 3 cities, 4 players, 5 top bar,
/// 6 notifications, 7 peace offers, 8 map basis, 9 trade routes).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionMask(pub u32);

impl SectionMask {
    /// True when the diff touches section `n`.
    pub const fn contains(self, n: u8) -> bool {
        n < 32 && self.0 & (1 << n) != 0
    }
}

impl ViewDiff {
    /// Which sections the diff touches, so a client needs no code change for a new section.
    pub fn touched(&self) -> SectionMask {
        let bits = [
            self.header.is_some(),
            !self.plots.is_empty(),
            !self.units.upsert.is_empty() || !self.units.remove.is_empty(),
            !self.cities.upsert.is_empty() || !self.cities.remove.is_empty(),
            !self.players.upsert.is_empty() || !self.players.remove.is_empty(),
            self.top_bar.is_some(),
            !self.notifications.upsert.is_empty() || !self.notifications.remove.is_empty(),
            self.peace_offers.is_some(),
            !self.basis.is_empty(),
            self.trade_routes.is_some(),
        ];
        let mut mask = 0;
        for (n, on) in bits.into_iter().enumerate() {
            if on {
                mask |= 1 << n;
            }
        }
        SectionMask(mask)
    }

    /// True when the diff changes nothing.
    pub fn is_empty(&self) -> bool {
        self.touched().0 == 0
    }
}

// ---------------------------------------------------------------- errors

/// Why a diff or a view is refused. `apply` checks everything before it changes anything.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ApplyError {
    /// Keys or indexes of a section are not strictly increasing (not sorted, or a repeated key).
    #[error("section {section}: keys are not strictly increasing")]
    Unsorted { section: u8 },
    /// A plot or basis index is outside the map.
    #[error("section {section}: index {index} is outside the map")]
    IndexOutOfRange { section: u8, index: u32 },
    /// A removed key is not in the view.
    #[error("section {section}: a removed key is not in the view")]
    UnknownKey { section: u8 },
    /// A key is both upserted and removed in one diff.
    #[error("section {section}: a key is both upserted and removed")]
    KeyInBoth { section: u8 },
    /// `plots.len()` is not `width * height`.
    #[error("plot count {actual} but the map has {expected} plots")]
    PlotCount { expected: usize, actual: usize },
    /// The hash of a map basis differs from the count over its records.
    #[error("map basis hash {actual:016x}, the message says {expected:016x}")]
    BasisHash { expected: u64, actual: u64 },
    /// The map header of a view differs from the one of the map basis.
    #[error("the map header of the view differs from the map basis")]
    MapHeader,
}

// ---------------------------------------------------------------- hash

const S_PLOTS: u8 = 1;
const S_UNITS: u8 = 2;
const S_CITIES: u8 = 3;
const S_PLAYERS: u8 = 4;
const S_TOP_BAR: u8 = 5;
const S_NOTIFICATIONS: u8 = 6;
const S_PEACE: u8 = 7;
const S_BASIS: u8 = 8;
const S_TRADE: u8 = 9;
/// Sections that are part of the view hash, in hash order. Section 8 is not (it has its own hash).
const VIEW_SECTIONS: [u8; 8] = [
    S_PLOTS,
    S_UNITS,
    S_CITIES,
    S_PLAYERS,
    S_TOP_BAR,
    S_NOTIFICATIONS,
    S_PEACE,
    S_TRADE,
];

/// Canonical bytes of a record and their hash, with one reused buffer.
#[derive(Default)]
struct Scratch(Vec<u8>);

impl Scratch {
    /// `xxh64(section, [index u32 LE] ++ postcard(value))`.
    fn rec<T: Serialize + ?Sized>(&mut self, section: u8, index: Option<u32>, value: &T) -> u64 {
        let mut buf = std::mem::take(&mut self.0);
        buf.clear();
        if let Some(i) = index {
            buf.extend_from_slice(&i.to_le_bytes());
        }
        // Serializing these plain types cannot fail; on a failure the buffer is lost and the hash is 0,
        // which shows up as a hash mismatch.
        match postcard::to_extend(value, buf) {
            Ok(b) => {
                let h = xxh64(u64::from(section), &b);
                self.0 = b;
                h
            }
            Err(_) => 0,
        }
    }
}

fn header_of(view: &WorldView) -> ViewHeader {
    ViewHeader {
        turn: view.turn,
        year: view.year,
        human: view.human,
        human_team: view.human_team,
        status: view.status,
        blocker: view.blocker,
    }
}

fn set_header(view: &mut WorldView, h: &ViewHeader) {
    view.turn = h.turn;
    view.year = h.year;
    view.human = h.human;
    view.human_team = h.human_team;
    view.status = h.status;
    view.blocker = h.blocker;
}

fn bytes_of<T: Serialize + ?Sized>(value: &T) -> Vec<u8> {
    postcard::to_allocvec(value).unwrap_or_default()
}

/// The hash of a `WorldView` kept up to date by `apply`: the header bytes and the sum of every section.
/// The map basis is not in it. Cheap to clone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewHash {
    /// postcard of `ViewHeader`.
    head: Vec<u8>,
    /// postcard of `(width, height, wrap_x, wrap_y, terrain_seed)`; constant for the whole game.
    map: Vec<u8>,
    /// Sum of record hashes by section number (index 0 and 8 unused).
    sums: [u64; 10],
}

impl ViewHash {
    /// Full count over every record: for a keyframe.
    pub fn of(view: &WorldView) -> Self {
        let m = &view.map;
        let mut s = Scratch::default();
        let mut sums = [0u64; 10];
        let add = |sum: &mut u64, h: u64| *sum = sum.wrapping_add(h);
        for (i, p) in m.plots.iter().enumerate() {
            add(
                &mut sums[usize::from(S_PLOTS)],
                s.rec(S_PLOTS, Some(i as u32), p),
            );
        }
        for u in &view.units {
            add(&mut sums[usize::from(S_UNITS)], s.rec(S_UNITS, None, u));
        }
        for c in &view.cities {
            add(&mut sums[usize::from(S_CITIES)], s.rec(S_CITIES, None, c));
        }
        for p in &view.players {
            add(&mut sums[usize::from(S_PLAYERS)], s.rec(S_PLAYERS, None, p));
        }
        sums[usize::from(S_TOP_BAR)] = s.rec(S_TOP_BAR, None, &view.top_bar);
        for n in &view.notifications {
            add(
                &mut sums[usize::from(S_NOTIFICATIONS)],
                s.rec(S_NOTIFICATIONS, None, n),
            );
        }
        sums[usize::from(S_PEACE)] = s.rec(S_PEACE, None, &view.peace_offers);
        sums[usize::from(S_TRADE)] = s.rec(S_TRADE, None, &view.trade_routes);
        ViewHash {
            head: bytes_of(&header_of(view)),
            map: bytes_of(&(m.width, m.height, m.wrap_x, m.wrap_y, m.terrain_seed)),
            sums,
        }
    }

    /// The hash of the view: `xxh64(0, header ++ map header ++ (section number ++ sum LE)*)`.
    pub fn value(&self) -> u64 {
        let mut buf =
            Vec::with_capacity(self.head.len() + self.map.len() + VIEW_SECTIONS.len() * 9);
        buf.extend_from_slice(&self.head);
        buf.extend_from_slice(&self.map);
        for sec in VIEW_SECTIONS {
            buf.push(sec);
            buf.extend_from_slice(&self.sums[usize::from(sec)].to_le_bytes());
        }
        xxh64(0, &buf)
    }
}

/// Map header bytes shared by the map basis hash: `postcard((width, height, wrap_x, wrap_y, terrain_seed))`.
fn basis_head(m: &MapBasis) -> Vec<u8> {
    bytes_of(&(m.width, m.height, m.wrap_x, m.wrap_y, m.terrain_seed))
}

fn basis_sum(m: &MapBasis) -> u64 {
    let mut s = Scratch::default();
    let mut sum = 0u64;
    for (i, p) in m.plots.iter().enumerate() {
        sum = sum.wrapping_add(s.rec(S_BASIS, Some(i as u32), p));
    }
    sum
}

fn basis_hash_from(m: &MapBasis, sum: u64) -> u64 {
    let mut buf = basis_head(m);
    buf.extend_from_slice(&sum.to_le_bytes());
    xxh64(0, &buf)
}

/// Full count of the `basis_hash` of a map basis (the field `m.basis_hash` is ignored): for the worker
/// that builds a `MapBasis`, and for tests.
pub fn basis_hash_of(m: &MapBasis) -> u64 {
    basis_hash_from(m, basis_sum(m))
}

/// The map basis at a receiver (client, gateway, worker): the basis and the sum of its record hashes, so
/// `basis` diffs recount the hash from the changed records only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasisState {
    map: MapBasis,
    sum: u64,
}

impl BasisState {
    /// Checks the basis (`check_basis`), counts its hash in full and compares it with `m.basis_hash`.
    pub fn new(m: MapBasis) -> Result<Self, ApplyError> {
        check_basis(&m)?;
        let sum = basis_sum(&m);
        let actual = basis_hash_from(&m, sum);
        if actual != m.basis_hash {
            return Err(ApplyError::BasisHash {
                expected: m.basis_hash,
                actual,
            });
        }
        Ok(BasisState { map: m, sum })
    }

    pub fn map(&self) -> &MapBasis {
        &self.map
    }

    /// The current `basis_hash`.
    pub fn hash(&self) -> u64 {
        basis_hash_from(&self.map, self.sum)
    }
}

// ---------------------------------------------------------------- checks

fn strictly_sorted<K: Ord, V>(list: &[V], key: impl Fn(&V) -> K) -> bool {
    list.windows(2).all(|w| key(&w[0]) < key(&w[1]))
}

fn unit_key(u: &UnitView) -> UnitKey {
    (u.owner, u.id)
}
fn city_key(c: &CityView) -> CityKey {
    (c.owner, c.id)
}
fn player_key(p: &PlayerView) -> PlayerId {
    p.id
}
fn notification_key(n: &NotificationView) -> i32 {
    n.id
}

/// A keyframe is well formed: sections sorted by key with unique keys, `plots.len() == width * height`.
pub fn check(view: &WorldView) -> Result<(), ApplyError> {
    let m = &view.map;
    let expected = usize::from(m.width) * usize::from(m.height);
    if m.plots.len() != expected {
        return Err(ApplyError::PlotCount {
            expected,
            actual: m.plots.len(),
        });
    }
    if !strictly_sorted(&view.units, unit_key) {
        return Err(ApplyError::Unsorted { section: S_UNITS });
    }
    if !strictly_sorted(&view.cities, city_key) {
        return Err(ApplyError::Unsorted { section: S_CITIES });
    }
    if !strictly_sorted(&view.players, player_key) {
        return Err(ApplyError::Unsorted { section: S_PLAYERS });
    }
    if !strictly_sorted(&view.notifications, notification_key) {
        return Err(ApplyError::Unsorted {
            section: S_NOTIFICATIONS,
        });
    }
    Ok(())
}

/// A map basis has one record per plot: `plots.len() == width * height`.
pub fn check_basis(m: &MapBasis) -> Result<(), ApplyError> {
    let expected = usize::from(m.width) * usize::from(m.height);
    if m.plots.len() != expected {
        return Err(ApplyError::PlotCount {
            expected,
            actual: m.plots.len(),
        });
    }
    Ok(())
}

/// The map header of a keyframe equals the header of the map basis.
pub fn check_pair(view: &WorldView, m: &MapBasis) -> Result<(), ApplyError> {
    let v = &view.map;
    if (v.width, v.height, v.wrap_x, v.wrap_y, v.terrain_seed)
        != (m.width, m.height, m.wrap_x, m.wrap_y, m.terrain_seed)
    {
        return Err(ApplyError::MapHeader);
    }
    Ok(())
}

// ---------------------------------------------------------------- diff

/// Merge of two sorted lists into a section diff: a key only in `old` is removed, a new key or another
/// record is upserted.
fn diff_keyed<K: Ord + Copy, V: PartialEq + Clone>(
    old: &[V],
    new: &[V],
    key: impl Fn(&V) -> K,
) -> SectionDiff<K, V> {
    let mut d = SectionDiff::default();
    let (mut i, mut j) = (0, 0);
    while i < old.len() || j < new.len() {
        match (old.get(i), new.get(j)) {
            (Some(o), Some(n)) => match key(o).cmp(&key(n)) {
                std::cmp::Ordering::Less => {
                    d.remove.push(key(o));
                    i += 1;
                }
                std::cmp::Ordering::Greater => {
                    d.upsert.push(n.clone());
                    j += 1;
                }
                std::cmp::Ordering::Equal => {
                    if o != n {
                        d.upsert.push(n.clone());
                    }
                    i += 1;
                    j += 1;
                }
            },
            (Some(o), None) => {
                d.remove.push(key(o));
                i += 1;
            }
            (None, Some(n)) => {
                d.upsert.push(n.clone());
                j += 1;
            }
            (None, None) => break,
        }
    }
    d
}

/// The difference `old` -> `new` (worker side). `dirty_plots` are the plot indexes the core marked dirty:
/// only they are compared; `None` compares all plots. `basis` of the result is empty (see `diff_basis`).
/// Both views must be keyframe-ordered (`check`) and have the same map size.
pub fn diff(old: &WorldView, new: &WorldView, dirty_plots: Option<&[u32]>) -> ViewDiff {
    let (ho, hn) = (header_of(old), header_of(new));
    let n_plots = old.map.plots.len().min(new.map.plots.len());
    let mut plots = Vec::new();
    let mut check_plot = |i: usize| {
        if old.map.plots[i] != new.map.plots[i] {
            plots.push((i as u32, new.map.plots[i].clone()));
        }
    };
    match dirty_plots {
        Some(dirty) => {
            let mut idx: Vec<u32> = dirty
                .iter()
                .copied()
                .filter(|&i| (i as usize) < n_plots)
                .collect();
            idx.sort_unstable();
            idx.dedup();
            for i in idx {
                check_plot(i as usize);
            }
        }
        None => {
            for i in 0..n_plots {
                check_plot(i);
            }
        }
    }
    ViewDiff {
        header: (ho != hn).then_some(hn),
        plots,
        units: diff_keyed(&old.units, &new.units, unit_key),
        cities: diff_keyed(&old.cities, &new.cities, city_key),
        players: diff_keyed(&old.players, &new.players, player_key),
        top_bar: (old.top_bar != new.top_bar).then(|| new.top_bar.clone()),
        notifications: diff_keyed(&old.notifications, &new.notifications, notification_key),
        peace_offers: (old.peace_offers != new.peace_offers).then(|| new.peace_offers.clone()),
        basis: Vec::new(),
        trade_routes: (old.trade_routes != new.trade_routes).then(|| new.trade_routes.clone()),
    }
}

/// The records of `fresh` (the core's answer for the dirty plots) that differ from `old`, sorted by index
/// and unique (the last record of an index wins); indexes outside the map are dropped. The worker puts the
/// result into `ViewDiff.basis`.
pub fn diff_basis(old: &MapBasis, fresh: &[(u32, PlotBasisView)]) -> Vec<(u32, PlotBasisView)> {
    let mut out: Vec<(u32, PlotBasisView)> = fresh
        .iter()
        .filter(|(i, rec)| old.plots.get(*i as usize).is_some_and(|o| o != rec))
        .cloned()
        .collect();
    // Stable sort keeps the input order of equal indexes; the last of a run wins.
    out.sort_by_key(|(i, _)| *i);
    let mut unique: Vec<(u32, PlotBasisView)> = Vec::with_capacity(out.len());
    for rec in out {
        match unique.last_mut() {
            Some(last) if last.0 == rec.0 => *last = rec,
            _ => unique.push(rec),
        }
    }
    // A later record may have equalled the old one again: drop those.
    unique.retain(|(i, rec)| old.plots.get(*i as usize).is_some_and(|o| o != rec));
    unique
}

// ---------------------------------------------------------------- apply

fn check_keyed<K: Ord + Copy, V>(
    section: u8,
    list: &[V],
    d: &SectionDiff<K, V>,
    key: impl Fn(&V) -> K,
) -> Result<(), ApplyError> {
    if !strictly_sorted(&d.upsert, &key) || !d.remove.windows(2).all(|w| w[0] < w[1]) {
        return Err(ApplyError::Unsorted { section });
    }
    for k in &d.remove {
        if list.binary_search_by_key(k, &key).is_err() {
            return Err(ApplyError::UnknownKey { section });
        }
        if d.upsert.binary_search_by_key(k, &key).is_ok() {
            return Err(ApplyError::KeyInBoth { section });
        }
    }
    Ok(())
}

fn check_indexes<V>(section: u8, recs: &[(u32, V)], len: usize) -> Result<(), ApplyError> {
    if !strictly_sorted(recs, |r| r.0) {
        return Err(ApplyError::Unsorted { section });
    }
    if let Some((index, _)) = recs.iter().find(|(i, _)| *i as usize >= len) {
        return Err(ApplyError::IndexOutOfRange {
            section,
            index: *index,
        });
    }
    Ok(())
}

/// Everything `apply` needs to be true, checked before the first change.
fn check_diff(view: &WorldView, basis: &BasisState, d: &ViewDiff) -> Result<(), ApplyError> {
    check_indexes(S_PLOTS, &d.plots, view.map.plots.len())?;
    check_keyed(S_UNITS, &view.units, &d.units, unit_key)?;
    check_keyed(S_CITIES, &view.cities, &d.cities, city_key)?;
    check_keyed(S_PLAYERS, &view.players, &d.players, player_key)?;
    check_keyed(
        S_NOTIFICATIONS,
        &view.notifications,
        &d.notifications,
        notification_key,
    )?;
    check_indexes(S_BASIS, &d.basis, basis.map.plots.len())?;
    Ok(())
}

fn apply_keyed<K: Ord + Copy, V: Serialize + Clone>(
    section: u8,
    list: &mut Vec<V>,
    sum: &mut u64,
    d: &SectionDiff<K, V>,
    key: impl Fn(&V) -> K,
    s: &mut Scratch,
) {
    for k in &d.remove {
        if let Ok(pos) = list.binary_search_by_key(k, &key) {
            let old = list.remove(pos);
            *sum = sum.wrapping_sub(s.rec(section, None, &old));
        }
    }
    for v in &d.upsert {
        match list.binary_search_by_key(&key(v), &key) {
            Ok(pos) => {
                *sum = sum.wrapping_sub(s.rec(section, None, &list[pos]));
                list[pos] = v.clone();
            }
            Err(pos) => list.insert(pos, v.clone()),
        }
        *sum = sum.wrapping_add(s.rec(section, None, v));
    }
}

/// Checks and applies a diff: the view, its hash and (when `d.basis` is not empty) the map basis and its
/// hash. On an error nothing changes. `apply` does not know the hashes of the message: the caller
/// compares `ViewHash::value` with `ViewDelta.hash` and `BasisState::hash` with `ViewDelta.basis_hash`
/// and, on a mismatch, restores with `UndoLog::restore` (take `UndoLog::capture` before `apply`).
pub fn apply(
    view: &mut WorldView,
    hash: &mut ViewHash,
    basis: &mut BasisState,
    d: &ViewDiff,
) -> Result<(), ApplyError> {
    check_diff(view, basis, d)?;
    let mut s = Scratch::default();
    if let Some(h) = &d.header {
        set_header(view, h);
        hash.head = bytes_of(h);
    }
    let sums = &mut hash.sums;
    for (i, rec) in &d.plots {
        let slot = &mut view.map.plots[*i as usize];
        let sum = &mut sums[usize::from(S_PLOTS)];
        *sum = sum.wrapping_sub(s.rec(S_PLOTS, Some(*i), &*slot));
        *slot = rec.clone();
        *sum = sum.wrapping_add(s.rec(S_PLOTS, Some(*i), rec));
    }
    apply_keyed(
        S_UNITS,
        &mut view.units,
        &mut sums[usize::from(S_UNITS)],
        &d.units,
        unit_key,
        &mut s,
    );
    apply_keyed(
        S_CITIES,
        &mut view.cities,
        &mut sums[usize::from(S_CITIES)],
        &d.cities,
        city_key,
        &mut s,
    );
    apply_keyed(
        S_PLAYERS,
        &mut view.players,
        &mut sums[usize::from(S_PLAYERS)],
        &d.players,
        player_key,
        &mut s,
    );
    if let Some(t) = &d.top_bar {
        view.top_bar = t.clone();
        sums[usize::from(S_TOP_BAR)] = s.rec(S_TOP_BAR, None, t);
    }
    apply_keyed(
        S_NOTIFICATIONS,
        &mut view.notifications,
        &mut sums[usize::from(S_NOTIFICATIONS)],
        &d.notifications,
        notification_key,
        &mut s,
    );
    if let Some(p) = &d.peace_offers {
        view.peace_offers = p.clone();
        sums[usize::from(S_PEACE)] = s.rec(S_PEACE, None, p);
    }
    if let Some(t) = &d.trade_routes {
        view.trade_routes = t.clone();
        sums[usize::from(S_TRADE)] = s.rec(S_TRADE, None, t);
    }
    if !d.basis.is_empty() {
        for (i, rec) in &d.basis {
            let slot = &mut basis.map.plots[*i as usize];
            basis.sum = basis.sum.wrapping_sub(s.rec(S_BASIS, Some(*i), &*slot));
            *slot = rec.clone();
            basis.sum = basis.sum.wrapping_add(s.rec(S_BASIS, Some(*i), rec));
        }
        basis.map.basis_hash = basis.hash();
    }
    Ok(())
}

// ---------------------------------------------------------------- undo

/// Old records of one keyed section: the ones to put back and the keys that did not exist.
#[derive(Debug, Clone)]
struct SectionUndo<K, V> {
    restore: Vec<V>,
    inserted: Vec<K>,
}

impl<K: Ord + Copy, V: Clone> SectionUndo<K, V> {
    fn capture(list: &[V], d: &SectionDiff<K, V>, key: impl Fn(&V) -> K) -> Self {
        let mut restore = Vec::new();
        let mut inserted = Vec::new();
        for k in &d.remove {
            if let Ok(pos) = list.binary_search_by_key(k, &key) {
                restore.push(list[pos].clone());
            }
        }
        for v in &d.upsert {
            match list.binary_search_by_key(&key(v), &key) {
                Ok(pos) => restore.push(list[pos].clone()),
                Err(_) => inserted.push(key(v)),
            }
        }
        SectionUndo { restore, inserted }
    }

    fn restore(self, list: &mut Vec<V>, key: impl Fn(&V) -> K) {
        for k in &self.inserted {
            if let Ok(pos) = list.binary_search_by_key(k, &key) {
                list.remove(pos);
            }
        }
        for v in self.restore {
            match list.binary_search_by_key(&key(&v), &key) {
                Ok(pos) => list[pos] = v,
                Err(pos) => list.insert(pos, v),
            }
        }
    }
}

/// Rollback of one applied diff. `apply` compares the hash after it replaced the records, so the client
/// and the gateway take a log first and restore the view, the basis and both hashes on a mismatch.
/// Costs O(diff). The log of a diff that `apply` would refuse is harmless: `restore` after a refused
/// `apply` changes nothing.
#[derive(Debug, Clone)]
pub struct UndoLog {
    header: Option<ViewHeader>,
    plots: Vec<(u32, PlotView)>,
    units: SectionUndo<UnitKey, UnitView>,
    cities: SectionUndo<CityKey, CityView>,
    players: SectionUndo<PlayerId, PlayerView>,
    top_bar: Option<TopBar>,
    notifications: SectionUndo<i32, NotificationView>,
    peace_offers: Option<Vec<PeaceOffer>>,
    trade_routes: Option<Vec<TradeRouteView>>,
    basis: Vec<(u32, PlotBasisView)>,
    hash: ViewHash,
    basis_sum: u64,
    basis_hash: u64,
}

impl UndoLog {
    /// Copies of the records `d` touches and the sums before it. Call before `apply`.
    pub fn capture(view: &WorldView, hash: &ViewHash, basis: &BasisState, d: &ViewDiff) -> UndoLog {
        UndoLog {
            header: d.header.map(|_| header_of(view)),
            plots: d
                .plots
                .iter()
                .filter_map(|(i, _)| view.map.plots.get(*i as usize).map(|p| (*i, p.clone())))
                .collect(),
            units: SectionUndo::capture(&view.units, &d.units, unit_key),
            cities: SectionUndo::capture(&view.cities, &d.cities, city_key),
            players: SectionUndo::capture(&view.players, &d.players, player_key),
            top_bar: d.top_bar.as_ref().map(|_| view.top_bar.clone()),
            notifications: SectionUndo::capture(
                &view.notifications,
                &d.notifications,
                notification_key,
            ),
            peace_offers: d.peace_offers.as_ref().map(|_| view.peace_offers.clone()),
            trade_routes: d.trade_routes.as_ref().map(|_| view.trade_routes.clone()),
            basis: d
                .basis
                .iter()
                .filter_map(|(i, _)| basis.map.plots.get(*i as usize).map(|p| (*i, p.clone())))
                .collect(),
            hash: hash.clone(),
            basis_sum: basis.sum,
            basis_hash: basis.map.basis_hash,
        }
    }

    /// Puts the view, the basis and both hashes back to what they were at `capture`.
    pub fn restore(self, view: &mut WorldView, hash: &mut ViewHash, basis: &mut BasisState) {
        if let Some(h) = &self.header {
            set_header(view, h);
        }
        for (i, rec) in self.plots {
            if let Some(slot) = view.map.plots.get_mut(i as usize) {
                *slot = rec;
            }
        }
        self.units.restore(&mut view.units, unit_key);
        self.cities.restore(&mut view.cities, city_key);
        self.players.restore(&mut view.players, player_key);
        if let Some(t) = self.top_bar {
            view.top_bar = t;
        }
        self.notifications
            .restore(&mut view.notifications, notification_key);
        if let Some(p) = self.peace_offers {
            view.peace_offers = p;
        }
        if let Some(t) = self.trade_routes {
            view.trade_routes = t;
        }
        for (i, rec) in self.basis {
            if let Some(slot) = basis.map.plots.get_mut(i as usize) {
                *slot = rec;
            }
        }
        basis.sum = self.basis_sum;
        basis.map.basis_hash = self.basis_hash;
        *hash = self.hash;
    }
}
