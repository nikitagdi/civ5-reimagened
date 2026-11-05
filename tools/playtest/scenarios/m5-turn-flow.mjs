// `m5-turn-flow` (live report 2026-10-31): when every human has ended the turn, the END TURN button of a seat goes from
// "Waiting for players" (disabled) straight to the new turn; it does not flip back and forth while the server runs the AI.
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8970 --scenario m5-turn-flow [--headed]
// Server: one real-core server with the plain game data (tools/playtest/m5/serve-one.sh), started with
// CIV5_DEV_AI_PHASE_DELAY_MS=12000 in its environment: the core call that ends the human phase then takes 12 s more, as the
// AI turns of a late game do. Two humans (alice hosts, bob joins) in a simultaneous room, TF_AI (4) slots AI, the others closed.
// TF_TURNS (3) turns. TF_AUTO=1: "Multiplayer Auto End Turn" is on for both. TF_POKE=1: both click the END TURN button again
// while the server runs the AI (the live flicker needs one of the two: game 936 had 16 END_TURNs refused in 9 s).
// For `turns` turns: both skip their units, alice ends the turn, then bob. From bob's click until the new turn has settled
// both pages are sampled as fast as the page answers: the END TURN text and its disabled flag, the turn, the net turn
// handshake (Network.HasSentNetTurnComplete / HasSentNetTurnAllComplete, the humans' HasReceivedNetTurnComplete), the
// replica's loads and generation; Lua listeners log every SerialEventEndTurnDirty / RemotePlayerTurnEnd /
// ActivePlayerTurnStart / ActivePlayerTurnEnd / NewGameTurn with the same handshake (console-<nick>.log).
// The timeline of the button's (text, disabled) per page is written to timeline-<nick>-t<turn>.txt in the log dir.
// Checks: after a seat's own END TURN the button never shows the old turn's actions again: once it waited it stays waiting
// (one text "Waiting for players"/"Please wait", disabled once everybody is done) until the turn number changes, and then
// it shows the new turn; ui.lua_errors == 0 and replica.diverged == 0 on both pages.
import { writeFileSync } from "node:fs";
import { openPage, act, until, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { uiDriver, BLOCK } from "../lib/ui-driver.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
/** One driver per page: the tags of its Lua calls are numbered per driver. */
const drv = (b) => (b.ui ??= uiDriver(b));
async function stats(page) {
  return page.evaluate(() => {
    const s = window.__civ5stats ?? {};
    return { ui: s.ui ?? null, replica: s.replica ?? null };
  });
}

/** Lua: the handshake as one `k=v` line (also used by the listeners). */
const PROBE = `local function __tf_probe(tag)
  local me = Game.GetActivePlayer()
  local p = Players[me]
  local t = { tag, "turn=" .. Game.GetGameTurn(), "sent=" .. tostring(Network.HasSentNetTurnComplete()),
    "all=" .. tostring(Network.HasSentNetTurnAllComplete()), "active=" .. tostring(p and p:IsTurnActive()),
    "busy=" .. tostring(Game.IsProcessingMessages and Game.IsProcessingMessages()) }
  for i = 0, 7 do local q = Players[i] if q and q:IsHuman() then t[#t + 1] = "rc" .. i .. "=" .. tostring(q:HasReceivedNetTurnComplete()) end end
  local function fields(l) local f, s = {}, 1 while true do local i = string.find(l, "\\t", s, true) if not i then f[#f + 1] = string.sub(l, s) break end f[#f + 1] = string.sub(l, s, i - 1) s = i + 1 end return f end
  local et, dis = "", "nil"
  for l in string.gmatch(__ui_dump_screen("EndTurn") or "", "[^\\n]+") do
    local f = fields(l)
    if string.find(f[1] or "", "/EndTurnText$") then et = et .. (f[5] or "") end
    if string.find(f[1] or "", "/EndTurnButton$") then dis = tostring(string.find(f[4] or "", "D", 1, true) ~= nil) end
  end
  t[#t + 1] = "dis=" .. dis
  t[#t + 1] = "text=" .. string.gsub(et, " ", "_")
  return table.concat(t, " ")
end`;

const LISTEN = `${PROBE}
__tf_probe_g = __tf_probe
if not __tf_listening then
  __tf_listening = true
  for _, name in ipairs({ "SerialEventEndTurnDirty", "RemotePlayerTurnEnd", "ActivePlayerTurnStart", "ActivePlayerTurnEnd", "NewGameTurn" }) do
    Events[name].Add(function() print("TFEV " .. __tf_probe_g(name)) end)
  end
end`;

function parse(line) {
  const o = {};
  for (const kv of (line ?? "").split(" ")) {
    const i = kv.indexOf("=");
    if (i < 0) continue;
    const k = kv.slice(0, i), v = kv.slice(i + 1);
    o[k] = v === "true" ? true : v === "false" ? false : v !== "" && !Number.isNaN(Number(v)) ? Number(v) : v;
  }
  return o;
}

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const turns = Number(process.env.TF_TURNS ?? 3);
  const aiSlots = Number(process.env.TF_AI ?? 4);
  const result = { scenario: "m5-turn-flow", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => { result.checks.push({ name, pass: !!pass, detail: detail ?? "" }); console.log(`  ${pass ? "ok  " : "FAIL"} ${name}${detail ? ` (${detail})` : ""}`); };
  const base = o.attach ?? ctx.server?.url;
  if (!base) { check("options", false, "needs --attach <url of the real-core server>"); return result; }
  const view = { width: 1600, height: 900 };
  const pages = [];
  const open = async (nick) => {
    const b = await openPage({ headed: o.headed, logDir: o.logDir, consoleFile: `console-${nick}.log`, viewport: view });
    b.nick = nick;
    pages.push(b);
    await b.page.goto(appUrl(base, o, { nick, ui: "original", stats: "1" }), { timeout: 180000 });
    return b;
  };
  const signIn = async (b) => {
    const s = await until(b.page, (x) => x.screen === "login" || x.screen === "lobby", 60000, `${b.nick}: sign-in window or lobby`);
    if (s.screen === "login") await act(b.page, { kind: "login", nick: b.nick, password: DEV_PASSWORD });
    return until(b.page, (x) => x.screen === "lobby", 30000, `${b.nick}: the lobby`);
  };
  const m2Up = async (b) => {
    const t0 = Date.now();
    for (;;) {
      const st = await page_stats(b);
      if (st?.phase === 2) return;
      if (st?.phase === -1) throw new Error(`${b.nick}: M2 failed`);
      if (Date.now() - t0 > 300000) throw new Error(`${b.nick}: M2 is not up`);
      await sleep(500);
    }
  };
  const page_stats = (b) => b.page.evaluate(() => window.__civ5stats?.m2 ?? null);
  const probe = async (b) => {
    const [l] = await drv(b).lua(`${PROBE}\nprint(TAG .. __tf_probe("poll"))`);
    return parse(l);
  };
  const clearBlockers = async (b, skip = true) => {
    const ui = drv(b);
    if (skip) await ui.lua(`for u in Players[Game.GetActivePlayer()]:Units() do if u:IsReadyToMove() then UI.SelectUnit(u) Game.SelectionListGameNetMessage(GameMessageTypes.GAMEMESSAGE_PUSH_MISSION, GameInfoTypes.MISSION_FORTIFY, 0, 0, 0, false, false) end end UI.ClearSelectionList()`);
    for (let i = 0; i < 14; i++) {
      await sleep(1500);
      const bl = await ui.blocking();
      if (bl.type === BLOCK.NONE) return true;
      console.log(`  . ${b.nick}: ${await ui.handleBlock(bl)}`);
    }
    return (await ui.blocking()).type === BLOCK.NONE;
  };
  /** Samples a page until `stop()`; the samples with the page's time. */
  const sampler = (b) => {
    const samples = [];
    let on = true;
    const done = (async () => {
      while (on) {
        try {
          const s = await probe(b);
          s.at = Date.now();
          const st = await stats(b.page);
          s.loads = st.replica?.loads; s.rstate = st.replica?.state ?? ""; s.resyncs = st.replica?.resyncs;
          samples.push(s);
        } catch (e) { samples.push({ at: Date.now(), err: String(e.message ?? e) }); await sleep(200); }
      }
    })();
    return { samples, stop: async () => { on = false; await done; } };
  };
  let error = null;
  try {
    const nickA = `tfa${Date.now() % 100000}`;
    const nickB = `tfb${Date.now() % 100000}`;
    const alice = await open(nickA);
    await signIn(alice);
    await act(alice.page, { kind: "createRoom" });
    await until(alice.page, (x) => x.screen === "room", 15000, "the room");
    const roomName = `tf ${nickA}`;
    await act(alice.page, { kind: "roomSetting", field: "name", value: roomName });
    await until(alice.page, (x) => x.net?.room?.name === roomName, 10000, "the room name");
    await act(alice.page, { kind: "roomSetting", field: "turnMode", value: "simultaneous" });
    await act(alice.page, { kind: "roomSetting", field: "minorCivs", value: 0 });
    const bob = await open(nickB);
    let s = await signIn(bob);
    s = await until(bob.page, (x) => x.net?.rooms?.some((r) => r.name === roomName), 15000, "the room in bob's list");
    await act(bob.page, { kind: "joinRoom", room: s.net.rooms.find((r) => r.name === roomName).id });
    await until(bob.page, (x) => x.screen === "room", 15000, "bob in the room");
    await until(alice.page, (x) => x.net?.room?.slots.some((y) => y.nick === nickB), 15000, "alice sees bob");
    await act(bob.page, { kind: "ready", value: true });
    s = await until(alice.page, (x) => x.net?.room?.slots.find((y) => y.nick === nickB)?.ready === true, 15000, "bob ready");
    for (const [i, slot] of s.net.room.slots.filter((y) => y.who === "open").entries()) await act(alice.page, { kind: "slot", index: slot.index, occupant: i < aiSlots ? "ai" : "closed" });
    await until(alice.page, (x) => x.net?.room?.slots.every((y) => y.who !== "open") && x.net.room.canStart === true, 15000, "Start is on");
    await act(alice.page, { kind: "start" });
    for (const b of [alice, bob]) await until(b.page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 180000, `${b.nick} in the game`);
    await Promise.all([m2Up(alice), m2Up(bob)]);
    await sleep(6000);
    for (const b of [alice, bob]) await drv(b).lua(LISTEN);
    // TF_AUTO=1: the options menu's "Multiplayer Auto End Turn" is on for both seats (the page's store).
    if (process.env.TF_AUTO === "1") for (const b of [alice, bob]) await b.page.evaluate(() => localStorage.setItem("civ5.options.MultiplayerAutoEndTurn", "1"));

    const textOf = (x) => String(x.text ?? "").replace(/_/g, " ");
    const waitingText = (t) => /wait/i.test(t);
    for (let k = 0; k < turns; k++) {
      const turn0 = (await probe(alice)).turn;
      check(`t${turn0} alice: nothing blocks`, await clearBlockers(alice, k === 0 || true));
      check(`t${turn0} bob: nothing blocks`, await clearBlockers(bob, k === 0 || true));
      const sa = sampler(alice), sb = sampler(bob);
      await sleep(800);
      if (!(await probe(alice)).sent) await drv(alice).clickEndTurn(); // (the auto end of turn may have ended it)
      const markA = Date.now();
      await sleep(4000);
      if (!(await probe(bob)).sent) await drv(bob).clickEndTurn();
      const markB = Date.now();
      // TF_POKE=1: while the server runs the AI the players click the END TURN button again ("Waiting for players"), as
      // players do while they wait.
      if (process.env.TF_POKE === "1") {
        await sleep(3000);
        await drv(bob).clickEndTurn().catch(() => {});
        await sleep(1500);
        await drv(alice).clickEndTurn().catch(() => {});
      }
      const t0 = Date.now();
      while (Date.now() - t0 < 240000) {
        const x = sa.samples.at(-1), y = sb.samples.at(-1);
        if (x?.turn > turn0 && y?.turn > turn0) break;
        await sleep(500);
      }
      await sleep(6000);
      await sa.stop(); await sb.stop();
      for (const [b, sm, mark] of [[alice, sa, markA], [bob, sb, markB]]) {
        const lines = [];
        let last = "";
        for (const x of sm.samples) {
          const key = x.err ? `err ${x.err}` : `t${x.turn} "${textOf(x)}" dis=${x.dis} sent=${x.sent} all=${x.all} active=${x.active} busy=${x.busy} ${Object.keys(x).filter((q) => q.startsWith("rc")).map((q) => `${q}=${x[q]}`).join(" ")} loads=${x.loads} ${x.rstate}`;
          if (key !== last) lines.push(`${((x.at - mark) / 1000).toFixed(2)}s ${key}`);
          last = key;
        }
        writeFileSync(`${o.logDir}/timeline-${b.nick}-t${turn0}.txt`, lines.join("\n") + "\n");
        // From the seat's own click to the new turn: the (text, disabled) states the button went through.
        const after = sm.samples.filter((x) => !x.err && x.at >= mark + 400);
        const seq = [];
        for (const x of after) {
          const st = x.turn > turn0 ? `new:${waitingText(textOf(x)) ? "wait" : "turn"}` : waitingText(textOf(x)) ? `wait${x.dis ? "-dis" : ""}` : `old:${textOf(x)}`;
          if (seq.at(-1) !== st) seq.push(st);
        }
        result.numbers[`seq_${b.nick}_t${turn0}`] = seq.join(" > ");
        const oldTurnBack = seq.some((q) => q.startsWith("old:"));
        const flips = seq.filter((q, i) => i > 0 && q.startsWith("wait") && seq[i - 1].startsWith("wait") && q !== seq[i - 1]).length;
        const newWait = seq.indexOf("new:turn") >= 0 && seq.slice(seq.indexOf("new:turn")).some((q) => q === "new:wait");
        check(`t${turn0} ${b.nick}: the button goes from waiting straight to the new turn`, !oldTurnBack && !newWait && seq.at(-1)?.startsWith("new:"), seq.join(" > "));
        // Once every human is done the button is disabled (Network.HasSentNetTurnAllComplete), as after the exe's NetTurnAllComplete.
        check(`t${turn0} ${b.nick}: the button is disabled while the turn is processed`, seq.includes("wait-dis"), seq.join(" > "));
        result.numbers[`flips_${b.nick}_t${turn0}`] = flips;
      }
    }
    for (const b of [alice, bob]) {
      const st = await stats(b.page);
      result.numbers[`stats_${b.nick}`] = { lua_errors: st.ui?.lua_errors, diverged: st.replica?.diverged, resyncs: st.replica?.resyncs, loads: st.replica?.loads };
      check(`${b.nick}: ui.lua_errors == 0`, st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
      check(`${b.nick}: replica.diverged == 0`, st.replica?.diverged === 0, `${st.replica?.diverged}`);
    }
  } catch (e) {
    error = e;
  }
  for (const b of pages) result.consoleErrors.push(...b.errors);
  for (const b of pages) await b.close();
  if (error) { check("no scenario error", false, error.message); result.error = error.stack ?? String(error); }
  result.pass = result.checks.every((x) => x.pass);
  result.durationMs = Date.now() - started;
  return result;
}
