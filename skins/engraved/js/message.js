(function () {
  "use strict";
  ui.model.on("message", root => {
    if (root && root.resolved) ui.intent("window.close", { "window": "message" });
  });
})();
