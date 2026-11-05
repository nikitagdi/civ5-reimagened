// `m5-turn-ready` (live reports 2026-10-30): in a Simultaneous game of humans the players panel (MPList) shows who has
// ended the turn, and an order given after END TURN takes the end of turn back, as in the original.
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8964 --scenario m5-turn-ready [--headed]
// Server: one real-core server with the plain game data (tools/playtest/m5/serve-one.sh). Two clients (alice hosts, bob joins)
// in a simultaneous room without other seats:
//  1. both skip their units; alice presses END TURN: her button waits, bob's replica has her turn complete
//     (Players[a]:HasReceivedNetTurnComplete) and bob's MPList dims her entry (alpha 0.5);
//  2. alice moves her warrior: her button is END TURN again (Network.HasSentNetTurnComplete false, replica.turn_reopened 1),
//     the warrior moved on both pages, bob's replica and MPList show her turn open again;
//  3. both press END TURN: the turn advances on both pages;
//  ui.lua_errors == 0 and replica.diverged == 0 on both pages.
import { openPage, act, until, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { uiDriver, BLOCK } from "../lib/ui-driver.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function stats(page) {
  return page.evaluate(() => {
    const s = window.__civ5stats ?? {};
    return { ui: s.ui ?? null, replica: s.replica ?? null, exe: s.exe ?? null, m2: s.m2 ?? null };
  });
}

/** Lua: the turn handshake as this page's UI sees it, one line `key=value ...`. */
const STATE_LUA = `
local me = Game.GetActivePlayer()
local t = { "me=" .. me, "turn=" .. Game.GetGameTurn(), "sent=" .. tostring(Network.HasSentNetTurnComplete()) }
for i = 0, 3 do local p = Players[i] if p and p:IsHuman() then t[#t + 1] = "rc" .. i .. "=" .. tostring(p:HasReceivedNetTurnComplete()) end end
print(TAG .. table.concat(t, " "))`;

function parse(line) {
  const o = {};
  for (const kv of (line ?? "").split(" ")) {
    const [k, v] = kv.split("=");
    if (k) o[k] = v === "true" ? true : v === "false" ? false : Number.isNaN(Number(v)) ? v : Number(v);
  }
  return o;
}

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "m5-turn-ready", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
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
  const shot = async (b, name) => { try { await b.page.bringToFront().catch(() => {}); await b.page.screenshot({ path: `${o.logDir}/${b.nick}-${name}.png`, timeout: 90000 }); } catch { /* gone */ } };
  const signIn = async (b) => {
    const s = await until(b.page, (x) => x.screen === "login" || x.screen === "lobby", 60000, `${b.nick}: sign-in window or lobby`);
    if (s.screen === "login") await act(b.page, { kind: "login", nick: b.nick, password: DEV_PASSWORD });
    return until(b.page, (x) => x.screen === "lobby", 30000, `${b.nick}: the lobby`);
  };
  const m2Up = async (b) => {
    const t0 = Date.now();
    for (;;) {
      const st = await stats(b.page);
      if (st.m2?.phase === 2) return st;
      if (st.m2?.phase === -1) throw new Error(`${b.nick}: M2 failed (phase -1)`);
      if (Date.now() - t0 > 300000) throw new Error(`${b.nick}: M2 is not up: ${JSON.stringify(st.m2)}`);
      await sleep(500);
    }
  };
  const state = async (b) => parse((await uiDriver(b).lua(STATE_LUA))[0]);
  /** The MPList entry of each player: `{ name: alpha }` of its name label (the dump's `a<alpha>` flag, 1 without it). */
  const mpList = async (b) => {
    const lines = await uiDriver(b).lua(`for l in string.gmatch(__ui_dump_screen("MPList") or "", "[^\\n]+") do print(TAG .. l) end`, 30000);
    const out = {};
    for (const l of lines) {
      const [path, cls, , flags = "", text = ""] = l.split("\t");
      if (cls !== "Label" || !/\/Name$/.test(path) || !text.trim()) continue;
      const m = /a([0-9.]+)/.exec(flags);
      out[text.trim()] = m ? Number(m[1]) : 1;
    }
    return out;
  };
  const endTurnText = async (b) => (await uiDriver(b).dump("EndTurnText")).map((r) => r.text.trim()).join("|");
  /** Waits for `pred(state)` up to `ms`; the last state. */
  const waitState = async (b, pred, ms = 20000) => {
    const t0 = Date.now();
    let s;
    for (;;) {
      s = await state(b);
      if (pred(s) || Date.now() - t0 > ms) return s;
      await sleep(700);
    }
  };
  /** Every unit of the seat skips the turn (they keep their moves); then the blockers are handled by the UI. */
  const clearBlockers = async (b, skip = true) => {
    const ui = uiDriver(b);
    if (skip) await ui.lua(`for u in Players[Game.GetActivePlayer()]:Units() do UI.SelectUnit(u) Game.SelectionListGameNetMessage(GameMessageTypes.GAMEMESSAGE_PUSH_MISSION, GameInfoTypes.MISSION_SKIP, 0, 0, 0, false, false) end UI.ClearSelectionList()`);
    for (let i = 0; i < 12; i++) {
      await sleep(1500);
      const bl = await ui.blocking();
      if (bl.type === BLOCK.NONE) return true;
      console.log(`  . ${b.nick}: ${await ui.handleBlock(bl)}`);
    }
    return (await ui.blocking()).type === BLOCK.NONE;
  };
  let error = null;
  try {
    const nickA = `m5ra${Date.now() % 100000}`;
    const nickB = `m5rb${Date.now() % 100000}`;
    const alice = await open(nickA);
    await signIn(alice);
    await act(alice.page, { kind: "createRoom" });
    await until(alice.page, (x) => x.screen === "room", 15000, "the room");
    const roomName = `m5 ready ${nickA}`;
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
    for (const slot of s.net.room.slots.filter((y) => y.who === "open")) await act(alice.page, { kind: "slot", index: slot.index, occupant: "closed" });
    await until(alice.page, (x) => x.net?.room?.slots.every((y) => y.who !== "open") && x.net.room.canStart === true, 15000, "Start is on");
    await act(alice.page, { kind: "start" });
    for (const b of [alice, bob]) await until(b.page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 120000, `${b.nick} in the game`);
    await Promise.all([m2Up(alice), m2Up(bob)]);
    await sleep(6000);

    let sa = await state(alice), sb = await state(bob);
    const a = sa.me, bId = sb.me;
    result.numbers.seats = { alice: a, bob: bId };
    console.log(`  . alice ${JSON.stringify(sa)} | bob ${JSON.stringify(sb)}`);
    const turn0 = sa.turn;
    check("alice: nobody has ended the turn yet", sa.sent === false && sa[`rc${a}`] === false && sa[`rc${bId}`] === false, JSON.stringify(sa));

    // ---- 1: alice ends the turn ----
    check("alice: nothing blocks END TURN after skipping the units", await clearBlockers(alice));
    check("bob: nothing blocks END TURN after skipping the units", await clearBlockers(bob));
    const listBefore = await mpList(bob);
    result.numbers.mplistBefore = listBefore;
    await shot(bob, "00-bob-before");
    await uiDriver(alice).clickEndTurn();
    sa = await waitState(alice, (x) => x[`rc${a}`] === true);
    check("alice: END TURN sent (Network.HasSentNetTurnComplete)", sa.sent === true, JSON.stringify(sa));
    check("alice: the server has her turn complete (her replica)", sa[`rc${a}`] === true, JSON.stringify(sa));
    const etA = await endTurnText(alice);
    result.numbers.endTurnTextAliceWaiting = etA;
    check("alice: the button waits for the players", /wait/i.test(etA), etA);
    sb = await waitState(bob, (x) => x[`rc${a}`] === true);
    check("bob: alice's turn complete reached his replica", sb[`rc${a}`] === true && sb[`rc${bId}`] === false, JSON.stringify(sb));
    await sleep(1500);
    const listDone = await mpList(bob);
    result.numbers.mplistAliceDone = listDone;
    await shot(bob, "01-bob-alice-done");
    await shot(alice, "01-alice-waiting");
    const alphaOf = (list, nick) => Object.entries(list).find(([k]) => k.includes(nick))?.[1];
    check("bob's MPList dims alice (turn complete)", alphaOf(listDone, nickA) === 0.5 && alphaOf(listDone, nickB) === 1, JSON.stringify(listDone));

    // ---- 2: alice moves her warrior: the end of turn is taken back ----
    const mv = (await uiDriver(alice).lua(`
      local p = Players[Game.GetActivePlayer()]
      local done = false
      for u in p:Units() do
        if not done and u:GetMoves() > 0 and u:IsCombatUnit() then
          for d = 0, 5 do
            local q = Map.PlotDirection(u:GetX(), u:GetY(), d)
            if not done and q and not q:IsWater() and not q:IsImpassable() and not q:IsMountain() and q:GetNumUnits() == 0 then
              print(TAG .. u:GetID() .. " " .. u:GetX() .. " " .. u:GetY() .. " " .. q:GetX() .. " " .. q:GetY())
              UI.SelectUnit(u)
              Game.SelectionListGameNetMessage(GameMessageTypes.GAMEMESSAGE_PUSH_MISSION, GameInfoTypes.MISSION_MOVE_TO, q:GetX(), q:GetY(), 0, false, false)
              done = true
            end
          end
        end
      end`))[0];
    result.numbers.move = mv;
    check("alice: a unit to move", !!mv, mv);
    const [uid, , , tx, ty] = (mv ?? "").split(" ").map(Number);
    sa = await waitState(alice, (x) => x[`rc${a}`] === false);
    check("alice: the order took the end of turn back (HasSentNetTurnComplete false)", sa.sent === false, JSON.stringify(sa));
    check("alice: the server has her turn open again (her replica)", sa[`rc${a}`] === false, JSON.stringify(sa));
    await sleep(1500);
    const etA2 = await endTurnText(alice);
    result.numbers.endTurnTextAliceReopened = etA2;
    check("alice: the button is END TURN again", !/wait/i.test(etA2), etA2);
    const where = async (b) => (await uiDriver(b).lua(`local u = Players[${a}]:GetUnitByID(${uid}) print(TAG .. (u and (u:GetX() .. " " .. u:GetY()) or "none"))`))[0];
    const wa = await where(alice), wb = await where(bob);
    check("the warrior moved (alice's and bob's replicas)", wa === `${tx} ${ty}` && wb === `${tx} ${ty}`, `alice ${wa}, bob ${wb}, target ${tx} ${ty}`);
    let st = await stats(alice.page);
    check("alice: replica.turn_reopened == 1", st.replica?.turn_reopened === 1, `${st.replica?.turn_reopened}`);
    sb = await waitState(bob, (x) => x[`rc${a}`] === false);
    check("bob: alice's turn is open again in his replica", sb[`rc${a}`] === false, JSON.stringify(sb));
    await sleep(1500);
    const listOpen = await mpList(bob);
    result.numbers.mplistAliceOpen = listOpen;
    check("bob's MPList shows alice not done", alphaOf(listOpen, nickA) === 1, JSON.stringify(listOpen));
    await shot(bob, "02-bob-alice-open");
    await shot(alice, "02-alice-reopened");

    // ---- 2b: "Waiting for players" takes the end of turn back (live report 2026-10-31: the click did nothing) ----
    await clearBlockers(alice, false);
    await uiDriver(alice).clickEndTurn();
    sa = await waitState(alice, (x) => x[`rc${a}`] === true);
    check("alice: END TURN again (waiting)", sa.sent === true && sa[`rc${a}`] === true, JSON.stringify(sa));
    await sleep(1500);
    await uiDriver(alice).clickEndTurn();
    sa = await waitState(alice, (x) => x[`rc${a}`] === false);
    check("alice: a click on 'Waiting for players' took the end of turn back", sa.sent === false && sa[`rc${a}`] === false, JSON.stringify(sa));
    st = await stats(alice.page);
    check("alice: no UI message refused (the move after END TURN was taken)", (st.replica?.net_rejected ?? 0) === 0, `${st.replica?.net_rejected}`);

    // ---- 3: both end the turn: the turn advances ----
    // Only what blocks (the moved warrior): a second MISSION_SKIP round on the same units makes the issuing replica diverge
    // (seen without any end of turn too; a separate defect).
    await clearBlockers(alice, false);
    await uiDriver(alice).clickEndTurn();
    await waitState(bob, (x) => x[`rc${a}`] === true);
    await uiDriver(bob).clickEndTurn();
    sa = await waitState(alice, (x) => x.turn > turn0, 120000);
    sb = await waitState(bob, (x) => x.turn > turn0, 60000);
    check("the turn advanced on both pages", sa.turn === turn0 + 1 && sb.turn === turn0 + 1, `alice ${sa.turn}, bob ${sb.turn}, was ${turn0}`);
    check("the new turn: nobody has ended it", sa.sent === false && sa[`rc${a}`] === false && sb[`rc${a}`] === false, `${JSON.stringify(sa)} | ${JSON.stringify(sb)}`);
    await sleep(3000);
    await shot(alice, "03-alice-next-turn");
    await shot(bob, "03-bob-next-turn");
    for (const b of [alice, bob]) {
      st = await stats(b.page);
      result.numbers[`stats_${b.nick}`] = { lua_errors: st.ui?.lua_errors, diverged: st.replica?.diverged, net_rejected: st.replica?.net_rejected, turn_reopened: st.replica?.turn_reopened };
      check(`${b.nick}: ui.lua_errors == 0`, st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
      check(`${b.nick}: replica.diverged == 0`, st.replica?.diverged === 0, `${st.replica?.diverged}`);
    }
  } catch (e) {
    error = e;
    for (const b of pages) await shot(b, "failure");
  }
  for (const b of pages) result.consoleErrors.push(...b.errors);
  for (const b of pages) await b.close();
  if (error) { check("no scenario error", false, error.message); result.error = error.stack ?? String(error); }
  result.pass = result.checks.every((x) => x.pass);
  result.durationMs = Date.now() - started;
  return result;
}
