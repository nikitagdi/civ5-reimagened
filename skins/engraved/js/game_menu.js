(function () {
 "use strict";
 const defaults = {"auto_end_turn": false, "auto_end_turn_sp": false, "next_unit_after_orders": true, "auto_unit_cycle": true, "air_rebase_rmb": true, "civilian_yields": true, "workers_keep_improvements": true, "workers_keep_features": true, "no_tile_recs": false, "skip_reward_popups": false, "policy_info_full": true, "score_list": true, "civ_ribbon": true, "city_ribbon": true, "unit_ribbon": true, "city_advisor_recs": true, "auto_close_city": false, "auto_close_tech": false, "clock": false, "all_great_people": false, "warn_unassigned": true, "no_basic_help": false, "scroll_inertia": true, "zoom_dynamic": true, "music": 70, "effects": 80, "ambience": 70, "speech": 80, "tooltip_delay": 500, "tooltip_delay_ext": 1500, "ui_scale": 100, "ribbon_side": "right"};
 function restore() {
  for (const [key,value] of Object.entries(defaults)) {
   const saved=ui.settings.get("options."+key);
   ui.state.set("opt."+key,typeof saved===typeof value?saved:value);
  }
 }
 let opened=false;
 ui.model.on("game_menu",root=>{
  if(!root) {opened=false;return;}
  if(!opened) {opened=true;ui.state.set("menu.options",false);ui.state.set("menu.panel","details");restore();}
 });
 ui.on("menu.open",()=>{restore();ui.state.set("opt.tab","general");ui.state.set("menu.options",true);});
 ui.on("menu.accept",()=>{
  for(const key of Object.keys(defaults)) ui.settings.set("options."+key,ui.state.get("opt."+key));
  ui.state.set("menu.options",false);
 });
 ui.on("menu.cancel",()=>{restore();ui.state.set("menu.options",false);});
})();
