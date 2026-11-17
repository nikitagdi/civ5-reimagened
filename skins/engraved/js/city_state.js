(function () {
  "use strict";
  let resolving = false, observedPending = false, errorSequence = 0;
  ui.on("city-state.tip", value => ui.state.set("cs.tip", value || ""));
  ui.on("city-state.resolve", action => {
    const model = ui.model.get("city_state");
    if (resolving || !model || !model.minor) return;
    const args = { screen: "city_state", action, a: model.minor.id };
    if (action === "liberate" && model.liberate) args.b = model.liberate.city_id;
    else if (action === "puppet" && model.liberate && model.liberate.can_puppet) args.a = model.liberate.city_id;
    else if (action !== "buyout" || !model.actions.buyout.enabled) return;
    errorSequence = Math.max(0, ...(ui.model.get("ui.errors") || []).map(error => error.seq));
    resolving = true;
    observedPending = false;
    ui.intent("screen.act", args);
  });
  ui.model.on("ui.pending", rows => {
    if (!resolving) return;
    const pending = (rows || []).some(row => row.intent === "screen.act");
    if (pending) observedPending = true;
    else if (observedPending) {
      resolving = false;
      const failed = (ui.model.get("ui.errors") || []).some(error => error.seq > errorSequence);
      if (!failed) ui.intent("window.close", { "window": "city_state" });
    }
  });
  ui.on("city-state.back", () => {
    const model = ui.model.get("city_state");
    if (model && model.view !== "diplo") return;
    if (ui.state.get("cs.confirm") !== "none") ui.state.set("cs.confirm", "none");
    else ui.intent("window.close", { "window": "city_state" });
  });
  ui.on("city-state.tribute", () => {
    const model = ui.model.get("city_state"), kind = ui.state.get("cs.confirm");
    if (kind === "war" && model && model.actions && model.actions.war.enabled) {
      ui.intent("screen.act", { screen: "city_state", action: "declare_war", a: model.minor.id });
      ui.state.set("cs.confirm", "none");
    } else if (model && model.minor && model.tribute && ["gold", "unit"].includes(kind) && model.tribute[kind].enabled) {
      ui.intent("screen.act", { screen: "city_state", action: "bully_" + kind, a: model.minor.id });
      ui.state.set("cs.confirm", "none");
    }
  });
  ui.on("city-state.locate", () => {
    const model = ui.model.get("city_state");
    if (model && model.minor && model.minor.capital_location) ui.intent("camera.to_plot", { plot: model.minor.capital_location });
  });
  ui.on("city-state.war", () => {
    const model = ui.model.get("city_state");
    if (model && model.minor && model.actions.war.enabled) ui.state.set("cs.confirm", "war");
  });
  ui.model.on("city_state", model => {
    if (model && model.status && model.status.at_war) {
      ui.state.set("cs.menu", "main");
      ui.state.set("cs.confirm", "none");
    }
  });
})();
