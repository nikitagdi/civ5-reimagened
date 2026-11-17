//! Military grouping, supply exclusion, fractional movement and seat ownership.
use lua_sys::Lua;

#[test]
fn military_counts_owned_groups_and_excludes_trade_units_from_supply() {
    let lua = Lua::new().unwrap();
    lua.sandbox().unwrap();
    lua.exec(include_str!("../screens/_lib.lua"), "=helpers")
        .unwrap();
    lua.exec(r#"
        Locale={ConvertTextKey=function(s) return s end}
        GameDefines={MOVE_DENOMINATOR=60}
        ActivityTypes={ACTIVITY_HEAL=1,ACTIVITY_SENTRY=2,ACTIVITY_SLEEP=3}
        GameInfo={Civilizations={[0]={Type="CIV_TEST",ShortDescription="Civilization",DefaultPlayerColor=0}},
            PlayerColors={[0]={PrimaryColor=0,SecondaryColor=0}},Colors={[0]={Red=0,Green=0,Blue=0}},
            Leaders={[0]={Description="Leader"}},Units={[0]={Type="UNIT_TEST"}}}
        local function unit(id,combat,trade,hp)
            local v={GetID=id,GetName="Unit "..id,GetUnitType=0,GetActivityType=0,GetFortifyTurns=0,GetBuildType=-1,
                NukeDamageLevel=-1,GetCurrHitPoints=hp,GetMaxHitPoints=100,MovesLeft=30,MaxMoves=120,
                GetBaseCombatStrength=combat and 5 or 0,GetBaseRangedCombatStrength=0}
            return setmetatable({IsCombatUnit=function() return combat end,IsTrade=function() return trade end,
                IsEmbarked=function() return false end,IsGarrisoned=function() return false end,IsAutomated=function() return false end,
                IsPromotionReady=function() return combat end},{__index=function(_,k) return function() assert(v[k]~=nil,k);return v[k] end end})
        end
        local function seat(list)
            return setmetatable({Units=function() local i=0;return function() i=i+1;return list[i] end end},
                {__index=function() return function() return 0 end end})
        end
        Players={[0]=seat({unit(1,true,false,50),unit(2,false,true,100)}),[1]=seat({})}
    "#, "=fixture").unwrap();
    let screen: lua_sys::Table = lua
        .load(include_str!("../screens/military.lua"))
        .eval()
        .unwrap();
    lua.globals().raw_set("screen", screen).unwrap();
    lua.exec(
        r#"
        local r=screen.data(0,{})
        assert(r.counts.military==1 and r.counts.civilian==1 and r.supply.used==1)
        assert(r.units[1].moves==0.5 and r.units[1].max_moves==2 and r.units[1].health_pct==50)
        assert(r.units[2].health_pct==nil and r.units[1].unit_key=="0:1")
        assert(#screen.data(1,{}).units==0 and next(screen.actions)==nil)
    "#,
        "=assertions",
    )
    .unwrap();
}
