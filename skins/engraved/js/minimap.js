(function () {
  "use strict";
  ["banners", "flags", "streamer"].forEach(function (key) {
    ui.on("ready", function () {
      const value = ui.settings.get("minimap." + key);
      if (typeof value === "boolean") ui.state.set("minimap." + key, value);
    });
    ui.state.on("minimap." + key, function (value) { ui.settings.set("minimap." + key, value); });
  });
}());
