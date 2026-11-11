// Engraved: the script of the production window. A classic script; it sees only `ui`.
//
// - The arrow keys (the window's up and down buttons, `data-hotkey` ArrowUp / ArrowDown, `call prod.step`) move the
//   focused option (`ui.production.focus`) through the list in the order the window shows it: the same tab, the
//   same filter (`state.prod.hide_locked`) and the same sort key (`state.prod.sort`) as the markup's
//   `data-bind-filter` and `data-bind-sort`, and the runtime's order of keys (numbers, then texts by code point,
//   nothing last; equal keys keep the model's order).
// - The sort and "hide locked" are kept in the skin's settings and put back into `state.prod.*` when the script
//   starts.
(function () {
  "use strict";

  // The sorts the markup knows (a name is a game text, which the profile does not sort by).
  const SORTS = ["order", "turns"];
  // The group kinds each tab shows (`all` shows every group).
  const TABS = {
    units: ["unit"],
    buildings: ["building"],
    wonders: ["wonder"],
    other: ["project", "process"],
  };

  // Set when the host has no skin state: the sort and the filter are their defaults then.
  let stateless = false;

  function stateGet(path, fallback) {
    if (stateless) return fallback;
    try {
      const v = ui.state.get(path);
      return v === null || v === undefined ? fallback : v;
    } catch (e) {
      return fallback;
    }
  }

  function sortKey(o, sort) {
    return sort === "turns" ? o.turns : o.order;
  }

  // 0: a number, 1: a text, 2: nothing.
  function rank(v) {
    if (typeof v === "number") return 0;
    if (typeof v === "string") return 1;
    return 2;
  }

  function compare(a, b) {
    const ra = rank(a);
    const rb = rank(b);
    if (ra !== rb) return ra - rb;
    if (ra === 2 || a === b) return 0;
    return a < b ? -1 : 1;
  }

  // The keys of the options in the order the window lists them.
  function shownItems() {
    const prod = ui.model.get("production");
    if (!prod || !Array.isArray(prod.groups)) return [];
    const tab = ui.state.get("ui.production.tab") || "all";
    const kinds = TABS[tab] || null;
    const sort = stateGet("prod.sort", "order");
    const hide = !!stateGet("prod.hide_locked", false);
    const out = [];
    for (const g of prod.groups) {
      if (kinds && kinds.indexOf(g.kind) < 0) continue;
      const rows = (g.options || [])
        .filter((o) => !hide || o.enabled)
        .map((o, i) => ({ o, i, k: sortKey(o, sort) }));
      rows.sort((a, b) => compare(a.k, b.k) || a.i - b.i);
      for (const r of rows) out.push(r.o.item);
    }
    return out;
  }

  // Moves the focus `step` rows (-1 up, +1 down); with no focus in the list, to its first or last row.
  function move(step) {
    const items = shownItems();
    if (!items.length) return;
    const at = items.indexOf(ui.state.get("ui.production.focus"));
    let next;
    if (at < 0) next = step > 0 ? 0 : items.length - 1;
    else next = Math.min(items.length - 1, Math.max(0, at + step));
    if (next !== at) ui.state.set("ui.production.focus", items[next]);
  }

  ui.on("prod.step", (step) => move(typeof step === "number" && step < 0 ? -1 : 1));

  ui.on("ready", () => {
    if (stateless) return;
    const sort = ui.settings.get("prod.sort");
    if (SORTS.indexOf(sort) >= 0 && sort !== stateGet("prod.sort", "order")) ui.state.set("prod.sort", sort);
    const hide = ui.settings.get("prod.hide_locked");
    if (typeof hide === "boolean" && hide !== stateGet("prod.hide_locked", false)) ui.state.set("prod.hide_locked", hide);
  });

  // A host that has not mounted the skin's state refuses `state.*` paths: the script must still load (a skin
  // script that fails to load takes the whole skin down), so the window then keeps the game's order.
  try {
    ui.state.on("prod.sort", (v) => {
      if (SORTS.indexOf(v) >= 0) ui.settings.set("prod.sort", v);
    });
    ui.state.on("prod.hide_locked", (v) => {
      if (typeof v === "boolean") ui.settings.set("prod.hide_locked", v);
    });
  } catch (e) {
    stateless = true;
    ui.log.warn("engraved: no skin state (" + e.message + "); the production list keeps the game's order");
  }
})();
