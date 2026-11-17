(function () {
  "use strict";
  ui.model.on("ideology", value => {
    if (!value) {
      ui.state.set("ideology.confirm", -1);
      ui.state.set("ideology.details", -1);
    } else if (value.current !== null && value.current !== undefined) {
      ui.intent("window.close", { "window": "ideology" });
    }
  });
})();
