//! The yield and resource icons of the map. The original draws both in Lua (`YieldIconManager.lua` of EUI,
//! `ResourceIconManager.lua`), as world anchors; the exe only tells them where:
//!
//! - The exe's `YieldIconManager` (0x086daa30..0x086dc2f4: `OnRequestYieldDisplay`, `SwitchToDisplayMode`,
//!   `BroadcastCurrent`, `OnHexFOWStateChanged`) answers `Events.RequestYieldDisplay(type, ...)` with
//!   `Events.ShowHexYield(x, y, shown)` per plot (grid coordinates). `UI.SetYieldVisibleMode(on)` (the minimap
//!   panel's "Yield Icons" box) asks for `USER_ALL_ON` / `USER_ALL_OFF`, `UI.RefreshYieldVisibleMode()` (the Lua
//!   manager at its start and on a new active player) shows the current set again.
//! - `UI.SetResourceVisibleMode(on)` asks for `USER_ALL_RESOURCE_ON` / `_OFF`: `ResourceIconManager.lua` shows or hides
//!   its container. The icons come from `Events.SerialEventRawResourceIconCreated(hexX, hexY, improvement, resource)`
//!   and `SerialEventRawResourceIconDestroyed(hexX, hexY)`, which the exe raises from the DLL's
//!   `GameplayPlotStateChange` (the plot's resource as the active team sees it, none on a city). The replica does not
//!   run the DLL's `CvMap::updateLayout`, so the same state is read through the DLL's Lua API once per dispatch after
//!   a change of the fog, a tech, a city, a turn or a load ([`Core::sync_resources`]).
//!
//! The exe keeps to the plots in the camera's hex rectangle (the host's [`crate::ExeHost::visible_cells`]: the grid span
//! of the view, one hex wider each side, clamped; two rectangles where a wrapping map's view straddles the seam):
//! `USER_ALL_ON` shows every plot of it (the Lua drops the unrevealed ones), the other sets only their plots in it, and
//! a move of the camera shows and hides the plots that entered and left (re: `YieldIconManager::OnVisibilityUpdated`).
//!
//! The icons follow the plots' yields ([`Core::sync_yield_changes`], SIMP-exe-620): the replica does not run the DLL's
//! `CvPlot::updateSymbols` for a belief, a policy or a tech, so the yields of the shown plots are read through the DLL's
//! Lua API and the plots whose numbers changed raise `HexYieldMightHaveChanged` (and `ShowHexYield`) again.
//!
//! SIMPLIFIED(SIMP-exe-133): the bodies of the exe's `YieldIconManager` are not recovered. The requests replace one set of
//! their own on top of `USER_ALL_ON`: `AREA(radius, x, y)` the
//! plots within `radius` of `(x, y)` (radius 0 clears it), `EMPIRE` the active player's plots, `CITY_OWNED` /
//! `CITY_WORKED` / `CITY_PURCHASABLE(x, y)` the plots the city at `(x, y)` works on (worked ones only for `CITY_WORKED`).
//! The resource icon events follow the DLL's plot state (`PLOT_STATE_CHANGE` / `PLOT_ICON_STATE_CHANGE`, or the read
//! of it): a new or changed resource raises `Created`, a resource gone raises `Destroyed`.

use std::collections::{BTreeSet, HashMap};

use lua_sys::LuaValue;

use crate::core::{Arg, Core, Fire};

/// `YieldDisplayTypes` (enums.rs).
pub const USER_ALL_ON: i32 = 0;
pub const USER_ALL_OFF: i32 = 1;
pub const USER_ALL_RESOURCE_ON: i32 = 2;
pub const USER_ALL_RESOURCE_OFF: i32 = 3;
pub const EMPIRE: i32 = 4;
pub const CITY_OWNED: i32 = 5;
pub const CITY_WORKED: i32 = 6;
pub const CITY_PURCHASABLE: i32 = 7;
pub const AREA: i32 = 8;

/// What the exe keeps of the map icons.
#[derive(Debug, Default)]
pub struct MapIcons {
    /// `USER_ALL_ON` is in force.
    pub user_all: bool,
    /// The last other request (type and its arguments), if it is in force.
    pub request: Option<(i32, [i32; 3])>,
    /// The plots `ShowHexYield(.., true)` was raised for last.
    pub shown: BTreeSet<(i32, i32)>,
    /// The plots that left the shown set and have not come back: their numbers were not polled meanwhile.
    pub left: BTreeSet<(i32, i32)>,
    /// The resource (and improvement) each plot's icon was announced with.
    pub resources: HashMap<(i32, i32), (i32, i32)>,
    /// The improvement and the road of each plot as the last read saw them (`None` before the first read, which only
    /// records): a change raises `SerialEventImprovementCreated` / `SerialEventRoadCreated`.
    pub built: Option<HashMap<(i32, i32), (i32, i32)>>,
    /// The fog, a tech, a city or a load may have changed which resources the active team sees: the next dispatch
    /// reads them again ([`Core::sync_resources`]).
    pub resources_dirty: bool,
    /// The camera's hex rectangles `(x0, y0, x1, y1)`, upper bounds exclusive, wrapped and clamped to the map; `None`
    /// until the host reports them (the whole map).
    pub rects: Option<Vec<[i32; 4]>>,
    /// The host's rectangle of the last frame (unwrapped), to see a move of the camera.
    pub rect_seen: Option<[i32; 4]>,
    /// The yields of each shown plot as last read ([`Core::sync_yield_changes`]).
    pub yield_sigs: HashMap<(i32, i32), String>,
    /// The yields may have changed (an event of the game, a turn, a load): the next scan reads them.
    pub yields_dirty: bool,
    /// The frame of the last scan.
    pub yield_scan_frame: u64,
    /// The stored map options were applied once ([`Core::apply_stored_map_options`]).
    pub options_applied: bool,
}

/// How often the yields of the shown plots are read at most, in frames.
const YIELD_SCAN_EVERY: u64 = 20;
/// The yields are read this often even when no event said they changed (a belief or a policy raises none).
const YIELD_SCAN_IDLE: u64 = 90;

/// The yields of the plots `x1, y1, x2, y2, ...` the active team sees, as `x, y, "food,prod,..."`.
const YIELDS_LUA: &str = r#"
local plots = ...
local out = {}
if not (Map and Map.GetPlot and Game) then return out end
local team = Game.GetActiveTeam and Game.GetActiveTeam() or 0
local n = (YieldTypes and YieldTypes.NUM_YIELD_TYPES or 5)
for i = 1, #plots, 2 do
	local x, y = plots[i], plots[i + 1]
	local p = Map.GetPlot(x, y)
	if p and (not p.IsRevealed or p:IsRevealed(team, false)) and p.CalculateYield then
		local t = {}
		for k = 0, n - 1 do
			local ok, v = pcall(p.CalculateYield, p, k, true)
			t[#t + 1] = ok and tostring(v) or "?"
		end
		if p.GetCulture then
			local ok, v = pcall(p.GetCulture, p)
			t[#t + 1] = ok and tostring(v) or "?"
		end
		out[#out + 1] = x; out[#out + 1] = y; out[#out + 1] = table.concat(t, ",")
	end
end
return out
"#;

/// The size of the map and whether its columns wrap.
const DIMS_LUA: &str = r#"
if not (Map and Map.GetGridSize) then return {0, 0, 0} end
local w, h = Map.GetGridSize()
local wrap = 0
if Map.IsWrapX then local ok, v = pcall(Map.IsWrapX); if ok and v then wrap = 1 end end
return {w, h, wrap}
"#;

/// The resources the active team sees: `x, y, resource, improvement` per plot (none under a city), as the DLL's
/// `CvPlot::updateLayout` gives them to `GameplayPlotStateChange`.
const RESOURCES_LUA: &str = r#"
local out, built = {}, {}
if not (Map and Map.GetGridSize and Game) then return out, built end
local team = Game.GetActiveTeam()
local w, h = Map.GetGridSize()
for y = 0, h - 1 do
	for x = 0, w - 1 do
		local p = Map.GetPlot(x, y)
		if p and p:IsRevealed(team, false) then
			local r = p:GetResourceType(team)
			if not p:IsCity() and r and r >= 0 then
				out[#out + 1] = x; out[#out + 1] = y; out[#out + 1] = r; out[#out + 1] = p:GetImprovementType()
			end
			local imp = p:GetImprovementType()
			local route = p.GetRouteType and p:GetRouteType() or -1
			if (imp and imp >= 0) or (route and route >= 0) then
				built[#built + 1] = x; built[#built + 1] = y; built[#built + 1] = imp or -1
				built[#built + 1] = route or -1; built[#built + 1] = p.GetOwner and p:GetOwner() or -1; built[#built + 1] = r or -1
			end
		end
	end
end
return out, built
"#;

/// The plots of a request, as the Lua of the game sees them: a flat list `x1, y1, x2, y2, ...`.
const PLOTS_LUA: &str = r#"
local ty, a, b, c = ...
local out = {}
if not (Map and Map.GetGridSize) then return out end
local w, h = Map.GetGridSize()
local function each(f)
	for y = 0, h - 1 do
		for x = 0, w - 1 do
			local p = Map.GetPlot(x, y)
			if p and f(p, x, y) then out[#out + 1] = x; out[#out + 1] = y end
		end
	end
end
if ty == 0 then
	each(function() return true end)
elseif ty == 8 then
	if a > 0 then each(function(_, x, y) return Map.PlotDistance(b, c, x, y) <= a end) end
elseif ty == 4 then
	local me = Game.GetActivePlayer()
	each(function(p) return p:GetOwner() == me end)
elseif ty == 5 or ty == 6 or ty == 7 then
	local cp = Map.GetPlot(a, b)
	local city = cp and cp:GetPlotCity()
	if city then
		local id, owner = city:GetID(), city:GetOwner()
		each(function(p, x, y)
			if Map.PlotDistance(a, b, x, y) > 5 then return false end
			local wc = p:GetWorkingCity()
			if not (wc and wc:GetID() == id and wc:GetOwner() == owner) then return false end
			return ty ~= 6 or p:IsBeingWorked()
		end)
	end
end
return out
"#;

impl Core {
    /// `Events.RequestYieldDisplay(type, a, b, c)`: the new set of plots with yield icons; the `ShowHexYield` of the
    /// plots that come and go.
    pub fn yield_display_fires(&self, ty: i32, args: [i32; 3]) -> Vec<Fire> {
        {
            let mut m = self.map_icons.borrow_mut();
            match ty {
                USER_ALL_ON => m.user_all = true,
                USER_ALL_OFF => m.user_all = false,
                USER_ALL_RESOURCE_ON | USER_ALL_RESOURCE_OFF => return Vec::new(),
                AREA if args[0] <= 0 => m.request = None,
                EMPIRE | CITY_OWNED | CITY_WORKED | CITY_PURCHASABLE | AREA => {
                    m.request = Some((ty, args));
                }
                _ => return Vec::new(),
            }
        }
        self.yield_fires(false)
    }

    /// The `ShowHexYield` events that bring the Lua to the current set: the plots that left it are hidden, the new
    /// ones shown (`again`: every plot of the set is shown again, `UI.RefreshYieldVisibleMode`).
    pub fn yield_fires(&self, again: bool) -> Vec<Fire> {
        let (user_all, request) = {
            let m = self.map_icons.borrow();
            (m.user_all, m.request)
        };
        self.refresh_rects();
        let rects = self.map_icons.borrow().rects.clone();
        let inside = |p: &(i32, i32)| {
            rects.as_ref().is_none_or(|r| {
                r.iter()
                    .any(|c| p.0 >= c[0] && p.0 < c[2] && p.1 >= c[1] && p.1 < c[3])
            })
        };
        let mut want: BTreeSet<(i32, i32)> = BTreeSet::new();
        if user_all {
            want.extend(self.request_plots(USER_ALL_ON, [0; 3]));
        }
        if let Some((ty, a)) = request {
            want.extend(self.request_plots(ty, a).into_iter().filter(inside));
        }
        let mut m = self.map_icons.borrow_mut();
        let show = |(x, y): (i32, i32), on: bool| {
            Fire::events(
                "ShowHexYield",
                vec![Arg::from(x), Arg::from(y), Arg::Bool(on)],
            )
        };
        let gone: Vec<(i32, i32)> = m.shown.difference(&want).copied().collect();
        let mut fires: Vec<Fire> = gone.iter().map(|p| show(*p, false)).collect();
        m.left.extend(gone);
        let fresh: Vec<(i32, i32)> = want
            .iter()
            .filter(|p| again || !m.shown.contains(p))
            .copied()
            .collect();
        for p in &fresh {
            if !m.shown.contains(p) && m.left.remove(p) {
                // A plot that comes into view again (the camera rectangle) has its numbers rebuilt first: while it was
                // out of the set nothing polled it, so the Lua's cached icon could show an old yield (audit 40).
                fires.push(Fire::events(
                    "HexYieldMightHaveChanged",
                    vec![Arg::from(p.0), Arg::from(p.1)],
                ));
            }
            fires.push(show(*p, true));
        }
        m.shown = want;
        fires
    }

    /// The size of the map and whether it wraps (0, 0 before the map exists).
    fn map_dims(&self) -> (i32, i32, bool) {
        let Some(ctx) = self.lua_ctx() else {
            return (0, 0, false);
        };
        match ctx
            .lua()
            .load(DIMS_LUA)
            .set_name("=exe_dims")
            .eval::<LuaValue>()
        {
            Ok(LuaValue::Table(t)) => {
                let v: Vec<i32> = t.sequence_values::<i32>().unwrap_or_default();
                if v.len() == 3 {
                    return (v[0], v[1], v[2] != 0);
                }
                (0, 0, false)
            }
            _ => (0, 0, false),
        }
    }

    /// Once per frame: the camera's hex rectangle (the host's, unwrapped, margin included) as the rectangles of the map
    /// (clamped in y, split at the seam of a wrapping map); a change shows the plots that came into view and hides those
    /// that left it.
    pub fn sync_camera_rect(&self) {
        if !self.refresh_rects() {
            return;
        }
        let any = {
            let m = self.map_icons.borrow();
            m.user_all || m.request.is_some()
        };
        if any {
            for f in self.yield_fires(false) {
                self.enqueue(f);
            }
        }
    }

    /// The camera's rectangles of the map from the host's; true when they changed.
    fn refresh_rects(&self) -> bool {
        if !self.setup_done.get() {
            return false;
        }
        let Some(r) = self.host.visible_cells() else {
            return false;
        };
        if self.map_icons.borrow().rect_seen == Some(r) {
            return false;
        }
        let (w, h, wrap) = self.map_dims();
        if w <= 0 || h <= 0 {
            return false;
        }
        let (y0, y1) = (r[1].max(0), r[3].min(h));
        let mut rects = Vec::new();
        if y0 < y1 {
            if wrap && r[2] - r[0] < w {
                // The columns past either edge are the other side's.
                let (x0, x1) = (r[0], r[2]);
                if x0 < 0 {
                    rects.push([x0 + w, y0, w, y1]);
                }
                if x1 > w {
                    rects.push([0, y0, x1 - w, y1]);
                }
                rects.push([x0.max(0), y0, x1.min(w), y1]);
            } else {
                rects.push([r[0].max(0), y0, r[2].min(w), y1]);
            }
            rects.retain(|c| c[0] < c[2]);
        }
        {
            let mut m = self.map_icons.borrow_mut();
            m.rect_seen = Some(r);
            if m.rects.as_ref() == Some(&rects) {
                return false;
            }
            m.rects = Some(rects);
        }
        true
    }

    /// The map options of the minimap panel, as the browser stored them, are in force from the first frame: the grid, the
    /// yield icons and the resource icons (the panel's own `ApplyMapOptions` can come before the map exists, and its boxes
    /// show the stored state whatever the map shows). Once, at the first setup of a game.
    pub fn apply_stored_map_options(&self) {
        if self.map_icons.borrow().options_applied {
            return;
        }
        self.map_icons.borrow_mut().options_applied = true;
        let get = |key: &str, default: bool| {
            self.host
                .kv()
                .get(&format!("{}{key}", crate::options::KEY_PREFIX))
                .and_then(|s| s.trim().parse::<i32>().ok())
                .map_or(default, |v| v != 0)
        };
        let (grid, yields, resources) = (
            get("GridOn", true),
            get("YieldOn", false),
            get("ResourceOn", false),
        );
        self.ui.borrow_mut().grid_on = grid;
        self.host.set_grid_visible(grid);
        self.fire_events(
            if grid {
                "SerialEventHexGridOn"
            } else {
                "SerialEventHexGridOff"
            },
            vec![],
        );
        for (on, off, want) in [
            (USER_ALL_ON, USER_ALL_OFF, yields),
            (USER_ALL_RESOURCE_ON, USER_ALL_RESOURCE_OFF, resources),
        ] {
            let ty = if want { on } else { off };
            self.enqueue(Fire {
                hook: Some(crate::core::Hook::RequestYieldDisplay(ty, [0; 3])),
                ..Fire::events("RequestYieldDisplay", vec![Arg::from(ty)])
            });
        }
    }

    /// The yields of a plot may have changed (the DLL's `updateSymbols`): the Lua rebuilds its numbers, and the plot is
    /// shown again when it is in the set (re: `OnHexYieldMightHaveChanged`).
    pub fn yield_changed(&self, x: i32, y: i32) {
        if self.map_icons.borrow().shown.contains(&(x, y)) {
            self.fire_events(
                "ShowHexYield",
                vec![Arg::from(x), Arg::from(y), Arg::Bool(true)],
            );
        }
        // The listeners read the grid position (`Map.GetPlot(x, y)`), not a hex.
        self.fire_events("HexYieldMightHaveChanged", vec![Arg::from(x), Arg::from(y)]);
    }

    /// Something may have changed the yields of the map (an event of the game, a new turn, a load).
    pub fn yields_changed(&self) {
        self.map_icons.borrow_mut().yields_dirty = true;
    }

    /// Once per dispatch: the yields of the shown plots against those last read; a plot whose numbers changed (a belief,
    /// a tech, a policy, an improvement, a feature, a city, the borders) is told again.
    pub fn sync_yield_changes(&self) {
        if !self.setup_done.get() {
            return;
        }
        let frame = self.counters.frames.get();
        let (due, plots) = {
            let m = self.map_icons.borrow();
            let since = frame.saturating_sub(m.yield_scan_frame);
            let due = (m.yields_dirty && since >= YIELD_SCAN_EVERY) || since >= YIELD_SCAN_IDLE;
            (due, m.shown.iter().copied().collect::<Vec<_>>())
        };
        if !due || plots.is_empty() {
            return;
        }
        {
            let mut m = self.map_icons.borrow_mut();
            m.yields_dirty = false;
            m.yield_scan_frame = frame;
        }
        let Some(ctx) = self.lua_ctx() else {
            return;
        };
        let lua = ctx.lua();
        let t = lua.create_table();
        for (i, (x, y)) in plots.iter().enumerate() {
            let _ = t.raw_set(2 * i + 1, *x);
            let _ = t.raw_set(2 * i + 2, *y);
        }
        let r = lua
            .load(YIELDS_LUA)
            .set_name("=exe_yields")
            .into_function()
            .and_then(|f| f.call::<_, LuaValue>(t));
        let flat: Vec<LuaValue> = match r {
            Ok(LuaValue::Table(t)) => t.sequence_values::<LuaValue>().unwrap_or_default(),
            Ok(_) => return,
            Err(e) => {
                tracing::debug!(target: "exe.ui", "yields: {e}");
                return;
            }
        };
        let mut now: HashMap<(i32, i32), String> = HashMap::new();
        for c in flat.chunks_exact(3) {
            let (LuaValue::Number(x), LuaValue::Number(y), LuaValue::String(s)) =
                (&c[0], &c[1], &c[2])
            else {
                continue;
            };
            now.insert((*x as i32, *y as i32), s.to_string_lossy().into_owned());
        }
        let mut changed: Vec<(i32, i32)> = {
            let m = self.map_icons.borrow();
            now.iter()
                .filter(|(p, s)| m.yield_sigs.get(p).is_some_and(|old| old != *s))
                .map(|(p, _)| *p)
                .collect()
        };
        changed.sort_unstable();
        self.map_icons.borrow_mut().yield_sigs = now;
        for (x, y) in changed {
            self.yield_changed(x, y);
        }
    }

    /// A plot came out of the fog: its yield is shown when its set holds it (the Lua shows a revealed plot only, so
    /// the exe tells it again once the plot is revealed).
    pub fn yield_revealed(&self, x: i32, y: i32) {
        if self.map_icons.borrow().shown.contains(&(x, y)) {
            self.fire_events(
                "ShowHexYield",
                vec![Arg::from(x), Arg::from(y), Arg::Bool(true)],
            );
        }
    }

    /// The DLL's state of a plot (`GameplayPlotStateChange`): the resource icon of the plot is created, changed or
    /// destroyed.
    pub fn plot_resource(&self, x: i32, y: i32, resource: i32, improvement: i32) {
        let old = {
            let mut m = self.map_icons.borrow_mut();
            if resource >= 0 {
                m.resources.insert((x, y), (resource, improvement))
            } else {
                m.resources.remove(&(x, y))
            }
        };
        let (hx, hy) = crate::hex::grid_to_hex(x, y);
        if resource >= 0 {
            if old.map(|(r, _)| r) != Some(resource) {
                self.fire_events(
                    "SerialEventRawResourceIconCreated",
                    vec![
                        Arg::from(hx),
                        Arg::from(hy),
                        Arg::from(improvement),
                        Arg::from(resource),
                    ],
                );
            }
        } else if old.is_some() {
            self.fire_events(
                "SerialEventRawResourceIconDestroyed",
                vec![Arg::from(hx), Arg::from(hy)],
            );
        }
    }

    /// A new load of the game: the Lua of the icons is told every resource again.
    pub fn forget_plot_resources(&self) {
        let mut m = self.map_icons.borrow_mut();
        m.resources.clear();
        m.resources_dirty = true;
    }

    /// The resources the active team sees may have changed (`resources_dirty`).
    pub fn resources_changed(&self) {
        self.map_icons.borrow_mut().resources_dirty = true;
    }

    /// Once per dispatch after a change: the resources the active team sees, read through the DLL's Lua API, against
    /// the icons announced (the replica does not run the DLL's `CvMap::updateLayout`, so no `PLOT_STATE_CHANGE` comes).
    pub fn sync_resources(&self) {
        if !self.map_icons.borrow().resources_dirty || !self.setup_done.get() {
            return;
        }
        self.map_icons.borrow_mut().resources_dirty = false;
        let Some(ctx) = self.lua_ctx() else {
            return;
        };
        let r = ctx
            .lua()
            .load(RESOURCES_LUA)
            .set_name("=exe_resources")
            .eval::<(LuaValue, LuaValue)>();
        let (flat, built): (Vec<i32>, Vec<i32>) = match r {
            Ok((LuaValue::Table(t), b)) => (
                t.sequence_values::<i32>().unwrap_or_default(),
                match b {
                    LuaValue::Table(b) => b.sequence_values::<i32>().unwrap_or_default(),
                    _ => Vec::new(),
                },
            ),
            Ok(_) => return,
            Err(e) => {
                tracing::debug!(target: "exe.ui", "resources: {e}");
                return;
            }
        };
        self.sync_built(&built);
        let now: HashMap<(i32, i32), (i32, i32)> = flat
            .chunks_exact(4)
            .map(|c| ((c[0], c[1]), (c[2], c[3])))
            .collect();
        let gone: Vec<(i32, i32)> = self
            .map_icons
            .borrow()
            .resources
            .keys()
            .filter(|p| !now.contains_key(p))
            .copied()
            .collect();
        for (x, y) in gone {
            self.plot_resource(x, y, -1, -1);
        }
        let mut plots: Vec<_> = now.into_iter().collect();
        plots.sort_unstable();
        for ((x, y), (res, imp)) in plots {
            self.plot_resource(x, y, res, imp);
        }
    }

    /// SIMPLIFIED(SIMP-exe-951): the exe raises `SerialEventImprovementCreated(hexX, hexY, culture, continent, player,
    /// improvement, resource, era, state)` and `SerialEventRoadCreated(hexX, hexY, player, route)` from the DLL's plot
    /// change; the replica does not run that, so the improvement and road of every revealed plot are read with the
    /// resources and a change is told. The first read only records (a load is not a build). The era and the state of the
    /// improvement are 0 (the listeners of the shipped Lua read the first five arguments only).
    fn sync_built(&self, flat: &[i32]) {
        let now: HashMap<(i32, i32), (i32, i32)> = flat
            .chunks_exact(6)
            .map(|c| ((c[0], c[1]), (c[2], c[3])))
            .collect();
        let owners: HashMap<(i32, i32), (i32, i32)> = flat
            .chunks_exact(6)
            .map(|c| ((c[0], c[1]), (c[4], c[5])))
            .collect();
        let old = self.map_icons.borrow_mut().built.replace(now.clone());
        let Some(old) = old else {
            return;
        };
        let mut changed: Vec<_> = now
            .iter()
            .filter(|(p, v)| old.get(p) != Some(v))
            .map(|(p, v)| (*p, *v, old.get(p).copied().unwrap_or((-1, -1))))
            .collect();
        changed.sort_unstable();
        for ((x, y), (imp, route), (old_imp, old_route)) in changed {
            let (owner, res) = owners.get(&(x, y)).copied().unwrap_or((-1, -1));
            let (hx, hy) = crate::hex::grid_to_hex(x, y);
            if imp >= 0 && imp != old_imp {
                let culture = self
                    .entities
                    .borrow()
                    .culture
                    .get(&owner)
                    .copied()
                    .unwrap_or(-1);
                self.fire_events(
                    "SerialEventImprovementCreated",
                    vec![
                        Arg::from(hx),
                        Arg::from(hy),
                        Arg::from(culture),
                        Arg::from(self.continent(x, y)),
                        Arg::from(owner),
                        Arg::from(imp),
                        Arg::from(res),
                        Arg::from(0),
                        Arg::from(0),
                    ],
                );
            }
            if route >= 0 && route != old_route {
                self.fire_events(
                    "SerialEventRoadCreated",
                    vec![
                        Arg::from(hx),
                        Arg::from(hy),
                        Arg::from(owner),
                        Arg::from(route),
                    ],
                );
            }
        }
    }

    fn request_plots(&self, ty: i32, a: [i32; 3]) -> Vec<(i32, i32)> {
        if ty == USER_ALL_ON {
            // The plots of the camera's rectangles (the whole map until the host tells them).
            let rects = self.map_icons.borrow().rects.clone();
            let rects = match rects {
                Some(r) => r,
                None => {
                    let (w, h, _) = self.map_dims();
                    vec![[0, 0, w, h]]
                }
            };
            let mut out = Vec::new();
            for c in rects {
                for y in c[1]..c[3] {
                    for x in c[0]..c[2] {
                        out.push((x, y));
                    }
                }
            }
            return out;
        }
        let Some(ctx) = self.lua_ctx() else {
            return Vec::new();
        };
        let lua = ctx.lua();
        let r = lua
            .load(PLOTS_LUA)
            .set_name("=exe_yield_plots")
            .into_function()
            .and_then(|f| f.call::<_, LuaValue>((ty, a[0], a[1], a[2])));
        let flat: Vec<i32> = match r {
            Ok(LuaValue::Table(t)) => t.sequence_values::<i32>().unwrap_or_default(),
            Ok(_) => Vec::new(),
            Err(e) => {
                tracing::debug!(target: "exe.ui", "yield plots ({ty}): {e}");
                Vec::new()
            }
        };
        flat.chunks_exact(2).map(|c| (c[0], c[1])).collect()
    }
}
