// `m5-g4` (M5 WO-L): the numbers of AC-G4 (the price of the game on a late Huge map) and AC-F5 (the UI frame) on the
// stored late-huge save, with the original UI (`?ui=original`) against the B3 path (no `ui` flag: egui) of the same build.
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8890 --scenario m5-g4 --late-nick late1 --late-token <file> [--perf-runs 3] [--turns 5]
// The server runs on a copy of the save's state dir (WSL); the second human (late2) must not hold the turn: kick its seat first
// (POST /admin/games/1/kick?player=1 on the admin port), then every END TURN is an AI turn of ten civilizations.
// Measured (one fresh Chrome per run):
//   join      time from the navigation to the game screen with the turn known (the B3 meaning of "entering the game"), and for
//             `ui=original` to the UI being up (`m2.phase == 2`); AC-G4: original <= B3 + 3 s
//   turns     `--turns` END TURNs by the driver of the original UI (a player's clicks, blockers handled): per turn the traffic the
//             client received (the websocket frames and `GET /replica/<hash>` bodies, as they went over the wire) and sent,
//             the time to the next turn, `replica.turn_switch_ms`, `replica.memory_mb`, `ram.wasm_linear`; AC-G4: <= 1 MB per turn
//   frame     `ui.frame_us` (the UI layer's frame: layout, Lua dispatch, draw list), `ui.layout_us`, `ui.lua_us` and the renderer's
//             `ui.cpu_ms`, sampled for 8 s idle, with the city screen and with the tech tree open; AC-F5: <= 2 ms of CPU
// Numbers go to `result.numbers`; the checks are the thresholds (a failed threshold is a failed check and is reported as it is).
import fs from "node:fs";
import { openPage, until, state, appUrl } from "../lib/browser.mjs";
import { uiDriver } from "../lib/ui-driver.mjs";
import { turnDriver } from "../lib/turns-woe.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const med = (v) => (v.length ? [...v].sort((a, b) => a - b)[Math.floor(v.length / 2)] : null);
const pct = (v, p) => (v.length ? [...v].sort((a, b) => a - b)[Math.min(v.length - 1, Math.floor((v.length * p) / 100))] : null);
const stats = (page) =>
  page.evaluate(() => {
    const s = window.__civ5stats ?? {};
    return { ui: s.ui ?? null, replica: s.replica ?? null, exe: s.exe ?? null, m2: s.m2 ?? null, c: s.counters ?? {}, timeToMapMs: s.timeToMapMs ?? null, frame: s.frame ?? null };
  });

/** Counts the bytes the page receives and sends: websocket frames and the bodies of `/replica/` fetches (wire size). */
function traffic(page) {
  const t = { wsIn: 0, wsOut: 0, replicaIn: 0, replicaReqs: 0, otherIn: 0 };
  page.on("websocket", (ws) => {
    ws.on("framereceived", (f) => (t.wsIn += typeof f.payload === "string" ? Buffer.byteLength(f.payload) : f.payload.length));
    ws.on("framesent", (f) => (t.wsOut += typeof f.payload === "string" ? Buffer.byteLength(f.payload) : f.payload.length));
  });
  page.on("requestfinished", async (req) => {
    try {
      const sz = await req.sizes();
      if (/\/replica\//.test(req.url())) {
        t.replicaIn += sz.responseBodySize + sz.responseHeadersSize;
        t.replicaReqs++;
      } else if (!/\/assets?\//.test(req.url())) t.otherIn += sz.responseBodySize;
    } catch {
      /* the page went away */
    }
  });
  return t;
}

async function joinRun(base, o, token, nick, original) {
  const b = await openPage({ headed: o.headed, logDir: o.logDir, consoleFile: `join-${original ? "ui" : "b3"}-${Date.now() % 100000}.log`, viewport: { width: 1600, height: 900 } });
  const r = { original };
  try {
    await b.page.addInitScript(([n, tok]) => {
      try {
        localStorage.setItem("civ5.nick", n);
        localStorage.setItem(`civ5.token.${n}`, tok);
      } catch {
        /* blocked */
      }
    }, [nick, token]);
    const t0 = Date.now();
    await b.page.goto(appUrl(base, o, { nick, stats: "1", ...(original ? { ui: "original" } : {}) }));
    await until(b.page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 300000, "the late game");
    r.playMs = Date.now() - t0;
    if (original) {
      for (;;) {
        const s = await stats(b.page);
        if (s.m2?.phase === 2) {
          r.uiUpMs = Date.now() - t0;
          break;
        }
        if (s.m2?.phase === -1) throw new Error("M2 failed");
        if (Date.now() - t0 > 400000) throw new Error("the UI is not up");
        await sleep(200);
      }
    }
    await sleep(2500);
    const s = await stats(b.page);
    r.timeToMapMs = s.timeToMapMs;
    r.m2 = s.m2 ? { up_ms: s.m2.up_ms, ui_ready_ms: s.m2.ui_ready_ms, create_ms: s.m2.create_ms, root_ms: s.m2.root_ms } : null;
    r.replica = s.replica ? { join_ms: s.replica.join_ms, memory_mb: s.replica.memory_mb } : null;
    r.wasmLinearMb = Math.round((s.c["ram.wasm_linear"] ?? 0) / 1048576);
    r.threads = s.c["app.threads"] ?? null;
  } catch (e) {
    r.error = e.message;
  }
  r.consoleErrors = b.errors.slice(0, 3);
  await b.close();
  return r;
}

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "m5-g4", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const base = o.attach ?? o.lateUrl;
  if (!base || !o.lateNick || !o.lateToken) {
    check("options", false, "m5-g4 needs --attach <url> (or --late-url), --late-nick and --late-token");
    return result;
  }
  const token = fs.readFileSync(o.lateToken, "utf8").trim();
  const runs = o.perfRuns ?? 3;
  const turns = o.turns ?? 5;
  const n = result.numbers;
  let error = null;
  try {
    // 1. Entering the game: the B3 path and the original UI, alternating so that both see the same warm caches.
    const b3 = [];
    const ui = [];
    for (let i = 0; i < runs; i++) {
      b3.push(await joinRun(base, o, token, o.lateNick, false));
      console.log(`  join b3 #${i + 1}: ${JSON.stringify(b3.at(-1))}`);
      ui.push(await joinRun(base, o, token, o.lateNick, true));
      console.log(`  join ui #${i + 1}: ${JSON.stringify(ui.at(-1))}`);
    }
    n.joinB3 = b3;
    n.joinOriginal = ui;
    const okb3 = b3.filter((r) => !r.error);
    const okui = ui.filter((r) => !r.error);
    const b3Play = med(okb3.map((r) => r.playMs));
    const uiPlay = med(okui.map((r) => r.playMs));
    const uiUp = med(okui.map((r) => r.uiUpMs));
    n.join = { b3PlayMsMedian: b3Play, originalPlayMsMedian: uiPlay, originalUiUpMsMedian: uiUp, b3TimeToMapMsMedian: med(okb3.map((r) => r.timeToMapMs)), originalTimeToMapMsMedian: med(okui.map((r) => r.timeToMapMs)) };
    check("AC-G4 enter the game: original UI game screen <= B3 + 3 s", b3Play !== null && uiPlay !== null && uiPlay <= b3Play + 3000, `B3 ${b3Play} ms, original ${uiPlay} ms (UI up at ${uiUp} ms)`);
    check("AC-G4 enter the game: original UI up <= B3 + 3 s (stricter reading)", b3Play !== null && uiUp !== null && uiUp <= b3Play + 3000, `B3 ${b3Play} ms, UI up ${uiUp} ms`);

    // 2. Turns, the frame and the memory in one long session of the original UI.
    const b = await openPage({ headed: o.headed, logDir: o.logDir, viewport: { width: 1600, height: 900 } });
    try {
      const tr = traffic(b.page);
      await b.page.addInitScript(([nick, tok]) => {
        try {
          localStorage.setItem("civ5.nick", nick);
          localStorage.setItem(`civ5.token.${nick}`, tok);
        } catch {
          /* blocked */
        }
      }, [o.lateNick, token]);
      await b.page.goto(appUrl(base, o, { nick: o.lateNick, stats: "1", ui: "original" }));
      await until(b.page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 300000, "the late game");
      for (let t0 = Date.now(); ; ) {
        const s = await stats(b.page);
        if (s.m2?.phase === 2) break;
        if (Date.now() - t0 > 400000) throw new Error("the UI is not up");
        await sleep(300);
      }
      await sleep(5000);
      const drv = uiDriver(b);

      // The frame: idle, city screen, tech tree.
      const sample = async (label, secs) => {
        const frame = [];
        const layout = [];
        const lua = [];
        const cpu = [];
        const t0 = Date.now();
        while (Date.now() - t0 < secs * 1000) {
          const s = await stats(b.page);
          if (s.ui) {
            frame.push(s.ui.frame_us);
            layout.push(s.ui.layout_us);
            lua.push(s.ui.lua_us);
          }
          if (typeof s.c["ui.cpu_ms"] === "number") cpu.push(s.c["ui.cpu_ms"] * 1000);
          await sleep(120);
        }
        const out = {
          samples: frame.length,
          frame_us: { p50: med(frame), p95: pct(frame, 95), max: Math.max(...frame, 0) },
          layout_us: { p50: med(layout), p95: pct(layout, 95), max: Math.max(...layout, 0) },
          lua_us: { p50: med(lua), p95: pct(lua, 95), max: Math.max(...lua, 0) },
          render_ui_cpu_us: { p50: med(cpu), p95: pct(cpu, 95), max: Math.max(...cpu, 0) },
        };
        n[`frame_${label}`] = out;
        const total = (out.frame_us.p95 ?? 0) + (out.render_ui_cpu_us.p95 ?? 0);
        check(`AC-F5 UI frame CPU (${label}): ui.frame_us p95 + ui.cpu_ms p95 <= 2 ms`, total <= 2000, `${total} us (frame ${out.frame_us.p95}, render ${out.render_ui_cpu_us.p95}); layout_us max ${out.layout_us.max}`);
      };
      await sample("idle", 8);
      const bannerBtn = await drv.find("CityBannerManager/CityBanners/Anchor/CityBannerButton", (r) => r.cls === "Button");
      if (bannerBtn) {
        await drv.click(bannerBtn);
        await sleep(4000);
        await sample("city-screen", 8);
        await drv.closeCityScreen();
        await sleep(2000);
      } else check("a city banner to open the city screen", false, "none in the dump");
      const logBtn = (await drv.dump("DiploCornerStack")).find((x) => x.cls === "Button" && /^tip:Notification Log/.test(x.text));
      if (logBtn) {
        await drv.click(logBtn);
        await sleep(3500);
        await sample("notification-log", 8);
        await b.page.keyboard.press("Escape");
        await sleep(1500);
      } else check("the notification log button", false, "not in the diplomacy corner");
      await drv.lua(`Events.SerialEventGameMessagePopup({ Type = ButtonPopupTypes.BUTTONPOPUP_TECH_TREE, Data2 = -1 })`);
      await sleep(4000);
      await sample("tech-tree", 8);
      await b.page.keyboard.press("Escape");
      await sleep(1500);
      let st = await stats(b.page);
      check("ui.lua_errors == 0 after the frame samples", st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);

      // The turns.
      // The notification list is a ring of 36: compare by TEXT, not by index. Texts stored in the save before the turns are
      // not the core's work of this build, so only texts that are new after the turns count.
      const readNotes = async () => {
        const lines = await drv.lua(`
          local p = Players[Game.GetActivePlayer()]
          for i = 0, 600 do
            local s = p:GetNotificationStr(i)
            local m = p:GetNotificationSummaryStr(i)
            if (s == nil or s == "") and (m == nil or m == "") then break end
            print(TAG .. "T " .. string.gsub(s or "", "[\\r\\n]+", " ") .. " || " .. string.gsub(m or "", "[\\r\\n]+", " "))
          end`);
        return lines.filter((l) => l.startsWith("T ")).map((l) => l.slice(2));
      };
      const unresolved = (s) => /\{@|: ?plural|\{\d_/.test(s);
      const notesBeforeSet = new Set(await readNotes());      const perTurn = [];
      const turnOf = async () => (await stats(b.page)).replica?.turn ?? 0;
      const td = turnDriver(b, drv);
      for (let k = 0; k < turns; k++) {
        const at = await turnOf();
        const before = { ...tr };
        const t0 = Date.now();
        await td.endTurn();
        const ms = Date.now() - t0;
        await sleep(4000); // the body of the patch and the rest of the turn's traffic
        st = await stats(b.page);
        const rec = {
          turn: at + 1,
          wallMs: ms,
          wsIn: tr.wsIn - before.wsIn,
          wsOut: tr.wsOut - before.wsOut,
          replicaIn: tr.replicaIn - before.replicaIn,
          replicaReqs: tr.replicaReqs - before.replicaReqs,
          otherIn: tr.otherIn - before.otherIn,
          turn_switch_ms: st.replica?.turn_switch_ms,
          turn_wait_ms: st.replica?.turn_wait_ms,
          memory_mb: st.replica?.memory_mb,
          wasm_linear_mb: Math.round((st.c["ram.wasm_linear"] ?? 0) / 1048576),
          diverged: st.replica?.diverged,
          lua_errors: st.ui?.lua_errors,
        };
        rec.totalIn = rec.wsIn + rec.replicaIn;
        perTurn.push(rec);
        console.log(`  ${JSON.stringify(rec)}`);
      }
      n.turns = perTurn;
      // The texts the core composed for the notifications of those AI turns: no unresolved grammar (`{@1_CivName: plural ...}`).
      const notesAfter = await readNotes();
      const fresh = notesAfter.filter((s) => !notesBeforeSet.has(s));
      const freshBad = fresh.filter(unresolved);
      n.notificationTexts = { before: notesBeforeSet.size, after: notesAfter.length, fresh: fresh.length, freshUnresolved: freshBad.length, sample: fresh.slice(0, 8), badSample: freshBad.slice(0, 5) };
      check("notification texts composed by the core in the AI turns above have no unresolved grammar", freshBad.length === 0, `${fresh.length} new texts, ${freshBad.length} unresolved${freshBad[0] ? ": " + freshBad[0] : ""}`);
      const avgIn = perTurn.reduce((a, r) => a + r.totalIn, 0) / Math.max(1, perTurn.length);
      const avgOut = perTurn.reduce((a, r) => a + r.wsOut, 0) / Math.max(1, perTurn.length);
      n.trafficPerTurn = { avgInBytes: Math.round(avgIn), avgOutBytes: Math.round(avgOut), maxInBytes: Math.max(...perTurn.map((r) => r.totalIn)), turns: perTurn.length };
      check("AC-G4 traffic per turn (AI turn included) <= 1 MB on average", perTurn.length > 0 && avgIn + avgOut <= 1048576, `in ${Math.round(avgIn)} B + out ${Math.round(avgOut)} B per turn over ${perTurn.length} turns`);
      const memMax = Math.max(...perTurn.map((r) => r.memory_mb ?? 0), st.replica?.memory_mb ?? 0);
      n.memory = { replica_memory_mb: st.replica?.memory_mb, replica_memory_mb_max: memMax, wasm_linear_mb: Math.round((st.c["ram.wasm_linear"] ?? 0) / 1048576), heap_live_mb: Math.round((st.c["ram.heap_live"] ?? 0) / 1048576), js_heap_used_mb: Math.round((st.c["ram.js_heap_used"] ?? 0) / 1048576) };
      check("AC-G4 replica memory <= 300 MB (wasm)", memMax <= 300, `replica module ${memMax} MB, whole wasm linear memory ${n.memory.wasm_linear_mb} MB`);
      check("replica.diverged == 0 after the turns", (st.replica?.diverged ?? 0) === 0, `${st.replica?.diverged}`);
      check("ui.lua_errors == 0 after the turns", st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
      n.finalTurn = (await state(b.page))?.turn;
    } finally {
      result.consoleErrors.push(...b.errors);
      await b.close();
    }
  } catch (e) {
    error = e;
  }
  check("no console errors", result.consoleErrors.length === 0, `${result.consoleErrors.length} error(s)${result.consoleErrors[0] ? `: ${result.consoleErrors[0]}` : ""}`);
  if (error) {
    check("no scenario error", false, error.message);
    result.error = error.stack ?? String(error);
  }
  result.pass = result.checks.every((c) => c.pass);
  result.durationMs = Date.now() - started;
  return result;
}
