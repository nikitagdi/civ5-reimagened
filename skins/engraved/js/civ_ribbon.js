(function () {
  "use strict";
  function subscribe() {
    if (!ui.model.get("ui.windows.civ_ribbon.open")) ui.intent("window.open", {"window":"civ_ribbon"});
  }
  // SIMPLIFIED(SIMP-455): a subscription-only window keeps this HUD root live until service subscriptions are available.
  ui.on("ready", subscribe);
  ui.model.on("game", subscribe);
  ui.on("civ_ribbon.toggle", function () { ui.state.set("civ_ribbon.visible", !ui.state.get("civ_ribbon.visible")); });
  ui.state.on("civ_ribbon.visible", value => ui.settings.set("civ_ribbon.visible", value));
  ui.state.on("civ_ribbon.left", value => ui.settings.set("civ_ribbon.left", value));
  ui.on("ready", function () {
    for (const key of ["visible", "left"]) {
      const value=ui.settings.get("civ_ribbon."+key);
      if (typeof value === "boolean") ui.state.set("civ_ribbon."+key,value);
    }
  });
}());
