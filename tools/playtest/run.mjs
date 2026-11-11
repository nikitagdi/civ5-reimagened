// Playtest runner: `cargo xtask playtest` builds the
// bundle and the server and runs this; by hand (after `npm ci` here):
//   node tools/playtest/run.mjs --web-root target/web --server target/debug/civ5-server.exe \
//     --assets ../assets-out/b1 [--scenario autoplay|saveload|smoke|fatal|simp|lobby|autostart|golden|all|a,b] \
//     [--turns 50] [--seed 1] [--ai 1] [--seeds 1,2,3 --ais 1,2] [--save-turns 10]
//     [--after-load-turns 20] [--smoke-turns 3] [--simp-turns 5] [--simp-check <simp_check.py>]
//     m5-acceptance (WO-J): --attach <url of the dev-game-data server> [--attach-mp <url of the plain one>] [--turns 50] [M5_MODE=ai|mp]; see scenarios/m5-acceptance.mjs
//     m5-select-hotkeys: --attach <url of a real-core server> (tools/playtest/m5/serve-one.sh); see scenarios/m5-select-hotkeys.mjs
//     [--attach http://127.0.0.1:8090] (a running server, e.g. tools/playtest/m2/server.ps1: no --server/--assets, nothing is started or stopped)
//     [--headed] [--swiftshader (headless on the CPU; on Windows headless draws on the GPU)] [--port N] [--out target/playtest] [--gfx webgl2] [--preset low|medium|high] [--verbose]
//     perf (use --headed for the FPS limits; `cargo xtask playtest --scenario perf --headed --release`):
//     [--perf-runs 3] [--perf-turns 100] [--perf-idle-s 10] [--profile release|debug (label)]
//     background (the spike of a hidden game tab, use --headed; the system's Chrome or CHROMIUM_PATH):
//     [--bg-minutes 10] [--bg-mode plain|frozen] [--bg-memory-saver on|off]
//     golden (one frame per scene capture, the pixels saved to <golden-out>; not in `all`):
//     --scenario golden --golden-dir <scene dir> [--golden-out <dir>] [--golden-captures S1a,S1b]
//     [--golden-backend webgpu|webgl2]
// `--seeds` / `--ais` run `autoplay` once per pair (the AC-9.2 matrix); `--turns` is its length.
// `--scenario all` runs that matrix at 100 turns unless --turns/--seed(s)/--ai(s) are given.
// Writes <out>/report.json and report.md (scenarios, numbers), logs in
// <out>/logs/<scenario>/; exit code 0 only when every scenario passed.
import { mkdirSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { startServer } from "./lib/server.mjs";
import { writeReport } from "./lib/report.mjs";
import { acTable, acCounts } from "./lib/ac.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const SCENARIOS = {
  autoplay: () => import("./scenarios/autoplay.mjs"),
  saveload: () => import("./scenarios/saveload.mjs"),
  smoke: () => import("./scenarios/smoke.mjs"),
  fatal: () => import("./scenarios/fatal.mjs"),
  simp: () => import("./scenarios/simp.mjs"),
  lobby: () => import("./scenarios/lobby.mjs"),
  autostart: () => import("./scenarios/autostart.mjs"),
  background: () => import("./scenarios/background.mjs"),
  perf: () => import("./scenarios/perf.mjs"),
  golden: () => import("./scenarios/golden.mjs"),
  "terrain-scroll": () => import("./scenarios/terrain-scroll.mjs"),
  "terrain-turn": () => import("./scenarios/terrain-turn.mjs"),
  gallery: () => import("./scenarios/gallery.mjs"),
  "perf-huge": () => import("./scenarios/perf-huge.mjs"),
  hud: () => import("./scenarios/hud.mjs"),
  "perf-late": () => import("./scenarios/perf-late.mjs"),
  bench: () => import("./scenarios/bench.mjs"),
  quality: () => import("./scenarios/quality.mjs"),
  "cold-start": () => import("./scenarios/cold-start.mjs"),
  "m2-k1k3": () => import("./scenarios/m2-k1k3.mjs"),
  "m3-corners-chat": () => import("./scenarios/m3-corners-chat.mjs"),
  "m3-tech-policy-religion": () => import("./scenarios/m3-tech-policy-religion.mjs"),
  "m3-woi-boot": () => import("./scenarios/m3-woi-boot.mjs"),
  // UI v2 over a live game (?ui=v2:engraved): --attach <url of a real-core server>.
  "ui2-host": () => import("./scenarios/ui2-host.mjs"),
  "ui-sound": () => import("./scenarios/ui-sound.mjs"),
  "m3-city": () => import("./scenarios/m3-city.mjs"),
  "m4-diplo-trade": () => import("./scenarios/m4-diplo-trade.mjs"),
  "m4-congress-spies-routes": () => import("./scenarios/m4-congress-spies-routes.mjs"),
  "m4-overviews": () => import("./scenarios/m4-overviews.mjs"),
  "m4-menus-endgame": () => import("./scenarios/m4-menus-endgame.mjs"),
  "m5-leave-rejoin": () => import("./scenarios/m5-leave-rejoin.mjs"),
  "m5-mp-trade-kick": () => import("./scenarios/m5-mp-trade-kick.mjs"),
  "m5-mp-ui": () => import("./scenarios/m5-mp-ui.mjs"),
  "m5-goldens-s45": () => import("./scenarios/m5-goldens-s45.mjs"),
  "m5-goldens": () => import("./scenarios/m5-goldens.mjs"),
  "m5-tails": () => import("./scenarios/m5-tails.mjs"),
  "m5-g4": () => import("./scenarios/m5-g4.mjs"),
  "m5-acceptance": () => import("./scenarios/m5-acceptance.mjs"),
  "m5-fix-meter": () => import("./scenarios/m5-fix-meter.mjs"),
  "fix-path-hover": () => import("./scenarios/fix-path-hover.mjs"),
  "fix-overlays": () => import("./scenarios/fix-overlays.mjs"),
  "fix-ui4": () => import("./scenarios/fix-ui4.mjs"),
  "choices-city": () => import("./scenarios/choices-city.mjs"),
  // The 3D map follows the game under the original UI (live report 2026-10-30): --attach <url of a real-core server>.
  "m5-map-sync": () => import("./scenarios/m5-map-sync.mjs"),
  "m5-select-hotkeys": () => import("./scenarios/m5-select-hotkeys.mjs"),
  // Who has ended the turn shows in the players panel, an order after END TURN takes it back (live report 2026-10-30).
  "m5-turn-ready": () => import("./scenarios/m5-turn-ready.mjs"),
  // The END TURN button goes from "Waiting for players" straight to the new turn (live report 2026-10-31).
  "m5-turn-flow": () => import("./scenarios/m5-turn-flow.mjs"),
  // UI v2 script workers without a game (page ui2-selftest.html): echo, hang, exception, CSP of the worker.
  "ui2-script": () => import("./scenarios/ui2-script.mjs"),
  "ui2-n6": () => import("./scenarios/ui2-n6.mjs"), // where skin scripts run: worker against main thread (page ui2/bench/bench.html)
};
/** The scenarios of `--scenario all` (perf is headed for real FPS numbers; headless it keeps them but enforces no FPS limit). */
// Every scenario starts its game through the server (`?autostart=1`, net-client NC9); `saveload` uses the server's saves
// (admin API: save, stop; [Take] wakes the game) and `fatal` the faults of a server session (catalog, dropped link).
const ALL = ["smoke", "lobby", "autostart", "autoplay", "saveload", "fatal", "simp", "perf"];

function parseArgs(argv) {
  const o = {
    scenario: "autoplay",
    turns: 50,
    seed: 1,
    ai: 1,
    seeds: null,
    ais: null,
    saveTurns: 10,
    afterLoadTurns: 20,
    smokeTurns: 3,
    simpTurns: 5,
    simpCheck: null,
    headed: false,
    port: 0,
    out: "target/playtest",
    webRoot: "target/web",
    server: null,
    assets: null,
    gfx: null,
    preset: null,
    civ: null,
    perfRuns: 3,
    perfTurns: 100,
    perfIdleS: 10,
    bgMinutes: 10,
    bgMode: "plain",
    bgMemorySaver: "off",
    profile: null,
    goldenDir: null,
    goldenOut: null,
    goldenCaptures: null,
    goldenBackend: "webgpu",
    goldenScenes: null,
    goldenNoThreshold: false,
    verbose: false,
    screenshot: true,
    lobbyTimeoutMs: 120000,
    mapTimeoutMs: 300000,
    turnTimeoutMs: 300000,
  };
  const num = (k, v) => {
    const n = Number(v);
    if (!Number.isFinite(n)) throw new Error(`${k} needs a number, got ${v}`);
    return n;
  };
  const list = (k, v) => String(v).split(",").map((x) => num(k, x.trim()));
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const [k, inline] = a.includes("=") ? a.split(/=(.*)/s) : [a, undefined];
    const val = () => inline ?? argv[++i];
    switch (k) {
      case "--scenario": o.scenario = val(); break;
      case "--turns": o.turns = num(k, val()); break;
      case "--explore": o.explore = num(k, val()); break; // m3-corners-chat: seconds to keep the clients open at the end
      case "--luadbg": o.luadbg = val(); break; // m3-corners-chat: a Lua snippet for ?luadbg=
      case "--seed": o.seed = num(k, val()); break;
      case "--ai": o.ai = num(k, val()); break;
      case "--seeds": o.seeds = list(k, val()); break;
      case "--ais": o.ais = list(k, val()); break;
      case "--save-turns": o.saveTurns = num(k, val()); break;
      case "--after-load-turns": o.afterLoadTurns = num(k, val()); break;
      case "--smoke-turns": o.smokeTurns = num(k, val()); break;
      case "--simp-turns": o.simpTurns = num(k, val()); break;
      case "--simp-check": o.simpCheck = val(); break;
      case "--headed": o.headed = true; break;
      case "--swiftshader": process.env.CIV5_SWIFTSHADER = "1"; break; // headless on the CPU (the default on Windows is the GPU)
      case "--port": o.port = num(k, val()); break;
      case "--out": o.out = val(); break;
      case "--web-root": o.webRoot = val(); break;
      case "--server": o.server = val(); break;
      case "--assets": o.assets = val(); break;
      case "--gfx": o.gfx = val(); break;
      case "--preset": o.preset = val(); break; // the page gets ?preset=
      case "--civ": o.civ = val(); break;
      case "--gallery": o.gallery = val(); break; // gallery: the asset report dir with units.csv and landmarks.csv (default: --assets)
      case "--scroll-s": o.scrollS = num(k, val()); break; // terrain-scroll: seconds per direction
      case "--perf-runs": o.perfRuns = num(k, val()); break;
      case "--perf-turns": o.perfTurns = num(k, val()); break;
      case "--perf-idle-s": o.perfIdleS = num(k, val()); break;
      case "--attach": o.attach = val(); break; // any scenario: use the running server at this URL instead of starting one (no --server/--assets needed)
      case "--bench-modes": o.benchModes = val(); break; // bench: capped,uncapped (default both)
      case "--bench-units": o.benchUnits = num(k, val()); break; // bench: units of the heavy fixture (default 1500)
      case "--bench-cities": o.benchCities = num(k, val()); break; // bench: extra cities (default 80)
      case "--bench-ai": o.benchAi = num(k, val()); break; // bench: AI players (default 6)
      case "--bench-turns": o.benchTurns = num(k, val()); break; // bench: end turns of the last phase (default 8)
      case "--bench-phase-s": o.benchPhaseS = num(k, val()); break; // bench: seconds of an idle phase (default 10)
      case "--bench-settle-s": o.benchSettleS = num(k, val()); break; // bench: settle after the map (default 8)
      case "--bench-size": o.benchSize = val(); break; // bench: viewport WxH (default 1920x1080)
      case "--bench-stats": o.benchStats = val(); break; // bench: 0 = no ?stats=1 (frames only: checks what the stats cost)
      case "--bench-label": o.benchLabel = val(); break; // bench: a name for the run in the report
      case "--attach-mp": o.attachMp = val(); break; // m5-acceptance: the server of the 2-client MP mode (plain game data); --attach is the dev-game-data server of the 50-turn mode
      case "--late-url": o.lateUrl = val(); break; // perf-late: a running server with the late save (real core, WSL)
      case "--late-nick": o.lateNick = val(); break;
      case "--late-token": o.lateToken = val(); break; // file with the token of the nick
      case "--bg-minutes": o.bgMinutes = num(k, val()); break;
      case "--bg-mode": o.bgMode = val(); break; // background: plain | frozen
      case "--bg-memory-saver": o.bgMemorySaver = val(); break; // background: on | off
      case "--profile": o.profile = val(); break; // "release" | "debug": the wasm bundle's profile (xtask passes it)
      case "--turn-timeout-ms": o.turnTimeoutMs = num(k, val()); break;
      case "--golden-dir": o.goldenDir = val(); break; // golden: the scene folder (mapbasis.wv3, world.wv3, *.golden)
      case "--golden-out": o.goldenOut = val(); break; // golden: where the pixels go (default: the scenario's log dir)
      case "--golden-captures": o.goldenCaptures = val(); break; // golden: S1a,S1b (default: every *.golden)
      case "--golden-backend": o.goldenBackend = val(); break; // golden: webgpu | webgl2
      case "--golden-scenes": o.goldenScenes = val(); break; // m5-goldens: S1,S2 (default: every scene of lib/goldens.mjs)
      case "--golden-no-threshold": o.goldenNoThreshold = true; break; // m5-goldens: record the FLIP numbers only
      case "--verbose": o.verbose = true; break;
      case "--threads": o.threads = true; break; // B3 render-mt (default from xtask): the gateway sends COOP/COEP, the page loads the threaded build (pkg-mt)
      case "--single": o.threads = false; break; // the single-threaded fallback: no COOP/COEP, the page loads pkg
      case "--help":
        console.log(readFileSync(fileURLToPath(import.meta.url), "utf8").split("\nimport")[0]);
        process.exit(0);
        break;
      default:
        throw new Error(`unknown argument ${a}`);
    }
  }
  // `all` is the acceptance run: autoplay as the AC-9.2 matrix (3 seeds x {1, 2} AI x 100 turns)
  // unless the matrix or the length is given.
  const given = (f) => argv.some((a) => a === f || a.startsWith(`${f}=`));
  if (o.scenario === "all" && !["--turns", "--seeds", "--ais", "--seed", "--ai"].some(given)) {
    o.turns = 100;
    o.seeds = [1, 2, 3];
    o.ais = [1, 2];
  }
  const late = o.scenario === "perf-late" || (o.scenario === "bench" && !!o.lateUrl);
  const attached = !!o.attach;
  if (!o.server && !late && !attached) throw new Error("--server <civ5-server executable> is required (or --attach <url>)");
  if (!o.assets && !late && !attached) throw new Error("--assets <assets dir with manifest> is required (or --attach <url>)");
  return o;
}

/** The runs: one per scenario, `autoplay` once per seed/AI pair of the matrix. */
function plan(o) {
  const names = o.scenario === "all" ? ALL : o.scenario.split(",").map((s) => s.trim());
  for (const n of names) if (!SCENARIOS[n]) throw new Error(`unknown scenario ${n} (have: ${Object.keys(SCENARIOS).join(", ")}, all)`);
  const runs = [];
  for (const n of names) {
    if (n === "autoplay" && (o.seeds || o.ais)) {
      for (const seed of o.seeds ?? [o.seed]) for (const ai of o.ais ?? [o.ai]) runs.push({ scenario: n, name: `autoplay[s${seed},ai${ai}]`, seed, ai });
    } else runs.push({ scenario: n, name: n });
  }
  return runs;
}

async function main() {
  const o = parseArgs(process.argv.slice(2));
  const runs = plan(o);
  const out = path.resolve(o.out);
  mkdirSync(out, { recursive: true });
  const playwright = JSON.parse(readFileSync(createRequire(import.meta.url).resolve("playwright/package.json"), "utf8")).version;
  const report = {
    startedAt: new Date().toISOString(),
    webRoot: path.resolve(o.webRoot),
    playwright,
    headed: o.headed,
    options: { profile: o.profile, scenario: o.scenario, turns: o.turns, seed: o.seed, ai: o.ai, seeds: o.seeds, ais: o.ais, saveTurns: o.saveTurns, afterLoadTurns: o.afterLoadTurns, gfx: o.gfx, preset: o.preset, threads: !!o.threads },
    scenarios: [],
  };
  let failed = false;
  for (const r of runs) {
    const logDir = path.join(out, "logs", r.name.replace(/[^\w.-]+/g, "_"));
    mkdirSync(logDir, { recursive: true });
    console.log(`== ${r.name}`);
    const t0 = Date.now();
    let server = null;
    let result;
    try {
      const env =
        r.scenario === "terrain-scroll"
          ? { CIV5_FIXTURE_REVEAL_ALL: process.env.CIV5_FIXTURE_REVEAL_ALL ?? "1", CIV5_FIXTURE_WRAP_X: "1" }
          : r.scenario === "terrain-turn"
            ? { CIV5_FIXTURE_REVEAL_ALL: process.env.CIV5_FIXTURE_REVEAL_ALL ?? "1" }
            : r.scenario === "gallery"
            ? { CIV5_FIXTURE_REVEAL_ALL: "1", CIV5_FIXTURE_GALLERY: path.resolve(o.gallery ?? o.assets) }
            : r.scenario === "bench"
              ? { CIV5_FIXTURE_REVEAL_ALL: "1", CIV5_FIXTURE_HEAVY: `${o.benchUnits ?? 1500},${o.benchCities ?? 80}` }
              : undefined;
      const lateAttached = r.scenario === "perf-late" || (r.scenario === "bench" && o.lateUrl);
      if (o.attach) server = { url: o.attach.replace(/\/+$/, ""), attached: true, stop: async () => {} };
      else if (!lateAttached) server = await startServer({ exe: o.server, webRoot: path.resolve(o.webRoot), assets: path.resolve(o.assets), port: o.port, logDir, env, crossOriginIsolation: !!o.threads });
      console.log(server ? `${server.attached ? "attached to " : "server "}${server.url}` : `attached to ${o.lateUrl}`);
      const mod = await SCENARIOS[r.scenario]();
      if (r.scenario === "perf" && !o.headed) console.log("note: perf without --headed runs on SwiftShader: the FPS numbers mean nothing");
      result = await mod.run({ server, opts: { ...o, logDir }, root: here, name: r.name, seed: r.seed, ai: r.ai, bundle: o.profile });
    } catch (e) {
      result = { scenario: r.name, pass: false, logs: logDir, error: e.stack ?? String(e), durationMs: Date.now() - t0, checks: [] };
    } finally {
      if (server) await server.stop();
    }
    report.scenarios.push(result);
    failed ||= !result.pass;
    for (const c of result.checks ?? []) console.log(`  ${c.pass ? "ok  " : "FAIL"} ${c.name}${c.detail ? `: ${c.detail}` : ""}`);
    if (result.numbers) console.log(`  numbers: ${JSON.stringify(result.numbers)}`);
    if (result.error) console.log(`  error: ${result.error.split("\n")[0]}`);
    console.log(`${r.name}: ${result.pass ? "PASS" : "FAIL"} (${((Date.now() - t0) / 1000).toFixed(1)} s)`);
  }
  report.ac = acTable(report.scenarios);
  report.acCounts = acCounts(report.ac);
  report.finishedAt = new Date().toISOString();
  const files = writeReport(out, report);
  const c = report.acCounts;
  console.log(`AC: ${c.all.pass} pass, ${c.all.fail} fail, ${c.all["not-covered"]} not-covered (Must: ${c.must.pass}/${c.must.fail}/${c.must["not-covered"]})`);
  const fails = report.ac.filter((a) => a.status === "fail").map((a) => a.id);
  if (fails.length) console.log(`AC failed: ${fails.join(", ")}`);
  console.log(`report: ${files.md}`);
  console.log(failed ? "RESULT: FAIL" : "RESULT: PASS");
  process.exit(failed ? 1 : 0);
}

main().catch((e) => {
  console.error(e.stack ?? String(e));
  process.exit(2);
});
