// The mockup shim in Chromium from `file://`: a skin page with a script of its own loads without console errors,
// markup events reach the script, its state writes come back to the page's bindings and islands.
//
//   node skins/_shim/shim.test.mjs
//
// Chromium comes from the pinned Playwright of `tools/playtest` (`npm ci` there once); without it the test is
// skipped with a note.
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, "..", "..");
const API = pathToFileURL(path.join(repo, "apps", "web", "www", "ui2", "api.js")).href;
const SHIM = pathToFileURL(path.join(here, "ui2-shim.js")).href;

let chromium = null;
try {
  const require = createRequire(path.join(repo, "tools", "playtest", "package.json"));
  ({ chromium } = require("playwright"));
} catch {
  chromium = null;
}

const SAMPLE = {
  model: { top: { gold: 312 }, game: { turn: 87 } },
  state: { sort: "name", open: false },
  ui: {},
};

// The skin's script: what an author writes against `ui`.
const SCRIPT = `
ui.on("sort", (by) => ui.state.set("sort", by));
ui.on("pick", (a) => ui.state.set("picked", a));
ui.on("hot", () => ui.state.set("hot", true));
ui.state.on("open", (v) => ui.island("clock").text("t", v ? "open" : "shut").cls("t", "lit", v));
ui.on("ready", () => ui.state.set("gold", ui.model.get("top.gold")));
`;

function page({ sample = true } = {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "ui2-shim-"));
  fs.mkdirSync(path.join(dir, "js"));
  fs.writeFileSync(path.join(dir, "js", "hud.js"), SCRIPT);
  const data = sample ? `<script type="application/json" data-ui2-sample>${JSON.stringify(SAMPLE)}</script>` : "";
  fs.writeFileSync(
    path.join(dir, "index.html"),
    `<!doctype html>
<html><head><meta charset="utf-8"><title>shim</title>
${data}
<script src="${API}" data-ui2-dev></script>
<script src="${SHIM}" data-ui2-dev></script>
</head><body>
<div data-ui2-island="clock"><span data-ui2-key="t">--</span></div>
<button id="sort" data-event="sort" data-event-arg="{{'turns'}}">Sort</button>
<div id="panel" data-bind-if="{{state.open}}" data-bind-class-toggle="open: {{state.open}}; by-turns: {{state.sort == 'turns'}}">panel</div>
<span id="label" data-bind-value="Sort: {{state.sort}}">Sort: ?</span>
<span id="gold" data-bind-value="{{top.gold}}">312</span>
<button id="toggle" data-on-click="toggle state.open">toggle</button>
<button id="call" data-on-click="call pick('a', 2)">call</button>
<button id="hot" data-hotkey="KeyP" data-on-click="call hot()">hot</button>
<button id="intent" data-on-click="intent window.open(window: 'production')">intent</button>
<script src="js/hud.js"></script>
</body></html>
`,
  );
  return { dir, url: pathToFileURL(path.join(dir, "index.html")).href };
}

async function open(browser, url) {
  const p = await browser.newPage();
  const errors = [];
  p.on("console", (m) => {
    if (m.type() === "error") errors.push(m.text());
  });
  p.on("pageerror", (e) => errors.push(String(e)));
  await p.goto(url);
  await p.evaluate(() => window.__ui2shim.ready);
  // Answers are taken a microtask later; a short wait lets a chain of them settle.
  const settle = () => p.waitForTimeout(30);
  await settle();
  return { p, errors, settle, state: () => p.evaluate(() => window.__ui2shim.roots.state) };
}

test("the shim runs a skin script from file:// and binds its state", { skip: !chromium && "no Playwright in tools/playtest (npm ci there)" }, async () => {
  const browser = await chromium.launch();
  const { dir, url } = page();
  try {
    const { p, errors, settle, state } = await open(browser, url);
    assert.deepEqual(errors, [], "no console errors while loading");
    // The sample's state is on the page; the model binding stays as written (SIMP-260).
    assert.equal(await p.textContent("#label"), "Sort: name");
    assert.equal(await p.textContent("#gold"), "312");
    assert.equal(await p.evaluate(() => getComputedStyle(document.querySelector("#panel")).display), "none");
    // `ready` ran after the files and read the model.
    assert.equal((await state()).gold, 312);

    // data-event -> the script's handler -> state -> class and text.
    await p.click("#sort");
    await settle();
    assert.equal((await state()).sort, "turns");
    assert.equal(await p.textContent("#label"), "Sort: turns");
    assert.ok(await p.evaluate(() => document.querySelector("#panel").classList.contains("by-turns")));

    // A markup `toggle` writes the state; the script's subscription drives an island.
    await p.click("#toggle");
    await settle();
    assert.equal((await state()).open, true);
    assert.ok(await p.evaluate(() => document.querySelector("#panel").classList.contains("open")));
    assert.equal(await p.evaluate(() => getComputedStyle(document.querySelector("#panel")).display), "block");
    assert.equal(await p.textContent("[data-ui2-key='t']"), "open");
    assert.ok(await p.evaluate(() => document.querySelector("[data-ui2-key='t']").classList.contains("lit")));

    // `call` with arguments, a hotkey, an intent.
    await p.click("#call");
    await p.keyboard.press("KeyP");
    await p.click("#intent");
    await settle();
    const s = await state();
    assert.deepEqual(s.picked, ["a", 2]);
    assert.equal(s.hot, true);
    const intents = await p.evaluate(() => window.__ui2shim.intents);
    assert.deepEqual(intents, [{ name: "window.open", args: { window: "production" } }]);
    assert.deepEqual(errors, [], "no console errors after the clicks");
  } finally {
    await browser.close();
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("with ?ui2shim=off the scripts load and the page is left alone", { skip: !chromium && "no Playwright" }, async () => {
  const browser = await chromium.launch();
  const { dir, url } = page();
  try {
    const { p, errors, settle } = await open(browser, url + "?ui2shim=off");
    await p.click("#sort");
    await settle();
    assert.equal(await p.textContent("#label"), "Sort: ?", "no bindings applied");
    assert.deepEqual(errors, []);
  } finally {
    await browser.close();
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("without a sample the page loads quietly with an empty model", { skip: !chromium && "no Playwright" }, async () => {
  const browser = await chromium.launch();
  const { dir, url } = page({ sample: false });
  try {
    const { p, errors, settle, state } = await open(browser, url);
    assert.deepEqual(errors, []);
    assert.equal((await state()).gold, null, "top.gold is not there");
    await p.click("#sort");
    await settle();
    assert.equal(await p.textContent("#label"), "Sort: turns");
    assert.deepEqual(errors, []);
  } finally {
    await browser.close();
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
