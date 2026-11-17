use super::runtime::Screens;
use gamecore::NetMessage;
use serde_json::Value;

fn runtime() -> Screens {
    let r = Screens::new().unwrap();
    r.load().unwrap();
    r.lua().exec(r#"
        Locale = {ConvertTextKey=function(key, ...) return key end}
        local unit = {GetOwner=function() return 0 end}
        Players = {[0]={GetGold=function() return 25 end, GetUnitByID=function(self,id) if id==7 then return unit end end},
                   [1]={IsMinorCiv=function() return true end}}
        Players[0].GetPlayerColors=function() return {x=.1,y=.2,z=.3,w=1},{x=.8,y=.7,z=.6,w=1} end
        GameDefines={BARBARIAN_UNIT_GOLD_RANSOM=100}
    "#, "=popup-fixture").unwrap();
    r
}

fn data(r: &Screens, name: &str, id: i64) -> Value {
    let json: Value = serde_json::from_str(&r.data(0, name, &[("a".into(), id)]).unwrap()).unwrap();
    ui2_model::Sample::parse(
        ui2_model::Schema::v1(),
        &serde_json::json!({"model":{name:json}}).to_string(),
    )
    .unwrap();
    json
}

#[test]
fn confirm_scopes_pending_events_and_emits_ransom_once() {
    let r = runtime();
    assert!(
        data(&r, "confirm", 42)["choices"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    r.lua()
        .exec(
            "__screens.confirm.enqueue(0,{id=42,kind='barbarian_ransom',unit=7,title='Ransom'})",
            "=event",
        )
        .unwrap();
    assert_eq!(data(&r, "confirm", 42)["facts"][0]["value"], 25);
    assert!(
        r.action(
            1,
            "confirm",
            "choose",
            &[("a".into(), 42), ("b".into(), 0)],
            None
        )
        .is_err()
    );
    assert!(
        r.action(
            0,
            "confirm",
            "choose",
            &[("a".into(), 42), ("b".into(), 2)],
            None
        )
        .is_err()
    );
    assert!(
        r.action(
            0,
            "confirm",
            "choose",
            &[("a".into(), 41), ("b".into(), 0)],
            None
        )
        .is_err()
    );
    assert_eq!(
        r.action(
            0,
            "confirm",
            "choose",
            &[("a".into(), 42), ("b".into(), 0)],
            None
        )
        .unwrap(),
        [NetMessage::new("sendBarbarianRansom", &[0, 7]).unwrap()]
    );
    assert_eq!(data(&r, "confirm", 42)["resolved"], false);
    r.lua()
        .exec("__screens.confirm.settle(0,42,true)", "=accepted")
        .unwrap();
    assert_eq!(data(&r, "confirm", 42)["resolved"], true);
    assert!(
        r.action(
            0,
            "confirm",
            "choose",
            &[("a".into(), 42), ("b".into(), 0)],
            None
        )
        .is_err()
    );
}

#[test]
fn confirm_return_keep_and_gift_use_the_event_recipient() {
    for (kind, choice, message, args) in [
        ("return_civilian", 0, "sendReturnCivilian", vec![1, 1, 7]),
        ("return_civilian", 1, "sendReturnCivilian", vec![0, 1, 7]),
        ("gift_unit", 0, "sendGiftUnit", vec![1, 7]),
        ("gift_unit", 1, "", vec![]),
    ] {
        let r = runtime();
        r.lua()
            .exec(
                &format!("__screens.confirm.enqueue(0,{{id=42,kind='{kind}',unit=7,recipient=1}})"),
                "=event",
            )
            .unwrap();
        data(&r, "confirm", 42);
        let out = r
            .action(
                0,
                "confirm",
                "choose",
                &[("a".into(), 42), ("b".into(), choice)],
                None,
            )
            .unwrap();
        if message.is_empty() {
            assert!(out.is_empty());
        } else {
            assert_eq!(out, [NetMessage::new(message, &args).unwrap()]);
        }
    }
}

#[test]
fn confirmation_replays_original_build_and_command_flags_after_revalidation() {
    for (kind, expected) in [
        (
            "improvement_rebuild",
            NetMessage::new("sendPushMission", &[7, 4, 9, 2, 16, 1]).unwrap(),
        ),
        (
            "confirm_command",
            NetMessage::new("sendDoCommand", &[7, 5, 9, 2, 1]).unwrap(),
        ),
    ] {
        let r = runtime();
        r.lua()
            .exec(
                r#"
            GameInfo={Missions={[4]={}},Commands={[5]={}}}
            Players[0].GetUnitByID=function(self,id) if id==7 then return {
                GetOwner=function() return 0 end,
                CanStartMission=function() return not order_locked end,
                CanDoCommand=function() return not order_locked end,
            } end end
        "#,
                "=order-api",
            )
            .unwrap();
        r.lua().exec(&format!("__screens.confirm.enqueue(0,{{id=8,kind='{kind}',unit=7,mission=4,command=5,data1=9,data2=2,flags=16,shift=true,alt=true}})"),"=event").unwrap();
        let args = [("a".into(), 8), ("b".into(), 0)];
        r.lua().exec("order_locked=true", "=lock").unwrap();
        let root = data(&r, "confirm", 8);
        assert_eq!(root["choices"][1]["enabled"], false);
        assert!(r.action(0, "confirm", "choose", &args, None).is_err());
        r.lua().exec("order_locked=false", "=unlock").unwrap();
        assert_eq!(
            r.action(0, "confirm", "choose", &args, None).unwrap(),
            [expected]
        );
    }
}

#[test]
fn message_acknowledgement_is_seat_scoped_and_one_shot() {
    let r = runtime();
    data(&r, "message", 8);
    r.lua()
        .exec(
            "__screens.message.enqueue(0,{id=8,kind='text',text='Event text'})",
            "=event",
        )
        .unwrap();
    assert_eq!(data(&r, "message", 8)["text"], "Event text");
    assert!(
        r.action(1, "message", "ok", &[("a".into(), 8)], None)
            .is_err()
    );
    assert!(
        r.action(0, "message", "ok", &[("a".into(), 9)], None)
            .is_err()
    );
    assert!(
        r.action(0, "message", "ok", &[("a".into(), 8)], None)
            .unwrap()
            .is_empty()
    );
    assert_eq!(data(&r, "message", 8)["resolved"], true);
    assert!(
        r.action(0, "message", "ok", &[("a".into(), 8)], None)
            .is_err()
    );
}

#[test]
fn goody_choice_rechecks_eligibility_and_rejects_repeat_or_foreign_answers() {
    let r = runtime();
    r.lua().exec(r#"
 local unit={GetOwner=function() return 0 end,GetX=function() return 4 end,GetY=function() return 5 end,
 GetUnitType=function() return 2 end,GetName=function() return 'Explorer' end}
 Players[0].GetUnitByID=function(self,id) if id==7 then return unit end end
 Players[0].CanGetGoody=function(self,plot,id,u) return id==3 and not reward_locked end
 Map={GetPlotByIndex=function(id) if id==9 then return {GetX=function() return 4 end,GetY=function() return 5 end} end end}
 local goody={ID=3,Type='GOODY_TECH',Description='Technology'}
 GameInfo={Units={[2]={Type='UNIT_PATHFINDER'}},GoodyHuts=setmetatable({[3]=goody},{__call=function() local done=false;return function() if not done then done=true;return goody end end end})}
 __screens.goody_choice.enqueue(0,{id=8,unit=7,plot=9})
 "#,"=goody-api").unwrap();
    assert_eq!(
        data(&r, "goody_choice", 8)["rewards"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let args = [("a".into(), 8), ("b".into(), 3)];
    assert!(r.action(1, "goody_choice", "take", &args, None).is_err());
    r.lua().exec("reward_locked=true", "=lock").unwrap();
    assert!(r.action(0, "goody_choice", "take", &args, None).is_err());
    r.lua().exec("reward_locked=false", "=unlock").unwrap();
    assert_eq!(
        r.action(0, "goody_choice", "take", &args, None).unwrap(),
        [NetMessage::new("sendGoodyChoice", &[4, 5, 3, 7]).unwrap()]
    );
    assert_eq!(data(&r, "goody_choice", 8)["resolved"], false);
    assert!(r.action(0, "goody_choice", "take", &args, None).is_err());
    r.lua()
        .exec("__screens.goody_choice.settle(0,8,false)", "=refused")
        .unwrap();
    assert_eq!(
        data(&r, "goody_choice", 8)["rewards"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    r.action(0, "goody_choice", "take", &args, None).unwrap();
    r.lua()
        .exec("__screens.goody_choice.settle(0,8,true)", "=accepted")
        .unwrap();
    assert_eq!(data(&r, "goody_choice", 8)["resolved"], true);
    assert!(r.action(0, "goody_choice", "take", &args, None).is_err());
}

#[test]
fn notification_log_keeps_native_ids_turns_and_dismissed_flags() {
    let r = runtime();
    r.lua()
        .exec(
            r#"
 GameInfo={Civilizations={[0]={Type='CIVILIZATION_TEST',DefaultPlayerColor='PC'}},
 PlayerColors={PC={PrimaryColor='A',SecondaryColor='B'}},
 Colors={A={Red=0,Green=0,Blue=0},B={Red=1,Green=1,Blue=1}}}
 Players[0].GetCivilizationType=function() return 0 end
 Players[0].GetNumNotifications=function() return 3 end
 Players[0].GetNotificationIndex=function(self,i) return 100+i end
 Players[0].GetNotificationTurn=function(self,i) return i==0 and 1 or 2 end
 Players[0].GetNotificationDismissed=function(self,i) return i==1 end
 Players[0].GetNotificationStr=function(self,i) return 'Notification '..i end
 Players[0].GetNotificationSummaryStr=function(self,i) return 'Summary' end
 "#,
            "=notification-api",
        )
        .unwrap();
    let root = data(&r, "notification_log", 0);
    assert_eq!(root["total"], 3);
    assert_eq!(root["open"], 2);
    assert_eq!(root["groups"][0]["turn"], 2);
    assert_eq!(root["groups"][0]["items"][0]["id"], 102);
    assert_eq!(root["groups"][0]["items"][1]["active"], false);
}

#[test]
fn advisor_never_reads_the_global_active_player() {
    let r = runtime();
    r.lua().exec(r#"
 GameInfo={Civilizations={[0]={Type='CIVILIZATION_TEST',ShortDescription='Test'}},Eras={[0]={Description='Ancient'}}}
 Players[0].GetCivilizationType=function() return 0 end
 Players[0].GetCurrentEra=function() return 0 end
 Game={GetAdvisorCounsel=function() error('unsafe global counsel getter') end}
 "#,"=advisor-api").unwrap();
    let root = data(&r, "advisor", 0);
    assert_eq!(root["roles"].as_array().unwrap().len(), 4);
    assert!(!root["reason"].as_str().unwrap().is_empty());
}

#[test]
fn menu_reads_game_rules_and_refuses_unavailable_session_actions() {
    let r = catalog_runtime();
    let root = data(&r, "game_menu", 0);
    assert_eq!(root["turn"], 7);
    assert_eq!(root["player"]["id"], 0);
    assert_eq!(root["options"]["quick_combat"], true);
    assert_eq!(root["settings"].as_array().unwrap().len(), 3);
    assert_eq!(root["can_save"], false);
    assert!(!root["save_reason"].as_str().unwrap().is_empty());
    for action in ["save", "exit", "set_quick", "set_email", "set_autosave"] {
        let error = r.action(0, "game_menu", action, &[], None).unwrap_err();
        assert!(!error.to_string().is_empty());
    }
}

#[test]
fn end_game_uses_winning_team_and_sorts_scores_and_historic_thresholds() {
    let r = catalog_runtime();
    let root = data(&r, "end_game", 0);
    assert_eq!(root["finished"], false);
    assert_eq!(root["players"].as_array().unwrap().len(), 1);
    assert_eq!(root["ranking"]["rows"][0]["threshold"], 100);
    assert_eq!(root["ranking"]["rows"][1]["me"], true);
    r.lua().exec("winner_team=2", "=winner").unwrap();
    let root = data(&r, "end_game", 0);
    assert_eq!(root["finished"], true);
    assert_eq!(root["outcome"], "victory");
    assert_eq!(root["winner"]["player"], 0);
    for action in ["continue", "exit"] {
        assert!(r.action(0, "end_game", action, &[], None).is_err());
    }
}

fn catalog_runtime() -> Screens {
    let r = runtime();
    r.lua().exec(r#"
        local function rows(values)
            return setmetatable(values,{__call=function()
                local i=0;return function() i=i+1;return values[i] end
            end})
        end
        local civ={Type='CIVILIZATION_TEST',ShortDescription='Test'}
        GameInfo={Civilizations={[0]=civ},Leaders={[0]={Type='LEADER_TEST',Description='Leader'}},
            Eras={[0]={Description='Ancient'}},Leader_Traits=rows({}),Worlds={[0]={Description='Duel'}},
            HandicapInfos={[0]={Description='Prince'}},GameSpeeds={[0]={Description='Quick'}},
            Victories=rows({}),GameOptions=rows({}),
            HistoricRankings=rows({{ID=1,HistoricalLeader='Lower',Score=10},{ID=2,HistoricalLeader='Higher',Score=100}})}
        GameInfo.GameOptions.GAMEOPTION_QUICK_COMBAT={ID=0}
        GameInfo.GameOptions.GAMEOPTION_QUICK_MOVEMENT={ID=1}
        local p=Players[0]
        p.GetCivilizationType=function() return 0 end;p.GetLeaderType=function() return 0 end
        p.GetCurrentEra=function() return 0 end;p.GetHandicapType=function() return 0 end
        p.GetTeam=function() return 2 end;p.GetScore=function() return 50 end
        p.IsEverAlive=function() return true end;p.IsMinorCiv=function() return false end
        Players[1].IsEverAlive=function() return false end
        Game={GetGameTurn=function() return 7 end,GetGameSpeedType=function() return 0 end,
            IsOption=function(id) return id==0 end,GetWinner=function() return winner_team or -1 end,
            GetVictory=function() return -1 end,GetTurnYear=function() return -3000 end}
        Map={GetWorldSize=function() return 0 end};PreGame={IsVictory=function() return false end}
        GameDefines.MAX_MAJOR_CIVS=2
    "#, "=catalog-api").unwrap();
    r
}
