//! The simplifications of this crate (WO-5 range SIMP-exe-10..29), as data: the coverage report
//! prints them next to the names they cover. Every id appears as a `SIMPLIFIED(SIMP-exe-N)` marker in
//! the code that makes the simplification.

/// One row of the SIMP registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimpRow {
    pub id: &'static str,
    pub what: &'static str,
    pub place: &'static str,
}

pub const SIMP_ROWS: &[SimpRow] = &[
    SimpRow {
        id: "SIMP-exe-10",
        what: "PopupPriority values are our own order of the names (the exe's numbers are not in the headers)",
        place: "exe-api/enums.rs",
    },
    SimpRow {
        id: "SIMP-exe-11",
        what: "HexToWorld/GridToWorld: the hex constants are the ones of the renderer; z is the host's relief height at the plot centre (0 without a relief)",
        place: "exe-api/hex.rs",
    },
    SimpRow {
        id: "SIMP-exe-12",
        what: "Steam.* and Matchmaking.* functions outside the lobby (cloud saves, overlays, kick) are stubs",
        place: "exe-api/stubs.rs",
    },
    SimpRow {
        id: "SIMP-exe-13",
        what: "OptionsManager graphics/resolution functions and the cache sync calls are stubs (K20, no graphics options in the browser)",
        place: "exe-api/options.rs",
    },
    SimpRow {
        id: "SIMP-exe-14",
        what: "Modding.* other than OpenUserData and GetActivatedModEntryPoints: no mods are active, answers are empty",
        place: "exe-api/modding.rs",
    },
    SimpRow {
        id: "SIMP-exe-15",
        what: "ContentManager knows the G&K and BNW packages only; ContentType values are ours",
        place: "exe-api/content.rs",
    },
    SimpRow {
        id: "SIMP-exe-16",
        what: "Without a localizer in ExeConfig, Locale answers with the key itself",
        place: "exe-api/locale.rs",
    },
    SimpRow {
        id: "SIMP-exe-17",
        what: "OptionsManager getters answer the committed value; Set*_Cached waits for CommitGameOptions (the exe's cache rules are not recovered)",
        place: "exe-api/options.rs",
    },
    SimpRow {
        id: "SIMP-exe-18",
        what: "PreGame.SetLeaderKey/GetLeaderKey keep the key in memory only",
        place: "exe-api/pregame.rs",
    },
    SimpRow {
        id: "SIMP-exe-19",
        what: "User data holds numbers, booleans and strings; a table value is dropped",
        place: "exe-api/modding.rs",
    },
    SimpRow {
        id: "SIMP-exe-20",
        what: "Strategic view, game-view type, UnitMoving, IsGameCoreBusy and the turn timer length are stubs (U-D8: no strategic view)",
        place: "exe-api/stubs.rs",
    },
    SimpRow {
        id: "SIMP-exe-21",
        what: "NetKicked values are our own numbers for the names the scripts use",
        place: "exe-api/enums.rs",
    },
    SimpRow {
        id: "SIMP-exe-2",
        what: "GameEvents reach the UI through the FIFO after the batch, not synchronously; the listeners' results are not used",
        place: "exe-api/engine_events.rs",
    },
    SimpRow {
        id: "SIMP-exe-65",
        what: "Game.DoControl(CONTROL_ENDTURN) of the UI only sends the turn handshake (Network.SendTurnComplete); the DLL's local end of the turn of the seat is left to the server's journal; a loaded game clears Network.HasSentNetTurnComplete",
        place: "exe-api/lib.rs",
    },
    SimpRow {
        id: "SIMP-exe-66",
        what: "The DLL functions the UI calls to change the saved game without a network message (Game.SetEverRightClickMoved, SetAdvisorMessageHasBeenSeen, SetStaticTutorialActive, SetPausePlayer, Player:AddNotification) do nothing in the replica's UI: the exe applied them on its own machine only, here they made the replica differ from the server",
        place: "exe-api/lib.rs",
    },
    SimpRow {
        id: "SIMP-exe-64",
        what: "When the replica loads a game again (the snapshot of a turn), the UI is told every known unit and city destroyed and created again, because the Lua objects of the old load are dead (their \"Not a valid instance\" errors in the next 30 frames are logged, not counted)",
        place: "exe-api/engine_events.rs",
    },
    SimpRow {
        id: "SIMP-exe-3",
        what: "The tables engine callback -> Events.* and dirty bit -> event follow the names and signatures of the EUI listeners, not the bodies of the exe (J-EVENTS of re-ui)",
        place: "exe-api/engine_events.rs",
    },
    SimpRow {
        id: "SIMP-exe-5",
        what: "The AI's turn is seen as a jump: units teleport to where they are after a load, no animation",
        place: "exe-api/entities.rs",
    },
    SimpRow {
        id: "SIMP-exe-31",
        what: "NonSerialContainer and SequencerContainer events are dispatched at once like LocalMachine ones",
        place: "exe-api/event_table.rs",
    },
    SimpRow {
        id: "SIMP-exe-32",
        what: "An Events name the exe does not register still gives an event object (counted as Events.<name>)",
        place: "exe-api/events.rs",
    },
    SimpRow {
        id: "SIMP-exe-34",
        what: "No movement animation: a unit's move is a teleport to the end of its path (SerialEventUnitTeleportedToHex)",
        place: "exe-api/engine_events.rs",
    },
    SimpRow {
        id: "SIMP-exe-35",
        what: "Network.Send<X> takes the arguments of gDLL->send<X> by position (string -> text); not from the bodies of cvLuaNetworkLibrary",
        place: "exe-api/network.rs",
    },
    SimpRow {
        id: "SIMP-exe-36",
        what: "UI functions outside K1-K3 (deal lists and history, advanced start, unit placing) are counted stubs",
        place: "exe-api/ui_api.rs",
    },
    SimpRow {
        id: "SIMP-exe-37",
        what: "Combat, walls, minimap pings and animation resets of the engine have no event (counted as engine.<Kind>)",
        place: "exe-api/engine_events.rs",
    },
    SimpRow {
        id: "SIMP-exe-38",
        what: "Network getters outside K1-K3 (turn slice info, message counts, debug level) are stubs",
        place: "exe-api/network.rs",
    },
    SimpRow {
        id: "SIMP-exe-4",
        what: "Contexts outside milestone M2 (K4-K20) and the Lekmod add-ins the game core runs itself load as empty hidden contexts (ui Shared::context_filter)",
        place: "ui/shared.rs, client-app/m2.rs",
    },
    SimpRow {
        id: "SIMP-exe-70",
        what: "A Button/GridButton without a String gets an empty text label on the first GetTextControl (EUI CityView headers)",
        place: "ui/lua_api.rs",
    },
    SimpRow {
        id: "SIMP-exe-71",
        what: "Events.X.Add(nil) adds nothing and raises no error (NaturalWonderPopup passes an undefined global)",
        place: "exe-api/events.rs",
    },
    SimpRow {
        id: "SIMP-exe-72",
        what: "The city screen camera zoom level (17.1) is measured on golden S3, not reversed; no tilt or offset change",
        place: "client-app/m2.rs",
    },
    SimpRow {
        id: "SIMP-exe-73",
        what: "ToolTipType elements read before ToolTips registers its basic controls wait and are built then",
        place: "ui/build.rs",
    },
    SimpRow {
        id: "SIMP-exe-74",
        what: "A Lua wild include with a directory pattern also matches Lua files of other directories (by last segment) that have no XML twin (ConfirmCommandPopup layout)",
        place: "lua-sys/rt/mod.rs",
    },
    SimpRow {
        id: "SIMP-exe-55",
        what: "A numeric highlight style is read as an index into the 34 styles of Highlights.xml (the Lua passes names)",
        place: "client-app/m2_highlights.rs",
    },
    SimpRow {
        id: "SIMP-exe-56",
        what: "A SerialEventHexHighlight with an unreadable hex or style is dropped",
        place: "client-app/m2_highlights.rs",
    },
    SimpRow {
        id: "SIMP-exe-60",
        what: "SUPERSEDED (WO-G): the options and user data of the original UI live in the page store (localStorage) and stick between sessions; the row only marks the first, in-memory version",
        place: "client-app/m2.rs",
    },
    SimpRow {
        id: "SIMP-exe-61",
        what: "WorldAnchor projects on the ground plane (z = 0) of the game camera; ui.lua_us is the time of ExeApi::dispatch",
        place: "client-app/m2.rs",
    },
    SimpRow {
        id: "SIMP-exe-63",
        what: "A change of the unit selection raises SerialEventUnitInfoDirty from the exe API (the replica's selection ops set no dirty bit)",
        place: "exe-api/engine_events.rs",
    },
    SimpRow {
        id: "SIMP-exe-130",
        what: "No strategic view (U-D8): GetStrategicViewOverlays / GetStrategicViewIconSettings answer the names of the exe's option lists and the setters (SetStrategicViewOverlay, SetStrategicViewIconSetting) do nothing",
        place: "exe-api/corners.rs",
    },
    SimpRow {
        id: "SIMP-exe-131",
        what: "Matchmaking.KickPlayer asks the host (ExeHost::kick_player); the exe's lobby slot list is not modelled, the MP player list reads Players[]",
        place: "exe-api/corners.rs",
    },
    SimpRow {
        id: "SIMP-exe-132",
        what: "The chat copy the Lekmod DiploCorner sends as Network.SendEnhanceReligion with the religion -1 is dropped; a chat line to a team goes as direct lines to the team (SIMP-exe-912)",
        place: "exe-api/corners.rs",
    },
    SimpRow {
        id: "SIMP-exe-133",
        what: "The exe's YieldIconManager is a reconstruction: USER_ALL_ON shows every plot of the camera's hex rectangle (SIMP-exe-620 for the rest), AREA/EMPIRE/CITY_* sets are our reading of the names; resource icons follow the DLL's plot state",
        place: "exe-api/yields.rs",
    },
    SimpRow {
        id: "SIMP-exe-134",
        what: "The deal functions of TradeLogic are real since K12 (SIMP-exe-300); only UI.SetOfferTradeRepeatCount stays a counted stub",
        place: "exe-api/corners.rs",
    },
    SimpRow {
        id: "SIMP-exe-300",
        what: "Deals with an AI: the replica evaluates the offer as the original does, the accepted deal goes to the server as a player message (sendNetDealAccepted / sendNetDemandAccepted) and every replica applies it. Deals between two humans (fix-mp-trade): UI.DoProposeDeal to a human sends sendNetDealProposed (every machine stores the proposal and posts PLAYER_DEAL / PLAYER_DEAL_RECEIVED), UI.DoFinalizePlayerDeal sends sendNetPlayerDealFinalized (ResponsePlayerDealFinalized), as the exe's NetPlayerDealPropose / NetPlayerDealFinalized do (civ5-re re-mp-answers item 2); unlike the exe, which trusts its peers, the server drops a message not sent by the deal's side, a second open proposal of a player and a proposal at war under ALWAYS_WAR / NO_CHANGING_WAR_PEACE",
        place: "exe-api/ui_api.rs, gamecore replica (rt_ui_deal.cpp, rt_net.cpp DeliverDeal / DeliverPlayerDeal)",
    },
    SimpRow {
        id: "SIMP-exe-301",
        what: "UI.OnHumanOpenedTradeScreen and UI.OnHumanDemand answer with the leader message of the trade state (DIPLO_MESSAGE_LETS_HEAR_IT text) from the replica: the exe's own body was not reverse-engineered; AI offers and demands to a human are not delivered at all (SIMP-003: the AI makes them to the server seat only)",
        place: "rt_ui_deal.cpp",
    },
    SimpRow {
        id: "SIMP-exe-135",
        what: "The MP list's ping of a remote player is the round trip of this client's line to the server (the client talks to the server only); a player who is not an online human has no ping",
        place: "client-app/app.rs",
    },
    SimpRow {
        id: "SIMP-exe-136",
        what: "The turn events of a new turn (NewGameTurn, ActivePlayerTurnStart) are raised by the exe API when the replica loads a snapshot of a later turn: the core fires none for a loaded game (no ActivePlayerTurnEnd / RemotePlayerTurn* either)",
        place: "exe-api/lib.rs",
    },
    SimpRow {
        id: "SIMP-exe-137",
        what: "PreGame.GetCivilizationShortDescription / GetCivilizationDescription / GetCivilizationAdjective answer the empty string when the replica's PreGame lacks them (no custom civilization names in MP; the scripts use GameInfo.Civilizations)",
        place: "exe-api/corners.rs",
    },
    SimpRow {
        id: "SIMP-exe-100",
        what: "Mouse.eLclick (a typo of CommonBehaviors.lua RegisterTabBehavior) is an alias of Mouse.eLClick: the exe takes a nil event slot, we make the tab buttons work",
        place: "exe-api/enums.rs",
    },
    SimpRow {
        id: "SIMP-exe-220",
        what: "A texture the ui set holds but no window group brings (an icon atlas picked through the game database: IconHookup, CivIconHookup) is fetched in the background when a control takes it with SetTexture; the control takes it at once with the size the index gives (every texture has one) and draws nothing until the texture has arrived (the original reads it synchronously)",
        place: "ui-client/store.rs, ui-client/boot.rs, ui-client/layer.rs",
    },
    SimpRow {
        id: "SIMP-exe-221",
        what: "The windows of the ui set do not require the icon atlases their Lua names (the atlas groups stay in the index, every atlas of every size is also an id of its own): every atlas is fetched on demand, so the first frames of a window show some icons late",
        place: "tools/asset-pipeline/src/ui/build.rs",
    },
    SimpRow {
        id: "SIMP-exe-190",
        what: "The volume of the UI sounds is not an OptionsManager option (the exe gives no volume getter to the Lua): the page sets it with ?volume=0..100 (default 50, the game's default) as the start of the four volume knobs a player never saved; the master gain of the Web Audio output stays 1.0 (the exe registers no MASTER option)",
        place: "client-app/ui_sound.rs, apps/web/lib.rs",
    },
    SimpRow {
        id: "SIMP-exe-191",
        what: "Button clicks (provisional rule SIMP-uiau-1), Events.AudioPlay2DSound of the Lua, new notifications (NotificationSounds.xml; the found-ruins and unit-promotion notifications carry the goody-hut and promotion sounds the players hear), popups, own unit/city created, unit selection and the city screen ambience make a UI sound; turn-start and advisor lines, mission sounds, unit action sounds (3D) and the terrain soundscapes (SIMP-exe-975) the exe plays itself are not wired (the per-reward goody-hut sounds do not exist in the original: SIMP-exe-1001; the promotion sound of the unit panel is a Lua AudioPlay2DSound and plays), and there is no hover sound",
        place: "client-app/ui_sound.rs, client-app/m2.rs",
    },
    SimpRow {
        id: "SIMP-exe-390",
        what: "LoadMenu, SaveMapMenu and TouchControlsMenu (single player, touch) stay empty; Exit to Windows leaves for the lobby like Exit to main menu; UI.GetMapPreview answers nothing (the map is a script)",
        place: "exe-api/m4_wog.rs",
    },
    SimpRow {
        id: "SIMP-exe-391",
        what: "The graphics, resolution, language, tutorial and e-mail options of the options menu are stored and read back but drive nothing (a browser tab); the menu lists one resolution (UIManager:GetResCount/GetResInfo) and one language",
        place: "exe-api/m4_wog.rs, ui/lua_api.rs",
    },
    SimpRow {
        id: "SIMP-exe-392",
        what: "Each volume knob of the options menu (MUSIC, SFX, SPEECH, AMBIENCE) is the gain of its own bus of the UI sounds and applies at once (the exe ramps a knob, FKnob, its time not read); the knobs start at the page's ?volume= (SIMP-exe-190) and stick in the page's store",
        place: "exe-api/m4_wog.rs, client-app/m2.rs, client-app/ui_sound.rs",
    },
    SimpRow {
        id: "SIMP-exe-393",
        what: "UI.SaveGame asks the server for a save of the game (GameCtl::Save, protocol v5; the host only, the server names the save and the name typed in the save menu is not used); the save menu lists the saves of the server (names and turn from the lobby state, no header data), the save window shows the game being played, the Steam Cloud check box is still drawn and lists nothing, quick save and delete do nothing",
        place: "exe-api/m4_wog.rs, server/game/actor.rs",
    },
    SimpRow {
        id: "SIMP-exe-394",
        what: "The Save game button of the game menu is hidden for a guest by Modding.AnyActivatedModsContainPropertyValue(\"DisableSaveGameOption\") answering true (the Lekmod menu shows it to everybody)",
        place: "exe-api/m4_wog.rs",
    },
    SimpRow {
        id: "SIMP-exe-395",
        what: "Events.EndGameShow is raised by the exe API when the replica loads a state that is over (the replica runs no turn; the server sends the final snapshot at the end of the game); EndGameTypes has our numbers; extended play (Network.SendExtendedGame) does nothing",
        place: "exe-api/m4_wog.rs, game-host/replica.rs",
    },
    SimpRow {
        id: "SIMP-exe-396",
        what: "Modding.OpenUserData answers an object whose Query (the Lekmod hotkey manager's SQL on its user data) returns no row and drops the writes: the hotkeys of the hotkey manager do not stick",
        place: "exe-api/modding.rs",
    },
    SimpRow {
        id: "SIMP-exe-192",
        what: "The UI sound files ship as the originals (45 files, 3.7 MB, IMA ADPCM wav decoded in the client), not transcoded to ogg/opus; the music and world sounds are not in the set",
        place: "tools/asset-pipeline/src/ui/audio.rs",
    },
    SimpRow {
        id: "SIMP-exe-330",
        what: "LuaEvents.AdditionalInformationDropdownGatherEntries runs its listeners last-registered first (the other events keep the registration order): the EUI NotificationPanel, loaded before the popups, builds the corner's buttons from the entries the others added, so it must run last or the espionage, religion and World Congress buttons are missing",
        place: "exe-api/events.rs",
    },
    SimpRow {
        id: "SIMP-exe-331",
        what: "Network.Send* drops the sending player from the Lua arguments of the messages whose first parameter is the player (SendMoveSpy, the League votes and proposals, SendFoundPantheon, ...): the core's message carries the arguments after it (the sender is the seat); the list is read off rt_net.cpp of the portable runtime, not off the exe's Lua library",
        place: "exe-api/network.rs",
    },
    SimpRow {
        id: "SIMP-exe-360",
        what: "SetVoid1 / SetVoid2 / SetVoids take a nil as 0 (the exe reads the voids with lua_tointeger; EconomicGeneralInfo and NotificationLogPopup of the original set a void to an undefined global and would stop their main chunk with an error here)",
        place: "ui/lua_api.rs",
    },
    SimpRow {
        id: "SIMP-exe-361",
        what: "The scratch deal (UI.GetScratchDeal) is the replica's real deal once the game's Lua libraries are attached (K12, SIMP-exe-300); before that (and in tests) it is a counted no-op object whose Get* methods answer 0 and Is*/Can*/Has* methods false (no nil), so DiploRelationships runs. UI.DoFinalizePlayerDeal and UI.HasMadeProposal fall back the same way (nothing / -1, counted)",
        place: "exe-api/ui_api.rs",
    },
    SimpRow {
        id: "SIMP-exe-332",
        what: "UI.GetLastSelectedUnit answers the head selected unit (the exe remembers the unit selected before the city screen; CityView.lua asks for it when the screen is left)",
        place: "exe-api/ui_api.rs",
    },
    SimpRow {
        id: "SIMP-exe-420",
        what: "Player:Cities() and Player:Units() list nothing until the first announcement of the loaded units and cities is done (the rebuild events): in the exe the UI contexts load before the game graphics are set up and meet an empty game, here the replica holds the game already and EUI's ribbons (UnitPanel: g_cities.Initialize) held every unit and city twice, once from the load and once from the Created events (golden S1/S2, the ribbons of the left column)",
        place: "exe-api/lib.rs",
    },
    SimpRow {
        id: "SIMP-exe-421",
        what: "A shown tooltip's position is checked against the screen edges again every frame, after the listeners ran (the exe checks on each mouse move and after the text is set): a tip that is set up after the move (EUI unit panel tooltip) otherwise hangs off the screen",
        place: "ui/tooltip.rs",
    },
    SimpRow {
        id: "SIMP-exe-422",
        what: "ToolTipType controls built after the tooltip context was set up are initialised at the next UI update, not at once (a label that is not initialised has size 0 and the tip's box made 16x16)",
        place: "ui/build.rs",
    },
    SimpRow {
        id: "SIMP-exe-423",
        what: "The client picks the `_small` variant of a UI context's xml (UnitPanel_small, CityView_small, ...) in the in-game layer, as the goldens show (S2, S3: the original ran with the small UI assets); the exe's rule (resolution, option) is not read off the exe",
        place: "ui/shared.rs",
    },
    SimpRow {
        id: "SIMP-exe-424",
        what: "__exe_camera_place(x, y, px, py, level) puts a plot at a screen fraction at a zoom level; a client-only hook for the golden scenarios (m5-goldens), the exe has no such call",
        place: "exe-api/ui_api.rs",
    },
    SimpRow {
        id: "SIMP-exe-425",
        what: "Known differences to the goldens that are not worth fixing (FLIP of whole frames is dominated by them, see tools/playtest/m5/thresholds.json): the terrain, water, clouds, sun rays, shadows and territory tint of the B3 renderer (it is not the original's), the 3D city and unit models, the multiplayer list shows the sign-in nick (the golden: the player name), the CivPanel and ribbon rows at the left edge differ in a few pixels of spacing",
        place: "tools/playtest/lib/goldens.mjs",
    },
    SimpRow {
        id: "SIMP-exe-540",
        what: "UI.SetCityScreenUp(true) takes the camera to the head selected city (the exe's city screen centres on its city whichever way it was opened; the banner click already did through DoSelectCityAtPlot), the zoom level stays SIMP-exe-72's",
        place: "exe-api/ui_api.rs",
    },
    SimpRow {
        id: "SIMP-exe-541",
        what: "Player:GetRecommendedWorkerPlots / GetRecommendedFoundCityPlots answer an empty table when the portable engine UI finds no worker / settler among its selected units (the original's early out is no value, which InGame.lua OnUpdateSelection walks with ipairs): no tile recommendation marks on the map (fork, LEKMOD_PORTABLE in CvLuaPlayer.cpp)",
        place: "fork: LEKMOD_DLL/.../Lua/CvLuaPlayer.cpp",
    },
    SimpRow {
        id: "SIMP-exe-542",
        what: "The core composes the notification texts with plural and gender choices evaluated by a small rule (a number picks its own count, a string argument is singular, a gender choice takes `other`) and the number formats ({1_Num: number #.##}) not evaluated: the full grammar is the `loc` crate's, which the core does not link (fork: headless/portable/firaxis/CvLocalization.cpp)",
        place: "fork: headless/portable/firaxis/CvLocalization.cpp",
    },
    SimpRow {
        id: "SIMP-exe-543",
        what: "UI.DoSelectCityAtPlot on a city of a city-state raises the popup BUTTONPOPUP_CITY_STATE_DIPLO (Data1 = the city-state's player id), the one the notification panel raises; the exe's own conditions (met team, turn state) are not replicated, the Lua caller (CityBannerManager.OnBannerClick) checks the met team",
        place: "exe-api/ui_api.rs",
    },
    SimpRow {
        id: "SIMP-exe-480",
        what: "Network.SendUpdatePolicies(policyOrBranch, isPolicy, true) (the Lua of SocialPolicyPopup) goes to the core as (isPolicy, policyOrBranch, true), the order of the DLL's message; the one Network.Send* whose Lua order differs from gDLL->send* (a click on a policy beyond the first sent policy 1)",
        place: "exe-api/network.rs",
    },
    SimpRow {
        id: "SIMP-exe-481",
        what: "The trade screen's equalize buttons (UI.DoEqualizeDealWithHuman / DoWhatDoesAIWant / DoWhatWillAIGive) change the scratch deal in the replica core and then fire an AILeaderMessage of the trade state with a leader line drawn from Diplomacy_Responses (no deal possible / deal unchanged / counter-deal): the core returns only ok, so 'cannot match' reads as 'no deal possible'; the exe's own body was not reverse-engineered",
        place: "exe-api/ui_api.rs",
    },
    SimpRow {
        id: "SIMP-exe-560",
        what: "The camera events of WorldView.lua's keys (SerialEventCameraStartMoving*/StopMoving*, SerialEventCameraIn/Out) move the camera through the map controller's own arrow keys and zoom steps (its speed and step), not through the exe's camera",
        place: "client-app/m2.rs",
    },
    SimpRow {
        id: "SIMP-exe-561",
        what: "The exe's hot keys (CvGame::GetAction + handleAction on a key no Lua handler took) run as Lua over GameInfoActions and Game.CanHandleAction / Game.HandleAction; a control action goes through Game.DoControl (END TURN is the turn handshake, SIMP-exe-65); a held key does not repeat its action; the numpad direction move (InterfaceBuddy::DirectionAction) does not check whether the unit is on screen; the 'core executing' gate is SIMP-exe-960",
        place: "exe-api/hotkeys.rs",
    },
    SimpRow {
        id: "SIMP-exe-562",
        what: "UI.LocationSelect (a click on a plot of the map) selects the first own unit the plot lists, the next one when the selected unit is there, else opens an own city of the plot; the exe's choice among the units (order, cargo, the Ctrl/Alt/Shift group selections) is not read",
        place: "exe-api/ui_api.rs",
    },
    SimpRow {
        id: "SIMP-exe-563",
        what: "TTManager:GetTypeControlTable of a tooltip type that is not built yet (EUI_context's types wait for the basic controls, SIMP-exe-73) fills the table on its first read; the exe's manager has every type when its XML is read (the unit flag tooltip of UnitFlagManager.lua raised 'attempt to index field Text')",
        place: "ui/lua_api.rs",
    },
    SimpRow {
        id: "SIMP-exe-570",
        what: "A human's turn complete coming or taken back (Player:HasReceivedNetTurnComplete) is read off the replica every frame, not from the net message: one Events.RemotePlayerTurnEnd (CvPlayerController::SetReceivedTurnComplete -> InterfaceBuddy::PublishRemotePlayerTurnEnd) and one SerialEventEndTurnDirty (NetTurnComplete / NetTurnUnready Execute) stand for all the changes of a frame",
        place: "exe-api/network.rs",
    },
    SimpRow {
        id: "SIMP-exe-580",
        what: "The selection sound of a unit of the seat (UnitGameplay2DScripts of its type) plays on every change of the head selected unit; a unit's first selection plays its FirstSelectionSound (the BIRTH script) instead, decided by the DLL's own ever-selected counter (CvUnit:IsFirstTimeSelected, saved with the unit, so a reload does not replay it; a core without that Lua method falls back to a count of this client's session, which does replay it once per unit after a reload). A selection made while the replica loads is silent as in the exe's loading screen; the exe's other conditions were not read",
        place: "exe-api/core.rs, exe-api/engine_events.rs",
    },
    SimpRow {
        id: "SIMP-exe-581",
        what: "UI.UpdateCityScreen raises the city-screen dirty bit; the exe's other effects (the city view's redraw order) are not replicated",
        place: "exe-api/ui_api.rs",
    },
    SimpRow {
        id: "SIMP-exe-582",
        what: "City ambience: while the city screen is up, the looping AS2D_AMBIENCE_CITY_ERA_<ERA>_<SMALL|LARGE> script of the owner's era and the city's size (population / 7, as CvCity::getCitySizeType) plays and stops on exit; Era_CitySoundscapes names (ASSS_CITY_*_SELECT_AMB) have no script in the game files, so the era scripts are used directly (the post-modern era, which has no script, plays the modern one: SIMP-exe-1000). The world ambience near cities on the map (camera distance, fMinGridDistance) and its mixing with the AMBIENCE volume knob are not wired",
        place: "exe-api/core.rs, exe-api/engine_events.rs, client-app/ui_sound.rs",
    },
    SimpRow {
        id: "SIMP-exe-620",
        what: "The yield icons follow the plots' yields by a read, not by the exe's signals: the replica never runs the DLL's CvPlot::updateSymbols for a belief, a policy or a tech, so the yields (CalculateYield) of the shown plots are read at most every 20 frames after an event of the game (and every 90 frames anyway) and a plot whose numbers changed gets HexYieldMightHaveChanged and ShowHexYield again; the camera rectangle comes from the view's frustum on the ground planes (the exe's GameViewState span), not from the exe's own globals; the stored map options (grid, yield icons, resource icons) are applied once at the first setup of a game because the panel's own apply can come before the map exists",
        place: "exe-api/yields.rs, exe-api/events.rs, client-app/m2.rs",
    },
    SimpRow {
        id: "SIMP-exe-590",
        what: "Automatic unit cycling (auto unit cycle option on): the head selected unit that was ready for orders and is not any more (or is gone) makes the exe API select the next unit (SIMP-exe-634) once per frame, read off the replica; the exe calls CvGame::updateSelectionList from its own events, and selects the settler on turn 0, which is not replicated (the start of a turn: SIMP-exe-631)",
        place: "exe-api/cycle.rs",
    },
    SimpRow {
        id: "SIMP-exe-630",
        what: "A load of the replica (each turn's snapshot, a resync) raises the unit-info dirty bits (SerialEventUnitInfoDirty) once: the DLL's turn sets them in CvPlayer::doTurnUnits and through the selected unit's new moves, which the replica does not run",
        place: "exe-api/lib.rs",
    },
    SimpRow {
        id: "SIMP-exe-631",
        what: "Auto unit cycle at the start of a turn: on the first free frame after a load the next unit is selected when the seat has no head selected unit or it is not ready for orders; the exe arms its cycle counter at setTurnActive only for an empty selection",
        place: "exe-api/cycle.rs",
    },
    SimpRow {
        id: "SIMP-exe-632",
        what: "The auto unit cycle waits while UI messages of the seat are out and not back in the journal (at most 3 s, replica::OWN_PENDING_MAX_MS), since the replica shows a unit just ordered as still ready; popups up or waiting, the city screen and PLACE_UNIT mode hold it as in the exe, a selected city (isCitySelection) does not",
        place: "exe-api/cycle.rs, replica/machine.rs",
    },
    SimpRow {
        id: "SIMP-exe-742",
        what: "Player:HasReceivedNetTurnComplete is also true for a human whose END_TURN command the replica's journal applied with no pump after it yet (the core sets the flag when its pump delivers the net message; the pump after the last human's end of turn is the AI phase, which reaches the replica only as the next turn's snapshot); the exe sets it when NetTurnComplete arrives",
        place: "replica/machine.rs, exe-api/network.rs, exe-api/lib.rs",
    },
    SimpRow {
        id: "SIMP-exe-633",
        what: "Network.HasSentNetTurnAllComplete is read off the replica (the seat sent its end of turn and every human seat has HasReceivedNetTurnComplete), not set by NetTurnAllComplete; Network.SendTurnUnready answers false then, true otherwise (the exe's further checks, the turn timer and its interface predicates, are not replicated)",
        place: "exe-api/network.rs",
    },
    SimpRow {
        id: "SIMP-exe-634",
        what: "The auto unit cycle selects the seat's unit ready for orders nearest to the one left (ties: the lower id) through the replica's selection (UI.SelectUnit) and looks at it, or clears the selection when none is left; the DLL's Game.CycleUnits (CvUnitCycler order) is not called because its selectUnit bumps the unit's m_iEverSelectedCount, which the replica digest checks (the replica diverged from the server)",
        place: "exe-api/cycle.rs",
    },
    SimpRow {
        id: "SIMP-exe-610",
        what: "Multiplayer Auto End Turn (the client's option, kept in the page's store for every game): both paths of CvGame::updateTestEndTurn (Part A: no unit moved yet or 10 turn slices of 0.1 s frame time (one per frame at most) after the last unit move of any unit, nothing selected; Part B: the seat gave a manual order, 3 frames) once the seat can end the turn; the unit move is the replica's UnitMoved, the manual order is the seat's sendPushMission / sendAutoMission / sendSwapUnits; the AI-processed checks are the server's, see SIMP-exe-1020..1022 for the rest",
        place: "exe-api/auto_end.rs",
    },
    SimpRow {
        id: "SIMP-exe-611",
        what: "Events.MultiplayerGamePlayerDisconnected / MultiplayerGamePlayerUpdated are raised from the lobby's list of seats on line and the replica's human seats, read once per frame (a seat that drops off line or goes to the AI on a kick is disconnected; every change is an update), not from the net layer's connection callbacks",
        place: "exe-api/network.rs",
    },
    SimpRow {
        id: "SIMP-exe-612",
        what: "A camera move of the core (LOOK_AT, CENTER_CAMERA) raised while the replica replays a journal batch that holds other humans' orders and none of the seat's is dropped: the core runs a human's order with that human as the active player (ActiveAs), so CvUnit::found and the combat code looked at another player's plot; in a batch with orders of both the seat and others the camera moves are kept",
        place: "replica/engine.rs",
    },
    SimpRow {
        id: "SIMP-exe-613",
        what: "GameplayBridgeChanged (the active team gets or loses bridges over rivers, and their style from the era LAST_BRIDGE_ART_ERA) is not delivered by a replica that loads each turn's snapshot: the exe API reads Teams[active]:IsBridgeBuilding() and :GetCurrentEra() after each load and every 30 frames and tells the host when the pair changed",
        place: "exe-api/bridge.rs",
    },
    SimpRow {
        id: "SIMP-120",
        what: "Bridges over rivers follow the viewer's bridge state (GameplayBridgeChanged: has the technology with BridgeBuilding, iron from LAST_BRIDGE_ART_ERA) once the host tells it; until then (no replica: the interim UI, tests) every road over a river has a bridge, iron when the plot's owner is of the Industrial era or later",
        place: "render/world/landmark/routes.rs",
    },
    SimpRow {
        id: "SIMP-121",
        what: "The plain road and railroad are laid as the first border piece of roadsandrails.gr2 (Road_HB1 / RR_HB1: the gravel strip), by the look of the S3 capture; the original's road type table (cvGameplayRoadHandler, RW-17) is not read",
        place: "render/world/landmark/routes.rs",
    },
    SimpRow {
        id: "SIMP-exe-670",
        what: "UI.LookAtSelectionPlot looks at the plot of the head selected unit, else of the head selected city (the exe's getSelectionPlot); it is a plain look at, not CITY_ZOOM_IN with the zoom level of the city-distance high-water mark",
        place: "exe-api/ui_api.rs",
    },
    SimpRow {
        id: "SIMP-uic-12",
        what: "A left press that became a drag (past the 4 px threshold) ends with a button release sent to the UI layer off-screen, so no control (city banner) gets a click and the world's button-up handler does not run; the exe drops the click in its camera drag code, the layer here has no such state to cancel",
        place: "client-app/app.rs",
    },
    SimpRow {
        id: "SIMP-exe-680",
        what: "The files of every notification, popup and event sound are fetched when the sound table arrives (about 15 small files), so a rare sound (a barbarian camp found, an era begun) is loaded when it is first called; the exe loads its banks by its own policy and keeps them",
        place: "audio/engine.rs, client-app/ui_sound.rs",
    },
    SimpRow {
        id: "SIMP-exe-681",
        what: "The seat's own new unit or city plays the sound of EventSoundConnections.xml (UnitCreated AS2D_BUILD_UNIT, CityCreated AS2D_BUILD_CITY) when the core announces it; the exe's own conditions (which creations, loads and upgrades sound) are not traced",
        place: "exe-api/engine_events.rs, client-app/m2.rs",
    },
    SimpRow {
        id: "SIMP-exe-682",
        what: "A popup sounds (ButtonPopupSounds.xml: new era, barbarian camp reward, tech award, wonder completed) when the core announces it, not when the exe's popup queue shows it, and the popup type has no other condition",
        place: "exe-api/engine_events.rs, client-app/m2.rs",
    },
    SimpRow {
        id: "SIMP-exe-660",
        what: "Trade route lines: the client asks the replica core for the routes' plot lists (Q_TRADE_PATHS, TradeConnection::m_aPlotList, the path of the DLL's trade route path finder) every 2 s (every 0.25 s while a route of the view has none) and lays each route of the server's view on the path whose ends are its two cities; the exe gets the same plots pushed by TradeVisuals_NewRoute / UpdateRouteDirection / DestroyRoute, which the client does not consume (the replica's UI event carries 9 plots at most). A route the replica has not replayed yet is the hex line between its cities (SIMP-069) for that moment; the popup route of the caravan's target choice is drawn (SIMP-exe-921)",
        place: "client-app/bridge/trade.rs, client-app/app.rs, client-app/m2.rs, exe-api/lib.rs",
    },
    SimpRow {
        id: "SIMP-exe-650",
        what: "In a running game PreGame.GetCivilization(player) answers Players[player]:GetCivilizationType() (the replica's pregame table gave one shared civilization for the AI seats, so the civ icons of the turn queue, diplomacy and score lists were one emblem); the exe reads the same from its own pregame record",
        place: "exe-api/m4_wog.rs",
    },
    SimpRow {
        id: "SIMP-exe-690",
        what: "The first trap of the wasm module is handed to the page's crash report by a line patched into wasm-bindgen's generated glue (__wbg_handle_catch calls globalThis.__civ5OnTrap(e, alreadyTerminated) before it marks the module terminated); wasm-bindgen's own abort hook gets no error and fires only with panic=unwind; another glue without that one line is left as it is (a build warning)",
        place: "xtask/main.rs, apps/web/www/bootstrap.js, jobs-worker.js",
    },
    SimpRow {
        id: "SIMP-exe-691",
        what: "After the module is terminated the page replaces its canvases with copies without listeners and stops the jobs workers, and keeps the later 'Module terminated' errors out of the console; the timers and window listeners of the dead module stay registered (they throw quietly), the page is not recovered: the player reloads",
        place: "apps/web/www/bootstrap.js",
    },
    SimpRow {
        id: "SIMP-exe-692",
        what: "After each frame's new journal batches, before any Lua runs, the cached Lua instances of units and cities (Unit/City.__instances, raw pointers) that no player lists any more (Players[i]:Units(), :Cities()) lose their pointer, so a script that kept a unit killed or consumed in the batch gets 'Not a valid instance' instead of reading freed memory; the exe kept freed objects of its pools mapped (a stale pointer read garbage), and plots, areas and leagues are cleared only on a load",
        place: "exe-api/lib.rs",
    },
    SimpRow {
        id: "SIMP-exe-700",
        what: "Multiplayer Auto End Turn is latched off for the rest of the turn when the player takes the end of turn back (Network.SendTurnUnready from the button, or an order that reopens the turn), whether the end was automatic or by hand; a new turn clears it. The DLL's updateTestEndTurn latches nothing and ends the turn again (deliberate deviation, the players' wish)",
        place: "exe-api/auto_end.rs",
    },
    SimpRow {
        id: "SIMP-exe-701",
        what: "Multiplayer Auto End Turn also waits while the city screen is up or a popup is up, waiting or delayed in the popup queue (diplomacy, trade, tech tree, menus, advisors), the same holds as the unit cycle; the DLL's updateTestEndTurn does not look at windows (deliberate deviation, the players' wish)",
        place: "exe-api/auto_end.rs",
    },
    SimpRow {
        id: "SIMP-exe-702",
        what: "Notification, popup and unit/city-created event sounds are not played while the replica joins (no base yet or journal batches queued: a reload of the page replays the turn's notifications) or loads, and for one frame after; the exe has no such replay, its notifications are born once",
        place: "exe-api/engine_events.rs",
    },
    SimpRow {
        id: "SIMP-exe-716",
        what: "Cyrillic text (chat lines, nicknames, city names, ru_RU texts) draws: the asset pipeline appends the letters U+0410-044F, Ё ё, І і, Ї ї, Є є, Ў ў, Ґ ґ and № to every Tw Cen MT atlas and .ggxml, rasterised from Ubuntu Light at the cap height of the Tw Cen MT size, with the Soft, Shadow and Stroke layers derived from the coverage by a dilation or an offset; the Latin glyphs are the game's own. Original: the game's Tw Cen MT fonts have no Cyrillic (a Russian install ships its own fonts); this install draws such characters as spaces",
        place: "tools/asset-pipeline/src/ui/cyrillic.rs",
    },
    SimpRow {
        id: "SIMP-uic-20",
        what: "Deliberate improvement over the original: the left civilization and city-state list of the Lekmod NotificationPanel (CivPanelL) is moved 80 px to the right when the ui set is built, so it does not lie over the city ribbon of the UnitPanel (a patch of the XML text, the game files are untouched)",
        place: "asset-pipeline/ui/patches.rs",
    },
    SimpRow {
        id: "SIMP-uic-21",
        what: "A label whose text changed its size measures the stack it sits in again (CalculateSize and re-anchoring); the exe's own way is not read off the exe: the EUI civ list has stacks without an ID that the Lua cannot measure, and a left-anchored stack growing left drew the trade icons over the leader portrait",
        place: "ui/layout.rs",
    },
    SimpRow {
        id: "SIMP-uic-30",
        what: "Touch pinch zoom is direct (no easing): the fingers drive the camera, the exe has no touch input; the wheel and the zoom keys ease like the exe",
        place: "render/game_camera.rs",
    },
    SimpRow {
        id: "SIMP-uic-31",
        what: "A drag, an arrow-key scroll and a drag glide set the camera and end any easing (target = camera); what the exe does with its target under them is not traced",
        place: "render/game_camera.rs",
    },
    SimpRow {
        id: "SIMP-exe-750",
        what: "UI.LookAt ignores its type argument: CITY_ZOOM_IN/OUT and SHRINE_ZOOM_IN (heights 500/700/800 and the remembered height) keep the player's zoom, only the pan eases; IMMEDIATE eases like NORMAL (as the exe), only game start teleports",
        place: "client-app/app.rs",
    },
    SimpRow {
        id: "SIMP-exe-740",
        what: "Network.HasSentNetTurnAllComplete counts the seat's own end of turn from the moment the UI sent it (not from the server's echo in the journal): the last human to end the turn is in the 'turn is being processed' state at the click; a refusal of that end of turn takes it back",
        place: "exe-api/network.rs",
    },
    SimpRow {
        id: "SIMP-exe-741",
        what: "'The turn is being processed' (every human has ended the turn, the exe's NetTurnAllComplete) is read off the replica and the UI's own end of turn once per frame and held until the next load: the router sends gameplay messages without a CancelEndTurn ahead of them, Network.SendTurnUnready answers false, and a refused duplicate END_TURN does not reopen the turn",
        place: "exe-api/network.rs, exe-api/corners.rs, replica/machine.rs",
    },
    SimpRow {
        id: "SIMP-130",
        what: "The trading post (ArtDefineTag ART_DEF_IMPROVEMENT_TRADING_POST, which has no ArtDefine_Landmarks rows) draws ART_DEF_IMPROVEMENT_TRADING_POST1..4 as the exe does (CvDLLUtility::GameplayPlotStateChange: tag + itoa(rand() % 4 + 1)); the number is a hash of the plot (mix(cell, 3)), not the exe's rand at each plot state change",
        place: "render/world/landmark/select.rs",
    },
    SimpRow {
        id: "SIMP-131",
        what: "The core's GameplayDoFX reaches the particle system for any plot; CvDLLUtility::GameplayDoFX drops a plot the active team has not revealed, here the fog mask of ART_DEF_VEFFECT_LOOK_AT (FOWMask 3) hides it there",
        place: "exe-api/engine_events.rs",
    },
    SimpRow {
        id: "SIMP-132",
        what: "Events.GameplayFX plays the look-here particles (ART_DEF_VEFFECT_LOOK_AT); its sounds (AS2D_WORLD_RAY_OF_GOD of the VisEffect, AS2D_BUILD_UNIT of the EventEffect) play with it since SIMP-exe-972",
        place: "client-app/app.rs",
    },
    SimpRow {
        id: "SIMP-exe-760",
        what: "UnitMoveQueueChanged (told at the end of a unit's mission) is not told for a unit the core no longer has: the replica applies a journal batch whole before it tells its events, so the unit of a mission that ended with its death (a settler founding a city) is already freed and SIMP-exe-692 cleared its Lua instance; the exe told the event with the unit alive and then destroyed the flag. The UnitFlagManager's FinishMove would raise 'Not a valid instance' on it.",
        place: "exe-api/engine_events.rs, exe-api/core.rs",
    },
    SimpRow {
        id: "SIMP-exe-770",
        what: "Our defaults of the EUI options (the players' wish): the civilization list on, the UnitPanel's unit and city ribbons off, set once per store over what was saved before (a mark in the options keeps the version of the defaults); the exe has no such defaults (the EUI reads an unset option as on)",
        place: "exe-api/modding.rs",
    },
    SimpRow {
        id: "SIMP-exe-780",
        what: "The core's lookAtSelectionPlot (the city screen's previous / next arrows, centre on selection) and the opening of the city screen move the camera to the seat's head selected city (or unit) with a plain look at, not CITY_ZOOM_IN with the zoom of the city-distance high-water mark; only the seat's own selection is looked at, never another player's city",
        place: "exe-api/engine_events.rs, exe-api/ui_api.rs",
    },
    SimpRow {
        id: "SIMP-exe-790",
        what: "While the server plays the AI phase the end-turn button says 'Processing AI turns: N of M' (English only), M the non-human civilizations the server's core started this turn (AI majors, city-states and barbarians) and N the ones it has ended (the turn queue of SIMP-exe-820, so the count is the core's own); the original shows the AI's moves as they are made and keeps 'Please Wait'",
        place: "exe-api/network.rs, game-host/timeline.rs",
    },
    SimpRow {
        id: "SIMP-exe-791",
        what: "The worker pumps the AI phase in calls of at most 4 core slices (not 32) so it takes orders, beats and sends the AI turn events between calls; the core runs the same slices in the same order, the replica journal records the same total",
        place: "game-host/timeline.rs",
    },
    SimpRow {
        id: "SIMP-uic-40",
        what: "The UI's texts (locale:en_US, locale:ru_RU) are the real game's Localization-Merged.db of the DB snapshot when the build gets --db (ui-loc-diff: 0 differences); without a snapshot they are the XML files merged in layer order, which differs from the game in hundreds of texts (the game's loader order is not copied)",
        place: "asset-pipeline/src/ui/build.rs, asset-pipeline/src/ui/oracle.rs",
    },
    SimpRow {
        id: "SIMP-uic-50",
        what: "A client report (POST /client-report/v2) pins the recording of its game (the newest recording of the game it says it was in, and the exact ones it names in recent_recordings) for 14 days, at most 3 pins, and puts a Report record into the live recording; the game number is the report's own claim (not checked against the sender), a report that names no game or whose game has no recording pins nothing",
        place: "server/src/http/reports.rs, server/src/state/retention.rs",
    },
    SimpRow {
        id: "SIMP-uic-51",
        what: "The client's turn timeline stamps its spans with the page's UTC clock (unix time, re-read every frame so a stalled clock does not drift), not corrected to the server's: the gateway clock of Pong is monotonic, not a calendar, so the skew between the player's machine and the server (normally the NTP error) stays and the lines say synced:false; the 'server' bucket is the whole wait from the click to the GcTurn, which the client cannot split",
        place: "client-app/src/timeline.rs",
    },
    SimpRow {
        id: "SIMP-uic-52",
        what: "No 'Send a bug report' item in the Esc menu (the game menu is the original's Lua/XML context): the report dialog opens with Ctrl+Shift+F8, from the crash overlay ('Send a full report') and from window.__civ5log.openReport()",
        place: "apps/web/www/clientlog.js, apps/web/www/bootstrap.js",
    },
    SimpRow {
        id: "SIMP-uic-53",
        what: "The Rust log lines reach the JS ring on a 250 ms timer (and at once for a warning or an error), not once per frame, so a page that draws no frames still logs; at most 250 ms of lines wait in the wasm memory",
        place: "platform-web/src/log.rs",
    },
    SimpRow {
        id: "SIMP-uic-54",
        what: "?trace=fine records the same spans as coarse: the per-Lua-handler spans (c.lua.<event>) and the per-render-scope spans of the latency spec are not recorded",
        place: "client-app/src/timeline.rs",
    },
    SimpRow {
        id: "SIMP-exe-810",
        what: "The recording of a game is a directory per game life (recordings/g<game>-<created ms>); retention counts every life as one of the 'newest 5 games' (a game that slept and woke twice is three entries), and there is no index.json: the store is the directory listing",
        place: "server/src/game/recording.rs, server/src/state/retention.rs",
    },
    SimpRow {
        id: "SIMP-exe-811",
        what: "The recording stops (and the manifest says why) when the disk under it has less than [recording] min_free_mb (default 512 MiB) free; the free space is asked every 30 s with df, so where df is not there (not unix) the byte caps alone hold, and a disk that fills between two asks is not noticed",
        place: "server/src/game/recording.rs, recording/src/writer.rs",
    },
    SimpRow {
        id: "SIMP-exe-812",
        what: "BatchMeta (size and part tags of every GameBatch sent to a human seat) is written, but within a fixed budget per game life (8 MiB of estimated raw bytes, shared with the order spans, SIMP-exe-982): past it the records are dropped and counted, so a long game keeps the batches of its first hours only; the tags are the GameMsg tags of the parts (view message 0 full / 1 delta), not the exact bytes of each part",
        place: "recording/src/format.rs",
    },
    SimpRow {
        id: "SIMP-exe-813",
        what: "The recording keeps one OrderSpan per order (stage cmd.worker: from the order going to the worker pipe to its final answer, microseconds) within the meta budget (SIMP-exe-982), and the turntrace envelope (one Trace record per end of turn) with the gateway clock on every record; cmd.recv and cmd.queue are not separate events (the Order record and its stamp give the receive time)",
        place: "server/src/game/trace.rs, recording/src/format.rs",
    },
    SimpRow {
        id: "SIMP-exe-814",
        what: "civ5-replay diff compares two core builds by running one civ5-replay per build (the core is a singleton per process) and comparing their digest lines, not in one process in lockstep; hang names the stack with gdb when it is installed and the wait channels of the threads otherwise",
        place: "tools/civ5-replay/src/main.rs",
    },
    SimpRow {
        id: "SIMP-exe-815",
        what: "The worker marks a core call before it runs (RecMark) when it is in the AI phase (at most twice a second); a call of any other phase (an order, a query) is marked by the watch thread once it has run a second and closed when it returns, so a call shorter than a second leaves no mark and a hang in it is named by the heartbeat log until it has run that long",
        place: "server/src/worker/watch.rs",
    },
    SimpRow {
        id: "SIMP-exe-816",
        what: "civ5-replay verify compares the replica digest at the server's check records (about once a second of human play) and at every keyframe, and the core's save hash at a keyframe when the core can save there; a difference inside the AI phase that heals before the next keyframe is not seen, and a turn whose keyframe body was compacted away is compared at the next kept one",
        place: "tools/civ5-replay/src/verify.rs",
    },
    SimpRow {
        id: "SIMP-exe-820",
        what: "The turn queue of the AI phase: the original DLL activates every AI at the start of the new turn and deactivates each when it has moved, and the exe raises the queued Events.AIProcessingStartedForPlayer / AIProcessingEndedForPlayer(id, \"\") for each setTurnActive (Lekmod MPTurnPanel lights and darkens the AI icons, EUI's civ list highlights); the replica does not play the AI phase, so the server's core reports each non-human setTurnActive (host event 16) and the client raises the two events in that order, with an overlay over Player:IsTurnActive (set before the event is handled, gone at the next load, whose flags have every AI done) until then",
        place: "exe-api/network.rs, exe-api/lib.rs, client-core/stream.rs, game-host/timeline.rs, game-host/real.rs",
    },
    SimpRow {
        id: "SIMP-exe-821",
        what: "The AI turn events reach the clients when the server's pump call that did them returns (calls of at most 4 slices, SIMP-exe-791), not at the instant of the core's setTurnActive, so several Ended can arrive together; the start burst keeps the ascending id order, not the DLL's shuffled order; events of the humans are not sent (the replica's own turn events cover them); a client that joins in the middle of the phase hears only the rest of it",
        place: "game-host/timeline.rs, exe-api/network.rs",
    },
    SimpRow {
        id: "SIMP-exe-830",
        what: "civ5-replay verify replays one worker as one process, as the server runs it: it loads the snapshot where a worker began and goes on with its own game at every later keyframe that equals the server's by the core's save hash; --reload loads every snapshot, and an AI phase that then differs is reported, not fixed: the core keeps state no save holds (after the fork's fix for the military AI counts, a homeland patrol move at turn 131 and 14 plot bytes at turn 133 of game 936 still differ after a load)",
        place: "tools/civ5-replay/src/verify.rs",
    },
    SimpRow {
        id: "SIMP-exe-831",
        what: "A server restart (a worker that loads its last save) continues the AI as the running worker would only for what the save holds: the fork now also saves the military AI's per-turn unit counts (CvPlayer record version 18) and the asynchronous RNG (GCS1 section async), which the original Civ5 save drops as well, but not every AI cache; the original game has the same property on save and load",
        place: "lek fork: CvMilitaryAI, CvPlayer, rt_save.cpp",
    },
    SimpRow {
        id: "SIMP-exe-840",
        what: "Accounts: the original game has none (LAN and Steam identities), ours are a nick with an argon2id password hash (8 to 128 characters) and up to 16 device tokens stored as sha256; no e-mail, no password change, no recovery except the admin reset of an account",
        place: "server/src/identity.rs",
    },
    SimpRow {
        id: "SIMP-exe-841",
        what: "(Superseded by SIMP-exe-854: the token now rides in an HttpOnly cookie.) The device token went to the page as JSON and stayed in localStorage",
        place: "server/src/http/auth.rs",
    },
    SimpRow {
        id: "SIMP-exe-842",
        what: "One live connection per account: a new login of an account (password or token) replaces the live one (Detached Replaced); several devices at once need the seat fan-out of the multi-device step",
        place: "server/src/identity.rs, server/src/session/login.rs",
    },
    SimpRow {
        id: "SIMP-exe-843",
        what: "auto_register: on a dev or test server (--dev, the in-process test config) the first WebSocket password login of an unknown nick creates the account, with the registration gate and without the password length rule; the deploy profiles keep it off",
        place: "server/src/session/login.rs, server/src/config.rs",
    },
    SimpRow {
        id: "SIMP-exe-844",
        what: "Wrong passwords only slow the answers (per address a growing delay and a 10 minute block, per account a growing delay from any address) and registrations are capped (5 per address and 50 in all per hour); an account is never locked",
        place: "server/src/session/limits.rs",
    },
    SimpRow {
        id: "SIMP-exe-845",
        what: "A legacy nick (migrated from identities.json, no password yet) that holds no seat, room or live session can be claimed by anyone who registers it, as it was free for anyone to log in with before accounts; with a seat it needs the old token or the admin",
        place: "server/src/identity.rs",
    },
    SimpRow {
        id: "SIMP-exe-850",
        what: "The custom options of a map script (World Age, Sea Level, ...) are read from the MapScriptOptions and MapScriptOptionPossibleValues tables of the game data (what the exe stored from GetMapScriptInfo().CustomOptions when it scanned the maps), at Describe, not by running each script's GetMapScriptInfo() in the core; a script that is not in the tables has none; hidden options are not offered (the core applies their default), a read-only one is offered but fixed at its default",
        place: "lek fork: gc_query.cpp (Q_CATALOG tables 22, 23); game-host/lobby_options.rs",
    },
    SimpRow {
        id: "SIMP-exe-851",
        what: "Every GameOptions row of the game data can be set in the room: the rows the Lekmod multiplayer screen does not show (Visible = 0 or not multiplayer, e.g. NO_SCIENCE, NO_RELIGION, ALWAYS_WAR) are offered under 'More' (group Hidden), which the original screen cannot do; the brain's own rows (PITBOSS, END_TURN_TIMER_ENABLED, QUICK_COMBAT, QUICK_MOVEMENT, DYNAMIC_TURNS, SIMULTANEOUS_TURNS, FIRSTMOVE, FAST_HAND) are never offered",
        place: "game-host/lobby_options.rs, server lobby/validate.rs",
    },
    SimpRow {
        id: "SIMP-exe-852",
        what: "The city-state slider ends at 41, the core's MAX_MINOR_CIVS, not at the 58 rows of MinorCivilizations; the room starts with the map size's default count and the core draws the chosen number from the seed (as before)",
        place: "game-host/lobby_options.rs (CORE_MAX_MINOR), game-host/real.rs",
    },
    SimpRow {
        id: "SIMP-exe-853",
        what: "Max turns is a whole number 0 to 2000 in the room (0 = the game speed's own limit, PreGame.SetMaxTurns(0)); the original edit box takes any positive number; the victory check boxes are the five rows of Victories without VICTORY_SCRAP, as the original, but they ride in game_options (VICTORY_*, MAX_TURNS) and are split into the core's victories and max_turns keys at the game start",
        place: "server lobby/validate.rs, game-host/lobby_options.rs, lek fork: rt_setup.cpp",
    },
    SimpRow {
        id: "SIMP-uic-60",
        what: "The sign-in, the lobby list and the room of the web client are a DOM page (forms, native selects and check boxes, sections as details, a sticky Ready/Start bar) fed with the same view data as the egui screens, not the original FrontEnd Lua screens (MPGameSetupScreen, LobbyScreen, StagingRoom): the original's layout, art and widgets are not reproduced; the native client keeps the egui screens",
        place: "apps/web/www/ui.js, ui.css, client-app/dom_ui.rs",
    },
    SimpRow {
        id: "SIMP-uic-61",
        what: "The native (egui) room shows the new fields (map script options, victories, max turns, grouped options, the AI's choices) as a plain list under the old settings, without the sections and the sticky bar of the web page; options of the AI and Additional groups still appear only while their switch is on",
        place: "interim-ui/screens/room.rs",
    },
    SimpRow {
        id: "SIMP-uic-62",
        what: "The sign-in is our own account page (nick and password, register with an optional code, the browser's password manager through real forms and PasswordCredential); the original game has no accounts. The session is an HttpOnly cookie (SIMP-exe-854); the page keeps only the nick",
        place: "apps/web/www/auth.js, client-app/net_ui.rs",
    },
    SimpRow {
        id: "SIMP-uic-63",
        what: "Victory conditions and the turn limit are not separate fields of the room: they ride in GameSettings::game_options as (VICTORY_x, 0|1) (a missing victory is on) and (MAX_TURNS, n) (absent = the speed's own limit), so saves keep their format; the page offers a limit of 1 to 2000 turns",
        place: "client-app/net_ui.rs, protocol/lobby.rs",
    },
    SimpRow {
        id: "SIMP-uic-64",
        what: "A new room starts with the default of each editable custom option of its map script, and changing the map script resets them to the new script's defaults (read only options are not sent); the original keeps the last value of options with the same index",
        place: "client-app/net_ui.rs, autostart.rs",
    },
    SimpRow {
        id: "SIMP-uic-65",
        what: "The number of city-states in the room goes up to the chosen map size's maximum (the server refuses more), not to the number of city-states in the game data as the original slider does",
        place: "client-app/net_ui.rs",
    },
    SimpRow {
        id: "SIMP-exe-854",
        what: "The web client's device token lives in an HttpOnly cookie (civ5_session=<nick>.<token>, 90 days, SameSite=Lax, Secure) that script cannot read, and the WebSocket Login of such a session carries no token and no password: the gateway takes the cookie of the upgrade request, and LoggedIn hands the page no token (protocol v9, Session.token = None). The original game has no accounts",
        place: "server/src/cookie.rs, server/src/session/login.rs, server/src/http/auth.rs, platform-web/store.rs",
    },
    SimpRow {
        id: "SIMP-exe-855",
        what: "The Secure attribute of the session cookie is a server setting (cookie_secure, on by default); a server reached over plain http on a LAN address (--dev, or cookie_secure = false) turns it off, because a browser drops a Secure cookie there; every https profile keeps it",
        place: "server/src/lib.rs, server/src/config.rs, deploy/server.toml",
    },
    SimpRow {
        id: "SIMP-exe-856",
        what: "No CSRF token: the auth POSTs and the WebSocket upgrade are guarded by SameSite=Lax on the cookie and by the Origin check (a present Origin must equal the page host or X-Forwarded-Host); a request without Origin (curl, bots, native clients) is allowed",
        place: "server/src/http/auth.rs, server/src/http/mod.rs",
    },
    SimpRow {
        id: "SIMP-exe-857",
        what: "POST /api/auth/adopt turns the device token that an earlier build kept in localStorage (civ5.token.<nick>) into the session cookie, once per browser at page load; the page then deletes every civ5.token.* key, also when the server refuses it. To be removed when no old token is left",
        place: "server/src/http/auth.rs, apps/web/www/auth.js",
    },
    SimpRow {
        id: "SIMP-exe-858",
        what: "Registration on a public server (deploy/compose.public.yaml) needs an invitation code, the shared server password CIV5_PASSWORD (\"Код приглашения\" field, shown when /api/auth/info says registration is \"code\"), instead of open registration; the code is no login password, and the deploy takes it from CIV5_PASSWORD or stdin and writes it only into civ5.env on the host",
        place: "deploy/compose.public.yaml, server/src/auth.rs, tools/xtask/src/deploy.rs",
    },
    SimpRow {
        id: "SIMP-exe-859",
        what: "A net-test bot registers over HTTP with the code and reads its device token out of the Set-Cookie header (it speaks to an https host through the runner's local TLS front); it logs in over the WebSocket with that token as a native client does, not with the cookie",
        place: "tools/net-test/src/bot/register.rs",
    },
    SimpRow {
        id: "SIMP-exe-900",
        what: "GCN1 (the UI's net messages) carries no turn, so the guard is on the client: UI messages given while the replica shows a turn the server's view has already left are dropped with a log line (stat gc_net_stale) and are not kept for the next turn; END_TURN / CancelEndTurn orders are stamped with the replica's turn, so the server refuses them as TurnChanged. The exe has no such window (its turn change is synchronous)",
        place: "client-core/src/lib.rs (send_gc_net_at, order_at), client-app/src/m2.rs",
    },
    SimpRow {
        id: "SIMP-exe-901",
        what: "A GcResync inside the server's 2 s per-seat gap is deferred and served by the next tick instead of dropped; a resync whose hand-over to the worker fails (worker restarting) is kept for the next tick and does not count against the gap. A UI message (GcNet) that cannot be handed to a restarting worker is answered with a private line to the player: the action was not applied. No counterpart in the exe",
        place: "server/src/game/actor.rs, server/src/game/replica.rs",
    },
    SimpRow {
        id: "SIMP-exe-902",
        what: "UI.DoFinalizePlayerDeal given before the replica attached its GcUi* (late attach after a reload) waits (newest per pair, at most 8) and goes out with the next deal call after the attach; the exe has no attach step",
        place: "exe-api/src/ui_api.rs",
    },
    SimpRow {
        id: "SIMP-exe-940",
        what: "The replay tab's map (ReplayMap) draws every plot as a plain rectangle of the colour the Lua gives, upside down so the north is on top, the rows not staggered; the exe draws terrain-shaded hexes",
        place: "ui/tree.rs, ui/draw.rs",
    },
    SimpRow {
        id: "SIMP-exe-941",
        what: "Core UI kinds without a listener: OpenTechTree, LookAtCityOffset and Flashing are declared by the DLL's interface but never called by this DLL; ClearEventMessages only runs in hot-seat and play-by-mail games; AddHexToUiRange feeds the exe's own move-range overlay, which the client computes itself (path.rs); the rest of the list (spaceship, wonders, trade routes, borders, strategic view) is render-only. All are counted, nothing more",
        place: "exe-api/engine_events.rs",
    },
    SimpRow {
        id: "SIMP-exe-942",
        what: "UI.GetReplayInfo and the front-end replay file functions (GetReplayFiles, GetReplayFileHeader, DeleteReplayFile, ...) do not exist: the end-game replay tab builds its data from the running game (Game.GetReplayMessages, Player:GetReplayData) and there is no main-menu list of saved replays",
        place: "exe-api/m4_wog.rs",
    },
    SimpRow {
        id: "SIMP-exe-970",
        what: "Unit selection sound: the exe's default AS2D_BUILD_UNIT plays for a unit type with no UnitGameplay2DScripts row (great people, space parts); a row whose column for this selection is empty plays the other column, then the default (the exe's handling of a NULL column is not traced; the shipped data fills both)",
        place: "exe-api/core.rs",
    },
    SimpRow {
        id: "SIMP-exe-971",
        what: "City screen ambience in the post-modern era (ERA_POSTMODERN of Brave New World): superseded by SIMP-exe-1000 (it was silent, because the exe builds AS2D_AMBIENCE_CITY_<era type>_<size> and the game has no such script)",
        place: "exe-api/core.rs",
    },
    SimpRow {
        id: "SIMP-exe-972",
        what: "The look-here effect (a ruin found, a clicked notification with a plot) plays AS2D_WORLD_RAY_OF_GOD and AS2D_BUILD_UNIT with its sparkle (the VisEffect's and the EventEffect's sounds from the art defines); that the exe plays both for this event follows from the players' report that the sparkle has a sound, not from a trace",
        place: "exe-api/events.rs, asset-pipeline/ui/audio.rs",
    },
    SimpRow {
        id: "SIMP-exe-973",
        what: "The hold on notification, popup and event sounds while the replica catches up (SIMP-exe-702) ends after 600 frames (about ten seconds) of one catch-up; the exe has no such hold. Until then a replica that stayed out of the live state silenced the rest of a turn after a reload of the page",
        place: "exe-api/lib.rs",
    },
    SimpRow {
        id: "SIMP-exe-974",
        what: "Sound files are fetched by priority: a file a call waits for first, the files fetched ahead (notification, popup, event sounds, the selection sounds of the unit types the seat has on the map) at most three at a time; a failed fetch is asked for again by the next call, three times in all; an encoded file (mp3, ogg) is played once the browser has decoded it. The exe loads its banks by its own policy",
        place: "audio/engine.rs, client-app/ui_sound.rs",
    },
    SimpRow {
        id: "SIMP-exe-920",
        what: "The pixel shader of the trade route lines (TradeArrowShader) is not in the game files: ps_trade reads arrow*.dds as a mask (white arrow on black, alpha 255 everywhere: the red channel is the coverage) in the owner's primary colour times the vertex alpha, instead of texture times colour, which drew the black of the texture as a dark box the width of the route; the UI path of UI.SendPathfinderUpdate is cut at 64 nodes and empty for an air unit (DOMAIN_AIR) as in the exe (that part is exact)",
        place: "render/shaders/overlay/trade.wgsl, render/world/trade.rs, exe-api/ui_api.rs",
    },
    SimpRow {
        id: "SIMP-exe-921",
        what: "ShowAttackTargets and the popup trade route: the attack targets of the ranged attack mode are the core's Q_RANGE_TARGETS (what the range attack accepts now) as attack-only AddUnitMoveHexRangeHex, not the exe's interface path finder flood with AttackPathAdd (order and cut not in the RE answers); the temporary popup route of the caravan's target choice (TradeVisuals_NewRoute(-1, ..)) arrives from the core with its first 9 plots only, so a longer route is those 9 plots and the hex line from the 9th to the plot of Game.SelectedUnit_SpeculativePopupTradeRoute_Display (the exe-api remembers it); the popup of an existing route (selected or hovered trade unit) lies on the core's Q_TRADE_PATHS of that route index, in the colour of the route's owner as found by its two cities",
        place: "exe-api/core.rs, exe-api/engine_events.rs, exe-api/lib.rs, client-app/bridge/trade.rs, render/world/trade.rs",
    },
    SimpRow {
        id: "SIMP-exe-930",
        what: "The city screen camera height is 500 / 700 / 800 (CITY_ZOOM_LEVEL_1/2/3) by the city-distance high-water mark, which the Lua API does not give: it is read off the borders (farthest owned plot within 5 hexes from the nearest own city) and kept as a maximum; the pan offset (y - 20) and the easing of CITY_ZOOM_IN are not taken (supersedes the measured level of SIMP-exe-72)",
        place: "crates/client/exe-api/src/lib.rs (city_zoom_height), crates/client/client-app/src/m2.rs",
    },
    SimpRow {
        id: "SIMP-exe-931",
        what: "The mouse cursor follows UIManager:SetUICursor (the interface modes' cursors and the busy globe, every file animated frame by frame); superseded in its remainder by SIMP-exe-1010",
        place: "crates/client/client-app/src/cursor.rs",
    },
    SimpRow {
        id: "SIMP-exe-950",
        what: "UI.RequestLeaveLeader leaves the leader view at once: it clears the leader-root flag (UI.SetLeaderHeadRootUp(false)) and raises Events.LeavingLeaderViewMode synchronously, which closes the leader root, the discussion dialog and the trade screen (the trade screen's Goodbye returns to the map); the exe's own order and queueing were not recovered",
        place: "exe-api/ui_api.rs",
    },
    SimpRow {
        id: "SIMP-exe-951",
        what: "SerialEventImprovementCreated and SerialEventRoadCreated are told from a read of every revealed plot's improvement and road with the resource read (the replica runs no DLL plot change); the first read only records; the improvement era and state arguments are 0",
        place: "exe-api/yields.rs",
    },
    SimpRow {
        id: "SIMP-exe-952",
        what: "SerialEventCityHexHighlightDirty is raised whenever the core reports a moved border (UPDATE_COUNTRY_BORDER / UPDATE_ALL_COUNTRY_BORDERS: a tile bought, a city founded); the exe's own sources were not recovered",
        place: "exe-api/engine_events.rs",
    },
    SimpRow {
        id: "SIMP-exe-953",
        what: "Events.UnitNameChanged is raised for a unit the seat renamed (Network.SendRenameUnit) when the rename is sent and again after the next batch of the game; the exe raises it from CvUnit::setName",
        place: "exe-api/engine_events.rs, network.rs",
    },
    SimpRow {
        id: "SIMP-exe-954",
        what: "A control method the class lists but the port has no body for does nothing and is logged once per method and class (target ui.lua), instead of a Lua error that stops the handler",
        place: "ui/lua_api.rs",
    },
    SimpRow {
        id: "SIMP-exe-955",
        what: "The save window's name field and delete button and the game menu's quick save (controls that do nothing here) are hidden for good: SetHide(false) on them is ignored",
        place: "ui/shared.rs, ui-client/layer.rs",
    },
    SimpRow {
        id: "SIMP-exe-980",
        what: "10 FX textures named by .psb files (fx_arrowup_01, fx_seagull_01, fx_smokecloud03, fx_tank_treads, fx_trailcloud_01, fx_trailfire_02, fx_unitselectedglow_02, fx_waterfalls01, fx_wave_base01, gui_unitselectedglow_01) are not in the install (the original's files lack them too): the frame has no texture and the particle renderer skips that quad (the effect keeps its other frames); no substitute texture is made",
        place: "asset-pipeline/src/sets/b2_fx.rs, render/world/particle/draw.rs",
    },
    SimpRow {
        id: "SIMP-exe-981",
        what: "224 UI textures named by the mod civs' icon/alpha atlases (Lekmod DLC rows whose files the mod never shipped: Allende/Pinochet, Ayyubid, Bolivia, Finns, Belgium, Minoa, Sioux ...), 12 unit flag textures, oplotflag, yieldatlas_128_tourism and some CityView citizen icons are absent from the install (also in the original: the file names exist in no pack, no case or name variant); those icons draw blank. The 223 atlas rows whose size disagrees with the file are harmless: the client sizes an atlas from the texture, not from the rows",
        place: "asset-pipeline/src/ui/build.rs",
    },
    SimpRow {
        id: "SIMP-exe-982",
        what: "The extra recording records (BatchMeta, OrderSpan) share a fixed budget per game life (8 MiB estimated raw bytes); past it they are dropped and counted (Recorder::meta_dropped), not written, so the journal cannot grow without a limit from them",
        place: "server/src/game/recording.rs",
    },
    SimpRow {
        id: "SIMP-exe-910",
        what: "NOTIFICATION_PLAYER_KICKED / PLAYER_DISCONNECTED (CvPlayer::disconnected adds them to the ACTIVE player; the replica's core adds them to the server seat, so no client saw them): the exe API makes the notification itself when it sees a seat stop being a human (kicked) or drop off line (disconnected), with an id of its own, outside the core's list (a reload drops it); Network.IsPlayerKicked is the same observation. MultiplayerGameLastPlayer, MultiplayerGameHostMigration, MultiplayerPingTimesChanged and MultiplayerGameAbandoned(2) for the kicked seat are raised from the same per-frame look (pings at most every 120 frames)",
        place: "exe-api/network.rs",
    },
    SimpRow {
        id: "SIMP-exe-911",
        what: "AIProcessingStartedForPlayer / AIProcessingEndedForPlayer for the human seats are announced from the replica's Player:IsTurnActive once per frame (the end when every human has ended the turn), not at the core's setTurnActive (the replica loads the new turn as a snapshot and never calls it for the humans); the core's own event for a human is not raised twice",
        place: "exe-api/network.rs",
    },
    SimpRow {
        id: "SIMP-exe-912",
        what: "Network.SendChat to a team: the server only has 'everybody' and 'one player' as addressees, so the client sends one direct line to each human of the team (to itself when it is alone); the team's members read it as a direct line from the sender, the sender sees a line per member; no Team recipient in the protocol (a version change)",
        place: "exe-api/corners.rs",
    },
    SimpRow {
        id: "SIMP-exe-960",
        what: "The exe's 'game core executing' gate of the hot keys (no key while the AI phase runs) is read off the turn handshake (every human has ended the turn, Core::turn_all_complete), not off a core flag; an observer (SS_OBSERVER) passes the turn gate",
        place: "exe-api/hotkeys.rs",
    },
    SimpRow {
        id: "SIMP-exe-961",
        what: "The staging room's civilization tip (leader, traits, unique units and buildings) comes from the first leader of the civilization (the lobby picks a civilization, not a leader); the texts are Q_CATALOG table 24 (fork), the pictures the web-lobby art (SIMP-exe-1050, the card SIMP-exe-1055)",
        place: "client-app/net_ui.rs, apps/web/www/ui.js, fork gc_query.cpp",
    },
    SimpRow {
        id: "SIMP-exe-962",
        what: "Observers (a player who joins a room to watch, holding no seat) are NOT built; hot keys already let an observer slot through the turn gate",
        place: "client-app/net_ui.rs, exe-api",
    },
    SimpRow {
        id: "SIMP-exe-1000",
        what: "City screen ambience in the post-modern era (ERA_POSTMODERN of Brave New World) plays the modern-era script (AS2D_AMBIENCE_CITY_ERA_MODERN_<SMALL|LARGE>): the game data has no script for that era and the exe is silent there as far as the data goes, but the user decided (2026-11-01) that the original cannot be silent and wants the modern sound",
        place: "exe-api/core.rs",
    },
    SimpRow {
        id: "SIMP-exe-1001",
        what: "Per-reward goody hut (ancient ruins) sounds are not played: GoodyHuts.Sound names AS2D_GOODY_WARRIOR/GOLD/MAP/HOSTILE/SETTLER/SCOUT/WORKER/XP/HEAL, but no Script2DSound of that name exists in Audio2DScripts.xml and the DLL never reads CvGoodyInfo::getSound (the exe's own use is not traced), so the original plays at most the ruins-found notification (AS2D_INTERFACE_ANCIENT_RUINS, already wired) and the silent BUTTONPOPUP_GOODY_HUT_REWARD popup. The unit promotion sound (AS2D_INTERFACE_UNIT_PROMOTION) is played by UnitPanel.lua through Events.AudioPlay2DSound and by the promotion notification, both wired (script in the audio set by the AS2D_INTERFACE_ prefix)",
        place: "exe-api/core.rs, asset-pipeline/ui/audio.rs",
    },
    SimpRow {
        id: "SIMP-exe-1002",
        what: "Every client log and journal line carries a plain absolute UTC time (unix ms `ts` in the log ring, the turn timeline, the crash report and the net-test bot's events; an RFC 3339 stamp in front of each browser console line, as the server's tracing output), so client and server logs line up by time; there is no clock-sync protocol, so the skew between the machines (normally the NTP error) stays. The `performance.now()` clock is used only for durations",
        place: "apps/web/www/bootstrap.js, platform-web/log.rs, tools/net-test/bot/mod.rs, client-app/timeline.rs",
    },
    SimpRow {
        id: "SIMP-exe-1010",
        what: "Cursors: the rows of the Cursors table that no interface mode uses (window resize arrows) and the files no data row or script names (attack.ani, Found.ani, Build.ani, target.ani, Mine.ani...) show the arrow; the cursor over a hex (move/attack/found hover) is not changed, only UIManager:SetUICursor from InGame.lua is followed",
        place: "client-app/cursor.rs, tools/asset-pipeline/src/cursors.rs",
    },
    SimpRow {
        id: "SIMP-exe-1011",
        what: "UI.IsAIRequestingConcessions answers false: the exe's byte (+0x1ad) is written only by the DLL through SetAIRequestingConcessions, and the Lekmod DLL source calls it with false only (DoTradeScreenClosed); no interface operation carries the byte to the exe if a DLL ever raised it",
        place: "exe-api/ui_api.rs",
    },
    SimpRow {
        id: "SIMP-exe-1012",
        what: "The caravan's temporary popup route comes whole: the fork sends every plot of TradeVisuals_NewRoute in the event's data (x, y pairs; the args keep the first 9); an older core's 9-plot event is still completed by the hex line (SIMP-exe-921)",
        place: "fork rt_engine_ui.cpp, gamecore/ui.rs, exe-api/engine_events.rs, client-app/bridge/trade.rs",
    },
    SimpRow {
        id: "SIMP-exe-1020",
        what: "Multiplayer Auto End Turn is checked once per page frame; the exe runs updateTestEndTurn only on frames with an empty unit cycle list and no popup/placement gate (updateIfNotPaused), and its turn slice counter stands still while the game is paused. Here the windows hold it as SIMP-exe-701 says and a pause is not looked at",
        place: "exe-api/auto_end.rs",
    },
    SimpRow {
        id: "SIMP-exe-1021",
        what: "Auto End Turn does not send sendAutoMoves for the seat's automated units before the end of turn (Part B, hasAutoUnit): the replica drops the message, the server's core moves the automated units itself when the end of turn comes, so they move after the end, not before it",
        place: "exe-api/auto_end.rs",
    },
    SimpRow {
        id: "SIMP-exe-1022",
        what: "Auto End Turn waits while one of the seat's cities CanRangeStrikeNow (read in Lua once per frame); the original gets it from the ENDTURN_BLOCKING_CITY_RANGE_ATTACK blocker of the notification, which the server's core never reports because it reads the client's option as off",
        place: "exe-api/auto_end.rs",
    },
    SimpRow {
        id: "SIMP-exe-1023",
        what: "MultiplayerHotJoinStarted / MultiplayerHotJoinCompleted and Network.IsPlayerHotJoining are read off the lobby seat list and the replica once per frame (a human comes on line in a game in progress: Started, a look later Completed), not raised by the host's NetHotJoinStart / NetHotJoinCompleted messages; the joiner's own machine raises nothing and a join is one Started, not one per message",
        place: "exe-api/network.rs",
    },
    SimpRow {
        id: "SIMP-exe-1024",
        what: "Events.UnitMemberOverlayAdd/Remove/ShowHide/Message/TargetColor and UnitMemberPositionChanged/CombatTargetChanged/CombatStateChanged are registered and never raised: the original raises them nowhere either (only Events.UnitMember*.Call from Lua would), so UnitMemberOverlay.lua stays idle; a Lua Call is delivered to its listeners at once (LocalMachineContainer)",
        place: "exe-api/data/events.tsv",
    },
    SimpRow {
        id: "SIMP-exe-1025",
        what: "World soundscapes (item 82, the exe's UpdateSoundscapes over the camera's grid rectangle) are mixed by our own rules where the exe's are not traced: only the weighted mode (four quarters around the camera's hex with their dominant sound, plus the camera's hex and its river) is done, not the uniform mode, and which mode the exe uses is unknown; the rectangle is ceil(height/51) by ceil(height/90) hexes (3..24 by 2..16) instead of the exe's CalculateGridRect table; the weight of a hex in the dominant sound is the soundscape's fPriority; the distance to a hex is flat hex distance plus a listener lift of height/200 hexes (at most 3); the fade between fMinGridDistance and fMaxGridDistance is linear (hard cut at the maximum, as traced); hexes are offset coordinates, the exe's axial",
        place: "audio/soundscape.rs",
    },
    SimpRow {
        id: "SIMP-exe-1026",
        what: "World soundscape sounds: a bed plays at its script volume times the soundscape volume (mean of iMinVolume and iMaxVolume) times the fade, mono, no speaker volumes or pan; a random element waits a draw of its iMinTimeMustNotPlayAgain..iMaxTimeMustNotPlayAgain (the first one a draw from 0 to the maximum) and picks one of its scripts, the engine layer FSoundScape is not traced; the music, SFX and ambience knobs do not scale it separately (the SFX knob is the master gain), and nothing is paused with the window",
        place: "audio/soundscape.rs, audio/engine/scape.rs, client-app/ui_sound.rs",
    },
    SimpRow {
        id: "SIMP-exe-1027",
        what: "World soundscape input: the map is the renderer's snapshot, so a plot gives its terrain (hills and mountains as TERRAIN_HILL and TERRAIN_MOUNTAIN, water as coast or ocean), its feature (only those the renderer draws), whether any river edge touches it and whether it was ever seen; improvements give no sound (the snapshot has none and no shipped row names a soundscape for one). ASSS_AMBIENCE_TUNDRA, used by the tundra and snow rows, is defined nowhere in the game and is silent (the exe's lookup of an unknown name is not traced), so tundra and snow are quiet and ASSS_AMBIENCE_SNOW is never used. The table (ui:scape) and the AS2D_AMBIENCE_* files come with the asset set; an older set has none and the map stays quiet",
        place: "client-app/world_sound.rs, asset-pipeline/ui/audio.rs",
    },
    SimpRow {
        id: "SIMP-exe-1028",
        what: "The god ray (the beam over the selected unit, exe cvMovementPathVisSystem) reproduces re-b11: the \"God Ray\" blend (colour ONE/SRCALPHA, alpha ZERO/SRCALPHA) on the gamma-space world target, the three-layer FXPS_CODE3 maths (squared texture decode, layers over each other, sqrt output), the textures per layer of the material (base_01/UV_01, base_02/UV_03, base_02/UV_02, alpha_01) and the time since the highlight event, unwrapped; its brightness has no golden capture of the original to be compared with (re-b11 estimates +0.05..0.1, up to 0.3 at the brightest texels)",
        place: "render/src/world/god_ray.rs",
    },
    SimpRow {
        id: "SIMP-exe-1029",
        what: "The god ray follows the selected unit's hex of the UI (Highlights.selected, the ring's hex) and restarts its clock when that hex changes or the selection returns after a clear; the exe restarts on every UnitHexHighlight event (also a reselect of another unit on the same hex); the start time travels in the four colour bytes of the instance; no fog or zoom culling (the exe has none in OnHexHighlight); one draw, where the exe draws it in the pass (of two rects) that contains the beam position",
        place: "render/src/world/god_ray.rs",
    },
    SimpRow {
        id: "SIMP-exe-1030",
        what: "God ray pixel shader variant: FXPS_CODE3 (three layers) is inferred from the three texture layers of the material and the package kind 0x38; the selecting code of the exe (dispatchShaderPackageGetID_ShaderDefDX9) is not decoded. fElapsedTimeR and fScrollCoordR (both the seconds since the highlight event, period 1.0) are assigned to the dynamic block offsets 0x40 and 0x50 by register order, not by a name table; fScrollCoordR has no visible effect because AlphaLookup.R is constant (126)",
        place: "render/src/shaders/unit/fx.wgsl, render/src/material/families.rs",
    },
    SimpRow {
        id: "SIMP-exe-1031",
        what: "God ray sampler states: repeat in u and v, linear with mips, no anisotropy or bias, as the shared object sampler of the effect models; the exe sets the per-sampler state of the FX material elsewhere and it was not decoded (re-b11 open item 3). The 256x4 base textures vary in u only, so only the u wrap of the slow scroll could differ",
        place: "render/src/world/unit/draw.rs",
    },
    SimpRow {
        id: "SIMP-exe-1032",
        what: "God ray depth and cull: the exe clears the depth to 1.0 and draws with LESSEQUAL and a depth write, CULL_CW; here the depth test is Always without a write (the same picture for the convex one-sided funnel, since a depth clear in the middle of the pass is not possible) and the culled side is Back by the counter-clockwise winding of the baked mesh in our basis (a pipeline test counts all 708 triangles outward). The beam is also drawn in the reflection pass; whether the reflection view of the exe draws it is not traced",
        place: "render/src/material/desc.rs",
    },
    SimpRow {
        id: "SIMP-exe-1033",
        what: "God ray gamma: the shader output (sqrt, the gamma encode of the original, written with SRGBWRITEENABLE off) goes unchanged into the blend of the Rgba8Unorm world target, which holds the original gamma-space values and is presented as it is; a surface that only offers sRGB is linearised by Present, so the on-screen value equals the original one. Not compared with a capture of the original",
        place: "render/src/material/pipeline_cache.rs",
    },
    SimpRow {
        id: "SIMP-exe-1034",
        what: "God ray textures: the three layers read the DDS files of the set as the build decodes them (base_01, base_02, alpha_01 of 10 mips and the three 4x4 UV maps); the hand decoded values in re-b11 differ by +-1 level from the GPU decoder, and a set built before the model was added to b2-misc has no model at all (the beam is then absent, nothing else changes)",
        place: "asset-pipeline/sets/b2_misc.rs",
    },
    SimpRow {
        id: "SIMP-exe-1050",
        what: "Lobby art (asset set web-lobby): each civilization strip carries the first leader of Civilization_Leaders by rowid, no leader variants (the lobby picks a civilization only, as the core's CIV_INFO tip)",
        place: "asset-pipeline/sets/web_lobby.rs",
    },
    SimpRow {
        id: "SIMP-exe-1051",
        what: "Lobby art: the icons are cut at fixed sizes (leader 128, civ and unique icons 64) and box-resampled when the atlas has no row of that size; the icon font and the Colors table go to the DOM page as glyph rects and RGBA bytes, not through the engine's text layers",
        place: "asset-pipeline/sets/web_lobby.rs",
    },
    SimpRow {
        id: "SIMP-exe-1054",
        what: "Game text markup on the DOM lobby and room (ui.js markup()): [NEWLINE], [TAB], [COLOR_*]...[ENDCOLOR] (nested, colours of the Colors table), [COLOR:r:g:b:a] and [ICON_*] (glyphs of the icon font at 18 px) are drawn; [STYLE_*] fonts and every other tag are dropped, an unknown colour keeps the text colour; native <option> labels show the text without markup",
        place: "apps/web/www/ui.js",
    },
    SimpRow {
        id: "SIMP-exe-1055",
        what: "Staging room civilization tip: a DOM card of our own layout (leader portrait, civ icon, traits split at the first \": \", unique units, buildings and improvements with icons from the web-lobby art) shown on mouse hover or keyboard focus of the civ picker entries; touch screens get the same card in an \"About this civilization\" fold of the slot. The original MP staging room's exact tooltip layout is not reproduced",
        place: "apps/web/www/ui.js",
    },
    SimpRow {
        id: "SIMP-exe-1060",
        what: "The staging room lists the civilizations with Playable = 1 (the original pull-down query); the core's civilization catalog has no Playable column, so the host drops the three Playable = 0 rows of the Lekmod data by type (CIVILIZATION_MINOR, CIVILIZATION_BARBARIAN, CIVILIZATION_VENICE) and the lobby refuses any civilization that is not in its pick list; the asset set web-lobby reads the column itself",
        place: "game-host/lobby_options.rs",
    },
    SimpRow {
        id: "SIMP-exe-1062",
        what: "Unique item details of the civilization tip (web:lobby uniques[]: replaces, stats, help): the original is the EUI tooltip of GetHelpTextForUnit/Building/Improvement; the lobby lists the \"Replaces:\" line, the movement, ranged, combat and production cost numbers of the row (cost only above 1) and the pre-written Help text; free-promotion bonuses, resource needs, gold and faith prices, upgrade lines and the other TXT_KEY_EUI_* tag lines are left out, as is everything that needs an active player",
        place: "asset-pipeline/sets/web_lobby.rs",
    },
    SimpRow {
        id: "SIMP-exe-1080",
        what: "A touch screen has no right button: a finger held still for 500 ms (10 CSS px of slop) holds the right button at the finger (the touch is cancelled off-screen, then the mouse moves and presses the right button; the finger's moves are mouse moves with the button held and the lift releases it, which is the click; a cancel or second finger releases it too); tap = left click, drag = pan, two fingers = pinch stay as they were. The original is a mouse game and has no touch input; the time and slop are set by feel",
        place: "client-app/touch_press.rs",
    },
    SimpRow {
        id: "SIMP-exe-1081",
        what: "Full screen on phones and tablets: a button (Fullscreen API on the tap, landscape lock, remembered for the next visit) and a web app manifest (display fullscreen, falling back to standalone) for Add to Home Screen; iPhone Safari has no Fullscreen API, so there the button explains Add to Home Screen. The original is a desktop game",
        place: "apps/web/www/fullscreen.js, apps/web/www/manifest.webmanifest, apps/web/www/index.html",
    },
    SimpRow {
        id: "SIMP-exe-1082",
        what: "Within 40 px of the window edge (48 px at the bottom) the game cursor is a page element that follows the mouse instead of the system's custom cursor, because browsers drop a custom cursor image near the window edge and show the arrow; it is one frame behind the pointer there",
        place: "apps/web/www/cursor-edge.js",
    },
    SimpRow {
        id: "SIMP-exe-1085",
        what: "Automated units of a human (explore, automated workers) move in the auto-moves phase after the player ended the turn, not at the start of the turn: the fork's CvPlayer::hasUnitsThatNeedAIUpdate is false for a human until isAutoMoves() (the stock DLL ran the homeland AI for them whenever the turn was active, so they moved before the player could give a manual order). The order of the auto-moves phase (AI_unitUpdate, then AutoMission) is the stock one; the exe's own timing of that phase is not recovered",
        place: "game-host/tests/real_automate.rs",
    },
    SimpRow {
        id: "SIMP-exe-1086",
        what: "The per-account store follows only the options-screen settings and in-game toggles (civ5.options.* of the page store) and the last room; the graphics settings (settings/graphics.ini blob), the user data (civ5.userdata.*) and the tokens stay on the device",
        place: "client-app/account_sync.rs",
    },
    SimpRow {
        id: "SIMP-exe-1087",
        what: "The last room of a host is saved when their game starts from a new room (not from a save) and is the default of their next CreateRoom: seeds are not kept, the name is the default one, and every pick the server's lists no longer have falls back to the quick default; the quick start and the autostart script ignore it",
        place: "client-app/autostart.rs",
    },
    SimpRow {
        id: "SIMP-exe-1088",
        what: "Account store bounds: 128 entries and 16 KiB of keys and values per account, keys of 1 to 64 characters of letters, digits and . _ -, values of at most 256 characters without control characters, 5000 accounts, a last room of at most 8 KiB encoded; a change over a bound is dropped whole with a line in the log",
        place: "server/prefs.rs",
    },
    SimpRow {
        id: "SIMP-exe-1089",
        what: "Account store sync: the server's value wins at login, a key only the device has is uploaded, a setting changed before the store arrived wins over the server's, later changes are last-writer-wins per key and are sent at most once a second; two devices of one account see each other's changes at their next login, not live",
        place: "client-app/account_sync.rs",
    },
    SimpRow {
        id: "SIMP-exe-1070",
        what: "Natural wonder pieces (b12-wonders): every piece of a wonder's model is seated at max(nearest-cell ground at its own origin, water level), replacing its authored height, as the original's per-element height step does; the four-point river test that hides a piece (bTerrainContour) is not ported, so every piece shows",
        place: "world/landmark.rs",
    },
    SimpRow {
        id: "SIMP-exe-1071",
        what: "Natural wonder models (b12-wonders): the pipeline keeps the pieces of a natural wonder's model apart (part scope) so each can be seated; a set built before this change merges them whole and the wonder keeps its old placement until the set is rebuilt",
        place: "tools/asset-pipeline/sets/b2_landmarks.rs",
    },
    SimpRow {
        id: "SIMP-exe-1072",
        what: "Wonders without a piece (b12-wonders): El Dorado and the Fountain of Youth keep the class of their terrain, Solomon's Mines the desert class, any other wonder of the install without a Wonder_* piece (Everest, Titicaca, Victoria Falls) the Fuji class, by the type name of the catalog; the numbers are the original's table, the per-continent choice of the Lake Victoria tiles is the matcher's",
        place: "world/terrain/build/bridge.rs",
    },
    SimpRow {
        id: "SIMP-exe-1073",
        what: "Landmark mesh decals (b12-wonders): the priority is the height of the first vertex of the merged mesh in 0..1 (the original reads the raw mesh vertex); a whole-merged model has its placement baked into that vertex",
        place: "world/landmark/ground.rs",
    },
    SimpRow {
        id: "SIMP-exe-1074",
        what: "Water height map (b12-lakevic): the map is stretched over the whole world of the original, which is 64 texel rows longer than our field along y; the rows north of the field are deep sea (height 0), the original's padding rows are not read from the exe",
        place: "world/water.rs",
    },
    SimpRow {
        id: "SIMP-exe-1090",
        what: "Performance panel on touch screens: three fingers held still for 3 s toggle it, the touch twin of F3 (operator's choice); the original has neither touch input nor this panel",
        place: "client-app/touch_hud.rs",
    },
    SimpRow {
        id: "SIMP-exe-1110",
        what: "Render scale and shadow quality (b13-rscale): the two controls are in the interim Settings window, reached from [Graphics settings] in the performance panel (F3, three fingers on touch); the original's Options menu (Lua and xml of the game) has no such option and its graphics options drive nothing; the choice is kept in the key-value store as civ5.options.RenderScale and civ5.options.ShadowQuality and follows the account",
        place: "client-app/gfx_prefs.rs",
    },
    SimpRow {
        id: "SIMP-exe-1111",
        what: "Touch device (b13-rscale): the web page says so by the user agent (Android, iPhone, iPad, Mobile) or by a Mac user agent with several touch points, not by a pointer media query; a touch device defaults to render scale 75% and medium shadows, a laptop with a touch screen does not",
        place: "client-app/gfx_prefs.rs",
    },
    SimpRow {
        id: "SIMP-exe-1112",
        what: "Render scale (b13-rscale): every target that follows the screen (scene, depth, MSAA, overlay, class mask, the raw shadow map) is made at scale times the surface, and Present stretches the scene over the surface with a linear filter; the original draws at the window size only; the UI passes draw on the surface itself",
        place: "render/render_scale.rs",
    },
    SimpRow {
        id: "SIMP-exe-1113",
        what: "Shadow quality (b13-rscale): High is the size ShadowLevel makes, Medium and Low halve and quarter every side of the raw and the blurred map (the blurred map keeps 128 at least), Off is ShadowLevel 0 (no pass, a cleared map); the original has the level only",
        place: "render/shadow/camera.rs",
    },
    SimpRow {
        id: "SIMP-exe-1091",
        what: "Texture variants (b13-texc): volume textures get no BC7 or ASTC variant and keep the original's format on every device, since a compressed volume needs the sliced-3D feature few devices have",
        place: "tools/asset-pipeline/textures/variants.rs",
    },
    SimpRow {
        id: "SIMP-exe-1092",
        what: "Texture variants (b13-texc): a world texture whose side is not a multiple of 4 stays plain RGBA8 on desktop and phones (a written exception); only UI images are padded to whole blocks, since padding a tiled world texture would move its seam",
        place: "tools/asset-pipeline/textures/variants.rs",
    },
    SimpRow {
        id: "SIMP-exe-1093",
        what: "Texture variants (b13-texc): a plain layer of a texture array (art-table terrain, feature, river, SV and tree textures, the world decals) gets no BC7 variant, since the desktop array has one format and the BC originals stay byte for byte; phones get ASTC for every layer",
        place: "tools/asset-pipeline/textures/variants.rs",
    },
    SimpRow {
        id: "SIMP-exe-1120",
        what: "Performance panel (b14-perfdiag): the desktop client's \"Send report\" writes perf-report-<ms>.json to $CIV5_LOG_DIR (default ./logs) instead of posting it to the server, and carries no log lines (the desktop log goes to the console); only the web client posts to /client-report/v2",
        place: "client-app/perf_report.rs",
    },
    SimpRow {
        id: "SIMP-exe-1121",
        what: "Performance panel (b14-perfdiag): the draw groups of Main are the subsystems that record into its slots (terrain, water, rivers, units, trees, cities, landmarks, decals, overlays, fog, effects); resources, improvements and wonders are drawn by the Landmark and City subsystems together with roads and buildings, so they cannot be told apart, and WebGPU times whole passes, so the time of a group is read from the GPU time of Main with the group switched off",
        place: "render/perf/groups.rs",
    },
    SimpRow {
        id: "SIMP-exe-1130",
        what: "Graphics MSAA setting (b15-gfx): the window offers Off and 4x only; WebGPU guarantees 1 and 4 samples for the formats of the main pass and every pipeline is made for one count, so there is no 2x; the choice is kept at once and takes effect at the next start (the pipelines and targets keep their count), the window says so; a ?preset= or ?gfx= in the address wins over the stored choice",
        place: "render/gfx_choice.rs",
    },
    SimpRow {
        id: "SIMP-exe-1131",
        what: "Shadow quality Low (b15-gfx): the static models do not cast into the map, only the units, landmarks and cities do; Medium and High draw every caster as the original does at every level",
        place: "render/shadow/camera.rs",
    },
    SimpRow {
        id: "SIMP-exe-1132",
        what: "Graphics presets (b15-gfx): Low is 60% scale, Low shadows, no MSAA (the phone default), Medium 80%, Medium shadows, no MSAA, High 100%, High shadows, 4x (the desktop default); they are ours, the original has none; Custom is shown when the three settings are none of them",
        place: "render/gfx_choice.rs",
    },
    SimpRow {
        id: "SIMP-exe-1133",
        what: "Graphics choices per device (b15-gfx): render scale, shadow quality and MSAA are kept under civ5.gfx.* in the device's own store (localStorage on the web, device-prefs.txt in the user's config folder on the desktop) and the account does not follow them; the values the account carried under civ5.options.RenderScale and ShadowQuality are not read, a device without its own value takes its default",
        place: "client-app/gfx_prefs.rs",
    },
    SimpRow {
        id: "SIMP-exe-1134",
        what: "Shadow quality pick (b15-gfx): a pick of Low, Medium or High is a real map even where the keys of the world's detail say ShadowLevel 0 (the Low detail), at the biggest map cut by the quality; without a pick (goldens, ?gfx=ShadowLevel:0) the key is the level as before",
        place: "render/shadow/camera.rs",
    },
    SimpRow {
        id: "SIMP-exe-1150",
        what: "The EUI option DB_bAnchorLeft (which of the two civilization lists the Lekmod NotificationPanel shows) is 1, the left list, when the player has not set it; the exe leaves it unset, which is the right list: our players want the trade partners list on the left",
        place: "exe-api/modding.rs",
    },
    SimpRow {
        id: "SIMP-exe-1140",
        what: "Terrain detail (b15-terrain): three levels (Low, Medium, High) of the player pick the coarsest lattice of a cell, the biggest page of the cache and the memory of the pages, and the page a cell asks for follows the size of the surface, not of the render scale; the original has one TerrainDetailLevel tied to the graphics preset and no render scale; the choice is kept as civ5.gfx.TerrainDetail on the device (SIMP-exe-1133) and is part of the graphics presets",
        place: "render/terrain_detail.rs",
    },
    SimpRow {
        id: "SIMP-exe-1170",
        what: "City buildings (b16-cities): the placement kind of a building (Buildings.DisplayPosition) is read from the building_type/ records of the art table (re-b17 5.1); with an art table built before them, from a table of building types taken from the RB13 answer by art tag (an Amphitheater then draws the Colosseum), and a building with a model that is not in it takes the corner kind 2",
        place: "render/world/city/wonders.rs",
    },
    SimpRow {
        id: "SIMP-exe-1171",
        what: "Lighthouse and corner models (b16-cities): a lighthouse takes a corner of the city hex or its ring with water on any of its three hexes (the corner flags of InitHexCorners are not read through; re-b17 1.4: a water edge flags the corner); the heading of corner models and of the four fixed kind-1 slots (street tensor, noise heading) is not read, the yaw is 0",
        place: "render/world/city/wonders.rs",
    },
    SimpRow {
        id: "SIMP-exe-1172",
        what: "Harbor yaw (b16-cities): +x of the harbor model is turned along land to water (re-b17 1.1b confirms +x is the sea side of the Harbor_*.gr2 meshes); the yaw of the other kind-16 models (Sydney Opera House) follows the same rule unchecked",
        place: "render/world/city/wonders.rs",
    },
    SimpRow {
        id: "SIMP-exe-1173",
        what: "House footprint (b16-cities): a house is dropped when any of the four corners or four side middles of a footprint of 5 units is on water; the original tests the four corners at 2 units only and houses stood partly in the sea",
        place: "render/world/city/layout.rs",
    },
    SimpRow {
        id: "SIMP-exe-1180",
        what: "World ambience random sounds (b16-ambient, b17-unitsfx): every random element of a soundscape waits a draw from its own min..max gap before its first sound and again from the end of each sound (re-b15 P7.4, as the exe); the one-second fade of a bed that ends is a linear ramp of the voice gain and does not reach the one-shots of the scape that still sound (the exe fades the scape's volume)",
        place: "audio/soundscape.rs, platform-web/audio.rs",
    },
    SimpRow {
        id: "SIMP-exe-1160",
        what: "Roads (b16-roads): g_HexToTex is not read, a spline unit is a texel (one world unit along X, a row is 0.866 of ours), so the width of the strip (0.1 x the piece, 4.74 for Road_reg), the tile length and the 12 texel ribbon are measured in the texel plane, and a road across the rows is 0.866 as wide as one along them; the B-spline is the uniform quadratic one, not the arc-length one",
        place: "render/world/objects/decals.rs",
    },
    SimpRow {
        id: "SIMP-exe-1161",
        what: "Road clearing (b16-roads): the ribbon of a road goes to the CPU cuts the trees test at their point (the landmark Tree Cut meshes do the same, SIMP-065), not into bit 0x02 of the terrain stencil in cells of 2 x 2 texels with the mask 7; the edge of the ribbon is exact, the coastal roads clear nothing",
        place: "render/world/objects/decals.rs",
    },
    SimpRow {
        id: "SIMP-exe-1162",
        what: "City roads (b16-roads): what sets the mode of a city plot (bits 6..7, the initial corner flag is open in the answer) is not read; every city plot has the ring of its corners at 0.45 and a spoke to each link, the ring and the spokes are a quadratic B-spline of the corners and straight lines",
        place: "render/world/objects/decals.rs",
    },
    SimpRow {
        id: "SIMP-exe-1163",
        what: "Road pieces (b16-roads): only Road_reg and RR_reg of RoadsandRails_new.gr2 are laid (the PL, HB and trade variants, picked by the route state and rand() per tile, are not), the pillaged road keeps the older _dam piece; the sampler of the road in the bake (ClampToEdge, linear, mip linear) and its texel are those of the other decals, the original's are not read",
        place: "render/world/objects/decals.rs",
    },
    SimpRow {
        id: "SIMP-exe-1200",
        what: "Unit 3D sounds (b17-unitsfx): the level of a sound between its fMinDistance and fCutoffDistance is linear (AudioParams iFalloffFunctionIndex 1) and its stereo position is the cosine of its bearing to the east (the camera looks north); Miles' per-speaker levels (fSpeakerFalloffPower 5) and its own rolloff are not traced; the level is set at the start only (a one-shot at a fixed point)",
        place: "audio/engine/positional.rs, platform-web/audio.rs",
    },
    SimpRow {
        id: "SIMP-exe-1201",
        what: "Unit 3D sounds (b17-unitsfx): $(TERRAIN) is never AIR (a flying member is not told), the listener stands at height 0 plus its lift (not on the terrain under the focus), and the looping unit scripts (tracked loops per script and unit, stopped 0.07 s / 0.1 s in combat after their last trigger with a 0.5 s fade) are not played",
        place: "audio/unit.rs, client-app/ui_sound.rs",
    },
    SimpRow {
        id: "SIMP-exe-1202",
        what: "Unit action sounds (b17-unitsfx): the port plays only the standing clips (SIMP-010), so a move, an attack and a death the seat sees cue the clip the exe would play and its sound rows play timed from the cue: one cycle of the run clip at the plot the unit went to, the attack clip (city attack against a city, a defender that strikes back attacks too), the death clip 0.5 s later with the killer's weapon and the killer's victory clip 1 s after that; the exe's combat and move simulation picks and times the clips",
        place: "exe-api/engine_events.rs, client-app/unit_sound.rs",
    },
    SimpRow {
        id: "SIMP-exe-1203",
        what: "Popup voice-overs (b17-unitsfx): of an era's several Era_NewEraVOs lines the game turn picks one (the exe takes a game value vf[0x14] modulo the count); a new popup's line ends the one before but closing a popup does not stop its line; they play when the core announces the popup, not when the exe's queue shows it (SIMP-exe-682)",
        place: "exe-api/core.rs, client-app/ui_sound.rs",
    },
    SimpRow {
        id: "SIMP-exe-1204",
        what: "Notification sound queue (b17-unitsfx): one at a time on whole-second polls with the tail-duplicate drop, as the exe; the exe drops the ones popped while a leader screen is open and plays none after the game is over, which the port's queue does not know",
        place: "audio/notify.rs",
    },
    SimpRow {
        id: "SIMP-exe-1190",
        what: "Music (b17-music): a song number below the playlist's count that has no script is left out of the list (the exe keeps its hash, plays nothing for it and waits the 20 s gap)",
        place: "audio/music.rs",
    },
    SimpRow {
        id: "SIMP-exe-1191",
        what: "Music (b17-music): the main menu loop goes on under the loading screen and the game's first song (the active leader's theme) crossfades in when the loading screen closes; what the exe plays while loading (LOADING_SCREEN_MUSIC_OVERRIDE, defined nowhere) is open",
        place: "audio/music.rs, client-app/app.rs",
    },
    SimpRow {
        id: "SIMP-exe-1192",
        what: "Music (b17-music): the exe's flag 0x49 of PickNonPlaylistTrack (after a leader's WAR theme on leaving his screen, the active leader's own WAR theme) is not copied, its meaning is not verified; after that WAR theme the playlist moves on",
        place: "audio/music.rs",
    },
    SimpRow {
        id: "SIMP-exe-1193",
        what: "Music (b17-music): RandomizeSongList swaps the first and the last song when the new round would start with the song that ended the last one (the stated effect, no song twice in a row across the wrap); the worker's reading compares the new last song with the old one",
        place: "audio/music.rs",
    },
    SimpRow {
        id: "SIMP-exe-1194",
        what: "Music (b17-music): the 2 s linear crossfade (and the 0.5 s / 1.0 s duck of the music bus) is stepped once a frame through set_voice_gain, which the browser glides over 0.25 s, not by Miles' own fades; a streamed song's fade-in starts when its <audio> element starts",
        place: "audio/engine/music.rs, audio/engine/mixer.rs",
    },
    SimpRow {
        id: "SIMP-exe-1195",
        what: "Music (b17-music): the AudioParams.xml <Music> values (StartingEraForBorderRegionTracks ERA_RENAISSANCE, 10 % per era, at most 50 %) are mapped to the fields UpdatePlayerTracks reads by their names; their loader was not traced",
        place: "audio/music.rs",
    },
    SimpRow {
        id: "SIMP-exe-1196",
        what: "Music (b17-music): the set takes the music of the Script2DFile XML only (with --ui-music); music a mod adds as Audio_2DSounds database rows (Lekmod's custom leaders) is not in it, such a leader's theme is silent and the playlist follows after the gap",
        place: "tools/asset-pipeline/src/ui/audio.rs",
    },
    SimpRow {
        id: "SIMP-exe-1197",
        what: "Music (b17-music): the leader screen's looping ambience (AS2D_AMBIENCE_<LEADER>_AMBIENCE) and the leaders' spoken lines are not played (the speech trigger is in the leader scene data, not found); the music does not pause while the tab is hidden (the exe's application flag 0x1eac)",
        place: "audio/music.rs",
    },
    SimpRow {
        id: "SIMP-exe-1210",
        what: "Farm plots (rb18): the original's plots are faces of streamlines of two families traced through the layout heading field (cvRoadNetworkManager, tracer and face walk not read); ours are straight 14.4-unit cells turned by the heading at the hex centre, so a field does not curve and fields of two neighbouring farms may overlap or leave a sliver at the hex border",
        place: "render/world/landmark/decals.rs",
    },
    SimpRow {
        id: "SIMP-exe-1212",
        what: "Units under the fog (rb18): the original fades a unit out over its per-unit fade time (cvUnitVis +0x48) when its hex stops being visible and never dims it; ours never dims it either but does not fade: a unit the view no longer has is gone at once",
        place: "render/shaders/skin/unit_skinned.wgsl",
    },
    SimpRow {
        id: "SIMP-exe-1230",
        what: "Water depth texture (b17-coast, re-b16 O1): one R8 texture of the whole world, at full field resolution on High water quality (as the original's 1024 x 1024 tiles) but averaged down to 4096 (Medium) or 2048 (Low, the phone default) texels a side and to the device's texture limit, instead of the original's tiles at full resolution everywhere",
        place: "world/water/ocean.rs",
    },
    SimpRow {
        id: "SIMP-exe-1250",
        what: "City tree cuts (b17-cities): the 10 x 10 square under each house (UpdateBuildingBase, re-b17 3.1) goes to the cuts the trees test on the CPU as the roads do (SIMP-exe-1161), not into the stencil bit 0x02; every house of the layout cuts, and the type-3 mesh decals of the catalog models and the wall polygon do not cut",
        place: "render/world/city.rs",
    },
    SimpRow {
        id: "SIMP-exe-1251",
        what: "Half-built wonders (b17-cities): the wonder a city is building is drawn with the model of the built one in its slot (re-b17 5.3); a building counts as a wonder when its tag has a model of its own and no variants (the art table does not say which buildings are wonders), and a league project in the capital is not drawn",
        place: "render/world/city.rs",
    },
    SimpRow {
        id: "SIMP-exe-1252",
        what: "City walls (b17-cities): the stretch is laid along each wall segment as AddSplineModel does (n = max(1, floor(L / length)) whole copies stretched to fill the spline, at even shares of its arc length; re-b17 2W), but each copy is a rigid instance along the chord of its share, lifted to the ground at its middle; the original bends every vertex along the spline and drapes it on the terrain under it",
        place: "render/world/city/walls.rs",
    },
    SimpRow {
        id: "SIMP-exe-1270",
        what: "Unit animation machine (rb18, b19-anim): the original steps a machine once per drawn frame with the frame's dt, only for units inside the view rectangle (+64), so an off-screen unit's machine is frozen; ours ticks every machine at a fixed 30 Hz, on screen or not (one roll per tick)",
        place: "render/anim/fsm.rs",
    },
    SimpRow {
        id: "SIMP-exe-1271",
        what: "Unit animation graphs (b20-dge, RB19): the .dge graphs of the install are decoded by the asset pipeline (set client-data, data:dge-graphs) and installed by the client at start; a set without them plays the clips of each action; a state plays the to side of its entry blend as the path options select it, crossfaded from the clip the figure played before (not the graph's own from tree), with the graph's BlendTime and the S-curve approximated by a uniform Catmull-Rom; the AIM_LAYER overlay (1700/1705/1710) is not drawn; a blend by a path option takes the nearer side; a Selection driven by BlendEC takes its first input",
        place: "render/anim/graph.rs",
    },
    SimpRow {
        id: "SIMP-exe-1272",
        what: "Unit formations (b19-anim): the B2 art table carries no formation records, so the client holds the 24 records of the shipped UnitFormations.xml and uses them when the table lacks the name (the exe reads the XML into cvUnitFormationLib); the jitter seed is the unit id, not the slot of its sim in the pool",
        place: "render/entities/formation.rs",
    },
];
pub fn row(id: &str) -> Option<&'static SimpRow> {
    SIMP_ROWS.iter().find(|r| r.id == id)
}
