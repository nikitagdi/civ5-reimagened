local function context(player,args)
    local p=args.popup
    if not p or p.kind~="BUTTONPOPUP_CHOOSE_GOODY_HUT_REWARD" or p.id~=args.a then return nil end
    assert(p.data1==player, "This ruin choice belongs to another seat")
    local unit=Players[player]:GetUnitByID(p.data2)
    assert(unit and unit:GetOwner()==player, "The ruin explorer is no longer yours")
    local plot=Map.GetPlot(unit:GetX(),unit:GetY())
    assert(plot, "The ruin plot is unavailable")
    return {id=p.id,unit=p.data2,plot=plot:GetPlotIndex()}
end
local function eligible(player,e)
    local p=Players[player]
    assert(p, "Unknown seat")
    local u=p:GetUnitByID(e.unit)
    assert(u and u:GetOwner()==player, "The ruin explorer is no longer yours")
    local plot=Map.GetPlotByIndex(e.plot)
    assert(plot and u:GetX()==plot:GetX() and u:GetY()==plot:GetY(), "The explorer has left the ruins")
    return p,u,plot
end
return {
    data=function(player,args)
        assert(Players[player], "Unknown seat")
        local e=context(player,args)
        if not e or e.id~=args.a then
            return {id=args.a or -1, unit={id=-1,name="",icon=""},plot={x=0,y=0},rewards=array(),
                reason="No matching ancient ruin event is pending for this seat", resolved=false}
        end
        if e.resolved then return {id=e.id,unit=e.unit_view,plot=e.plot_view,rewards=array(),resolved=true} end
        local p,u,plot=eligible(player,e)
        local info=GameInfo.Units[u:GetUnitType()]
        local r={id=e.id,unit={id=e.unit,name=u:GetName(),icon="unit:"..info.Type},
            plot={x=plot:GetX(),y=plot:GetY()},rewards=array(),resolved=e.resolved or false}
        if e.resolved then return r end
        if e.submitted then r.reason="Awaiting the game response";return r end
        for goody in GameInfo.GoodyHuts() do
            if p:CanGetGoody(plot,goody.ID,u) then
                r.rewards[#r.rewards+1]={id=goody.ID,key=goody.Type,icon=icon(goody.IconAtlas,goody.PortraitIndex),
                    parts=array({{id=0,text=L(goody.Description)}})}
            end
        end
        if #r.rewards==0 then r.reason="There are no eligible rewards for this explorer" end
        return r
    end,
    actions={take=function(player,args,net)
        local e=context(player,args)
        assert(e and e.id==args.a and not e.resolved, "This ancient ruin choice is no longer pending")
        assert(not e.submitted, "A reward answer is already awaiting the game response")
        local p,u,plot=eligible(player,e)
        assert(args.b and GameInfo.GoodyHuts[args.b] and p:CanGetGoody(plot,args.b,u), "This reward is not eligible")
        net.send("sendGoodyChoice",plot:GetX(),plot:GetY(),args.b,e.unit)
        net.popup_done(args.popup_id)
        return {close=true}
    end},
}
