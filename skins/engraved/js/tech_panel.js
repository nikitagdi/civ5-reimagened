// SIMPLIFIED(SIMP-344): completion is recognized only after observing the research in this session; unlock metadata needs HUD enrichment.
(function () {
  "use strict";
  function update() {
    const current = ui.model.get("research.current");
    if (current) {
      ui.state.set("tech_panel.last_id", current.id);
      ui.state.set("tech_panel.last_name", current.name);
      ui.state.set("tech_panel.done", false);
    } else {
      const hud = ui.model.get("hud");
      const last = ui.state.get("tech_panel.last_id");
      ui.state.set("tech_panel.done", !!(hud && hud.research.techs.some(t => t.id === last && (t.status === 0 || t.status === "researched"))));
    }
  }
  ui.on("tech_panel.tree", function () {
    if (ui.model.get("ui.windows.techs.open")) ui.intent("window.close", {window: "techs"});
    else ui.intent("window.open", {window: "techs", a: 1});
  });
  ui.on("ready", update);
  ui.model.on("research", update);
  ui.model.on("hud", update);
}());
