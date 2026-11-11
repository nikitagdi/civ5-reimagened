//! Strict, bounds-checked parser of `GCQ1` query replies (`gamecore_abi.h`).
//!
//! Layout: 24-byte header, `count * record_size` record bytes, then the string table. Fields are only
//! ever appended to a record, so a reader needs `record_size >= known size` and skips the rest.
//! Every length, offset and string is checked; malformed data is a [`QueryError`], never a panic.

use gamecore_sys::ffi;

use crate::play::{
    Activity, BuildFlags, City, CityArt, CityBuild, CityFlags, CityOrder, Notification, OrderType,
    PathNode, PlayerResource, PolicyBranchState, RangeTarget, RangeTargetFlags, ReachFlags,
    Reachable, ResourceUsage, TechState, TechStatus, TopBar, TopBarFlags, TradePath, TradeRoute,
    TreeNode, Unit, UnitAction, UnitFlags,
};
use crate::setup::GameType;
use crate::slots::{HumanSlot, TurnFlags};
use crate::types::{
    BasisFlags, CatalogEntry, CombatFlags, CombatForecast, DiploFlags, DiploRelation, Direction,
    GameInfo, GameOptionEntry, GameStatus, MapOptionEntry, MapOptionValueEntry, NamedEntry, Player,
    PlayerFlags, Plot, PlotBasis, PlotFlags, PlotType, TimerEntry, WorldSizeEntry, opt_i16, opt_u8,
};
use crate::types::{Phase, TurnMode, UnsafeReason};

/// Size of the `gcq_header`.
pub const HEADER_SIZE: usize = 24;
/// `format` field of a `GCQ1` header.
pub const FORMAT: u16 = 1;

/// Bytes of the records the reader knows (smaller `record_size` = incompatible reply).
pub const GAME_RECORD_SIZE: usize = 36;
/// Size of a `Q_GAME` record with the v2 fields (`game_type` ... `map_seed`).
pub const GAME_V2_RECORD_SIZE: usize = 56;
/// Size of a `Q_TURN` record.
pub const TURN_RECORD_SIZE: usize = 24;
pub const PLOT_RECORD_SIZE: usize = 28;
/// Size of a `Q_PLOTS` record with the v2 fields `build_type` and `build_pct`.
pub const PLOT_V2_RECORD_SIZE: usize = 32;
pub const PLAYER_RECORD_SIZE: usize = 28;
/// Size of a `Q_PLAYERS` record with the v2 fields `nick`, `slot`, `art_techs`.
pub const PLAYER_V2_RECORD_SIZE: usize = 48;
pub const CATALOG_RECORD_SIZE: usize = 36;
pub const UNIT_RECORD_SIZE: usize = 36;
/// Size of a `Q_UNITS` record with the v2 fields `transport_id`, `build_type` and `facing`.
pub const UNIT_V2_RECORD_SIZE: usize = 44;
pub const UNIT_ACTION_RECORD_SIZE: usize = 8;
/// Size of a `Q_UNIT_ACTIONS` record with the UI v2 field `cost`.
pub const UNIT_ACTION_COST_RECORD_SIZE: usize = 12;
pub const PATH_NODE_RECORD_SIZE: usize = 8;
/// Size of a `Q_PATH` / `Q_REACHABLE` record with the v2 `flags`; a smaller one reads as no flags.
pub const PATH_NODE_FLAGS_RECORD_SIZE: usize = 12;
pub const DIPLO_RECORD_SIZE: usize = 8;
pub const TREE_NODE_RECORD_SIZE: usize = 16;
pub const PLOT_BASIS_RECORD_SIZE: usize = 16;
pub const COMBAT_PREVIEW_RECORD_SIZE: usize = 20;
pub const PLOT_DIRTY_RECORD_SIZE: usize = 4;
pub const NOTIFICATION_RECORD_SIZE: usize = 32;
pub const CITY_RECORD_SIZE: usize = 52;
/// Size of a `Q_CITIES` record with the v2 field `founder`.
pub const CITY_V2_RECORD_SIZE: usize = 56;
/// Size of a `Q_CITIES` record with the UI v2 field `overflow`.
pub const CITY_OVERFLOW_RECORD_SIZE: usize = 60;
pub const TOP_BAR_RECORD_SIZE: usize = 60;
/// Size of a `Q_TOP_BAR` record with the UI v2 field `tourism`.
pub const TOP_BAR_TOURISM_RECORD_SIZE: usize = 64;
pub const CITY_BUILD_RECORD_SIZE: usize = 24;
/// UI v2 read-only queries.
pub const CITY_ORDER_RECORD_SIZE: usize = 16;
pub const TECH_RECORD_SIZE: usize = 16;
pub const RESOURCE_RECORD_SIZE: usize = 16;
pub const POLICY_BRANCH_RECORD_SIZE: usize = 8;
pub const PROMOTION_RECORD_SIZE: usize = 4;
pub const RANGE_TARGET_RECORD_SIZE: usize = 8;
pub const CITY_ART_RECORD_SIZE: usize = 12;
pub const TRADE_ROUTE_RECORD_SIZE: usize = 16;
pub const TRADE_PLOT_RECORD_SIZE: usize = 8;

/// A malformed or unexpected `GCQ1` reply.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QueryError {
    #[error("reply of {0} bytes is shorter than the 24-byte header")]
    TooShort(usize),
    #[error("bad magic {0:02x?}, expected \"GCQ1\"")]
    BadMagic([u8; 4]),
    #[error("unsupported reply format {0}")]
    BadFormat(u16),
    #[error("reply is of query kind {got}, expected {expected}")]
    WrongKind { expected: u16, got: u16 },
    #[error("record_size {got} is below the {need} bytes this reader needs")]
    RecordTooSmall { need: usize, got: u32 },
    #[error("count {count} * record_size {record_size} does not fit the reply")]
    BadLayout { count: u32, record_size: u32 },
    #[error("strings_offset {got} must be {want} (header + records)")]
    BadStringsOffset { got: u32, want: usize },
    #[error("string table ({offset}+{size}) lies outside the {len}-byte reply")]
    StringsOutOfBounds { offset: u32, size: u32, len: usize },
    #[error("string offset {0} is outside the string table")]
    StringOutOfBounds(u32),
    #[error("string at offset {0} is not NUL-terminated inside the table")]
    StringUnterminated(u32),
    #[error("string at offset {0} is not valid UTF-8")]
    StringNotUtf8(u32),
    #[error("reply holds {got} records, expected {want}")]
    CountMismatch { want: usize, got: u32 },
}

/// A validated `GCQ1` reply borrowing the bytes.
#[derive(Debug, Clone, Copy)]
pub struct Reply<'a> {
    kind: u16,
    count: u32,
    record_size: usize,
    records: &'a [u8],
    strings: &'a [u8],
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

impl<'a> Reply<'a> {
    /// Checks the header, the record area and the string table of `buf` against query `kind`.
    pub fn parse(buf: &'a [u8], kind: u16) -> Result<Self, QueryError> {
        if buf.len() < HEADER_SIZE {
            return Err(QueryError::TooShort(buf.len()));
        }
        let magic = [buf[0], buf[1], buf[2], buf[3]];
        if &magic != b"GCQ1" {
            return Err(QueryError::BadMagic(magic));
        }
        let format = u16_at(buf, 4);
        if format != FORMAT {
            return Err(QueryError::BadFormat(format));
        }
        let got_kind = u16_at(buf, 6);
        if got_kind != kind {
            return Err(QueryError::WrongKind {
                expected: kind,
                got: got_kind,
            });
        }
        let count = u32_at(buf, 8);
        let record_size = u32_at(buf, 12);
        let strings_offset = u32_at(buf, 16);
        let strings_size = u32_at(buf, 20);

        let bad_layout = QueryError::BadLayout { count, record_size };
        let records_len = (count as usize)
            .checked_mul(record_size as usize)
            .ok_or_else(|| bad_layout.clone())?;
        let want_offset = HEADER_SIZE
            .checked_add(records_len)
            .ok_or_else(|| bad_layout.clone())?;
        if want_offset > buf.len() {
            return Err(bad_layout);
        }
        if strings_offset as usize != want_offset {
            return Err(QueryError::BadStringsOffset {
                got: strings_offset,
                want: want_offset,
            });
        }
        let strings_end = want_offset.checked_add(strings_size as usize);
        let strings_end = match strings_end {
            Some(e) if e <= buf.len() => e,
            _ => {
                return Err(QueryError::StringsOutOfBounds {
                    offset: strings_offset,
                    size: strings_size,
                    len: buf.len(),
                });
            }
        };
        Ok(Reply {
            kind: got_kind,
            count,
            record_size: record_size as usize,
            records: &buf[HEADER_SIZE..want_offset],
            strings: &buf[want_offset..strings_end],
        })
    }

    pub fn kind(&self) -> u16 {
        self.kind
    }

    pub fn count(&self) -> usize {
        self.count as usize
    }

    pub fn record_size(&self) -> usize {
        self.record_size
    }

    /// Fails unless every record has at least `need` bytes (the size this reader knows).
    pub fn require_record_size(&self, need: usize) -> Result<(), QueryError> {
        if self.record_size < need {
            Err(QueryError::RecordTooSmall {
                need,
                got: self.record_size as u32,
            })
        } else {
            Ok(())
        }
    }

    /// Fails unless the reply holds exactly `want` records.
    pub fn require_count(&self, want: usize) -> Result<(), QueryError> {
        if self.count as usize == want {
            Ok(())
        } else {
            Err(QueryError::CountMismatch {
                want,
                got: self.count,
            })
        }
    }

    /// The `i`-th record (`record_size` bytes), `None` past the end.
    pub fn record(&self, i: usize) -> Option<&'a [u8]> {
        let start = i.checked_mul(self.record_size)?;
        self.records
            .get(start..start.checked_add(self.record_size)?)
    }

    /// All records.
    pub fn records(&self) -> impl Iterator<Item = &'a [u8]> + '_ {
        // record_size 0 is legal for an empty reply only; chunks_exact panics on 0.
        let size = self.record_size.max(1);
        self.records.chunks_exact(size).take(self.count as usize)
    }

    /// The string at `offset` of the string table. `GCQ_NO_STRING` is `Ok(None)`.
    pub fn string(&self, offset: u32) -> Result<Option<&'a str>, QueryError> {
        if offset == ffi::GCQ_NO_STRING {
            return Ok(None);
        }
        let tail = self
            .strings
            .get(offset as usize..)
            .filter(|t| !t.is_empty())
            .ok_or(QueryError::StringOutOfBounds(offset))?;
        let len = tail
            .iter()
            .position(|&b| b == 0)
            .ok_or(QueryError::StringUnterminated(offset))?;
        std::str::from_utf8(&tail[..len])
            .map(Some)
            .map_err(|_| QueryError::StringNotUtf8(offset))
    }

    fn string_or_empty(&self, offset: u32) -> Result<String, QueryError> {
        Ok(self.string(offset)?.unwrap_or_default().to_owned())
    }

    fn opt_string(&self, offset: u32) -> Result<Option<String>, QueryError> {
        Ok(self.string(offset)?.map(str::to_owned))
    }
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    let mut v = [0u8; 8];
    v.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(v)
}

fn i16_at(b: &[u8], at: usize) -> i16 {
    i16::from_le_bytes([b[at], b[at + 1]])
}

fn i32_at(b: &[u8], at: usize) -> i32 {
    i32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Decodes a `Q_GAME` reply.
pub fn parse_game(buf: &[u8]) -> Result<GameInfo, QueryError> {
    let r = Reply::parse(buf, ffi::GC_Q_GAME as u16)?;
    r.require_record_size(GAME_RECORD_SIZE)?;
    r.require_count(1)?;
    // `game_type` ... `map_seed` are v2 fields: read only when the record has them.
    let v2 = r.record_size() >= GAME_V2_RECORD_SIZE;
    let b = r
        .record(0)
        .ok_or(QueryError::CountMismatch { want: 1, got: 0 })?;
    Ok(GameInfo {
        turn: i32_at(b, 0),
        year: i32_at(b, 4),
        blocker_unit: i32_at(b, 8),
        blocker_city: i32_at(b, 12),
        blocker_x: i16_at(b, 16),
        blocker_y: i16_at(b, 18),
        map_width: i16_at(b, 20),
        map_height: i16_at(b, 22),
        human_player: b[24] as i8,
        human_team: b[25] as i8,
        active_player: b[26] as i8,
        status: GameStatus::from_abi(b[27] as i8),
        blocker: b[28] as i8,
        wrap_x: b[29] != 0,
        wrap_y: b[30] != 0,
        difficulty: b[31] as i8,
        winner: b[32] as i8,
        victory_type: b[33] as i8,
        game_type: if v2 && b[36] == 1 {
            GameType::Network
        } else {
            GameType::Single
        },
        turn_mode: if v2 { turn_mode_of(b[37] as i8) } else { None },
        phase: if v2 && b[38] as i8 == ffi::GC_PHASE_AI as i8 {
            Phase::Ai
        } else {
            Phase::Humans
        },
        humans: if v2 { b[39] } else { 0 },
        humans_left: if v2 { b[40] } else { 0 },
        safe_point: v2 && b[41] != 0,
        unsafe_reason: if v2 {
            UnsafeReason::from_abi(b[42] as i8)
        } else {
            None
        },
        turn_len_s: if v2 { i32_at(b, 44) } else { 0 },
        turn_base_s: if v2 { i32_at(b, 48) } else { 0 },
        map_seed: if v2 { u32_at(b, 52) } else { 0 },
    })
}

fn turn_mode_of(v: i8) -> Option<TurnMode> {
    match i32::from(v) {
        ffi::GC_TURN_MODE_SIMULTANEOUS => Some(TurnMode::Simultaneous),
        ffi::GC_TURN_MODE_HYBRID => Some(TurnMode::Hybrid),
        _ => None,
    }
}

/// Decodes a `Q_TURN` reply: one record per slot that is or was a human, by player id.
pub fn parse_turn(buf: &[u8]) -> Result<Vec<HumanSlot>, QueryError> {
    let r = Reply::parse(buf, ffi::GC_Q_TURN as u16)?;
    r.require_record_size(TURN_RECORD_SIZE)?;
    r.records()
        .map(|b| {
            Ok(HumanSlot {
                player: b[0],
                team: b[1] as i8,
                slot_status: b[2] as i8,
                blocker: b[3] as i8,
                flags: TurnFlags(u16_at(b, 4)),
                turn_order: u8::try_from(i16_at(b, 6)).ok(),
                blocker_unit: i32_at(b, 8),
                blocker_city: i32_at(b, 12),
                blocker_x: i16_at(b, 16),
                blocker_y: i16_at(b, 18),
                nick: r.opt_string(u32_at(b, 20))?,
            })
        })
        .collect()
}

/// Decodes a `Q_PLOTS` reply.
pub fn parse_plots(buf: &[u8]) -> Result<Vec<Plot>, QueryError> {
    let r = Reply::parse(buf, ffi::GC_Q_PLOTS as u16)?;
    r.require_record_size(PLOT_RECORD_SIZE)?;
    // The build fields are v2: read them only when the record has them.
    let v2 = r.record_size() >= PLOT_V2_RECORD_SIZE;
    Ok(r.records()
        .map(|b| {
            let flags = PlotFlags(u16_at(b, 4));
            // An unrevealed plot is all zero: no terrain, owner or improvement is known. Ids that are
            // zero there would read as "type 0", so they become None.
            let revealed = flags.0 & ffi::GC_PLOT_F_REVEALED != 0;
            let id = |v: i16| if revealed { opt_i16(v) } else { None };
            Plot {
                x: i16_at(b, 0),
                y: i16_at(b, 2),
                flags,
                plot_type: PlotType::from_abi(b[6] as i8),
                terrain: i16_at(b, 8),
                feature: id(i16_at(b, 10)),
                resource: id(i16_at(b, 12)),
                resource_qty: b[14] as i8,
                improvement: id(i16_at(b, 16)),
                route: id(i16_at(b, 18)),
                owner: if revealed { opt_u8(b[20] as i8) } else { None },
                continent_art: b[21] as i8,
                yields: [
                    b[22] as i8,
                    b[23] as i8,
                    b[24] as i8,
                    b[25] as i8,
                    b[26] as i8,
                    b[27] as i8,
                ],
                build_type: if v2 { opt_i16(i16_at(b, 28)) } else { None },
                build_pct: if v2 { b[30] } else { 0 },
            }
        })
        .collect())
}

/// Decodes a `Q_PLAYERS` reply.
pub fn parse_players(buf: &[u8]) -> Result<Vec<Player>, QueryError> {
    let r = Reply::parse(buf, ffi::GC_Q_PLAYERS as u16)?;
    r.require_record_size(PLAYER_RECORD_SIZE)?;
    // `nick`, `slot` and `art_techs` are v2 fields: read only when the record has them.
    let v2 = r.record_size() >= PLAYER_V2_RECORD_SIZE;
    r.records()
        .map(|b| {
            Ok(Player {
                nick: if v2 {
                    r.opt_string(u32_at(b, 28))?
                } else {
                    None
                },
                slot: if v2 { b[32] as i8 } else { -1 },
                art_techs: if v2 { u64_at(b, 40) } else { 0 },
                id: b[0] as i8,
                team: b[1] as i8,
                civ_type: opt_i16(i16_at(b, 2)),
                leader_type: opt_i16(i16_at(b, 4)),
                flags: PlayerFlags(u16_at(b, 6)),
                color_primary: u32_at(b, 8),
                color_secondary: u32_at(b, 12),
                name: r.string_or_empty(u32_at(b, 16))?,
                civ_name: r.string_or_empty(u32_at(b, 20))?,
                era: opt_i16(i16::from(b[24] as i8)),
            })
        })
        .collect()
}

/// Decodes a `Q_CATALOG` reply.
pub fn parse_catalog(buf: &[u8]) -> Result<Vec<CatalogEntry>, QueryError> {
    let r = Reply::parse(buf, ffi::GC_Q_CATALOG as u16)?;
    r.require_record_size(CATALOG_RECORD_SIZE)?;
    r.records()
        .map(|b| {
            Ok(CatalogEntry {
                id: i16_at(b, 0),
                icon_index: opt_i16(i16_at(b, 2)),
                flag_index: opt_i16(i16_at(b, 4)),
                type_name: r.string_or_empty(u32_at(b, 8))?,
                name: r.string_or_empty(u32_at(b, 12))?,
                art_define: r.opt_string(u32_at(b, 16))?,
                icon_atlas: r.opt_string(u32_at(b, 20))?,
                flag_atlas: r.opt_string(u32_at(b, 24))?,
                art_style: r.opt_string(u32_at(b, 28))?,
                strategic_view: r.opt_string(u32_at(b, 32))?,
            })
        })
        .collect()
}

/// Size of a record of the TurnTimers table of `Q_CATALOG` (`gcq_catalog_timer`).
pub const CATALOG_TIMER_RECORD_SIZE: usize = 24;

/// Decodes a `Q_CATALOG` reply of the TurnTimers table (`GC_CATALOG_TURN_TIMERS`).
pub fn parse_catalog_timers(buf: &[u8]) -> Result<Vec<TimerEntry>, QueryError> {
    let r = Reply::parse(buf, ffi::GC_Q_CATALOG as u16)?;
    r.require_record_size(CATALOG_TIMER_RECORD_SIZE)?;
    r.records()
        .map(|b| {
            Ok(TimerEntry {
                id: i16_at(b, 0),
                type_name: r.string_or_empty(u32_at(b, 4))?,
                name: r.string_or_empty(u32_at(b, 8))?,
                base_time: i32_at(b, 12),
                city_resource: i32_at(b, 16),
                unit_resource: i32_at(b, 20),
            })
        })
        .collect()
}

/// Size of a record of the Worlds table of `Q_CATALOG` (`gcq_catalog_world_size`).
pub const CATALOG_WORLD_SIZE_RECORD_SIZE: usize = 20;

/// Decodes a `Q_CATALOG` reply of the Worlds table (`GC_CATALOG_WORLD_SIZES`).
pub fn parse_catalog_world_sizes(buf: &[u8]) -> Result<Vec<WorldSizeEntry>, QueryError> {
    let r = Reply::parse(buf, ffi::GC_Q_CATALOG as u16)?;
    r.require_record_size(CATALOG_WORLD_SIZE_RECORD_SIZE)?;
    r.records()
        .map(|b| {
            Ok(WorldSizeEntry {
                id: i16_at(b, 0),
                type_name: r.string_or_empty(u32_at(b, 4))?,
                name: r.string_or_empty(u32_at(b, 8))?,
                width: i16_at(b, 12),
                height: i16_at(b, 14),
                default_players: i16_at(b, 16),
                default_minor: i16_at(b, 18),
            })
        })
        .collect()
}

/// Size of a record of the v3 tables of `Q_CATALOG` (`gcq_catalog_ext`).
pub const CATALOG_EXT_RECORD_SIZE: usize = 32;

/// One decoded `gcq_catalog_ext`: four integers and four strings.
pub type ExtRow = ([i32; 4], [String; 4]);

/// Decodes a `Q_CATALOG` reply of a v3 table (`GC_CATALOG_GAME_OPTIONS` ... `GC_CATALOG_MAP_OPTION_VALUES`).
pub fn parse_catalog_ext(buf: &[u8]) -> Result<Vec<ExtRow>, QueryError> {
    let r = Reply::parse(buf, ffi::GC_Q_CATALOG as u16)?;
    r.require_record_size(CATALOG_EXT_RECORD_SIZE)?;
    r.records()
        .map(|b| {
            let n = [i32_at(b, 0), i32_at(b, 4), i32_at(b, 8), i32_at(b, 12)];
            let s = [
                r.string_or_empty(u32_at(b, 16))?,
                r.string_or_empty(u32_at(b, 20))?,
                r.string_or_empty(u32_at(b, 24))?,
                r.string_or_empty(u32_at(b, 28))?,
            ];
            Ok((n, s))
        })
        .collect()
}

/// The GameOptions rows of a `GC_CATALOG_GAME_OPTIONS` reply.
pub fn game_options_of(rows: Vec<ExtRow>) -> Vec<GameOptionEntry> {
    rows.into_iter()
        .map(|(n, [type_name, name, help, _])| GameOptionEntry {
            id: n[0],
            type_name,
            name,
            help,
            default: n[1] != 0,
            visible: n[2] != 0,
            supports_multiplayer: n[3] != 0,
        })
        .collect()
}

/// The rows of a `GC_CATALOG_VICTORIES` or `GC_CATALOG_MINOR_CIVS` reply.
pub fn named_of(rows: Vec<ExtRow>) -> Vec<NamedEntry> {
    rows.into_iter()
        .map(|(n, [type_name, name, _, _])| NamedEntry {
            id: n[0],
            type_name,
            name,
        })
        .collect()
}

/// The rows of a `GC_CATALOG_MAP_OPTIONS` reply.
pub fn map_options_of(rows: Vec<ExtRow>) -> Vec<MapOptionEntry> {
    rows.into_iter()
        .map(|(n, [script, option_id, name, help])| MapOptionEntry {
            script,
            option_id,
            name,
            help,
            sort_priority: n[0],
            default_value: n[1],
            read_only: n[2] != 0,
            hidden: n[3] != 0,
        })
        .collect()
}

/// The rows of a `GC_CATALOG_MAP_OPTION_VALUES` reply.
pub fn map_option_values_of(rows: Vec<ExtRow>) -> Vec<MapOptionValueEntry> {
    rows.into_iter()
        .map(|(n, [script, option_id, name, _])| MapOptionValueEntry {
            script,
            option_id,
            name,
            value: n[0],
            sort_index: n[1],
        })
        .collect()
}

fn opt_i32(v: i32) -> Option<i32> {
    (v >= 0).then_some(v)
}

/// `facing` of a unit: `None` for a value outside `DirectionTypes`.
fn direction_at(v: i8) -> Option<Direction> {
    usize::try_from(v)
        .ok()
        .and_then(|i| Direction::ALL.get(i).copied())
}

/// Decodes a `Q_UNITS` reply.
pub fn parse_units(buf: &[u8]) -> Result<Vec<Unit>, QueryError> {
    let r = Reply::parse(buf, ffi::GC_Q_UNITS as u16)?;
    r.require_record_size(UNIT_RECORD_SIZE)?;
    // `transport_id`, `build_type` and `facing` are v2 fields: read only when the record has them.
    let v2 = r.record_size() >= UNIT_V2_RECORD_SIZE;
    r.records()
        .map(|b| {
            Ok(Unit {
                id: i32_at(b, 0),
                owner: b[4] as i8,
                unit_type: i16_at(b, 6),
                x: i16_at(b, 8),
                y: i16_at(b, 10),
                hp: i16_at(b, 12),
                max_hp: i16_at(b, 14),
                moves_left: i16_at(b, 16),
                max_moves: i16_at(b, 18),
                strength: i16_at(b, 20),
                ranged: i16_at(b, 22),
                range: b[24] as i8,
                domain: b[25] as i8,
                activity: Activity::from_abi(b[26] as i8),
                level: b[27] as i8,
                experience: i16_at(b, 28),
                flags: UnitFlags(u16_at(b, 30)),
                transport_id: if v2 { opt_i32(i32_at(b, 36)) } else { None },
                build_type: if v2 { opt_i16(i16_at(b, 40)) } else { None },
                facing: if v2 { direction_at(b[42] as i8) } else { None },
                name: r.string_or_empty(u32_at(b, 32))?,
            })
        })
        .collect()
}

/// Decodes a `Q_UNIT_ACTIONS` reply.
pub fn parse_unit_actions(buf: &[u8]) -> Result<Vec<UnitAction>, QueryError> {
    let r = Reply::parse(buf, ffi::GC_Q_UNIT_ACTIONS as u16)?;
    r.require_record_size(UNIT_ACTION_RECORD_SIZE)?;
    // `cost` is a UI v2 field: read only when the record has it.
    let has_cost = r.record_size() >= UNIT_ACTION_COST_RECORD_SIZE;
    r.records()
        .map(|b| {
            Ok(UnitAction {
                action: i16_at(b, 0),
                enabled: b[2] != 0,
                reason: r.opt_string(u32_at(b, 4))?,
                cost: if has_cost {
                    opt_i32(i32_at(b, 8))
                } else {
                    None
                },
            })
        })
        .collect()
}

fn path_nodes(buf: &[u8], kind: i32) -> Result<Vec<PathNode>, QueryError> {
    let r = Reply::parse(buf, kind as u16)?;
    r.require_record_size(PATH_NODE_RECORD_SIZE)?;
    // The flags are v2: read them only when the record has them.
    let has_flags = r.record_size() >= PATH_NODE_FLAGS_RECORD_SIZE;
    Ok(r.records()
        .map(|b| PathNode {
            x: i16_at(b, 0),
            y: i16_at(b, 2),
            turn: i16_at(b, 4),
            moves_left: i16_at(b, 6),
            flags: ReachFlags(if has_flags { u16_at(b, 8) } else { 0 }),
        })
        .collect())
}

/// Decodes a `Q_DIPLO` reply: one record per other living major civ, by id.
pub fn parse_diplo(buf: &[u8]) -> Result<Vec<DiploRelation>, QueryError> {
    let r = Reply::parse(buf, ffi::GC_Q_DIPLO as u16)?;
    r.require_record_size(DIPLO_RECORD_SIZE)?;
    Ok(r.records()
        .map(|b| DiploRelation {
            other: b[0] as i8,
            team: b[1] as i8,
            flags: DiploFlags(u16_at(b, 2)),
            war_locked_turns: i16_at(b, 4),
        })
        .collect())
}

/// Decodes a `Q_REACHABLE_TREE` reply: the plots the unit reaches within the horizon (the root is
/// not listed).
pub fn parse_reachable_tree(buf: &[u8]) -> Result<Vec<TreeNode>, QueryError> {
    let r = Reply::parse(buf, ffi::GC_Q_REACHABLE_TREE as u16)?;
    r.require_record_size(TREE_NODE_RECORD_SIZE)?;
    Ok(r.records()
        .map(|b| TreeNode {
            x: i16_at(b, 0),
            y: i16_at(b, 2),
            parent_x: i16_at(b, 4),
            parent_y: i16_at(b, 6),
            moves_left: i16_at(b, 8),
            turn: b[10],
            flags: ReachFlags(u16_at(b, 12)),
        })
        .collect())
}

/// Decodes a `Q_PLOT_BASIS` reply.
pub fn parse_plot_basis(buf: &[u8]) -> Result<Vec<PlotBasis>, QueryError> {
    let r = Reply::parse(buf, ffi::GC_Q_PLOT_BASIS as u16)?;
    r.require_record_size(PLOT_BASIS_RECORD_SIZE)?;
    Ok(r.records()
        .map(|b| PlotBasis {
            index: u32_at(b, 0),
            plot_type: PlotType::from_abi(b[4] as i8),
            continent_art: b[5],
            terrain: i16_at(b, 6),
            feature: opt_i16(i16_at(b, 8)),
            flags: BasisFlags(u16_at(b, 10)),
            rivers: b[12],
            river_flow: [b[13] as i8, b[14] as i8, b[15] as i8],
        })
        .collect())
}

/// Decodes a `Q_COMBAT_PREVIEW` reply: no record = the attacker cannot attack that plot.
pub fn parse_combat_preview(buf: &[u8]) -> Result<Option<CombatForecast>, QueryError> {
    let r = Reply::parse(buf, ffi::GC_Q_COMBAT_PREVIEW as u16)?;
    r.require_record_size(COMBAT_PREVIEW_RECORD_SIZE)?;
    match r.count() {
        0 => Ok(None),
        1 => {
            let b = r
                .record(0)
                .ok_or(QueryError::CountMismatch { want: 1, got: 0 })?;
            Ok(Some(CombatForecast {
                damage_to_target: i16_at(b, 0),
                damage_to_attacker: i16_at(b, 2),
                attacker_strength: i32_at(b, 4),
                defender_strength: i32_at(b, 8),
                target_hp: i16_at(b, 12),
                target_max_hp: i16_at(b, 14),
                flags: CombatFlags(u16_at(b, 16)),
            }))
        }
        n => Err(QueryError::CountMismatch {
            want: 1,
            got: n as u32,
        }),
    }
}

/// Decodes a `Q_PLOTS_DIRTY` reply: the plot indexes (`y * map_width + x`) that may have changed.
pub fn parse_plots_dirty(buf: &[u8]) -> Result<Vec<u32>, QueryError> {
    let r = Reply::parse(buf, ffi::GC_Q_PLOTS_DIRTY as u16)?;
    r.require_record_size(PLOT_DIRTY_RECORD_SIZE)?;
    Ok(r.records().map(|b| u32_at(b, 0)).collect())
}

/// Decodes a `Q_PATH` reply: first node = the unit's plot, no nodes = no path.
pub fn parse_path(buf: &[u8]) -> Result<Vec<PathNode>, QueryError> {
    path_nodes(buf, ffi::GC_Q_PATH)
}

/// Decodes a `Q_REACHABLE` reply: the plots the unit reaches this turn, by y then x.
pub fn parse_reachable(buf: &[u8]) -> Result<Vec<Reachable>, QueryError> {
    path_nodes(buf, ffi::GC_Q_REACHABLE)
}

/// Decodes a `Q_RANGE_TARGETS` reply: the plots the shooter can hit now, by y then x.
pub fn parse_range_targets(buf: &[u8]) -> Result<Vec<RangeTarget>, QueryError> {
    let r = Reply::parse(buf, ffi::GC_Q_RANGE_TARGETS as u16)?;
    r.require_record_size(RANGE_TARGET_RECORD_SIZE)?;
    Ok(r.records()
        .map(|b| RangeTarget {
            x: i16_at(b, 0),
            y: i16_at(b, 2),
            flags: RangeTargetFlags(u16_at(b, 4)),
        })
        .collect())
}

/// Decodes a `Q_NOTIFICATIONS` reply.
pub fn parse_notifications(buf: &[u8]) -> Result<Vec<Notification>, QueryError> {
    let r = Reply::parse(buf, ffi::GC_Q_NOTIFICATIONS as u16)?;
    r.require_record_size(NOTIFICATION_RECORD_SIZE)?;
    r.records()
        .map(|b| {
            Ok(Notification {
                id: i32_at(b, 0),
                kind: u32_at(b, 4),
                x: i16_at(b, 8),
                y: i16_at(b, 10),
                data1: i32_at(b, 12),
                data2: i32_at(b, 16),
                turn: i16_at(b, 20),
                dismissed: b[22] != 0,
                message: r.string_or_empty(u32_at(b, 24))?,
                summary: r.string_or_empty(u32_at(b, 28))?,
            })
        })
        .collect()
}

/// Decodes a `Q_CITIES` reply.
pub fn parse_cities(buf: &[u8]) -> Result<Vec<City>, QueryError> {
    let r = Reply::parse(buf, ffi::GC_Q_CITIES as u16)?;
    r.require_record_size(CITY_RECORD_SIZE)?;
    // `founder` is a v2 field, `overflow` a UI v2 field: read only when the record has them.
    let v2 = r.record_size() >= CITY_V2_RECORD_SIZE;
    let has_overflow = r.record_size() >= CITY_OVERFLOW_RECORD_SIZE;
    r.records()
        .map(|b| {
            let mut yields = [0i16; 6];
            for (i, y) in yields.iter_mut().enumerate() {
                *y = i16_at(b, 36 + 2 * i);
            }
            Ok(City {
                id: i32_at(b, 0),
                owner: b[4] as i8,
                production_order: (b[5] as i8 >= 0).then(|| OrderType::from_abi(b[5] as i8)),
                x: i16_at(b, 6),
                y: i16_at(b, 8),
                population: i16_at(b, 10),
                flags: CityFlags(u16_at(b, 12)),
                hp: i16_at(b, 14),
                max_hp: i16_at(b, 16),
                defense: i16_at(b, 18),
                production_item: opt_i16(i16_at(b, 20)),
                production_turns: opt_i16(i16_at(b, 22)),
                growth_turns: opt_i16(i16_at(b, 24)),
                production_stored: i32_at(b, 28),
                production_needed: i32_at(b, 32),
                yields,
                founder: if v2 { opt_u8(b[52] as i8) } else { None },
                overflow: if has_overflow {
                    opt_i32(i32_at(b, 56))
                } else {
                    None
                },
                name: r.string_or_empty(u32_at(b, 48))?,
            })
        })
        .collect()
}

/// Decodes a `Q_TOP_BAR` reply (one record).
pub fn parse_top_bar(buf: &[u8]) -> Result<TopBar, QueryError> {
    let r = Reply::parse(buf, ffi::GC_Q_TOP_BAR as u16)?;
    r.require_record_size(TOP_BAR_RECORD_SIZE)?;
    r.require_count(1)?;
    let b = r
        .record(0)
        .ok_or(QueryError::CountMismatch { want: 1, got: 0 })?;
    Ok(TopBar {
        gold: i32_at(b, 0),
        gold_per_turn: i32_at(b, 4),
        science_per_turn: i32_at(b, 8),
        research_tech: opt_i16(i16_at(b, 12)),
        research_turns: i16_at(b, 14),
        research_progress: i32_at(b, 16),
        research_cost: i32_at(b, 20),
        culture: i32_at(b, 24),
        culture_per_turn: i32_at(b, 28),
        next_policy_cost: i32_at(b, 32),
        happiness: i16_at(b, 36),
        golden_age_turns: i16_at(b, 38),
        faith: i32_at(b, 40),
        faith_per_turn: i32_at(b, 44),
        era: i16_at(b, 48),
        flags: TopBarFlags(u16_at(b, 50)),
        golden_age_progress: i32_at(b, 52),
        golden_age_threshold: i32_at(b, 56),
        // A UI v2 field: read only when the record has it.
        tourism: (r.record_size() >= TOP_BAR_TOURISM_RECORD_SIZE).then(|| i32_at(b, 60)),
    })
}

/// Decodes a `Q_CITY_BUILDS` reply.
pub fn parse_city_builds(buf: &[u8]) -> Result<Vec<CityBuild>, QueryError> {
    let r = Reply::parse(buf, ffi::GC_Q_CITY_BUILDS as u16)?;
    r.require_record_size(CITY_BUILD_RECORD_SIZE)?;
    r.records()
        .map(|b| {
            let cost = |v: i32| (v >= 0).then_some(v);
            Ok(CityBuild {
                order: OrderType::from_abi(b[0] as i8),
                item: i16_at(b, 2),
                turns: opt_i16(i16_at(b, 4)),
                flags: BuildFlags(u16_at(b, 6)),
                production_cost: i32_at(b, 8),
                gold_cost: cost(i32_at(b, 12)),
                reason: r.opt_string(u32_at(b, 16))?,
                faith_cost: cost(i32_at(b, 20)),
            })
        })
        .collect()
}

/// Decodes a `Q_CITY_ART` reply: the cities to draw, by the viewer's view.
pub fn parse_city_art(buf: &[u8]) -> Result<Vec<CityArt>, QueryError> {
    let r = Reply::parse(buf, ffi::GC_Q_CITY_ART as u16)?;
    r.require_record_size(CITY_ART_RECORD_SIZE)?;
    Ok(r.records()
        .map(|b| CityArt {
            city_id: i32_at(b, 0),
            owner: b[4],
            building: i16_at(b, 6),
            x: i16_at(b, 8),
            y: i16_at(b, 10),
        })
        .collect())
}

/// Decodes a `Q_TRADE_ROUTES` reply.
pub fn parse_trade_routes(buf: &[u8]) -> Result<Vec<TradeRoute>, QueryError> {
    let r = Reply::parse(buf, ffi::GC_Q_TRADE_ROUTES as u16)?;
    r.require_record_size(TRADE_ROUTE_RECORD_SIZE)?;
    Ok(r.records()
        .map(|b| TradeRoute {
            id: i32_at(b, 0),
            owner: b[4],
            dest_owner: b[5],
            domain: b[6] as i8,
            from: (i16_at(b, 8), i16_at(b, 10)),
            to: (i16_at(b, 12), i16_at(b, 14)),
        })
        .collect())
}

/// Decodes a `Q_TRADE_PATHS` reply: consecutive records of one route id make its path.
pub fn parse_trade_paths(buf: &[u8]) -> Result<Vec<TradePath>, QueryError> {
    let r = Reply::parse(buf, ffi::GC_Q_TRADE_PATHS as u16)?;
    r.require_record_size(TRADE_PLOT_RECORD_SIZE)?;
    let mut out: Vec<TradePath> = Vec::new();
    for b in r.records() {
        let id = i32_at(b, 0);
        let plot = (i16_at(b, 4), i16_at(b, 6));
        match out.last_mut() {
            Some(p) if p.id == id => p.plots.push(plot),
            _ => out.push(TradePath {
                id,
                plots: vec![plot],
            }),
        }
    }
    Ok(out)
}

/// Decodes a `Q_CITY_QUEUE` reply: the city's order queue, head first.
pub fn parse_city_queue(buf: &[u8]) -> Result<Vec<CityOrder>, QueryError> {
    let r = Reply::parse(buf, ffi::GC_Q_CITY_QUEUE as u16)?;
    r.require_record_size(CITY_ORDER_RECORD_SIZE)?;
    Ok(r.records()
        .map(|b| CityOrder {
            slot: i32_at(b, 0),
            order: OrderType::from_abi(b[4] as i8),
            item: i16_at(b, 6),
            turns: opt_i16(i16_at(b, 8)),
            needed: i32_at(b, 12),
        })
        .collect())
}

/// Decodes a `Q_TECHS` reply: every technology of the game data, by id.
pub fn parse_techs(buf: &[u8]) -> Result<Vec<TechState>, QueryError> {
    let r = Reply::parse(buf, ffi::GC_Q_TECHS as u16)?;
    r.require_record_size(TECH_RECORD_SIZE)?;
    Ok(r.records()
        .map(|b| TechState {
            tech: i16_at(b, 0),
            status: TechStatus::from_abi(b[2]),
            queue: u8::try_from(b[3] as i8).ok(),
            turns: opt_i16(i16_at(b, 4)),
            progress: i32_at(b, 8),
            cost: i32_at(b, 12),
        })
        .collect())
}

/// Decodes a `Q_PLAYER_RESOURCES` reply: the resources the player has or uses, by id.
pub fn parse_player_resources(buf: &[u8]) -> Result<Vec<PlayerResource>, QueryError> {
    let r = Reply::parse(buf, ffi::GC_Q_PLAYER_RESOURCES as u16)?;
    r.require_record_size(RESOURCE_RECORD_SIZE)?;
    Ok(r.records()
        .map(|b| PlayerResource {
            resource: i16_at(b, 0),
            usage: ResourceUsage::from_abi(b[2]),
            available: i32_at(b, 4),
            used: i32_at(b, 8),
            total: i32_at(b, 12),
        })
        .collect())
}

/// Decodes a `Q_POLICY_BRANCHES` reply: every policy branch, by id.
pub fn parse_policy_branches(buf: &[u8]) -> Result<Vec<PolicyBranchState>, QueryError> {
    let r = Reply::parse(buf, ffi::GC_Q_POLICY_BRANCHES as u16)?;
    r.require_record_size(POLICY_BRANCH_RECORD_SIZE)?;
    Ok(r.records()
        .map(|b| PolicyBranchState {
            branch: i16_at(b, 0),
            unlocked: b[2] != 0,
            owned: b[3],
            total: b[4],
        })
        .collect())
}

/// Decodes a `Q_UNIT_PROMOTIONS` reply: the `PromotionTypes` the unit has, by id.
pub fn parse_unit_promotions(buf: &[u8]) -> Result<Vec<i16>, QueryError> {
    let r = Reply::parse(buf, ffi::GC_Q_UNIT_PROMOTIONS as u16)?;
    r.require_record_size(PROMOTION_RECORD_SIZE)?;
    Ok(r.records().map(|b| i16_at(b, 0)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a `GCQ1` reply from records and strings.
    fn reply(kind: u16, record_size: usize, records: &[Vec<u8>], strings: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"GCQ1");
        b.extend_from_slice(&FORMAT.to_le_bytes());
        b.extend_from_slice(&kind.to_le_bytes());
        b.extend_from_slice(&(records.len() as u32).to_le_bytes());
        b.extend_from_slice(&(record_size as u32).to_le_bytes());
        b.extend_from_slice(&((HEADER_SIZE + records.len() * record_size) as u32).to_le_bytes());
        b.extend_from_slice(&(strings.len() as u32).to_le_bytes());
        for r in records {
            assert_eq!(r.len(), record_size);
            b.extend_from_slice(r);
        }
        b.extend_from_slice(strings);
        b
    }

    fn game_record() -> Vec<u8> {
        let mut r = Vec::new();
        for v in [3i32, -4000, -1, -1] {
            r.extend_from_slice(&v.to_le_bytes());
        }
        for v in [-1i16, -1, 40, 24] {
            r.extend_from_slice(&v.to_le_bytes());
        }
        // human_player, human_team, active_player, status, blocker, wrap_x, wrap_y, difficulty,
        // winner, victory_type, reserved x2
        r.extend_from_slice(&[0, 0, 0, 0, 0xFF, 1, 0, 3, 0xFF, 0xFF, 0, 0]);
        assert_eq!(r.len(), GAME_RECORD_SIZE);
        r
    }

    fn plot_record(x: i16, y: i16, flags: u16, owner: i8) -> Vec<u8> {
        let mut r = Vec::new();
        r.extend_from_slice(&x.to_le_bytes());
        r.extend_from_slice(&y.to_le_bytes());
        r.extend_from_slice(&flags.to_le_bytes());
        r.extend_from_slice(&[2, 0]); // plot_type LAND, reserved0
        r.extend_from_slice(&5i16.to_le_bytes()); // terrain
        r.extend_from_slice(&(-1i16).to_le_bytes()); // feature
        r.extend_from_slice(&7i16.to_le_bytes()); // resource
        r.extend_from_slice(&[2, 0]); // qty, reserved1
        r.extend_from_slice(&(-1i16).to_le_bytes()); // improvement
        r.extend_from_slice(&1i16.to_le_bytes()); // route
        r.push(owner as u8);
        r.push(3); // continent art
        r.extend_from_slice(&[1, 2, 0, 0, 0, 0xFF]); // yields
        assert_eq!(r.len(), PLOT_RECORD_SIZE);
        r
    }

    fn player_record(id: i8, name: u32, civ: u32) -> Vec<u8> {
        let mut r = vec![id as u8, id as u8];
        r.extend_from_slice(&4i16.to_le_bytes());
        r.extend_from_slice(&(-1i16).to_le_bytes());
        r.extend_from_slice(&0x0003u16.to_le_bytes());
        r.extend_from_slice(&0x11223344u32.to_le_bytes());
        r.extend_from_slice(&0x55667788u32.to_le_bytes());
        r.extend_from_slice(&name.to_le_bytes());
        r.extend_from_slice(&civ.to_le_bytes());
        r.extend_from_slice(&[2, 0, 0, 0]); // era, reserved
        assert_eq!(r.len(), PLAYER_RECORD_SIZE);
        r
    }

    fn catalog_record(id: i16, type_off: u32, name_off: u32, art: u32) -> Vec<u8> {
        let mut r = Vec::new();
        r.extend_from_slice(&id.to_le_bytes());
        r.extend_from_slice(&(-1i16).to_le_bytes());
        r.extend_from_slice(&2i16.to_le_bytes());
        r.extend_from_slice(&0i16.to_le_bytes());
        for off in [
            type_off,
            name_off,
            art,
            ffi::GCQ_NO_STRING,
            ffi::GCQ_NO_STRING,
            ffi::GCQ_NO_STRING,
            ffi::GCQ_NO_STRING,
        ] {
            r.extend_from_slice(&off.to_le_bytes());
        }
        assert_eq!(r.len(), CATALOG_RECORD_SIZE);
        r
    }

    #[test]
    fn game_record_decodes() {
        let b = reply(1, GAME_RECORD_SIZE, &[game_record()], b"");
        let g = parse_game(&b).unwrap();
        assert_eq!((g.turn, g.year), (3, -4000));
        assert_eq!((g.map_width, g.map_height), (40, 24));
        assert_eq!(g.status, GameStatus::WaitingHuman);
        assert!(g.wrap_x && !g.wrap_y);
        assert_eq!(g.blocker, -1);
        assert_eq!(g.difficulty, 3);
        assert_eq!(g.blocker_plot(), None);
        assert_eq!(g.winner, -1);
        assert_eq!(g.plot_count(), 960);
    }

    #[test]
    fn game_record_v2_fields_are_read_when_the_record_has_them() {
        let mut r = game_record();
        // game_type, turn_mode, phase, humans, humans_left, safe_point, unsafe_reason, reserved
        r.extend_from_slice(&[1, 2, 1, 3, 2, 0, 4, 0]);
        r.extend_from_slice(&90i32.to_le_bytes()); // turn_len_s
        r.extend_from_slice(&30i32.to_le_bytes()); // turn_base_s
        r.extend_from_slice(&0xDEAD_BEEFu32.to_le_bytes()); // map_seed
        assert_eq!(r.len(), GAME_V2_RECORD_SIZE);
        let g = parse_game(&reply(1, GAME_V2_RECORD_SIZE, &[r.clone()], b"")).unwrap();
        assert_eq!(g.turn, 3);
        assert_eq!(g.game_type, GameType::Network);
        assert_eq!(g.turn_mode, Some(TurnMode::Hybrid));
        assert_eq!(g.phase, Phase::Ai);
        assert_eq!((g.humans, g.humans_left), (3, 2));
        assert!(!g.safe_point);
        assert_eq!(g.unsafe_reason, Some(UnsafeReason::Automoves));
        assert_eq!(
            (g.turn_len_s, g.turn_base_s, g.map_seed),
            (90, 30, 0xDEAD_BEEF)
        );
        // A safe point with a simultaneous game; an unknown reason is kept.
        r[37] = 1;
        r[41] = 1;
        r[42] = 0;
        let g = parse_game(&reply(1, GAME_V2_RECORD_SIZE, &[r.clone()], b"")).unwrap();
        assert_eq!(g.turn_mode, Some(TurnMode::Simultaneous));
        assert!(g.safe_point);
        assert_eq!(g.unsafe_reason, None);
        r[42] = 9;
        let g = parse_game(&reply(1, GAME_V2_RECORD_SIZE, &[r], b"")).unwrap();
        assert_eq!(g.unsafe_reason, Some(UnsafeReason::Other(9)));
        // The v1 record: the v2 fields read as unknown.
        let g = parse_game(&reply(1, GAME_RECORD_SIZE, &[game_record()], b"")).unwrap();
        assert_eq!(g.game_type, GameType::Single);
        assert_eq!(g.turn_mode, None);
        assert_eq!((g.humans, g.humans_left, g.map_seed), (0, 0, 0));
        assert!(!g.safe_point);
    }

    fn turn_record(player: u8, flags: u16, order: i16, nick: u32) -> Vec<u8> {
        let mut r = vec![player, player, 3, 0xFF];
        r.extend_from_slice(&flags.to_le_bytes());
        r.extend_from_slice(&order.to_le_bytes());
        r.extend_from_slice(&(-1i32).to_le_bytes()); // blocker_unit
        r.extend_from_slice(&7i32.to_le_bytes()); // blocker_city
        r.extend_from_slice(&5i16.to_le_bytes()); // blocker_x
        r.extend_from_slice(&(-1i16).to_le_bytes()); // blocker_y
        r.extend_from_slice(&nick.to_le_bytes());
        assert_eq!(r.len(), TURN_RECORD_SIZE);
        r
    }

    #[test]
    fn turn_records_decode() {
        let flags = ffi::GC_TURN_F_HUMAN
            | ffi::GC_TURN_F_ALIVE
            | ffi::GC_TURN_F_CONNECTED
            | ffi::GC_TURN_F_TURN_ACTIVE
            | ffi::GC_TURN_F_WAITING_FOR;
        let b = reply(
            14,
            TURN_RECORD_SIZE,
            &[turn_record(1, flags, 0, 0), turn_record(2, 0, -1, u32::MAX)],
            b"anna ",
        );
        let t = parse_turn(&b).unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(
            (t[0].player, t[0].team, t[0].slot_status, t[0].blocker),
            (1, 1, 3, -1)
        );
        assert!(t[0].flags.human() && t[0].flags.connected() && t[0].flags.waiting_for());
        assert!(t[0].flags.turn_active() && !t[0].flags.turn_ended() && !t[0].flags.kicked());
        assert_eq!(t[0].turn_order, Some(0));
        assert_eq!(t[0].nick.as_deref(), Some("anna"));
        assert_eq!((t[0].blocker_unit, t[0].blocker_city), (-1, 7));
        assert_eq!(t[0].blocker_plot(), None);
        assert_eq!(t[1].turn_order, None);
        assert_eq!(t[1].nick, None);
        assert!(!t[1].flags.human());
        // Empty reply, and a record that is too short.
        assert!(
            parse_turn(&reply(14, TURN_RECORD_SIZE, &[], b""))
                .unwrap()
                .is_empty()
        );
        let short = reply(
            14,
            TURN_RECORD_SIZE - 4,
            &[turn_record(1, 0, 0, 0)[..20].to_vec()],
            b"",
        );
        assert!(matches!(
            parse_turn(&short),
            Err(QueryError::RecordTooSmall { .. })
        ));
    }

    #[test]
    fn plot_flags_rivers_and_options() {
        let flags = ffi::GC_PLOT_F_REVEALED
            | ffi::GC_PLOT_F_COASTAL
            | (1u16 << (ffi::GC_PLOT_F_RIVER_SHIFT + 1)) // E
            | (1u16 << (ffi::GC_PLOT_F_RIVER_SHIFT + 5)) // NW
            | ffi::GC_PLOT_F_NATURAL_WONDER;
        let b = reply(
            2,
            PLOT_RECORD_SIZE,
            &[
                plot_record(1, 2, flags, -1),
                plot_record(3, 4, 0, 5),
                plot_record(5, 6, ffi::GC_PLOT_F_REVEALED, 5),
            ],
            b"",
        );
        let p = parse_plots(&b).unwrap();
        assert_eq!(p.len(), 3);
        let a = &p[0];
        assert_eq!((a.x, a.y), (1, 2));
        assert!(a.revealed() && !a.visible() && a.coastal() && !a.lake() && !a.has_city());
        assert!(a.natural_wonder() && !a.pillaged());
        assert!(a.river(Direction::East) && a.river(Direction::NorthWest));
        assert!(!a.river(Direction::NorthEast) && !a.river(Direction::SouthWest));
        assert_eq!(
            a.rivers(),
            [false, true, false, false, false, true],
            "NE E SE SW W NW"
        );
        assert_eq!(a.plot_type, PlotType::Land);
        assert_eq!(
            (a.terrain, a.feature, a.resource, a.improvement, a.route),
            (5, None, Some(7), None, Some(1))
        );
        assert_eq!(a.owner, None);
        assert_eq!(a.yields, [1, 2, 0, 0, 0, -1]);
        assert_eq!(p[2].owner, Some(5));
        // Unrevealed: the zeroed/garbage ids read as "none".
        assert!(!p[1].revealed());
        assert_eq!(
            (p[1].owner, p[1].feature, p[1].improvement),
            (None, None, None)
        );
        assert_eq!(p[1].rivers(), [false; 6]);
    }

    #[test]
    fn players_and_catalog_read_strings() {
        let strings = b"Caesar\0Rome\0UNIT_WARRIOR\0Warrior\0";
        let b = reply(
            5,
            PLAYER_RECORD_SIZE,
            &[
                player_record(0, 0, 7),
                player_record(1, ffi::GCQ_NO_STRING, ffi::GCQ_NO_STRING),
            ],
            strings,
        );
        let p = parse_players(&b).unwrap();
        assert_eq!(
            (p[0].name.as_str(), p[0].civ_name.as_str()),
            ("Caesar", "Rome")
        );
        assert_eq!((p[0].civ_type, p[0].leader_type), (Some(4), None));
        assert!(p[0].flags.alive() && p[0].flags.human() && !p[0].flags.minor());
        assert_eq!(p[0].color_primary, 0x11223344);
        assert_eq!(p[1].name, "");

        let b = reply(
            13,
            CATALOG_RECORD_SIZE,
            &[catalog_record(9, 12, 25, ffi::GCQ_NO_STRING)],
            strings,
        );
        let c = parse_catalog(&b).unwrap();
        assert_eq!(c[0].id, 9);
        assert_eq!(c[0].type_name, "UNIT_WARRIOR");
        assert_eq!(c[0].name, "Warrior");
        assert_eq!(c[0].art_define, None);
        assert_eq!((c[0].icon_index, c[0].flag_index), (None, Some(2)));
    }

    #[test]
    fn timer_catalog_decodes() {
        let strings = b"TURNTIMER_DYNAMIC Dynamic ";
        let mut rec = Vec::new();
        rec.extend_from_slice(&3i16.to_le_bytes());
        rec.extend_from_slice(&0i16.to_le_bytes());
        rec.extend_from_slice(&0u32.to_le_bytes());
        rec.extend_from_slice(&18u32.to_le_bytes());
        for v in [30i32, 2, 4] {
            rec.extend_from_slice(&v.to_le_bytes());
        }
        let b = reply(13, CATALOG_TIMER_RECORD_SIZE, &[rec], strings);
        let t = parse_catalog_timers(&b).unwrap();
        assert_eq!((t[0].id, t[0].type_name.as_str()), (3, "TURNTIMER_DYNAMIC"));
        assert_eq!(t[0].name, "Dynamic");
        assert_eq!(
            (t[0].base_time, t[0].city_resource, t[0].unit_resource),
            (30, 2, 4)
        );
    }

    #[test]
    fn world_size_catalog_decodes() {
        let strings = b"WORLDSIZE_DUEL Duel ";
        let mut rec = Vec::new();
        rec.extend_from_slice(&0i16.to_le_bytes());
        rec.extend_from_slice(&0i16.to_le_bytes());
        rec.extend_from_slice(&0u32.to_le_bytes());
        rec.extend_from_slice(&15u32.to_le_bytes());
        for v in [40i16, 24, 2, 4] {
            rec.extend_from_slice(&v.to_le_bytes());
        }
        let b = reply(13, CATALOG_WORLD_SIZE_RECORD_SIZE, &[rec], strings);
        let w = parse_catalog_world_sizes(&b).unwrap();
        assert_eq!((w[0].id, w[0].type_name.as_str()), (0, "WORLDSIZE_DUEL"));
        assert_eq!(w[0].name, "Duel");
        assert_eq!(
            (
                w[0].width,
                w[0].height,
                w[0].default_players,
                w[0].default_minor
            ),
            (40, 24, 2, 4)
        );
    }

    fn ext_record(n: [i32; 4], s: [u32; 4]) -> Vec<u8> {
        let mut r = Vec::new();
        for v in n {
            r.extend_from_slice(&v.to_le_bytes());
        }
        for v in s {
            r.extend_from_slice(&v.to_le_bytes());
        }
        assert_eq!(r.len(), CATALOG_EXT_RECORD_SIZE);
        r
    }

    #[test]
    fn option_catalogs_decode() {
        let strings =
            b"GAMEOPTION_NO_BARBARIANS\0No Barbarians\0Help\0\0Maps\\a.lua\0\0\0".to_vec();
        let at = |needle: &[u8]| -> u32 {
            strings
                .windows(needle.len())
                .position(|w| w == needle)
                .unwrap() as u32
        };
        let (ty, nm, help) = (at(b"GAMEOPTION"), at(b"No Barbarians"), at(b"Help"));
        let empty = at(b"\0\0") + 1;
        let script = at(b"Maps\\a.lua");
        let rows = parse_catalog_ext(&reply(
            13,
            CATALOG_EXT_RECORD_SIZE,
            &[ext_record([7, 0, 1, 0], [ty, nm, help, empty])],
            &strings,
        ))
        .unwrap();
        let go = game_options_of(rows);
        assert_eq!(go.len(), 1);
        assert_eq!(
            (go[0].id, go[0].type_name.as_str(), go[0].name.as_str()),
            (7, "GAMEOPTION_NO_BARBARIANS", "No Barbarians")
        );
        assert_eq!(go[0].help, "Help");
        assert!(!go[0].default && go[0].visible && !go[0].supports_multiplayer);

        let rows = parse_catalog_ext(&reply(
            13,
            CATALOG_EXT_RECORD_SIZE,
            &[ext_record([-99, 2, 0, 1], [script, empty, nm, help])],
            &strings,
        ))
        .unwrap();
        let mo = map_options_of(rows.clone());
        assert_eq!(
            (mo[0].script.as_str(), mo[0].option_id.as_str()),
            ("Maps\\a.lua", "")
        );
        assert_eq!((mo[0].sort_priority, mo[0].default_value), (-99, 2));
        assert!(!mo[0].read_only && mo[0].hidden);
        let mv = map_option_values_of(rows.clone());
        assert_eq!((mv[0].value, mv[0].sort_index), (-99, 2));
        let nv = named_of(rows);
        assert_eq!((nv[0].id, nv[0].type_name.as_str()), (-99, "Maps\\a.lua"));
        // A short record is refused.
        assert!(parse_catalog_ext(&reply(13, 28, &[vec![0u8; 28]], b"")).is_err());
    }

    fn player_record_v2(id: i8, nick: u32, slot: i8, art: u64, flags: u16) -> Vec<u8> {
        let mut r = player_record(id, ffi::GCQ_NO_STRING, ffi::GCQ_NO_STRING);
        r[6..8].copy_from_slice(&flags.to_le_bytes());
        r.extend_from_slice(&nick.to_le_bytes());
        r.extend_from_slice(&[slot as u8, 0, 0, 0]);
        r.extend_from_slice(&0u32.to_le_bytes()); // reserved2
        r.extend_from_slice(&art.to_le_bytes());
        assert_eq!(r.len(), PLAYER_V2_RECORD_SIZE);
        r
    }

    #[test]
    fn players_v2_fields_are_read_when_the_record_has_them() {
        let strings = b"alice\0";
        let b = reply(
            5,
            PLAYER_V2_RECORD_SIZE,
            &[
                player_record_v2(0, 0, 2, 0b101, ffi::GC_PLAYER_F_HUMAN),
                player_record_v2(1, ffi::GCQ_NO_STRING, 3, 1 << 63, ffi::GC_PLAYER_F_OBSERVER),
            ],
            strings,
        );
        let p = parse_players(&b).unwrap();
        assert_eq!(p[0].nick.as_deref(), Some("alice"));
        assert_eq!((p[0].slot, p[0].art_techs), (2, 0b101));
        assert!(p[0].has_art_tech(0) && !p[0].has_art_tech(1) && p[0].has_art_tech(2));
        assert!(!p[0].has_art_tech(64));
        assert_eq!(p[1].nick, None);
        assert!(p[1].flags.observer() && !p[0].flags.observer());
        assert!(p[1].has_art_tech(63));
        assert_prefixes_fail(&b, parse_players);
        // A v1 record has none of them.
        let v1 = reply(
            5,
            PLAYER_RECORD_SIZE,
            &[player_record(0, ffi::GCQ_NO_STRING, ffi::GCQ_NO_STRING)],
            b"",
        );
        let p = parse_players(&v1).unwrap();
        assert_eq!(
            (p[0].nick.clone(), p[0].slot, p[0].art_techs),
            (None, -1, 0)
        );
        // A bad nick string is an error.
        let bad = reply(
            5,
            PLAYER_V2_RECORD_SIZE,
            &[player_record_v2(0, 99, 2, 0, 0)],
            strings,
        );
        assert_eq!(parse_players(&bad), Err(QueryError::StringOutOfBounds(99)));
        // A record in between (28 < size < 48) is read as v1.
        let mid = reply(
            5,
            40,
            &[[
                player_record(0, ffi::GCQ_NO_STRING, ffi::GCQ_NO_STRING),
                vec![7; 12],
            ]
            .concat()],
            b"",
        );
        assert_eq!(parse_players(&mid).unwrap()[0].slot, -1);
    }

    #[test]
    fn empty_reply_is_ok() {
        let b = reply(2, PLOT_RECORD_SIZE, &[], b"");
        assert!(parse_plots(&b).unwrap().is_empty());
    }

    #[test]
    fn longer_record_is_read_by_the_old_reader() {
        // Record N+4: the added field is skipped, the known ones still decode.
        let grown: Vec<Vec<u8>> = [plot_record(1, 2, 1, 3), plot_record(5, 6, 0, -1)]
            .into_iter()
            .map(|mut r| {
                r.extend_from_slice(&[9, 9, 9, 9]);
                r
            })
            .collect();
        let b = reply(2, PLOT_RECORD_SIZE + 4, &grown, b"");
        let p = parse_plots(&b).unwrap();
        assert_eq!((p[1].x, p[1].y), (5, 6));
        assert_eq!(p[0].owner, Some(3));
    }

    #[test]
    fn truncated_buffers_are_errors() {
        let good = reply(2, PLOT_RECORD_SIZE, &[plot_record(0, 0, 0, -1)], b"");
        assert_eq!(parse_plots(&good[..10]), Err(QueryError::TooShort(10)));
        assert_eq!(parse_plots(&[]), Err(QueryError::TooShort(0)));
        // Records cut short.
        assert!(matches!(
            parse_plots(&good[..good.len() - 1]),
            Err(QueryError::BadLayout { .. })
        ));
        // String table cut short.
        let with_str = reply(5, PLAYER_RECORD_SIZE, &[player_record(0, 0, 0)], b"abc\0");
        assert!(matches!(
            parse_players(&with_str[..with_str.len() - 2]),
            Err(QueryError::StringsOutOfBounds { .. })
        ));
        // Every prefix of a valid reply is an error and never a panic.
        for n in 0..with_str.len() {
            assert!(parse_players(&with_str[..n]).is_err(), "prefix {n}");
        }
    }

    #[test]
    fn header_mismatches_are_errors() {
        let good = reply(2, PLOT_RECORD_SIZE, &[plot_record(0, 0, 0, -1)], b"");
        let mut bad = good.clone();
        bad[0] = b'X';
        assert!(matches!(parse_plots(&bad), Err(QueryError::BadMagic(_))));
        let mut bad = good.clone();
        bad[4] = 2;
        assert_eq!(parse_plots(&bad), Err(QueryError::BadFormat(2)));
        assert_eq!(
            parse_players(&good),
            Err(QueryError::WrongKind {
                expected: 5,
                got: 2
            })
        );
        // record_size too small for the reader.
        let small = reply(2, 20, &[vec![0; 20]], b"");
        assert_eq!(
            parse_plots(&small),
            Err(QueryError::RecordTooSmall { need: 28, got: 20 })
        );
        // Wrong strings_offset.
        let mut bad = good.clone();
        bad[16] = 99;
        assert!(matches!(
            parse_plots(&bad),
            Err(QueryError::BadStringsOffset { got: 99, .. })
        ));
        // Count that overflows the buffer, and one that overflows usize math.
        let mut bad = good.clone();
        bad[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        bad[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            parse_plots(&bad),
            Err(QueryError::BadLayout { .. })
        ));
        // Q_GAME must hold exactly one record.
        let two = reply(1, GAME_RECORD_SIZE, &[game_record(), game_record()], b"");
        assert_eq!(
            parse_game(&two),
            Err(QueryError::CountMismatch { want: 1, got: 2 })
        );
    }

    #[test]
    fn string_errors() {
        // Offset out of the table.
        let b = reply(
            5,
            PLAYER_RECORD_SIZE,
            &[player_record(0, 100, ffi::GCQ_NO_STRING)],
            b"ab\0",
        );
        assert_eq!(parse_players(&b), Err(QueryError::StringOutOfBounds(100)));
        // Offset exactly at the end of the table.
        let b = reply(
            5,
            PLAYER_RECORD_SIZE,
            &[player_record(0, 3, ffi::GCQ_NO_STRING)],
            b"ab\0",
        );
        assert_eq!(parse_players(&b), Err(QueryError::StringOutOfBounds(3)));
        // No NUL before the table ends.
        let b = reply(
            5,
            PLAYER_RECORD_SIZE,
            &[player_record(0, 0, ffi::GCQ_NO_STRING)],
            b"abc",
        );
        assert_eq!(parse_players(&b), Err(QueryError::StringUnterminated(0)));
        // Invalid UTF-8.
        let b = reply(
            5,
            PLAYER_RECORD_SIZE,
            &[player_record(0, 0, ffi::GCQ_NO_STRING)],
            b"\xff\xfe\0",
        );
        assert_eq!(parse_players(&b), Err(QueryError::StringNotUtf8(0)));
        // Empty table with a real offset.
        let b = reply(
            5,
            PLAYER_RECORD_SIZE,
            &[player_record(0, 0, ffi::GCQ_NO_STRING)],
            b"",
        );
        assert_eq!(parse_players(&b), Err(QueryError::StringOutOfBounds(0)));
    }

    // ---- G1 queries: records built field by field exactly as the header lays them out ----

    fn push_i16(r: &mut Vec<u8>, v: i16) {
        r.extend_from_slice(&v.to_le_bytes());
    }

    fn push_i32(r: &mut Vec<u8>, v: i32) {
        r.extend_from_slice(&v.to_le_bytes());
    }

    fn push_u32(r: &mut Vec<u8>, v: u32) {
        r.extend_from_slice(&v.to_le_bytes());
    }

    fn unit_record(id: i32, owner: i8, name: u32, flags: u16) -> Vec<u8> {
        let mut r = Vec::new();
        push_i32(&mut r, id);
        r.extend_from_slice(&[owner as u8, 0]);
        for v in [5i16, 10, 11, 20, 30, 120, 60, 8, 0] {
            // type, x, y, hp, max_hp, moves_left (2 moves), max_moves, strength, ranged
            push_i16(&mut r, v);
        }
        r.extend_from_slice(&[0, 1, 4, 2]); // range, domain, activity SENTRY, level
        push_i16(&mut r, 15); // experience
        r.extend_from_slice(&flags.to_le_bytes());
        push_u32(&mut r, name);
        assert_eq!(r.len(), UNIT_RECORD_SIZE);
        r
    }

    fn action_record(action: i16, enabled: u8, reason: u32) -> Vec<u8> {
        let mut r = Vec::new();
        push_i16(&mut r, action);
        r.extend_from_slice(&[enabled, 0]);
        push_u32(&mut r, reason);
        assert_eq!(r.len(), UNIT_ACTION_RECORD_SIZE);
        r
    }

    fn node_record(x: i16, y: i16, turn: i16, moves: i16) -> Vec<u8> {
        let mut r = Vec::new();
        for v in [x, y, turn, moves] {
            push_i16(&mut r, v);
        }
        r
    }

    fn notification_record(id: i32, x: i16, y: i16, msg: u32, sum: u32) -> Vec<u8> {
        let mut r = Vec::new();
        push_i32(&mut r, id);
        push_u32(&mut r, 0xDEADBEEF);
        push_i16(&mut r, x);
        push_i16(&mut r, y);
        push_i32(&mut r, 7);
        push_i32(&mut r, -8);
        push_i16(&mut r, 12);
        r.extend_from_slice(&[0, 0]);
        push_u32(&mut r, msg);
        push_u32(&mut r, sum);
        assert_eq!(r.len(), NOTIFICATION_RECORD_SIZE);
        r
    }

    fn city_record(order: i8, flags: u16, turns: i16, name: u32) -> Vec<u8> {
        let mut r = Vec::new();
        push_i32(&mut r, 3);
        r.extend_from_slice(&[0, order as u8]);
        for v in [4i16, 5, 2] {
            push_i16(&mut r, v); // x, y, population
        }
        r.extend_from_slice(&flags.to_le_bytes());
        for v in [20i16, 20, 8] {
            push_i16(&mut r, v); // hp, max_hp, defense
        }
        push_i16(&mut r, if order >= 0 { 9 } else { -1 }); // item
        push_i16(&mut r, turns); // production_turns
        push_i16(&mut r, -1); // growth_turns
        push_i16(&mut r, 0);
        push_i32(&mut r, 14);
        push_i32(&mut r, 40);
        for v in [3i16, 2, 1, 1, 1, 0] {
            push_i16(&mut r, v);
        }
        push_u32(&mut r, name);
        assert_eq!(r.len(), CITY_RECORD_SIZE);
        r
    }

    fn top_bar_record() -> Vec<u8> {
        let mut r = Vec::new();
        push_i32(&mut r, 120); // gold
        push_i32(&mut r, 3);
        push_i32(&mut r, 2);
        push_i16(&mut r, -1); // research_tech
        push_i16(&mut r, -1);
        push_i32(&mut r, 0);
        push_i32(&mut r, 0);
        push_i32(&mut r, 11); // culture
        push_i32(&mut r, 1);
        push_i32(&mut r, 25); // next policy
        push_i16(&mut r, -2); // happiness
        push_i16(&mut r, 0);
        push_i32(&mut r, 5); // faith
        push_i32(&mut r, 1);
        push_i16(&mut r, 0); // era
        r.extend_from_slice(&0x0005u16.to_le_bytes()); // unhappy + golden age
        push_i32(&mut r, 10);
        push_i32(&mut r, 500);
        assert_eq!(r.len(), TOP_BAR_RECORD_SIZE);
        r
    }

    fn build_record(order: i8, item: i16, flags: u16, gold: i32, reason: u32) -> Vec<u8> {
        let mut r = vec![order as u8, 0];
        push_i16(&mut r, item);
        push_i16(&mut r, 7); // turns
        r.extend_from_slice(&flags.to_le_bytes());
        push_i32(&mut r, 30);
        push_i32(&mut r, gold);
        push_u32(&mut r, reason);
        push_i32(&mut r, -1); // faith_cost
        assert_eq!(r.len(), CITY_BUILD_RECORD_SIZE);
        r
    }

    /// Every record of `parse` fails cleanly for every proper prefix of `good`.
    fn assert_prefixes_fail<T: std::fmt::Debug>(
        good: &[u8],
        parse: impl Fn(&[u8]) -> Result<T, QueryError>,
    ) {
        assert!(parse(good).is_ok());
        for n in 0..good.len() {
            assert!(parse(&good[..n]).is_err(), "prefix {n}");
        }
    }

    #[test]
    fn units_decode() {
        let flags = ffi::GC_UNIT_F_CAN_MOVE | ffi::GC_UNIT_F_CIVILIAN | ffi::GC_UNIT_F_NEEDS_ORDERS;
        let b = reply(
            3,
            UNIT_RECORD_SIZE,
            &[
                unit_record(7, 0, 0, flags),
                unit_record(8, 2, ffi::GCQ_NO_STRING, ffi::GC_UNIT_F_FORTIFIED),
            ],
            b"Settler\0",
        );
        let u = parse_units(&b).unwrap();
        assert_eq!(u.len(), 2);
        assert_eq!((u[0].id, u[0].owner, u[0].unit_type), (7, 0, 5));
        assert_eq!((u[0].x, u[0].y, u[0].hp, u[0].max_hp), (10, 11, 20, 30));
        assert_eq!(u[0].moves_left, 120);
        assert_eq!(u[0].moves_left / crate::play::MOVE_DENOMINATOR, 2);
        assert_eq!((u[0].max_moves, u[0].strength, u[0].ranged), (60, 8, 0));
        assert_eq!((u[0].range, u[0].domain, u[0].level), (0, 1, 2));
        assert_eq!(u[0].activity, Activity::Sentry);
        assert_eq!(u[0].experience, 15);
        assert!(u[0].flags.can_move() && u[0].flags.civilian() && u[0].flags.needs_orders());
        assert!(!u[0].flags.fortified() && !u[0].flags.embarked() && !u[0].flags.garrisoned());
        assert_eq!(u[0].name, "Settler");
        assert!(u[1].flags.fortified() && !u[1].flags.can_move());
        assert_eq!((u[1].owner, u[1].name.as_str()), (2, ""));
        assert!(
            parse_units(&reply(3, UNIT_RECORD_SIZE, &[], b""))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn units_errors_and_growth() {
        let good = reply(3, UNIT_RECORD_SIZE, &[unit_record(1, 0, 0, 0)], b"Ab\0");
        assert_prefixes_fail(&good, parse_units);
        // Bad string offset / unterminated / not UTF-8.
        let b = reply(3, UNIT_RECORD_SIZE, &[unit_record(1, 0, 9, 0)], b"Ab\0");
        assert_eq!(parse_units(&b), Err(QueryError::StringOutOfBounds(9)));
        let b = reply(3, UNIT_RECORD_SIZE, &[unit_record(1, 0, 0, 0)], b"Ab");
        assert_eq!(parse_units(&b), Err(QueryError::StringUnterminated(0)));
        let b = reply(
            3,
            UNIT_RECORD_SIZE,
            &[unit_record(1, 0, 0, 0)],
            b"\xc3\x28\0",
        );
        assert_eq!(parse_units(&b), Err(QueryError::StringNotUtf8(0)));
        // Too small / wrong kind.
        let small = reply(3, 20, &[vec![0; 20]], b"");
        assert_eq!(
            parse_units(&small),
            Err(QueryError::RecordTooSmall { need: 36, got: 20 })
        );
        assert!(matches!(
            parse_units(&reply(4, UNIT_RECORD_SIZE, &[], b"")),
            Err(QueryError::WrongKind {
                expected: 3,
                got: 4
            })
        ));
        // A bigger record is accepted and the extra bytes skipped.
        let grown: Vec<Vec<u8>> = [unit_record(1, 0, 0, 0), unit_record(2, 1, 0, 0)]
            .into_iter()
            .map(|mut r| {
                r.extend_from_slice(&[0xAA; 8]);
                r
            })
            .collect();
        let u = parse_units(&reply(3, UNIT_RECORD_SIZE + 8, &grown, b"Ab\0")).unwrap();
        assert_eq!((u[1].id, u[1].owner, u[1].name.as_str()), (2, 1, "Ab"));
    }

    #[test]
    fn unit_actions_decode() {
        let b = reply(
            8,
            UNIT_ACTION_RECORD_SIZE,
            &[
                action_record(3, 1, ffi::GCQ_NO_STRING),
                action_record(4, 0, 0),
            ],
            b"Cannot fortify\0",
        );
        let a = parse_unit_actions(&b).unwrap();
        assert_eq!(
            a,
            vec![
                UnitAction {
                    action: 3,
                    enabled: true,
                    reason: None,
                    cost: None
                },
                UnitAction {
                    action: 4,
                    enabled: false,
                    reason: Some("Cannot fortify".into()),
                    cost: None
                }
            ]
        );
        assert_prefixes_fail(&b, parse_unit_actions);
        let bad = reply(
            8,
            UNIT_ACTION_RECORD_SIZE,
            &[action_record(4, 0, 50)],
            b"x\0",
        );
        assert_eq!(
            parse_unit_actions(&bad),
            Err(QueryError::StringOutOfBounds(50))
        );
        let grown = reply(
            8,
            12,
            &[[action_record(5, 1, ffi::GCQ_NO_STRING), vec![1; 4]].concat()],
            b"",
        );
        assert_eq!(parse_unit_actions(&grown).unwrap()[0].action, 5);
    }

    #[test]
    fn path_and_reachable_decode() {
        let nodes = [node_record(1, 2, 0, 60), node_record(2, 2, 1, 0)];
        let b = reply(9, PATH_NODE_RECORD_SIZE, &nodes, b"");
        let p = parse_path(&b).unwrap();
        assert_eq!(
            p,
            vec![
                PathNode {
                    x: 1,
                    y: 2,
                    turn: 0,
                    moves_left: 60,
                    flags: ReachFlags(0)
                },
                PathNode {
                    x: 2,
                    y: 2,
                    turn: 1,
                    moves_left: 0,
                    flags: ReachFlags(0)
                }
            ]
        );
        assert!(
            parse_path(&reply(9, PATH_NODE_RECORD_SIZE, &[], b""))
                .unwrap()
                .is_empty(),
            "no path = no nodes"
        );
        assert_prefixes_fail(&b, parse_path);
        // The same record under the Q_REACHABLE kind; the kinds are not interchangeable.
        let r = reply(10, PATH_NODE_RECORD_SIZE, &nodes, b"");
        assert_eq!(parse_reachable(&r).unwrap().len(), 2);
        assert!(matches!(
            parse_reachable(&b),
            Err(QueryError::WrongKind {
                expected: 10,
                got: 9
            })
        ));
        assert_prefixes_fail(&r, parse_reachable);
        let grown = reply(
            10,
            12,
            &[[node_record(3, 4, 0, 30), vec![0; 4]].concat()],
            b"",
        );
        assert_eq!(parse_reachable(&grown).unwrap()[0].x, 3);
        let small = reply(9, 6, &[vec![0; 6]], b"");
        assert_eq!(
            parse_path(&small),
            Err(QueryError::RecordTooSmall { need: 8, got: 6 })
        );
    }

    fn range_target_record(x: i16, y: i16, flags: u16) -> Vec<u8> {
        let mut r = Vec::new();
        push_i16(&mut r, x);
        push_i16(&mut r, y);
        r.extend_from_slice(&flags.to_le_bytes());
        r.extend_from_slice(&[0, 0]); // reserved
        assert_eq!(r.len(), RANGE_TARGET_RECORD_SIZE);
        r
    }

    #[test]
    fn range_targets_decode() {
        let b = reply(
            11,
            RANGE_TARGET_RECORD_SIZE,
            &[
                range_target_record(17, 10, 0),
                range_target_record(18, 11, ffi::GC_RANGE_TARGET_F_CITY),
            ],
            b"",
        );
        let t = parse_range_targets(&b).unwrap();
        assert_eq!(
            t,
            vec![
                RangeTarget {
                    x: 17,
                    y: 10,
                    flags: RangeTargetFlags(0)
                },
                RangeTarget {
                    x: 18,
                    y: 11,
                    flags: RangeTargetFlags(ffi::GC_RANGE_TARGET_F_CITY)
                }
            ]
        );
        assert!(!t[0].flags.city() && t[1].flags.city());
        assert!(
            parse_range_targets(&reply(11, RANGE_TARGET_RECORD_SIZE, &[], b""))
                .unwrap()
                .is_empty(),
            "nothing to shoot at = no records"
        );
        assert_prefixes_fail(&b, parse_range_targets);
        // Another kind with the same record size is not a range target reply.
        assert_eq!(
            parse_range_targets(&reply(10, 8, &[node_record(1, 1, 0, 0)], b"")),
            Err(QueryError::WrongKind {
                expected: 11,
                got: 10
            })
        );
        let small = reply(11, 6, &[vec![0; 6]], b"");
        assert_eq!(
            parse_range_targets(&small),
            Err(QueryError::RecordTooSmall { need: 8, got: 6 })
        );
        // Appended fields are skipped.
        let grown = reply(
            11,
            RANGE_TARGET_RECORD_SIZE + 4,
            &[[range_target_record(3, 4, 1), vec![7; 4]].concat()],
            b"",
        );
        let g = parse_range_targets(&grown).unwrap();
        assert_eq!((g[0].x, g[0].y, g[0].flags.city()), (3, 4, true));
        // The shooter argument: a unit id as is, a city as -id; ids the encoding cannot carry are None.
        use crate::play::RangeShooter;
        assert_eq!(RangeShooter::Unit(24578).abi(), Some(24578));
        assert_eq!(RangeShooter::Unit(0).abi(), Some(0));
        assert_eq!(RangeShooter::City(8192).abi(), Some(-8192));
        assert_eq!(RangeShooter::Unit(-3).abi(), None);
        assert_eq!(RangeShooter::City(0).abi(), None);
        assert_eq!(RangeShooter::City(-5).abi(), None);
    }

    #[test]
    fn notifications_decode() {
        let b = reply(
            12,
            NOTIFICATION_RECORD_SIZE,
            &[
                notification_record(4, 9, 3, 0, 8),
                notification_record(5, -1, -1, 8, ffi::GCQ_NO_STRING),
            ],
            b"Hello\0\0\0Sum\0Bye\0",
        );
        // Offsets: "Hello\0" at 0, "\0\0" padding, "Sum\0" at 8.
        let n = parse_notifications(&b).unwrap();
        assert_eq!((n[0].id, n[0].kind, n[0].turn), (4, 0xDEADBEEF, 12));
        assert_eq!((n[0].data1, n[0].data2), (7, -8));
        assert_eq!(n[0].plot(), Some((9, 3)));
        assert_eq!(
            (n[0].message.as_str(), n[0].summary.as_str()),
            ("Hello", "Sum")
        );
        assert!(!n[0].dismissed);
        assert_eq!(n[1].plot(), None);
        assert_eq!((n[1].message.as_str(), n[1].summary.as_str()), ("Sum", ""));
        assert_prefixes_fail(&b, parse_notifications);
        let bad = reply(
            12,
            NOTIFICATION_RECORD_SIZE,
            &[notification_record(1, 0, 0, 0, 77)],
            b"a\0",
        );
        assert_eq!(
            parse_notifications(&bad),
            Err(QueryError::StringOutOfBounds(77))
        );
        let unterminated = reply(
            12,
            NOTIFICATION_RECORD_SIZE,
            &[notification_record(1, 0, 0, 0, ffi::GCQ_NO_STRING)],
            b"abc",
        );
        assert_eq!(
            parse_notifications(&unterminated),
            Err(QueryError::StringUnterminated(0))
        );
    }

    #[test]
    fn cities_decode() {
        let own = ffi::GC_CITY_F_CAPITAL | ffi::GC_CITY_F_VISIBLE | ffi::GC_CITY_F_CAN_RANGE_ATTACK;
        let b = reply(
            4,
            CITY_RECORD_SIZE,
            &[
                city_record(0, own, 5, 0),
                city_record(-1, 0, -1, 5), // a fogged foreign city
            ],
            b"Rome\0Veii\0",
        );
        let c = parse_cities(&b).unwrap();
        assert_eq!((c[0].id, c[0].owner, c[0].name.as_str()), (3, 0, "Rome"));
        assert_eq!((c[0].x, c[0].y, c[0].population), (4, 5, 2));
        assert!(c[0].flags.capital() && c[0].flags.visible() && c[0].flags.can_range_attack());
        assert!(!c[0].flags.puppet() && !c[0].flags.razing() && !c[0].flags.occupied());
        assert!(!c[0].flags.resistance());
        assert_eq!((c[0].hp, c[0].max_hp, c[0].defense), (20, 20, 8));
        assert_eq!(c[0].production_order, Some(OrderType::Train));
        assert_eq!(c[0].production_item, Some(9));
        assert_eq!((c[0].production_turns, c[0].growth_turns), (Some(5), None));
        assert_eq!((c[0].production_stored, c[0].production_needed), (14, 40));
        assert_eq!(c[0].yields, [3, 2, 1, 1, 1, 0]);
        assert_eq!(c[1].name, "Veii");
        assert!(!c[1].flags.visible());
        assert_eq!(
            (
                c[1].production_order,
                c[1].production_item,
                c[1].production_turns
            ),
            (None, None, None)
        );
        assert_prefixes_fail(&b, parse_cities);
        let bad = reply(4, CITY_RECORD_SIZE, &[city_record(0, 0, 1, 40)], b"Rome\0");
        assert_eq!(parse_cities(&bad), Err(QueryError::StringOutOfBounds(40)));
        let grown = reply(
            4,
            CITY_RECORD_SIZE + 4,
            &[[city_record(1, 0, 1, 0), vec![9; 4]].concat()],
            b"Rome\0",
        );
        let g = parse_cities(&grown).unwrap();
        assert_eq!(g[0].production_order, Some(OrderType::Construct));
        assert_eq!(g[0].name, "Rome");
        let small = reply(4, 48, &[vec![0; 48]], b"");
        assert_eq!(
            parse_cities(&small),
            Err(QueryError::RecordTooSmall { need: 52, got: 48 })
        );
    }

    /// A v1 plot record with the v2 `build_type` and `build_pct` appended.
    fn plot_record_v2(x: i16, y: i16, flags: u16, build: i16, pct: u8) -> Vec<u8> {
        let mut r = plot_record(x, y, flags, -1);
        push_i16(&mut r, build);
        r.extend_from_slice(&[pct, 0]); // build_pct, reserved
        assert_eq!(r.len(), PLOT_V2_RECORD_SIZE);
        r
    }

    #[test]
    fn plots_v2_build_fields_are_read_when_the_record_has_them() {
        let flags = ffi::GC_PLOT_F_REVEALED
            | ffi::GC_PLOT_F_VISIBLE
            | ffi::GC_PLOT_F_WORKED
            | ffi::GC_PLOT_F_ROUTE_PILLAGED;
        let b = reply(
            2,
            PLOT_V2_RECORD_SIZE,
            &[
                plot_record_v2(1, 2, flags, 4, 60),
                plot_record_v2(3, 4, ffi::GC_PLOT_F_REVEALED, -1, 0),
            ],
            b"",
        );
        let p = parse_plots(&b).unwrap();
        assert_eq!((p[0].build_type, p[0].build_pct), (Some(4), 60));
        assert!(p[0].flags.worked() && p[0].flags.route_pillaged());
        assert_eq!((p[1].build_type, p[1].build_pct), (None, 0));
        assert!(!p[1].flags.worked() && !p[1].flags.route_pillaged());
        assert_prefixes_fail(&b, parse_plots);
        // A v1 record has no build fields.
        let v1 = reply(2, PLOT_RECORD_SIZE, &[plot_record(1, 2, 0, -1)], b"");
        let p = parse_plots(&v1).unwrap();
        assert_eq!((p[0].build_type, p[0].build_pct), (None, 0));
    }

    /// A v1 unit record with the v2 `transport_id`, `build_type`, `facing` appended.
    fn unit_record_v2(id: i32, flags: u16, transport: i32, build: i16, facing: u8) -> Vec<u8> {
        let mut r = unit_record(id, 0, ffi::GCQ_NO_STRING, flags);
        push_i32(&mut r, transport);
        push_i16(&mut r, build);
        r.extend_from_slice(&[facing, 0]); // facing, reserved
        assert_eq!(r.len(), UNIT_V2_RECORD_SIZE);
        r
    }

    #[test]
    fn units_v2_fields_are_read_when_the_record_has_them() {
        let carried = ffi::GC_UNIT_F_CARGO | ffi::GC_UNIT_F_CIVILIAN;
        let b = reply(
            3,
            UNIT_V2_RECORD_SIZE,
            &[
                unit_record_v2(7, carried, 12, 9, 3),
                // -1 = none; facing 9 is outside DirectionTypes.
                unit_record_v2(8, 0, -1, -1, 9),
            ],
            b"",
        );
        let u = parse_units(&b).unwrap();
        assert_eq!(
            (u[0].transport_id, u[0].build_type, u[0].facing),
            (Some(12), Some(9), Some(Direction::SouthWest))
        );
        assert!(u[0].flags.cargo() && !u[1].flags.cargo());
        assert_eq!(
            (u[1].transport_id, u[1].build_type, u[1].facing),
            (None, None, None)
        );
        assert_prefixes_fail(&b, parse_units);
        // A v1 record has none of them.
        let v1 = reply(
            3,
            UNIT_RECORD_SIZE,
            &[unit_record(7, 0, ffi::GCQ_NO_STRING, carried)],
            b"",
        );
        let u = parse_units(&v1).unwrap();
        assert_eq!(
            (u[0].transport_id, u[0].build_type, u[0].facing),
            (None, None, None)
        );
    }

    /// A v1 city record with the v2 `founder` appended.
    fn city_record_v2(founder: i8) -> Vec<u8> {
        let mut r = city_record(0, 0, 1, ffi::GCQ_NO_STRING);
        r.extend_from_slice(&[founder as u8, 0, 0, 0]); // founder, reserved
        assert_eq!(r.len(), CITY_V2_RECORD_SIZE);
        r
    }

    #[test]
    fn cities_v2_founder_is_read_when_the_record_has_it() {
        let b = reply(
            4,
            CITY_V2_RECORD_SIZE,
            &[city_record_v2(1), city_record_v2(-1)],
            b"",
        );
        let c = parse_cities(&b).unwrap();
        assert_eq!((c[0].founder, c[1].founder), (Some(1), None));
        assert_prefixes_fail(&b, parse_cities);
        let v1 = reply(
            4,
            CITY_RECORD_SIZE,
            &[city_record(0, 0, 1, ffi::GCQ_NO_STRING)],
            b"",
        );
        assert_eq!(parse_cities(&v1).unwrap()[0].founder, None);
    }

    fn city_art_record(city_id: i32, owner: u8, building: i16, x: i16, y: i16) -> Vec<u8> {
        let mut r = Vec::new();
        push_i32(&mut r, city_id);
        r.extend_from_slice(&[owner, 0]); // owner, reserved
        push_i16(&mut r, building);
        push_i16(&mut r, x);
        push_i16(&mut r, y);
        assert_eq!(r.len(), CITY_ART_RECORD_SIZE);
        r
    }

    #[test]
    fn city_art_decodes() {
        let b = reply(
            17,
            CITY_ART_RECORD_SIZE,
            &[
                city_art_record(3, 0, 27, 4, 5),
                city_art_record(9, 2, -1, -1, 8),
            ],
            b"",
        );
        assert_eq!(
            parse_city_art(&b).unwrap(),
            vec![
                CityArt {
                    city_id: 3,
                    owner: 0,
                    building: 27,
                    x: 4,
                    y: 5
                },
                CityArt {
                    city_id: 9,
                    owner: 2,
                    building: -1,
                    x: -1,
                    y: 8
                },
            ]
        );
        assert_prefixes_fail(&b, parse_city_art);
        let empty = reply(17, CITY_ART_RECORD_SIZE, &[], b"");
        assert!(parse_city_art(&empty).unwrap().is_empty());
        let small = reply(17, 8, &[vec![0; 8]], b"");
        assert_eq!(
            parse_city_art(&small),
            Err(QueryError::RecordTooSmall { need: 12, got: 8 })
        );
    }

    fn trade_route_record(
        id: i32,
        owner: u8,
        dest: u8,
        domain: i8,
        from: (i16, i16),
        to: (i16, i16),
    ) -> Vec<u8> {
        let mut r = Vec::new();
        push_i32(&mut r, id);
        r.extend_from_slice(&[owner, dest, domain as u8, 0]); // owner, dest_owner, domain, reserved
        for v in [from.0, from.1, to.0, to.1] {
            push_i16(&mut r, v);
        }
        assert_eq!(r.len(), TRADE_ROUTE_RECORD_SIZE);
        r
    }

    #[test]
    fn trade_routes_decode() {
        let b = reply(
            18,
            TRADE_ROUTE_RECORD_SIZE,
            &[
                trade_route_record(4, 0, 1, 1, (2, 3), (8, 9)),
                trade_route_record(5, 1, 0, 0, (-1, -1), (0, 0)),
            ],
            b"",
        );
        let t = parse_trade_routes(&b).unwrap();
        assert_eq!(
            t[0],
            TradeRoute {
                id: 4,
                owner: 0,
                dest_owner: 1,
                domain: 1,
                from: (2, 3),
                to: (8, 9)
            }
        );
        assert_eq!((t[1].owner, t[1].domain), (1, 0));
        assert_prefixes_fail(&b, parse_trade_routes);
        let empty = reply(18, TRADE_ROUTE_RECORD_SIZE, &[], b"");
        assert!(parse_trade_routes(&empty).unwrap().is_empty());
        let small = reply(18, 12, &[vec![0; 12]], b"");
        assert_eq!(
            parse_trade_routes(&small),
            Err(QueryError::RecordTooSmall { need: 16, got: 12 })
        );
    }

    fn trade_plot_record(route: i32, x: i16, y: i16) -> Vec<u8> {
        let mut r = Vec::new();
        push_i32(&mut r, route);
        push_i16(&mut r, x);
        push_i16(&mut r, y);
        assert_eq!(r.len(), TRADE_PLOT_RECORD_SIZE);
        r
    }

    /// Consecutive records of one route are its path, in order; the next id starts the next path.
    #[test]
    fn trade_paths_group_the_plots_of_each_route() {
        let b = reply(
            22,
            TRADE_PLOT_RECORD_SIZE,
            &[
                trade_plot_record(4, 2, 3),
                trade_plot_record(4, 3, 3),
                trade_plot_record(4, 4, 4),
                trade_plot_record(7, 9, 9),
                trade_plot_record(7, 8, 9),
            ],
            b"",
        );
        let t = parse_trade_paths(&b).unwrap();
        assert_eq!(
            t,
            vec![
                TradePath {
                    id: 4,
                    plots: vec![(2, 3), (3, 3), (4, 4)]
                },
                TradePath {
                    id: 7,
                    plots: vec![(9, 9), (8, 9)]
                },
            ]
        );
        assert_prefixes_fail(&b, parse_trade_paths);
        assert!(
            parse_trade_paths(&reply(22, TRADE_PLOT_RECORD_SIZE, &[], b""))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            parse_trade_paths(&reply(22, 4, &[vec![0; 4]], b"")),
            Err(QueryError::RecordTooSmall { need: 8, got: 4 })
        );
    }

    #[test]
    fn top_bar_decodes() {
        let b = reply(6, TOP_BAR_RECORD_SIZE, &[top_bar_record()], b"");
        let t = parse_top_bar(&b).unwrap();
        assert_eq!((t.gold, t.gold_per_turn, t.science_per_turn), (120, 3, 2));
        assert_eq!(t.research_tech, None);
        assert_eq!(
            (t.research_turns, t.research_progress, t.research_cost),
            (-1, 0, 0)
        );
        assert_eq!(
            (t.culture, t.culture_per_turn, t.next_policy_cost),
            (11, 1, 25)
        );
        assert_eq!((t.happiness, t.golden_age_turns), (-2, 0));
        assert_eq!((t.faith, t.faith_per_turn, t.era), (5, 1, 0));
        assert!(t.flags.unhappy() && t.flags.golden_age() && !t.flags.very_unhappy());
        assert_eq!((t.golden_age_progress, t.golden_age_threshold), (10, 500));
        assert_prefixes_fail(&b, parse_top_bar);
        // Exactly one record.
        let none = reply(6, TOP_BAR_RECORD_SIZE, &[], b"");
        assert_eq!(
            parse_top_bar(&none),
            Err(QueryError::CountMismatch { want: 1, got: 0 })
        );
        let two = reply(
            6,
            TOP_BAR_RECORD_SIZE,
            &[top_bar_record(), top_bar_record()],
            b"",
        );
        assert_eq!(
            parse_top_bar(&two),
            Err(QueryError::CountMismatch { want: 1, got: 2 })
        );
        let grown = reply(
            6,
            TOP_BAR_RECORD_SIZE + 8,
            &[[top_bar_record(), vec![0; 8]].concat()],
            b"",
        );
        assert_eq!(parse_top_bar(&grown).unwrap().gold, 120);
        let small = reply(6, 56, &[vec![0; 56]], b"");
        assert_eq!(
            parse_top_bar(&small),
            Err(QueryError::RecordTooSmall { need: 60, got: 56 })
        );
    }

    #[test]
    fn city_builds_decode() {
        let flags =
            ffi::GC_BUILD_F_ENABLED | ffi::GC_BUILD_F_CAN_PURCHASE | ffi::GC_BUILD_F_CURRENT;
        let b = reply(
            7,
            CITY_BUILD_RECORD_SIZE,
            &[
                build_record(0, 9, flags, 60, ffi::GCQ_NO_STRING),
                build_record(1, 3, ffi::GC_BUILD_F_WONDER, -1, 0),
                build_record(4, 1, 0, 0, ffi::GCQ_NO_STRING),
            ],
            b"Needs a Granary\0",
        );
        let c = parse_city_builds(&b).unwrap();
        assert_eq!(
            (c[0].order, c[0].item, c[0].turns),
            (OrderType::Train, 9, Some(7))
        );
        assert!(c[0].flags.enabled() && c[0].flags.can_purchase() && c[0].flags.current());
        assert!(!c[0].flags.wonder());
        assert_eq!(
            (c[0].production_cost, c[0].gold_cost, c[0].faith_cost),
            (30, Some(60), None)
        );
        assert_eq!(c[0].reason, None);
        assert_eq!(c[1].order, OrderType::Construct);
        assert!(c[1].flags.wonder() && !c[1].flags.enabled());
        assert_eq!(
            (c[1].gold_cost, c[1].reason.as_deref()),
            (None, Some("Needs a Granary"))
        );
        assert_eq!(c[2].order, OrderType::Maintain);
        assert_prefixes_fail(&b, parse_city_builds);
        let bad = reply(
            7,
            CITY_BUILD_RECORD_SIZE,
            &[build_record(0, 1, 0, 0, 99)],
            b"a\0",
        );
        assert_eq!(
            parse_city_builds(&bad),
            Err(QueryError::StringOutOfBounds(99))
        );
        let grown = reply(
            7,
            CITY_BUILD_RECORD_SIZE + 4,
            &[[build_record(2, 1, 0, 0, ffi::GCQ_NO_STRING), vec![0; 4]].concat()],
            b"",
        );
        assert_eq!(
            parse_city_builds(&grown).unwrap()[0].order,
            OrderType::Create
        );
        let unknown = reply(
            7,
            CITY_BUILD_RECORD_SIZE,
            &[build_record(9, 1, 0, 0, ffi::GCQ_NO_STRING)],
            b"",
        );
        assert_eq!(
            parse_city_builds(&unknown).unwrap()[0].order,
            OrderType::Other(9)
        );
    }

    // ---- v2 queries: Q_PATH flags, Q_DIPLO, Q_REACHABLE_TREE, Q_PLOT_BASIS, Q_COMBAT_PREVIEW, Q_PLOTS_DIRTY ----

    fn flagged_node(x: i16, y: i16, turn: i16, moves: i16, flags: u16) -> Vec<u8> {
        let mut r = node_record(x, y, turn, moves);
        r.extend_from_slice(&flags.to_le_bytes());
        r.extend_from_slice(&[0, 0]); // reserved
        assert_eq!(r.len(), PATH_NODE_FLAGS_RECORD_SIZE);
        r
    }

    #[test]
    fn path_node_flags_are_read_only_when_the_record_has_them() {
        let flags = ffi::GC_REACH_F_ATTACK | ffi::GC_REACH_F_STOP | ffi::GC_REACH_F_UNREACHABLE;
        let b = reply(
            9,
            PATH_NODE_FLAGS_RECORD_SIZE,
            &[
                flagged_node(4, 5, 2, 0, flags),
                flagged_node(1, 1, 0, 60, 0),
            ],
            b"",
        );
        let p = parse_path(&b).unwrap();
        assert_eq!(p[0].flags, ReachFlags(flags));
        assert!(p[0].flags.attack() && p[0].flags.stop() && p[0].flags.unreachable());
        assert!(!p[0].flags.embark() && !p[0].flags.city() && !p[0].flags.capture());
        assert_eq!(p[1].flags, ReachFlags(0));
        assert_eq!((p[0].x, p[0].y, p[0].turn), (4, 5, 2));
        // Q_REACHABLE has the same record.
        let r = reply(
            10,
            PATH_NODE_FLAGS_RECORD_SIZE,
            &[flagged_node(2, 2, 0, 30, ffi::GC_REACH_F_CITY)],
            b"",
        );
        assert!(parse_reachable(&r).unwrap()[0].flags.city());
        // A v1 record (8 bytes) reads as no flags, a longer one is skipped past.
        let v1 = reply(9, PATH_NODE_RECORD_SIZE, &[node_record(1, 2, 0, 60)], b"");
        assert_eq!(parse_path(&v1).unwrap()[0].flags, ReachFlags(0));
        let grown = reply(
            9,
            PATH_NODE_FLAGS_RECORD_SIZE + 4,
            &[[flagged_node(1, 2, 0, 60, 0x0002), vec![9; 4]].concat()],
            b"",
        );
        assert!(parse_path(&grown).unwrap()[0].flags.embark());
        assert_prefixes_fail(&b, parse_path);
    }

    fn diplo_record(other: i8, team: i8, flags: u16, locked: i16) -> Vec<u8> {
        let mut r = vec![other as u8, team as u8];
        r.extend_from_slice(&flags.to_le_bytes());
        push_i16(&mut r, locked);
        push_i16(&mut r, 0);
        assert_eq!(r.len(), DIPLO_RECORD_SIZE);
        r
    }

    #[test]
    fn diplo_decodes() {
        let war = ffi::GC_DIPLO_F_MET
            | ffi::GC_DIPLO_F_AT_WAR
            | ffi::GC_DIPLO_F_HUMAN
            | ffi::GC_DIPLO_F_CAN_MAKE_PEACE;
        let b = reply(
            15,
            DIPLO_RECORD_SIZE,
            &[
                diplo_record(1, 1, war, 0),
                diplo_record(
                    3,
                    3,
                    ffi::GC_DIPLO_F_MET | ffi::GC_DIPLO_F_CAN_DECLARE_WAR,
                    12,
                ),
            ],
            b"",
        );
        let d = parse_diplo(&b).unwrap();
        assert_eq!(d.len(), 2);
        assert_eq!((d[0].other, d[0].team, d[0].war_locked_turns), (1, 1, 0));
        let f = d[0].flags;
        assert!(f.met() && f.at_war() && f.human() && f.can_make_peace() && !f.can_declare_war());
        assert_eq!((d[1].other, d[1].war_locked_turns), (3, 12));
        assert!(d[1].flags.can_declare_war() && !d[1].flags.at_war() && !d[1].flags.human());
        assert!(
            parse_diplo(&reply(15, DIPLO_RECORD_SIZE, &[], b""))
                .unwrap()
                .is_empty()
        );
        assert_prefixes_fail(&b, parse_diplo);
        let small = reply(15, 6, &[vec![0; 6]], b"");
        assert_eq!(
            parse_diplo(&small),
            Err(QueryError::RecordTooSmall { need: 8, got: 6 })
        );
        assert!(matches!(
            parse_diplo(&reply(14, DIPLO_RECORD_SIZE, &[], b"")),
            Err(QueryError::WrongKind {
                expected: 15,
                got: 14
            })
        ));
        let grown = reply(
            15,
            12,
            &[[diplo_record(2, 2, 1, 0), vec![1; 4]].concat()],
            b"",
        );
        assert_eq!(parse_diplo(&grown).unwrap()[0].other, 2);
    }

    fn tree_record(x: i16, y: i16, px: i16, py: i16, moves: i16, turn: u8, flags: u16) -> Vec<u8> {
        let mut r = Vec::new();
        for v in [x, y, px, py, moves] {
            push_i16(&mut r, v);
        }
        r.extend_from_slice(&[turn, 0]);
        r.extend_from_slice(&flags.to_le_bytes());
        push_i16(&mut r, 0);
        assert_eq!(r.len(), TREE_NODE_RECORD_SIZE);
        r
    }

    #[test]
    fn reachable_tree_decodes() {
        let b = reply(
            16,
            TREE_NODE_RECORD_SIZE,
            &[
                tree_record(5, 5, 4, 5, 30, 0, 0),
                tree_record(
                    6,
                    5,
                    5,
                    5,
                    0,
                    1,
                    ffi::GC_REACH_F_STOP | ffi::GC_REACH_F_UNREVEALED,
                ),
            ],
            b"",
        );
        let t = parse_reachable_tree(&b).unwrap();
        assert_eq!(
            t[0],
            TreeNode {
                x: 5,
                y: 5,
                parent_x: 4,
                parent_y: 5,
                moves_left: 30,
                turn: 0,
                flags: ReachFlags(0)
            }
        );
        assert_eq!((t[1].turn, t[1].parent_x, t[1].moves_left), (1, 5, 0));
        assert!(t[1].flags.stop() && t[1].flags.unrevealed() && !t[1].flags.unreachable());
        assert!(
            parse_reachable_tree(&reply(16, TREE_NODE_RECORD_SIZE, &[], b""))
                .unwrap()
                .is_empty()
        );
        assert_prefixes_fail(&b, parse_reachable_tree);
        let small = reply(16, 12, &[vec![0; 12]], b"");
        assert_eq!(
            parse_reachable_tree(&small),
            Err(QueryError::RecordTooSmall { need: 16, got: 12 })
        );
        let grown = reply(
            16,
            20,
            &[[tree_record(1, 2, 3, 4, 5, 2, 0), vec![0; 4]].concat()],
            b"",
        );
        assert_eq!(parse_reachable_tree(&grown).unwrap()[0].turn, 2);
    }

    struct BasisRec {
        index: u32,
        plot_type: i8,
        art: u8,
        terrain: i16,
        feature: i16,
        flags: u16,
        rivers: u8,
        flow: [i8; 3],
    }

    fn basis_record(b: BasisRec) -> Vec<u8> {
        let mut r = Vec::new();
        push_u32(&mut r, b.index);
        r.extend_from_slice(&[b.plot_type as u8, b.art]);
        push_i16(&mut r, b.terrain);
        push_i16(&mut r, b.feature);
        r.extend_from_slice(&b.flags.to_le_bytes());
        r.push(b.rivers);
        r.extend_from_slice(&b.flow.map(|v| v as u8));
        assert_eq!(r.len(), PLOT_BASIS_RECORD_SIZE);
        r
    }

    fn basis(index: u32, plot_type: i8, flags: u16, feature: i16) -> BasisRec {
        BasisRec {
            index,
            plot_type,
            art: 3,
            terrain: 5,
            feature,
            flags,
            rivers: 0,
            flow: [-1; 3],
        }
    }

    #[test]
    fn plot_basis_decodes() {
        let b = reply(
            19,
            PLOT_BASIS_RECORD_SIZE,
            &[
                basis_record(BasisRec {
                    rivers: 0b100010,
                    flow: [-1, 2, -1],
                    ..basis(41, 2, ffi::GC_BASIS_F_COASTAL | ffi::GC_BASIS_F_LAKE, -1)
                }),
                basis_record(basis(42, 0, ffi::GC_BASIS_F_NATURAL_WONDER, 4)),
            ],
            b"",
        );
        let p = parse_plot_basis(&b).unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(
            (p[0].index, p[0].plot_type, p[0].continent_art),
            (41, PlotType::Land, 3)
        );
        assert_eq!((p[0].terrain, p[0].feature), (5, None));
        assert!(p[0].flags.coastal() && p[0].flags.lake() && !p[0].flags.natural_wonder());
        assert!(p[0].river(Direction::East) && p[0].river(Direction::NorthWest));
        assert!(!p[0].river(Direction::NorthEast));
        assert_eq!(p[0].river_edges(), [false, true, false, false, false, true]);
        assert_eq!(p[0].river_flow, [-1, 2, -1]);
        assert_eq!(
            (p[1].plot_type, p[1].feature),
            (PlotType::Mountain, Some(4))
        );
        assert!(p[1].flags.natural_wonder());
        assert_eq!(p[1].river_edges(), [false; 6]);
        assert!(
            parse_plot_basis(&reply(19, PLOT_BASIS_RECORD_SIZE, &[], b""))
                .unwrap()
                .is_empty()
        );
        assert_prefixes_fail(&b, parse_plot_basis);
        let small = reply(19, 12, &[vec![0; 12]], b"");
        assert_eq!(
            parse_plot_basis(&small),
            Err(QueryError::RecordTooSmall { need: 16, got: 12 })
        );
        let grown = reply(
            19,
            24,
            &[[basis_record(basis(9, 3, 0, -1)), vec![0; 8]].concat()],
            b"",
        );
        assert_eq!(parse_plot_basis(&grown).unwrap()[0].index, 9);
    }

    fn combat_record(flags: u16) -> Vec<u8> {
        let mut r = Vec::new();
        push_i16(&mut r, 31);
        push_i16(&mut r, 12);
        push_i32(&mut r, 1450);
        push_i32(&mut r, 900);
        push_i16(&mut r, 40);
        push_i16(&mut r, 100);
        r.extend_from_slice(&flags.to_le_bytes());
        push_i16(&mut r, 0);
        assert_eq!(r.len(), COMBAT_PREVIEW_RECORD_SIZE);
        r
    }

    #[test]
    fn combat_preview_decodes() {
        let b = reply(
            20,
            COMBAT_PREVIEW_RECORD_SIZE,
            &[combat_record(
                ffi::GC_COMBAT_F_CITY | ffi::GC_COMBAT_F_KILLS,
            )],
            b"",
        );
        let c = parse_combat_preview(&b).unwrap().unwrap();
        assert_eq!((c.damage_to_target, c.damage_to_attacker), (31, 12));
        assert_eq!((c.attacker_strength, c.defender_strength), (1450, 900));
        assert_eq!((c.target_hp, c.target_max_hp), (40, 100));
        assert!(c.flags.city() && c.flags.kills() && !c.flags.captures());
        // Zero records: the attacker cannot attack that plot.
        let none = reply(20, COMBAT_PREVIEW_RECORD_SIZE, &[], b"");
        assert_eq!(parse_combat_preview(&none), Ok(None));
        // More than one is not a preview.
        let two = reply(
            20,
            COMBAT_PREVIEW_RECORD_SIZE,
            &[combat_record(0), combat_record(0)],
            b"",
        );
        assert_eq!(
            parse_combat_preview(&two),
            Err(QueryError::CountMismatch { want: 1, got: 2 })
        );
        assert_prefixes_fail(&b, parse_combat_preview);
        let small = reply(20, 16, &[vec![0; 16]], b"");
        assert_eq!(
            parse_combat_preview(&small),
            Err(QueryError::RecordTooSmall { need: 20, got: 16 })
        );
        let grown = reply(
            20,
            24,
            &[[combat_record(ffi::GC_COMBAT_F_CAPTURES), vec![0; 4]].concat()],
            b"",
        );
        assert!(
            parse_combat_preview(&grown)
                .unwrap()
                .unwrap()
                .flags
                .captures()
        );
    }

    #[test]
    fn plots_dirty_decodes() {
        let recs: Vec<Vec<u8>> = [0u32, 7, 10_239, u32::MAX]
            .iter()
            .map(|v| v.to_le_bytes().to_vec())
            .collect();
        let b = reply(21, PLOT_DIRTY_RECORD_SIZE, &recs, b"");
        assert_eq!(parse_plots_dirty(&b).unwrap(), vec![0, 7, 10_239, u32::MAX]);
        assert!(
            parse_plots_dirty(&reply(21, PLOT_DIRTY_RECORD_SIZE, &[], b""))
                .unwrap()
                .is_empty()
        );
        assert_prefixes_fail(&b, parse_plots_dirty);
        assert!(matches!(
            parse_plots_dirty(&reply(2, PLOT_DIRTY_RECORD_SIZE, &[], b"")),
            Err(QueryError::WrongKind {
                expected: 21,
                got: 2
            })
        ));
        let small = reply(21, 2, &[vec![0; 2]], b"");
        assert_eq!(
            parse_plots_dirty(&small),
            Err(QueryError::RecordTooSmall { need: 4, got: 2 })
        );
        let grown = reply(21, 8, &[vec![5, 0, 0, 0, 9, 9, 9, 9]], b"");
        assert_eq!(parse_plots_dirty(&grown).unwrap(), vec![5]);
    }

    // ---- UI v2 read-only queries and appended fields ----

    #[test]
    fn appended_fields_are_read_only_when_the_record_has_them() {
        // Q_UNIT_ACTIONS: cost (12-byte record); -1 = none.
        let rows = reply(
            8,
            UNIT_ACTION_COST_RECORD_SIZE,
            &[
                [
                    action_record(4, 1, ffi::GCQ_NO_STRING),
                    (-1i32).to_le_bytes().to_vec(),
                ]
                .concat(),
                [
                    action_record(ffi::GC_UNIT_ROW_UPGRADE, 0, 0),
                    145i32.to_le_bytes().to_vec(),
                ]
                .concat(),
            ],
            b"no gold\0",
        );
        let a = parse_unit_actions(&rows).unwrap();
        assert_eq!((a[0].action, a[0].cost, a[0].row()), (4, None, None));
        assert_eq!(
            (a[1].row(), a[1].enabled, a[1].cost, a[1].reason.as_deref()),
            (
                Some(crate::UnitRow::Upgrade),
                false,
                Some(145),
                Some("no gold")
            )
        );
        // The v1 record has no cost.
        let old = reply(
            8,
            UNIT_ACTION_RECORD_SIZE,
            &[action_record(4, 1, ffi::GCQ_NO_STRING)],
            b"",
        );
        assert_eq!(parse_unit_actions(&old).unwrap()[0].cost, None);

        // Q_CITIES: overflow (60-byte record); -1 = not the viewer's city.
        let city = |founder: u8, overflow: i32| {
            [
                city_record(0, ffi::GC_CITY_F_VISIBLE, 3, 0),
                vec![founder, 0, 0, 0],
                overflow.to_le_bytes().to_vec(),
            ]
            .concat()
        };
        let b = reply(
            4,
            CITY_OVERFLOW_RECORD_SIZE,
            &[city(0, 17), city(1, 0), city(2, -1)],
            b"Rome\0",
        );
        let c = parse_cities(&b).unwrap();
        assert_eq!(
            c.iter().map(|c| c.overflow).collect::<Vec<_>>(),
            vec![Some(17), Some(0), None]
        );
        assert_eq!(c[1].founder, Some(1));
        let v2 = reply(
            4,
            CITY_V2_RECORD_SIZE,
            &[city(0, 17)[..56].to_vec()],
            b"Rome\0",
        );
        assert_eq!(parse_cities(&v2).unwrap()[0].overflow, None);

        // Q_TOP_BAR: tourism (64-byte record).
        let b = reply(
            6,
            TOP_BAR_TOURISM_RECORD_SIZE,
            &[[top_bar_record(), 23i32.to_le_bytes().to_vec()].concat()],
            b"",
        );
        assert_eq!(parse_top_bar(&b).unwrap().tourism, Some(23));
        let old = reply(6, TOP_BAR_RECORD_SIZE, &[top_bar_record()], b"");
        assert_eq!(parse_top_bar(&old).unwrap().tourism, None);
    }

    fn city_order_record(slot: i32, order: i8, item: i16, turns: i16, needed: i32) -> Vec<u8> {
        let mut r = Vec::new();
        push_i32(&mut r, slot);
        r.extend_from_slice(&[order as u8, 0]);
        push_i16(&mut r, item);
        push_i16(&mut r, turns);
        r.extend_from_slice(&[0, 0]);
        push_i32(&mut r, needed);
        assert_eq!(r.len(), CITY_ORDER_RECORD_SIZE);
        r
    }

    #[test]
    fn city_queue_decodes() {
        let b = reply(
            23,
            CITY_ORDER_RECORD_SIZE,
            &[
                city_order_record(0, 1, 40, 6, 312), // a wonder being built
                city_order_record(1, 1, 12, 9, 160),
                city_order_record(2, 0, 21, -1, 100), // no production: never
                city_order_record(3, 4, 2, -1, 0),    // a process
                city_order_record(4, 3, 1, 5, 40),    // a specialist order of the core
            ],
            b"",
        );
        let q = parse_city_queue(&b).unwrap();
        assert_eq!(q.len(), 5);
        assert_eq!(
            q[0],
            CityOrder {
                slot: 0,
                order: OrderType::Construct,
                item: 40,
                turns: Some(6),
                needed: 312
            }
        );
        assert_eq!((q[2].order, q[2].turns), (OrderType::Train, None));
        assert_eq!((q[3].order, q[3].needed), (OrderType::Maintain, 0));
        assert_eq!(q[4].order, OrderType::Other(3));
        assert!(
            parse_city_queue(&reply(23, CITY_ORDER_RECORD_SIZE, &[], b""))
                .unwrap()
                .is_empty()
        );
        assert_prefixes_fail(&b, parse_city_queue);
        assert_eq!(
            parse_city_queue(&reply(24, CITY_ORDER_RECORD_SIZE, &[], b"")),
            Err(QueryError::WrongKind {
                expected: 23,
                got: 24
            })
        );
        assert_eq!(
            parse_city_queue(&reply(23, 12, &[vec![0; 12]], b"")),
            Err(QueryError::RecordTooSmall { need: 16, got: 12 })
        );
        let grown = reply(
            23,
            CITY_ORDER_RECORD_SIZE + 4,
            &[[city_order_record(0, 0, 7, 2, 40), vec![9; 4]].concat()],
            b"",
        );
        assert_eq!(parse_city_queue(&grown).unwrap()[0].item, 7);
    }

    fn tech_record(
        tech: i16,
        status: u8,
        queue: i8,
        turns: i16,
        progress: i32,
        cost: i32,
    ) -> Vec<u8> {
        let mut r = Vec::new();
        push_i16(&mut r, tech);
        r.extend_from_slice(&[status, queue as u8]);
        push_i16(&mut r, turns);
        r.extend_from_slice(&[0, 0]);
        push_i32(&mut r, progress);
        push_i32(&mut r, cost);
        assert_eq!(r.len(), TECH_RECORD_SIZE);
        r
    }

    #[test]
    fn techs_decode() {
        let b = reply(
            24,
            TECH_RECORD_SIZE,
            &[
                tech_record(0, ffi::GC_TECH_HAVE, -1, -1, 35, 35),
                tech_record(7, ffi::GC_TECH_CAN_RESEARCH, 0, 4, 120, 412),
                tech_record(8, ffi::GC_TECH_LOCKED, 1, 9, 0, 650),
                tech_record(9, 7, -1, -1, 0, 900),
            ],
            b"",
        );
        let t = parse_techs(&b).unwrap();
        assert_eq!(
            t[0],
            TechState {
                tech: 0,
                status: TechStatus::Have,
                queue: None,
                turns: None,
                progress: 35,
                cost: 35
            }
        );
        assert_eq!(
            (
                t[1].status,
                t[1].queue,
                t[1].turns,
                t[1].progress,
                t[1].cost
            ),
            (TechStatus::CanResearch, Some(0), Some(4), 120, 412)
        );
        assert_eq!((t[2].status, t[2].queue), (TechStatus::Locked, Some(1)));
        assert_eq!(t[3].status, TechStatus::Other(7));
        assert_prefixes_fail(&b, parse_techs);
        assert_eq!(
            parse_techs(&reply(24, 8, &[vec![0; 8]], b"")),
            Err(QueryError::RecordTooSmall { need: 16, got: 8 })
        );
    }

    fn resource_record(resource: i16, usage: u8, available: i32, used: i32, total: i32) -> Vec<u8> {
        let mut r = Vec::new();
        push_i16(&mut r, resource);
        r.extend_from_slice(&[usage, 0]);
        for v in [available, used, total] {
            push_i32(&mut r, v);
        }
        assert_eq!(r.len(), RESOURCE_RECORD_SIZE);
        r
    }

    #[test]
    fn player_resources_decode() {
        let b = reply(
            25,
            RESOURCE_RECORD_SIZE,
            &[
                resource_record(3, ffi::GC_RESOURCE_USAGE_STRATEGIC, 2, 4, 6),
                resource_record(15, ffi::GC_RESOURCE_USAGE_LUXURY, 1, 0, 1),
                resource_record(20, ffi::GC_RESOURCE_USAGE_BONUS, 3, 0, 3),
                resource_record(4, ffi::GC_RESOURCE_USAGE_STRATEGIC, -2, 2, 0), // used more than it has
                resource_record(30, 9, 0, 0, 1),
            ],
            b"",
        );
        let r = parse_player_resources(&b).unwrap();
        assert_eq!(
            r[0],
            PlayerResource {
                resource: 3,
                usage: ResourceUsage::Strategic,
                available: 2,
                used: 4,
                total: 6
            }
        );
        assert_eq!(r[1].usage, ResourceUsage::Luxury);
        assert_eq!(r[2].usage, ResourceUsage::Bonus);
        assert_eq!((r[3].available, r[3].used, r[3].total), (-2, 2, 0));
        assert_eq!(r[4].usage, ResourceUsage::Other(9));
        assert_prefixes_fail(&b, parse_player_resources);
        assert_eq!(
            parse_player_resources(&reply(25, 8, &[vec![0; 8]], b"")),
            Err(QueryError::RecordTooSmall { need: 16, got: 8 })
        );
    }

    #[test]
    fn policy_branches_decode() {
        let rec = |branch: i16, unlocked: u8, owned: u8, total: u8| {
            let mut r = Vec::new();
            push_i16(&mut r, branch);
            r.extend_from_slice(&[unlocked, owned, total, 0, 0, 0]);
            assert_eq!(r.len(), POLICY_BRANCH_RECORD_SIZE);
            r
        };
        let b = reply(
            26,
            POLICY_BRANCH_RECORD_SIZE,
            &[rec(0, 1, 3, 5), rec(1, 0, 0, 5), rec(9, 1, 255, 255)],
            b"",
        );
        let p = parse_policy_branches(&b).unwrap();
        assert_eq!(
            p[0],
            PolicyBranchState {
                branch: 0,
                unlocked: true,
                owned: 3,
                total: 5
            }
        );
        assert!(!p[1].unlocked);
        assert_eq!((p[2].branch, p[2].owned, p[2].total), (9, 255, 255));
        assert_prefixes_fail(&b, parse_policy_branches);
        assert_eq!(
            parse_policy_branches(&reply(26, 4, &[vec![0; 4]], b"")),
            Err(QueryError::RecordTooSmall { need: 8, got: 4 })
        );
    }

    #[test]
    fn unit_promotions_decode() {
        let rec = |p: i16| [p.to_le_bytes().to_vec(), vec![0, 0]].concat();
        let b = reply(27, PROMOTION_RECORD_SIZE, &[rec(2), rec(17), rec(40)], b"");
        assert_eq!(parse_unit_promotions(&b).unwrap(), vec![2, 17, 40]);
        assert!(
            parse_unit_promotions(&reply(27, PROMOTION_RECORD_SIZE, &[], b""))
                .unwrap()
                .is_empty()
        );
        assert_prefixes_fail(&b, parse_unit_promotions);
        assert_eq!(
            parse_unit_promotions(&reply(27, 2, &[vec![0; 2]], b"")),
            Err(QueryError::RecordTooSmall { need: 4, got: 2 })
        );
        let grown = reply(27, 8, &[[rec(5), vec![1; 4]].concat()], b"");
        assert_eq!(parse_unit_promotions(&grown).unwrap(), vec![5]);
    }
}
