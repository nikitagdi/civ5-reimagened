local function seat_colors(p)
    local primary,secondary=p:GetPlayerColors()
    local function rgba(v)
        return math.floor(v.x*255+.5)*16777216+math.floor(v.y*255+.5)*65536+math.floor(v.z*255+.5)*256+math.floor(v.w*255+.5)
    end
    return rgba(primary),rgba(secondary)
end
local function civ(p)
    local c=GameInfo.Civilizations[p:GetCivilizationType()]
    local bg,fg=seat_colors(p)
    return {key=c.Type,icon="civ:"..c.Type,color_bg=bg,color_fg=fg}
end
return {
    data=function(player)
        local p=Players[player]; assert(p,"Unknown seat")
        local total=p:GetNumNotifications()
        local r={civ=civ(p),total=total,open=0,groups=array()}
        local by_turn={}
        -- SIMPLIFIED(SIMP-391): native history is capped at 200; type metadata and grouped HUD decisions need an enriched notification source.
        for i=total-1,math.max(0,total-200),-1 do
            local id=p:GetNotificationIndex(i)
            if id>=0 then
                local turn=p:GetNotificationTurn(i)
                local g=by_turn[turn]
                if not g then g={turn=turn,count=0,items=array()};by_turn[turn]=g;r.groups[#r.groups+1]=g end
                local active=not p:GetNotificationDismissed(i)
                if active then r.open=r.open+1 end
                g.count=g.count+1
                g.items[#g.items+1]={id=id,type="",icon="",tint="neutral",text=p:GetNotificationStr(i),
                    category=p:GetNotificationSummaryStr(i),active=active}
            end
        end
        table.sort(r.groups,function(a,b) return a.turn>b.turn end)
        return r
    end,
    actions={},
}
