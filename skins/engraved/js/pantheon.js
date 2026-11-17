(function () {
  "use strict";
  let submitted = false;
  ui.state.on("pantheon.confirm", value => { if (value >= 0) submitted = true; });
  ui.model.on("pantheon", value => {
    if (!value) {
      submitted = false;
      ui.state.set("pantheon.confirm", -1);
    } else if (value.completed && submitted) {
      ui.intent("window.close", {"window": "pantheon"});
    } else if (!value.beliefs.some(b => b.id === ui.state.get("pantheon.confirm"))) {
      ui.state.set("pantheon.confirm", -1);
    }
  });
})();
