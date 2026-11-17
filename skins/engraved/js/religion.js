(function () {
  "use strict";
  const keys = ["pantheon", "founder", "follower", "follower2", "enhancer", "bonus"];
  let root = null;
  let submitted = false;
  const get = key => ui.state.get("religion." + key);
  const set = (key, value) => ui.state.set("religion." + key, value);
  function refresh() {
    if (!root) return;
    const symbol = root.symbols.find(s => s.id === get("symbol") && (!s.taken || root.mode === "enhance"));
    if (!symbol && root.mode === "found") set("symbol", -1);
    set("symbol_name", root.mode === "enhance" && root.religion ? root.religion.name : symbol ? symbol.name : "");
    const used = new Set(root.slots.filter(s => s.fixed).map(s => s.fixed.id));
    let ready = root.enabled && (root.mode === "enhance" || !!symbol);
    for (const slot of root.slots) {
      let id = get("pick." + slot.key);
      let choice = slot.options.find(b => b.id === id);
      if (!choice || used.has(id)) { set("pick." + slot.key, 65535); choice = null; }
      if (choice) used.add(id);
      set("label." + slot.key, choice ? choice.name : "");
      if (slot.required && !choice) ready = false;
    }
    if (get("name").trim() && !root.custom_name_available) ready = false;
    set("ready", !!ready);
  }
  ui.on("religion.symbol", id => { set("symbol", id); refresh(); });
  ui.on("religion.pick", (...p) => {
    const [key, id] = Array.isArray(p[0]) ? p[0] : p;
    if (keys.includes(key)) { set("pick." + key, id); set("list", ""); refresh(); }
  });
  ui.state.on("religion.name", refresh);
  ui.on("religion.submit", () => {
    refresh();
    if (!root || !get("ready") || !root.city) return;
    // Preserve the packed unsigned pair through the screen argument's signed 32-bit representation.
    const pair = (a, b) => {
      const value = a + b * 65536;
      return value > 2147483647 ? value - 4294967296 : value;
    };
    const enhance = root.mode === "enhance";
    ui.intent("screen.act", {screen: "religion", action: enhance ? "enhance" : "found",
      a: enhance ? root.religion.id : get("symbol"), b: root.city.id,
      c: enhance ? get("pick.follower2") : pair(get("pick.founder"), get("pick.follower")),
      d: enhance ? get("pick.enhancer") : pair(get("pick.pantheon"), get("pick.bonus")), text: get("name").trim()});
    submitted = true;
    set("confirm", false);
  });
  ui.model.on("religion", value => {
    root = value;
    if (!value) {
      submitted = false;
      set("symbol", -1); set("name", ""); set("list", ""); set("confirm", false); set("rename", false);
      keys.forEach(k => { set("pick." + k, 65535); set("label." + k, ""); });
      return;
    }
    if (submitted && value.religion && (value.mode === "found" || value.slots.some(s => s.key === "follower2" && s.state === "fixed"))) {
      ui.intent("window.close", {"window": "religion"});
    }
    refresh();
  });
})();
