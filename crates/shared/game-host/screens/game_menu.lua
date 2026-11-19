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
-- The key of a game action as the HUD's unit panel binds it (hud.lua `action_hotkey`): `[Ctrl+][Alt+][Shift+]<code>`.
local function action_hotkey(a)
    local n, code = a.HotKeyVal
    if n and n >= 13 and n <= 38 then code = "Key" .. string.char(65 + n - 13)
    elseif n and n >= 2 and n <= 11 then code = "Digit" .. (n - 2)
    elseif n and n >= 59 and n <= 70 then code = "F" .. (n - 58)
    elseif n and n >= 73 and n <= 82 then code = "Numpad" .. (n - 73)
    else code = ({[1]="Escape", [12]="Minus", [39]="Equal", [40]="Backspace", [41]="Tab", [42]="BracketLeft", [43]="BracketRight", [44]="Enter",
        [46]="Semicolon", [47]="Quote", [48]="Backquote", [50]="Backslash", [51]="Comma", [52]="Period", [53]="Slash", [55]="NumpadMultiply",
        [57]="Space", [58]="CapsLock", [71]="NumLock", [72]="ScrollLock", [83]="NumpadSubtract", [84]="NumpadAdd", [85]="NumpadDecimal",
        [95]="NumpadDivide", [98]="Pause", [99]="Home", [100]="ArrowUp", [101]="PageUp", [102]="ArrowLeft", [103]="ArrowRight", [104]="End",
        [105]="ArrowDown", [106]="PageDown", [107]="Insert", [108]="Delete"})[n] end
    if not code then return nil end
    return (a.CtrlDown and "Ctrl+" or "") .. (a.AltDown and "Alt+" or "") .. (a.ShiftDown and "Shift+" or "") .. code
end
-- The groups of the game's actions with a key, by the prefix of their type; the controls (`CONTROL_*`) are left out:
-- the unit panel binds the unit actions only.
local groups={{"MISSION_","missions","Missions"},{"COMMAND_","commands","Commands"},{"AUTOMATE_","automates","Automation"},
    {"BUILD_","builds","Builds"},{"PROMOTION_","promotions","Promotions"},{"INTERFACEMODE_","modes","Targeted actions"}}
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
            -- The game itself never forbids a save; who may save (the host) is the session's to say (`session.is_host`).
            can_save=true,
            options={quick_combat=option("GAMEOPTION_QUICK_COMBAT"),quick_movement=option("GAMEOPTION_QUICK_MOVEMENT"),
                quick_locked_reason="Quick combat and movement are fixed when a network game is created"},hotkeys=array()}
        for trait in GameInfo.Leader_Traits{LeaderType=leader.Type} do
            local t=GameInfo.Traits[trait.TraitType]
            if t then r.player.trait={name=plain(L(t.ShortDescription)),short=plain(L(t.Description)),long=plain(L(t.Description))} end
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
        -- The game's own keys: the actions of `GameInfoActions` with a key, as the unit panel binds them (the client's
        -- options window lists them with the skin's keys and lets the player rebind them).
        local by={}
        for _,g in ipairs(groups) do
            by[g[2]]={key=g[2],name=g[3],count=0,actions=array()}
        end
        local id,seen=0,{}
        while GameInfoActions and GameInfoActions[id] do
            local a=GameInfoActions[id]
            local key=a.Visible and a.Type and action_hotkey(a)
            if key and not seen[a.Type] then
                for _,g in ipairs(groups) do
                    if string.sub(a.Type,1,#g[1])==g[1] then
                        seen[a.Type]=true
                        local group=by[g[2]]
                        group.actions[#group.actions+1]={key=a.Type,name=L(a.TextKey or a.Type),
                            icon=a.IconAtlas and icon(a.IconAtlas,a.IconIndex or 0) or "",
                            help=a.Help and L(a.Help) or "",default=key,hidden=false}
                        break
                    end
                end
            end
            id=id+1
        end
        for _,g in ipairs(groups) do
            local group=by[g[2]]
            group.count=#group.actions
            if group.count>0 then r.hotkeys[#r.hotkeys+1]=group end
        end
        return r
    end,
    actions={
        -- The client's options are the client's (its options window keeps them in the classic client's store); the
        -- game menu screen has no option to set. Saving and leaving are session operations: the skin sends
        -- `session.op(op: 'save' | 'leave')`.
        save=function() error("Saving the game is a session operation") end,
        exit=function() error("Leaving the game is a session operation") end,
        set_quick=function() error("Quick options are fixed when a network game is created") end,
    },
}
