// A real, visible Chrome driven by the DevTools protocol alone (no Playwright between the browser and the page).
// Playwright makes every page of its context think it is visible and focused (focus emulation), which is
// exactly what the background tab spike (`scenarios/background.mjs`) must not have. Here a tab is hidden when
// another tab of the window is in front, as for a player. Node's own WebSocket and fetch are used.
import { spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function freePort() {
  return new Promise((resolve, reject) => {
    const s = net.createServer();
    s.once("error", reject);
    s.listen(0, "127.0.0.1", () => {
      const { port } = s.address();
      s.close(() => resolve(port));
    });
  });
}

/** The browser-level DevTools connection: commands with an optional session, events by name. */
class Cdp {
  constructor(ws) {
    this.ws = ws;
    this.next = 1;
    this.pending = new Map();
    this.handlers = [];
    ws.addEventListener("message", (m) => {
      const msg = JSON.parse(m.data);
      if (msg.id !== undefined) {
        const p = this.pending.get(msg.id);
        if (!p) return;
        this.pending.delete(msg.id);
        if (msg.error) p.reject(new Error(`${p.method}: ${msg.error.message}`));
        else p.resolve(msg.result);
      } else for (const h of this.handlers) h(msg);
    });
  }

  static async connect(url) {
    const ws = new WebSocket(url);
    await new Promise((resolve, reject) => {
      ws.addEventListener("open", resolve, { once: true });
      ws.addEventListener("error", () => reject(new Error(`cannot connect to ${url}`)), { once: true });
    });
    return new Cdp(ws);
  }

  send(method, params = {}, sessionId) {
    const id = this.next++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject, method });
      this.ws.send(JSON.stringify({ id, method, params, sessionId }));
    });
  }

  onEvent(h) {
    this.handlers.push(h);
  }
}

/** One tab: `evaluate` like Playwright's (a function and one JSON argument), the page's console errors. */
export class Tab {
  constructor(cdp, targetId, sessionId) {
    this.cdp = cdp;
    this.targetId = targetId;
    this.sessionId = sessionId;
    this.errors = [];
    this.lines = [];
    this.t0 = Date.now();
    cdp.onEvent((msg) => {
      if (msg.sessionId !== sessionId) return;
      if (msg.method === "Runtime.consoleAPICalled") {
        const text = msg.params.args.map((a) => a.value ?? a.description ?? "").join(" ");
        this.lines.push({ at: Date.now() - this.t0, type: msg.params.type, text });
        if (msg.params.type === "error") this.errors.push(text);
      } else if (msg.method === "Runtime.exceptionThrown") {
        this.errors.push(`uncaught: ${msg.params.exceptionDetails.exception?.description ?? msg.params.exceptionDetails.text}`);
      } else if (msg.method === "Log.entryAdded" && msg.params.entry.level === "error" && !/favicon/.test(msg.params.entry.url ?? "")) {
        this.errors.push(`${msg.params.entry.text} (${msg.params.entry.url ?? ""})`);
      }
    });
  }

  send(method, params) {
    return this.cdp.send(method, params, this.sessionId);
  }

  async init() {
    await this.send("Runtime.enable");
    await this.send("Log.enable");
    await this.send("Page.enable");
  }

  /** The script runs in every document this tab loads, before the page's own scripts. */
  addInitScript(source) {
    return this.send("Page.addScriptToEvaluateOnNewDocument", { source });
  }

  async goto(url) {
    await this.send("Page.navigate", { url });
  }

  /** `page.evaluate(fn, arg)`: the result by value; a thrown error or a rejected promise is thrown here. */
  async evaluate(fn, arg) {
    const expression = typeof fn === "function" ? `(${fn.toString()})(${JSON.stringify(arg ?? null)})` : fn;
    const r = await this.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text);
    return r.result.value;
  }

  async screenshot(file) {
    const r = await this.send("Page.captureScreenshot", { format: "png" });
    writeFileSync(file, Buffer.from(r.data, "base64"));
  }

  close() {
    return this.cdp.send("Target.closeTarget", { targetId: this.targetId });
  }

  /** The tab comes to the front of its window: the tab that was there is hidden. */
  activate() {
    return this.cdp.send("Target.activateTarget", { targetId: this.targetId });
  }

  /** What the browser does to a tab that stays hidden: "frozen" (timers and tasks stop) and back to "active". */
  setLifecycle(state) {
    return this.send("Page.setWebLifecycleState", { state });
  }
}

/**
 * Starts Chrome with its own profile (`prefs` is the `Default/Preferences` the profile starts with) and
 * connects to it. Returns `{ tab(url?), close(), exe, version }`.
 * @param {{exe: string, prefs?: object, args?: string[]}} o
 */
export async function launchChrome(o) {
  const dir = mkdtempSync(path.join(os.tmpdir(), "civ5-chrome-"));
  mkdirSync(path.join(dir, "Default"), { recursive: true });
  if (o.prefs) writeFileSync(path.join(dir, "Default", "Preferences"), JSON.stringify(o.prefs));
  const port = await freePort();
  const child = spawn(
    o.exe,
    [
      `--remote-debugging-port=${port}`,
      "--remote-allow-origins=*",
      `--user-data-dir=${dir}`,
      "--no-first-run",
      "--no-default-browser-check",
      "--window-size=1100,760",
      ...(o.args ?? []),
      "about:blank",
    ],
    { stdio: "ignore", windowsHide: false },
  );
  let version = null;
  for (let i = 0; i < 100; i++) {
    try {
      version = await (await fetch(`http://127.0.0.1:${port}/json/version`)).json();
      break;
    } catch {
      await sleep(200);
    }
  }
  if (!version) {
    child.kill();
    throw new Error(`Chrome did not open its DevTools port ${port}`);
  }
  const cdp = await Cdp.connect(version.webSocketDebuggerUrl);
  const attach = async (targetId) => {
    const { sessionId } = await cdp.send("Target.attachToTarget", { targetId, flatten: true });
    const tab = new Tab(cdp, targetId, sessionId);
    await tab.init();
    return tab;
  };
  return {
    exe: o.exe,
    version: version.Browser,
    dir,
    /** The first tab (about:blank) when `url` is not given, else a new tab in front. */
    async tab(url) {
      if (url === undefined) {
        const { targetInfos } = await cdp.send("Target.getTargets");
        const first = targetInfos.find((t) => t.type === "page");
        return attach(first.targetId);
      }
      const { targetId } = await cdp.send("Target.createTarget", { url });
      return attach(targetId);
    },
    async close() {
      try {
        await cdp.send("Browser.close");
      } catch {
        /* the browser went away */
      }
      await sleep(500);
      child.kill();
    },
  };
}
