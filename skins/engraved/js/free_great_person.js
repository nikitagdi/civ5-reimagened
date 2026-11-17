(function () {
  "use strict";
  let mode = null;
  let awaiting = null;
  ui.model.on("free_great_person", function (root) {
    if (!root || root.mode !== mode) {
      mode = root ? root.mode : null;
      awaiting = null;
      ui.state.set("free_gp.selected", -1);
    }
    if (root && awaiting && root.mode === awaiting.mode && root.remaining < awaiting.remaining) {
      awaiting = null;
      ui.intent("window.close", {"window": "free_great_person"});
    }
  });
  ui.on("free_gp.confirm", function (unit) {
    const root = ui.model.get("free_great_person");
    if (!root || !root.people.some(p => p.unit === unit && p.available)) return;
    awaiting = {mode: root.mode, remaining: root.remaining};
    ui.intent("screen.act", {screen: "free_great_person", action: "choose", a: unit, b: root.mode_id});
  });
})();
