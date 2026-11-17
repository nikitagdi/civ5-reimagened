(function () {
  "use strict";
  // SIMPLIFIED(SIMP-356): Required-choice Escape/Enter suppression and popup ordering need host support.
  let awaiting = null;
  let currentCity = null;
  ui.model.on("city_rename", function (root) {
    if (!root) { awaiting = null; currentCity = null; }
    else if (root.city !== currentCity) {
      currentCity = root.city;
      ui.state.set("city_rename.draft", typeof root.name === "string" ? root.name : "");
      ui.state.set("city_rename.touched", false);
      ui.state.set("city_rename.valid", false);
      ui.state.set("city_rename.why", "");
    }
    else if (awaiting && root.city === awaiting.city && root.mode === awaiting.mode && root.done) {
      awaiting = null;
      ui.intent("window.close", {"window": "city_rename"});
    }
  });
  ui.on("city_rename.confirm", function () {
    const root = ui.model.get("city_rename");
    if (!root || !root.enabled || !root.task) return;
    awaiting = {city: root.city, mode: root.mode};
    ui.intent("screen.act", {screen: "city_rename", action: "do_task", a: root.city, b: root.task.id});
  });
  ui.on("city_rename.validate", function () {
    const draft = String(ui.state.get("city_rename.draft") || "");
    const name = draft.trim();
    const valid = [...name.replace(/\s/gu, "")].length >= 3 && [...name].length <= 15 && !/[%"<>|/\\*?\[\]\u0000-\u001f\u007f-\u009f]/u.test(name);
    ui.state.set("city_rename.touched", true);
    ui.state.set("city_rename.valid", valid);
    ui.state.set("city_rename.why", valid ? "" : "Use 3 to 15 characters without reserved symbols.");
  });
})();
