local function seat_colors(p)
    local primary,secondary=p:GetPlayerColors()
    local function rgba(v)
        return math.floor(v.x*255+.5)*16777216+math.floor(v.y*255+.5)*65536+math.floor(v.z*255+.5)*256+math.floor(v.w*255+.5)
    end
    return rgba(primary),rgba(secondary)
end
local victory_names={VICTORY_SPACE_RACE="science",VICTORY_DOMINATION="domination",VICTORY_CULTURAL="cultural",
    VICTORY_DIPLOMATIC="diplomatic",VICTORY_TIME="time",VICTORY_SCRAP="scrap"}
return {
    data=function(player)
        local p=Players[player];assert(p,"Unknown seat")
        local c=GameInfo.Civilizations[p:GetCivilizationType()]
        local bg,fg=seat_colors(p)
        local winning_team=Game.GetWinner()
        local finished=winning_team>=0
        local won=finished and p:GetTeam()==winning_team
        local v=GameInfo.Victories[Game.GetVictory()]
        local score=p:GetScore(true,won)
        local r={outcome=won and "victory" or "defeat",victory_type=v and victory_names[v.Type],
            civ={key=c.Type,icon="civ:"..c.Type,color_bg=bg,color_fg=fg},
            kicker=L(c.ShortDescription),title=finished and L(won and "TXT_KEY_VICTORY" or "TXT_KEY_DEFEAT") or "Game in progress",
            headline=v and L(v.Description) or "",text=v and L((won and v.VictoryText or v.DefeatText) or v.Description) or "",
            art="",delay_ms=v and victory_names[v.Type]=="science" and 7000 or 0,turn=Game.GetGameTurn(),
            year=tostring(Game.GetTurnYear(Game.GetGameTurn())),can_continue=false,
            continue_reason=finished and "Continuing requires a session extended-game operation" or "The game has not ended",
            players=array(),ranking={title=L("TXT_KEY_HISTORIC_RANKING"),sub="",my_score=score,rows=array()},finished=finished}
        for id=0,GameDefines.MAX_MAJOR_CIVS-1 do
            local other=Players[id]
            if other and other:IsEverAlive() and not other:IsMinorCiv() and (id==player or met(player,id)) then
                local oc=GameInfo.Civilizations[other:GetCivilizationType()]
                local obg,ofg=seat_colors(other)
                r.players[#r.players+1]={id=id,name=L(oc.ShortDescription),civ_icon="civ:"..oc.Type,
                    color_bg=obg,color_fg=ofg,score=other:GetScore(true,other:GetTeam()==winning_team),me=id==player}
                if finished and other:GetTeam()==winning_team and not r.winner then
                    r.winner={player=id,name=L(oc.ShortDescription),civ_name=L(oc.ShortDescription)}
                end
            end
        end
        table.sort(r.players,function(a,b) if a.score==b.score then return a.id<b.id end return a.score>b.score end)
        if GameInfo.HistoricRankings then
            for h in GameInfo.HistoricRankings() do
                r.ranking.rows[#r.ranking.rows+1]={id=h.ID,name=L(h.HistoricalLeader),threshold=h.LeaderScore,me=false,quote=h.LeaderQuote and L(h.LeaderQuote)}
            end
            table.sort(r.ranking.rows,function(a,b) return a.threshold>b.threshold end)
            for _,row in ipairs(r.ranking.rows) do
                if score>=row.threshold then row.me=true;r.ranking.title=row.name;break end
            end
        end
        return r
    end,
    actions={
        -- SIMPLIFIED(SIMP-393): extended-game and exit are absent from the UI net allowlist; embedded demographics/replay require separate roots.
        ["continue"]=function() error("Continuing requires a session extended-game operation") end,
        exit=function() error("Leaving the game requires the client session handler") end,
    },
}
