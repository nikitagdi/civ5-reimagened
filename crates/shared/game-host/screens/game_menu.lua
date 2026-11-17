local function seat_colors(p)
    local primary,secondary=p:GetPlayerColors()
    local function rgba(v)
        return math.floor(v.x*255+.5)*16777216+math.floor(v.y*255+.5)*65536+math.floor(v.z*255+.5)*256+math.floor(v.w*255+.5)
    end
    return rgba(primary),rgba(secondary)
end
local function option(name)
    local o=GameInfo.GameOptions[name]
    return o and Game.IsOption(o.ID) or false
end
local unavailable="This setting is managed by the session service"
return {
    data=function(player)
        local p=Players[player];assert(p,"Unknown seat")
        local c=GameInfo.Civilizations[p:GetCivilizationType()]
        local bg,fg=seat_colors(p)
        local leader=GameInfo.Leaders[p:GetLeaderType()]
        local era=GameInfo.Eras[p:GetCurrentEra()]
        local r={mode="network",turn=Game.GetGameTurn(),
            player={id=player,leader=L(leader.Description),leader_icon="leader:"..leader.Type,
                civ=L(c.ShortDescription),civ_icon="civ:"..c.Type,color_bg=bg,color_fg=fg,
                trait={name="",short="",long=""}},
            settings=array(),details=array({{key="era",label="Current era",value=L(era.Description)}}),
            victories=array(),game_options=array(),mods=array(),mods_reason="The session mod catalog is not exposed to screen scripts",
            can_save=false,save_reason="Saving requires the session save service",
            options={quick_combat=option("GAMEOPTION_QUICK_COMBAT"),quick_movement=option("GAMEOPTION_QUICK_MOVEMENT"),
                quick_locked_reason="Quick combat and movement are fixed when a network game is created",
                email_turn=false,autosave_every=0,autosave_keep=0},hotkeys=array()}
        for trait in GameInfo.Leader_Traits{LeaderType=leader.Type} do
            local t=GameInfo.Traits[trait.TraitType]
            if t then r.player.trait={name=L(t.ShortDescription),short=L(t.Description),long=L(t.Description)} end
        end
        local function setting(key,label,info)
            if info then
                local text=L(info.Description)
                r.settings[#r.settings+1]={key=key,label=L(label),value=text,short=text,icon="",tooltip=text}
                r.details[#r.details+1]={key=key,label=L(label),value=text}
            end
        end
        setting("map_size","Map size",GameInfo.Worlds[Map.GetWorldSize()])
        setting("difficulty","TXT_KEY_HANDICAP",GameInfo.HandicapInfos[p:GetHandicapType()])
        setting("speed","TXT_KEY_GAME_SPEED",GameInfo.GameSpeeds[Game.GetGameSpeedType()])
        for v in GameInfo.Victories() do
            if PreGame.IsVictory(v.ID) then r.victories[#r.victories+1]={key=v.Type,name=L(v.Description)} end
        end
        for o in GameInfo.GameOptions() do
            if o.Visible and o.Visible~=0 and Game.IsOption(o.ID) then
                r.game_options[#r.game_options+1]={key=o.Type,name=L(o.Description)}
            end
        end
        local function hotkey(row)
            local key=row.HotKey
            if not key or key=="" then return nil end
            key=string.gsub(key,"^KB_","")
            if #key==1 and string.match(key,"%u") then key="Key"..key
            elseif #key==1 and string.match(key,"%d") then key="Digit"..key
            elseif not string.match(key,"^F%d+$") then return nil end
            if row.ShiftDown==true or row.ShiftDown==1 then key="Shift+"..key end
            if row.AltDown==true or row.AltDown==1 then key="Alt+"..key end
            if row.CtrlDown==true or row.CtrlDown==1 then key="Ctrl+"..key end
            return key
        end
        for _,source in ipairs({{"commands","Commands","Unit commands"},{"builds","Builds","Build improvements"},
            {"promotions","UnitPromotions","Promotions"},{"controls","Controls","Game controls"},{"missions","Missions","Missions"}}) do
            local rows=GameInfo[source[2]]
            if rows then
                local group={key=source[1],name=source[3],count=0,actions=array()}
                for row in rows() do
                    group.actions[#group.actions+1]={key=row.Type,name=L(row.Description),icon=icon(row.IconAtlas,row.PortraitIndex),
                        help=row.Help and L(row.Help) or "",default=hotkey(row),hidden=false}
                end
                group.count=#group.actions
                if group.count>0 then r.hotkeys[#r.hotkeys+1]=group end
            end
        end
        return r
    end,
    actions={
        -- SIMPLIFIED(SIMP-392): session save, exit, mail, autosave and hotkey application need host services; browser preference drafts remain local.
        save=function() error("Saving requires the session save service") end,
        exit=function() error("Leaving the game requires the client session handler") end,
        set_quick=function() error("Quick options are fixed when a network game is created") end,
        set_email=function() error(unavailable) end,
        set_autosave=function() error(unavailable) end,
    },
}
