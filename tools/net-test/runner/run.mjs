// Net-test runner: starts the server under test, runs the
// scenarios, checks the conditions, writes `report.json` and `report.md` (the B1 format).
//
//   node tools/net-test/runner/run.mjs --server <civ5-server> [--bot <net-bot>] [--netproxy <netproxy>] [--gamedata-dir <dir>]
//        --scenario <id|a,b|quick|all> [--full|--quick] [--profile win|wsl] [--core fixture|real]
//        [--link none|proxy|netem] [--attach URL [--admin-via docker:<context>:<container>]
//        [--password-file F]] [--acceptance] [--seed N] [--out target/net-test] [--keep-state]
//        [--turns N] [--bots N] [--web-root <dir> --assets <dir>] [--bg-minutes N] [--verbose] [--list]
//
// Usually started as `cargo xtask nettest ...`, which builds the binaries and fills `--server`, `--bot`.
// Exit code 0 only when no scenario failed (and, with `--acceptance`, none was skipped).
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { createAdmin } from "./lib/admin.mjs";
import { AC_LIST, acCounts, acTable } from "./lib/ac.mjs";
import { hostFor } from "./lib/host.mjs";
import { createBots } from "./lib/bots.mjs";
import { bigintReplacer } from "./lib/json.mjs";
import { statusText, writeReport } from "./lib/report.mjs";
import { startProxyLink } from "./lib/link.mjs";
import { attachServer, startServer } from "./lib/server.mjs";
import { th, thText, thv } from "./lib/thresholds.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, "../../..");
const QUICK_SET = ["calibrate", "m1"];

function parseArgs(argv) {
  const o = {
    scenario: "m1",
    preset: "full",
    profile: "win",
    core: "fixture",
    link: "none",
    attach: null,
    adminVia: null,
    passwordFile: null,
    acceptance: false,
    seed: null,
    out: "target/net-test",
    keepState: false,
    verbose: false,
    server: null,
    bot: null,
    netproxy: null,
    gamedata: null,
    turns: null,
    bots: null,
    webRoot: null,
    assets: null,
    bgMinutes: null,
    list: false,
  };
  const num = (k, v) => {
    const n = Number(v);
    if (!Number.isFinite(n)) throw new Error(`${k} needs a number, got ${v}`);
    return n;
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const [k, inline] = a.includes("=") ? a.split(/=(.*)/s) : [a, undefined];
    const val = () => inline ?? argv[++i];
    switch (k) {
      case "--scenario": o.scenario = val(); break;
      case "--quick": o.preset = "quick"; break;
      case "--full": o.preset = "full"; break;
      case "--profile": o.profile = val(); break;
      case "--core": o.core = val(); break;
      case "--link": o.link = val(); break;
      case "--attach": o.attach = val(); break;
      case "--admin-via": o.adminVia = val(); break;
      case "--password-file": o.passwordFile = val(); break;
      case "--acceptance": o.acceptance = true; break;
      case "--seed": o.seed = num(k, val()); break;
      case "--out": o.out = val(); break;
      case "--keep-state": o.keepState = true; break;
      case "--verbose": o.verbose = true; break;
      case "--server": o.server = val(); break;
      case "--bot": o.bot = val(); break;
      case "--netproxy": o.netproxy = val(); break;
      case "--gamedata-dir": o.gamedata = val(); break;
      case "--turns": o.turns = num(k, val()); break;
      case "--bots": o.bots = num(k, val()); break;
      case "--web-root": o.webRoot = val(); break;
      case "--assets": o.assets = val(); break;
      case "--bg-minutes": o.bgMinutes = num(k, val()); break;
      case "--list": o.list = true; break;
      default: throw new Error(`unknown argument ${a}`);
    }
  }
  if (!["fixture", "real"].includes(o.core)) throw new Error(`--core ${o.core}: expected fixture or real`);
  if (!["none", "proxy", "netem"].includes(o.link)) throw new Error(`--link ${o.link}: expected none, proxy or netem`);
  if (o.seed === null) o.seed = Math.floor(Date.now() / 1000) % 100000;
  return o;
}

async function loadScenarios() {
  const dir = path.join(here, "scenarios");
  const out = new Map();
  for (const f of readdirSync(dir).filter((f) => f.endsWith(".mjs")).sort()) {
    const mod = await import(pathToFileURL(path.join(dir, f)).href);
    if (!mod.meta?.id || typeof mod.run !== "function") throw new Error(`${f}: a scenario exports \`meta\` and \`run(ctx)\``);
    out.set(mod.meta.id, mod);
  }
  return out;
}

function selection(spec, all) {
  const ids = [...all.keys()];
  if (spec === "all") return ids.filter((i) => i !== "selftest");
  if (spec === "quick") return ids.filter((i) => QUICK_SET.includes(i));
  const names = spec.split(",").map((s) => s.trim()).filter(Boolean);
  for (const n of names) if (!all.has(n)) throw new Error(`unknown scenario ${n} (have: ${ids.join(", ")}, quick, all)`);
  return names;
}

async function detectCapabilities(o, profile, host) {
  const probes = {
    browser: () => existsSync(path.join(root, "tools/playtest/node_modules/playwright/package.json")),
    // The browser scenarios need the web client: a bundle of ours served by the server, or the deployed one.
    web: () => !!o.attach || (!!o.webRoot && existsSync(path.join(o.webRoot, "index.html"))),
    // NETTEST_HEADLESS=1: no windows on this machine's screen (the scenarios that need one are skipped).
    headed: async (caps) => caps.browser && profile.host === "local" && process.platform === "win32" && !process.env.NETTEST_HEADLESS,
    docker: async () => (await hostFor("local").exec("docker", ["--version"], { timeoutMs: 10000 })).code === 0,
    proxy: () => !!o.netproxy && existsSync(o.netproxy),
    netem: async () => {
      if (profile.host !== "wsl") return false;
      const r = await host.exec("bash", ["-lc", "test $(id -u) = 0 && tc -V >/dev/null 2>&1"], { timeoutMs: 20000 });
      return r.code === 0;
    },
  };
  const caps = {};
  for (const [name, v] of Object.entries(profile.capabilities)) {
    caps[name] = v === "probe" ? !!(await probes[name](caps)) : !!v;
  }
  // Signals (SIGSTOP), /proc: the processes of the stand live on Linux.
  caps.linux = host.os !== "win32";
  if (o.attach) {
    // A deployed server has no test hooks; its admin API is only reachable through the docker exec route.
    caps.hooks = false;
    caps.web = true;
    caps.admin = !!o.adminVia;
    caps["core-real"] = true;
  }
  if (o.link === "netem" && !caps.netem) caps.netem = false;
  return caps;
}

/** The game data of the real core (`civ5-assets export-gamedata`): `--gamedata-dir`, ${NETTEST_GAMEDATA}, or `assets-out/gamedata-b2` of the workspace. */
function findGamedata(o) {
  if (o.core !== "real") return undefined;
  const cands = [o.gamedata, process.env.NETTEST_GAMEDATA];
  for (let d = root, i = 0; i < 4; i++, d = path.dirname(d)) cands.push(path.join(d, "assets-out", "gamedata-b2"));
  const found = cands.find((c) => c && existsSync(path.join(c, "manifest.sha256")));
  if (!found) throw new Error("core real needs the game data: --gamedata-dir <dir> (or NETTEST_GAMEDATA, or assets-out/gamedata-b2 in the workspace)");
  return path.resolve(found);
}

function gitCommit() {
  try {
    return execFileSync("git", ["rev-parse", "--short", "HEAD"], { cwd: root, encoding: "utf8" }).trim();
  } catch {
    return "unknown";
  }
}

/** The counters worth printing next to a scenario (before and after). */
function pickCounters(m) {
  if (!m) return null;
  const games = {};
  for (const [id, g] of Object.entries(m.games ?? {})) {
    games[id] = {
      status: g.status,
      turn: g.turn,
      cmds: g.cmds,
      views: g.views,
      worker: g.worker,
      fog_violations: g.fog_violations,
      seats: Object.fromEntries(Object.entries(g.seats ?? {}).map(([p, s]) => [p, { conn: s.conn, view_hash: s.view_hash, tx: s.tx, resume: s.resume }])),
    };
  }
  return { sessions: m.server?.sessions, games };
}

function makeCtx({ o, caps, host, server, bots, logDir, params, checks, links, extras }) {
  const expect = (name, cond, detail = "") => {
    checks.push({ name, pass: !!cond, detail: String(detail) });
    if (o.verbose || !cond) console.log(`  ${cond ? "ok  " : "FAIL"} ${name}${detail ? `: ${detail}` : ""}`);
    return !!cond;
  };
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  // A part of the scenario that waits for a feature main does not have yet: said in the report, never faked.
  const pendingList = [];
  const pending = (what) => {
    pendingList.push(what);
    console.log(`  PENDING ${what}`);
  };
  // A channel in front of the server (needs the capability `proxy`): the bots connect to `link.wsUrl`.
  const proxyLink = async () => {
    if (!caps.proxy) throw new Error("no netproxy: the scenario needs the capability proxy");
    if (!server || server.attached) throw new Error("the proxy link needs a server the runner started");
    const link = await startProxyLink({ host, netproxy: o.netproxy, server, seed: o.seed });
    links.push(link);
    return link;
  };
  // A second server of its own config (a control run: other test hooks), with its own bots. Stopped by the runner.
  const extraServer = async (name, config = {}) => {
    if (!server || server.attached) throw new Error("extraServer needs a server the runner started");
    const dir = path.join(logDir, name);
    mkdirSync(dir, { recursive: true });
    rmSync(path.join(dir, "bots"), { recursive: true, force: true });
    const s = await startServer({ host, exe: o.server, core: o.core, logDir: dir, gamedataDir: findGamedata(o), ...config });
    const b = createBots({ host, bot: o.bot, server: s, logDir: dir, seed: o.seed, verbose: o.verbose });
    extras.push({ server: s, bots: b });
    return { server: s, bots: b, logDir: dir };
  };
  const sample = (everyMs, fn) => {
    const values = [];
    const timer = setInterval(async () => {
      try {
        values.push(await fn());
      } catch (e) {
        values.push({ error: String(e) });
      }
    }, everyMs);
    return { values, stop: () => clearInterval(timer) };
  };
  return { root, server, bots, host, tools: { netproxy: o.netproxy, bot: o.bot }, link: null, browser: null, th: { get: th, value: thv, text: thText }, opts: { ...params, ...(o.bgMinutes !== null ? { bgMinutes: o.bgMinutes } : {}), seed: o.seed, preset: o.preset, core: o.core, link: o.link, verbose: o.verbose }, caps, logDir, sleep, sample, expect, pending, pendingList, proxyLink, extraServer };
}

async function runScenario(mod, o, caps, host, envInfo) {
  const meta = mod.meta;
  const logDir = path.join(path.resolve(o.out), "logs", meta.id);
  mkdirSync(logDir, { recursive: true });
  // Tokens of an earlier run belong to the identities of an earlier server: not reused.
  rmSync(path.join(logDir, "bots"), { recursive: true, force: true });
  const base = { id: meta.id, logs: logDir, checks: [], numbers: {}, durationMs: 0 };
  const skip = (why) => ({ ...base, status: "skip", skip: why });

  const missing = (meta.needs ?? []).filter((n) => !caps[n]);
  if (missing.length) return skip(`needs ${missing.join(", ")}`);
  if (meta.core && !meta.core.includes(o.core)) return skip(`core ${o.core} not supported (${meta.core.join(", ")})`);
  if (o.attach && meta.attach === false) return skip("not for --attach");
  if (!o.attach && meta.attach === "only") return skip("only for --attach (a deployed service)");

  const params = { ...(meta[o.preset] ?? {}) };
  // The server config of the scenario: `meta.serverConfig`, over it `meta.<preset>.serverConfig` (the full preset keeps
  // the real limits, the quick one shortens them).
  const serverConfig = { ...(meta.serverConfig ?? {}), ...(params.serverConfig ?? {}) };
  delete params.serverConfig;
  if (o.turns !== null) params.turns = o.turns;
  if (o.bots !== null) params.bots = o.bots;

  const checks = [];
  const t0 = Date.now();
  let server = null;
  let bots = null;
  let before = null;
  // The games of a deployed server that were there before the scenario: the others are the scenario's and are closed.
  let gamesBefore = null;
  let result = {};
  let error = null;
  let pending = null;
  let ctxRef = null;
  const links = [];
  const extras = [];
  try {
    if (meta.server !== false) {
      if (o.attach) {
        const password = existsSync(o.passwordFile ?? "") ? readFileSync(o.passwordFile, "utf8").trim() : (process.env.NETTEST_PASSWORD ?? "");
        let admin = null;
        if (o.adminVia) {
          const [kind, context, container] = o.adminVia.split(":");
          if (kind !== "docker") throw new Error(`--admin-via ${o.adminVia}: expected docker:<context>:<container>`);
          admin = createAdmin({ via: "docker", context, container, host: hostFor("local") });
        }
        server = attachServer({ url: o.attach, admin, password });
      } else {
        server = await startServer({
          host, exe: o.server, core: o.core, logDir, keepState: o.keepState, gamedataDir: findGamedata(o),
          webRoot: o.webRoot ? path.resolve(o.webRoot) : undefined, assetsRoot: o.assets ? path.resolve(o.assets) : undefined,
          ...serverConfig,
        });
      }
      console.log(`server ${server.url}${server.attached ? " (attached)" : ` (admin :${server.adminPort})`}`);
      if (caps.admin && server.admin) {
        if (server.attached) gamesBefore = new Set(((await server.admin.games().catch(() => null))?.games ?? []).map((g) => g.id));
        before = await server.admin.metrics().catch(() => null);
        if (before && !envInfo.serverBuild) {
          envInfo.serverBuild = before.build ?? "unknown";
          envInfo.coreBuild = before.core_build ?? "unknown";
        }
      }
    }
    bots = createBots({ host, bot: o.bot, server, logDir, seed: o.seed, verbose: o.verbose });
    const ctx = makeCtx({ o, caps, host, server, bots, logDir, params, checks, links, extras });
    ctxRef = ctx;
    result = (await mod.run(ctx)) ?? {};

    // The common checks of 4.4 (the bots first: their results are what is checked).
    await bots.stopAll();
    if (meta.commonChecks !== false && server) {
      const after = caps.admin && server.admin ? await server.admin.metrics().catch(() => null) : null;
      if (!server.attached) {
        ctx.expect("common: the server is alive and was not restarted", server.alive() && !server.unrequestedExit && server.starts === 1 + (ctx.restarts ?? 0), `starts ${server.starts}`);
        const errors = server.log().split("\n").filter((l) => /\bERROR\b/.test(l));
        ctx.expect("common: no ERROR in the gateway log", errors.length === 0, errors[0] ?? "");
      }
      if (after) {
        const fog = Object.values(after.games ?? {}).reduce((s, g) => s + (g.fog_violations ?? 0), 0);
        ctx.expect("common: gateway fog_violations is 0", fog === 0, `${fog}`);
      }
      for (const r of bots.results()) {
        const tag = `common: bot ${r.nick}`;
        const unexpected = (r.errors ?? []).filter((e) => !(meta.allowErrors ?? []).some((re) => re.test(e)));
        ctx.expect(`${tag} has no errors`, unexpected.length === 0, unexpected.slice(0, 3).join("; "));
        const dbl = meta.allowDoubleOutcome ?? 0;
        ctx.expect(`${tag} ledger.double_outcome is ${dbl}`, (r.ledger?.double_outcome ?? 0) <= dbl, `${r.ledger?.double_outcome}`);
        if (!meta.hashMismatchAllowed) ctx.expect(`${tag} hash.mismatch is 0`, r.hash?.mismatch === 0, `${r.hash?.mismatch}`);
        ctx.expect(`${tag} fog.violations is 0`, r.fog?.violations === 0, `${r.fog?.violations}`);
      }
      result.counters = { before: pickCounters(before), after: pickCounters(after) };
    }
  } catch (e) {
    // A feature of the game core that main does not have yet: the scenario is pending, not failed, and says so.
    if (e.describeFailed && /not built into this binary/.test(e.describeFailed)) pending = "pending: the real core is not built into the server yet (game-host-mp H8)";
    else error = e.stack ?? String(e);
  } finally {
    try {
      await bots?.stopAll();
    } catch {
      // the bots are gone with the run
    }
    for (const x of extras) {
      await x.bots.stopAll().catch(() => {});
      await x.server.stop().catch(() => {});
    }
    for (const l of links) await l.stop().catch(() => {});
    // Games of the scenario on a deployed server are closed here (the user, 2026-11-02): they do not wait for the
    // server's 5-minute rule. A local server dies with its state.
    if (gamesBefore && server?.admin) {
      const closed = await server.admin.closeGamesExcept(gamesBefore).catch(() => []);
      if (closed.length) console.log(`closed the scenario's games: ${closed.join(", ")}`);
    }
    if (server) await server.stop().catch(() => {});
  }
  if (pending) return skip(pending);
  // A scenario that checked nothing did not pass.
  const pass = !error && checks.length > 0 && checks.every((c) => c.pass);
  return {
    ...base,
    status: pass ? "pass" : "fail",
    durationMs: Date.now() - t0,
    params: { ...params, seed: o.seed },
    checks,
    numbers: result.numbers ?? {},
    pending: ctxRef?.pendingList ?? [],
    details: result.details,
    counters: result.counters,
    error,
  };
}

async function main() {
  const o = parseArgs(process.argv.slice(2));
  const all = await loadScenarios();
  if (o.list) {
    for (const [id, m] of all) console.log(`${id}\tneeds ${(m.meta.needs ?? []).join(",") || "-"}\tac ${(m.meta.ac ?? []).join(",") || "-"}`);
    return 0;
  }
  if (!o.server && !o.attach) throw new Error("--server <civ5-server executable> is required (cargo xtask nettest fills it)");
  const profilePath = path.join(here, "profiles", `${o.profile}.json`);
  if (!existsSync(profilePath)) throw new Error(`unknown profile ${o.profile} (have: ${readdirSync(path.join(here, "profiles")).join(", ")})`);
  const profile = JSON.parse(readFileSync(profilePath, "utf8"));
  const host = hostFor(profile.host);
  const caps = await detectCapabilities(o, profile, host);
  const ids = selection(o.scenario, all);
  const out = path.resolve(o.out);
  mkdirSync(out, { recursive: true });

  console.log(`profile ${profile.name}; capabilities: ${Object.entries(caps).map(([k, v]) => `${k}=${v ? "yes" : "no"}`).join(" ")}`);
  console.log(`preset ${o.preset}; seed ${o.seed}; core ${o.core}; link ${o.link}; scenarios: ${ids.join(", ")}`);

  const envInfo = { serverBuild: null, coreBuild: null };
  const results = [];
  for (const id of ids) {
    console.log(`== ${id}`);
    const r = await runScenario(all.get(id), o, caps, host, envInfo);
    results.push(r);
    if (r.status === "skip") console.log(`${id}: ${statusText(r)}`);
    else {
      if (r.error) console.log(`  error: ${r.error.split("\n")[0]}`);
      if (Object.keys(r.numbers).length) console.log(`  numbers: ${JSON.stringify(r.numbers, bigintReplacer)}`);
      console.log(`${id}: ${statusText(r)} (${(r.durationMs / 1000).toFixed(1)} s)`);
    }
  }

  const ac = acTable([...all.values()].map((m) => m.meta), results);
  const report = {
    startedAt: new Date().toISOString(),
    env: {
      profile: profile.name,
      mode: o.attach ? `attach ${o.attach}` : "local",
      core: o.core,
      link: o.link,
      serverBuild: envInfo.serverBuild ?? "n/a",
      coreBuild: envInfo.coreBuild ?? "n/a",
      protocol: 3,
      commit: gitCommit(),
      host: `${process.platform}, ${(await import("node:os")).cpus().length} cpus`,
      seed: o.seed,
      preset: o.preset,
      acceptance: o.acceptance,
    },
    capabilities: caps,
    scenarios: results,
    link: null,
    ac,
    acCounts: acCounts(ac),
    finishedAt: new Date().toISOString(),
  };
  const files = writeReport(out, report);
  const c = report.acCounts;
  console.log(`AC: ${c.all.pass} pass, ${c.all.fail} fail, ${c.all["not-covered"]} not-covered (of ${AC_LIST.length}; Must: ${c.must.pass}/${c.must.fail}/${c.must["not-covered"]})`);
  console.log(`report: ${files.md}`);
  const failed = results.some((r) => r.status === "fail");
  // An acceptance run may not skip a Must scenario (NT-D5).
  const skippedMust = o.acceptance && results.some((r) => r.status === "skip" && all.get(r.id).meta.must !== false);
  if (skippedMust) console.log("acceptance: a Must scenario was skipped");
  const bad = failed || skippedMust;
  console.log(bad ? "RESULT: FAIL" : "RESULT: PASS");
  return bad ? 1 : 0;
}

main().then(
  (code) => process.exit(code),
  (e) => {
    console.error(e.stack ?? String(e));
    process.exit(2);
  },
);
