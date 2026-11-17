(function () {
  "use strict";
  ui.on("leader.escape", function () {
    const root=ui.model.get("leader");
    if (!root || !root.can_close) return;
    const action=["coop_target", "confirm_denounce", "confirm_war"].includes(root.mode) ? "back" : "goodbye";
    ui.intent("screen.act", {screen:"leader", action});
  });
}());
