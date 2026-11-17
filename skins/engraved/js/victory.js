(function () {
  "use strict";
  // SIMPLIFIED(SIMP-378): Enter retains the runtime's focused-control precedence; Escape uses the screen back policy.
  function back() {
    if (ui.state.get("victory.view") === "summary") ui.intent("window.close", { "window": "victory" });
    else ui.state.set("victory.view", "summary");
  }
  ui.on("victory.back", back);
  ui.model.on("ui.windows.victory.open", function (open) {
    if (open) ui.state.set("victory.view", "summary");
  });
})();
