// `ui2-maplink`: the link of UI v2 and the map, live against a real-core server:
//   node tools/playtest/run.mjs --scenario ui2-maplink --attach http://127.0.0.1:8870 --out <dir>
// Env: UI2_SHOT (path prefix for screenshots).
//
// 1. Movement preview: a selected unit has the selection ring on its plot and its reach outlined; the mouse over a
//    plot shows the path with a turn badge where each turn ends and at the target (the skin's `map.path-turn`
//    anchored on the plot); a plot outside this turn's reach gets its path from the server; the mouse over a panel
//    shows none; a right click moves the unit; deselecting clears everything.
// 2. The plot tooltip after the mouse rests on a plot; it goes at once when the mouse moves on; fog hides the plot.
// 3. The map options (yields, resources, grid) and their keeping across a reload.
// 4. Notification clicks: left does the core's activation for the type, right dismisses what may be dismissed.
import { openPage, act, until, appUrl, state } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { FOUND_CITY } from "../lib/policy.mjs";
import { probe, waitFor, intent } from "./ui2-screen-top_panel.mjs";
import { partBox, clickPart } from "./ui2-v1.mjs";

const VIEW = { width: 1600, height: 900 };

/** The rectangles of a part, CSS px. */
async function rects(page, part) {
  const r = await page.evaluate(p => window.__civ5test.ui2Rects([p]), part).catch(() => null);
  return Array.isArray(r) ? r : [];
}

/** The flag buttons of the map (40 x 48 CSS px each), with their list keys. */
async function flags(page) {
  // The flag button: the flag on its pole, 32 x 86.
  return (await rects(page, "unit-flag")).filter(r => Math.abs(r.w - 32) <= 2 && Math.abs(r.h - 86) <= 2
    && r.x > 0 && r.y > 60 && r.x + r.w < VIEW.width && r.y + r.h < VIEW.height - 200);
}

const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);

/** The rectangles of a part, one per place on the screen (a part may be reported more than once). */
async function distinct(page, part) {
  const seen = new Set();
  return (await rects(page, part)).filter(r => {
    const k = `${Math.round(r.x)},${Math.round(r.y)},${Math.round(r.w)},${Math.round(r.h)}`;
    return !seen.has(k) && seen.add(k);
  });
}

export async function run({ server, opts }) {
  if (!opts.attach) throw new Error("ui2-maplink requires --attach to a real-core server");
  const shotPrefix = process.env.UI2_SHOT;
  const browser = await openPage({ headed: opts.headed, logDir: opts.logDir, viewport: VIEW, consoleFile: "ui2-maplink.log" });
  const result = { scenario: "ui2-maplink", pass: false, checks: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const shot = async (page, label, clip) => {
    if (shotPrefix) await page.screenshot({ path: `${shotPrefix}-${label}.png`, ...(clip ? { clip } : {}) });
  };
  const around = (b, r = 220) => ({
    x: Math.max(0, b.cx - r), y: Math.max(0, b.cy - r * 0.7),
    width: Math.min(2 * r, VIEW.width - Math.max(0, b.cx - r)), height: Math.min(1.4 * r, VIEW.height - Math.max(0, b.cy - r * 0.7)),
  });
  try {
    const page = browser.page;
    const nick = `maplink${Date.now() % 1000000}`;
    await page.goto(appUrl(server.url, opts, { ui: "v2:engraved", nick, autostart: "1", ai: "2", size: "small", seed: "7", mode: "simultaneous", manual: "1" }));
    const first = await until(page, s => s.screen === "login" || s.screen === "play", 90000, "login or game");
    if (first.screen === "login") await act(page, { kind: "login", nick, password: DEV_PASSWORD });
    await until(page, s => s.screen === "play" && s.turn !== null, 90000, "running game");
    await waitFor(page, p => p?.state === "running" && p.frames > 10 && (p.counters["ui2.anchors.shown"] ?? 0) > 0, "UI v2 with flags on the map");
    // A city first (the settler founds it where it stands): its notes are the ones step 4 clicks, and the unit
    // left for step 1 is the one that moves.
    {
      let s = await state(page);
      if (!s.cities?.length) {
        const settler = s.units.find(u => /settler/i.test(u.name));
        if (!settler) throw new Error("a settler is required");
        await act(page, { kind: "selectUnit", id: settler.id });
        await act(page, { kind: "unitAction", action: FOUND_CITY });
        s = await until(page, v => v.cities?.length > 0, 30000, "a founded city");
      }
      await page.waitForTimeout(1000);
      await intent(page, "window.close", { window: "city" });
      await intent(page, "unit.deselect", {});
    }
    await page.waitForTimeout(1500);

    // The unit: click its flag.
    const flag = (await flags(page))[0];
    if (!flag) throw new Error("a unit flag on the screen is required");
    await page.mouse.click(flag.x + flag.w / 2, flag.y + flag.h / 2);
    const sel = await waitFor(page, p => p.ui.selection?.unit != null, "a selected unit");
    const [owner, id] = sel.ui.selection.unit;
    const unit = (await state(page)).units.find(u => u.id === id);
    result.unit = { owner, id, at: unit && [unit.x, unit.y], name: unit?.name };
    // The tree comes with the focus the UI sends.
    const reach = await waitFor(page, p => p.preview?.selected && p.preview.reachable.length > 0, "the reach of the selected unit");
    check("1. the ring is on the selected unit's plot", unit && same(reach.preview.selected, [unit.x, unit.y]), JSON.stringify(reach.preview.selected));
    check("1. the plots it reaches this turn are outlined (move range styles)", reach.preview.reachable.length > 0 && reach.preview.styles >= reach.preview.reachable.length, `${reach.preview.reachable.length} plots, ${reach.preview.styles} styled cells`);
    await page.mouse.move(VIEW.width / 2, 40);
    await page.waitForTimeout(300);
    const box = { cx: flag.x + flag.w / 2, cy: flag.y + flag.h / 2 + 40 };
    await shot(page, "reach", around(box));

    // The path under the mouse: try plots around the unit until one shows a path with badges.
    const offsets = [[110, 40], [-110, 40], [0, 120], [110, 120], [-110, 120], [200, 60], [-200, 60], [60, -40], [-60, -40]];
    let near = null;
    for (const [dx, dy] of offsets) {
      await page.mouse.move(box.cx + dx, box.cy + dy, { steps: 4 });
      await page.waitForTimeout(450);
      const p = await probe(page);
      if (p.preview?.hover && p.preview.path.length >= 2 && p.preview.badges.length > 0) {
        near = { at: [box.cx + dx, box.cy + dy], preview: p.preview, ui: p.ui.selection.path };
        break;
      }
    }
    result.near = near;
    check("1. the mouse over a plot shows the path to it", !!near, JSON.stringify(near?.preview ?? null));
    if (near) {
      const last = near.preview.path[near.preview.path.length - 1];
      check("1. the path ends at the hovered plot", same([last[0], last[1]], near.preview.hover), JSON.stringify({ last, hover: near.preview.hover }));
      const badge = near.preview.badges[near.preview.badges.length - 1];
      check("1. the target has a turn badge with the path's last turn", same(badge, last), JSON.stringify({ badge, last }));
      check("1. the badges reach the skin (ui.path)", same(near.ui, near.preview.badges), JSON.stringify(near.ui));
      const turn = await partBox(page, "map.path-turn");
      result.badgeBox = turn;
      check("1. the skin draws the turn badge on the map", turn && turn.w > 10 && turn.h > 10, JSON.stringify(turn));
      await shot(page, "path", around({ cx: near.at[0], cy: near.at[1] }));
      await shot(page, "path-full");
    }

    // A plot beyond this turn's reach: the path comes from the server (Query::Path) after the dwell.
    let far = null;
    for (const [dx, dy] of [[420, 120], [-420, 120], [420, -80], [-420, -80], [300, 220], [-300, 220]]) {
      const x = box.cx + dx, y = box.cy + dy;
      if (x < 40 || x > VIEW.width - 40 || y < 80 || y > VIEW.height - 260) continue;
      await page.mouse.move(x, y, { steps: 6 });
      await page.waitForTimeout(900);
      const p = await probe(page);
      if (p.preview?.path.length >= 2 && p.preview.path.some(s => s[2] >= 2)) {
        far = { at: [x, y], preview: p.preview };
        break;
      }
    }
    result.far = far;
    check("1. a plot of a later turn shows its path with a badge per turn", far && far.preview.badges.length >= 2, JSON.stringify(far?.preview ?? null));
    if (far) await shot(page, "path-far", around({ cx: far.at[0], cy: far.at[1] }, 320));

    // 2. The plot tooltip: after the mouse rested on a plot, the server's help for that plot hangs on it.
    if (near) {
      await page.mouse.move(near.at[0], near.at[1], { steps: 4 });
      const tip = await waitFor(page, p => p.preview?.rested && (p.texts ?? []).some(t => t.part === "plot-tip.nature"), "the plot tooltip").catch(() => null);
      const tipBox = await partBox(page, "map.plot-tip");
      result.tip = { rested: tip?.preview?.rested, texts: (tip?.texts ?? []).filter(t => t.part?.startsWith("plot-tip")).map(t => `${t.part}: ${t.text}`), box: tipBox };
      check("2. resting on a plot shows its tooltip (nature, yields) on the plot", tip && tipBox && same(tip.preview.rested, near.preview.hover), JSON.stringify(result.tip));
      check("2. the tooltip names the terrain of the plot", (result.tip.texts ?? []).some(t => t.startsWith("plot-tip.nature: ") && t.length > 18), JSON.stringify(result.tip.texts));
      await shot(page, "tip", around({ cx: near.at[0] + 120, cy: near.at[1] + 60 }, 260));
      // Moving on: the tooltip goes at once and comes back only after the delay on the new plot.
      let moved = null;
      for (const [dx, dy] of [[140, 60], [-140, 60], [140, -60], [-160, 0]]) {
        await page.mouse.move(near.at[0] + dx, near.at[1] + dy, { steps: 2 });
        await page.waitForTimeout(120);
        const p = await probe(page);
        if (p.preview?.hover && !same(p.preview.hover, near.preview.hover)) { moved = p; break; }
      }
      check("2. moving on to another plot hides the tooltip at once", moved && !moved.preview.rested && !await partBox(page, "map.plot-tip"), JSON.stringify(moved?.preview ?? null));
    }
    // A plot under the fog of war: the tooltip says it is unexplored and shows nothing of it.
    if (far) {
      for (const [dx, dy] of [[600, -200], [-600, -200], [650, 150], [-650, 150], [700, -300]]) {
        const x = box.cx + dx, y = box.cy + dy;
        if (x < 40 || x > VIEW.width - 40 || y < 80 || y > VIEW.height - 260) continue;
        await page.mouse.move(x, y, { steps: 4 });
        await page.waitForTimeout(1300);
        const p = await probe(page);
        const parts = (p.texts ?? []).filter(t => t.part?.startsWith("plot-tip") || t.text === "Unexplored territory");
        if (p.preview?.rested && (p.texts ?? []).some(t => t.text === "Unexplored territory")) {
          result.fog = { at: p.preview.rested, parts: parts.map(t => `${t.part}: ${t.text}`) };
          await shot(page, "tip-fog", around({ cx: x + 120, cy: y + 60 }, 260));
          break;
        }
      }
      check("2. an unexplored plot's tooltip shows nothing of it", result.fog && !result.fog.parts.some(t => /plot-tip\.(nature|resource|improvement|owner|unit)/.test(t)), JSON.stringify(result.fog ?? null));
    }

    // Over a panel: no path.
    const panel = await partBox(page, "unit.title") ?? await partBox(page, "helm.state");
    if (panel) {
      await page.mouse.move(panel.cx, panel.cy, { steps: 4 });
      await page.waitForTimeout(400);
      const p = await probe(page);
      check("1. the mouse over a panel shows no path", p.preview && p.preview.hover == null && p.preview.badges.length === 0 && p.ui.selection.path.length === 0, JSON.stringify(p.preview));
    }

    // A right click on the near plot moves the unit there.
    if (near) {
      const target = near.preview.hover;
      await page.mouse.move(near.at[0], near.at[1], { steps: 4 });
      await page.waitForTimeout(300);
      await page.mouse.click(near.at[0], near.at[1], { button: "right" });
      const moved = await until(page, s => {
        const u = s.units.find(v => v.id === id);
        return u && (u.x !== unit.x || u.y !== unit.y);
      }, 15000, "the unit moved").catch(() => null);
      const u = moved?.units.find(v => v.id === id);
      result.moved = u && [u.x, u.y];
      check("1. a right click moves the unit along the path", u && (same([u.x, u.y], target) || near.preview.path.some(s => s[0] === u.x && s[1] === u.y)), JSON.stringify({ to: result.moved, target }));
      await page.waitForTimeout(800);
      await shot(page, "moved", around({ cx: near.at[0], cy: near.at[1] }));
    }

    // Deselecting clears the ring, the reach and the badges.
    await intent(page, "unit.deselect", {});
    const gone = await waitFor(page, p => p.ui.selection.unit == null && p.preview && p.preview.selected == null, "the selection gone");
    check("1. deselecting clears the ring, the reach and the badges", gone.preview.reachable.length === 0 && gone.preview.badges.length === 0 && gone.ui.selection.path.length === 0, JSON.stringify(gone.preview));

    // 3. The map options of the minimap panel: yield and resource icons on the map, the grid, kept for the player.
    await page.mouse.move(VIEW.width / 2, VIEW.height / 2 - 100);
    await page.waitForTimeout(300);
    const anchorsBefore = (await probe(page)).counters["ui2.anchors"] ?? 0;
    if (await clickPart(page, "minimap.options-toggle")) {
      await waitFor(page, p => p.ui.state["minimap.options"] === true, "the map options open");
      for (const part of ["minimap.option.yields", "minimap.option.resources", "minimap.option.grid"]) {
        check(`3. the panel has ${part}`, await clickPart(page, part));
        await page.waitForTimeout(200);
      }
      const on = await waitFor(page, p => p.ui.state["minimap.yields"] && p.ui.state["minimap.resources"] && p.ui.state["minimap.grid"], "the switches on").catch(() => null);
      check("3. the yield, resource and grid switches turn on", !!on, JSON.stringify(on?.ui.state ?? null));
      await clickPart(page, "minimap.options-close");
      await page.mouse.move(VIEW.width / 2, 40);
      const icons = await waitFor(page, p => (p.counters["ui2.anchors"] ?? 0) > anchorsBefore + 5, "the yield icons on the map").catch(() => null);
      result.icons = { before: anchorsBefore, after: icons?.counters["ui2.anchors"], shown: icons?.counters["ui2.anchors.shown"] };
      check("3. yield icons hang on the plots in view", !!icons && !!await partBox(page, "map.yield-icons"), JSON.stringify(result.icons));
      result.resourceIcons = !!await partBox(page, "map.resource-icon");
      await page.waitForTimeout(600);
      await shot(page, "options-on");
      // Kept for the player: a reload of the page brings them back.
      await page.reload();
      const again = await until(page, s => s.screen === "login" || s.screen === "play", 90000, "login or game after reload");
      if (again.screen === "login") await act(page, { kind: "login", nick, password: DEV_PASSWORD });
      await until(page, s => s.screen === "play" && s.turn !== null, 90000, "running game after reload");
      const back = await waitFor(page, p => p?.state === "running" && p.frames > 10 && p.ui.state["minimap.yields"] !== undefined, "UI v2 after reload");
      await page.waitForTimeout(1000);
      const restored = await probe(page);
      check("3. the map options are kept for the player across a reload", restored.ui.state["minimap.yields"] === true && restored.ui.state["minimap.grid"] === true && restored.ui.state["minimap.resources"] === true, JSON.stringify(restored.ui.state));
      await shot(page, "options-reloaded");
      // Off again: the icons go.
      await clickPart(page, "minimap.options-toggle");
      await waitFor(page, p => p.ui.state["minimap.options"] === true, "the map options open again");
      await clickPart(page, "minimap.option.yields");
      await clickPart(page, "minimap.option.resources");
      await clickPart(page, "minimap.option.grid");
      const off = await waitFor(page, p => !p.ui.state["minimap.yields"] && !p.ui.state["minimap.resources"], "the switches off").catch(() => null);
      await page.waitForTimeout(500);
      check("3. switching the icons off takes them off the map", !!off && !await partBox(page, "map.yield-icons"), JSON.stringify(off?.counters?.["ui2.anchors"] ?? null));
      void back;
    } else {
      check("3. the minimap has a map options button", false);
    }

    // 4. Notifications: a left click does what the core's activation does for the type (the research note opens the
    //    tech window as the chooser, the production note the production window of its city); a right click
    //    dismisses only what the core lets the player dismiss.
    {
      const withNotes = await waitFor(page, p => (p.notes?.list ?? []).some(n => n.go === "choose_tech") && (p.notes?.list ?? []).some(n => n.go === "production"), "the research and production notes").catch(() => null);
      result.notes = withNotes?.notes?.list?.map(n => `${n.go}${n.dismissable ? "" : " (kept)"}: ${n.summary}`);
      check("4. the notes carry their click action and dismissability", !!withNotes, JSON.stringify(result.notes ?? null));
      /** The card of the first note with this action: its rectangle, by the order of the cards' summaries. */
      const card = async go => {
        const p = await probe(page);
        const note = (p.notes?.list ?? []).find(n => n.go === go);
        if (!note) return null;
        // The card that names this note when the mouse is over it (a card writes its summary to the skin state).
        // The rectangles name one card of the list only: walk down the list instead.
        const first = (await distinct(page, "notes.card")).sort((a, b) => a.y - b.y)[0];
        if (!first) return null;
        const cx = first.x + first.w / 2;
        for (let cy = first.y + 8; cy < Math.min(first.y + 480, VIEW.height - 40); cy += 16) {
          await page.mouse.move(cx, cy, { steps: 2 });
          await page.waitForTimeout(200);
          if ((await probe(page)).ui.state["notification.hover"] === note.summary) return { note, cx, cy: cy + 8 };
        }
        return null;
      };
      if (withNotes) {
        const kept = withNotes.notes.list.filter(n => !n.dismissable).length;
        const shown = withNotes.notes.list.length;
        const xs = (await distinct(page, "notes.dismiss")).length;
        check("4. only the notes the player may dismiss have a dismiss button", xs === shown - kept, JSON.stringify({ shown, kept, buttons: xs }));
        const tech = await card("choose_tech");
        check("4. the research note has a card", !!tech, JSON.stringify(tech?.note ?? null));
        if (tech) {
          await page.mouse.click(tech.cx, tech.cy);
          const open = await waitFor(page, p => p.notes?.windows?.techs != null, "the tech window").catch(() => null);
          result.techWindow = open?.notes?.windows?.techs;
          check("4. a left click on the research note opens the tech window as the chooser", open && same(open.notes.windows.techs, [1, null]), JSON.stringify(result.techWindow ?? null));
          await page.waitForTimeout(800);
          await shot(page, "note-tech");
          await intent(page, "window.close", { window: "techs" });
          await waitFor(page, p => p.notes?.windows?.techs == null, "the tech window closed");
          const again = await card("choose_tech");
          await page.mouse.click(again.cx, again.cy, { button: "right" });
          await page.waitForTimeout(600);
          const after = await probe(page);
          check("4. a right click does not dismiss the research note (the core keeps it)", after.notes.list.some(n => n.id === tech.note.id) && after.notes.windows.techs == null, JSON.stringify(after.notes.list.map(n => n.go)));
        }
        const prod = await card("production");
        check("4. the production note has a card", !!prod, JSON.stringify(prod?.note ?? null));
        if (prod) {
          await page.mouse.click(prod.cx, prod.cy);
          const open = await waitFor(page, p => p.ui.windows.includes("production"), "the production window").catch(() => null);
          check("4. a left click on the production note opens the production window of its city", !!open, JSON.stringify(open?.ui.windows ?? null));
          await page.waitForTimeout(800);
          await shot(page, "note-production");
          await intent(page, "window.close", { window: "production" });
        }
        // A note the player may dismiss: a right click takes it away.
        const free = (await probe(page)).notes.list.find(n => n.dismissable);
        if (free) {
          const c = await card(free.go);
          await page.mouse.click(c.cx, c.cy, { button: "right" });
          const gone = await waitFor(page, p => !p.notes.list.some(n => n.id === free.id), "the dismissed note gone").catch(() => null);
          check("4. a right click dismisses a note the player may dismiss", !!gone, JSON.stringify(free));
        } else {
          result.noDismissable = "no dismissable note in this game state";
        }
      }
    }

    // 5. The city screen's tiles on the map: the view looks at the city, each tile in reach has a citizen button on
    //    its plot, and a click on an unworked tile works it (the core locks it), as the panel's list does.
    {
      const s = await state(page);
      const city = s.cities?.[0];
      if (city) {
        await intent(page, "window.open", { window: "city", a: city.id });
        const open = await waitFor(page, p => p.ui.windows.includes("city") && (p.counters["ui2.anchors.shown"] ?? 0) > 0, "the city screen").catch(() => null);
        await page.waitForTimeout(1500);
        const tiles = await distinct(page, "map.city-tile.work");
        const inView = tiles.filter(r => r.x > VIEW.width * 0.25 && r.x + r.w < VIEW.width * 0.75 && r.y > 60);
        result.cityTiles = { all: tiles.length, middle: inView.length };
        check("5. the city screen shows its tiles on the map around the city", open && inView.length >= 1, JSON.stringify(result.cityTiles));
        await shot(page, "city-tiles");
        const worked = await distinct(page, "map.city-tile.worked");
        const lockedBefore = (await distinct(page, "map.city-tile.locked")).length;
        // An unworked tile: a work button that is neither worked nor locked.
        const busy = [...worked, ...await distinct(page, "map.city-tile.locked")];
        const free = inView.find(r => !busy.some(b => b.x >= r.x && b.x + b.w <= r.x + r.w + 1 && b.y >= r.y && b.y + b.h <= r.y + r.h + 1));
        if (free) {
          await page.mouse.click(free.x + free.w / 2, free.y + free.h / 2);
          let lockedAfter = lockedBefore;
          for (let i = 0; i < 30 && lockedAfter === lockedBefore; i++) {
            await page.waitForTimeout(300);
            lockedAfter = (await distinct(page, "map.city-tile.locked")).length;
          }
          result.cityTiles.locked = [lockedBefore, lockedAfter];
          check("5. a click on an unworked tile on the map works it (locked by the player)", lockedAfter === lockedBefore + 1, JSON.stringify(result.cityTiles));
          await page.waitForTimeout(500);
          await shot(page, "city-tile-worked");
        } else {
          check("5. an unworked tile is on the map", false, JSON.stringify({ tiles: inView.length, busy: busy.length }));
        }
        await intent(page, "window.close", { window: "city" });
      } else {
        check("5. the game has a city", false);
      }
    }

    check("no browser errors", browser.errors.filter(e => !e.includes("/pkg-mt/")).length === 0, browser.errors.slice(0, 3).join(" | "));
    result.pass = result.checks.every(c => c.pass);
    return result;
  } finally { await browser.close(); }
}
