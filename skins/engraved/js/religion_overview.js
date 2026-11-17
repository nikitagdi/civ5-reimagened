(function () {
  "use strict";
  const kinds = {none: 0, save_prophet: 1, unit: 2, building: 3};
  ui.on("ready", () => {
    const tab = ui.settings.get("religion_overview.tab");
    if (["mine", "world", "beliefs"].includes(tab)) ui.state.set("religion_overview.tab", tab);
  });
  ui.state.on("religion_overview.tab", value => ui.settings.set("religion_overview.tab", value));
  ui.on("religion_overview.sort", (...p) => {
    const [table, key] = Array.isArray(p[0]) ? p[0] : p;
    const allowed = table === "world" ? ["name", "holy_city", "founder", "cities"] : ["name", "type", "religion_name"];
    if (!["world", "beliefs"].includes(table) || !allowed.includes(key)) return;
    const prefix = "religion_overview." + table;
    const same = ui.state.get(prefix + "_sort") === key;
    ui.state.set(prefix + "_desc", same ? !ui.state.get(prefix + "_desc") : true);
    ui.state.set(prefix + "_sort", key);
  });
  ui.on("religion_overview.purchase", key => {
    const root = ui.model.get("religion_overview");
    const option = root && root.auto_purchase.options.find(o => o.key === key);
    if (!option || !root.enabled) return;
    ui.state.set("religion_overview.optimistic", option.label);
    ui.state.set("religion_overview.menu", false);
    ui.intent("screen.act", {screen: "religion_overview", action: "auto_purchase", a: kinds[option.kind], b: option.id});
  });
  ui.model.on("religion_overview", () => ui.state.set("religion_overview.optimistic", ""));
})();
