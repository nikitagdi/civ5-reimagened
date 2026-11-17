(function () {
  "use strict";
  ui.on("trade_routes.back", function () {
    if (ui.state.get("trade_routes.confirm") >= 0) ui.state.set("trade_routes.confirm", -1);
    else ui.intent("window.close", { "window": "trade_routes" });
  });
  ui.model.on("ui.windows.trade_routes.open", function () {
    ui.state.set("trade_routes.confirm", -1);
    ui.state.set("trade_routes.overview", false);
  });
  ui.model.on("ui.windows.trade_routes.a", function (tab) {
    if (tab === 0 || tab === 1 || tab === 2) ui.state.set("trade_routes.tab", ["yours", "available", "to_you"][tab]);
  });
  ["tab", "sort", "sort_desc", "choose_sort", "collapse_yours", "collapse_majors", "collapse_city_states"].forEach(function (field) {
    const path = "trade_routes." + field;
    ui.on("ready", function () {
      const saved = ui.settings.get(path);
      if (typeof saved === typeof ui.state.get(path)) ui.state.set(path, saved);
    });
    ui.state.on(path, function (value) { ui.settings.set(path, value); });
  });
})();
