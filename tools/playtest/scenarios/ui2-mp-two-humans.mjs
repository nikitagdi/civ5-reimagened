// `ui2-mp-two-humans`: two humans in one room, both on UI v2 (two browser contexts). Proves the session operations seen
// from both seats: a save by the host shows in both game menus, a non-host's host operation is refused, a pause and a
// resume by the host are seen by both, the host sees the other seat's connection change when its page closes, and the
// host kicks that seat from the civilization ribbon (the seat becomes the AI's).
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8880 --scenario ui2-mp-two-humans
// The room is made as `m5-mp-trade-kick` makes it (alice hosts, bob joins, no AI or city-state seats).
import { openPage, act, until, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { waitFor, texts, openScreen, clickPart } from "./ui2-screen-city-support.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export async function run(ctx) {
  const o = ctx.opts;
  const result = { scenario: "ui2-mp-two-humans", pass: false, checks: [] };
  const check = (name, pass, detail) => {
    result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
    console.log(`  ${pass ? "ok  " : "FAIL"} ${name}${detail ? ` (${detail})` : ""}`);
    if (!pass) throw new Error(name);
  };
  const base = o.attach ?? ctx.server?.url;
  if (!base) throw new Error("ui2-mp-two-humans requires --attach to a real-core server");
  const pages = [];
  const open = async (nick) => {
    const b = await openPage({ headed: o.headed, logDir: o.logDir, consoleFile: `console-${nick}.log`, viewport: { width: 1600, height: 900 } });
    b.nick = nick;
    pages.push(b);
    await b.page.goto(appUrl(base, o, { nick, ui: "v2:engraved", manual: "1" }), { timeout: 180000 });
    return b;
  };
  const signIn = async (b) => {
    const s = await until(b.page, (x) => x.screen === "login" || x.screen === "lobby", 90000, `${b.nick}: sign-in window or lobby`);
    if (s.screen === "login") await act(b.page, { kind: "login", nick: b.nick, password: DEV_PASSWORD });
    return until(b.page, (x) => x.screen === "lobby", 30000, `${b.nick}: the lobby`);
  };
  const shot = async (b, name) => { try { await b.page.screenshot({ path: `${o.logDir}/${b.nick}-${name}.png`, timeout: 60000 }); } catch { /* gone */ } };
  const saveKeys = async (b) => ((await b.page.evaluate(() => window.__civ5test.ui2Rects(["game-menu.save-row"]))) ?? [])
    .filter((r) => r.keys && r.w > 0).map((r) => String(r.keys));
  try {
    const stamp = Date.now() % 100000;
    const nickA = `uita${stamp}`, nickB = `uitb${stamp}`;
    const alice = await open(nickA);
    await signIn(alice);
    await act(alice.page, { kind: "createRoom" });
    await until(alice.page, (x) => x.screen === "room", 15000, "the room");
    const roomName = `v2 two humans ${nickA}`;
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
    for (const b of [alice, bob]) {
      await until(b.page, (x) => x.screen === "play" && x.turn !== null, 180000, `${b.nick} in the game`);
      await waitFor(b.page, (p) => p?.state === "running" && p.frames > 10, `${b.nick}: UI v2 running`, 180000);
    }
    const A = alice.page, B = bob.page;
    // The ribbon starts folded behind its toggle (the notifications keep the right edge): both open it with 1.
    for (const pg of [A, B]) {
      await pg.keyboard.press("1");
      await waitFor(pg, (p) => texts(p, "civ-ribbon.name").length > 0, "the unfolded civilization ribbon", 30000);
    }

    // The ribbon lists both humans (met or not, as the original multiplayer list does); only the host has a kick button.
    await waitFor(A, (p) => texts(p, "civ-ribbon.kick").length === 1, "alice: one kick button (bob's seat)", 60000);
    check("the host's ribbon has a kick button for the other human", true);
    const bobView = await waitFor(B, (p) => texts(p, "civ-ribbon.seat").includes("Host"), "bob: the host mark on alice's seat", 60000);
    check("the other seat sees who hosts and has no kick button", texts(bobView, "civ-ribbon.kick").length === 0);

    // A host operation from the other seat is refused, and the refusal is shown.
    await openScreen(B, "game_menu");
    await waitFor(B, (p) => texts(p, "game-menu.save").length > 0, "bob: the game menu");
    await B.evaluate(() => window.__civ5test.ui2Intent({ name: "session.op", args: { op: "save" } }));
    await waitFor(B, (p) => texts(p, "game-menu.refused").includes("Only the host can do this"), "bob: the refusal");
    check("a non-host's save is refused with the host reason", true);

    // A save by the host joins the list of saves of both seats.
    await openScreen(A, "game_menu");
    await waitFor(A, (p) => texts(p, "game-menu.save").length > 0, "alice: the game menu");
    const beforeA = new Set(await saveKeys(alice)), beforeB = new Set(await saveKeys(bob));
    await clickPart(A, "game-menu.save");
    const fresh = async (b, before) => {
      const end = Date.now() + 60000;
      while (Date.now() < end) {
        const k = (await saveKeys(b)).filter((x) => !before.has(x));
        if (k.length) return k;
        await sleep(300);
      }
      return [];
    };
    const [newA, newB] = await Promise.all([fresh(alice, beforeA), fresh(bob, beforeB)]);
    check("the host's save shows in the host's menu", newA.length > 0, newA.join(","));
    check("the host's save shows in the other seat's menu", newB.length > 0 && newB.some((k) => newA.includes(k)), newB.join(","));
    await shot(bob, "1-save-seen");

    // Pause and resume by the host, seen by both seats.
    await clickPart(A, "game-menu.pause");
    await B.evaluate(() => window.__civ5test.ui2Intent({ name: "window.close", args: { window: "game_menu" } }));
    await waitFor(B, (p) => texts(p, "helm.paused").length > 0, "bob: the paused notice", 30000);
    check("the other seat sees the pause", true);
    await waitFor(A, (p) => texts(p, "game-menu.resume").length > 0, "alice: the resume button", 30000);
    check("the host's menu offers to resume", true);
    await shot(bob, "2-paused");
    await clickPart(A, "game-menu.resume");
    await waitFor(B, (p) => texts(p, "helm.paused").length === 0, "bob: the pause is over", 30000);
    await A.evaluate(() => window.__civ5test.ui2Intent({ name: "window.close", args: { window: "game_menu" } }));
    await waitFor(A, (p) => texts(p, "helm.paused").length === 0, "alice: the pause is over", 30000);
    check("both seats see the resume", true);

    // The other seat's page closes: the host sees its connection change, then kicks it from the ribbon.
    await waitFor(A, (p) => texts(p, "civ-ribbon.seat").filter((t) => t === "Connected").length >= 2, "alice: both seats connected", 30000);
    await bob.close();
    pages.splice(pages.indexOf(bob), 1);
    await waitFor(A, (p) => texts(p, "civ-ribbon.seat").some((t) => t === "Connection lost" || t === "Disconnected"), "alice: bob's connection is lost", 90000);
    check("the host sees the closed seat's connection change", true);
    await shot(alice, "3-lost");
    await clickPart(A, "civ-ribbon.kick");
    await clickPart(A, "civ-ribbon.kick-yes");
    // The seat is the AI's now: it has no kick button, and (an AI the host has not met) it leaves the ribbon, whose
    // seats are all connected again; the turn waits for one human.
    await waitFor(A, (p) => texts(p, "civ-ribbon.kick").length === 0 && texts(p, "civ-ribbon.seat").every((t) => t !== "Connection lost" && t !== "Disconnected")
      && texts(p, "helm.state").some((t) => / of 1$/.test(t)), "alice: bob's seat is the AI's", 90000);
    check("the host's kick from the ribbon gives the seat to the AI", true);
    await shot(alice, "4-kicked");
    result.pass = result.checks.every((c) => c.pass);
  } catch (e) {
    result.error = String(e?.stack ?? e);
    console.log(`  FAIL ${e?.message ?? e}`);
    for (const b of pages) await shot(b, "error");
  } finally {
    for (const b of pages) await b.close().catch(() => {});
  }
  return result;
}
