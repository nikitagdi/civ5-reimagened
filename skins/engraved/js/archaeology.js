(function () {
  "use strict";
  ui.on("archaeology.back", () => {
    if (ui.state.get("archaeology.confirm")) ui.state.set("archaeology.confirm", 0);
    else ui.intent("window.close", { "window": "archaeology" });
  });
  ui.on("archaeology.choose", () => {
    const model = ui.model.get("archaeology");
    const id = ui.state.get("archaeology.confirm");
    const choice = model && model.choices.find(item => item.id === id);
    if (!choice || !choice.enabled) return;
    ui.intent("screen.act", { screen: "archaeology", action: "choose", a: id, b: model.unit_id });
    ui.state.set("archaeology.confirm", 0);
  });
  let pending = false, focusedUnit;
  ui.model.on("ui.windows.archaeology.open", open => {
    if (open) {
      const unit = ui.model.get("unit");
      focusedUnit = unit ? unit.key : null;
    }
  });
  ui.model.on("unit", unit => {
    const key = unit ? unit.key : null;
    if (ui.model.get("ui.windows.archaeology.open") && focusedUnit !== undefined && key !== focusedUnit) {
      ui.intent("window.close", { "window": "archaeology" });
    }
    focusedUnit = key;
  });
  ui.model.on("archaeology", model => {
    if (!model) { pending = false; return; }
    if (pending && model.status === "idle") ui.intent("window.close", { "window": "archaeology" });
    if (!pending && model.status === "pending" && model.plot) ui.intent("camera.to_plot", { plot: model.plot });
    pending = model.status === "pending";
    if (!model.choices.some(item => item.id === ui.state.get("archaeology.confirm") && item.enabled)) ui.state.set("archaeology.confirm", 0);
  });
})();
