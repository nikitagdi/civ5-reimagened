(function () {
  "use strict";
  ["sort", "sort_desc"].forEach(function (field) {
    const path = "military." + field;
    ui.on("ready", function () {
      const saved = ui.settings.get(path);
      if (typeof saved === typeof ui.state.get(path)) ui.state.set(path, saved);
    });
    ui.state.on(path, function (value) { ui.settings.set(path, value); });
  });
})();
