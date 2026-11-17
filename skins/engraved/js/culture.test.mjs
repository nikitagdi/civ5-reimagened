import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import * as generated from "../../../apps/web/www/ui2/model.gen.js";

function mount() {
  const model = JSON.parse(readFileSync(new URL("../sample/culture.json", import.meta.url))).model.culture;
  const state = new Map(Object.entries({ pick: -1, click_mode: "move", pair_work: -1, pair_partner: -1, source: -1 }).map(([key, value]) => ["culture." + key, value]));
  const handlers = new Map(), watches = new Map(), intents = [];
  const ui = {
    on: (name, callback) => handlers.set(name, callback),
    model: { get: path => path === "culture" ? model : null, on: (path, callback) => watches.set(path, callback) },
    state: { get: key => state.get(key), set: (key, value) => state.set(key, value), on: (path, callback) => watches.set(path, callback) },
    settings: { get: () => null, set() {} },
    intent: (name, args) => intents.push({ name, args }),
  };
  vm.runInNewContext(readFileSync(new URL("culture.js", import.meta.url), "utf8"), { ui });
  return { model, state, handlers, watches, intents };
}

test("compatible slots send a move and view opens only viewable works", () => {
  const m = mount();
  const slots = m.model.cities.flatMap(city => city.cells.concat(city.extra).flatMap(cell => cell.slots));
  const from = slots.find(slot => slot.work && slot.work.viewable);
  const to = slots.find(slot => slot.id !== from.id && slot.slot_type === from.slot_type);
  m.handlers.get("culture.slot")(from.id);
  m.watches.get("culture")(m.model);
  assert.equal(m.state.get("culture.pick"), from.id);
  m.handlers.get("culture.slot")(to.id);
  assert.equal(m.intents[0].name, "screen.act");
  assert.equal(m.intents[0].args.a, from.id);
  assert.equal(m.intents[0].args.b, to.id);
  m.handlers.get("culture.view")(from.id);
  assert.equal(m.intents[1].args.window, "great_work");
  const artifact = slots.find(slot => slot.work && !slot.work.viewable);
  m.handlers.get("culture.view")(artifact.id);
  assert.equal(m.intents.length, 2);
});

test("swap refreshes use the current offer and discard vanished partner works", () => {
  const m = mount();
  const partner = m.model.swap.partners.find(item => item.art);
  m.handlers.get("culture.pair")(partner.art.id);
  const offer = m.model.swap.offers.find(item => item.class === "art");
  assert.equal(m.state.get("culture.own_work"), offer.work.id);
  offer.work = null;
  m.watches.get("culture")(m.model);
  assert.equal(m.state.get("culture.own_work"), -1);
  partner.art = null;
  m.watches.get("culture")(m.model);
  assert.equal(m.state.get("culture.pair_partner"), -1);
});

test("opening without a tab argument preserves the remembered tab", () => {
  const m = mount();
  m.state.set("culture.tab", "influence");
  m.watches.get("ui.windows.culture.open")(true);
  assert.equal(m.state.get("culture.tab"), "influence");
});

test("all group scripts load under the real API and its readonly model rules", () => {
  const sandbox = vm.createContext({ setTimeout, clearTimeout, setInterval, clearInterval });
  vm.runInContext(readFileSync(new URL("../../../apps/web/www/ui2/api.js", import.meta.url), "utf8"), sandbox);
  const schema = {
    hash: generated.SCHEMA_HASH,
    parts: [{
      base: 0,
      roots: generated.ROOTS.map(([name, type], id) => [id, name, type]),
      enums: generated.ENUMS,
      types: generated.TYPES,
      paths: generated.PATHS.map(path => [path.path, path.type, path.key ?? null, path.js ?? false]),
    }, {
      base: generated.PATHS.length,
      roots: [[generated.ROOTS.length, "state", ["struct", "state.Root"]]],
      enums: {}, types: { "state.Root": [] },
      paths: [["state", ["struct", "state.Root"], null, false]],
    }],
  };
  const posted = [];
  const context = sandbox.__ui2api.createContext({
    t: "init", api: "1.0.0", ctx: "skin", files: [], model: null, schema,
    reads: ["production", "culture", "archaeology", "city_state", "congress", "mp_vote", "ui", "unit", "turn"],
    ns: { state: "state", intents: null }, settings: [],
    env: { lang: "en_US", viewport: [1366, 768], scale: 1, cell: 0, skin: "engraved" },
    dev: false,
  }, { post: value => posted.push(value), now: () => 0 });
  sandbox.ui = context.ui;
  for (const file of ["culture", "archaeology", "city_state", "congress", "mp_vote"]) {
    vm.runInContext(readFileSync(new URL(file + ".js", import.meta.url), "utf8"), sandbox, { filename: file + ".js" });
  }
  context.ready();
  assert.deepEqual(posted.flatMap(value => value.log).filter(line => line.level === "error"), []);
});
