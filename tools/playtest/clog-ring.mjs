// The client log ring and the full bug report, in a real browser against the real civ5-server:
//   node tools/playtest/clog-ring.mjs [--exe <civ5-server>] [--headed]
//
// Checks, on `clientlog-selftest.html` (the ring alone, no game):
//  1. a line written before a reload is in the ring after it (gzip chunk of the timer's flush; plain chunk of the
//     pagehide flush for the line written less than 2 s before);
//  2. an error line survives a page that is closed at once (the urgent flush, the crash path);
//  3. the caps: at most 8 MiB stored, at most the last 10 page loads;
//  4. the report round trip: "send report" posts the logs of the earlier page loads with the comment, the server
//     stores it, and the admin API reads it back (list, meta, log with grep and level);
//  5. refusals: no login in the page means no report (the dialog says so, nothing is posted); a wrong token is 401.
// Exits 1 on a failed check. Needs Playwright (`npm ci` in tools/playtest) and a built `civ5-server`.
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, "../..");
const args = process.argv.slice(2);
const opt = (name, dflt) => (args.includes(name) ? args[args.indexOf(name) + 1] : dflt);
const exe = opt("--exe", path.join(root, "target", "debug", process.platform === "win32" ? "civ5-server.exe" : "civ5-server"));
const headed = args.includes("--headed");

const checks = [];
const check = (name, pass, detail = "") => {
  checks.push({ name, pass: !!pass, detail });
  console.log(`${pass ? "PASS" : "FAIL"}  ${name}${detail ? `  (${detail})` : ""}`);
};

const freePort = () =>
  new Promise((resolve, reject) => {
    const s = net.createServer();
    s.once("error", reject);
    s.listen(0, "127.0.0.1", () => {
      const { port } = s.address();
      s.close(() => resolve(port));
    });
  });

async function waitHttp(url, ms) {
  const t0 = Date.now();
  while (Date.now() - t0 < ms) {
    try {
      if ((await fetch(url)).ok) return;
    } catch (_) {}
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`no answer from ${url}`);
}

if (!existsSync(exe)) throw new Error(`civ5-server not found: ${exe} (cargo build -p server --bin civ5-server)`);
const work = mkdtempSync(path.join(os.tmpdir(), "clog-ring-"));
const stateDir = path.join(work, "state");
mkdirSync(stateDir, { recursive: true });
const NICK = "Ann";
const TOKEN = "0123456789abcdef0123456789abcdef";
// The identity of the logged-in player, as the gateway keeps it (`identities.json`, format 1).
writeFileSync(
  path.join(stateDir, "identities.json"),
  JSON.stringify({ format: 1, records: [{ nick: NICK, token: TOKEN, last_seen_ms: Date.now() }] }),
);
const port = await freePort();
let adminPort = await freePort();
while (adminPort === port) adminPort = await freePort();
const server = spawn(
  exe,
  ["--dev", "--core", "fixture", "--listen", `127.0.0.1:${port}`, "--admin-port", String(adminPort), "--state-dir", stateDir, "--web-root", path.join(root, "apps/web/www"), "--no-cross-origin-isolation"],
  { stdio: ["ignore", "pipe", "pipe"], windowsHide: true },
);
let serverLog = "";
server.stdout.on("data", (d) => (serverLog += d));
server.stderr.on("data", (d) => (serverLog += d));
const base = `http://127.0.0.1:${port}`;
let browser;
try {
  await waitHttp(`${base}/clientlog-selftest.html`, 30000);
  const adminToken = readFileSync(path.join(stateDir, "admin.token"), "utf8").trim();
  const admin = async (route) => {
    const r = await fetch(`http://127.0.0.1:${adminPort}${route}`, { headers: { "x-admin-token": adminToken } });
    return { status: r.status, text: await r.text() };
  };
  browser = await chromium.launch({ headless: !headed });
  const ctx = await browser.newContext();
  const login = (nick, token) =>
    ctx.addInitScript(([n, t]) => {
      if (n) {
        localStorage.setItem("civ5.nick", n);
        localStorage.setItem(`civ5.token.${n}`, t);
      } else {
        localStorage.clear();
      }
    }, [nick, token]);
  await login(NICK, TOKEN);
  const url = `${base}/clientlog-selftest.html`;
  const line = (n, msg, lvl = "info") => JSON.stringify({ ts: Date.now(), kind: "log", src: "client", lvl, target: "test", name: "test.line", msg, n });

  // ---- 1. a line before a reload is in the ring after it
  let page = await ctx.newPage();
  await page.goto(url);
  await page.evaluate(() => window.__civ5log.ready);
  await page.evaluate(async (l) => {
    window.__civ5log.push(3, l);
    await window.__civ5log.flush(false); // the timer's kind: gzip
  }, line(1, "written before the reload, flushed"));
  // A second line only a moment old: the reload's pagehide flush must save it.
  await page.evaluate((l) => window.__civ5log.push(3, l), line(2, "written just before the reload, not flushed yet"));
  await page.reload();
  await page.evaluate(() => window.__civ5log.ready);
  let sess = await page.evaluate(() => window.__civ5log.sessions());
  const all1 = sess.map((s) => s.lines).join("\n");
  check("a flushed line is in the ring after a reload", all1.includes("written before the reload, flushed"));
  check("a line less than 2 s old is saved by the pagehide flush", all1.includes("just before the reload, not flushed"));
  check("the earlier page load is a session of its own", sess.length === 2, `sessions ${sess.length}`);
  const raw = await page.evaluate(async () => {
    const db = await new Promise((res, rej) => {
      const r = indexedDB.open("civ5-log", 1);
      r.onsuccess = () => res(r.result);
      r.onerror = () => rej(r.error);
    });
    const all = await new Promise((res) => {
      const r = db.transaction("index").objectStore("index").getAll();
      r.onsuccess = () => res(r.result);
    });
    return all;
  });
  check("a timer flush is a gzip chunk, an urgent one plain", raw.some((c) => c.gz) && raw.some((c) => !c.gz), JSON.stringify(raw.map((c) => c.gz)));

  // ---- 2. an error survives a page closed at once
  await page.evaluate((l) => window.__civ5log.push(1, l), line(3, "ERROR just before the page died", "error"));
  await page.close();
  page = await ctx.newPage();
  await page.goto(url);
  await page.evaluate(() => window.__civ5log.ready);
  sess = await page.evaluate(() => window.__civ5log.sessions());
  check("an error line survives a page closed at once", sess.map((s) => s.lines).join("\n").includes("ERROR just before the page died"));

  // ---- 3. the caps
  // Lines of random text do not compress: 12 MiB of them must leave at most 8 MiB stored.
  await page.evaluate(async () => {
    const rnd = () => Array.from(crypto.getRandomValues(new Uint8Array(3000)), (b) => "abcdefghijklmnopqrstuvwxyz0123456789"[b % 36]).join("");
    for (let i = 0; i < 4000; i++) {
      window.__civ5log.push(3, JSON.stringify({ ts: Date.now(), kind: "log", lvl: "info", msg: rnd() }));
      if (i % 200 === 199) await window.__civ5log.flush(false);
    }
    await window.__civ5log.flush(false);
  });
  const st = await page.evaluate(() => window.__civ5log.stats());
  check("at most 8 MiB are stored", st.bytes <= 8 * 1024 * 1024, `${(st.bytes / 1048576).toFixed(2)} MiB in ${st.chunks} chunks`);
  for (let i = 0; i < 11; i++) {
    await page.reload();
    await page.evaluate(() => window.__civ5log.ready);
  }
  const st2 = await page.evaluate(() => window.__civ5log.stats());
  check("only the last 10 page loads are kept", st2.sessions <= 10, `sessions ${st2.sessions}`);

  // ---- 4. the report round trip (a fresh set of sessions with known lines)
  await page.evaluate(async (l) => {
    window.__civ5log.push(3, l[0]);
    window.__civ5log.push(2, l[1]);
    await window.__civ5log.flush(true);
  }, [line(10, "report me: info line"), line(11, "report me: warn line", "warn")]);
  const sent = await page.evaluate(() => window.__civ5log.sendReport({ comment: "my archer did not move", earlier: true }));
  check("the report is accepted (201 with an id)", sent.ok && /^\d{8}-\d{6}-Ann-[0-9a-f]{6}$/.test(sent.id ?? ""), JSON.stringify({ ...sent, body: undefined }));
  if (sent.ok) {
    const list = JSON.parse((await admin("/admin/reports")).text);
    check("the admin list shows it with the comment", list.reports.some((r) => r.id === sent.id && r.comment.includes("archer")));
    const meta = JSON.parse((await admin(`/admin/reports/${sent.id}`)).text);
    check("meta has the nick, the comment and the sessions", meta.nick === NICK && meta.comment === "my archer did not move" && meta.sessions.length >= 2, `${meta.sessions.length} sessions, ${meta.lines} lines`);
    check("the token is nowhere in the stored meta", !JSON.stringify(meta).includes(TOKEN));
    const log = (await admin(`/admin/reports/${sent.id}/log?grep=report%20me&limit=50`)).text;
    check("the log has the lines, readable through grep", log.includes("report me: info line") && log.includes("report me: warn line"));
    const warn = (await admin(`/admin/reports/${sent.id}/log?level=warn&limit=5000`)).text;
    check("level=warn drops the info lines", warn.includes("report me: warn line") && !warn.includes("report me: info line"));
    const envs = (await admin(`/admin/reports/${sent.id}/log?grep=env.page&limit=50`)).text.trim().split("\n").filter(Boolean);
    // The 11 reloads of the caps test each left their environment line (the tail of a leaving page is kept).
    check("the log holds the earlier page loads too (an env line each)", envs.length >= 9 && meta.sessions.length >= 9, `${envs.length} env lines, ${meta.sessions.length} sessions`);
  }
  // The dialog: opens with the hotkey, counts the size, sends, shows the id.
  await page.keyboard.press("Control+Shift+F8");
  await page.waitForSelector("#civ5-report");
  await page.fill("#civ5-report textarea", "from the dialog");
  await page.waitForFunction(() => /About \d/.test(document.querySelector("#civ5-report").innerText));
  await page.click("#civ5-report button:has-text('Send')");
  await page.waitForFunction(() => /Report \d{8}-/.test(document.querySelector("#civ5-report")?.innerText ?? ""), null, { timeout: 15000 });
  check("the dialog (Ctrl+Shift+F8) sends and shows the report id", true);

  // ---- 5. refusals
  const rejected = await page.evaluate(async () => {
    localStorage.setItem("civ5.token.Ann", "ffffffffffffffffffffffffffffffff");
    const r = await window.__civ5log.sendReport({ comment: "wrong token" });
    return { ok: r.ok, status: r.status };
  });
  check("a wrong token is refused (401)", !rejected.ok && rejected.status === 401, JSON.stringify(rejected));
  const nobody = await page.evaluate(async () => {
    localStorage.clear();
    const r = await window.__civ5log.sendReport({ comment: "not logged in" });
    return { ok: r.ok, status: r.status, error: r.error };
  });
  check("with no login nothing is posted (the dialog says to log in)", !nobody.ok && nobody.status === 0 && /log/i.test(nobody.error), JSON.stringify(nobody));
  const listAfter = JSON.parse((await admin("/admin/reports")).text);
  check("neither refusal left a report", !listAfter.reports.some((r) => /wrong token|not logged in/.test(r.comment)));
} catch (e) {
  check("the script ran to its end", false, e.stack ?? String(e));
  console.log(serverLog.split("\n").slice(-15).join("\n"));
} finally {
  await browser?.close().catch(() => {});
  server.kill();
  await new Promise((r) => setTimeout(r, 300));
  rmSync(work, { recursive: true, force: true });
}
const failed = checks.filter((c) => !c.pass);
console.log(`\n${checks.length - failed.length}/${checks.length} checks passed`);
process.exit(failed.length ? 1 : 0);
