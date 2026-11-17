(function () {
  "use strict";
  let present = false;
  let hadIdeology = false;
  const open = [];
  for (const name of ["policies", "ideology", "pantheon", "religion", "religion_overview"]) {
    ui.model.on(name, value => {
      const at = open.indexOf(name);
      if (!value && at >= 0) open.splice(at, 1);
      if (value && at < 0) open.push(name);
    });
  }
  ui.on("policies.escape", () => {
    const windows = ui.model.get("ui.windows");
    const name = open[open.length - 1] || ["religion_overview", "religion", "pantheon", "ideology", "policies"].find(n => windows && windows[n] && windows[n].open);
    if (!name) return;
    const get = key => ui.state.get(name + "." + key);
    const set = (key, value) => ui.state.set(name + "." + key, value);
    if (name === "policies") {
      if (get("confirm") >= 0) { set("confirm", -1); return; }
      if (get("tenet_level") > 0) { set("tenet_level", 0); return; }
    } else if (name === "ideology") {
      if (get("details") >= 0) { set("details", -1); return; }
      if (get("confirm") >= 0) { set("confirm", -1); return; }
    } else if (name === "pantheon" && get("confirm") >= 0) {
      set("confirm", -1); return;
    } else if (name === "religion") {
      if (get("confirm")) { set("confirm", false); return; }
      if (get("rename")) { set("rename", false); return; }
      if (get("list")) { set("list", ""); return; }
    } else if (name === "religion_overview" && get("menu")) {
      set("menu", false); return;
    }
    ui.intent("window.close", {"window": name});
  });
  ui.on("ready", () => {
    const expanded = ui.settings.get("policies.expanded");
    if (typeof expanded === "boolean") ui.state.set("policies.expanded", expanded);
  });
  ui.state.on("policies.expanded", value => ui.settings.set("policies.expanded", value));
  ui.model.on("policies", value => {
    if (!value) {
      present = false;
      hadIdeology = false;
      ui.state.set("policies.confirm", -1);
      ui.state.set("policies.tenet_level", 0);
      ui.state.set("policies.tab", "");
    } else {
      if (hadIdeology && !value.ideology) {
        ui.intent("window.close", {"window": "policies"});
        ui.intent("window.open", {"window": "ideology"});
      }
      hadIdeology = !!value.ideology;
      if (!present) {
        present = true;
        ui.state.set("policies.tab", value.open_tab);
      }
    }
  });
})();
