// The save of a late Huge game (`make-late-save`, used by `ai-time`; net-test 4.5, item 6 of section 12). It is kept
// outside git in `assets-out/saves/late-huge/` of the workspace as a copy of the server's state dir (the save files,
// the game record, the identities of the two nicks) plus the tokens of the bots, and `meta.json` with the build of
// the game core: a save is only good for the core it was made with (gc-abi-v2, decision 12), so a new core build
// means a new save.
import { cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";

/** `<workspace>/assets-out/saves/late-huge`, found by walking up from the repository; else under `target/`. */
export function lateSaveDir(root) {
  // The workspace's `assets-out` is the one that holds the game data (`gamedata-b2`), as in the runner's `findGamedata`.
  let any = null;
  for (let d = root, i = 0; i < 4; i++, d = path.dirname(d)) {
    const c = path.join(d, "assets-out");
    if (existsSync(path.join(c, "gamedata-b2"))) return path.join(c, "saves", "late-huge");
    if (existsSync(c)) any ??= c;
  }
  return any ? path.join(any, "saves", "late-huge") : path.join(root, "target", "late-save");
}

/** The notes of the stored save, or `null` when there is none. */
export function readLateMeta(dir) {
  try {
    return JSON.parse(readFileSync(path.join(dir, "meta.json"), "utf8"));
  } catch {
    return null;
  }
}

const skipLogs = (src) => path.basename(src) !== "logs";

/** Stores the state dir of the server and the tokens of `nicks` (the bots' dirs under `logDir/bots`). */
export function storeLateSave({ dir, stateDir, botsDir, nicks, meta }) {
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  cpSync(stateDir, path.join(dir, "state"), { recursive: true, filter: skipLogs });
  for (const n of nicks) {
    mkdirSync(path.join(dir, "bots", n), { recursive: true });
    cpSync(path.join(botsDir, n, "token"), path.join(dir, "bots", n, "token"));
  }
  writeFileSync(path.join(dir, "meta.json"), JSON.stringify({ ...meta, nicks, stored_at: new Date().toISOString() }, null, 2));
}

/** Puts the stored state dir and tokens in place (the server must be down). */
export function restoreLateSave({ dir, stateDir, botsDir, nicks }) {
  rmSync(stateDir, { recursive: true, force: true });
  cpSync(path.join(dir, "state"), stateDir, { recursive: true });
  for (const n of nicks) {
    mkdirSync(path.join(botsDir, n), { recursive: true });
    cpSync(path.join(dir, "bots", n, "token"), path.join(botsDir, n, "token"));
  }
}
