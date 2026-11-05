//! `UI.*`: the interface library of the exe (`cvLuaInterfaceBuddy`), for K1-K3.
//!
//! The game part of the interface (selection, interface mode, the city screen, what the DLL reads through
//! pointers) lives in the replica and is asked with `GameSide::ui_op`; the rest is the client's (mouse, keys,
//! popups, camera). The Lua objects are written as a short Lua glue over a few Rust primitives (`__exe_*`): the
//! glue turns `pUnit`/`pCity`/`pPlot` objects into ids and back through the API the DLL registered, so no
//! gamecore method is written here.
//!
//! SIMPLIFIED(SIMP-exe-36): the UI functions outside K1-K3 (deals, advanced start, the leader head, the path
//! finder) are stubs: they count their calls (`exe.simp_calls`, `UI.<name>`) and answer nothing / false.
//! SIMPLIFIED(SIMP-exe-361): the scratch deal's Get* methods answer 0 and its Is*/Can*/Has* methods false (the
//! overviews of M4 WO-F read it before the trade window brings the real deal).

use std::rc::Rc;

use lua_sys::{Lua, LuaError, LuaValue, MultiValue, Result, Table};

use crate::UiOp;
use crate::buddy::PopupInfo;
use crate::core::{Arg, Core, Family, Fire, Payload, pack_lua};
use crate::registry::Status;

/// The interface operations by the name the Lua glue uses (`op("SelectUnit", ..)`); the real `UiOp` has no names.
pub const UI_OPS: [(&str, UiOp); 28] = [
    ("SelectUnit", UiOp::SelectUnit),
    ("SelectGroup", UiOp::SelectGroup),
    ("HeadSelectedUnit", UiOp::HeadSelectedUnit),
    ("SelectedUnits", UiOp::SelectedUnits),
    ("ClearSelection", UiOp::ClearSelection),
    ("LastSelectedUnit", UiOp::LastSelectedUnit),
    ("SelectCity", UiOp::SelectCity),
    ("HeadSelectedCity", UiOp::HeadSelectedCity),
    ("ClearSelectedCities", UiOp::ClearSelectedCities),
    ("DoSelectCityAtPlot", UiOp::DoSelectCityAtPlot),
    ("SetInterfaceMode", UiOp::SetInterfaceMode),
    ("GetInterfaceMode", UiOp::GetInterfaceMode),
    ("SetInterfaceModeValue", UiOp::SetInterfaceModeValue),
    ("GetInterfaceModeValue", UiOp::GetInterfaceModeValue),
    ("CanDoInterfaceMode", UiOp::CanDoInterfaceMode),
    ("SetCityScreenUp", UiOp::SetCityScreenUp),
    ("IsCityScreenUp", UiOp::IsCityScreenUp),
    ("CanSelectionListFound", UiOp::CanSelectionListFound),
    ("CanSelectionListWork", UiOp::CanSelectionListWork),
    ("CanEndTurn", UiOp::CanEndTurn),
    ("ActivateNotification", UiOp::ActivateNotification),
    ("RemoveNotification", UiOp::RemoveNotification),
    ("GetCycleSelectionCounter", UiOp::GetCycleSelectionCounter),
    ("SetCycleSelectionCounter", UiOp::SetCycleSelectionCounter),
    ("IsUnitSelected", UiOp::IsUnitSelected),
    ("IsCitySelected", UiOp::IsCitySelected),
    ("SelectAllAtPlot", UiOp::SelectAllAtPlot),
    ("GetSelectionCount", UiOp::GetSelectionCount),
];

/// The operation of a glue name.
pub fn op_by_name(name: &str) -> Option<UiOp> {
    UI_OPS.iter().find(|(n, _)| *n == name).map(|(_, op)| *op)
}

/// The stubs of `UI` outside K1-K3 (SIMP-exe-36): name and the Lua literal they answer. `GetScratchDeal` answers
/// an object whose every method is a counted no-op.
pub const UI_STUBS: &[(&str, &str)] = &[
    ("LoadCurrentDeal", "nil"),
    ("LoadHistoricDeal", "nil"),
    ("ChangeStartDiploRepeatCount", "nil"),
    ("SetRepeatActionPlayer", "nil"),
    ("HighlightCanPlacePlots", "nil"),
    ("ClearPlaceUnit", "nil"),
    ("SetPlaceUnit", "nil"),
    ("GetNumCurrentDeals", "0"),
    ("GetNumHistoricDeals", "0"),
    ("CanPlaceUnitAt", "false"),
    ("GetPlaceUnit", "-1"),
    ("GetScratchDeal", "scratch"),
    ("DebugFlag", "nil"),
];

const GLUE: &str = r#"
local op, simp = __exe_op, __exe_simp
local UI = {}
local state = { leader = false, city_view = false, sem = 0, goto_x = -1, goto_y = -1, place_unit = -1 }
local function xy(plot) return plot:GetX(), plot:GetY() end
local function unit_of(o, id)
	if o and o >= 0 then local p = Players[o]; return p and p:GetUnitByID(id) end
	return nil
end
local function city_of(o, id)
	if o and o >= 0 then local p = Players[o]; return p and p:GetCityByID(id) end
	return nil
end

-- selection and interface mode (the replica keeps them)
function UI.GetHeadSelectedUnit() return (unit_of(op("HeadSelectedUnit"))) end
-- SIMPLIFIED(SIMP-exe-332): the last selected unit is the head selected one (CityView.lua:2342 asks on leaving the city screen).
function UI.GetLastSelectedUnit() return UI.GetHeadSelectedUnit() end
function UI.GetHeadSelectedCity() return (city_of(op("HeadSelectedCity"))) end
function UI.GetSelectedUnitID() local o, id = op("HeadSelectedUnit"); if o and o >= 0 then return id end return -1 end
function UI.SelectUnit(u) op("SelectUnit", u:GetOwner(), u:GetID()) end
function UI.SelectCity(c) op("SelectCity", c:GetOwner(), c:GetID()) end
function UI.ClearSelectedCities() op("ClearSelectedCities") end
function UI.ClearSelectionList() op("ClearSelection") end
-- The exe's DoSelectCityAtPlot opens the city screen of an own city and the camera goes to it. A city of a city-state
-- opens its diplomacy popup (live report 2026-10-30: the banner click of EUI's CityBannerManager.OnBannerClick calls
-- only this for a met city-state, and the replica selects own cities only).
-- SIMPLIFIED(SIMP-exe-543): the popup is the one the notification panel raises (BUTTONPOPUP_CITY_STATE_DIPLO, Data1 = the
-- city-state's player id); the exe's own conditions for it (met team, turn state) are not replicated, the callers check them.
function UI.DoSelectCityAtPlot(p)
	if op("DoSelectCityAtPlot", xy(p)) == 1 then
		__exe_look_at(xy(p))
		op("SetCityScreenUp", 1)
		return
	end
	local city = p.GetPlotCity and p:GetPlotCity()
	local owner = city and city:GetOwner()
	local player = owner and Players and Players[owner]
	if player and owner ~= Game.GetActivePlayer() and player:IsMinorCiv() then
		Events.SerialEventGameMessagePopup{ Type = ButtonPopupTypes.BUTTONPOPUP_CITY_STATE_DIPLO, Data1 = owner }
	end
end
function UI.CanSelectionListFound() return op("CanSelectionListFound") == 1 end
function UI.CanSelectionListWork(b) return op("CanSelectionListWork", b or 0) == 1 end
function UI.CanEndTurn() return op("CanEndTurn") == 1 end
-- A click on a plot of the map (InGame.lua ClickSelect, in the selection mode): the exe selects an own unit there; a
-- second click on the plot of the selected unit selects the next own unit there; a plot with no own unit but an own
-- city opens the city. SIMPLIFIED(SIMP-exe-562): the exe's choice among the units of the plot (its order, cargo, the
-- modifiers that select groups) is not read; the first own unit the plot lists is taken, the modifiers do nothing.
function UI.LocationSelect(plot, ctrl, alt, shift)
	if not plot then return end
	local me = Game.GetActivePlayer()
	local own = {}
	for i = 0, plot:GetNumUnits() - 1 do
		local u = plot:GetUnit(i)
		if u and u:GetOwner() == me then own[#own + 1] = u end
	end
	if #own > 0 then
		local pick = own[1]
		local ho, hid = op("HeadSelectedUnit")
		for i, u in ipairs(own) do
			if ho == me and u:GetID() == hid then pick = own[i % #own + 1] break end
		end
		UI.SelectUnit(pick)
		return
	end
	local c = plot:GetPlotCity()
	if c and c:GetOwner() == me then UI.DoSelectCityAtPlot(plot) end
end
function UI.SetInterfaceMode(m, v) op("SetInterfaceMode", m); if v then op("SetInterfaceModeValue", v) end end
function UI.GetInterfaceMode() return (op("GetInterfaceMode")) or 0 end
function UI.CanDoInterfaceMode(m) return op("CanDoInterfaceMode", m) == 1 end
function UI.GetInterfaceModeValue() return (op("GetInterfaceModeValue")) or 0 end
function UI.GetInterfaceModeDebugItemID1() return (op("GetInterfaceModeValue")) or 0 end
function UI.GetInterfaceModeDebugItemID2() return 0 end
function UI.IsCityScreenUp() return op("IsCityScreenUp") == 1 end
-- The exe's city screen takes the camera to the city it shows, whichever way it was opened (a banner, the "choose
-- production" block of END TURN, a popup): the head selected city is where the camera goes (M5 WO-L).
function UI.SetCityScreenUp(v)
	op("SetCityScreenUp", v and 1 or 0)
	if v then
		local c = UI.GetHeadSelectedCity()
		local p = c and c:Plot()
		if p then __exe_look_at(xy(p)) end
	end
end
function UI.SetCityScreenViewingMode(v) state.city_view = v and true or false end
function UI.IsCityScreenViewingMode() return state.city_view end

-- notifications
function UI.ActivateNotification(id) op("ActivateNotification", id) end
function UI.RemoveNotification(id) op("RemoveNotification", id) end
function UI.RebroadcastNotifications() __exe_rebroadcast() end

-- camera and mouse (the client's)
-- SIMPLIFIED(SIMP-exe-64): a plot a script kept across a turn's load is an object of the previous game (InGame.lua's
-- `lastCityEnteredPlot`, looked at when the city screen closes): the camera stays where it is instead of an error.
function UI.LookAt(plot)
	if not plot then return end
	local ok, x, y = pcall(xy, plot)
	if ok then __exe_look_at(x, y) end
end
-- The exe's InterfaceBuddy::lookAtSelectionPlot looks at the plot of the selected unit, else of the selected city (EUI's
-- CityView.lua selects the next city that needs production with UI.SelectCity and then calls this; live report
-- 2026-10-31: the camera stayed where it was).
-- A non-zero argument only releases a locked camera (the exe's lLookAtSelectionPlot): no move. While the city screen is up
-- the city is the selection plot (SIMP-exe-780).
function UI.LookAtSelectionPlot(release)
	if release and release ~= 0 then return end
	local u = UI.GetHeadSelectedUnit()
	local c = UI.GetHeadSelectedCity()
	if u and c and not UI.IsCityScreenUp() then c = nil end
	local p = c and c:Plot() or u and u:GetPlot()
	if p then __exe_look_at(xy(p)) end
end
function UI.IsCameraMoving() return __exe_camera_moving() end
function UI.GetMouseOverHex() return __exe_mouse_hex() end
-- The interface path finder: the head selected unit's path to the plot under the mouse comes back as Events.UIPathFinderUpdate.
function UI.SendPathfinderUpdate()
	local u = UI.GetHeadSelectedUnit()
	if u then __exe_pathfinder_update(u:GetOwner(), u:GetID()) else __exe_pathfinder_update(-1, -1) end
end
function UI.CtrlKeyDown() return __exe_mod(1) end
function UI.ShiftKeyDown() return __exe_mod(2) end
function UI.AltKeyDown() return __exe_mod(3) end
function UI.IsTouchScreenEnabled() return false end
function UI.HasShownLegal() return true end
function UI.ToggleGridVisibleMode() __exe_toggle_grid() end
function UI.SetGotoPlot(p) if p then state.goto_x, state.goto_y = xy(p) else state.goto_x, state.goto_y = -1, -1 end end
function UI.GetGotoPlot() if state.goto_x >= 0 then return Map.GetPlot(state.goto_x, state.goto_y) end return nil end

-- popups (InterfaceBuddy)
function UI.IsPopupUp() return __exe_popup_up() end
function UI.IsPopupTypeOpen(t) return __exe_popup_open(t) end
function UI.AddPopup(info) __exe_popup_add(info) end

-- dirty bits and the turn timer
function UI.SetDirty(bit, v) __exe_dirty(bit, v ~= false) end
-- CityView.lua's tile purchase calls this before it asks for the purchase sound: without it the call raised and the
-- sound never came (live report 2026-10-30). The exe redraws the city screen (CityScreen_DIRTY_BIT).
function UI.UpdateCityScreen() __exe_dirty(21, true) end
function UI.incTurnTimerSemaphore() state.sem = state.sem + 1 end
function UI.decTurnTimerSemaphore() state.sem = state.sem - 1 end
function UI.interruptTurnTimer() end
function UI.WaitingForRemotePlayers() return false end
function UI.GetLeaderHeadRootUp() return state.leader end
-- While the leader screen is up the engine shows nothing of the HUD (the notification panel's civ list moves into the leader
-- context for that: EUI `g_LeaderPopups`): the InGame context is hidden and comes back with it (M5 WO-I, golden S5).
function UI.SetLeaderHeadRootUp(v)
	v = v and true or false
	if v == state.leader then return end
	state.leader = v
	local ok, c = pcall(LookUpControl, "/InGame")
	if ok and c then c:SetHide(v) end
end

-- icons from the database
local function unit_type(x)
	if type(x) == "number" then return x end
	if x ~= nil and type(x) ~= "string" and x.GetUnitType then return x:GetUnitType() end
	return x
end
function UI.GetUnitPortraitIcon(unitType)
	local t = unit_type(unitType)
	local u = t ~= nil and GameInfo.Units[t]
	if u then return u.PortraitIndex, u.IconAtlas end
	return nil
end
function UI.GetUnitFlagIcon(unitType)
	local t = unit_type(unitType)
	local u = t ~= nil and GameInfo.Units[t]
	if u then return u.UnitFlagIconOffset, u.UnitFlagAtlas end
	return nil
end

-- InterfaceBuddy::IsAIRequestingConcessions (0x086b1c0a) returns one byte of the interface (+0x1ad), cleared by resetState and
-- by CvDealAI::DoTradeScreenClosed. Only the DLL can write it (there is no Lua setter), and the DLL source (Lekmod, as the
-- SDK) calls SetAIRequestingConcessions(false) only: the byte is false for the whole game, so that is the answer.
-- SIMPLIFIED(SIMP-exe-1011): no interface operation carries the byte if a DLL ever raised it.
function UI.IsAIRequestingConcessions() return false end
-- stubs (SIMP-exe-36), generated from UI_STUBS
local function stub(name, ret) UI[name] = function() simp("UI." .. name) return ret end end
--STUBS--
-- The trade screen (K12): the GcUi* globals are the replica's (rt_ui_deal.cpp), set when the game's Lua libraries are attached.
local scratch = setmetatable({}, { __index = function(t, k)
	-- M4 WO-F (SIMP-exe-361): the overviews read the scratch deal's counts and limits (DiploRelationships prints
	-- GetGoldAvailable): Get* answers 0 (GetNextItem nothing: the item loops end), Is*/Can*/Has* false.
	local name = tostring(k)
	local answer = nil
	if name == "GetNextItem" then answer = nil elseif name:find("^Get") then answer = 0 elseif name:find("^Is") or name:find("^Can") or name:find("^Has") then answer = false end
	local f = function() simp("UI.ScratchDeal." .. name) return answer end
	rawset(t, k, f)
	return f
end })
function UI.GetScratchDeal() if GcUiGetScratchDeal then return GcUiGetScratchDeal() end return scratch end
function UI.DoProposeDeal() if GcUiProposeDeal then return GcUiProposeDeal() end end
function UI.DoDemand() if GcUiDemand then return GcUiDemand() end end
-- The three "what do you want / what will you give / what will make this work" buttons. The replica changes the scratch
-- deal (GcUiEqualize) but says nothing, and the screen redraws its table only on a leader message of the trade state
-- (TradeLogic LeaderMessageHandler: ClearTable, DisplayDeal, DoUpdateButtons): the buttons did nothing. The exe answers
-- with the leader's line of the outcome; the text is the leader's Diplomacy_Responses row (as CvGame::GetDiploResponse).
-- SIMPLIFIED(SIMP-exe-481): ok is the only outcome the core gives, so "cannot match" reads as "no deal possible", and
-- "changed" is told from the item list; the exe's own body was not reverse-engineered.
local function deal_signature(d)
	local parts = { tostring(d:GetNumItems()) }
	pcall(function()
		d:ResetIterator()
		for _ = 1, 64 do
			local item = { d:GetNextItem() }
			if #item < 1 then break end
			for i = 1, #item do parts[#parts + 1] = tostring(item[i]) end
		end
	end)
	return table.concat(parts, ",")
end
local function diplo_response_text(them, tag)
	local ok, text = pcall(function()
		local leader = GameInfo.Leaders[Players[them]:GetLeaderType()]
		local rows, total = {}, 0
		for row in GameInfo.Diplomacy_Responses() do
			if row.LeaderType == leader.Type and row.ResponseType == tag then
				local bias = tonumber(row.Bias) or 1
				if bias > 0 then rows[#rows + 1] = { row.Response, bias } total = total + bias end
			end
		end
		if total == 0 then return nil end
		local pick = math.random(total)
		for _, r in ipairs(rows) do
			if pick <= r[2] then return Locale.ConvertTextKey(r[1]) end
			pick = pick - r[2]
		end
	end)
	return ok and text or nil
end
local function equalize(mode)
	if not GcUiEqualize then return end
	local d = UI.GetScratchDeal()
	local them = d:GetToPlayer()
	local before = deal_signature(d)
	local ok = GcUiEqualize(mode)
	local tag = "RESPONSE_TRADE_NO_DEAL_POSSIBLE"
	if ok then tag = deal_signature(d) == before and "RESPONSE_TRADE_DEAL_UNCHANGED" or "RESPONSE_TRADE_AI_MAKES_OFFER" end
	local state = DiploUIStateTypes and DiploUIStateTypes.DIPLO_UI_STATE_TRADE
	local text = diplo_response_text(them, tag)
	if text and state and them and them >= 0 then
		local anim = LeaderheadAnimationTypes and LeaderheadAnimationTypes.LEADERHEAD_ANIM_LETS_HEAR_IT or 0
		Events.AILeaderMessage(them, state, text, anim, 0)
	elseif GcUiOpenedTradeScreen and them and them >= 0 then
		GcUiOpenedTradeScreen(them)
	end
	return ok
end
function UI.DoEqualizeDealWithHuman() return equalize(0) end
function UI.DoWhatDoesAIWant() return equalize(1) end
function UI.DoWhatWillAIGive() return equalize(2) end
function UI.ProposedDealExists(a, b) return GcUiProposedDealExists ~= nil and GcUiProposedDealExists(a, b) or false end
-- The text the core left for the next popup (SetTempString: the city-state message of its greeting popup).
function UI.GetTempString() return __exe_temp_string or "" end
function UI.LoadProposedDeal(a, b) if GcUiLoadProposedDeal then GcUiLoadProposedDeal(a, b) end end
-- The running and the ended deals of a player (the Diplomatic Overview's two lists, the top panel's resource and gold-per-turn
-- lines, the war declaration popup): the replica's CvGameDeals through the scratch deal (rt_ui_deal.cpp). Without a replica
-- they stay the counted stubs (no deals).
local stub_num_current, stub_load_current = UI.GetNumCurrentDeals, UI.LoadCurrentDeal
local stub_num_historic, stub_load_historic = UI.GetNumHistoricDeals, UI.LoadHistoricDeal
function UI.GetNumCurrentDeals(p)
	if GcUiGetNumCurrentDeals then return GcUiGetNumCurrentDeals(p) end
	return stub_num_current(p)
end
function UI.LoadCurrentDeal(p, i)
	if GcUiLoadCurrentDeal then return GcUiLoadCurrentDeal(p, i) end
	return stub_load_current(p, i)
end
function UI.GetNumHistoricDeals(p)
	if GcUiGetNumHistoricDeals then return GcUiGetNumHistoricDeals(p) end
	return stub_num_historic(p)
end
function UI.LoadHistoricDeal(p, i)
	if GcUiLoadHistoricDeal then return GcUiLoadHistoricDeal(p, i) end
	return stub_load_historic(p, i)
end
-- Deals between two humans (fix-mp-trade): UI.DoProposeDeal to a human sends the proposal, the other side answers with
-- DoFinalizePlayerDeal (accept / refuse) and the proposer withdraws with it; the replica's GcUi* send the net messages.
-- Without a replica they stay the counted stubs.
-- Before the replica attaches (and in tests) the two answer nothing / -1 and count the call.
local function stub_finalize() simp("UI.DoFinalizePlayerDeal") return nil end
local function stub_has_made() simp("UI.HasMadeProposal") return -1 end
-- SIMPLIFIED(SIMP-exe-902): An answer given before the replica attached its GcUi* (a reload: the Lua libraries attach late) is not lost: it waits
-- (the newest per pair of players) and goes out with the first call after the attach.
local pending_finalize = {}
local function replay_finalize()
	if not GcUiFinalizePlayerDeal or #pending_finalize == 0 then return end
	local list = pending_finalize
	pending_finalize = {}
	for _, a in ipairs(list) do GcUiFinalizePlayerDeal(a[1], a[2], a[3]) end
end
function UI.DoFinalizePlayerDeal(from, to, accepted)
	if GcUiFinalizePlayerDeal then
		replay_finalize()
		return GcUiFinalizePlayerDeal(from, to, accepted)
	end
	for i = #pending_finalize, 1, -1 do
		if pending_finalize[i][1] == from and pending_finalize[i][2] == to then table.remove(pending_finalize, i) end
	end
	if #pending_finalize < 8 then pending_finalize[#pending_finalize + 1] = { from, to, accepted } end
	return stub_finalize(from, to, accepted)
end
function UI.HasMadeProposal(p)
	if GcUiHasMadeProposal then replay_finalize() return GcUiHasMadeProposal(p) end
	return stub_has_made(p)
end
-- The exe leaves the leader view mode and raises LeavingLeaderViewMode, which closes every leader screen (the leader root,
-- the discussion dialog, the trade screen: their listeners DequeuePopup / hide). Without it the trade screen's Goodbye
-- (TradeLogic OnBack) only closed itself and the leader's diplomacy screen below it came back instead of the map.
-- SIMPLIFIED(SIMP-exe-950): the leader root flag is cleared too (the exe's own order was not recovered) and the event is
-- raised at once, not queued.
function UI.RequestLeaveLeader()
	UI.SetLeaderHeadRootUp(false)
	if Events and Events.LeavingLeaderViewMode then Events.LeavingLeaderViewMode() end
end
function UI.OnHumanOpenedTradeScreen(p) replay_finalize() if GcUiOpenedTradeScreen then GcUiOpenedTradeScreen(p) end end
function UI.OnHumanDemand(p) if GcUiHumanDemand then GcUiHumanDemand(p) end end
return UI
"#;

fn int_args(args: &[LuaValue<'_>]) -> Vec<i32> {
    args.iter()
        .map(|a| match a {
            LuaValue::Number(n) => *n as i32,
            LuaValue::Boolean(b) => i32::from(*b),
            _ => 0,
        })
        .collect()
}

/// A Lua `popupInfo` table to a popup: the numbers, booleans and strings of its fields.
pub fn popup_from_table(t: &Table<'_>) -> PopupInfo {
    let mut fields = Vec::new();
    let mut ty = -1;
    for pair in t.pairs::<LuaValue, LuaValue>() {
        let Ok((LuaValue::String(k), v)) = pair else {
            continue;
        };
        let k = k.to_string_lossy().into_owned();
        let arg = match v {
            LuaValue::Number(n) => Arg::Num(n),
            LuaValue::Boolean(b) => Arg::Bool(b),
            LuaValue::String(s) => Arg::Str(s.to_string_lossy().into_owned()),
            _ => continue,
        };
        if k == "Type"
            && let Arg::Num(n) = arg
        {
            ty = n as i32;
        }
        fields.push((k, arg));
    }
    // Stable order for the listeners and the tests.
    fields.sort_by(|a, b| a.0.cmp(&b.0));
    PopupInfo { ty, fields }
}

/// Nodes of a path the exe keeps (`PathFinderData`: an int16 count and 64 `{x, y, turn}`).
pub const PATH_NODES_MAX: usize = 64;

/// The path cut at [`PATH_NODES_MAX`] nodes, the way the exe copies it into the event.
pub fn cap_path(mut nodes: Vec<crate::PathNode>) -> Vec<crate::PathNode> {
    nodes.truncate(PATH_NODES_MAX);
    nodes
}

/// `UI.SendPathfinderUpdate` for `owner`'s unit (`-1`: none selected): the path to the plot under the mouse is kept
/// for the drawing (`ExeApi::movement_path`) and goes to the Lua as `Events.UIPathFinderUpdate({ {x, y, turn}, .. })`,
/// the unit's plot first, `turn` 1 for this turn (PathHelpManager.lua puts a numbered marker where the turn changes).
/// A target the unit cannot reach gives the Lua an empty path (the exe's count 0) and the drawing the one node of the target
/// with the unreachable flag (the exe draws its "inaccessible" marker at the hovered plot).
fn pathfinder_update(core: &Rc<Core>, lua: &Lua, owner: i32, unit: i32) -> Result<()> {
    // SIMPLIFIED(SIMP-exe-920): an air unit gets no path (`DOMAIN_AIR`: the exe does not ask the core, count 0), and a
    // path is cut at 64 nodes (the event's array, `PathFinderData`); the cut path is what the exe draws too.
    let nodes = if owner < 0 || core.unit_is_air(lua, owner, unit) {
        Vec::new()
    } else {
        let (x, y) = core.ui.borrow().mouse_hex;
        let r = core.game().map(|g| {
            g.try_borrow_mut()
                .map_err(|_| "the game is busy".to_owned())
                .and_then(|mut g| g.unit_path(owner, unit, x, y))
        });
        match r {
            Some(Ok(n)) => n,
            None => Vec::new(),
            Some(Err(e)) => {
                tracing::debug!(target: "exe.ui", "SendPathfinderUpdate: {e}");
                Vec::new()
            }
        }
    };
    let nodes = cap_path(nodes);
    let data = lua.create_table();
    let reachable = nodes.iter().all(|n| !n.flags.unreachable());
    for (i, n) in nodes.iter().filter(|_| reachable).enumerate() {
        let t = lua.create_table();
        t.raw_set("x", i32::from(n.x))?;
        t.raw_set("y", i32::from(n.y))?;
        t.raw_set("turn", i32::from(n.turn) + 1)?;
        data.raw_set(i + 1, t)?;
    }
    core.ui.borrow_mut().path = nodes;
    core.fire(Fire {
        family: Family::Events,
        name: "UIPathFinderUpdate".to_owned(),
        payload: Payload::Lua(pack_lua(lua, &[LuaValue::Table(data)])?),
        hook: None,
    });
    Ok(())
}

pub fn install(core: &Rc<Core>, lua: &Lua, g: &Table<'_>) -> Result<()> {
    let weak = Rc::downgrade(core);
    macro_rules! up {
        ($w:expr) => {
            $w.upgrade()
                .ok_or_else(|| LuaError::msg("the exe API is gone"))?
        };
    }

    {
        let w = weak.clone();
        g.raw_set(
            "__exe_popup_dest",
            lua.create_function(move |_, (x, y): (i32, i32)| {
                up!(w).ui.borrow_mut().popup_dest = Some((x, y));
                Ok(())
            }),
        )?;
    }
    {
        let w = weak.clone();
        g.raw_set(
            "__exe_simp",
            lua.create_function(move |_, name: String| {
                up!(w).simp_call(&name);
                Ok(())
            }),
        )?;
    }
    {
        let w = weak.clone();
        g.raw_set(
            "__exe_op",
            lua.create_function(move |_, args: MultiValue| {
                let core = up!(w);
                let v = args.into_vec();
                let Some(LuaValue::String(name)) = v.first() else {
                    return Err(LuaError::msg("__exe_op: an operation name is expected"));
                };
                let name = name.to_string_lossy().into_owned();
                let op = op_by_name(&name)
                    .ok_or_else(|| LuaError::msg(format!("unknown interface operation {name}")))?;
                let out = core.op(op, &int_args(&v[1..]));
                Ok(MultiValue::from_vec(
                    out.into_iter()
                        .map(|n| LuaValue::Number(f64::from(n)))
                        .collect(),
                ))
            }),
        )?;
    }
    {
        let w = weak.clone();
        g.raw_set(
            "__exe_setup_done",
            lua.create_function(move |_, ()| Ok(up!(w).setup_done.get())),
        )?;
    }
    {
        // SIMPLIFIED(SIMP-exe-424): a client-only hook of the golden scenario, not a function of the exe.
        let w = weak.clone();
        g.raw_set(
            "__exe_camera_place",
            lua.create_function(move |_, (x, y, px, py, level): (i32, i32, f32, f32, f32)| {
                up!(w).host.camera_place(x, y, px, py, level);
                Ok(())
            }),
        )?;
    }
    {
        let w = weak.clone();
        g.raw_set(
            "__exe_look_at",
            lua.create_function(move |_, (x, y): (i32, i32)| {
                up!(w).host.look_at(x, y);
                Ok(())
            }),
        )?;
    }
    {
        let w = weak.clone();
        g.raw_set(
            "__exe_camera_moving",
            lua.create_function(move |_, _: MultiValue| Ok(up!(w).camera_moving.get())),
        )?;
    }
    {
        let w = weak.clone();
        g.raw_set(
            "__exe_mouse_hex",
            lua.create_function(move |_, _: MultiValue| {
                let core = up!(w);
                let (x, y) = core.ui.borrow().mouse_hex;
                Ok((x, y))
            }),
        )?;
    }
    {
        let w = weak.clone();
        g.raw_set(
            "__exe_pathfinder_update",
            lua.create_function(move |lua, (owner, unit): (i32, i32)| {
                let core = up!(w);
                pathfinder_update(&core, lua, owner, unit)
            }),
        )?;
    }
    {
        let w = weak.clone();
        g.raw_set(
            "__exe_mod",
            lua.create_function(move |_, which: i32| {
                let core = up!(w);
                let ui = core.ui.borrow();
                Ok(match which {
                    1 => ui.ctrl,
                    2 => ui.shift,
                    _ => ui.alt,
                })
            }),
        )?;
    }
    {
        let w = weak.clone();
        g.raw_set(
            "__exe_popup_up",
            lua.create_function(move |_, _: MultiValue| Ok(up!(w).buddy.borrow().is_popup_up())),
        )?;
    }
    {
        let w = weak.clone();
        g.raw_set(
            "__exe_popup_open",
            lua.create_function(move |_, ty: i32| Ok(up!(w).buddy.borrow().is_type_open(ty))),
        )?;
    }
    {
        let w = weak.clone();
        g.raw_set(
            "__exe_popup_add",
            lua.create_function(move |_, t: Table| {
                let core = up!(w);
                core.add_popup(popup_from_table(&t));
                Ok(())
            }),
        )?;
    }
    {
        let w = weak.clone();
        g.raw_set(
            "__exe_dirty",
            lua.create_function(move |_, (bit, on): (i32, bool)| {
                if on {
                    up!(w).dirty.borrow_mut().insert(bit);
                }
                Ok(())
            }),
        )?;
    }
    {
        let w = weak.clone();
        g.raw_set(
            "__exe_rebroadcast",
            lua.create_function(move |_, _: MultiValue| {
                let core = up!(w);
                let notes: Vec<_> = core.notes.borrow().values().cloned().collect();
                for n in notes {
                    core.fire_note(&n);
                }
                Ok(())
            }),
        )?;
    }
    {
        let w = weak;
        g.raw_set(
            "__exe_toggle_grid",
            lua.create_function(move |_, _: MultiValue| {
                let core = up!(w);
                let on = {
                    let mut ui = core.ui.borrow_mut();
                    ui.grid_on = !ui.grid_on;
                    ui.grid_on
                };
                core.host.set_grid_visible(on);
                core.fire_events(
                    if on {
                        "SerialEventHexGridOn"
                    } else {
                        "SerialEventHexGridOff"
                    },
                    vec![],
                );
                Ok(())
            }),
        )?;
    }
    let mut stubs = String::new();
    for (name, ret) in UI_STUBS.iter().filter(|(n, _)| *n != "GetScratchDeal") {
        stubs.push_str(&format!("stub(\"{name}\", {ret})\n"));
    }
    let glue = GLUE.replace("--STUBS--", &stubs);
    let ui: Table = lua
        .load(&glue)
        .set_name("=exe_ui")
        .set_environment(g.clone())
        .eval()?;
    for pair in ui.pairs::<String, LuaValue>() {
        let (name, _) = pair?;
        let status = if UI_STUBS.iter().any(|(n, _)| *n == name) {
            Status::Simp("SIMP-exe-36")
        } else {
            Status::Real
        };
        core.inner
            .registry
            .borrow_mut()
            .add(format!("UI.{name}"), status);
    }
    {
        let mut reg = core.inner.registry.borrow_mut();
        reg.add("UI", Status::Real);
        // `UI.debugItem1` is read once by WorldView and is nil in the exe (a typo of the original); no field is made.
        reg.add("UI.debugItem1", Status::Simp("SIMP-exe-36"));
    }
    g.raw_set("UI", ui)?;
    Ok(())
}
