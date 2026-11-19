local kinds = {BUTTONPOPUP_TEXT="text", BUTTONPOPUP_NEW_ERA="new_era",
    BUTTONPOPUP_WONDER_COMPLETED_ACTIVE_PLAYER="wonder", BUTTONPOPUP_GREAT_PERSON_REWARD="great_person",
    BUTTONPOPUP_BARBARIAN_CAMP_REWARD="barbarian_camp", BUTTONPOPUP_GOODY_HUT_REWARD="goody",
    BUTTONPOPUP_NATURAL_WONDER_REWARD="natural_wonder", BUTTONPOPUP_GOLDEN_AGE_REWARD="golden_age",
    BUTTONPOPUP_DEAL_CANCELED="text"}
local function context(args)
    local p=args.popup
    if not p or p.id~=args.a or not kinds[p.kind] then return nil end
    return {id=p.id,kind=kinds[p.kind],text=p.text,data1=p.data1}
end
local function root(player,args)
    local e = context(args)
    if not e or e.id ~= args.a then
        return {id=args.a or -1, kind="text", width="narrow", art_anchor="center",
            text="No matching message event is pending for this seat", button="OK",
            facts=array(), available=false, resolved=false}
    end
    local r = {id=e.id, kind=e.kind, width=e.width or "medium", civ=e.civ, kicker=e.kicker,
        title=e.title, art=e.art, art_anchor=e.art_anchor or "center", icon=e.icon,
        band=e.band, quote=e.quote, text=e.text and e.text:find("%S") and e.text or nil, facts_title=e.facts_title,
        facts=e.facts or array(), button=e.button or "OK", available=true, resolved=e.resolved or false}
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
    data = function(player,args)
        assert(Players[player], "Unknown seat")
        return root(player,args)
    end,
    actions = {ok=function(player,args,net)
        assert(context(args), "This message is no longer pending")
        net.popup_done(args.popup_id)
        return {close=true}
    end},
}
