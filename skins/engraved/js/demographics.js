(function () {
  "use strict";
  let open = false;
  let token = ui.settings.get("demographics.popup_token") || 0;
  function opened(value) {
    if (value && !open) {
      open = true;
      ui.intent("screen.act", { screen: "demographics", action: "mark_open" });
      const win = ui.model.get("ui.windows.demographics");
      if (win && win.a === 1) {
        ui.settings.set("demographics.popup_token", ++token);
        ui.intent("window.open", { "window": "demographics", a: 1, b: token });
      }
    } else if (!value) open = false;
  }
  ui.model.on("ui.windows.demographics.open", opened);
  ui.on("ready", function () { opened(ui.model.get("ui.windows.demographics.open")); });
})();
