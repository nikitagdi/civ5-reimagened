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

fn event(r: &Screens, payload: &str) {
    r.lua()
        .exec(&format!("__popup = {payload}"), "=queue-context")
        .unwrap();
}

#[test]
fn the_exit_question_is_answered_here_and_leaving_is_left_to_the_session() {
    let r = runtime();
    event(&r, "{id=9,kind='BUTTONPOPUP_CONFIRM_MENU'}");
    let root = data(&r, "confirm", 9);
    assert_eq!(root["kind"], "exit_game");
    assert_eq!(root["text"], "");
    let yes = root["choices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["key"] == "yes")
        .unwrap();
    assert_eq!(yes["enabled"], true);
    for b in [0, 1] {
        let out = r
            .action(
                0,
                "confirm",
                "choose",
                &[("a".into(), 9), ("b".into(), b)],
                None,
            )
            .unwrap();
        assert!(out.messages.is_empty(), "nothing for the game");
        assert_eq!(out.done, Some(9));
        assert!(out.close);
    }
}

#[test]
fn confirm_scopes_pending_events_and_stages_ransom_completion() {
    let r = runtime();
    assert!(
        data(&r, "confirm", 42)["choices"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    event(&r, "{id=42,kind='BUTTONPOPUP_BARBARIAN_RANSOM',data1=7}");
    assert_eq!(data(&r, "confirm", 42)["facts"][0]["value"], 25);
    for (id, choice) in [(41, 0), (42, 2)] {
        assert!(
            r.action(
                0,
                "confirm",
                "choose",
                &[("a".into(), id), ("b".into(), choice)],
                None
            )
            .is_err()
        );
    }
    let out = r
        .action(
            0,
            "confirm",
            "choose",
            &[("a".into(), 42), ("b".into(), 0)],
            None,
        )
        .unwrap();
    assert_eq!(
        out.messages,
        [NetMessage::new("sendBarbarianRansom", &[0, 7]).unwrap()]
    );
    assert_eq!(out.done, Some(42));
    assert!(out.close);
    // A refused batch leaves the queue context intact and retryable.
    assert_eq!(data(&r, "confirm", 42)["resolved"], false);
    assert!(
        r.action(
            0,
            "confirm",
            "choose",
            &[("a".into(), 42), ("b".into(), 0)],
            None
        )
        .is_ok()
    );
    r.context(None).unwrap();
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
fn confirm_return_keep_and_missing_gift_context_are_validated() {
    for choice in [0, 1] {
        let r = runtime();
        event(
            &r,
            "{id=42,kind='BUTTONPOPUP_RETURN_CIVILIAN',data1=0,data2=1,data3=7}",
        );
        data(&r, "confirm", 42);
        assert!(
            r.action(
                1,
                "confirm",
                "choose",
                &[("a".into(), 42), ("b".into(), choice)],
                None
            )
            .is_err()
        );
        let out = r
            .action(
                0,
                "confirm",
                "choose",
                &[("a".into(), 42), ("b".into(), choice)],
                None,
            )
            .unwrap();
        assert_eq!(
            out.messages,
            [NetMessage::new("sendReturnCivilian", &[i32::from(choice == 0), 1, 7]).unwrap()]
        );
        assert_eq!(out.done, Some(42));
    }
    let r = runtime();
    event(
        &r,
        "{id=42,kind='BUTTONPOPUP_GIFT_CONFIRM',data1=1,data2=7}",
    );
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
    let out = r
        .action(
            0,
            "confirm",
            "choose",
            &[("a".into(), 42), ("b".into(), 1)],
            None,
        )
        .unwrap();
    assert!(out.messages.is_empty());
    assert_eq!(out.done, Some(42));
}

#[test]
fn native_command_and_build_prompts_fail_closed_without_selected_unit_snapshot() {
    for kind in [
        "BUTTONPOPUP_CONFIRMCOMMAND",
        "BUTTONPOPUP_CONFIRM_IMPROVEMENT_REBUILD",
    ] {
        let r = runtime();
        event(
            &r,
            &format!("{{id=8,kind='{kind}',data1=9,data2=2,flag=16,option1=true}}"),
        );
        let root = data(&r, "confirm", 8);
        assert_eq!(root["choices"][1]["enabled"], false);
        assert!(
            r.action(
                0,
                "confirm",
                "choose",
                &[("a".into(), 8), ("b".into(), 0)],
                None
            )
            .is_err()
        );
        let out = r
            .action(
                0,
                "confirm",
                "choose",
                &[("a".into(), 8), ("b".into(), 1)],
                None,
            )
            .unwrap();
        assert!(out.messages.is_empty());
        assert_eq!(out.done, Some(8));
    }
}

#[test]
fn message_acknowledgement_requires_current_queue_context() {
    let r = runtime();
    data(&r, "message", 8);
    event(&r, "{id=8,kind='BUTTONPOPUP_TEXT'}");
    assert!(data(&r, "message", 8).get("text").is_none());
    event(&r, "{id=8,kind='BUTTONPOPUP_TEXT',text=''}");
    assert!(data(&r, "message", 8).get("text").is_none());
    event(&r, "{id=8,kind='BUTTONPOPUP_TEXT',text='Event text'}");
    assert_eq!(data(&r, "message", 8)["text"], "Event text");
    assert!(
        r.action(0, "message", "ok", &[("a".into(), 9)], None)
            .is_err()
    );
    let out = r
        .action(0, "message", "ok", &[("a".into(), 8)], None)
        .unwrap();
    assert!(out.messages.is_empty());
    assert_eq!(out.done, Some(8));
    assert!(out.close);
    r.context(None).unwrap();
    assert!(
        r.action(0, "message", "ok", &[("a".into(), 8)], None)
            .is_err()
    );
}

#[test]
fn confirm_without_body_preserves_facts_and_empty_text_is_normalized() {
    let r = runtime();
    event(
        &r,
        "{id=8,kind='BUTTONPOPUP_BARBARIAN_RANSOM',data1=7,text='  '}",
    );
    let root = data(&r, "confirm", 8);
    assert_eq!(root["text"], "");
    assert_eq!(root["facts"][0]["value"], 25);
    event(
        &r,
        "{id=8,kind='BUTTONPOPUP_CONFIRM_MENU',text='Leave now?'}",
    );
    assert_eq!(data(&r, "confirm", 8)["text"], "Leave now?");
}

#[test]
fn goody_choice_rechecks_eligibility_and_rejects_stale_or_foreign_answers() {
    let r = runtime();
    r.lua().exec(r#"
 local plot={GetX=function() return 4 end,GetY=function() return 5 end,GetPlotIndex=function() return 9 end}
 local unit={GetOwner=function() return 0 end,GetX=function() return 4 end,GetY=function() return 5 end,
 GetUnitType=function() return 2 end,GetName=function() return 'Explorer' end,
 IsDead=function() return false end,IsDelayedDeath=function() return explorer_gone end}
 Players[0].GetUnitByID=function(self,id) if id==7 then return unit end end
 Players[0].CanGetGoody=function(self,p,id,u) return p==plot and u==unit and not reward_locked end
 Map={GetPlot=function(x,y) if x==4 and y==5 then return plot end end}
 local gold={ID=2,Type='GOODY_GOLD',ChooseDescription='Receive [ICON_GOLD] gold'}
 local tech={ID=3,Type='GOODY_TECH',ChooseDescription='Discover a technology'}
 GameInfo={Units={[2]={Type='UNIT_PATHFINDER'}},GoodyHuts=setmetatable({[2]=gold,[3]=tech},
 {__call=function() local i=1;return function(t) i=i+1;return i==2 and gold or i==3 and tech or nil end end})}
 "#,"=goody-api").unwrap();
    event(
        &r,
        "{id=8,kind='BUTTONPOPUP_CHOOSE_GOODY_HUT_REWARD',data1=0,data2=7}",
    );
    let root = data(&r, "goody_choice", 8);
    assert_eq!(
        root["rewards"][0]["parts"],
        serde_json::json!([
            {"id": 0, "text": "Receive "},
            {"id": 1, "icon": "font:ICON_GOLD"},
            {"id": 2, "text": " gold"}
        ])
    );
    assert_eq!(
        root["rewards"][1]["parts"],
        serde_json::json!([{"id": 0, "text": "Discover a technology"}])
    );
    let args = [("a".into(), 8), ("b".into(), 3)];
    let cancel = [("a".into(), 8)];
    assert!(r.action(1, "goody_choice", "take", &args, None).is_err());
    assert!(
        r.action(0, "goody_choice", "cancel", &cancel, None)
            .is_err()
    );
    r.lua().exec("reward_locked=true", "=lock").unwrap();
    assert!(r.action(0, "goody_choice", "take", &args, None).is_err());
    r.lua().exec("reward_locked=false", "=unlock").unwrap();
    let out = r.action(0, "goody_choice", "take", &args, None).unwrap();
    assert_eq!(
        out.messages,
        [NetMessage::new("sendGoodyChoice", &[4, 5, 3, 7]).unwrap()]
    );
    assert_eq!(out.done, Some(8));
    assert!(out.close);
    // Refusal keeps the immutable head; successful completion removes it in the worker.
    assert_eq!(
        data(&r, "goody_choice", 8)["rewards"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    r.lua()
        .exec("explorer_gone=true", "=lost-explorer")
        .unwrap();
    let missing = data(&r, "goody_choice", 8);
    assert_eq!(missing["id"], 8);
    assert_eq!(missing["unit"]["id"], -1);
    assert_eq!(missing["rewards"], serde_json::json!([]));
    assert_eq!(
        missing["reason"],
        "The ruin explorer is no longer available"
    );
    assert!(
        r.action(1, "goody_choice", "cancel", &cancel, None)
            .is_err()
    );
    assert!(r.action(0, "goody_choice", "take", &args, None).is_err());
    let out = r
        .action(0, "goody_choice", "cancel", &cancel, None)
        .unwrap();
    assert!(out.messages.is_empty());
    assert_eq!(out.done, Some(8));
    assert!(out.close);
    r.context(None).unwrap();
    assert!(r.action(0, "goody_choice", "take", &args, None).is_err());
    assert!(
        r.action(0, "goody_choice", "cancel", &cancel, None)
            .is_err()
    );
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
    // The game allows a save; the host check is the session's (`session.is_host`, `session.op`).
    assert_eq!(root["can_save"], true);
    assert!(root["save_reason"].is_null());
    for action in ["save", "exit", "set_quick"] {
        let error = r
            .action(0, "game_menu", action, &[], None)
            .err()
            .expect("unavailable action");
        assert!(!error.to_string().is_empty());
    }
    // No faked client options: the client's options window keeps those.
    assert!(root["options"].get("email_turn").is_none());
    assert!(root["options"].get("autosave_every").is_none());
}

#[test]
fn menu_lists_the_game_action_keys_as_the_unit_panel_binds_them() {
    let r = catalog_runtime();
    r.lua()
        .exec(
            r#"
        GameInfoActions={[0]={Type='MISSION_FORTIFY',TextKey='Fortify',Visible=true,HotKeyVal=18},
            [1]={Type='COMMAND_DELETE',TextKey='Delete',Visible=true,HotKeyVal=108,CtrlDown=true},
            [2]={Type='CONTROL_ENDTURN',TextKey='End turn',Visible=true,HotKeyVal=44},
            [3]={Type='MISSION_EMBARK',TextKey='Embark',Visible=true},
            [4]={Type='BUILD_ROAD',TextKey='Road',Visible=false,HotKeyVal=29},
            [5]={Type='MISSION_SLEEP',TextKey='Sleep',Visible=true,HotKeyVal=31,ShiftDown=true}}
        "#,
            "=actions",
        )
        .unwrap();
    let root = data(&r, "game_menu", 0);
    let groups = root["hotkeys"].as_array().unwrap();
    let keys: Vec<(String, String)> = groups
        .iter()
        .flat_map(|g| g["actions"].as_array().unwrap().iter())
        .map(|a| {
            (
                a["key"].as_str().unwrap().to_owned(),
                a["default"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        keys,
        [
            ("MISSION_FORTIFY".to_owned(), "KeyF".to_owned()),
            ("MISSION_SLEEP".to_owned(), "Shift+KeyS".to_owned()),
            ("COMMAND_DELETE".to_owned(), "Ctrl+Delete".to_owned()),
        ],
        "no control, no action without a key, no hidden action"
    );
    assert_eq!(groups[0]["key"], "missions");
    assert_eq!(groups[0]["count"], 2);
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
    // After a victory the server core plays on (GAMESTATE_EXTENDED): one more turn closes the screen.
    r.lua()
        .exec(
            "GameplayGameStateTypes={GAMESTATE_ON=0,GAMESTATE_OVER=1,GAMESTATE_EXTENDED=2}
             Game.GetGameState=function() return game_state or 2 end",
            "=state",
        )
        .unwrap();
    let root = data(&r, "end_game", 0);
    assert_eq!(root["can_continue"], true);
    assert!(root["continue_reason"].is_null());
    let out = r.action(0, "end_game", "continue", &[], None).unwrap();
    assert!(out.close && out.messages.is_empty() && out.done.is_none());
    // Every human dead (GAMESTATE_OVER): the game is over for everybody.
    r.lua().exec("game_state=1", "=over").unwrap();
    let root = data(&r, "end_game", 0);
    assert_eq!(root["can_continue"], false);
    assert!(!root["continue_reason"].as_str().unwrap().is_empty());
    assert!(r.action(0, "end_game", "continue", &[], None).is_err());
    // Leaving is the session's: the screen refuses it with a reason.
    assert!(r.action(0, "end_game", "exit", &[], None).is_err());
    // Not finished: nothing to continue.
    r.lua()
        .exec("winner_team=nil game_state=0", "=running")
        .unwrap();
    assert!(r.action(0, "end_game", "continue", &[], None).is_err());
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
            HistoricRankings=rows({{ID=1,HistoricalLeader='Lower',LeaderScore=10},{ID=2,HistoricalLeader='Higher',LeaderScore=100}})}
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
