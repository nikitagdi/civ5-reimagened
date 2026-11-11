//! `Game`: the safe API over `gamecore-sys` (ABI v2).

pub use gamecore_sys::PumpResult;
use gamecore_sys::{Command as RawCommand, GcError, ffi};

use crate::command::{Command, CommandError, RawCommand as RawCmd};
use crate::host::{HostAdapter, HostServices};
use crate::play::{
    Attacker, City, CityArt, CityBuild, CityOrder, Notification, PathNode, PlayerResource,
    PolicyBranchState, RangeShooter, RangeTarget, Reachable, TechState, TopBar, TradePath,
    TradeRoute, TreeNode, Unit, UnitAction,
};
use crate::query::{self, QueryError};
use crate::setup::Setup;
use crate::slots::{HumanSlot, SlotOp};
use crate::types::{
    CatalogEntry, CatalogTable, CivInfo, CombatForecast, DiploRelation, GameInfo, Player, Plot,
    PlotBasis, RejectReason, SetupDescription, TimerEntry, WorldSizeEntry,
};

/// gamecore error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GameError {
    #[error("a gamecore instance already exists in this process")]
    AlreadyExists,
    /// The core refused (a command, a save away from the save point, a slot operation).
    #[error("command rejected: {message}")]
    Rejected {
        reason: RejectReason,
        message: String,
    },
    #[error("malformed or incompatible save data: {0}")]
    BadData(String),
    #[error("game not started")]
    NotStarted,
    /// A replica's game left the server's (a journal check failed); it needs a snapshot.
    #[error("replica diverged from the server: {0}")]
    Diverged(String),
    /// The linked gamecore archive speaks another ABI version than this wrapper (a v1 archive
    /// with the v2 wrapper): its structs and calls differ, so nothing is called.
    #[error("gamecore ABI version mismatch: the archive is v{found}, this build needs v{expected}")]
    AbiVersion { found: u32, expected: u32 },
    #[error("malformed query reply: {0}")]
    Query(#[from] QueryError),
    #[error("gamecore error: {0}")]
    Core(GcError),
}

impl From<GcError> for GameError {
    fn from(e: GcError) -> Self {
        match e {
            GcError::AlreadyExists => GameError::AlreadyExists,
            GcError::Rejected { reason, message } => GameError::Rejected {
                reason: RejectReason::from_abi(reason),
                message,
            },
            GcError::BadData(why) => GameError::BadData(why),
            GcError::NotStarted => GameError::NotStarted,
            GcError::Diverged(why) => GameError::Diverged(why),
            other => GameError::Core(other),
        }
    }
}

/// Refuses an archive of another ABI version before any `gc_*` call that depends on the layout.
fn check_abi_version(found: u32) -> Result<(), GameError> {
    if found == ffi::GC_ABI_VERSION {
        Ok(())
    } else {
        Err(GameError::AbiVersion {
            found,
            expected: ffi::GC_ABI_VERSION,
        })
    }
}

/// Query arguments: the fixed ones, then plot indexes (`u32` in the API, `i32` on the ABI).
fn index_args(head: &[i32], indices: &[u32]) -> Result<Vec<i32>, GameError> {
    let mut args = Vec::with_capacity(head.len() + indices.len());
    args.extend_from_slice(head);
    for &i in indices {
        args.push(i32::try_from(i).map_err(|_| {
            GameError::Core(GcError::InvalidArg(format!(
                "plot index {i} does not fit the query"
            )))
        })?);
    }
    Ok(args)
}

/// The game. `Send`, not `Sync`; one per process.
pub struct Game {
    game: gamecore_sys::Game,
}

impl Game {
    pub(crate) fn sys(&mut self) -> &mut gamecore_sys::Game {
        &mut self.game
    }

    /// Creates the instance and boots the core (game database and localization through
    /// `HostServices::read_file`). `AlreadyExists` if the process already has one.
    pub fn new(host: impl HostServices + 'static) -> Result<Self, GameError> {
        check_abi_version(gamecore_sys::abi_version())?;
        let game = gamecore_sys::Game::create(Box::new(HostAdapter(host)));
        crate::flush_cpp_output();
        Ok(Game { game: game? })
    }

    /// Starts a game. Takes seconds on the real core.
    pub fn new_game(&mut self, setup: &Setup) -> Result<(), GameError> {
        self.new_game_json(&setup.to_json())
    }

    /// Starts a game from raw setup JSON; `new_game` is the typed way.
    pub fn new_game_json(&mut self, setup_json: &str) -> Result<(), GameError> {
        let r = self.game.new_game(setup_json);
        crate::flush_cpp_output();
        Ok(r?)
    }

    /// Loads `save` bytes (from [`Game::save`]) instead of `new_game`, on a new instance; `pump`
    /// then goes on from where the save was made.
    ///
    /// # Errors
    /// * `BadData(reason)`: not `GCS1`, another format version, truncated or corrupt, or a save of
    ///   another core build. Found before anything is loaded: the instance can still take a game.
    /// * `AlreadyExists`: the instance already has a game.
    /// * anything else (`Core(Internal)`): the load failed half way; the instance is used up.
    pub fn load(&mut self, save: &[u8]) -> Result<(), GameError> {
        let r = self.game.load_game(save);
        crate::flush_cpp_output();
        Ok(r?)
    }

    /// Runs at most `max_slices` slices of the turn loop.
    pub fn pump(&mut self, max_slices: u32) -> Result<PumpResult, GameError> {
        let r = self.game.pump(max_slices);
        crate::flush_cpp_output();
        Ok(r?)
    }

    /// Queues a command of the human `player` (a slot number); the next `pump` delivers it.
    /// `CommandError::Rejected` carries gamecore's `RejectReason` and text, and nothing is queued.
    pub fn command(&mut self, player: u8, cmd: &Command) -> Result<(), CommandError> {
        let (kind, args) = cmd.encode();
        self.command_raw(&RawCmd {
            kind,
            player,
            args,
            text: None,
        })
        .map_err(CommandError::from)
    }

    /// Queues a command in the raw `gc_command` shape.
    pub fn command_raw(&mut self, cmd: &RawCmd) -> Result<(), GameError> {
        let r = self.game.apply_command(&RawCommand {
            kind: cmd.kind,
            player: cmd.player,
            args: &cmd.args,
            text: cmd.text.as_deref(),
        });
        crate::flush_cpp_output();
        Ok(r?)
    }

    /// `Q_GAME`.
    pub fn game_info(&mut self) -> Result<GameInfo, GameError> {
        let buf = self.raw_query(ffi::GC_Q_GAME, &[])?;
        Ok(query::parse_game(&buf)?)
    }

    /// `Q_GAME {player}`: the game as seen by `player`; the blocker fields are that player's.
    pub fn game_info_for(&mut self, player: u8) -> Result<GameInfo, GameError> {
        let buf = self.raw_query(ffi::GC_Q_GAME, &[i32::from(player)])?;
        Ok(query::parse_game(&buf)?)
    }

    /// `Q_TURN`: the turn status of every slot that is or was a human, by player id.
    pub fn turn_state(&mut self) -> Result<Vec<HumanSlot>, GameError> {
        let buf = self.raw_query(ffi::GC_Q_TURN, &[])?;
        Ok(query::parse_turn(&buf)?)
    }

    /// Changes the connection of a human slot, at a safe point (`GameInfo::safe_point`); `nick`
    /// is required by a `Connect` onto an AI slot. `CommandError::Rejected` carries the reason
    /// (`NotSafePoint`, `SlotState`).
    pub fn slot_control(
        &mut self,
        player: u8,
        op: SlotOp,
        nick: Option<&str>,
    ) -> Result<(), CommandError> {
        let r = self.game.slot_control(i32::from(player), op.abi(), nick);
        crate::flush_cpp_output();
        r.map_err(|e| CommandError::from(GameError::from(e)))
    }

    /// `Q_PLOTS` for `viewer_team` (-1 = the whole truth): `map_width * map_height` plots, rows by y,
    /// then x.
    pub fn plots(&mut self, viewer_team: i32) -> Result<Vec<Plot>, GameError> {
        let buf = self.raw_query(ffi::GC_Q_PLOTS, &[viewer_team])?;
        Ok(query::parse_plots(&buf)?)
    }

    /// `Q_PLOTS` of only the plots with these indexes (`y * map_width + x`), for `viewer_team`, in
    /// the order given. A bad index is an argument error. An empty list gives an empty answer
    /// without asking the core (an empty list means the whole map to the ABI: use [`Game::plots`]).
    pub fn plots_at(&mut self, viewer_team: i32, indices: &[u32]) -> Result<Vec<Plot>, GameError> {
        if indices.is_empty() {
            return Ok(Vec::new());
        }
        let args = index_args(&[viewer_team], indices)?;
        let buf = self.raw_query(ffi::GC_Q_PLOTS, &args)?;
        Ok(query::parse_plots(&buf)?)
    }

    /// `Q_PLOT_BASIS`: the static terrain of the whole map (`None`) or of the plots with these
    /// indexes, revealed or not, natural wonders included, no fog, no team. By index. `Some(&[])`
    /// gives an empty answer without asking the core (an empty list means the whole map to the
    /// ABI: pass `None`).
    pub fn plot_basis(&mut self, subset: Option<&[u32]>) -> Result<Vec<PlotBasis>, GameError> {
        let args = match subset {
            None => Vec::new(),
            Some([]) => return Ok(Vec::new()),
            Some(list) => index_args(&[], list)?,
        };
        let buf = self.raw_query(ffi::GC_Q_PLOT_BASIS, &args)?;
        Ok(query::parse_plot_basis(&buf)?)
    }

    /// `Q_PLOTS_DIRTY`: indexes of the plots whose `Q_PLOTS` / `Q_PLOT_BASIS` data may have changed
    /// for a watched team since the last call (the call resets the set; the first call, and the
    /// first after a load, lists every plot). `None` when the linked core does not have the query
    /// (the stub): take every plot. Changes only the ABI layer's snapshot, never the game.
    pub fn plots_dirty(&mut self) -> Result<Option<Vec<u32>>, GameError> {
        match self.raw_query(ffi::GC_Q_PLOTS_DIRTY, &[]) {
            Ok(buf) => Ok(Some(query::parse_plots_dirty(&buf)?)),
            Err(GameError::Core(GcError::NotImplemented)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// `Q_REACHABLE_TREE`: the plots the unit reaches within `horizon` (1 to 3) turns, each with
    /// the plot it comes from. The unit's own plot (the root) is not listed. Same pathfinder as
    /// `UnitMoveTo`.
    pub fn reachable_tree(
        &mut self,
        player: u8,
        unit: i32,
        horizon: u8,
    ) -> Result<Vec<TreeNode>, GameError> {
        if !(1..=3).contains(&horizon) {
            return Err(GameError::Core(GcError::InvalidArg(format!(
                "Q_REACHABLE_TREE: horizon {horizon} is not 1..=3"
            ))));
        }
        let buf = self.raw_query(
            ffi::GC_Q_REACHABLE_TREE,
            &[i32::from(player), unit, i32::from(horizon)],
        )?;
        Ok(query::parse_reachable_tree(&buf)?)
    }

    /// `Q_COMBAT_PREVIEW`: what `attacker` (a unit or a city of `player`) would do to the plot
    /// `(x, y)`, without doing it. `None` when it cannot attack that plot. Only for a plot the
    /// player's team sees now.
    pub fn combat_preview(
        &mut self,
        player: u8,
        attacker: Attacker,
        x: i16,
        y: i16,
    ) -> Result<Option<CombatForecast>, GameError> {
        let who = attacker.abi().ok_or_else(|| {
            GameError::Core(GcError::InvalidArg(format!(
                "Q_COMBAT_PREVIEW: {attacker:?} is not a valid attacker id"
            )))
        })?;
        let buf = self.raw_query(
            ffi::GC_Q_COMBAT_PREVIEW,
            &[i32::from(player), who, i32::from(x), i32::from(y)],
        )?;
        Ok(query::parse_combat_preview(&buf)?)
    }

    /// `Q_DIPLO`: the relation of `player` to every other living major civ, by id: met, at war,
    /// human, whether war or peace can be made, and the turns peace stays locked.
    pub fn diplo(&mut self, player: u8) -> Result<Vec<DiploRelation>, GameError> {
        let buf = self.raw_query(ffi::GC_Q_DIPLO, &[i32::from(player)])?;
        Ok(query::parse_diplo(&buf)?)
    }

    /// `Q_PLAYERS` for `viewer_team` (-1 = all met).
    pub fn players(&mut self, viewer_team: i32) -> Result<Vec<Player>, GameError> {
        let buf = self.raw_query(ffi::GC_Q_PLAYERS, &[viewer_team])?;
        Ok(query::parse_players(&buf)?)
    }

    /// `Q_CATALOG` of one table. Static data: the caller may keep it for the whole game.
    pub fn catalog(&mut self, table: CatalogTable) -> Result<Vec<CatalogEntry>, GameError> {
        let buf = self.raw_query(ffi::GC_Q_CATALOG, &[table.abi()])?;
        Ok(query::parse_catalog(&buf)?)
    }

    /// `Q_CATALOG` of the TurnTimers table (v2): the turn timer presets of the game data.
    pub fn catalog_timers(&mut self) -> Result<Vec<TimerEntry>, GameError> {
        let buf = self.raw_query(ffi::GC_Q_CATALOG, &[ffi::GC_CATALOG_TURN_TIMERS])?;
        Ok(query::parse_catalog_timers(&buf)?)
    }

    /// `Q_CATALOG` of the Worlds table (v2): the map sizes of the game data.
    pub fn catalog_world_sizes(&mut self) -> Result<Vec<WorldSizeEntry>, GameError> {
        let buf = self.raw_query(ffi::GC_Q_CATALOG, &[ffi::GC_CATALOG_WORLD_SIZES])?;
        Ok(query::parse_catalog_world_sizes(&buf)?)
    }

    /// `Q_CATALOG` of one v3 table (`GC_CATALOG_GAME_OPTIONS` ... `GC_CATALOG_MAP_OPTION_VALUES`).
    fn catalog_ext(&mut self, table: i32) -> Result<Vec<query::ExtRow>, GameError> {
        let buf = self.raw_query(ffi::GC_Q_CATALOG, &[table])?;
        Ok(query::parse_catalog_ext(&buf)?)
    }

    /// The hover tips of the civilizations (v4, `Q_CATALOG` table 24), by civilization id. Valid before a game.
    pub fn civ_infos(&mut self) -> Result<Vec<CivInfo>, GameError> {
        let lines = |s: &str| -> Vec<String> {
            s.split('\n')
                .map(|l| l.replace("[NEWLINE]", " ").trim().to_owned())
                .filter(|l| !l.is_empty())
                .collect()
        };
        Ok(self
            .catalog_ext(ffi::GC_CATALOG_CIV_INFO)?
            .into_iter()
            .map(|(_, [type_name, leader, traits, unique])| CivInfo {
                type_name,
                leader,
                traits: lines(&traits),
                unique: lines(&unique),
            })
            .collect())
    }

    /// The staging room options of the game data (v3, `Q_CATALOG` tables 19 to 23): every `GameOptions` row, the
    /// `Victories`, the `MinorCivilizations` and the custom options of the map scripts with their values, localized
    /// (en_US). Valid on a loaded session, before a game.
    pub fn describe_setup(&mut self) -> Result<SetupDescription, GameError> {
        Ok(SetupDescription {
            game_options: query::game_options_of(self.catalog_ext(ffi::GC_CATALOG_GAME_OPTIONS)?),
            victories: query::named_of(self.catalog_ext(ffi::GC_CATALOG_VICTORIES)?),
            minor_civs: query::named_of(self.catalog_ext(ffi::GC_CATALOG_MINOR_CIVS)?),
            map_options: query::map_options_of(self.catalog_ext(ffi::GC_CATALOG_MAP_OPTIONS)?),
            map_option_values: query::map_option_values_of(
                self.catalog_ext(ffi::GC_CATALOG_MAP_OPTION_VALUES)?,
            ),
        })
    }

    /// The build id of the core, as in the header of its saves. Valid before a game.
    pub fn build_id(&mut self) -> Result<String, GameError> {
        Ok(self.game.build_id()?)
    }

    /// `Q_UNITS` for `viewer_team` (-1 = every unit): own units and the visible foreign ones, by owner
    /// then id.
    pub fn units(&mut self, viewer_team: i32) -> Result<Vec<Unit>, GameError> {
        let buf = self.raw_query(ffi::GC_Q_UNITS, &[viewer_team])?;
        Ok(query::parse_units(&buf)?)
    }

    /// `Q_UNIT_ACTIONS`: the unit commands that apply to the unit, and whether each is possible now.
    pub fn unit_actions(&mut self, player: u8, unit: i32) -> Result<Vec<UnitAction>, GameError> {
        let buf = self.raw_query(ffi::GC_Q_UNIT_ACTIONS, &[i32::from(player), unit])?;
        Ok(query::parse_unit_actions(&buf)?)
    }

    /// `Q_UNIT_ACTIONS` with the extra rows (UI v2): the unit commands, then the rows of
    /// [`UnitRow`](crate::UnitRow) that apply to the unit (alert, pillage, upgrade with its price). A core
    /// without the rows refuses the third argument (`GcError::InvalidArg`).
    pub fn unit_actions_with_rows(
        &mut self,
        player: u8,
        unit: i32,
    ) -> Result<Vec<UnitAction>, GameError> {
        let buf = self.raw_query(
            ffi::GC_Q_UNIT_ACTIONS,
            &[i32::from(player), unit, ffi::GC_UNIT_ACTIONS_ROWS_EXTRA],
        )?;
        Ok(query::parse_unit_actions(&buf)?)
    }

    /// `Q_UNIT_PROMOTIONS` (UI v2): the `PromotionTypes` the unit of `player` has, by id.
    pub fn unit_promotions(&mut self, player: u8, unit: i32) -> Result<Vec<i16>, GameError> {
        let buf = self.raw_query(ffi::GC_Q_UNIT_PROMOTIONS, &[i32::from(player), unit])?;
        Ok(query::parse_unit_promotions(&buf)?)
    }

    /// `Q_PATH`: the unit's path to `(x, y)` (the unit of `player`), first node = its plot. A target
    /// that cannot be reached is the one node of the target with `flags.unreachable()`; an empty path
    /// is a core without the v2 flags. A bad unit or an off-map target is an error.
    pub fn path(
        &mut self,
        player: u8,
        unit: i32,
        x: i32,
        y: i32,
    ) -> Result<Vec<PathNode>, GameError> {
        let buf = self.raw_query(ffi::GC_Q_PATH, &[i32::from(player), unit, x, y])?;
        Ok(query::parse_path(&buf)?)
    }

    /// `Q_REACHABLE`: the plots the unit reaches this turn (not its own), by y then x.
    pub fn reachable(&mut self, player: u8, unit: i32) -> Result<Vec<Reachable>, GameError> {
        let buf = self.raw_query(ffi::GC_Q_REACHABLE, &[i32::from(player), unit])?;
        Ok(query::parse_reachable(&buf)?)
    }

    /// `Q_RANGE_TARGETS`: the plots `from` (a unit or a city of `player`) can hit with a ranged
    /// attack right now, by y then x; empty when it cannot shoot now. A unit or city the player
    /// does not have, or an id the query cannot encode, is an argument error.
    pub fn range_targets(
        &mut self,
        player: u8,
        from: RangeShooter,
    ) -> Result<Vec<RangeTarget>, GameError> {
        let shooter = from.abi().ok_or_else(|| {
            GameError::Core(GcError::InvalidArg(format!(
                "Q_RANGE_TARGETS: {from:?} is not a valid shooter id"
            )))
        })?;
        let buf = self.raw_query(ffi::GC_Q_RANGE_TARGETS, &[i32::from(player), shooter])?;
        Ok(query::parse_range_targets(&buf)?)
    }

    /// `Q_NOTIFICATIONS`: the live notifications of `player` (any human of the game; an AI or a
    /// minor civ has none).
    pub fn notifications(&mut self, player: u8) -> Result<Vec<Notification>, GameError> {
        let buf = self.raw_query(ffi::GC_Q_NOTIFICATIONS, &[i32::from(player)])?;
        Ok(query::parse_notifications(&buf)?)
    }

    /// `Q_CITIES` for `viewer_team` (-1 = every city): own cities with live data, foreign ones when
    /// visible or last seen, by owner then id.
    pub fn cities(&mut self, viewer_team: i32) -> Result<Vec<City>, GameError> {
        let buf = self.raw_query(ffi::GC_Q_CITIES, &[viewer_team])?;
        Ok(query::parse_cities(&buf)?)
    }

    /// `Q_CITY_ART` for `viewer_team`: the cities to draw with their buildings (v2).
    pub fn city_art(&mut self, viewer_team: i32) -> Result<Vec<CityArt>, GameError> {
        let buf = self.raw_query(ffi::GC_Q_CITY_ART, &[viewer_team])?;
        Ok(query::parse_city_art(&buf)?)
    }

    /// `Q_TRADE_ROUTES` for `viewer_team` (v2): the trade routes the viewer's view shows.
    pub fn trade_routes(&mut self, viewer_team: i32) -> Result<Vec<TradeRoute>, GameError> {
        let buf = self.raw_query(ffi::GC_Q_TRADE_ROUTES, &[viewer_team])?;
        Ok(query::parse_trade_routes(&buf)?)
    }

    /// `Q_TRADE_PATHS` for `viewer_team` (B4 fix-path): the plots of the routes `trade_routes` lists, as the DLL's
    /// trade route path finder laid them (origin city first).
    pub fn trade_paths(&mut self, viewer_team: i32) -> Result<Vec<TradePath>, GameError> {
        let buf = self.raw_query(ffi::GC_Q_TRADE_PATHS, &[viewer_team])?;
        Ok(query::parse_trade_paths(&buf)?)
    }

    /// `Q_CITY_BUILDS`: what the city of `player` (any living major civ) could build.
    pub fn city_builds(&mut self, player: u8, city: i32) -> Result<Vec<CityBuild>, GameError> {
        let buf = self.raw_query(ffi::GC_Q_CITY_BUILDS, &[i32::from(player), city])?;
        Ok(query::parse_city_builds(&buf)?)
    }

    /// `Q_TOP_BAR` of `player` (any living major civ).
    pub fn top_bar(&mut self, player: u8) -> Result<TopBar, GameError> {
        let buf = self.raw_query(ffi::GC_Q_TOP_BAR, &[i32::from(player)])?;
        Ok(query::parse_top_bar(&buf)?)
    }

    /// `Q_CITY_QUEUE` (UI v2): the order queue of the city of `player` (any living major civ), head first.
    pub fn city_queue(&mut self, player: u8, city: i32) -> Result<Vec<CityOrder>, GameError> {
        let buf = self.raw_query(ffi::GC_Q_CITY_QUEUE, &[i32::from(player), city])?;
        Ok(query::parse_city_queue(&buf)?)
    }

    /// `Q_TECHS` (UI v2): every technology and its state for `player` (any living major civ), by id.
    pub fn techs(&mut self, player: u8) -> Result<Vec<TechState>, GameError> {
        let buf = self.raw_query(ffi::GC_Q_TECHS, &[i32::from(player)])?;
        Ok(query::parse_techs(&buf)?)
    }

    /// `Q_PLAYER_RESOURCES` (UI v2): the resources `player` (any living major civ) has or uses, by id.
    pub fn player_resources(&mut self, player: u8) -> Result<Vec<PlayerResource>, GameError> {
        let buf = self.raw_query(ffi::GC_Q_PLAYER_RESOURCES, &[i32::from(player)])?;
        Ok(query::parse_player_resources(&buf)?)
    }

    /// `Q_POLICY_BRANCHES` (UI v2): every policy branch and the progress of `player` (any living major civ) in it.
    pub fn policy_branches(&mut self, player: u8) -> Result<Vec<PolicyBranchState>, GameError> {
        let buf = self.raw_query(ffi::GC_Q_POLICY_BRANCHES, &[i32::from(player)])?;
        Ok(query::parse_policy_branches(&buf)?)
    }

    /// Raw `GCQ1` reply of query `kind` (`ffi::GC_Q_*`), for queries without a typed method yet.
    pub fn raw_query(&mut self, kind: i32, args: &[i32]) -> Result<Vec<u8>, GameError> {
        let r = self.game.query(kind, args);
        crate::flush_cpp_output();
        Ok(r?)
    }

    /// The game as `GCS1` bytes ([`SaveInfo`](crate::SaveInfo) reads their header).
    ///
    /// # Errors
    /// `Rejected` (with the core's reason) away from the save point (SIMP-026): the last `pump`
    /// must have ended at `WAITING_HUMAN` with no command since. `NotStarted` before a game.
    pub fn save(&mut self) -> Result<Vec<u8>, GameError> {
        let r = self.game.save();
        crate::flush_cpp_output();
        Ok(r?)
    }

    /// 64-bit digest of the state; `viewer_team` is -1 (whole game) or a team id.
    pub fn state_digest(&mut self, viewer_team: i32) -> Result<u64, GameError> {
        Ok(self.game.state_digest(viewer_team)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A refusal of the core arrives with its `gc_error.reason` typed, and survives the mapping to
    /// `CommandError` (the gateway reads the reason, not the text).
    #[test]
    fn refusal_reason_is_typed() {
        for (code, want) in [
            (ffi::GC_REASON_NOT_HUMAN, RejectReason::NotHuman),
            (ffi::GC_REASON_AI_RUNNING, RejectReason::AiRunning),
            (ffi::GC_REASON_NOT_SAFE_POINT, RejectReason::NotSafePoint),
            (0, RejectReason::Unknown(0)),
            (99, RejectReason::Unknown(99)),
        ] {
            let e = GameError::from(GcError::Rejected {
                reason: code,
                message: "why".into(),
            });
            assert_eq!(
                e,
                GameError::Rejected {
                    reason: want,
                    message: "why".into()
                }
            );
            assert!(matches!(
                CommandError::from(e),
                CommandError::Rejected { reason, .. } if reason == want
            ));
        }
        // Other core errors are not refusals.
        assert!(matches!(
            CommandError::from(GameError::from(GcError::NotImplemented)),
            CommandError::Other(GameError::Core(GcError::NotImplemented))
        ));
    }

    #[test]
    fn plot_indexes_become_query_args() {
        assert_eq!(index_args(&[3], &[0, 7, 10_239]), Ok(vec![3, 0, 7, 10_239]));
        assert_eq!(index_args(&[], &[5]), Ok(vec![5]));
        assert_eq!(index_args(&[-1], &[]), Ok(vec![-1]));
        let too_big = index_args(&[1], &[2, i32::MAX as u32 + 1]);
        assert!(
            matches!(too_big, Err(GameError::Core(GcError::InvalidArg(m))) if m.contains("2147483648"))
        );
        assert_eq!(index_args(&[], &[i32::MAX as u32]), Ok(vec![i32::MAX]));
    }

    #[test]
    fn abi_version_of_the_archive_is_checked() {
        assert_eq!(check_abi_version(ffi::GC_ABI_VERSION), Ok(()));
        // A v1 archive built before the v2 header.
        assert_eq!(
            check_abi_version(1),
            Err(GameError::AbiVersion {
                found: 1,
                expected: ffi::GC_ABI_VERSION
            })
        );
        let text = check_abi_version(1).unwrap_err().to_string();
        assert!(text.contains("v1") && text.contains("v2"), "{text}");
    }
}
