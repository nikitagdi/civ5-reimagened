// `saveload` (playtest §3.2, task H7; AC-8.1, AC-8.2, AC-9.4): the saves of the server, as a player meets them. The
// client has no saves of its own any more (net-client NC9): the server saves each turn (the ring) and when the game is
// stopped; a stopped game sleeps, and a seat of it ([Take] in the lobby) wakes it from the newest usable save.
// A game through the server (`?autostart=1`); K turns by the policy (city founded, production chosen); at the start of
// turn T the server's ring holds the save of T (SIMP-019, SIMP-026: the save point is the start of the human's turn,
// before any order) and the visible state is recorded; the administrator saves and stops the game (admin API, the
// player is told); the lobby lists the sleeping game and its save; [Take] wakes it: the same turn, digest and view
// hash; M more turns without errors.
// Then the damaged saves: the saves of the newest turn are damaged on disk; waking takes the older turn. Last no
// usable save at all: the game stays asleep, the player stays in the lobby, nothing is fatal.
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { openPage, state, act, until, settle } from "../lib/browser.mjs";
import { startServerGame } from "../lib/game.mjs";
import { createPlayer } from "../lib/policy.mjs";
import { JournalLog } from "../lib/journal.mjs";

/** The part of the state a player sees, for the before/after comparison. */
function visible(s) {
  return {
    turn: s.turn,
    year: s.year,
    units: s.units,
    cities: s.cities,
    rivalUnits: s.rivalUnits,
    rivalCities: s.rivalCities,
    players: s.players,
  };
}

/** The fields of `a` and `b` that differ (top level; units and cities by id). */
function differences(a, b) {
  const out = [];
  for (const k of Object.keys(a)) {
    if (JSON.stringify(a[k]) === JSON.stringify(b[k])) continue;
    if (Array.isArray(a[k])) {
      const byId = new Map((b[k] ?? []).map((x) => [x.id, x]));
      for (const x of a[k]) {
        const y = byId.get(x.id);
        if (JSON.stringify(x) !== JSON.stringify(y)) out.push(`${k} ${x.id}: ${JSON.stringify(x)} -> ${JSON.stringify(y ?? null)}`);
      }
      if ((b[k] ?? []).length !== a[k].length) out.push(`${k}: ${a[k].length} -> ${(b[k] ?? []).length}`);
    } else out.push(`${k}: ${JSON.stringify(a[k])} -> ${JSON.stringify(b[k])}`);
  }
  return out;
}

/** The `.c5sv` files of a game in the server's state dir, with the turn the name tells (`<seq>-t<turn>.c5sv`). */
function saveFiles(stateDir, game) {
  const out = [];
  const walk = (dir) => {
    for (const e of readdirSync(dir, { withFileTypes: true })) {
      const f = path.join(dir, e.name);
      if (e.isDirectory()) walk(f);
      else {
        const m = /^(\d+)-t(\d+)\.c5sv$/.exec(e.name);
        if (m) out.push({ file: f, seq: Number(m[1]), turn: Number(m[2]) });
      }
    }
  };
  walk(path.join(stateDir, "games", String(game)));
  return out;
}

/** Flips the last byte: the checksum of the payload no longer matches. */
function damage(file) {
  const bytes = readFileSync(file);
  bytes[bytes.length - 1] ^= 0xff;
  writeFileSync(file, bytes);
}

export async function run(ctx) {
  const o = ctx.opts;
  const K = o.saveTurns;
  const M = o.afterLoadTurns;
  const started = Date.now();
  const result = { scenario: "saveload", pass: false, logs: o.logDir, checks: [], numbers: {}, steps: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const step = (what, data) => {
    result.steps.push({ at: Date.now() - started, what, ...(data ?? {}) });
    if (o.verbose) console.log(`  [saveload] ${what}${data ? ` ${JSON.stringify(data)}` : ""}`);
  };
  const b = await openPage({ headed: o.headed, logDir: o.logDir });
  const { page } = b;
  const journal = new JournalLog();
  const player = createPlayer(page, { turnTimeoutMs: o.turnTimeoutMs, verbose: o.verbose, observe: (s) => journal.add(s) });
  const server = ctx.server;
  const admin = async (method, route) => {
    const r = await server.admin(method, route);
    if (!r.ok) throw new Error(`admin ${method} ${route}: HTTP ${r.status}`);
    return r.body;
  };
  const digestNow = () => page.evaluate(() => window.__civ5test.digest());
  /** Stops the game (the server saves first), tells the player's notice away and waits for the lobby. */
  const stopToLobby = async (game) => {
    await admin("POST", `/admin/games/${game}/stop`);
    // The saves of the stop are written before the game sleeps: wait for that before looking at the files.
    const t0 = Date.now();
    while (((await admin("GET", "/admin/games")).games ?? []).find((g) => g.id === game)?.status === "Running" && Date.now() - t0 < 30000) await new Promise((r) => setTimeout(r, 100));
    await until(page, (s) => s.screen === "notice" || s.screen === "lobby", 30000, "the notice of the stopped game");
    const s = await state(page);
    if (s.screen === "notice") await act(page, { kind: "dismissNotice" });
    return until(page, (x) => x.screen === "lobby", 30000, "lobby after the stop");
  };
  const take = async (game, seat) => {
    await act(page, { kind: "takeSeat", game, player: seat });
    return until(page, (s) => s.screen === "play" && s.status === "human", o.mapTimeoutMs, "map after [Take]");
  };
  let error = null;
  let before = null;
  let after = null;
  let last = null;
  try {
    const nick = "saveload";
    check("no game and no save before the first game", ((await admin("GET", "/admin/games")).games ?? []).length === 0, "the server starts empty");
    const first = await startServerGame(page, ctx, { nick });
    step("map", { turn: first.turn });
    const game = ((await admin("GET", "/admin/games")).games ?? [])[0]?.id;
    const seat = first.net.seats.find((x) => x.who === nick)?.player ?? 0;

    // K turns, then the start of turn T and its save.
    const T = first.turn + K;
    const atT = await player.play(T);
    journal.add(atT);
    if (atT.turn !== T) throw new Error(`the game did not reach turn ${T} (turn ${atT.turn}, status ${atT.status})`);
    const savedAtT = async () => ((await admin("GET", `/admin/games/${game}`)).saves ?? []).filter((s) => s.turn === T);
    const t0 = Date.now();
    while (!(await savedAtT()).length && Date.now() - t0 < 60000) await new Promise((r) => setTimeout(r, 100));
    const ring = (await admin("GET", `/admin/games/${game}`)).saves ?? [];
    const turnsSaved = [...new Set(ring.map((s) => s.turn))];
    check("the server saved the start of turn T (autosave each turn)", (await savedAtT()).length > 0 && turnsSaved.length >= Math.min(K, 8), `turns in the ring: ${turnsSaved.sort((x, y) => x - y).join(",")}`);
    before = { ...visible(await state(page)), digest: await digestNow(), viewHash: (await state(page)).net.viewHash };
    step("before the stop", { turn: before.turn, digest: before.digest, units: before.units.length, cities: before.cities.length });
    check(`played ${K} turns with a city and production`, before.cities.length > 0 && before.cities.every((c) => c.productionItem >= 0), `${before.cities.length} cities, builds ${before.cities.map((c) => c.productionItem).join(",")}, policy ${JSON.stringify(player.policy)}`);

    // The administrator saves and stops the game: the player is told, the lobby shows the sleeping game and its save.
    const t1 = Date.now();
    await admin("POST", `/admin/games/${game}/save`);
    const saveMs = Date.now() - t1;
    const lobby1 = await stopToLobby(game);
    const row = lobby1.net.games.find((g) => g.id === game);
    const mine = row?.seats.find((x) => x.player === seat);
    check("the lobby offers [Take] of the stopped game at turn T", !!row && row.turn === T && !!mine && mine.takeable === true && mine.yours === true, row ? `${row.status}, turn ${row.turn}, seats ${JSON.stringify(row.seats)}` : "no game row");
    check("the lobby lists the save of turn T", lobby1.net.saves.some((s) => s.turn === T), lobby1.net.saves.map((s) => `${s.id} turn ${s.turn}`).join("; ") || "no saves listed");
    check("no save error in the lobby", !lobby1.net.lobbyError || /seat in a game/.test(lobby1.net.lobbyError), lobby1.net.lobbyError ?? "");

    // No usable save at all (the files are damaged for a moment, then put back): the game stays asleep, the player in
    // the lobby, nothing is fatal.
    const originals = saveFiles(server.stateDir, game).map((f) => ({ ...f, bytes: readFileSync(f.file) }));
    for (const f of originals) damage(f.file);
    await act(page, { kind: "takeSeat", game, player: seat });
    const stay = await settle(page, (s) => s.screen === "play", 6000);
    const row3 = (await admin("GET", "/admin/games")).games?.find((g) => g.id === game);
    check("no usable save: the game stays asleep", stay?.screen !== "play" && row3?.status !== "Running", `screen ${stay?.screen}, game ${row3?.status}, ${originals.length} saves damaged`);
    check("no usable save: nothing is fatal, the player stays in the lobby", stay?.screen === "lobby" && !stay?.fatal, `screen ${stay?.screen}${stay?.net?.lobbyError ? `, ${stay.net.lobbyError}` : ""}`);
    for (const f of originals) writeFileSync(f.file, f.bytes);

    // [Take]: the game wakes from its save.
    const t2 = Date.now();
    const loaded = await take(game, seat);
    const continueMs = Date.now() - t2;
    after = { ...visible(loaded), digest: await digestNow(), viewHash: loaded.net.viewHash };
    step("after [Take]", { turn: after.turn, digest: after.digest });
    check("same turn after [Take]", after.turn === before.turn, `${before.turn} -> ${after.turn}`);
    check("same digest after [Take]", after.digest === before.digest, `${before.digest} -> ${after.digest}`);
    check("same view hash after [Take]", after.viewHash === before.viewHash, `${before.viewHash} -> ${after.viewHash}`);
    const diff = differences(before, after);
    check("same visible units, cities, rivals after [Take]", diff.length === 0, diff.length ? diff.slice(0, 5).join("; ") : "equal");

    // M more turns.
    last = await player.play(T + M);
    journal.add(last);
    const playedAfter = last.turn - T;
    check("turns after the load", last.turn >= T + M || last.status === "over", `${playedAfter} of ${M} turns (turn ${last.turn})`);
    check("no stuck turn after the load", !player.stuck, player.stuck ? JSON.stringify(player.stuck) : "");

    // Damaged saves: those of the newest turn; waking takes the older turn.
    const newestTurn = last.turn;
    await stopToLobby(game);
    const files = saveFiles(server.stateDir, game);
    const bad = files.filter((f) => f.turn === newestTurn);
    const olderTurn = Math.max(...files.filter((f) => f.turn < newestTurn).map((f) => f.turn));
    for (const f of bad) damage(f.file);
    step("damaged", { files: bad.length, newestTurn, olderTurn });
    const fallback = await take(game, seat);
    check("damaged save: the older turn is loaded", bad.length > 0 && fallback.turn === olderTurn, `turn ${fallback.turn}, older turn ${olderTurn}, ${bad.length} saves of turn ${newestTurn} damaged`);
    check("damaged save: no fatal error, the game plays", !fallback.fatal, `turn ${fallback.turn}`);

    last = fallback;

    result.numbers = {
      seed: o.seed,
      rivals: o.ai,
      savedTurn: T,
      digest: before.digest,
      unitsAtSave: before.units.length,
      citiesAtSave: before.cities.length,
      turnsAfterLoad: playedAfter,
      savedTurns: turnsSaved.length,
      adminSaveMs: saveMs,
      continueMs,
      damagedFiles: bad.length,
      policy: player.policy,
      journal: journal.summary(),
    };
  } catch (e) {
    error = e;
  }
  try {
    last = (await state(page)) ?? last;
  } catch {
    /* gone */
  }
  const fatal = error?.fatal ?? last?.fatal ?? null;
  result.consoleErrors = b.errors.slice();
  check("no fatal error", !fatal, fatal ?? "");
  check("no console errors", b.errors.length === 0, `${b.errors.length} error(s)${b.errors[0] ? `: ${b.errors[0]}` : ""}`);
  check("no scenario error", !error || !!fatal, error && !fatal ? error.message : "");
  if (error) result.error = error.stack ?? String(error);
  result.before = before;
  result.after = after;
  result.journalLines = journal.lines();
  result.pass = result.checks.every((c) => c.pass);
  result.durationMs = Date.now() - started;
  if (o.screenshot) await page.screenshot({ path: `${o.logDir}/last.png` }).catch(() => {});
  await b.close();
  return result;
}
