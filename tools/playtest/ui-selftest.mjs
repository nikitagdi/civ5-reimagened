// The page's own screens (apps/web/www/ui.js) without the game, in headless Chromium (P15 lobby UI):
//   node tools/playtest/ui-selftest.mjs [--www apps/web/www] [--shots <dir>]
//                                       [--assets <asset set dir> [--civ CIVILIZATION_SPAIN] [--traits "<text>"]]
// Serves the www dir, opens ui-selftest.html, hands ui.js made-up art and views through window.__civ5ui.render and
// checks the DOM: the game's text markup ([COLOR_*], [ICON_*], [NEWLINE], unknown tags dropped), the civilization
// tip with its pictures, the AI / Additional option groups drawn as headers, the saves grouped per game (newest
// first) and the online players panel. Exit code 0 only when every check passed. Needs Playwright (`npm ci` here).
// With --assets <dir> (a built asset set with the `web-lobby` blobs) the page gets the real `web:lobby` table and the
// real PNG sheets instead of the made-up art, hovers --civ (default CIVILIZATION_SPAIN) and saves the tip as
// <shots>/civtip2.png: the evidence for the unique items' descriptions; the checks of the made-up art are skipped.
import { createReadStream, existsSync, mkdirSync, readFileSync, statSync } from "node:fs";
import { createServer } from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

const here = path.dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const take = (flag, dflt) => {
  const i = args.indexOf(flag);
  return i >= 0 ? args.splice(i, 2)[1] : dflt;
};
const www = path.resolve(take("--www", path.join(here, "..", "..", "apps", "web", "www")));
const shots = take("--shots", null);
const assetsDir = take("--assets", null);
const realCiv = take("--civ", "CIVILIZATION_SPAIN");
const realTraits = take("--traits", "");

// The asset set of --assets: `manifest` names the hash of the manifest blob (C5MF, postcard: id, one variant of
// format, 32 byte hash, size, prefetch group); blobs are the files named by their hash.
function readManifest(dir) {
  const m = readFileSync(path.join(dir, readFileSync(path.join(dir, "manifest"), "utf8").trim()));
  let i = 6;
  const varint = () => {
    let n = 0;
    for (let sh = 0; ; sh += 7) {
      const c = m[i++];
      n += (c & 127) * 2 ** sh;
      if (c < 128) return n;
    }
  };
  const ids = new Map();
  for (let n = varint(); n > 0; n--) {
    const len = varint();
    const id = m.toString("utf8", i, i + len);
    i += len;
    for (let v = varint(); v > 0; v--) {
      varint();
      ids.set(id, m.subarray(i, i + 32).toString("hex"));
      i += 32;
      varint();
      varint();
    }
  }
  return ids;
}
const REAL = assetsDir ? readManifest(path.resolve(assetsDir)) : null;

// A 4x4 opaque PNG: the "sheet" every cell of the made-up art points at.
const PNG = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAIAAAAmkwkpAAAAEklEQVR4nGP4z8DAwMDAxMDAAAAPAAEBNZ4J1QAAAABJRU5ErkJggg==",
  "base64",
);
const types = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css" };
const server = createServer((req, res) => {
  const p = new URL(req.url, "http://x").pathname;
  if (p.startsWith("/assets/")) {
    // As the game server: blobs go out as octet-stream; the page must still show them.
    const hash = p.slice("/assets/".length);
    const blob = REAL && /^[0-9a-f]{64}$/.test(hash) ? path.join(path.resolve(assetsDir), hash) : null;
    res.writeHead(200, { "content-type": "application/octet-stream" }).end(blob && existsSync(blob) ? readFileSync(blob) : PNG);
    return;
  }
  const file = path.join(www, path.normalize(decodeURIComponent(p)).replace(/^([/\\]|\.\.[/\\])+/, ""));
  if (!file.startsWith(www) || !existsSync(file) || !statSync(file).isFile()) {
    res.writeHead(404).end();
    return;
  }
  res.writeHead(200, { "content-type": types[path.extname(file)] ?? "application/octet-stream" });
  createReadStream(file).pipe(res);
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const base = `http://127.0.0.1:${server.address().port}`;

const cell = (img, w) => ({ img, x: 0, y: 0, w, h: w });
const ART = {
  urls: { "web:png:fonticons-0": base + "/assets/f", "web:png:civ-civilization_rome": base + "/assets/r" },
  art: {
    version: 1,
    colors: { COLOR_POSITIVE_TEXT: [10, 200, 250, 255], COLOR_YELLOW: [255, 255, 0, 255] },
    icons: {
      ICON_GOLD: cell("web:png:fonticons-0", 22),
      ICON_PROMOTION_SENTRY: cell("web:png:fonticons-0", 22),
      ICON_MOVES: cell("web:png:fonticons-0", 22),
      ICON_STRENGTH: cell("web:png:fonticons-0", 22),
      ICON_PRODUCTION: cell("web:png:fonticons-0", 22),
    },
    civs: {
      CIVILIZATION_ROME: {
        name: "Rome",
        leader: { type: "LEADER_AUGUSTUS", name: "Augustus Caesar", cell: cell("web:png:civ-civilization_rome", 128) },
        icon: cell("web:png:civ-civilization_rome", 64),
        uniques: [
          {
            kind: "Unit",
            type: "UNIT_ROMAN_LEGION",
            name: "Legion",
            cell: cell("web:png:civ-civilization_rome", 64),
            replaces: "Replaces: Swordsman",
            stats: ["Movement: 2[ICON_MOVES]", "Combat: 16[ICON_STRENGTH]", "Cost: 75[ICON_PRODUCTION]"],
            help: "Strong, front-line [COLOR_POSITIVE_TEXT]land[ENDCOLOR] Unit.[NEWLINE]Only the Romans may build it. [ICON_GOLD]",
          },
          { kind: "Building", type: "BUILDING_COLOSSEUM", name: "Colosseum", cell: null },
        ],
      },
    },
  },
};

const choice = (key, name) => ({ key, name });
const ROOM = {
  host: "alice",
  me_host: true,
  starting: false,
  settings: {
    name: "alice's game",
    world_size: "WORLDSIZE_DUEL",
    map_script: "a.lua",
    speed: "GAMESPEED_STANDARD",
    era: "ERA_ANCIENT",
    handicap: "HANDICAP_PRINCE",
    minor_civs: 0,
    max_minor: 4,
    size_info: "40 x 24",
    turn_mode: "Hybrid",
    timer: "Off",
    options: [
      { key: "GAMEOPTION_GOLDENAGE_ALT", name: "Alternate Golden Age", value: 0, values: [], help: "Gain [ICON_GOLD] Gold[NEWLINE]Lose [COLOR_POSITIVE_TEXT]nothing[ENDCOLOR] [STYLE_WEIRD]", group: "General" },
      { key: "GAMEOPTION_AI_OPTIONS", name: "[COLOR_YELLOW]AI Options[ENDCOLOR]", value: 1, values: [], help: "[COLOR_YELLOW]AI Options[ENDCOLOR]", group: "General" },
      { key: "GAMEOPTION_AI_GIMP_NO_DEALS", name: "AI makes no deals", value: 0, values: [], help: "", group: "Ai" },
      { key: "GAMEOPTION_ADDITIONAL", name: "[COLOR_YELLOW]Additional Options[ENDCOLOR]", value: 0, values: [], help: "[COLOR_YELLOW]Additional Options[ENDCOLOR]", group: "General" },
      { key: "GAMEOPTION_X", name: "Hidden x", value: 0, values: [], help: "", group: "Additional" },
    ],
    victories: [{ key: "VICTORY_TIME", name: "Time", on: true }],
    max_turns: 0,
    map_options: [],
    auto_choices: [],
    seed: null,
    map_seed: null,
    sizes: [choice("WORLDSIZE_DUEL", "Duel")],
    scripts: [choice("a.lua", "Pangaea")],
    speeds: [choice("GAMESPEED_STANDARD", "Standard")],
    eras: [choice("ERA_ANCIENT", "Ancient")],
    handicaps: [choice("HANDICAP_PRINCE", "Prince")],
    timers: [],
  },
  slots: [
    { index: 0, who: { Human: { nick: "alice", ready: false, me: true } }, civ: "CIVILIZATION_ROME", was: null, yours: false },
    { index: 1, who: "Ai", civ: null, was: null, yours: false },
  ],
  civs: [choice("CIVILIZATION_ROME", "Rome"), choice("CIVILIZATION_EGYPT", "Egypt")],
  civ_tips: [
    {
      key: "CIVILIZATION_ROME",
      leader: "Augustus Caesar",
      traits: ["Glory of Rome: All [COLOR_POSITIVE_TEXT]Military Land[ENDCOLOR] units have +1 [ICON_PROMOTION_SENTRY] sight.[NEWLINE]Cost of tiles with [ICON_GOLD] Gold [BOGUS_TAG]is reduced."],
      unique: ["Unit: Legion", "Building: Colosseum"],
    },
  ],
  has_slot: true,
  can_start: false,
  why_not: "Need at least two civilizations",
  from_save: null,
  note: null,
  error: null,
  saves: [],
};
const LOBBY = {
  rooms: [{ id: 1, name: "bob's game", host: "bob", players: "1/4", map: "Duel, Pangaea", starting: false }],
  games: [],
  saves: [
    { id: "7-00000003", name: "Second", turn: 12, saved: "2026-11-02 14:00 UTC", civs: "Rome (alice)", game: 7, host: "alice" },
    { id: "5-00000009", name: "First", turn: 40, saved: "2026-11-02 13:00 UTC", civs: "Egypt (bob)", game: 5, host: "bob" },
    { id: "7-00000002", name: "Second", turn: 11, saved: "2026-11-02 13:50 UTC", civs: "Rome (alice)", game: 7, host: "alice" },
  ],
  error: null,
  note: null,
  nick: "alice",
  conn: { dot: "Green", status: null },
  my_seat: null,
  confirm_give_up: false,
  online: [
    { nick: "alice", me: true, place: "lobby", status: "In the lobby", ready: null, host: false },
    { nick: "bob", me: false, place: "room", status: "In room bob's game", ready: true, host: true },
  ],
};

if (REAL) {
  // The real table and sheets: every sheet of the manifest is served under its own hash.
  ART.art = JSON.parse(readFileSync(path.join(path.resolve(assetsDir), REAL.get("web:lobby"))));
  ART.urls = Object.fromEntries([...REAL.entries()].filter(([id]) => id.startsWith("web:png:")).map(([id, hash]) => [id, `${base}/assets/${hash}`]));
  const c = ART.art.civs[realCiv];
  if (!c) throw new Error(`${realCiv} is not in web:lobby`);
  ROOM.civs = Object.entries(ART.art.civs).map(([k, v]) => choice(k, v.name));
  ROOM.slots[0].civ = realCiv;
  ROOM.civ_tips = [{ key: realCiv, leader: c.leader ? c.leader.name : "", traits: realTraits ? [realTraits] : [], unique: [] }];
}

let failed = 0;
const check = (what, ok, detail) => {
  console.log(`${ok ? "ok  " : "FAIL"} ${what}${ok ? "" : ` (${JSON.stringify(detail)})`}`);
  if (!ok) failed++;
};

const browser = await chromium.launch({ executablePath: process.env.CHROMIUM_PATH || undefined, args: ["--no-sandbox"] });
const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
page.on("pageerror", (e) => {
  console.log(`[pageerror] ${e.message}`);
  failed++;
});
await page.goto(base + "/ui-selftest.html");
const render = (screen, v) => page.evaluate(([s, j]) => window.__civ5ui.render(s, j), [screen, JSON.stringify(v)]);
const frame = () => page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));

await render("art", ART);
await render("room", ROOM);
await frame();
await page.waitForTimeout(300); // the sheets arrive, a redraw follows
await frame();

if (REAL) {
  await page.click('[data-k="slot0"] .civpick-btn');
  await frame();
  const opt = page.locator(`.civpick-opt[data-civtip="${realCiv}"]`);
  await opt.scrollIntoViewIfNeeded();
  await opt.hover();
  await frame();
  await page.waitForTimeout(300);
  const real = await page.evaluate(() => {
    const t = document.querySelector(".civtip-float");
    const q = (sel) => (t ? [...t.querySelectorAll(sel)].map((x) => x.textContent.replace(/\s+/g, " ").trim()) : []);
    return { shown: !!t && !t.hidden, names: q(".u-name"), replaces: q(".u-replaces"), stats: q(".u-stat"), help: q(".u-help"), raw: t ? /\[[A-Z_]+\]/.test(t.textContent) : true, icons: t ? t.querySelectorAll(".u-help .gicon, .u-stat .gicon").length : 0 };
  });
  console.log(JSON.stringify(real, null, 1));
  check("the real tip shows the unique items with their descriptions", real.shown && real.names.length > 0 && real.help.length > 0 && real.help.every((x) => x.length > 0), real);
  check("no raw markup tag in the real tip", !real.raw, real);
  if (shots) {
    mkdirSync(shots, { recursive: true });
    await page.screenshot({ path: path.join(shots, "civtip2.png") });
  }
  await browser.close();
  server.close();
  console.log(failed ? `RESULT: FAIL (${failed})` : "RESULT: PASS");
  process.exit(failed ? 1 : 0);
}

const room = await page.evaluate(() => {
  const m = document.getElementById("civ5-main");
  const golden = [...m.querySelectorAll(".check")].find((c) => c.textContent.includes("Alternate Golden Age"));
  const gc = golden ? golden.querySelector(".gc") : null;
  const heads = [...m.querySelectorAll(".opt-head-title")];
  return {
    raw: /\[[A-Z_]+\]/.test(m.textContent),
    goldenIcons: golden ? golden.querySelectorAll(".gicon").length : -1,
    goldenBr: golden ? golden.querySelectorAll("br").length : -1,
    goldenColor: gc ? gc.style.color : null,
    goldenText: golden ? golden.textContent.replace(/\s+/g, " ").trim() : null,
    heads: heads.map((x) => x.textContent),
    headColor: heads[0] && heads[0].querySelector(".gc") ? heads[0].querySelector(".gc").style.color : null,
    aiRows: m.querySelectorAll('[data-k="grpGAMEOPTION_AI_OPTIONS"] .check').length,
    addRows: m.querySelectorAll('[data-k="grpGAMEOPTION_ADDITIONAL"] .check').length,
    plainChecks: [...m.querySelectorAll(".check .name")].map((x) => x.textContent).filter((t) => /AI Options|Additional Options/.test(t)),
    slotIcon: !!m.querySelector('[data-k="slot0"] .civpick-btn .spr .cell'),
  };
});
check("no raw markup tag left in the room", !room.raw, room);
check("[ICON_GOLD] is a glyph of the icon font", room.goldenIcons === 1, room.goldenIcons);
check("[NEWLINE] breaks the line", room.goldenBr === 1, room.goldenBr);
check("[COLOR_POSITIVE_TEXT] takes the colour of the Colors table", room.goldenColor === "rgba(10, 200, 250, 1)" || room.goldenColor === "rgb(10, 200, 250)", room.goldenColor);
check("unknown tags are dropped", room.goldenText && !room.goldenText.includes("STYLE"), room.goldenText);
check("AI / Additional options are group headers", room.heads.join("|") === "AI Options|Additional Options" && room.plainChecks.length === 0, room);
check("the header keeps its game colour", room.headColor === "rgb(255, 255, 0)" || room.headColor === "rgba(255, 255, 0, 1)", room.headColor);
check("the AI group shows its rows while on, the Additional group hides them while off", room.aiRows === 1 && room.addRows === 0, room);
check("the slot's civ button shows the civ icon", room.slotIcon, room);

// The tip: open the picker of slot 1, hover Rome.
await page.click('[data-k="slot0"] .civpick-btn');
await frame();
await page.hover('.civpick-opt[data-civtip="CIVILIZATION_ROME"]');
await frame();
const tip = await page.evaluate(() => {
  const t = document.querySelector(".civtip-float");
  const cellEl = t && t.querySelector(".portrait .cell");
  return {
    shown: !!t && !t.hidden,
    civ: t && t.querySelector(".tip-civ") ? t.querySelector(".tip-civ").textContent : null,
    leader: t && t.querySelector(".tip-leader") ? t.querySelector(".tip-leader").textContent : null,
    traitName: t && t.querySelector(".trait-name") ? t.querySelector(".trait-name").textContent : null,
    traitIcons: t ? t.querySelectorAll(".trait-desc .gicon").length : -1,
    raw: t ? /\[[A-Z_]+\]/.test(t.textContent) : true,
    portrait: cellEl ? cellEl.style.background : null,
    uniques: t ? [...t.querySelectorAll(".u-name")].map((x) => x.textContent) : [],
    uniqueIcons: t ? t.querySelectorAll(".unique .spr.uicon").length : -1,
    replaces: t ? [...t.querySelectorAll(".u-replaces")].map((x) => x.textContent) : [],
    stats: t ? [...t.querySelectorAll(".u-stat")].map((x) => x.textContent.replace(/\s+/g, " ").trim()) : [],
    statIcons: t ? t.querySelectorAll(".u-stat .gicon").length : -1,
    help: t ? [...t.querySelectorAll(".u-help")].map((x) => x.textContent.replace(/\s+/g, " ").trim()) : [],
    helpBr: t ? t.querySelectorAll(".u-help br").length : -1,
    helpIcons: t ? t.querySelectorAll(".u-help .gicon").length : -1,
    helpColor: t && t.querySelector(".u-help .gc") ? t.querySelector(".u-help .gc").style.color : null,
  };
});
check("hovering a civilization shows its tip", tip.shown, tip);
check("the tip names the civ and the leader", tip.civ === "Rome" && tip.leader === "Augustus Caesar", tip);
check("the trait is split into a name and a description with icons", tip.traitName === "Glory of Rome" && tip.traitIcons === 2 && !tip.raw, tip);
check("the leader portrait is a cell of the fetched sheet", !!tip.portrait && tip.portrait.includes("blob:"), tip.portrait);
check("the unique items come from the art with their icons", tip.uniques.join("|") === "Legion|Colosseum" && tip.uniqueIcons === 1, tip);
check("each unique says what it replaces", tip.replaces.join("|") === "Replaces: Swordsman", tip.replaces);
check("the unique's numbers are listed, with their glyphs", tip.stats.join("|") === "Movement: 2|Combat: 16|Cost: 75" && tip.statIcons === 3, tip);
check("the help text is under the name, markup rendered", tip.help.length === 1 && tip.help[0].startsWith("Strong, front-line land Unit.") && tip.helpBr === 1 && !tip.raw, tip);
check("the help text has its inline glyph", tip.helpIcons === 1, tip.helpIcons);
check("the help text keeps its colour", tip.helpColor === "rgb(10, 200, 250)" || tip.helpColor === "rgba(10, 200, 250, 1)", tip.helpColor);
if (shots) {
  mkdirSync(shots, { recursive: true });
  await page.screenshot({ path: path.join(shots, "selftest-room-tip.png") });
}
await page.mouse.move(5, 5);
await frame();
check("the tip hides when the mouse leaves", await page.evaluate(() => document.querySelector(".civtip-float").hidden));

// The lobby: saves per game, newest first; the online panel.
await render("lobby", LOBBY);
await page.click('[data-k="tab-saves"]');
await frame();
const lobby = await page.evaluate(() => {
  const m = document.getElementById("civ5-main");
  const groups = [...m.querySelectorAll(".save-group")];
  return {
    groups: groups.map((g) => g.querySelector(".title").textContent),
    rows: groups.map((g) => g.querySelectorAll(".save-rows li").length),
    firstOpen: groups[0] ? groups[0].open : null,
    secondOpen: groups[1] ? groups[1].open : null,
    host: groups[0] ? groups[0].querySelector(".facts").textContent : "",
    online: [...m.querySelectorAll(".online-list li .o-nick")].map((x) => x.textContent),
    ready: [...m.querySelectorAll(".online-list .ready-mark")].map((x) => x.textContent),
  };
});
check("the saves are grouped per game, newest game first", lobby.groups.join("|") === "Second|First" && lobby.rows.join(",") === "2,1", lobby);
check("the newest game is open, the others folded", lobby.firstOpen === true && lobby.secondOpen === false, lobby);
check("the group header has the host and the newest turn", lobby.host.includes("alice") && lobby.host.includes("Turn 12"), lobby.host);
check("the online panel lists the players with their place", lobby.online.join("|") === "aliceyou|bobhost" && lobby.ready.join() === "Ready", lobby);
if (shots) await page.screenshot({ path: path.join(shots, "selftest-lobby-saves.png") });

await browser.close();
server.close();
console.log(failed ? `RESULT: FAIL (${failed})` : "RESULT: PASS");
process.exit(failed ? 1 : 0);
