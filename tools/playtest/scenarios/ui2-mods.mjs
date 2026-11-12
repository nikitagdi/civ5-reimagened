// `ui2-mods`: the UI v2 panel of the example mod `hello` over a live game. Needs a real-core server with hello linked in
// and the skins and the mods' panels in its web bundle (`cargo xtask dev --wsl --mods examples/mods/hello/mods.toml ...`)
// and `--attach <url>`:
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8080 --scenario ui2-mods [--headed]
//
// A quick game with `&mods=hello` and `&ui=v2:engraved`: the panel `echo` stands in `hud.right` and reads "Echoes N";
// a click on its **Say hello** button sends `Say` through the mod's Rust half, the server's echo comes back and the
// panel shows "Echoes N+1" and "last: <nick>: hello #1". Screenshots: <logs>/ui2-mods-*.png.
import { openPage, act, until, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const UP_MS = Number(process.env.UI2_UP_MS ?? 420000);

async function ui2(page) {
  return page.evaluate(() => (window.__civ5test?.ui2 ? window.__civ5test.ui2() : null)).catch(() => null);
}

/** The texts whose nearest `data-part` is `part`, in document order. */
function partTexts(probe, part) {
  return (probe?.texts ?? []).filter((t) => t.part === part).map((t) => t.text);
}

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "ui2-mods", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const base = o.attach ?? ctx.server?.url;
  if (!o.attach) {
    check("options", false, "ui2-mods needs --attach <url of a real-core server with the mod hello>");
    result.durationMs = Date.now() - started;
    return result;
  }
  const b = await openPage({ headed: o.headed, logDir: o.logDir, viewport: { width: 1600, height: 900 }, consoleFile: "console-mods.log" });
  const shot = async (name) => {
    for (let k = 0; k < 3; k++) {
      try { await b.page.screenshot({ path: `${o.logDir}/${name}.png`, timeout: 120000 }); return; } catch { /* retry */ }
    }
  };
  try {
    const nick = `mods${Date.now() % 1000000}`;
    const t0 = Date.now();
    await b.page.goto(appUrl(base, o, { nick, autostart: "1", ai: "1", size: "duel", seed: "7", mode: "simultaneous", stats: "1", ui: "v2:engraved", mods: "hello" }));
    const s = await until(b.page, (x) => x.screen === "login" || x.screen === "play", 90000, "sign-in window or the game");
    if (s.screen === "login") await act(b.page, { kind: "login", nick, password: DEV_PASSWORD });
    await until(b.page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 120000, "the game");
    let probe = null;
    for (;;) {
      probe = await ui2(b.page);
      if (probe?.state === "classic") throw new Error("UI v2 fell back to the classic UI (see console-mods.log)");
      if (probe?.state === "running" && partTexts(probe, "echo").length) break;
      if (Date.now() - t0 > UP_MS) throw new Error(`no panel of hello after ${UP_MS / 1000} s: ${JSON.stringify({ state: probe?.state, frames: probe?.frames })}`);
      await sleep(250);
    }
    result.numbers.toPanelMs = Date.now() - t0;
    const before = partTexts(probe, "echo");
    result.numbers.before = before;
    check("the panel echo shows Echoes N and its button", before.some((t) => /^Echoes \d+$/.test(t)) && before.includes("Say hello"), JSON.stringify(before));
    const rects = await b.page.evaluate(() => window.__civ5test.ui2Rects(["echo"])).catch((e) => String(e));
    result.numbers.rects = rects;
    const r = Array.isArray(rects) ? rects[0] : null;
    check("the panel has a box", r && r.w > 0 && r.h > 0, JSON.stringify(rects));
    await shot("ui2-mods-before");
    if (r) {
      const n = Number((before.find((t) => t.startsWith("Echoes ")) ?? "Echoes 0").slice(7));
      // The button is the panel's last line: 8 px in from its left edge, 10 px above its bottom (the panel's padding is 6 px).
      await b.page.mouse.click(r.x + 30, r.y + r.h - 12);
      const t1 = Date.now();
      let after = before;
      while (Date.now() - t1 < 10000) {
        after = partTexts(await ui2(b.page), "echo");
        if (after.includes(`Echoes ${n + 1}`)) break;
        await sleep(200);
      }
      result.numbers.after = after;
      result.numbers.echoMs = Date.now() - t1;
      check("Say hello: the echo comes back to the panel", after.includes(`Echoes ${n + 1}`) && after.some((t) => t.startsWith("last: ") && t.endsWith(": hello #1")), JSON.stringify(after));
      await shot("ui2-mods-after");
    }
  } catch (e) {
    check("run", false, String(e?.stack ?? e));
    await shot("ui2-mods-error");
  } finally {
    result.consoleErrors = b.errors.filter((e) => !e.includes("/pkg-mt/"));
    await b.close();
  }
  result.pass = result.checks.length > 0 && result.checks.every((c) => c.pass);
  result.durationMs = Date.now() - started;
  return result;
}
