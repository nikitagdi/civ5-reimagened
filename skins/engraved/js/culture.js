(function () {
  "use strict";
  const get = key => ui.state.get("culture." + key);
  const set = (key, value) => ui.state.set("culture." + key, value);
  const root = () => ui.model.get("culture");

  function findSlot(id) {
    const model = root();
    if (!model) return null;
    for (const city of model.cities) {
      for (const cell of city.cells.concat(city.extra)) {
        const slot = cell.slots.find(item => item.id === id);
        if (slot) return slot;
      }
    }
    return null;
  }

  function pair() {
    const model = root();
    if (!model) return;
    const partner = model.swap.partners.find(item => item.id === get("pair_partner"));
    const theirs = partner && [partner.writing, partner.art, partner.artifact].find(item => item && item.id === get("pair_work"));
    const offer = theirs && model.swap.offers.find(item => item.class === theirs.class);
    const ours = offer && offer.work;
    set("own_work", ours ? ours.id : -1);
    set("pair_label", theirs ? theirs.name + (ours ? " / " + ours.name : "") : "");
    set("reason", theirs ? "Offer one of your works of the same class first." : "Pick a work from another civilization.");
    if (!theirs) {
      set("pair_partner", -1);
      set("pair_work", -1);
    }
  }

  ui.on("culture.clear", () => {
    set("pair_partner", -1);
    set("pair_work", -1);
    pair();
  });
  ui.on("culture.pair", id => {
    const model = root();
    if (!model) return;
    for (const partner of model.swap.partners) {
      if ([partner.writing, partner.art, partner.artifact].some(item => item && item.id === id)) {
        set("pair_partner", partner.id);
        set("pair_work", id);
        pair();
        return;
      }
    }
  });
  function view(id) {
    const slot = findSlot(id);
    if (slot && slot.work && slot.work.viewable) ui.intent("window.open", { "window": "great_work", a: slot.work.id });
  }
  ui.on("culture.view", view);
  ui.on("culture.slot", id => {
    const slot = findSlot(id);
    if (!slot) return;
    if (get("click_mode") === "view") return view(id);
    const from = findSlot(get("pick"));
    if (from && from.id !== id && from.slot_type === slot.slot_type) {
      ui.intent("screen.act", { screen: "culture", action: "move", a: from.id, b: id });
      set("pick", -1);
    } else if (slot.work) {
      set("pick", slot.id);
      set("slot_type", slot.slot_type);
    }
  });
  ui.on("culture.tip", value => set("tip", value || ""));
  ui.on("culture.slot-tip", id => {
    const slot = findSlot(id);
    set("tip", slot && slot.work ? slot.work.tooltip : "Empty great work slot");
  });
  function refresh(model) {
    if (!model) return;
    if (!findSlot(get("pick"))) set("pick", -1);
    if (!model.influence.sources.some(item => item.id === get("source"))) set("source", model.player.id);
    pair();
  }
  ui.model.on("culture", refresh);
  ui.model.on("ui.windows.culture.open", open => {
    if (!open) return;
    const tabs = ["yours", "swap", "victory", "influence"];
    const tab = ui.model.get("ui.windows.culture.a");
    if (Number.isInteger(tab) && tab >= 0 && tab < tabs.length) set("tab", tabs[tab]);
  });
  ui.on("ready", () => {
    for (const key of ["tab", "click_mode"]) {
      const value = ui.settings.get("culture." + key);
      const allowed = key === "tab" ? ["yours", "swap", "victory", "influence"] : ["move", "view"];
      if (allowed.includes(value)) set(key, value);
    }
    refresh(root());
  });
  for (const key of ["tab", "click_mode"]) {
    ui.state.on("culture." + key, value => ui.settings.set("culture." + key, value));
  }
})();
