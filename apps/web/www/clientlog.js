// The client log ring (packet P4; live report 2026-10-31).
//
// The log lives here, in JS, not in the wasm memory, so it survives a wasm trap and a tab reload. The Rust side
// (`platform-web/src/log.rs`, `RingLayer`) hands over lines of the shared envelope (one JSON object per line) with
// `push(level, text)`; this file adds its own (page errors, the environment) and keeps them:
//
// * in memory: the last 512 KiB of the page's lines (for "send report" when storage fails);
// * in IndexedDB (`civ5-log`, store `chunks`, key `[sessionStart, n]`): chunks of up to 64 KiB of lines, gzip
//   (`CompressionStream`) when the flush is a timer's, plain when it is urgent (an error, `pagehide`, the crash
//   path: gzip is asynchronous and the page may not live to finish it). At most 8 MiB in all and the last 10 page
//   loads; the oldest chunks go first. A failed write (quota, a private window) turns the storage off for the page.
//
// Flushes: every 2 s, on an error line, when the page is hidden or leaves (`visibilitychange`, `pagehide`), and from
// the crash path (`finishCrash` in `bootstrap.js`). An IndexedDB write begun while the page is leaving may not be
// finished, so the lines not yet stored are also kept in `localStorage` (`civ5.logtail`, synchronous, at most 256 KiB)
// at those moments; the next page load stores them as the chunk of the page they came from. Lines since the last flush
// are lost only with a tab killed hard, at most 2 s of them.
//
// "Send a bug report" (`openReport`, Ctrl+Shift+F8): a plain DOM dialog (it works with a dead wasm module) with an
// optional comment, a box for the logs of the earlier page loads and the size; it posts the gzip of one JSON
// document to `POST /client-report/v2` (<= 4 MiB; logged-in players only: the nick and the token of the login go in
// headers) and shows the report id. When the post fails, "Save to a file" keeps the same body. Never in the logs: the
// token, the password, chat text (the Rust side keeps lengths only), the query string of the address.
//
// "Send report" of the performance panel (`sendPerfReport`): the same document with a `perf` section, the JSON that the
// wasm side makes (`client-app/src/perf_report.rs`) plus `perf.page` (user agent, pixel ratio, screen and canvas sizes,
// cores, memory) and the log lines of this page load only. The server keeps it as `perf.json` next to the report
// (`GET /admin/reports/{id}/perf`).

const DB_NAME = "civ5-log";
const STORE = "chunks";
const INDEX = "index";
const CAP_BYTES = 8 * 1024 * 1024;
const KEEP_SESSIONS = 10;
const CHUNK_MAX = 64 * 1024;
const MEM_MAX = 512 * 1024;
const TAIL_KEY = "civ5.logtail";
const TAIL_MAX = 256 * 1024;
const FLUSH_MS = 2000;
const REPORT_MAX = 4 * 1024 * 1024;
const COMMENT_MAX = 2000;
/** JS-made error lines per page (a dead module raises one per callback). */
const JS_ERRORS_MAX = 50;

const LEVELS = ["", "error", "warn", "info", "debug", "trace"];

const sessionStart = Date.now();
let nextChunk = 0;
let pending = [];
let pendingBytes = 0;
/** Lines handed to a write that has not finished (and the pending ones): what the tail in localStorage holds. */
let unstored = [];
/** The page's lines, newest last, for when the storage does not work. */
let mem = [];
let memBytes = 0;
let jsErrors = 0;
let timer = null;
let db = null;
let storageOff = typeof indexedDB === "undefined";
/** `{s, n, size, lines, gz}` of every stored chunk, oldest first. */
let index = [];
/** Chunks made before the database opened. */
let waiting = [];
let writes = Promise.resolve();

const encoder = new TextEncoder();

function lineOf(lvl, name, msg, fields) {
  const o = { ts: Date.now(), kind: "log", src: "client", lvl: LEVELS[lvl] ?? "info", target: "js" };
  if (name) o.name = name;
  o.msg = String(msg ?? "").slice(0, 4000);
  if (fields) o.f = fields;
  return JSON.stringify(o);
}

// ------------------------------------------------------------------------------------------ IndexedDB

function openDb() {
  return new Promise((resolve) => {
    if (typeof indexedDB === "undefined") return resolve(null);
    let req;
    try {
      req = indexedDB.open(DB_NAME, 1);
    } catch (_) {
      return resolve(null);
    }
    req.onupgradeneeded = () => {
      const d = req.result;
      d.createObjectStore(STORE, { keyPath: ["s", "n"] });
      d.createObjectStore(INDEX, { keyPath: ["s", "n"] });
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => resolve(null);
    req.onblocked = () => resolve(null);
  });
}

function reqPromise(req) {
  return new Promise((resolve, reject) => {
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

function txDone(tx) {
  return new Promise((resolve, reject) => {
    tx.oncomplete = () => resolve();
    tx.onerror = () => reject(tx.error);
    tx.onabort = () => reject(tx.error);
  });
}

async function loadIndex() {
  const tx = db.transaction(INDEX, "readonly");
  const all = await reqPromise(tx.objectStore(INDEX).getAll());
  // Chunks this page wrote while the index was being read are kept.
  const stored = new Set(all.map((c) => `${c.s}:${c.n}`));
  index = all.concat(index.filter((c) => !stored.has(`${c.s}:${c.n}`))).sort((a, b) => a.s - b.s || a.n - b.n);
}

/** Removes the oldest chunks until the caps hold: 10 page loads, 8 MiB. The newest chunk is never removed. */
async function enforceCaps() {
  if (!db) return;
  const sessions = () => new Set(index.map((c) => c.s)).size;
  const total = () => index.reduce((a, c) => a + c.size, 0);
  const drop = [];
  while (index.length > 1 && (sessions() > KEEP_SESSIONS || total() > CAP_BYTES)) drop.push(index.shift());
  if (!drop.length) return;
  const tx = db.transaction([STORE, INDEX], "readwrite");
  for (const c of drop) {
    tx.objectStore(STORE).delete([c.s, c.n]);
    tx.objectStore(INDEX).delete([c.s, c.n]);
  }
  await txDone(tx);
}

async function putChunk(rec) {
  const tx = db.transaction([STORE, INDEX], "readwrite");
  tx.objectStore(STORE).put(rec);
  const meta = { s: rec.s, n: rec.n, size: rec.size, lines: rec.lines, gz: rec.gz };
  tx.objectStore(INDEX).put(meta);
  await txDone(tx);
  index.push(meta);
}

// ------------------------------------------------------------------------------------------ gzip

async function gzip(bytes) {
  const stream = new Blob([bytes]).stream().pipeThrough(new CompressionStream("gzip"));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

async function gunzip(bytes) {
  const stream = new Blob([bytes]).stream().pipeThrough(new DecompressionStream("gzip"));
  return await new Response(stream).text();
}

const canGzip = () => typeof CompressionStream !== "undefined" && typeof DecompressionStream !== "undefined";

// ------------------------------------------------------------------------------------------ writing

/** One chunk into the database; the lines of this page's own chunk are then stored, not "unstored". */
async function store(rec, own) {
  await putChunk(rec);
  if (own) unstored.splice(0, rec.lines);
  await enforceCaps();
}

/** The lines not yet stored go to `localStorage` (synchronous: it lands even when the page is going away). */
function saveTail() {
  if (!unstored.length) return;
  try {
    let size = 0;
    let from = unstored.length;
    while (from > 0 && size + unstored[from - 1].length + 1 <= TAIL_MAX) size += unstored[--from].length + 1;
    localStorage.setItem(TAIL_KEY, JSON.stringify({ s: sessionStart, lines: unstored.slice(from) }));
  } catch (_) {}
}

/** The tail an earlier page left: stored as a plain chunk of that page (after its own chunks). */
function recoverTail() {
  try {
    const raw = localStorage.getItem(TAIL_KEY);
    if (!raw) return;
    localStorage.removeItem(TAIL_KEY);
    const t = JSON.parse(raw);
    if (!t || !Array.isArray(t.lines) || !t.lines.length || t.s === sessionStart) return;
    const data = encoder.encode(t.lines.join("\n") + "\n");
    waiting.push({ s: t.s, n: 1_000_000, gz: false, size: data.byteLength, lines: t.lines.length, data, own: false });
  } catch (_) {}
}

/** Stores `text` (whole lines) as one chunk; `urgent`: plain, no async gzip. */
function writeChunk(text, urgent) {
  const n = nextChunk++;
  const lines = text.split("\n").filter(Boolean).length;
  const job = async () => {
    if (storageOff) return;
    let data = null;
    let gz = false;
    if (!urgent && canGzip()) {
      try {
        data = await gzip(encoder.encode(text));
        gz = true;
      } catch (_) {
        data = null;
      }
    }
    if (!data) data = encoder.encode(text);
    const rec = { s: sessionStart, n, gz, size: data.byteLength, lines, data };
    if (!db) {
      waiting.push({ ...rec, own: true });
      return;
    }
    try {
      await store(rec, true);
    } catch (_) {
      // A quota or a private window: the storage is off for this page; the memory ring still works.
      storageOff = true;
    }
  };
  writes = writes.then(job, job);
  return writes;
}

/** Flushes the pending lines to the storage. `urgent`: a plain write at once (an error, the page leaving, a crash). */
function flush(urgent) {
  if (timer) {
    clearTimeout(timer);
    timer = null;
  }
  if (!pending.length) return writes;
  const text = pending.join("\n") + "\n";
  pending = [];
  pendingBytes = 0;
  // Whole lines per chunk, none over CHUNK_MAX (a single line is cut by the producers at 8 KB).
  let from = 0;
  let last = null;
  while (from < text.length) {
    let to = Math.min(text.length, from + CHUNK_MAX);
    if (to < text.length) {
      const nl = text.lastIndexOf("\n", to - 1);
      to = nl > from ? nl + 1 : to;
    }
    last = writeChunk(text.slice(from, to), !!urgent);
    from = to;
  }
  return last ?? writes;
}

function schedule() {
  if (!timer) timer = setTimeout(() => flush(false), FLUSH_MS);
}

/** Lines (joined by `\n`) from the Rust side or from this file; `level` is the most important level among them. */
function push(level, text) {
  if (typeof text !== "string" || !text) return;
  for (const line of text.split("\n")) {
    if (!line) continue;
    pending.push(line);
    unstored.push(line);
    pendingBytes += line.length + 1;
    mem.push(line);
    memBytes += line.length + 1;
  }
  while (memBytes > MEM_MAX && mem.length > 1) memBytes -= mem.shift().length + 1;
  // A storage that fails keeps nothing: the tail's list does not grow without end.
  if (unstored.length > 20000) unstored.splice(0, unstored.length - 20000);
  if (level <= 1) {
    saveTail();
    flush(true);
  } else schedule();
}

function log(lvl, name, msg, fields) {
  push(lvl, lineOf(lvl, name, msg, fields));
}

// ------------------------------------------------------------------------------------------ reading

/**
 * The stored page loads, oldest first, as `{start, lines}` (lines joined by `\n`): the newest ones that fit in
 * `budget` bytes of stored (gzip) size, at most `max` of them. The current page's lines are flushed first.
 */
async function sessions(max = KEEP_SESSIONS, budget = REPORT_MAX) {
  await flush(true);
  await writes;
  const out = [];
  if (!db || storageOff) {
    // No storage: what the memory holds is the page's log.
    if (mem.length) out.push({ start: sessionStart, lines: mem.join("\n") });
    return out;
  }
  const by = new Map();
  for (const c of index) {
    if (!by.has(c.s)) by.set(c.s, []);
    by.get(c.s).push(c);
  }
  const starts = [...by.keys()].sort((a, b) => b - a).slice(0, max);
  let used = 0;
  for (const s of starts) {
    const chunks = by.get(s);
    const size = chunks.reduce((a, c) => a + c.size, 0);
    if (out.length && used + size > budget) break;
    used += size;
    // All the reads in one go: a transaction does not live across the awaits of the gzip.
    const tx = db.transaction(STORE, "readonly");
    const st = tx.objectStore(STORE);
    const recs = await Promise.all(chunks.map((c) => reqPromise(st.get([c.s, c.n]))));
    const texts = [];
    for (const [i, rec] of recs.entries()) {
      const c = chunks[i];
      if (!rec) continue;
      try {
        texts.push(rec.gz ? await gunzip(rec.data) : new TextDecoder().decode(rec.data));
      } catch (_) {
        texts.push(JSON.stringify({ kind: "log", lvl: "warn", msg: `chunk ${c.n} of ${s} is unreadable` }) + "\n");
      }
    }
    out.unshift({ start: s, lines: texts.join("").replace(/\n+$/, "") });
  }
  return out;
}

/** What the stored log holds: for the console and the tests. */
async function stats() {
  await writes;
  return {
    sessionStart,
    storage: db && !storageOff ? "indexeddb" : "memory",
    chunks: index.length,
    bytes: index.reduce((a, c) => a + c.size, 0),
    sessions: new Set(index.map((c) => c.s)).size,
    pending: pending.length,
    memBytes,
  };
}

// ------------------------------------------------------------------------------------------ the report

const num = (v) => (typeof v === "number" && Number.isFinite(v) && v >= 0 ? v : null);

/** Where the player is: from the numbers the app publishes (`__civ5stats.ctx*`, the replica group). */
function wherePlace() {
  const st = window.__civ5stats ?? {};
  const rep = st.replica ?? {};
  const nextCmd = num(st.ctxNextCmd);
  return {
    game: num(st.ctxGame),
    player: num(st.ctxPlayer),
    turn: num(st.ctxTurn),
    replica_turn: num(rep.turn),
    replica_applied: num(rep.applied),
    batch_seq: num(st.ctxBatchSeq),
    view_seq: num(st.ctxViewSeq),
    cmd_last_id: nextCmd === null ? null : Math.max(0, nextCmd - 1),
    server_ms_est: num(st.ctxServerMs),
    open_turn_trace: num(st.turn?.open_turn),
  };
}

function clientInfo() {
  const t = window.__civ5threads;
  return {
    build: new URL("bootstrap.js", location.href).pathname,
    ua: navigator.userAgent,
    threads: t ? `${t.mode} ${t.workers}` : null,
    screen: `${screen.width}x${screen.height}@${window.devicePixelRatio}`,
    cores: navigator.hardwareConcurrency ?? null,
    deviceMemoryGB: navigator.deviceMemory ?? null,
    lang: navigator.language,
  };
}

/**
 * The nick of the last login, from the page's store. `null`: nobody signed in on this page. The device token is in the
 * HttpOnly session cookie, which the report request carries by itself (same origin); `token` is set only for a browser
 * that still holds an old localStorage token.
 */
function credentials() {
  try {
    const nick = localStorage.getItem("civ5.nick");
    if (!nick) return null;
    const token = localStorage.getItem(`civ5.token.${nick}`);
    return { nick, token: token && /^[0-9a-f]{32}$/.test(token) ? token : null };
  } catch (_) {
    return null;
  }
}

/** The games the logged sessions attached to (`net.attach` lines). */
function recentGames(list) {
  const seen = new Set();
  for (const s of list) {
    for (const l of s.lines.split("\n")) {
      if (!l.includes('"net.attach"')) continue;
      try {
        const g = JSON.parse(l)?.f?.game;
        if (Number.isInteger(g)) seen.add(g);
      } catch (_) {}
    }
  }
  return [...seen].slice(-8).map((game) => ({ game }));
}

/**
 * The gzip body of a report: `{body, doc, dropped}`. The oldest sessions are dropped until it fits in 4 MiB, then the
 * oldest half of the lines of the first session, until it does (the newest lines are the ones that matter).
 */
async function buildReport({ comment = "", earlier = true, perf = null } = {}) {
  const list = await sessions(earlier ? KEEP_SESSIONS : 1, REPORT_MAX);
  const doc = {
    v: 2,
    comment: String(comment).slice(0, COMMENT_MAX),
    client: clientInfo(),
    where: wherePlace(),
    recent_recordings: recentGames(list),
    sessions: list.map((s) => ({ start: s.start, lines: s.lines })),
  };
  if (perf && typeof perf === "object") doc.perf = perf;
  let dropped = 0;
  for (;;) {
    const body = canGzip() ? await gzip(encoder.encode(JSON.stringify(doc))) : encoder.encode(JSON.stringify(doc));
    if (body.byteLength <= REPORT_MAX - 1024) return { body, doc, dropped };
    if (doc.sessions.length > 1) {
      doc.sessions.shift();
      dropped++;
      continue;
    }
    const s = doc.sessions[0];
    if (!s || s.lines.length < 2048) return { body, doc, dropped };
    const cut = s.lines.indexOf("\n", s.lines.length >> 1);
    s.lines = cut < 0 ? s.lines.slice(s.lines.length >> 1) : s.lines.slice(cut + 1);
    dropped++;
  }
}

/** Posts a report. Resolves `{ok, status, id?, error?, body}`; `body` is kept for "Save to a file". */
async function sendReport(opts = {}) {
  const built = await buildReport(opts);
  const cred = credentials();
  if (!cred) return { ok: false, status: 0, error: "Reports are for logged-in players: log in first.", body: built.body };
  try {
    const res = await fetch(new URL("/client-report/v2", location.href), {
      method: "POST",
      headers: {
        "content-type": "application/gzip",
        "x-civ5-nick": encodeURIComponent(cred.nick),
        ...(cred.token ? { "x-civ5-token": cred.token } : {}),
      },
      body: built.body,
    });
    let json = null;
    try {
      json = await res.json();
    } catch (_) {}
    if (res.status === 201 && json && json.id) return { ok: true, status: 201, id: json.id, body: built.body };
    const why =
      res.status === 401
        ? "The server does not know this login: log in again."
        : res.status === 429
          ? "Too many reports for now: try again in an hour."
          : res.status === 413
            ? "The report is too big for the server."
            : (json && json.error) || `The server answered ${res.status}.`;
    return { ok: false, status: res.status, error: why, body: built.body };
  } catch (e) {
    return { ok: false, status: 0, error: `No connection to the server (${e && e.message ? e.message : e}).`, body: built.body };
  }
}

/** What only the page knows about the device: for the `perf.page` of a performance report. */
function pageInfo() {
  const canvas = document.getElementById("civ5-canvas");
  const conn = navigator.connection;
  const vv = window.visualViewport;
  const heap = performance.memory;
  return {
    ua: navigator.userAgent,
    devicePixelRatio: window.devicePixelRatio,
    screen: { w: screen.width, h: screen.height, availW: screen.availWidth, availH: screen.availHeight },
    window: { inner: [window.innerWidth, window.innerHeight], outer: [window.outerWidth, window.outerHeight] },
    visualViewportScale: vv ? vv.scale : null,
    canvas: canvas
      ? { width: canvas.width, height: canvas.height, client: [canvas.clientWidth, canvas.clientHeight] }
      : null,
    cores: navigator.hardwareConcurrency ?? null,
    deviceMemoryGB: navigator.deviceMemory ?? null,
    maxTouchPoints: navigator.maxTouchPoints ?? null,
    lang: navigator.language,
    crossOriginIsolated: self.crossOriginIsolated === true,
    threads: window.__civ5threads ? `${window.__civ5threads.mode} ${window.__civ5threads.workers}` : null,
    connection: conn ? { type: conn.effectiveType, downlinkMbps: conn.downlink, rttMs: conn.rtt } : null,
    jsHeap: heap ? { used: heap.usedJSHeapSize, total: heap.totalJSHeapSize, limit: heap.jsHeapSizeLimit } : null,
  };
}

/**
 * The performance panel's "Send report": `json` is the text the wasm side made. Resolves `{ok, status, id?, error?}`
 * like `sendReport`. Only this page load's log lines go with it.
 */
async function sendPerfReport(json) {
  let perf;
  try {
    perf = JSON.parse(json);
  } catch (_) {
    return { ok: false, status: 0, error: "The performance report is not JSON." };
  }
  perf.page = pageInfo();
  const adapter = perf.device && perf.device.adapter ? perf.device.adapter : "?";
  const scale = perf.graphics && perf.graphics.render_scale_pct;
  const comment = `performance report: ${adapter}, render scale ${scale ?? "?"}%`;
  const r = await sendReport({ comment, earlier: false, perf });
  if (r.ok) log(3, "report.sent", `a performance report was sent: ${r.id}`);
  else log(2, "report.failed", `a performance report was not sent: ${r.error}`, { status: r.status });
  return { ok: r.ok, status: r.status, id: r.id, error: r.error };
}

function saveToFile(body) {
  const url = URL.createObjectURL(new Blob([body], { type: "application/gzip" }));
  const a = document.createElement("a");
  a.href = url;
  a.download = `civ5-report-${new Date().toISOString().replace(/[:.]/g, "-")}.json.gz`;
  document.body.append(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 10000);
}

const fmtSize = (n) => (n >= 1048576 ? `${(n / 1048576).toFixed(1)} MB` : `${Math.max(1, Math.round(n / 1024))} KB`);

/** The dialog. Plain DOM, high z-index (over the crash overlay too). One at a time. */
function openReport() {
  const prior = document.getElementById("civ5-report");
  if (prior) return prior;
  const el = (tag, style, text) => {
    const e = document.createElement(tag);
    if (style) e.setAttribute("style", style);
    if (text !== undefined) e.textContent = text;
    return e;
  };
  const box = el(
    "div",
    "position:fixed;inset:0;z-index:2147483647;display:flex;align-items:center;justify-content:center;padding:16px;box-sizing:border-box;background:rgba(5,8,15,0.7);font:14px/1.45 system-ui,sans-serif;color:#e6ebf2",
  );
  box.id = "civ5-report";
  box.setAttribute("role", "dialog");
  const card = el(
    "div",
    "width:min(520px,100%);display:flex;flex-direction:column;gap:10px;padding:18px;background:#0d1320;border:1px solid #26324a;border-radius:8px;box-shadow:0 8px 40px rgba(0,0,0,0.6)",
  );
  const title = el("div", "font-size:17px;font-weight:600", "Send a bug report");
  const hint = el(
    "div",
    "color:#8a96aa;font-size:13px",
    "The log of this page and the earlier ones goes to the server with your note, so the problem can be looked into. No passwords or chat text are in it.",
  );
  const area = el(
    "textarea",
    "min-height:84px;resize:vertical;padding:8px;background:#05080f;color:#e6ebf2;border:1px solid #26324a;border-radius:6px;font:13px/1.4 system-ui,sans-serif",
  );
  area.maxLength = COMMENT_MAX;
  area.placeholder = "What happened? (optional, a line is enough)";
  const label = el("label", "display:flex;gap:8px;align-items:center;font-size:13px");
  const earlier = el("input");
  earlier.type = "checkbox";
  earlier.checked = true;
  label.append(earlier, el("span", "", "Include the logs of earlier page loads"));
  const size = el("div", "color:#8a96aa;font-size:12px", "Counting the size ...");
  const status = el("div", "min-height:20px;font-size:13px", "");
  const row = el("div", "display:flex;gap:8px;justify-content:flex-end;flex-wrap:wrap");
  const btn = (text, primary) =>
    el(
      "button",
      `padding:7px 16px;border-radius:6px;border:1px solid ${primary ? "#2f6fd6" : "#26324a"};background:${primary ? "#2f6fd6" : "#0d1320"};color:#fff;cursor:pointer;font:inherit`,
      text,
    );
  const send = btn("Send", true);
  const save = btn("Save to a file", false);
  save.style.display = "none";
  const cancel = btn("Cancel", false);
  row.append(save, cancel, send);
  card.append(title, hint, area, label, size, status, row);
  box.append(card);
  document.body.append(box);
  area.focus();

  let last = null;
  let sizing = 0;
  const resize = async () => {
    const mine = ++sizing;
    try {
      const b = await buildReport({ comment: area.value, earlier: earlier.checked });
      if (mine !== sizing) return;
      last = b.body;
      size.textContent = `About ${fmtSize(b.body.byteLength)}${b.dropped ? " (the oldest part is left out to fit)" : ""}.`;
    } catch (e) {
      size.textContent = `The size is not known (${e && e.message ? e.message : e}).`;
    }
  };
  earlier.onchange = resize;
  area.oninput = () => {
    clearTimeout(resize.t);
    resize.t = setTimeout(resize, 400);
  };
  resize();
  const close = () => box.remove();
  cancel.onclick = close;
  box.addEventListener("keydown", (ev) => {
    if (ev.key === "Escape") close();
    ev.stopPropagation();
  });
  save.onclick = async () => {
    if (!last) await resize();
    if (last) saveToFile(last);
  };
  send.onclick = async () => {
    send.disabled = true;
    status.textContent = "Sending ...";
    const r = await sendReport({ comment: area.value, earlier: earlier.checked });
    last = r.body;
    if (r.ok) {
      status.style.color = "#7bd88f";
      status.textContent = `Sent. Report ${r.id}: quote it when you tell what happened.`;
      send.style.display = "none";
      cancel.textContent = "Close";
      log(3, "report.sent", `a report was sent: ${r.id}`);
    } else {
      status.style.color = "#ff8a80";
      status.textContent = r.error;
      save.style.display = "";
      send.disabled = false;
      log(2, "report.failed", `a report was not sent: ${r.error}`, { status: r.status });
    }
  };
  return box;
}

// ------------------------------------------------------------------------------------------ the page

function jsError(name, e) {
  if (jsErrors++ >= JS_ERRORS_MAX) return;
  const msg = e && e.message !== undefined ? e.message : e;
  log(1, name, String(msg), { stack: String((e && e.stack) || "").slice(0, 1500) });
}

window.addEventListener("error", (ev) => jsError("js.error", ev.error ?? ev.message));
window.addEventListener("unhandledrejection", (ev) => jsError("js.rejection", ev.reason));
window.addEventListener("pagehide", () => {
  saveTail();
  flush(true);
});
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "hidden") {
    saveTail();
    flush(true);
  }
});
// SIMPLIFIED(SIMP-uic-52): no item in the Esc menu (the original's GameMenu context): the hotkey, the crash overlay.
window.addEventListener(
  "keydown",
  (ev) => {
    if (ev.ctrlKey && ev.shiftKey && !ev.altKey && ev.key === "F8") {
      ev.preventDefault();
      ev.stopPropagation();
      openReport();
    }
  },
  true,
);

recoverTail();
const ready = (async () => {
  db = await openDb();
  if (!db) {
    storageOff = true;
    return;
  }
  try {
    await loadIndex();
    for (const { own, ...rec } of waiting.splice(0)) await store(rec, own === true);
    await enforceCaps();
  } catch (_) {
    storageOff = true;
  }
})();

window.__civ5log = {
  push,
  flush,
  log,
  sessions,
  stats,
  buildReport,
  sendReport,
  sendPerfReport,
  openReport,
  ready,
  sessionStart,
};

{
  // The environment, once per page load. The address without its query (it can hold a name or a password).
  const t = window.__civ5threads;
  log(3, "env.page", "page loaded", {
    url: location.origin + location.pathname,
    ua: navigator.userAgent,
    screen: `${screen.width}x${screen.height}@${window.devicePixelRatio}`,
    cores: navigator.hardwareConcurrency ?? 0,
    mode: t ? t.mode : "?",
    crossOriginIsolated: self.crossOriginIsolated === true,
  });
}
