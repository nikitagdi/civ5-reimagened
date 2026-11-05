// `fatal` (playtest §3.2, task H7; AC-9.5): a fatal error of the game shows the error screen, not a white page, the
// error is in the console, and Reload brings the game back. On the server path (the client has no local game host,
// net-client NC9) the faults are the ones a session meets:
// - `catalogError`: the game catalog of the server answers HTTP 500 (the shell retries five times, then "Cannot load
//   the game catalog" is fatal);
// - `catalogBytes`: the catalog comes with bytes that are no catalog (its hash does not match: the same fatal);
// - `badMessage`: a frame that is no `ClientMsg` goes to the server, which drops the connection. That is no fatal
//   error: the client reconnects by itself (net-client 4.12) and the game plays on; there must be no error screen.
// Reload is the error screen's action (`UiAction::Reload`, the page reloads); with the catalog healthy again the
// game comes back from the server (the seat is still ours).
import { openPage, state, act, until } from "../lib/browser.mjs";
import { startServerGame, signInAndWait } from "../lib/game.mjs";
import { screenStats } from "../lib/png.mjs";

const CATALOG = "**/catalog/*";

const CASES = [
  { fault: "catalogError", route: (r) => r.fulfill({ status: 500, body: "boom" }), expect: /catalog/i },
  { fault: "catalogBytes", route: (r) => r.fulfill({ status: 200, contentType: "application/octet-stream", body: "not a catalog" }), expect: /catalog/i },
];

/** Reload through the test port: the page navigates, so no `act` wait for `applied`. */
async function reloadFromErrorScreen(page) {
  const loaded = page.waitForEvent("load", { timeout: 60000 });
  await page.evaluate(() => window.__civ5test.act({ kind: "reload" })).catch(() => {});
  await loaded;
}

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "fatal", pass: false, logs: o.logDir, checks: [], numbers: { cases: [] } };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const b = await openPage({ headed: o.headed, logDir: o.logDir });
  const { page } = b;
  let error = null;
  try {
    for (const c of CASES) {
      const nick = `fatal-${c.fault}`.toLowerCase();
      const errorsBefore = b.errors.length;
      const linesBefore = b.console.length;
      await page.route(CATALOG, c.route);
      await page.goto(`${ctx.server.url}/?${new URLSearchParams({ test: "1", ui: "interim", nick, autostart: "1", ai: String(o.ai), size: "duel", seed: String(o.seed) })}`);
      // The sign-in first (the dev server wants its password once), then the fatal after the retries.
      const login = await until(page, (s) => s.screen === "login" || s.screen === "fatal", o.lobbyTimeoutMs, `sign-in window before ${c.fault}`);
      if (login.screen === "login") await act(page, { kind: "login", nick, password: "dev" });
      const shown = await until(page, (s) => s.screen === "fatal", 90000, `error screen after ${c.fault}`).catch((e) => (e.fatal ? state(page) : Promise.reject(e)));
      // Let the error screen draw a few frames, then look at it.
      await page.waitForTimeout(500);
      const png = await page.screenshot({ path: `${o.logDir}/fatal-${c.fault}.png` });
      const screen = screenStats(png);
      const consoleLines = b.errors.slice(errorsBefore);
      // The console says why: the failed request (an error line) or the shell's warning about the answer it got.
      const said = b.console.slice(linesBefore).filter((l) => (l.type === "error" || l.type === "warning") && /catalog/i.test(l.text)).map((l) => l.text);
      const text = shown?.fatal ?? "";
      check(`${c.fault}: error screen with the message`, text.length > 0 && c.expect.test(text), text);
      check(`${c.fault}: not a blank page`, screen.whiteShare < 0.9 && screen.colours >= 8 && screen.dominantShare < 0.97, JSON.stringify(screen));
      check(`${c.fault}: the error is in the console`, said.length > 0, said.slice(0, 2).join(" | ") || "no console error or warning");
      // The catalog is healthy again; Reload brings the game back.
      await page.unroute(CATALOG);
      await reloadFromErrorScreen(page);
      const back = await signInAndWait(page, ctx, nick);
      check(`${c.fault}: Reload brings the game back`, back.screen === "play" && !back.fatal, `turn ${back.turn}, frames ${back.frames}`);
      result.numbers.cases.push({ fault: c.fault, turn: back.turn, message: text, screen, consoleErrors: consoleLines.length });
    }

    // A dropped connection is no fatal error: the game plays on.
    {
      const nick = "fatal-badmessage";
      const playing = await startServerGame(page, ctx, { nick });
      const breaks0 = (await page.evaluate(() => window.__civ5test.metrics())).client?.reconnects ?? null;
      await act(page, { kind: "fault", fault: "badMessage" });
      // Quiet for a moment, then an order of the player must go through on the new connection.
      await page.waitForTimeout(1500);
      const now = await state(page);
      check("badMessage: no error screen, the client stays in the game", now.screen !== "fatal" && !now.fatal, `screen ${now.screen}${now.fatal ? `, fatal ${now.fatal}` : ""}`);
      await act(page, { kind: "endTurn", force: true });
      const next = await until(page, (s) => s.screen === "play" && s.status === "human" && s.turn === playing.turn + 1, 60000, "the next turn after badMessage");
      check("badMessage: the game plays on after the reconnect", next.turn === playing.turn + 1, `turn ${playing.turn} -> ${next.turn}; ${next.net?.conn}`);
      result.numbers.cases.push({ fault: "badMessage", turn: next.turn, reconnectsBefore: breaks0 });
    }

    // A panic (B2 huge-black: the frame panicked and the page stayed black): the crash overlay says so.
    {
      const errorsBefore = b.errors.length;
      await page.evaluate(() => window.__civ5test.act({ kind: "panic" })).catch(() => {});
      await page.waitForTimeout(500);
      const overlay = await page.evaluate(() => document.getElementById("civ5-crash")?.textContent ?? null);
      await page.screenshot({ path: `${o.logDir}/fatal-panic.png` }).catch(() => {});
      check("panic: the crash overlay shows the panic and a Reload", !!overlay && /panic/i.test(overlay) && /Reload/.test(overlay), overlay ?? "no overlay");
      // The panic is expected here: not an error of the run.
      b.errors.splice(errorsBefore);
      result.numbers.cases.push({ fault: "panic", overlay: !!overlay });
    }
  } catch (e) {
    error = e;
  }
  const last = await state(page).catch(() => null);
  const uncaught = b.errors.filter((l) => /uncaught|page crashed/.test(l));
  check("no uncaught page error or crash", uncaught.length === 0, uncaught.slice(0, 3).join(" | ") || "none");
  check("no scenario error", !error, error ? error.message : "");
  if (error) result.error = error.stack ?? String(error);
  result.consoleErrors = b.errors.slice();
  result.lastScreen = last?.screen ?? null;
  result.pass = result.checks.every((c) => c.pass);
  result.durationMs = Date.now() - started;
  if (o.screenshot) await page.screenshot({ path: `${o.logDir}/last.png` }).catch(() => {});
  await b.close();
  return result;
}
