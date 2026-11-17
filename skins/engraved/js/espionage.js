(function () {
  "use strict";
  ui.on("espionage.back", function () {
    if (ui.state.get("espionage.ask") >= 0) ui.state.set("espionage.ask", -1);
    else if (ui.state.get("espionage.confirm") !== "") ui.state.set("espionage.confirm", "");
    else {
      ui.state.set("espionage.moving", -1);
      ui.intent("window.close", { "window": "espionage" });
    }
  });
  ui.model.on("ui.windows.espionage.open", function (open) {
    if (open) {
      const args = ui.model.get("ui.windows.espionage");
      if (args && typeof args.a === "number" && args.a >= 0) ui.state.set("espionage.moving", args.a);
    } else {
      ui.state.set("espionage.moving", -1);
      ui.state.set("espionage.ask", -1);
      ui.state.set("espionage.confirm", "");
    }
  });
  ui.on("ready", function () {
    const tab = ui.settings.get("espionage.tab");
    if (tab === "overview" || tab === "intrigue") ui.state.set("espionage.tab", tab);
  });
  ui.state.on("espionage.tab", function (value) { ui.settings.set("espionage.tab", value); });
  ["agents_sort", "agents_sort_desc", "own_sort", "own_sort_desc", "other_sort", "other_sort_desc", "intrigue_sort", "intrigue_sort_desc"].forEach(function (field) {
    const path = "espionage." + field;
    ui.on("ready", function () {
      const saved = ui.settings.get(path);
      if (typeof saved === typeof ui.state.get(path)) ui.state.set(path, saved);
    });
    ui.state.on(path, function (value) { ui.settings.set(path, value); });
  });
})();
