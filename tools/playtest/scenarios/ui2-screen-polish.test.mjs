import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const source = path => readFileSync(new URL(`../../../${path}`, import.meta.url), "utf8");
const part = name => source(`skins/engraved/parts/${name}.html`);
const table = source("skins/engraved/l10n/en_US.toml");

function enumCaptions(screen, type, expression, prefix) {
  const schema = source(`crates/shared/ui2-model/schema/screens/${screen}.toml`);
  const declaration = schema.split("\n").find(line => line.startsWith(`${type} = [`));
  assert.ok(declaration, type);
  const values = JSON.parse(declaration.slice(declaration.indexOf("[")).trim());
  const html = part(screen);
  assert.ok(!html.includes(`data-bind-value="{{${expression}}}"`), expression);
  for (const value of values) {
    assert.ok(html.includes(`${expression} == '${value}'`), `${type}.${value} condition`);
    assert.ok(html.includes(`data-l10n="${prefix}.${value}"`), `${type}.${value} text binding`);
    assert.ok(table.includes(`"${prefix}.${value}" = `), `${type}.${value} caption`);
  }
}

test("city enum captions cover every supported value", () => {
  for (const [type, expression, prefix] of [
    ["CityGroupKind", "g.kind", "city.group"],
    ["CityYieldKind", "y.key", "city.yield"],
    ["CityFocus", "f.key", "city.focus"],
    ["CityBuildingKind", "g.key", "city.building"],
    ["CityTileState", "t.state", "city.tile"],
    ["CityTaskKind", "city.task.kind", "city.task"],
    ["CityGrowth", "city.food.state", "city.growth"],
  ]) enumCaptions("city", type, expression, prefix);
});

test("branch, victory and city-list captions cover their enum domains", () => {
  enumCaptions("policies", "PoliciesBranchState", "b.state", "policies.state");
  enumCaptions("ideology", "IdeologyVictoryKind", "v.kind", "ideology.victory");
  enumCaptions("city_list", "CityListGrowth", "c.growth_state", "city_list.growth");
  enumCaptions("city_list", "CityListStatus", "c.status", "city_list.status");
  enumCaptions("city_rename", "CityRenameMode", "city_rename.mode", "city_rename.mode");
  enumCaptions("techs", "TechsStatus", "t.status", "techs.status");
});

test("city catalog and compact ledgers expose existing model fields", () => {
  const html = part("city");
  for (const field of ["o.icon", "q.icon", "b.icon", "p.icon"]) {
    assert.ok(html.includes(`data-bind-src="{{${field}}}"`), field);
  }
  assert.ok(html.includes("fmt.signed(y.value)"));
  assert.ok(html.includes("{{b.yields}}"));
  assert.ok(html.includes("{{t.yields}}"));
  assert.ok(html.includes("{{b.specialist.filled}}, {{b.specialist.slots}}"));
  assert.ok(source("skins/engraved/skin.toml").includes("tiles_open = false"));
  assert.ok(html.includes('data-bind-if="{{state.city.tiles_open}}"'));
});

test("full-screen tree suppresses HUD content and retains SVG definitions", () => {
  const html = source("skins/engraved/index.html");
  const wrapper = html.indexOf('data-bind-if="{{!ui.windows.techs.open}}"');
  assert.ok(wrapper > html.indexOf('data-include="icons.svg"'));
  for (const name of ["top", "notes", "unit", "chat", "helm", "tech_panel"]) {
    assert.ok(html.indexOf(`data-include="parts/${name}.html"`) > wrapper, name);
  }
});

test("new city tooltip templates stay in the shared tooltip part", () => {
  const tips = part("screen_tips");
  for (const id of ["city-yield-tip", "city-tile-tip", "city-gp-tip"]) {
    assert.ok(tips.includes(`data-layer="tooltip" id="${id}"`));
    assert.ok(part("city").includes(`data-tooltip="${id}"`));
  }
});
