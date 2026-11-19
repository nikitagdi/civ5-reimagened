-- Helpers shared by built-in data scripts. Scripts always receive the seat explicitly.
function array(values) return setmetatable(values or {}, { __screen_array = true }) end
function L(key, ...)
    -- Rows without a text column (no help, no title) read as empty text.
    if key == nil then return "" end
    if Locale and Locale.ConvertTextKey then return Locale.ConvertTextKey(key, ...) end
    error("server localization is unavailable")
end
-- A game text without its markup ([NEWLINE], [COLOR_*]/[ENDCOLOR], [ICON_*]): the skins show plain text.
function plain(text)
    return (tostring(text or ""):gsub("%[NEWLINE%]", " "):gsub("%[[^%]]*%]", ""):gsub("%s+", " "):gsub("^ ", ""):gsub(" $", ""))
end
function icon(atlas, index)
    if not atlas or not index or index < 0 then return "" end
    return "icon:" .. atlas .. ":" .. index
end
function key_city(owner, id) return owner .. ":" .. id end
function key_unit(owner, id) return owner .. ":" .. id end
function key_item(order, item) return order .. ":" .. item end
function team_of(player) return Teams[Players[player]:GetTeam()] end
function met(player, other) return team_of(player):IsHasMet(Players[other]:GetTeam()) end
function visible(plot, team) return plot:IsVisible(team, false) end
-- The worker runs every screen query and action through this: with the real core, GcUiReadScope puts back the synced
-- state that Lua getters change on the asking machine only (the game RNG, the might caches), so a screen cannot change
-- the simulation. Without it (the stub, tests) the function is called directly.
-- Where a seat's unit stands, for the worker's stale move questions: true, x, y; true when the unit is gone; false
-- without the core's API (the stub).
function __unit_plot(player, id)
    local players = rawget(_G, "Players")
    if not players then return false end
    local p = players[player]
    local u = p and p:GetUnitByID(id)
    if not u or u:IsDead() or u:IsDelayedDeath() then return true end
    return true, u:GetX(), u:GetY()
end
function __screen_call(f, ...)
    local scope = rawget(_G, "GcUiReadScope")
    if scope then return scope(f, ...) end
    return f(...)
end
