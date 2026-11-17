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
        for _,key in ipairs({"economic","military","foreign","science"}) do
            roles[#roles+1]={key=key,name=names[key],role=names[key],portrait="",pages=array()}
        end
        -- SIMPLIFIED(SIMP-394): native GetAdvisorCounsel reads the global active player; a seat-scoped read-only bridge is required.
        return {civ={key=c.Type,name=L(c.ShortDescription),icon="civ:"..c.Type,color_bg=bg,color_fg=fg},
            era=L(era.Description),roles=roles,reason="Counsel is unavailable until the game exposes a seat-scoped advisor query"}
    end,
    actions={},
}
