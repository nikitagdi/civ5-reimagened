// `deploy-smoke` (net-test 4.5, T9; game-server 4.15 step 8): against a deployed service (`--attach <base>`):
//   - `/healthz` answers 200 with nothing left to restore; the web page, `/netcheck/` and the asset manifest load;
//   - the admin API is not reachable from outside (`/admin/metrics` on the public listener is not 200);
//   - https (a public host behind a reverse proxy): `http://` redirects to `https://`, the certificate is valid for a week more;
//   - the `/netcheck/ws` echo through the same path: the round trip from this machine (min, p50, p95);
//   - a short round over `/ws`: one bot logs in, hosts a room with an AI seat, starts and plays 3 turns (the `m1`
//     pattern); its hashes hold. With the admin route (`--admin-via docker:<context>:civ5`) the gateway's copy of the
//     view equals the bot's, and the game is closed afterwards (admin close: Finished, seats free, saves kept), so a smoke run leaves no game behind.
//   - whatever the outcome, the smoke game is purged at the end (admin POST purge: its saves, autosaves and record are
//     deleted), so the lobby's list of saved games does not collect `smoke<n>` entries.
// On https the bot goes through a local TLS front (lib/tls-front.mjs): `net-bot` speaks plain ws:// only.
import tls from "node:tls";
import { startTlsFront } from "../lib/tls-front.mjs";
import { botChecks, budgetFor, finishAll, gatewayChecks, gatewayGame, startGame } from "../lib/play.mjs";

export const meta = {
  id: "deploy-smoke",
  ac: [],
  needs: [],
  core: ["fixture", "real"],
  attach: "only",
  must: false,
  // See NO_LISTS below: reported as pending, not as a bot error.
  allowErrors: [/list of map scripts is empty|list of world sizes is empty|list of (game )?speeds is empty/],
  quick: { turns: 3 },
  full: { turns: 3 },
};

const NO_LISTS = /list of (map scripts|world sizes|(game )?speeds) is empty/;

const quantile = (xs, q) => {
  if (!xs.length) return null;
  const s = [...xs].sort((a, b) => a - b);
  return Math.round(s[Math.min(s.length - 1, Math.floor(q * s.length))] * 10) / 10;
};

async function get(url, init = {}) {
  try {
    const r = await fetch(url, { redirect: "manual", signal: AbortSignal.timeout(15000), ...init });
    return { status: r.status, headers: r.headers, text: await r.text() };
  } catch (e) {
    return { status: 0, headers: new Headers(), text: "", error: String(e.cause?.code ?? e.message ?? e) };
  }
}

/** The certificate the server presents for its name (verified against the system roots). */
function certificate(host, port = 443) {
  return new Promise((resolve) => {
    const s = tls.connect({ host, port, servername: host, timeout: 15000 }, () => {
      const c = s.getPeerCertificate();
      resolve({ authorized: s.authorized, error: s.authorizationError ? String(s.authorizationError) : null, validTo: c.valid_to, issuer: c.issuer?.O ?? c.issuer?.CN ?? "?" });
      s.end();
    });
    s.on("error", (e) => resolve({ authorized: false, error: String(e.code ?? e.message), validTo: null, issuer: null }));
    s.on("timeout", () => {
      s.destroy();
      resolve({ authorized: false, error: "timeout", validTo: null, issuer: null });
    });
  });
}

/** Round trips over `/netcheck/ws` (the gateway allows 2 messages a second: one every 600 ms). */
async function netcheckRtt(wsUrl, n) {
  const ws = new WebSocket(wsUrl);
  const rtts = [];
  const opened = await new Promise((resolve) => {
    ws.onopen = () => resolve(true);
    ws.onerror = () => resolve(false);
    setTimeout(() => resolve(false), 15000);
  });
  if (!opened) return { opened, rtts };
  let pending = null;
  ws.onmessage = (e) => {
    if (pending && String(e.data) === pending.id) {
      rtts.push(performance.now() - pending.t0);
      pending.done();
    }
  };
  for (let i = 0; i < n; i++) {
    await new Promise((resolve) => {
      pending = { id: `p${i}`, t0: performance.now(), done: resolve };
      ws.send(pending.id);
      setTimeout(resolve, 3000);
    });
    await new Promise((r) => setTimeout(r, 600));
  }
  ws.close();
  return { opened, rtts };
}

export async function run(ctx) {
  const { server, expect, opts } = ctx;
  const base = server.url.replace(/\/$/, "");
  const url = new URL(base);
  const https = url.protocol === "https:";
  const numbers = { base };

  // ---- HTTP
  const hz = await get(`${base}/healthz`);
  let health = null;
  try {
    health = JSON.parse(hz.text);
  } catch {
    // checked below
  }
  expect("/healthz answers 200", hz.status === 200, `${hz.status} ${hz.error ?? ""}`);
  expect("/healthz: nothing left to restore", health?.restoring === 0, hz.text.slice(0, 200));
  const page = await get(`${base}/`);
  expect("the web page loads (200, html)", page.status === 200 && /<html/i.test(page.text), `${page.status}, ${page.text.length} bytes`);
  const nc = await get(`${base}/netcheck/`);
  expect("/netcheck/ loads (200, html)", nc.status === 200 && /<html/i.test(nc.text), `${nc.status}`);
  const man = await get(`${base}/assets/manifest`);
  const manHash = man.text.trim();
  expect("/assets/manifest names a manifest blob", man.status === 200 && /^[0-9a-f]{64}$/.test(manHash), `${man.status} ${manHash.slice(0, 70)}`);
  if (/^[0-9a-f]{64}$/.test(manHash)) {
    const blob = await get(`${base}/assets/${manHash}`, { method: "HEAD" });
    expect("the manifest blob is served, immutable", blob.status === 200 && /immutable/.test(blob.headers.get("cache-control") ?? ""), `${blob.status} ${blob.headers.get("cache-control")}`);
  }
  const adm = await get(`${base}/admin/metrics`);
  expect("the admin API is not reachable from outside", adm.status !== 200, `${adm.status} ${adm.error ?? ""}`);

  // ---- TLS (public host)
  if (https) {
    const plain = await get(`http://${url.host}/healthz`);
    const loc = plain.headers.get("location") ?? "";
    expect("http:// redirects to https://", [301, 302, 307, 308].includes(plain.status) && loc.startsWith(`https://${url.hostname}`), `${plain.status} -> ${loc}`);
    const cert = await certificate(url.hostname, Number(url.port || 443));
    const daysLeft = cert.validTo ? Math.floor((Date.parse(cert.validTo) - Date.now()) / 86400000) : null;
    numbers.certificate = { issuer: cert.issuer, valid_to: cert.validTo, days_left: daysLeft };
    expect("the certificate is valid for the name", cert.authorized, cert.error ?? "");
    expect("the certificate is valid for 7 days more", daysLeft !== null && daysLeft >= 7, `${daysLeft} days`);
  }

  // ---- the netcheck echo: round trip from this machine over the same path as the game
  const wsBase = base.replace(/^http/, "ws");
  const echo = await netcheckRtt(`${wsBase}/netcheck/ws`, 12);
  numbers.netcheck_rtt_ms = { samples: echo.rtts.length, min: quantile(echo.rtts, 0), p50: quantile(echo.rtts, 0.5), p95: quantile(echo.rtts, 0.95) };
  expect("/netcheck/ws: the WebSocket opens and echoes", echo.opened && echo.rtts.length >= 10, `${echo.rtts.length} of 12 echoes`);

  // ---- a short round over /ws
  if (!ctx.tools.bot) {
    ctx.pending("the round over /ws: no net-bot (run through `cargo xtask nettest`, which builds it)");
    return { numbers };
  }
  const turns = opts.turns ?? 3;
  const front = https ? await startTlsFront({ url: base }) : null;
  // A nick of its own per run: an earlier smoke's identity may still hold its session.
  const names = [`smoke${Date.now() % 1000000}`];
  let gameId;
  try {
    const running = startGame(ctx, names, { turns, humans: 1, ai_slots: [1], url: front?.wsUrl, lobby_timeout_ms: 120000, turn_timeout_ms: 120000 });
    const budget = budgetFor(turns, 40000);
    const finished = await finishAll(running, budget);
    gameId = finished?.[0]?.result?.play?.game;
    if (!expect("the bot finished the round in time", finished !== null, `${budget / 1000} s budget`)) return { numbers };
    // The real core of main lists no world sizes, map scripts or speeds yet (game-host real.rs, gc-abi-v2 4.16):
    // no room settings pass the gateway's check, so the round cannot start. Said, not faked.
    const noLists = (finished[0].result?.errors ?? []).find((e) => NO_LISTS.test(e));
    if (noLists) {
      expect("the bot logged in over /ws and reached the lobby", !/rejected|connect/i.test(noLists), noLists);
      if (front) expect("the bot went through the TLS front without TLS errors", front.connections() >= 1 && front.tlsErrors.length === 0, `${front.connections()} connection(s) ${front.tlsErrors.join("; ")}`);
      ctx.pending(`the ${turns}-turn round: ${noLists} (the real core does not list world sizes, map scripts, speeds yet: gc-abi-v2 4.16)`);
      return { numbers };
    }
    const results = botChecks(expect, names, finished, turns);
    const r = results[0];
    if (front) expect("the bot went through the TLS front without TLS errors", front.connections() >= 1 && front.tlsErrors.length === 0, `${front.connections()} connection(s) ${front.tlsErrors.join("; ")}`);
    numbers.round = { game: r.play?.game, turns, orders: r.cmds?.unique, applied: r.cmds?.applied, rtt_ms_p50: r.rtt_median, latency_ms: { p50: r.latency_ms?.p50, p95: r.latency_ms?.p95 } };
    if (ctx.caps.admin && server.admin && r.play?.game !== undefined) {
      const g = await gatewayGame(ctx, r.play.game, results);
      if (expect("the gateway knows the game", !!g, `game ${r.play.game}`)) gatewayChecks(expect, g, results, names);
      const close = await server.admin.close(r.play.game);
      expect("the smoke game is closed afterwards (admin POST close)", close.ok, JSON.stringify(close.body ?? "").slice(0, 200));
    } else {
      ctx.pending("gateway checks and the clean-up of the smoke game: no admin route (--admin-via docker:<context>:civ5); the server closes it 5 minutes after the bot left");
    }
  } finally {
    await front?.stop();
    await purgeSmokeGame(ctx, server, names[0], gameId, expect);
  }
  return { numbers };
}

/** The smoke game and its saves are deleted, pass or fail (a failed round may not know its game number: the open
 * games of the nick are looked up). Without the admin route nothing can be deleted; the run says so. */
async function purgeSmokeGame(ctx, server, nick, gameId, expect) {
  if (!(ctx.caps.admin && server.admin)) return;
  const ids = new Set(gameId === undefined ? [] : [gameId]);
  try {
    for (const g of (await server.admin.games()).games ?? []) if (g.host === nick) ids.add(g.id);
  } catch {
    // the id from the bot's report is all there is
  }
  for (const id of ids) {
    const r = await server.admin.purge(id);
    expect(`the smoke game ${id} and its saves are deleted (admin POST purge)`, r.ok, JSON.stringify(r.body ?? "").slice(0, 200));
  }
}
