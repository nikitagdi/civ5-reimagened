local pending = {}
local function root(player,args)
    local e = pending[player]
    if not e or e.id ~= args.a then
        return {id=args.a or -1, kind="text", width="narrow", art_anchor="center",
            text="No matching message event is pending for this seat", button=L("TXT_KEY_OK"),
            facts=array(), available=false, resolved=false}
    end
    local r = {id=e.id, kind=e.kind, width=e.width or "medium", civ=e.civ, kicker=e.kicker,
        title=e.title, art=e.art, art_anchor=e.art_anchor or "center", icon=e.icon,
        band=e.band, quote=e.quote, text=e.text, facts_title=e.facts_title,
        facts=e.facts or array(), button=e.button or L("TXT_KEY_OK"), available=true, resolved=e.resolved or false}
    if e.kind == "wonder" then
        local building = GameInfo.Buildings[e.data1]
        assert(building, "Unknown wonder")
        r.title = L(building.Description)
        r.icon = "building:" .. building.Type
        r.art = r.icon
        r.quote = building.Quote and {text=L(building.Quote)} or nil
        r.band = {name=r.title, note=tostring(Game.GetGameTurn())}
    elseif e.kind == "new_era" then
        local era = GameInfo.Eras[e.data1]
        assert(era, "Unknown era")
        r.title = L(era.Description)
    elseif e.kind == "great_person" then
        local unit = GameInfo.Units[e.data1]
        assert(unit, "Unknown great person")
        r.title = L(unit.Description); r.art = "unit:" .. unit.Type
    elseif e.kind == "barbarian_camp" then
        r.facts = array({{key="gold", label=L("TXT_KEY_YIELD_GOLD"),
            yields=array({{yield="gold",value=e.data1 or 0}})}})
    end
    return r
end
return {
    -- Trusted queue ingress, including localized event text and reward facts which cannot be reconstructed later.
    enqueue = function(player,event)
        assert(Players[player], "Unknown seat")
        assert(not pending[player] or pending[player].resolved, "A message is already pending")
        assert(event.id and event.id >= 0 and event.kind, "Invalid message event")
        local copy={}; for k,v in pairs(event) do copy[k]=v end
        pending[player]=copy
    end,
    data = function(player,args)
        assert(Players[player], "Unknown seat")
        return root(player,args)
    end,
    actions = {ok=function(player,args)
        local e=pending[player]
        assert(e and e.id==args.a and not e.resolved, "This message is no longer pending")
        e.resolved=true
    end},
}
