(function () {
  "use strict";
  const fields = ["tab", "sort", "sort_desc"];
  ui.on("ready", function () {
    fields.forEach(function (field) {
      const path = "economy." + field;
      const saved = ui.settings.get(path);
      if (typeof saved === typeof ui.state.get(path)) ui.state.set(path, saved);
    });
  });
  fields.forEach(function (field) {
    const path = "economy." + field;
    ui.state.on(path, function (value) { ui.settings.set(path, value); });
  });
  ui.model.on("ui.windows.economy.a", function (tab) {
    if (tab === 1) ui.state.set("economy.tab", "happiness");
  });
})();
