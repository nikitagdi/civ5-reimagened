(function () {
  "use strict";
  // The HUD panels (notifications, helm, ribbon, unit lists, unit panel, research) step aside while a window that
  // covers the map is open, and under the trade screen with an AI: `state.hud.covered`, which the two layer roots
  // of index.html read.
  const covering = ["economy", "military", "trade_routes", "espionage", "victory", "demographics", "culture",
    "great_work", "archaeology", "city_state", "congress", "mp_vote", "notification_log", "game_menu", "advisor",
    "end_game", "techs", "city", "policies", "ideology", "pantheon", "religion", "religion_overview", "diplo", "leader",
    "city_captured", "free_great_person"];
  function update() {
    let covered = covering.some(w => ui.model.get("ui.windows." + w + ".open") === true);
    if (!covered && ui.model.get("ui.windows.deal.open") === true) {
      const deal = ui.model.get("deal");
      covered = !!(deal && deal.partner && !deal.partner.is_human);
    }
    if (ui.state.get("hud.covered") !== covered) ui.state.set("hud.covered", covered);
  }
  for (const w of covering.concat(["deal"])) ui.model.on("ui.windows." + w + ".open", update);
  ui.model.on("deal", update);
  ui.on("ready", update);
})();
