// The journal lines a scenario saw. `state().journal` is the newest 20 lines ("T3: Research:
// Pottery (auto, SIMP-002)"); the scenarios look at it often enough to keep every line.

/** "(auto, SIMP-002)" at the end of an auto-choice or auto-decline line. */
const SIMP_TAG = /\(auto, SIMP-(\d{3})\)$/;
/**
 * A Lua error line: the journal does not tell the kind to the page, so the text is matched (a Lua
 * message names its chunk as `file.lua:LINE:`, or the runtime's "attempt to ...").
 */
const LUA_ERROR = /\.lua:\d+:|\blua error\b|attempt to (index|call|perform|compare|concatenate)/i;

export class JournalLog {
  constructor() {
    this.seen = new Set();
    this.order = [];
  }

  /** Takes the journal of a state (any number of times). */
  add(s) {
    for (const line of s?.journal ?? []) {
      if (!this.seen.has(line)) {
        this.seen.add(line);
        this.order.push(line);
      }
    }
  }

  lines() {
    return this.order.slice();
  }

  size() {
    return this.order.length;
  }

  /** Lines with a SIMP tag, by number: { "002": [lines], ... }. */
  bySimp() {
    const out = {};
    for (const l of this.order) {
      const m = SIMP_TAG.exec(l);
      if (m) (out[m[1]] ??= []).push(l);
    }
    return out;
  }

  luaErrors() {
    return this.order.filter((l) => LUA_ERROR.test(l));
  }

  summary() {
    const simp = Object.fromEntries(Object.entries(this.bySimp()).map(([k, v]) => [`SIMP-${k}`, v.length]));
    return { lines: this.order.length, simpTagged: simp, luaErrors: this.luaErrors().length };
  }
}

export { LUA_ERROR, SIMP_TAG };
