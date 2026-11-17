-- Helpers shared by built-in data scripts. Scripts always receive the seat explicitly.
function array(values) return setmetatable(values or {}, { __screen_array = true }) end
function L(key, ...)
    -- Rows without a text column (no help, no title) read as empty text.
    if key == nil then return "" end
    if Locale and Locale.ConvertTextKey then return Locale.ConvertTextKey(key, ...) end
    error("server localization is unavailable")
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
