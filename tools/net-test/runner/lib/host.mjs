// Host abstraction (net-test 4.1): where processes run. `local` is this OS; `wsl` runs the commands in
// WSL2 through `wsl -d <distro> -u root -- bash -lc` (the ports are forwarded to localhost, the files
// are reached through /mnt/<drive>). Server start, `kill -9`, file reads and `netem.sh` go through it.
import { spawn } from "node:child_process";
import { access, readFile } from "node:fs/promises";
import net from "node:net";

/** POSIX single quoting. */
export const shq = (s) => `'${String(s).replace(/'/g, `'\\''`)}'`;

export class LocalHost {
  constructor() {
    this.kind = "local";
    this.os = process.platform;
  }

  /** The path as this host sees it. */
  path(p) {
    return p;
  }

  spawn(cmd, args = [], opts = {}) {
    return spawn(cmd, args, { windowsHide: true, ...opts });
  }

  /** Runs to the end; resolves `{code, stdout, stderr}` (never rejects on a non-zero code). */
  exec(cmd, args = [], opts = {}) {
    const { timeoutMs = 60000, ...rest } = opts;
    return new Promise((resolve) => {
      let child;
      try {
        child = this.spawn(cmd, args, { stdio: ["ignore", "pipe", "pipe"], ...rest });
      } catch (e) {
        resolve({ code: -1, stdout: "", stderr: String(e) });
        return;
      }
      let stdout = "";
      let stderr = "";
      const timer = setTimeout(() => child.kill(), timeoutMs);
      child.stdout.on("data", (d) => (stdout += d));
      child.stderr.on("data", (d) => (stderr += d));
      child.on("error", (e) => {
        clearTimeout(timer);
        resolve({ code: -1, stdout, stderr: stderr + String(e) });
      });
      child.on("close", (code) => {
        clearTimeout(timer);
        resolve({ code: code ?? -1, stdout, stderr });
      });
    });
  }

  async readFile(p) {
    return readFile(p, "utf8");
  }

  async exists(p) {
    try {
      await access(p);
      return true;
    } catch {
      return false;
    }
  }

  /** Stops the process and its children. Resolves when it is gone (or after a short wait). */
  async killTree(child) {
    if (!child || child.exitCode !== null) return;
    if (process.platform === "win32") {
      await this.exec("taskkill", ["/PID", String(child.pid), "/T", "/F"], { timeoutMs: 10000 });
    } else {
      child.kill("SIGKILL");
    }
    await new Promise((r) => {
      if (child.exitCode !== null) return r();
      child.once("exit", r);
      setTimeout(r, 5000);
    });
  }

  /**
   * Kills the process alone, no children (a `kill -9` of the gateway: its worker must be left to see EOF).
   * `pattern` finds it again by its command line where the child handle is only a launcher (WSL).
   */
  async killProcess(child, pattern) {
    if (!child || child.exitCode !== null) return;
    if (process.platform === "win32") await this.exec("taskkill", ["/PID", String(child.pid), "/F"], { timeoutMs: 10000 });
    else child.kill("SIGKILL");
    await new Promise((r) => {
      if (child.exitCode !== null) return r();
      child.once("exit", r);
      setTimeout(r, 5000);
    });
  }

  /** Whether the process with this id (a worker the gateway started) is still alive. */
  async pidAlive(pid) {
    if (process.platform === "win32") {
      const r = await this.exec("tasklist", ["/FI", `PID eq ${pid}`, "/FO", "CSV", "/NH"], { timeoutMs: 10000 });
      return r.stdout.includes(`"${pid}"`);
    }
    try {
      process.kill(pid, 0);
      return true;
    } catch {
      return false;
    }
  }

  /** The resident memory of a process in KiB, or `null` (Windows: `tasklist`; Linux: the metrics of the gateway carry it). */
  async rssKb(pid) {
    if (!pid) return null;
    if (process.platform === "win32") {
      const r = await this.exec("tasklist", ["/FI", `PID eq ${pid}`, "/FO", "CSV", "/NH"], { timeoutMs: 10000 });
      // "node.exe","96364","Console","1","52 916 K": the unit is localised and in the OEM code page, only the digits are read.
      const field = r.stdout.trim().split('","')[4] ?? "";
      return Number(field.replace(/\D/g, "")) || null;
    }
    try {
      const t = await this.readFile(`/proc/${pid}/status`);
      const m = /VmRSS:\s+(\d+)/.exec(t);
      return m ? Number(m[1]) : null;
    } catch {
      return null;
    }
  }

  /** A free loopback port (the OS picks it; a race with another process is possible but rare). */
  freePort() {
    return new Promise((resolve, reject) => {
      const s = net.createServer();
      s.once("error", reject);
      s.listen(0, "127.0.0.1", () => {
        const { port } = s.address();
        s.close(() => resolve(port));
      });
    });
  }
}

export class WslHost extends LocalHost {
  constructor(distro = "Ubuntu-24.04") {
    super();
    this.kind = "wsl";
    this.os = "linux";
    this.distro = distro;
  }

  /** `E:\a\b` to `/mnt/e/a/b`; POSIX paths pass through. */
  path(p) {
    const m = /^([A-Za-z]):[\\/](.*)$/.exec(p);
    return m ? `/mnt/${m[1].toLowerCase()}/${m[2].replace(/\\/g, "/")}` : p.replace(/\\/g, "/");
  }

  spawn(cmd, args = [], opts = {}) {
    const { env, cwd, ...rest } = opts;
    const vars = Object.entries(env ?? {})
      .filter(([k]) => /^(CIV5_|NETTEST_|RUST_|GAMECORE_)/.test(k))
      .map(([k, v]) => `${k}=${shq(v)}`)
      .join(" ");
    // The executable may be a Windows path (xtask gives it so): convert it as well.
    const line = `${cwd ? `cd ${shq(this.path(cwd))} && ` : ""}${vars} exec ${[this.path(cmd), ...args].map(shq).join(" ")}`;
    return spawn("wsl", ["-d", this.distro, "-u", "root", "--", "bash", "-lc", line], { windowsHide: true, ...rest });
  }

  async killTree(child) {
    // wsl.exe forwards the end of the session to the process it started.
    await new LocalHost().killTree(child);
  }

  /** `kill -9` inside WSL: the gateway is found by its `--listen` argument (the launcher process is wsl.exe). */
  async killProcess(child, pattern) {
    if (!child || child.exitCode !== null) return;
    if (pattern) await this.exec("pkill", ["-9", "-f", "--", pattern], { timeoutMs: 10000 });
    else child.kill();
    await new Promise((r) => {
      if (child.exitCode !== null) return r();
      child.once("exit", r);
      setTimeout(r, 8000);
    });
  }

  // The processes live in WSL: the pid is a Linux one, seen through `kill -0` / `/proc` there.
  async pidAlive(pid) {
    const r = await this.exec("kill", ["-0", String(pid)], { timeoutMs: 10000 });
    return r.code === 0;
  }

  async rssKb(pid) {
    if (!pid) return null;
    const r = await this.exec("grep", ["VmRSS", `/proc/${pid}/status`], { timeoutMs: 10000 });
    const m = /VmRSS:\s+(\d+)/.exec(r.stdout);
    return m ? Number(m[1]) : null;
  }
}

export function hostFor(kind) {
  return kind === "wsl" ? new WslHost() : new LocalHost();
}
