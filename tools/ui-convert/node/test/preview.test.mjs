// The preview script in a real page: includes, bindings, l10n, node numbering, modes, errors.
// Uses the converter's own server (preview: true) and Chromium. The skin under test is skins/engraved
// (the top strip of the mockup) plus a small fixture skin for the corners.
// Run: node --test "tools/ui-convert/node/test/*.test.mjs"
// Set UI2_PREVIEW_SHOT=<file.png> to keep a 1920x1080 screenshot of the top strip.
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { startServer } from "../server.mjs";
import { launchChromium } from "../chromium.mjs";
import { createSession } from "../session.mjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO = path.resolve(HERE, "..", "..", "..", "..");
const ENGRAVED = path.join(REPO, "skins", "engraved");
const FIXTURE = path.join(HERE, "fixtures", "preview");

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "ui2-preview-"));
// The fonts the skin's @font-face rules name, taken from the repository's own font folder.
const fontsDir = path.join(tmp, "fonts-root");
fs.mkdirSync(path.join(fontsDir, "fonts"), { recursive: true });
for (const [name, from] of [["FiraSans-Regular.ttf", "fira-sans/FiraSans-Regular.ttf"], ["FiraSans-Medium.ttf", "fira-sans/FiraSans-Medium.ttf"], ["Alegreya.ttf", "alegreya/Alegreya[wght].ttf"]]) {
  fs.copyFileSync(path.join(REPO, "third_party", "fonts", from), path.join(fontsDir, "fonts", name));
}

let browser;
let engraved;
let fixture;
const external = [];

before(async () => {
  engraved = await startServer({ root: ENGRAVED, fontsDir, preview: true });
  fixture = await startServer({ root: FIXTURE, preview: true });
  browser = await launchChromium();
});

after(async () => {
  await browser?.close();
  await engraved?.close();
  await fixture?.close();
  fs.rmSync(tmp, { recursive: true, force: true });
});

async function newContext(init) {
  const ctx = await browser.newContext({ viewport: { width: 1920, height: 1080 }, deviceScaleFactor: 1, serviceWorkers: "block" });
  await ctx.route("**/*", (route) => {
    const url = route.request().url();
    if (/^http:\/\/127\.0\.0\.1:/.test(url) || /^(data|blob|about):/.test(url)) return route.continue();
    external.push(url);
    return route.abort("blockedbyclient");
  });
  if (init) await ctx.addInitScript(init);
  return ctx;
}

/** Opens a page of a started server; resolves when the preview is ready. */
async function openPage(ctx, server, file) {
  const page = await ctx.newPage();
  const problems = [];
  page.on("console", (m) => { if (m.type() === "error") problems.push(m.text()); });
  page.on("pageerror", (e) => problems.push(String(e)));
  await page.goto(`${server.url}/${file}`, { waitUntil: "load" });
  await page.evaluate(() => window.__ui2preview.ready);
  return { page, problems };
}

const nodeList = (page) => page.evaluate(() => [...document.querySelectorAll("[data-ui2-node]")]
  .map((e) => [e.getAttribute("data-ui2-node"), e.getAttribute("data-ui2-inst") || "", e.localName, e.getAttribute("data-ui2-src") || ""].join("|")));
const text = (page, sel) => page.evaluate((s) => { const e = document.querySelector(s); return e ? e.textContent : null; }, sel);
const texts = (page, sel) => page.evaluate((s) => [...document.querySelectorAll(s)].map((e) => e.textContent), sel);

// --- the engraved top strip ---

test("engraved: includes are expanded and the strip shows the mockup numbers", async () => {
  const ctx = await newContext();
  try {
    const { page, problems } = await openPage(ctx, engraved, "index.html?ui2state=hud");
    assert.equal(await page.evaluate(() => document.querySelectorAll("[data-include]").length), 0, "no include left");
    assert.equal(await page.evaluate(() => !!document.querySelector('header.top[data-part="top"]')), true, "top.html is in");
    assert.equal(await page.evaluate(() => !!document.querySelector("svg symbol#i-sci")), true, "icons.svg is in");
    assert.equal(await page.evaluate(() => document.querySelectorAll('script[src$="preview.js"]').length), 1, "only the page's own script tag");

    const t = (part, sel) => text(page, `[data-part="top.yield.${part}"] ${sel}`);
    assert.equal(await t("science", ".v"), "+48");
    assert.equal(await t("gold", ".v"), "312");
    assert.equal(await t("gold", ".d"), "+21");
    assert.equal(await t("happiness", ".v"), "7");
    assert.equal(await t("happiness", ".d"), "412/650");
    assert.equal(await t("culture", ".v"), "+31");
    assert.equal(await t("culture", ".d"), "6 turns");
    assert.equal(await t("faith", ".v"), "140");
    assert.equal(await t("faith", ".d"), "+9");
    assert.equal(await t("tourism", ".v"), "4");
    assert.equal(await text(page, ".research .name"), "Education");
    assert.equal(await text(page, ".research .turns"), "3 turns");
    assert.equal(await text(page, ".research .next"), "Astronomy next");
    assert.equal(await page.evaluate(() => document.querySelector(".research .fill").style.width), "62%");
    assert.equal(await page.evaluate(() => document.querySelector(".research .ticks").style.backgroundSize), "12.6667% 100%");
    // gold and faith rates are positive: no `neg` class
    assert.equal(await page.evaluate(() => document.querySelectorAll(".yield .d.neg").length), 0);

    // the resource row: one instance per sample entry, keyed by the item id, `off` when none is left
    const res = await page.evaluate(() => [...document.querySelectorAll('[data-part="top.resources"] > .yield')].map((e) => ({
      inst: e.getAttribute("data-ui2-inst"), v: e.querySelector(".v").textContent, off: e.classList.contains("off"),
      asset: e.querySelector("img").getAttribute("data-ui2-asset"), src: e.querySelector("img").getAttribute("src").slice(0, 5),
    })));
    assert.deepEqual(res.map((r) => [r.inst, r.v, r.off]), [["0:1", "2", false], ["0:2", "4", false], ["0:3", "0", true]]);
    assert.deepEqual(res.map((r) => r.asset), ["icon:RESOURCE_ATLAS:1", "icon:RESOURCE_ATLAS:2", "icon:RESOURCE_ATLAS:3"]);
    assert.ok(res.every((r) => r.src === "data:"), "assets show a placeholder, no request leaves");

    const errors = await page.evaluate(() => window.__ui2preview.errors);
    assert.deepEqual(errors, []);
    assert.deepEqual(problems, [], "no console errors");

    if (process.env.UI2_PREVIEW_SHOT) {
      fs.mkdirSync(path.dirname(process.env.UI2_PREVIEW_SHOT), { recursive: true });
      await page.screenshot({ path: process.env.UI2_PREVIEW_SHOT, clip: { x: 0, y: 0, width: 1920, height: 1080 } });
    }
  } finally {
    await ctx.close();
  }
  assert.deepEqual(external, [], "no request left the local server");
});

test("engraved: node numbering is the same on every load and follows the rule", async () => {
  const ctx = await newContext();
  try {
    const a = await openPage(ctx, engraved, "index.html");
    const first = await nodeList(a.page);
    const b = await openPage(ctx, engraved, "index.html");
    const second = await nodeList(b.page);
    assert.ok(first.length > 40, `${first.length} numbered elements`);
    assert.equal(first.join("\n"), second.join("\n"), "byte-identical list");

    // the layers are the first nodes of the document template (template 0); <body> and the dev link are not nodes
    const layers = await a.page.evaluate(() => [...document.querySelectorAll("body > .layer")].map((e) => e.getAttribute("data-ui2-node")));
    assert.deepEqual(layers.slice(0, 2), ["0:0", "0:1"]);
    assert.ok(layers.every((l) => l.startsWith("0:")));
    // the three `if` nodes and the `for` node are roots of their own templates: element 0 there
    const roots = await a.page.evaluate(() => [...document.querySelectorAll("[data-bind-if],[data-bind-for]")].map((e) => e.getAttribute("data-ui2-node")));
    assert.ok(roots.every((r) => r.endsWith(":0") && !r.startsWith("0:")), roots.join(" "));
    assert.equal(new Set(roots).size, 4, "three `if` roots and one `for` root (its three instances share a number)");
    // inside an instance: the same numbers as the template, plus the key path
    const inst = first.filter((l) => l.split("|")[1] !== "");
    assert.ok(inst.length >= 6 && inst.every((l) => /^\d+:\d+\|0:[123]\|/.test(l)), inst.join("\n"));
    // the numbers do not depend on the data
    const c = await openPage(ctx, engraved, "index.html?ui2mode=templates");
    const templates = await nodeList(c.page);
    const stripInst = (l) => l.split("|").filter((_, i) => i !== 1).join("|");
    const uniq = (xs) => [...new Set(xs)];
    assert.deepEqual(uniq(first.map(stripInst)), templates.map(stripInst));
  } finally {
    await ctx.close();
  }
});

// --- the fixture skin ---

test("fixture: nested includes, loops, sort, filter, keys, toggles, styles, l10n, texts of the model", async () => {
  const ctx = await newContext();
  try {
    const { page, problems } = await openPage(ctx, fixture, "index.html");
    assert.equal(await text(page, "#inner"), "Hello");
    assert.equal(await text(page, "#deep"), "deep"); // include inside an include
    assert.equal(await page.evaluate(() => document.querySelectorAll("[data-include]").length), 0);

    // filter drops "skip", sort by name, descending off
    const lis = await page.evaluate(() => [...document.querySelectorAll("#list > li")].map((li) => ({
      inst: li.getAttribute("data-ui2-inst"), name: li.querySelector("b").textContent,
      tags: [...li.querySelectorAll("i")].map((i) => [i.getAttribute("data-ui2-inst"), i.textContent, i.classList.contains("hot")]),
    })));
    assert.deepEqual(lis.map((l) => [l.inst, l.name]), [["2", "apple"], ["1", "pear"]]);
    assert.deepEqual(lis[1].tags, [["1/a", "a", true], ["1/b", "b", false]]); // nested keys, class toggle

    assert.equal(await page.evaluate(() => !!document.querySelector("#maybe")), false, "if is false");
    assert.equal(await page.evaluate(() => [document.querySelector("#on").disabled, document.querySelector("#off").disabled]).then((x) => x.join()), "false,true");
    assert.equal(await page.evaluate(() => document.querySelector("#bar").style.width), "50%");
    assert.equal(await page.evaluate(() => document.querySelector("#ico").getAttribute("data-ui2-asset")), "icon:ATLAS:3");
    assert.equal(await text(page, "#cnt"), "3 things");
    assert.equal(await text(page, "#who"), "Ann has 12");
    assert.equal(await text(page, "#when"), "4 things"); // a model text {k, a}
    assert.equal(await page.evaluate(() => !!document.querySelector("#empty")), false, "an empty for has no instance in layout mode");

    // the numbering rule, spelled out (see the header of preview.js); the dev element is not a node
    const nums = await page.evaluate(() => Object.fromEntries(["#outer", "#inner", "#deep", "#list", "#list > li", "#list > li > b", "#list i", "#on", "#off", "#bar", "#ico", "#cnt", "#who", "#when", "#dev", "#dev span"]
      .map((s) => [s, document.querySelector(s).getAttribute("data-ui2-node")])));
    assert.deepEqual(nums, {
      "#outer": "0:1", "#inner": "0:2", "#deep": "0:3", "#list": "0:4", "#list > li": "1:0", "#list > li > b": "1:1", "#list i": "2:0",
      "#on": "0:5", "#off": "0:6", "#bar": "0:7", "#ico": "0:8", "#cnt": "0:9", "#who": "0:10", "#when": "0:11", "#dev": null, "#dev span": null,
    });

    assert.deepEqual(await page.evaluate(() => window.__ui2preview.errors), []);
    assert.deepEqual(problems, []);
  } finally {
    await ctx.close();
  }
});

test("fixture: modes", async () => {
  const ctx = await newContext();
  try {
    // extract: every `if` drawn, exactly one instance of every `for`, an empty record for an empty list
    const ex = (await openPage(ctx, fixture, "index.html?ui2mode=extract")).page;
    assert.equal(await text(ex, "#maybe"), "shown");
    assert.deepEqual(await texts(ex, "#list > li > b"), ["apple"]);
    assert.equal(await ex.evaluate(() => document.querySelector("#empty").getAttribute("data-ui2-inst")), "0");
    assert.equal(await text(ex, "#empty span"), "");
    assert.equal(await ex.evaluate(() => window.__ui2preview.mode), "extract");

    // templates: includes and numbers only, the mockup text stays, nothing is instantiated
    const tp = (await openPage(ctx, fixture, "index.html?ui2mode=templates")).page;
    assert.equal(await text(tp, "#inner"), "t");
    assert.deepEqual(await texts(tp, "#list > li > b"), ["x"]);
    assert.equal(await tp.evaluate(() => document.querySelectorAll("[data-ui2-inst]").length), 0);
    assert.equal(await tp.evaluate(() => !!document.querySelector("#maybe") && !!document.querySelector("#empty")), true);
    assert.equal(await text(tp, "#cnt"), "0 things");
    assert.deepEqual(await tp.evaluate(() => ["#maybe", "#empty", "#empty span"].map((q) => document.querySelector(q).getAttribute("data-ui2-node"))), ["3:0", "4:0", "4:1"]);

    // the language and the state through the URL
    const alt = (await openPage(ctx, fixture, "index.html?ui2state=alt&ui2lang=en_US")).page;
    assert.equal(await text(alt, "#inner"), "Other");
    assert.deepEqual(await texts(alt, "#list > li > b"), ["kiwi", "fig"]); // state.desc is true
    assert.equal(await text(alt, "#maybe"), "shown");
    assert.equal(await text(alt, "#when"), "plain");
    assert.equal(await alt.evaluate(() => document.querySelector("#on").disabled), true);
  } finally {
    await ctx.close();
  }

  // a global set before the script loads wins over the defaults
  const ctx2 = await newContext(() => { window.__UI2_PREVIEW = { mode: "extract", state: "alt" }; });
  try {
    const g = (await openPage(ctx2, fixture, "index.html")).page;
    assert.equal(await g.evaluate(() => [window.__ui2preview.mode, window.__ui2preview.state].join()), "extract,alt");
    assert.equal(await text(g, "#inner"), "Other");
  } finally {
    await ctx2.close();
  }
});

test("fixture: rerender with a state name, an object, and the same data", async () => {
  const ctx = await newContext();
  try {
    const { page } = await openPage(ctx, fixture, "index.html");
    assert.equal(await text(page, "#inner"), "Hello");
    await page.evaluate(() => window.__ui2preview.rerender("alt"));
    assert.equal(await text(page, "#inner"), "Other");
    assert.equal(await page.evaluate(() => window.__ui2preview.state), "alt");
    await page.evaluate(() => window.__ui2preview.rerender({ model: { title: "Direct", items: [{ id: 1, n: "x", tags: [] }] }, state: {} }));
    assert.equal(await text(page, "#inner"), "Direct");
    assert.deepEqual(await texts(page, "#list > li > b"), ["x"]);
    // the numbers survive a render, and a second render of the same data is the same page
    const before = await nodeList(page);
    await page.evaluate(() => window.__ui2preview.rerender());
    assert.equal((await nodeList(page)).join("\n"), before.join("\n"));
    assert.equal(await page.evaluate(() => window.__ui2preview.errors.length), 0);
  } finally {
    await ctx.close();
  }
});

test("fixture: problems become error entries and never throw", async () => {
  const ctx = await newContext();
  try {
    const { page } = await openPage(ctx, fixture, "errors.html");
    const errors = await page.evaluate(() => window.__ui2preview.errors);
    const byCode = (c) => errors.filter((e) => e.code === c);
    assert.equal(byCode("PV-002").length, 1, "include cycle");
    assert.match(byCode("PV-002")[0].message, /cycle-a\.html -> parts\/cycle-b\.html -> parts\/cycle-a\.html/);
    assert.match(byCode("PV-002")[0].src, /^parts\/cycle-b\.html:1:/, "the element that closes the cycle");
    assert.equal(byCode("PV-001").length, 1, "missing include");
    assert.match(byCode("PV-001")[0].src, /^errors\.html:\d+:\d+$/);
    assert.equal(byCode("E1-027").length, 1, "include path outside the skin");
    assert.equal(byCode("E4-001").length, 1, "bad expression");
    assert.match(byCode("E4-001")[0].src, /^errors\.html:\d+:\d+$/);
    assert.equal(byCode("E4-001")[0].attr, "data-bind-value");
    assert.equal(byCode("PV-005").length, 1, "missing l10n key");
    assert.equal(byCode("E4-009").length, 1, "for over a string");
    assert.equal(byCode("PV-007").length, 1, "duplicate keys");
    assert.equal(byCode("PV-008").length, 1, "division by zero");
    assert.equal(byCode("PV-008")[0].level, "warn");
    assert.equal(byCode("PV-006").length, 1, "a transform that is not previewed");
    // the page still rendered what it could
    assert.equal(await text(page, "#bad"), "kept");
    assert.equal(await text(page, "#nokey"), "no.such");
    assert.equal(await text(page, "#grp"), '{1: number "#,###"}');
    assert.equal(await page.evaluate(() => !!document.querySelector("#cycle-a") && !!document.querySelector("#cycle-b")), true);
  } finally {
    await ctx.close();
  }
});

test("a missing sample is one error entry; the page stays a static mockup", async () => {
  const ctx = await newContext();
  try {
    const { page } = await openPage(ctx, fixture, "index.html?ui2state=nope");
    const errors = await page.evaluate(() => window.__ui2preview.errors);
    assert.equal(errors.filter((e) => e.code === "PV-003").length, 1);
    assert.equal(await text(page, "#inner"), "");
    const bad = await openPage(ctx, fixture, "index.html?ui2mode=weird");
    assert.equal(await bad.page.evaluate(() => window.__ui2preview.errors.some((e) => e.code === "PV-004")), true);
    assert.equal(await text(bad.page, "#inner"), "Hello");
  } finally {
    await ctx.close();
  }
});

test("session open({preview: true}): the scan sees the page after includes and bindings", async () => {
  const s = createSession();
  try {
    await s.open({ skin_dir: FIXTURE, preview: true });
    const ir = await s.scan({ state: "hud" });
    const dump = JSON.stringify(ir.dom);
    assert.ok(dump.includes("Hello") && dump.includes("deep"), "bound text and included parts are in the IR");
    assert.ok(ir.resources.some((r) => r.path === "/__ui2/preview.js" && r.status === 200));
    assert.deepEqual(ir.errors, []);
  } finally {
    await s.close();
  }
});
