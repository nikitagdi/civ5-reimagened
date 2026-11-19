import {popupScenario,texts,waitPopup,clickRequired} from "./ui2-popup.mjs";
import {until} from "../lib/browser.mjs";
// The game menu of UI v2: its options are the client's (the classic client's `civ5.options.*` of the page's store, which
// the account follows), drafts until Accept; the hotkeys tab rebinds the skin's and the game's keys.
export async function run(ctx) {
 return popupScenario(ctx,"game_menu",async(page,check)=>{
  const intent=(name,args={})=>page.evaluate(i=>window.__civ5test.ui2Intent(i),{name,args});
  const stored=key=>page.evaluate(k=>localStorage.getItem("civ5.options."+k),key);
  const optionsOpen=p=>p.ui.state["menu.options"]===true;
  await waitPopup(page,p=>texts(p,"game-menu.leader").length>0,"real leader");
  const before=await stored("MultiplayerAutoEndTurn");
  // A draft dropped by Cancel is not stored.
  await clickRequired(page,"game-menu.options");
  await waitPopup(page,optionsOpen,"options open");
  await intent("options.set",{key:"MultiplayerAutoEndTurn",value:before==="1"?0:1});
  await clickRequired(page,"game-menu.cancel");
  await waitPopup(page,p=>!optionsOpen(p),"options closed");
  check("a cancelled option is not stored",await stored("MultiplayerAutoEndTurn")===before);
  // An accepted one is stored in the classic form; an option that does not apply says why.
  await clickRequired(page,"game-menu.options");
  await waitPopup(page,optionsOpen,"options open again");
  const rows=await waitPopup(page,p=>texts(p,"game-menu.option").length>0,"option rows");
  check("options that do not apply say why",texts(rows,"game-menu.option").some(t=>t.includes("server")));
  await intent("options.set",{key:"Volume.MUSIC",value:40});
  await clickRequired(page,"game-menu.accept");
  await waitPopup(page,p=>!optionsOpen(p),"options accepted");
  check("the music volume is stored as the classic client stores it",await stored("Volume.MUSIC")==="0.4");
  // A hotkey: F6 (the technology tree) moves to Y.
  await clickRequired(page,"game-menu.options");
  await waitPopup(page,optionsOpen,"options for the keys");
  await clickRequired(page,"game-menu.tab-hotkeys");
  await waitPopup(page,p=>texts(p,"game-menu.hotkey-key").length>0,"hotkey rows");
  await intent("hotkey.capture",{id:"F6"});
  await waitPopup(page,p=>texts(p,"game-menu.hotkey-key").some(t=>t.includes("Press")),"waiting for a key");
  await page.keyboard.press("KeyY");
  await waitPopup(page,p=>texts(p,"game-menu.hotkey-key").includes("Y"),"F6 bound to Y");
  // A key another row has is refused and the clash is shown.
  await intent("hotkey.capture",{id:"F2"});
  await waitPopup(page,p=>texts(p,"game-menu.hotkey-key").some(t=>t.includes("Press")),"waiting again");
  await page.keyboard.press("KeyY");
  await waitPopup(page,p=>texts(p,"game-menu.hotkey-clash").length>0,"the clash");
  await clickRequired(page,"game-menu.accept");
  await waitPopup(page,p=>!optionsOpen(p),"keys accepted");
  check("the binding is stored",await stored("UI2Hotkeys")==="F6=KeyY");
  await clickRequired(page,"game-menu.return");
  await waitPopup(page,p=>!p.ui.windows.includes("game_menu"),"back to the game");
  await page.keyboard.press("KeyY");
  await waitPopup(page,p=>p.ui.windows.includes("techs"),"Y opens the technology tree");
  check("a rebound key does what its default did",true);
  await intent("window.close",{window:"techs"});
  // Defaults again: the binding and the volume go back.
  await intent("window.open",{window:"game_menu"});
  await waitPopup(page,p=>p.ui.windows.includes("game_menu"),"menu for the defaults");
  await clickRequired(page,"game-menu.options");
  await waitPopup(page,optionsOpen,"options for the defaults");
  await clickRequired(page,"game-menu.reset");
  await clickRequired(page,"game-menu.accept");
  await waitPopup(page,p=>!optionsOpen(p),"defaults accepted");
  check("defaults leave no binding stored",await stored("UI2Hotkeys")===null);
  check("defaults bring the auto end turn back to off",(await stored("MultiplayerAutoEndTurn")??"0")==="0");
  // Saving is the host's session operation: the new save shows in the list of this game's saves (a row keyed by the
  // save's id; the newest ring save of a game replaces the older one, so the key is what changes).
  await waitPopup(page,p=>texts(p,"game-menu.save").length>0,"save button");
  const rowKeys=async()=>((await page.evaluate(()=>window.__civ5test.ui2Rects(["game-menu.save-row"])))??[]).filter(r=>r.keys&&r.w>0).map(r=>String(r.keys));
  const saved0=new Set(await rowKeys());
  await clickRequired(page,"game-menu.save");
  const end=Date.now()+30000;
  let fresh=[];
  while(Date.now()<end&&!fresh.length){fresh=(await rowKeys()).filter(k=>!saved0.has(k));if(!fresh.length)await new Promise(r=>setTimeout(r,300));}
  check("the host's save joins the list of saves",fresh.length>0);
  const saved=await waitPopup(page,p=>true,"probe");
  check("no refusal after the host's save",texts(saved,"game-menu.refused").length===0);
  await clickRequired(page,"game-menu.return");
  await waitPopup(page,p=>!p.ui.windows.includes("game_menu"),"return to game");
  // Leaving is asked once more, then the session takes the seat to the lobby.
  await intent("window.open",{window:"game_menu"});
  await waitPopup(page,p=>p.ui.windows.includes("game_menu"),"menu again");
  await clickRequired(page,"game-menu.exit");
  await clickRequired(page,"game-menu.exit-no");
  await clickRequired(page,"game-menu.exit");
  await clickRequired(page,"game-menu.exit-yes");
  await until(page,s=>s.screen==="lobby",60000,"the lobby after leaving");
  check("exit leaves the game for the lobby",true);
 });
}
