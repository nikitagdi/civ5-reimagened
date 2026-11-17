(function () {
  "use strict";
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
