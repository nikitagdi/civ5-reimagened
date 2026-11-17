(function () {
  "use strict";
  let previous = null;
  ui.model.on("confirm", root => {
    if (!root) { previous = null; return; }
    if (previous !== root.id) {
      previous = root.id;
      ui.state.set("confirm.dont_warn", false);
      ui.state.set("confirm.hover", -1);
    }
    if (root.resolved) ui.intent("window.close", { "window": "confirm" });
  });
})();
