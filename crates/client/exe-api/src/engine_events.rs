//! Engine callbacks to `Events.*`: the translation table.
//!
//! The kinds and the layout of their arguments are the core's (`gamecore::UiEventKind`, `gamecore_abi.h`): a unit
//! is a block of [`UNIT_INTS`] (owner, id, x, y, type, culture, civ, colours, flag index, fog, selected, military,
//! visible), a city a block of [`CITY_INTS`] (owner, id, x, y, population, damage, max hit points, size, team, puppet,
//! capital); a kind's own arguments follow its block.
//!
//! SIMPLIFIED(SIMP-exe-3): the table "callback -> event(s) with arguments" and "dirty bit -> event" follow the
//! names and the signatures of the EUI Lua listeners (`handlers` of K1-K3), not the bodies of the exe (the J-EVENTS
//! batches of `re-ui` are not read). The order inside one callback is the order of the exe where the exe is known
//! (a unit is created before it is selected) and the order of the listeners' needs elsewhere. Every kind without a
//! listener in K1-K3 is counted (`exe.unmapped`) and dropped.
//!
//! What the exe makes of the interface state, and the core reports as state, is derived here: the core's
//! `UnitSelectionChanged` (count, head) becomes the exe's `UnitSelectionChanged(owner, id, hex, selected)` for the
//! old and the new head and `UnitSelectionCleared`; `CityScreenChanged` becomes `SerialEventEnterCityScreen` /
//! `SerialEventExitCityScreen`. The turn events (`NewGameTurn`, `ActivePlayerTurnStart`, ...) are the core's.
//! The fog the core reports is `FogOfWarModeTypes`; the Lua listeners get the exe's scale ([`fog_to_lua`]).
//! A move of a unit the UI was not told of yet is dropped: the DLL places a new unit before announcing it.

use crate::buddy::PopupInfo;
use crate::core::{Arg, Core, Family, Fire, NoteRec, Payload};
use crate::entities::{CityRec, Seen, UnitRec};
use crate::{UiEvent, UiEventKind as K};

/// Ints of the unit block (`GC_UI_UNIT_INTS`).
pub const UNIT_INTS: usize = 14;
/// Ints of the city block (`GC_UI_CITY_INTS`).
pub const CITY_INTS: usize = 11;

/// `FogOfWarModeTypes::FOGOFWARMODE_UNEXPLORED`: a plot the core never reported (its fog sync starts there).
const FOG_UNEXPLORED: i32 = 1;

/// The fog of the core (`FogOfWarModeTypes`: 0 off = seen, 1 unexplored, 2 no visibility) as the exe gives it to
/// Lua (`BlackFog` 0, `GreyFog` 1, `WhiteFog` 2: `fogState ~= 2` hides a flag).
pub fn fog_to_lua(mode: i32) -> i32 {
    match mode {
        0 => 2,
        2 => 1,
        _ => 0,
    }
}

/// `args[i]` of an event, 0 when the core packed fewer.
trait ArgAt {
    fn a(&self, i: usize) -> i32;
}

impl ArgAt for UiEvent {
    fn a(&self, i: usize) -> i32 {
        self.args.get(i).copied().unwrap_or(0)
    }
}

fn hex(x: i32, y: i32) -> Arg {
    let (hx, hy) = crate::hex::grid_to_hex(x, y);
    Arg::Vec(vec![f64::from(hx), f64::from(hy)])
}

fn b(v: i32) -> Arg {
    Arg::Bool(v != 0)
}

fn n(v: i32) -> Arg {
    Arg::from(v)
}

/// `InterfaceDirtyBits` (CvEnums.h) that have a listener, with the event each one raises.
/// Several bits may raise the same event; it is raised once per frame.
pub fn dirty_event(bit: i32) -> Option<&'static str> {
    Some(match bit {
        4..=6 => "SerialEventUnitInfoDirty", // Misc / PlotList / Selection buttons
        8 => "SerialEventResearchDirty",
        11 => "SerialEventGameDataDirty",
        12 => "SerialEventScoreDirty",
        13 => "SerialEventTurnTimerDirty",
        18 => "SerialEventCityInfoDirty",
        19 => "SerialEventUnitInfoDirty",
        21 => "SerialEventCityScreenDirty",
        22 => "SerialEventInfoPaneDirty",
        32 => "EventPoliciesDirty",
        34 => "SerialEventEspionageScreenDirty",
        35 => "SerialEventLeagueScreenDirty",
        36 => "SerialEventGreatWorksScreenDirty",
        _ => return None,
    })
}

impl Core {
    /// Translates what the game core announced. Events that are not `rebuild` go first, in order; the `rebuild`
    /// group (made by `setup_graphical`) is reconciled against the entity table. `rebuilt` is true when
    /// `setup_graphical` ran for this batch, so an entity the group lacks is gone.
    pub fn translate_batch(&self, events: Vec<UiEvent>, rebuilt: bool) {
        // Most events can change a plot's yield (a tech, a belief, a build, a city, the borders); a unit's step does not.
        if rebuilt
            || events.iter().any(|e| {
                !matches!(
                    e.kind,
                    K::UnitMoved
                        | K::UnitSetDamage
                        | K::UnitFortify
                        | K::UnitVisibility
                        | K::UnitCombat
                        | K::UnitMissionEnd
                        | K::UnitResetAnimation
                        | K::UnitShouldDimFlag
                        | K::UnitMarkThreatening
                )
            })
        {
            self.yields_changed();
        }
        let (rebuild, normal): (Vec<UiEvent>, Vec<UiEvent>) =
            events.into_iter().partition(|e| e.rebuild);
        if rebuilt {
            // A load announces every plot's state again: every resource icon is told again.
            self.forget_plot_resources();
        }
        for e in &normal {
            self.apply(e, None);
        }
        if rebuilt || !rebuild.is_empty() {
            self.reconcile(&rebuild, rebuilt);
        }
        self.tell_renamed();
    }

    /// SIMPLIFIED(SIMP-exe-953): `Events.UnitNameChanged(player, unit)` for a unit the seat renamed: told when the
    /// rename is sent and again once the next batch of the game has been applied (the name changes there); the exe
    /// raises it from `CvUnit::setName`, which the replica runs on the server's answer.
    pub fn tell_renamed(&self) {
        let ids = std::mem::take(&mut *self.renamed_units.borrow_mut());
        if ids.is_empty() {
            return;
        }
        let Some(ctx) = self.lua_ctx() else {
            return;
        };
        let me = ctx
            .lua()
            .load("return Game and Game.GetActivePlayer and Game.GetActivePlayer() or -1")
            .set_name("=exe_active")
            .eval::<i32>()
            .unwrap_or(-1);
        if me < 0 {
            return;
        }
        for id in ids {
            self.fire_events("UnitNameChanged", vec![n(me), n(id)]);
        }
    }

    fn reconcile(&self, group: &[UiEvent], rebuilt: bool) {
        // SIMPLIFIED(SIMP-exe-64): a game loaded again (a turn's snapshot) makes new unit and city objects in
        // the replica, and the Lua objects the flags and banners hold are dead: every unit and city the UI knows
        // is destroyed and told as created again.
        if rebuilt && self.init_fired.get() {
            let (units, cities): (Vec<_>, Vec<_>) = {
                let t = self.entities.borrow();
                (
                    t.units.keys().copied().collect(),
                    t.cities.iter().map(|(k, c)| (*k, c.clone())).collect(),
                )
            };
            for (o, id) in units {
                self.entities.borrow_mut().units.remove(&(o, id));
                self.fire_events("SerialEventUnitDestroyed", vec![n(o), n(id)]);
            }
            for ((o, id), c) in cities {
                self.entities.borrow_mut().cities.remove(&(o, id));
                self.fire_events(
                    "SerialEventCityDestroyed",
                    vec![hex(c.x, c.y), n(o), n(id), n(o)],
                );
            }
        }
        let mut seen = Seen::default();
        for e in group {
            self.apply(e, Some(&mut seen));
        }
        if rebuilt {
            let gone = self.entities.borrow().missing(&seen);
            for ((o, id), _) in gone.units {
                self.entities.borrow_mut().units.remove(&(o, id));
                self.fire_events("SerialEventUnitDestroyed", vec![n(o), n(id)]);
            }
            for ((o, id), c) in gone.cities {
                self.entities.borrow_mut().cities.remove(&(o, id));
                self.fire_events(
                    "SerialEventCityDestroyed",
                    vec![hex(c.x, c.y), n(o), n(id), n(o)],
                );
            }
            // In the FIFO, behind the announcements: the listeners that run before them (`GameplaySetActivePlayer` makes the ribbons
            // of EUI's unit panel ask for the lists) still meet the gated lists.
            self.enqueue(crate::core::Fire {
                family: crate::core::Family::Events,
                name: "ExeSetupDone".to_owned(),
                payload: crate::core::Payload::Args(Vec::new()),
                hook: Some(crate::core::Hook::SetupDone),
            });
            self.after_rebuild();
        }
    }

    /// After a load: `SequenceGameInitComplete` once, and the data dirty bits. The turn start of a new turn is
    /// the core's (`PUBLISH_NEW_GAME_TURN`, `ACTIVE_PLAYER_TURN_START`, ...), not made here.
    fn after_rebuild(&self) {
        if !self.init_fired.get() {
            self.init_fired.set(true);
            if self.cfg.fire_init_complete {
                self.fire_events("SequenceGameInitComplete", vec![]);
            }
        }
        self.fire_events("SerialEventGameDataDirty", vec![]);
    }

    /// Raises the events of the dirty bits set since the last call, one per event, in bit order.
    pub fn flush_dirty(&self) {
        let bits: Vec<i32> = std::mem::take(&mut *self.dirty.borrow_mut())
            .into_iter()
            .collect();
        let mut raised: Vec<&'static str> = Vec::new();
        for bit in bits {
            if let Some(name) = dirty_event(bit)
                && !raised.contains(&name)
            {
                raised.push(name);
            }
        }
        for name in raised {
            self.fire_events(name, vec![]);
        }
    }

    /// SIMPLIFIED(SIMP-exe-37): combat, walls, minimap pings, animation resets and the kinds of later screens
    /// (trade routes, wonders, spaceship, borders, deals, the tech tree) have no listener in K1-K3 (the combat
    /// simulation `RunCombatSim` / `EndCombatSim` and the minimap are K6 and later); they are counted.
    fn unmapped(&self, kind: K) {
        self.counters.unmapped.set(self.counters.unmapped.get() + 1);
        self.simp_call(&format!("engine.{kind:?}"));
    }

    /// The continent art type of a plot through the DLL's Lua API (`Plot:GetContinentArtType`); 0 without one.
    pub(crate) fn continent(&self, x: i32, y: i32) -> i32 {
        let Some(ctx) = self.lua_ctx() else {
            return 0;
        };
        ctx.lua()
            .load(&format!(
                "local p = Map and Map.GetPlot and Map.GetPlot({x}, {y}) \
                 return p and p.GetContinentArtType and p:GetContinentArtType()"
            ))
            .set_name("=exe_continent")
            .eval::<Option<f64>>()
            .ok()
            .flatten()
            .map_or(0, |v| v as i32)
    }

    /// The camera goes to the plot of the seat's head selected unit, or of its head selected city (first when the city
    /// screen is up): `InterfaceBuddy::lookAtSelectionPlot`. Only the seat's own selection is looked at: another
    /// player's city never moves the camera (live reports 2026-10-30, commit 54e0b6f).
    ///
    /// SIMPLIFIED(SIMP-exe-780): a plain look at (the exe uses CITY_ZOOM_IN with the city-distance zoom while the city
    /// screen is up); the zoom is the client's (SIMP-exe-72, SIMP-exe-750).
    fn look_at_selection(&self) {
        let Some(ctx) = self.lua_ctx() else {
            return;
        };
        let at = ctx
            .lua()
            .load(
                "local me = Game and Game.GetActivePlayer and Game.GetActivePlayer()
                 local u = UI.GetHeadSelectedUnit()
                 local c = UI.GetHeadSelectedCity()
                 local o = (UI.IsCityScreenUp() and c) or u or c
                 if not o or o:GetOwner() ~= me then return nil end
                 local p = (o == c) and c:Plot() or u:GetPlot()
                 return p and p:GetX(), p and p:GetY()",
            )
            .set_name("=exe_look_selection")
            .eval::<(Option<i32>, Option<i32>)>();
        match at {
            Ok((Some(x), Some(y))) => self.host.look_at(x, y),
            Ok(_) => {}
            Err(err) => {
                tracing::debug!(target: "exe.camera", "look at the selection failed: {err}")
            }
        }
    }

    /// `UnitSelectionChanged(owner, id, hexX, hexY, hexZ, selected)` at the plot the UI knows the unit on.
    fn fire_selection(&self, o: i32, id: i32, selected: bool) {
        let at = self
            .entities
            .borrow()
            .units
            .get(&(o, id))
            .map(|u| crate::hex::grid_to_hex(u.x, u.y));
        let (hx, hy) = at.unwrap_or((0, 0));
        self.fire_events(
            "UnitSelectionChanged",
            vec![n(o), n(id), n(hx), n(hy), n(-hx - hy), Arg::Bool(selected)],
        );
        self.fire_unit_hex_highlight(hx, hy, selected, id);
    }

    /// `UnitHexHighlight(i, j, k, on, unitId)`: the exe's `PublishUnitHexHighlight` is raised on select, on every move
    /// of the selected unit and on deselect; InGame.lua's `OnUnitHexHighlight` asks for the settler's yields from it.
    fn fire_unit_hex_highlight(&self, hx: i32, hy: i32, on: bool, id: i32) {
        self.fire_events(
            "UnitHexHighlight",
            vec![n(hx), n(hy), n(-hx - hy), Arg::Bool(on), n(id)],
        );
    }

    /// One engine event. `seen` is `Some` in the `rebuild` group.
    fn apply(&self, e: &UiEvent, mut seen: Option<&mut Seen>) {
        // The owner and id of the unit or city block.
        let (o, id) = (e.a(0), e.a(1));
        // The kind's own arguments after a unit / city block.
        let u = |i: usize| e.a(UNIT_INTS + i);
        let c = |i: usize| e.a(CITY_INTS + i);
        match e.kind {
            K::UnitCreated => {
                let rec = UnitRec {
                    x: e.a(2),
                    y: e.a(3),
                    damage: 0,
                    embarked: false,
                    garrisoned: false,
                    dim: false,
                    visible: e.a(13) != 0,
                    fog: e.a(10),
                };
                self.entities.borrow_mut().culture.insert(o, e.a(5));
                if let Some(s) = seen.as_deref_mut() {
                    s.units.insert((o, id));
                }
                let known = self.entities.borrow().units.get(&(o, id)).cloned();
                match (known, seen.is_some()) {
                    (Some(old), true) => self.unit_diff(o, id, &old, &rec),
                    _ => {
                        self.entities.borrow_mut().units.insert((o, id), rec);
                        // (the files of the unit type's selection sounds are fetched before the first click on it)
                        if !self.hinted_unit_types.borrow().contains(&e.a(4))
                            && self.is_seat_player(o)
                        {
                            self.hint_unit_sounds(e.a(4));
                        }
                        // SIMPLIFIED(SIMP-exe-681): the sound of a unit created follows `EventSoundConnections.xml`
                        // for the seat's own new units; the exe's own conditions are not traced.
                        if seen.is_none() && self.is_seat_player(o) && !self.sound_quiet() {
                            self.host.event_sound("UnitCreated");
                        }
                        self.fire_events(
                            "SerialEventUnitCreated",
                            vec![
                                n(o),
                                n(id),
                                hex(e.a(2), e.a(3)),
                                n(e.a(4)),
                                n(e.a(5)),
                                n(e.a(6)),
                                n(e.a(7)),
                                n(e.a(8)),
                                n(e.a(9)),
                                n(fog_to_lua(e.a(10))),
                                b(e.a(11)),
                                b(e.a(12)),
                                b(e.a(13)),
                            ],
                        );
                    }
                }
            }
            // The block holds the plot the unit stands on after the move; `path` is the way it went.
            K::UnitMoved => {
                self.note_unit_moved();
                self.move_unit(o, id, e.a(2), e.a(3));
                if seen.is_none() {
                    self.unit_cue(o, id, crate::EC_RUN, (e.a(2), e.a(3)), None);
                }
            }
            K::UnitTeleported => self.move_unit(o, id, u(0), u(1)),
            K::UnitRebased => self.move_unit(o, id, u(2), u(3)),
            K::UnitDestroyed | K::UnitDestroyedInCombat => {
                if e.kind == K::UnitDestroyedInCombat && seen.is_none() {
                    self.unit_cue(o, id, crate::EC_DEATH, (e.a(2), e.a(3)), None);
                }
                self.entities.borrow_mut().units.remove(&(o, id));
                self.fire_events("SerialEventUnitDestroyed", vec![n(o), n(id)]);
            }
            K::UnitSetDamage => {
                let old = {
                    let mut t = self.entities.borrow_mut();
                    t.units
                        .get_mut(&(o, id))
                        .map(|r| std::mem::replace(&mut r.damage, u(0)))
                };
                // In a rebuild only a change is told; a normal callback is told as it came (the exe calls it
                // only when the damage changed).
                if seen.is_none() || old.is_none_or(|d| d != u(0)) {
                    self.fire_events(
                        "SerialEventUnitSetDamage",
                        vec![n(o), n(id), n(u(0)), n(old.unwrap_or(u(1)))],
                    );
                }
            }
            K::UnitFortify | K::UnitWork | K::UnitActivate | K::UnitParadrop => {
                self.fire_events("UnitActionChanged", vec![n(o), n(id)]);
            }
            K::UnitEmbark => {
                if let Some(r) = self.entities.borrow_mut().units.get_mut(&(o, id)) {
                    r.embarked = u(0) != 0;
                }
                self.fire_events("UnitEmbark", vec![n(o), n(id), b(u(0))]);
            }
            K::UnitGarrison => {
                if let Some(r) = self.entities.borrow_mut().units.get_mut(&(o, id)) {
                    r.garrisoned = u(0) != 0;
                }
                self.fire_events("UnitGarrison", vec![n(o), n(id), b(u(0))]);
            }
            K::UnitShouldDimFlag => {
                if let Some(r) = self.entities.borrow_mut().units.get_mut(&(o, id)) {
                    r.dim = u(0) != 0;
                }
                self.fire_events("UnitShouldDimFlag", vec![n(o), n(id), b(u(0))]);
            }
            K::UnitMarkThreatening => {
                self.fire_events("UnitMarkThreatening", vec![n(o), n(id), b(u(0))]);
            }
            K::UnitVisibility => {
                if let Some(r) = self.entities.borrow_mut().units.get_mut(&(o, id)) {
                    r.visible = u(0) != 0;
                }
                self.fire_events(
                    "UnitVisibilityChanged",
                    vec![
                        n(o),
                        n(id),
                        b(u(0)),
                        b(u(1)),
                        Arg::Num(f64::from(u(2)) / 1000.0),
                    ],
                );
            }
            K::UnitMissionEnd => {
                // SIMPLIFIED(SIMP-exe-760): the replica applies a whole batch before its events are told, so the unit
                // of a mission that ended with its death (a settler founding a city, a unit consumed) is already
                // freed (SIMP-exe-692 cleared its Lua instance) when the UnitFlagManager's `FinishMove` asks it for
                // its transport: the Lua error "Not a valid instance" (m5-mp-ui). The exe told it with the unit
                // alive and the flag was destroyed after; here the move-queue event of a unit that is gone is dropped
                // (SerialEventUnitDestroyed of the same batch removes the flag).
                if !self.unit_gone(o, id) {
                    self.fire_events("UnitMoveQueueChanged", vec![n(o), n(id), Arg::Bool(false)]);
                }
            }

            K::CityCreated => {
                let rec = CityRec {
                    x: e.a(2),
                    y: e.a(3),
                    population: e.a(4),
                    damage: e.a(5),
                };
                if let Some(s) = seen.as_deref_mut() {
                    s.cities.insert((o, id));
                }
                let known = self.entities.borrow().cities.get(&(o, id)).cloned();
                match (known, seen.is_some()) {
                    (Some(old), true) => {
                        if old.population != rec.population {
                            self.fire_events(
                                "SerialEventCityPopulationChanged",
                                vec![n(o), n(id), n(rec.population)],
                            );
                        }
                        if old.damage != rec.damage {
                            self.fire_events(
                                "SerialEventCitySetDamage",
                                vec![n(o), n(id), n(rec.damage), n(old.damage)],
                            );
                        }
                        if let Some(r) = self.entities.borrow_mut().cities.get_mut(&(o, id)) {
                            *r = rec;
                        }
                    }
                    _ => {
                        let (x, y) = (rec.x, rec.y);
                        let (culture, fog) = {
                            let t = self.entities.borrow();
                            (
                                t.culture.get(&o).copied().unwrap_or(-1),
                                t.fog.get(&(x, y)).copied().unwrap_or(FOG_UNEXPLORED),
                            )
                        };
                        self.entities.borrow_mut().cities.insert((o, id), rec);
                        // SIMPLIFIED(SIMP-exe-681): see UnitCreated.
                        if seen.is_none() && self.is_seat_player(o) && !self.sound_quiet() {
                            self.host.event_sound("CityCreated");
                        }
                        // A city hides the resource icon of its plot.
                        self.resources_changed();
                        // (hexPos, owner, id, cultureType, eraType, continent, population, size, fowState)
                        self.fire_events(
                            "SerialEventCityCreated",
                            vec![
                                hex(x, y),
                                n(o),
                                n(id),
                                n(culture),
                                n(c(0)),
                                n(self.continent(x, y)),
                                n(e.a(4)),
                                n(e.a(7)),
                                n(fog_to_lua(fog)),
                            ],
                        );
                    }
                }
            }
            K::CityDestroyed | K::CityCaptured => {
                self.resources_changed();
                self.entities.borrow_mut().cities.remove(&(o, id));
                let name = if e.kind == K::CityDestroyed {
                    "SerialEventCityDestroyed"
                } else {
                    "SerialEventCityCaptured"
                };
                self.fire_events(name, vec![hex(e.a(2), e.a(3)), n(o), n(id), n(c(0))]);
            }
            K::CitySetDamage => {
                let old = {
                    let mut t = self.entities.borrow_mut();
                    t.cities
                        .get_mut(&(o, id))
                        .map(|r| std::mem::replace(&mut r.damage, c(0)))
                };
                if seen.is_none() || old.is_none_or(|d| d != c(0)) {
                    self.fire_events(
                        "SerialEventCitySetDamage",
                        vec![n(o), n(id), n(c(0)), n(old.unwrap_or(c(1)))],
                    );
                }
            }
            K::CityPopulationChanged => {
                let old = {
                    let mut t = self.entities.borrow_mut();
                    t.cities
                        .get_mut(&(o, id))
                        .map(|r| std::mem::replace(&mut r.population, c(0)))
                };
                if seen.is_none() || old.is_none_or(|p| p != c(0)) {
                    self.fire_events(
                        "SerialEventCityPopulationChanged",
                        vec![n(o), n(id), n(c(0))],
                    );
                }
            }

            K::FowChanged => {
                let (x, y, ty, all) = (e.a(0), e.a(1), e.a(2), e.a(3) != 0);
                let changed = {
                    let mut t = self.entities.borrow_mut();
                    if all {
                        let any = t.fog.values().any(|v| *v != ty) || t.fog.is_empty();
                        for v in t.fog.values_mut() {
                            *v = ty;
                        }
                        any
                    } else {
                        t.fog.insert((x, y), ty) != Some(ty)
                    }
                };
                if seen.is_none() || changed {
                    self.fire_events(
                        "HexFOWStateChanged",
                        vec![hex(x, y), n(fog_to_lua(ty)), Arg::Bool(all)],
                    );
                    if !all && ty != FOG_UNEXPLORED {
                        self.yield_revealed(x, y);
                    }
                    self.resources_changed();
                }
            }
            // The plot's state as the active team sees it (CvPlot::updateLayout): its resource icon.
            K::PlotStateChange | K::PlotIconStateChange => {
                self.plot_resource(e.a(0), e.a(1), e.a(2), e.a(3));
            }
            K::RefreshYieldVisibleMode => {
                for f in self.yield_fires(true) {
                    self.enqueue(f);
                }
            }
            K::YieldMightHaveChanged => {
                self.yield_changed(e.a(0), e.a(1));
            }
            K::NaturalWonderRevealed => {
                self.fire_events("NaturalWonderRevealed", vec![hex(e.a(0), e.a(1))]);
            }
            K::DoFx => {
                let (hx, hy) = crate::hex::grid_to_hex(e.a(0), e.a(1));
                // The exe's own listener (the particle system) plays the effect, then the Lua listeners run.
                // SIMPLIFIED(SIMP-131): `CvDLLUtility::GameplayDoFX` drops the event for a plot the active team has not
                // revealed; here it goes on and the effect's fog mask (FOWMask 3 of ART_DEF_VEFFECT_LOOK_AT) hides it there.
                self.fire(crate::core::Fire {
                    hook: Some(crate::core::Hook::GameplayFx { hx, hy }),
                    ..crate::core::Fire::events("GameplayFX", vec![n(hx), n(hy), n(e.a(2))])
                });
            }

            K::WarStateChanged => {
                self.fire_events("WarStateChanged", vec![n(e.a(0)), n(e.a(1)), b(e.a(2))]);
            }
            K::DiploAiLeaderMessage => {
                self.fire_events(
                    "AILeaderMessage",
                    vec![
                        n(e.a(0)),
                        n(e.a(1)),
                        Arg::Str(e.text.clone().unwrap_or_default()),
                        n(e.a(2)),
                        n(e.a(3)),
                    ],
                );
            }
            // K12: the DLL clears the trade table of the screen (an accepted deal) / opens the deal screen with a player.
            K::ClearDiploTradeTable => self.fire_events("ClearDiplomacyTradeTable", vec![]),
            K::OpenPlayerDealScreen => {
                self.fire_events("OpenPlayerDealScreenEvent", vec![n(e.a(0))]);
            }
            K::MetTeam => self.fire_events("TeamMet", vec![n(e.a(0)), n(e.a(1))]),
            K::TechAcquired => {
                // A tech may reveal resources.
                self.resources_changed();
                self.fire_events("TechAcquired", vec![n(e.a(0)), n(e.a(1))]);
            }
            K::EraChanged => self.fire_events("SerialEventEraChanged", vec![n(e.a(0)), n(e.a(1))]),
            K::WorldAnchor => {
                self.fire_events(
                    "GenericWorldAnchor",
                    vec![n(e.a(0)), b(e.a(1)), n(e.a(2)), n(e.a(3)), n(e.a(4))],
                );
            }
            K::SearchForPediaEntry => {
                self.fire_events(
                    "SearchForPediaEntry",
                    vec![Arg::Str(e.text.clone().unwrap_or_default())],
                );
            }
            K::OpenOptionsScreen => self.fire_events("EventOpenOptionsScreen", vec![]),
            K::OpenInfoCorner => self.fire_events("OpenInfoCorner", vec![n(e.a(0))]),
            K::GoldenAgeStarted => self.fire_events("GoldenAgeStarted", vec![]),
            K::GoldenAgeEnded => self.fire_events("GoldenAgeEnded", vec![]),

            K::TurnChanged => {
                self.entities.borrow_mut().turn = Some(e.a(0));
            }
            K::PublishNewGameTurn => {
                self.resources_changed();
                self.entities.borrow_mut().turn = Some(e.a(0));
                self.fire_events("NewGameTurn", vec![n(e.a(0))]);
            }
            K::ActivePlayerChanged | K::PublishEraChanges => {}
            K::PublishActivePlayer => {
                self.fire_events("GameplaySetActivePlayer", vec![n(e.a(0)), n(e.a(1))]);
            }
            K::ActivePlayerTurnStart => {
                self.ui.borrow_mut().sent_turn_complete = false;
                self.fire_events("ActivePlayerTurnStart", vec![]);
            }
            K::ActivePlayerTurnEnd => self.fire_events("ActivePlayerTurnEnd", vec![]),
            K::RemotePlayerTurnStart => self.fire_events("RemotePlayerTurnStart", vec![]),
            K::RemotePlayerTurnEnd => self.fire_events("RemotePlayerTurnEnd", vec![]),
            K::PlayerTurnStatus => {
                let tag = Arg::Str(e.text.clone().unwrap_or_default());
                // A human's own change may have been announced from the replica's flags already (SIMP-exe-911).
                let who = e.a(1);
                if (0..64).contains(&who)
                    && self
                        .seat_masks
                        .get()
                        .is_some_and(|(_, h)| h & (1u64 << who) != 0)
                    && matches!(e.a(0), 0 | 1)
                {
                    self.announce_human(who, e.a(0) == 0);
                    return;
                }
                match e.a(0) {
                    0 => self.fire_events("AIProcessingStartedForPlayer", vec![n(e.a(1)), tag]),
                    1 => self.fire_events("AIProcessingEndedForPlayer", vec![n(e.a(1)), tag]),
                    _ => {}
                }
            }
            K::PublishEndTurnDirty | K::UpdateEndTurn => {
                self.fire_events("SerialEventEndTurnDirty", vec![]);
            }
            K::EndTurnBlockingChanged => {
                self.fire_events("EndTurnBlockingChanged", vec![n(e.a(0)), n(e.a(1))]);
                self.fire_events("SerialEventEndTurnDirty", vec![]);
            }

            K::NotificationAdded => {
                // id, type, x, y, data1, data2, player; text = message, text2 = summary
                let rec = NoteRec {
                    id: e.a(0),
                    kind: e.a(1),
                    tip: e.text.clone().unwrap_or_default(),
                    summary: e.text2.clone().unwrap_or_default(),
                    game_data: e.a(4),
                    extra: e.a(5),
                    player: e.a(6),
                };
                let fresh = !e.rebuild && !self.notes.borrow().contains_key(&rec.id);
                self.notes.borrow_mut().insert(rec.id, rec.clone());
                if fresh
                    && !self.sound_quiet()
                    && let Some(name) = self.notification_type_name(rec.kind)
                {
                    self.host.notification_sound(&name);
                }
                self.fire_note(&rec);
            }
            K::NotificationRemoved => {
                self.notes.borrow_mut().remove(&e.a(0));
                self.fire_events("NotificationRemoved", vec![n(e.a(0))]);
            }
            K::NotificationActivated => {
                self.fire_events(
                    "NotificationActivated",
                    vec![
                        n(e.a(0)),
                        n(e.a(1)),
                        Arg::Str(e.text.clone().unwrap_or_default()),
                        n(e.a(2)),
                        n(e.a(3)),
                        n(e.a(4)),
                        n(e.a(5)),
                        n(e.a(6)),
                    ],
                );
            }
            K::Message => {
                self.fire_events(
                    "GameplayAlertMessage",
                    vec![Arg::Str(e.text.clone().unwrap_or_default())],
                );
            }
            K::Popup => {
                // ButtonPopupTypes, data1, data2, data3, option1, option2, player; text
                let info = PopupInfo::from_core(
                    e.a(0),
                    &[e.a(1), e.a(2), e.a(3)],
                    &[e.a(4) != 0, e.a(5) != 0],
                    e.text.as_deref().unwrap_or_default(),
                );
                self.popup_sound(e.a(0), [e.a(1), e.a(2), e.a(3)]);
                self.add_popup(info);
            }
            K::PopupArray => {
                // ButtonPopupTypes, n ints, the ints, n bools, the bools
                let ints_n = e.a(1).max(0) as usize;
                let ints: Vec<i32> = (0..ints_n).map(|i| e.a(2 + i)).collect();
                let bools_n = e.a(2 + ints_n).max(0) as usize;
                let bools: Vec<bool> = (0..bools_n).map(|i| e.a(3 + ints_n + i) != 0).collect();
                let info = PopupInfo::from_core(
                    e.a(0),
                    &ints,
                    &bools,
                    e.text.as_deref().unwrap_or_default(),
                );
                let data = [0, 1, 2].map(|i| ints.get(i).copied().unwrap_or(-1));
                self.popup_sound(e.a(0), data);
                self.add_popup(info);
            }
            K::PopupText => {
                let [wx, wy, wz] = crate::hex::grid_to_world_on(&*self.host, e.a(0), e.a(1));
                self.fire_events(
                    "AddPopupTextEvent",
                    vec![
                        Arg::Vec(vec![f64::from(wx), f64::from(wy), f64::from(wz)]),
                        Arg::Str(e.text.clone().unwrap_or_default()),
                        Arg::Num(f64::from(e.a(2)) / 1000.0),
                    ],
                );
            }
            K::SetDirty => {
                if e.a(1) != 0 {
                    self.dirty.borrow_mut().insert(e.a(0));
                }
            }
            // `UI.GetTempString`: the line the next popup shows (CvNotifications::Activate puts the city-state's message here
            // right before BUTTONPOPUP_CITY_STATE_MESSAGE, whose Lua reads it as the popup is raised).
            K::SetTempString => {
                if let Some(ctx) = self.lua_ctx() {
                    let text = e.text.clone().unwrap_or_default();
                    let _ = ctx.lua().globals().raw_set("__exe_temp_string", text);
                }
            }
            // SIMPLIFIED(SIMP-exe-952): a border that moved (a tile bought, a city founded or grown) redraws the plots the
            // city screen highlights (CityView `UpdateWorkingHexes`, a dirty flag); the exe's own sources of
            // SerialEventCityHexHighlightDirty were not recovered.
            K::UpdateCountryBorder | K::UpdateAllCountryBorders => {
                self.fire_events("SerialEventCityHexHighlightDirty", vec![]);
            }
            K::LookAt => self.host.look_at(e.a(0), e.a(1)),
            // The DLL's `lookAtSelectionPlot(release)` (CONTROL_NEXTCITY / PREVCITY, CONTROL_CENTERONSELECTION, the unit
            // cycle): the city screen's arrows switch the city and then centre on it (RB4 night-1b, L4). The argument
            // releases a locked camera and moves nothing.
            K::LookAtSelectionPlot => {
                if e.a(0) == 0 {
                    self.look_at_selection();
                }
            }
            // CENTER_CAMERA carries a unit block: the camera goes to its plot.
            K::CenterCamera => self.host.look_at(e.a(2), e.a(3)),
            K::SpecificCityInfoDirty => {
                self.fire_events("SpecificCityInfoDirty", vec![n(o), n(id), n(c(0))]);
            }
            K::UnitSelectionChanged => {
                // selected count, head owner, head id (-1, -1 = none)
                let head = (e.a(0) > 0 && e.a(1) >= 0).then(|| (e.a(1), e.a(2)));
                let prev = std::mem::replace(&mut self.entities.borrow_mut().selected, head);
                if prev != head {
                    if let Some((po, pid)) = prev {
                        self.fire_selection(po, pid, false);
                    }
                    if let Some((ho, hid)) = head {
                        self.fire_selection(ho, hid, true);
                        if !e.rebuild {
                            self.play_unit_selection(ho, hid);
                        }
                    }
                    if head.is_none() {
                        self.fire_events("UnitSelectionCleared", vec![]);
                    }
                    // SIMPLIFIED(SIMP-exe-63): the game sets the unit-info dirty bits on a selection; the core does not.
                    // It is a dirty bit (bit 19), so it is raised once per frame after the other events (K1 order).
                    self.dirty.borrow_mut().insert(19);
                }
            }
            K::InterfaceModeChanged => {
                self.fire_events("InterfaceModeChanged", vec![n(e.a(0)), n(e.a(1))]);
            }
            // SIMPLIFIED(SIMP-exe-63): the exe redraws the city screen (CityScreen_DIRTY_BIT) when the game asks
            // (`updateCityScreen`) and when the selected city changes; the core only announces both. Without it
            // CityView.lua (EUI) kept the state of its last refresh: a screen opened again after RETURN stayed in
            // its "viewing mode" of the exit and took no order (live report 2026-10-30, "the city screen does not
            // react after a turn").
            K::UpdateCityScreen | K::CitySelectionChanged => {
                self.dirty.borrow_mut().insert(21);
            }
            K::CityScreenChanged => {
                let up = e.a(0) != 0;
                if up {
                    self.dirty.borrow_mut().insert(21);
                }
                if !up && self.city_exit_echo.get() > 0 {
                    // The echo of an exit a script fired itself (Hook::ExitCityScreen).
                    self.city_exit_echo.set(self.city_exit_echo.get() - 1);
                } else {
                    self.host.city_screen(up);
                    // The exe's city screen shows the city it opened on, however the core opened it (the Lua entry
                    // points look at it themselves; this one covers the core's own `setCityScreenUp`).
                    if up && !e.rebuild {
                        self.look_at_selection();
                    }
                    if !e.rebuild {
                        match up.then(|| self.city_ambience_script()).flatten() {
                            Some(s) => self.host.city_ambience(Some(&s)),
                            None => self.host.city_ambience(None),
                        }
                    }
                    let name = if up {
                        "SerialEventEnterCityScreen"
                    } else {
                        "SerialEventExitCityScreen"
                    };
                    self.fire_events(name, vec![]);
                }
            }

            K::GameEvent => {
                self.counters
                    .game_events_in
                    .set(self.counters.game_events_in.get() + 1);
                // SIMPLIFIED(SIMP-exe-2): GameEvents reach the UI through the FIFO, after the batch.
                if let Some(name) = &e.text {
                    self.enqueue(Fire {
                        family: Family::GameEvents,
                        name: name.clone(),
                        payload: Payload::Args(e.values.iter().map(Arg::from).collect()),
                        hook: None,
                    });
                }
            }
            K::LuaEvent => {
                if let Some(name) = &e.text {
                    self.fire(Fire {
                        family: Family::Events,
                        name: name.clone(),
                        payload: Payload::Args(e.values.iter().map(Arg::from).collect()),
                        hook: None,
                    });
                }
            }

            // SIMPLIFIED(SIMP-exe-921): of the trade route visuals only the popup route is kept (the lines of the routes
            // come from `Q_TRADE_PATHS`, SIMP-exe-660); the temporary route's event carries the first 9 plots in the args
            // and, past 9, all of them in `path` (x, y pairs), so the whole route is kept.
            K::TradeNewRoute => {
                if e.a(0) == -1 {
                    let plots: Vec<(i32, i32)> = if e.path.len() >= 2 {
                        e.path
                            .chunks_exact(2)
                            .map(|c| (c[0] as i32, c[1] as i32))
                            .collect()
                    } else {
                        (0..e.a(3).clamp(0, 9))
                            .map(|i| (e.a(4 + 2 * i as usize), e.a(5 + 2 * i as usize)))
                            .collect()
                    };
                    let count = if e.path.len() >= 2 {
                        plots.len() as i32
                    } else {
                        e.a(3)
                    };
                    self.ui.borrow_mut().temp_route = Some(crate::core::TempRoute {
                        player: e.a(1),
                        kind: e.a(2),
                        count,
                        plots,
                    });
                }
            }
            K::TradeDestroyRoute => {
                let mut ui = self.ui.borrow_mut();
                if e.a(0) == -1 {
                    ui.temp_route = None;
                }
                if ui.popup_route == Some(e.a(0)) {
                    ui.popup_route = None;
                }
            }
            K::TradeActivatePopupRoute => self.ui.borrow_mut().popup_route = Some(e.a(0)),
            K::TradeDeactivatePopupRoute => self.ui.borrow_mut().popup_route = None,

            // The fight has no listener either; its sounds are the units' (re-b15 ITEM 4, SIMP-exe-1202).
            K::UnitCombat | K::CityCombat => {
                if seen.is_none() {
                    self.combat_sounds(e);
                }
                self.unmapped(e.kind);
            }
            // SIMPLIFIED(SIMP-exe-941): the kinds below have no listener (the DLL never raises some of them, the rest is render-only).
            K::UnitResetAnimation
            | K::WallCreated
            | K::MinimapUnitSelect
            | K::MinimapNotification
            | K::BridgeChanged
            | K::FeatureChanged
            | K::NotifyAiLeadersInGame
            | K::NotifySpecificAiLeader
            | K::WonderCreated
            | K::WonderEdited
            | K::WonderRemoved
            | K::SpaceshipCreated
            | K::SpaceshipEdited
            | K::SpaceshipRemoved
            | K::TradeRouteDirection
            | K::ClearEventMessages
            | K::AddHexToUiRange
            | K::OpenEndGameMenu
            | K::OpenTechTree
            | K::AddDeferredWonderCommand
            | K::ToggleStrategicView
            | K::LookAtCityOffset
            | K::Flashing
            | K::Unknown(_) => self.unmapped(e.kind),
        }
    }

    /// `NotificationAdded(id, type, toolTip, summary, gameValue, extra, player)`.
    pub fn fire_note(&self, r: &NoteRec) {
        self.fire_events(
            "NotificationAdded",
            vec![
                n(r.id),
                n(r.kind),
                Arg::Str(r.tip.clone()),
                Arg::Str(r.summary.clone()),
                n(r.game_data),
                n(r.extra),
                n(r.player),
            ],
        );
    }

    /// Sounds are held while the replica joins or loads (SIMP-exe-702).
    pub(crate) fn sound_quiet(&self) -> bool {
        self.counters.frames.get() <= self.sound_quiet_until.get()
    }

    /// The sound of a popup type (`ButtonPopupSounds.xml`) as the popup comes, and its spoken line
    /// (`OnButtonPopupDisplayShow`, re-b15 ITEM 5 P5.2): a city-state's popups speak its greeting instead of the table
    /// sound; a new era, a tech and a wonder of the seat speak their line after the sound. SIMPLIFIED(SIMP-exe-682): it
    /// plays when the core announces the popup, the exe plays it when its queue shows it.
    fn popup_sound(&self, kind: i32, data: [i32; 3]) {
        if self.sound_quiet() {
            return;
        }
        let Some(name) = self.popup_type_name(kind) else {
            return;
        };
        if crate::sound_lines::is_city_state_popup(&name)
            && let Some(minor) = self.minor_civ_type(data[0])
        {
            self.host.minor_civ_sound(&minor);
            return;
        }
        self.host.popup_sound(&name);
        if let Some((script, delay_ms)) = self.popup_voice(&name, data) {
            self.host.voice_over(&script, delay_ms);
        }
    }

    /// A unit's action the port does not animate, for its sounds: only a unit the seat sees, and not while the replica
    /// joins or loads.
    fn unit_cue(&self, o: i32, id: i32, ec: u32, at: (i32, i32), opponent: Option<(i32, i32)>) {
        if self.sound_quiet() {
            return;
        }
        let visible = self
            .entities
            .borrow()
            .units
            .get(&(o, id))
            .is_some_and(|r| r.visible);
        if !visible {
            return;
        }
        self.host.unit_clip_sound(crate::UnitClipCue {
            owner: o,
            id,
            ec,
            x: at.0,
            y: at.1,
            opponent,
        });
    }

    /// `UNIT_COMBAT` / `CITY_COMBAT`: the attacker plays its attack (the city attack against a city) and a defender that
    /// strikes back its own; only a fight the core asks to show (flag 2). The deaths come as `UnitDestroyedInCombat`.
    fn combat_sounds(&self, e: &UiEvent) {
        const UNIT: i32 = 1;
        const CITY: i32 = 2;
        let flags = e.a(11);
        if flags & 2 == 0 {
            return;
        }
        let (ak, ao, aid) = (e.a(0), e.a(1), e.a(2));
        let (dk, d_o, did) = (e.a(3), e.a(4), e.a(5));
        let at = (e.a(9), e.a(10));
        let defender = (dk == UNIT).then_some((d_o, did));
        if ak == UNIT {
            let ec = if dk == CITY {
                crate::EC_ATTACK_CITY
            } else {
                crate::EC_ATTACK
            };
            self.unit_cue(ao, aid, ec, at, defender);
        }
        if let Some((d_o, did)) = defender
            && flags & 4 != 0
        {
            self.unit_cue(
                d_o,
                did,
                crate::EC_ATTACK,
                at,
                (ak == UNIT).then_some((ao, aid)),
            );
        }
    }

    /// `InterfaceBuddy::AddPopup`.
    pub fn add_popup(&self, info: PopupInfo) {
        let fires = self.buddy.borrow_mut().add(info);
        for f in fires {
            self.enqueue(f);
        }
    }

    fn move_unit(&self, o: i32, id: i32, x: i32, y: i32) {
        // The DLL places a new unit (`CvUnit::init` -> `setXY`) before it announces it: a unit the UI has not
        // been told of has no flag to move; its `UnitCreated` carries the plot.
        match self.entities.borrow_mut().units.get_mut(&(o, id)) {
            Some(u) => {
                u.x = x;
                u.y = y;
            }
            None => return,
        }
        // SIMPLIFIED(SIMP-exe-34): no movement animation; the flag jumps to the new plot (the exe walks it with
        // LocalMachineUnitPositionChanged).
        let (hx, hy) = crate::hex::grid_to_hex(x, y);
        self.fire_events(
            "SerialEventUnitTeleportedToHex",
            vec![n(hx), n(hy), n(o), n(id)],
        );
    }

    /// A known unit announced again: what changed.
    fn unit_diff(&self, o: i32, id: i32, old: &UnitRec, new: &UnitRec) {
        if let Some(u) = self.entities.borrow_mut().units.get_mut(&(o, id)) {
            u.x = new.x;
            u.y = new.y;
            u.visible = new.visible;
            u.fog = new.fog;
        }
        if (old.x, old.y) != (new.x, new.y) {
            let (hx, hy) = crate::hex::grid_to_hex(new.x, new.y);
            self.fire_events(
                "SerialEventUnitTeleportedToHex",
                vec![n(hx), n(hy), n(o), n(id)],
            );
            if self.entities.borrow().selected == Some((o, id)) {
                self.fire_unit_hex_highlight(hx, hy, true, id);
            }
        }
        if old.fog != new.fog {
            self.fire_events(
                "UnitStateChangeDetected",
                vec![n(o), n(id), n(fog_to_lua(new.fog))],
            );
        }
        if old.visible != new.visible {
            self.fire_events(
                "UnitVisibilityChanged",
                vec![
                    n(o),
                    n(id),
                    Arg::Bool(new.visible),
                    Arg::Bool(true),
                    Arg::Num(0.0),
                ],
            );
        }
    }
}
