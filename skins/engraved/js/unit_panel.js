(function () {
  "use strict";
  let unit = -1;
  ui.on("on_map.hover", function (...p) { const [kind, key] = Array.isArray(p[0]) ? p[0] : p; ui.state.set("on_map." + kind, String(key)); });
  ui.on("unit_panel.act", function (...p) {
    let [id, client, alt, shift] = Array.isArray(p[0]) ? p[0] : p;
    const selected = ui.model.get("unit");
    const hud = ui.model.get("hud");
    if (!selected || !hud || !hud.unit || hud.unit.id !== Number(String(selected.key).split(":").pop())) return;
    const row = hud.unit.catalog.find(a => a.id === id);
    if (!row || !row.enabled) return;
    client = row.client;
    if (client === "trade_route" || client === "trade_home" || client === "admiral_port") {
      ui.intent("window.open", { "window": "trade_routes", a: client === "trade_home" ? 3 : (client === "admiral_port" ? 4 : 0), b: hud.unit.id });
    }
    else if (client === "move" || client === "range_attack") ui.intent("unit.action", { unit: selected.key, action: client });
    else ui.intent("screen.act", { screen: "hud", action: "do_action", a: hud.unit.id, b: id, c: alt ? 1 : 0, d: shift ? 1 : 0 });
  });
  ui.model.on("unit_rename", function (root) {
    if (!root) { unit = -1; return; }
    if (root.unit !== unit) {
      unit = root.unit;
      ui.state.set("unit_panel.draft", typeof root.name === "string" ? root.name : "");
      ui.state.set("unit_panel.valid", false);
    }
  });
  ui.on("unit_panel.validate", function () {
    const name = String(ui.state.get("unit_panel.draft") || "").trim();
    ui.state.set("unit_panel.valid", [...name.replace(/\s/gu, "")].length >= 3 && [...name].length <= 15 && !/[%"<>|/\\*?\[\]\u0000-\u001f\u007f-\u009f]/u.test(name));
  });
}());
