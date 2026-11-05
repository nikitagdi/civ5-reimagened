// The real civ5-server for the browser scenarios (playtest §3.1): `--dev --web-root <bundle>
// --assets-root <assets>` on 127.0.0.1:<port>, as `cargo xtask dev` runs it.
import { spawn } from "node:child_process";
import { createWriteStream, existsSync, readFileSync, rmSync } from "node:fs";
import net from "node:net";
import path from "node:path";

/** A free local port (the OS picks one). */
export async function freePort() {
  return new Promise((resolve, reject) => {
    const s = net.createServer();
    s.once("error", reject);
    s.listen(0, "127.0.0.1", () => {
      const { port } = s.address();
      s.close(() => resolve(port));
    });
  });
}

async function waitHttp(url, timeoutMs, child) {
  const start = Date.now();
  let last = null;
  while (Date.now() - start < timeoutMs) {
    if (child.exitCode !== null) throw new Error(`server exited early (code ${child.exitCode})`);
    try {
      const r = await fetch(url);
      if (r.ok) return;
      last = `HTTP ${r.status}`;
    } catch (e) {
      last = e.message;
    }
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`server did not answer ${url} within ${timeoutMs} ms (${last})`);
}

/**
 * Starts the server; returns { url, port, adminPort, stateDir, admin(method, route), stop() }. Its output goes to `<logDir>/server.log`.
 * @param {{exe: string, webRoot: string, assets: string, port?: number, logDir: string, crossOriginIsolation?: boolean}} o
 */
export async function startServer(o) {
  for (const [name, p] of [["server", o.exe], ["web root", path.join(o.webRoot, "index.html")], ["assets", o.assets]]) {
    if (!existsSync(p)) throw new Error(`${name} not found: ${p}`);
  }
  const port = o.port || (await freePort());
  let adminPort = await freePort();
  while (adminPort === port) adminPort = await freePort();
  const log = createWriteStream(path.join(o.logDir, "server.log"));
  // Dev mode: loopback, the `dev` password; a private state dir per run.
  const stateDir = path.join(o.logDir, "state");
  // A run starts on a clean state dir: the games and saves of an earlier run would come back otherwise.
  rmSync(stateDir, { recursive: true, force: true });
  const args = [
    "--dev",
    "--core", "fixture",
    "--listen", `127.0.0.1:${port}`,
    // A free admin port of our own (the scenarios save and stop games through it), so parallel runs do not collide.
    "--admin-port", String(adminPort),
    "--state-dir", stateDir,
    "--web-root", o.webRoot,
    "--assets-root", o.assets,
    // B3 render-mt: COOP/COEP on the web files, so the page may use the threaded build.
    // The gateway isolates by default (threads by default); the single-threaded mode turns it off.
    ...(o.crossOriginIsolation ? ["--cross-origin-isolation"] : ["--no-cross-origin-isolation"]),
  ];
  const child = spawn(o.exe, args, {
    stdio: ["ignore", "pipe", "pipe"],
    windowsHide: true,
    env: { ...process.env, ...(o.env ?? {}) },
  });
  child.stdout.pipe(log);
  child.stderr.pipe(log);
  const url = `http://127.0.0.1:${port}`;
  try {
    await waitHttp(`${url}/`, 30000, child);
  } catch (e) {
    child.kill();
    throw e;
  }
  const tokenFile = path.join(stateDir, "admin.token");
  /** The admin API (`X-Admin-Token`): `admin("GET", "/admin/games")`, `admin("POST", "/admin/games/1/save")`. */
  async function admin(method, route) {
    let token = "";
    for (let i = 0; i < 50 && !token; i++) {
      token = existsSync(tokenFile) ? readFileSync(tokenFile, "utf8").trim() : "";
      if (!token) await new Promise((r) => setTimeout(r, 100));
    }
    const r = await fetch(`http://127.0.0.1:${adminPort}${route}`, { method, headers: { "X-Admin-Token": token } });
    const text = await r.text();
    let body = text;
    try {
      body = text ? JSON.parse(text) : null;
    } catch {
      /* not JSON */
    }
    return { status: r.status, ok: r.ok, body };
  }
  return {
    url,
    port,
    adminPort,
    stateDir,
    admin,
    async stop() {
      if (child.exitCode === null) {
        child.kill();
        await new Promise((r) => {
          child.once("exit", r);
          setTimeout(r, 3000);
        });
      }
      log.end();
    },
  };
}
