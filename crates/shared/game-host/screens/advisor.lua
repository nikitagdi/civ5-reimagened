local function seat_colors(p)
    local primary,secondary=p:GetPlayerColors()
    local function rgba(v)
        return math.floor(v.x*255+.5)*16777216+math.floor(v.y*255+.5)*65536+math.floor(v.z*255+.5)*256+math.floor(v.w*255+.5)
    end
    return rgba(primary),rgba(secondary)
end
return {
    data=function(player)
        local p=Players[player];assert(p,"Unknown seat")
        local c=GameInfo.Civilizations[p:GetCivilizationType()]
        local bg,fg=seat_colors(p)
        local era=GameInfo.Eras[p:GetCurrentEra()]
        local roles=array()
        local names={economic="Economic advisor",military="Military advisor",foreign="Foreign advisor",science="Science advisor"}
        -- Seat-scoped read of the core's advisor counsel (same shape as Game.GetAdvisorCounsel: advisor enum -> zero-based texts).
        local counsel=GcUiGetAdvisorCounsel and GcUiGetAdvisorCounsel(player)
        local A=counsel and AdvisorTypes or {}
        local advisor={economic=A.ADVISOR_ECONOMIC,military=A.ADVISOR_MILITARY,foreign=A.ADVISOR_FOREIGN,science=A.ADVISOR_SCIENCE}
        for _,key in ipairs({"economic","military","foreign","science"}) do
            local pages=array()
            local texts=counsel and counsel[advisor[key]]
            if texts then
                local i=0
                while texts[i] do pages[#pages+1]={id=i,text=plain(texts[i])};i=i+1 end
            end
            roles[#roles+1]={key=key,name=names[key],role=names[key],portrait="",pages=pages}
        end
        return {civ={key=c.Type,name=L(c.ShortDescription),icon="civ:"..c.Type,color_bg=bg,color_fg=fg},
            era=L(era.Description),roles=roles,reason=not counsel and "Counsel is unavailable: this game core has no seat-scoped advisor query" or nil}
    end,
    actions={},
}
