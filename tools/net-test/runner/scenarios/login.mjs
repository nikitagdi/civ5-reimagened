// `login` (N4.1, net-test 4.5): the door of the gateway. Bots and raw frames, one server:
//   - a password login gives a token; a wrong password gives BadPassword; a foreign token without a password
//     gives BadPassword too (the token belongs to another nick);
//   - a nick that is empty or over the limit gives BadNick (before any password check);
//   - a nick that is live in another session gives NickTaken (the password was right);
//   - a second connection with the token of a live session replaces it: the first gets `Detached { Replaced }`;
//   - six wrong passwords in a row: the answer comes later each time, and the sixth waits longer than the first
//     (the gateway holds a BadPassword back by 1 s, then grows it after `password_soft_fails` = 5);
//   - a v2 client (bare postcard Hello, protocol 2) gets the bare postcard `Reject(VersionMismatch { server: 3 })`;
//   - a text frame and a message over 64 KiB close the connection;
//   - the token still works after a restart of the gateway (the identities are on disk);
//   - ten wrong passwords block the address: the right password is then refused, and a token still logs in.
// Known gateway defect found here (FAIL, not a test fault): on a refused nick the gateway sends `Reject(BadNick)` and
// closes the socket while the client's `Login` (sent right behind `Hello`, one round trip) is still unread; the close
// resets the connection and the reject is lost in about half of the tries (the bot then sees os error 10054).
// Pending: the session limit (`max_sessions` = 64): it needs 65 live sessions, and the scenario does not open them.
// Bots are controlled with `ctl` (one nick each); the raw frames use Node's WebSocket and postcard by hand.
import path from "node:path";
import { existsSync } from "node:fs";

export const meta = {
  id: "login",
  ac: ["AC-N4.1"],
  needs: ["admin", "hooks"],
  core: ["fixture", "real"],
  // A refused login is a `Reject` that the bot keeps as an error of its own; these scenarios make refusals on purpose.
  allowErrors: [/^rejected: /],
  quick: {},
  full: {},
};

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function until(pred, ms) {
  const t0 = Date.now();
  for (;;) {
    const v = pred();
    if (v) return v;
    if (Date.now() - t0 > ms) return v;
    await sleep(50);
  }
}

/** postcard varint (unsigned LEB128). */
function varint(n) {
  const out = [];
  let v = n;
  while (v >= 0x80) {
    out.push((v & 0x7f) | 0x80);
    v = Math.floor(v / 128);
  }
  out.push(v);
  return out;
}

/** postcard string: varint length in bytes, then UTF-8. */
function pstr(s) {
  const b = Buffer.from(s, "utf8");
  return [...varint(b.length), ...b];
}

/** A raw socket with its frames and its close, opened by hand (no bot). */
function openRaw(url) {
  const rec = { frames: [], closed: null };
  const ws = new WebSocket(url);
  ws.binaryType = "arraybuffer";
  rec.ws = ws;
  rec.opened = new Promise((resolve, reject) => {
    ws.onopen = () => resolve();
    ws.onerror = () => {};
    setTimeout(() => reject(new Error("no open within 5 s")), 5000);
  });
  ws.onmessage = (e) => rec.frames.push(new Uint8Array(e.data));
  ws.onclose = (e) => {
    rec.closed = { code: e.code, reason: e.reason };
  };
  return rec;
}

export async function run(ctx) {
  const { server, expect } = ctx;
  const numbers = {};
  ctx.restarts = 1; // one gateway restart below: the common check knows it
  const tokenOf = (nick) => path.join(ctx.logDir, "bots", nick, "token");
  process.env.NT_WRONG_PASSWORD = "not-the-password";
  process.env.NT_EMPTY_PASSWORD = "";
  const rejectOf = (h) => h.events.find((e) => e.ev === "error" && e.what === "Reject")?.reason ?? null;
  const waitReject = async (h, ms = 15000) => {
    await until(() => rejectOf(h), ms);
    return rejectOf(h);
  };
  const loggedIn = (h) => h.events.some((e) => e.ev === "lobby" && e.what === "logged_in");

  // ---- raw frames (no state needed): a v2 Hello, a text frame, a frame over 64 KiB
  const v2 = openRaw(server.wsUrl);
  await v2.opened;
  // Bare postcard ClientMsg::Hello { protocol: 2, build, nick }: no frame byte, as a v2 client sends it.
  v2.ws.send(Uint8Array.from([0, ...varint(2), ...pstr("net-test-v2"), ...pstr("v2probe")]));
  await until(() => v2.frames.length > 0 || v2.closed, 5000);
  const v2got = v2.frames[0] ? [...v2.frames[0]] : null;
  expect("protocol 2: a bare postcard Reject(VersionMismatch { server: 3 }) comes back", JSON.stringify(v2got) === JSON.stringify([1, 0, 3]), `got ${JSON.stringify(v2got)}`);

  const txt = openRaw(server.wsUrl);
  await txt.opened;
  txt.ws.send("hello");
  expect("a text frame closes the connection", !!(await until(() => txt.closed, 5000)), "");

  const big = openRaw(server.wsUrl);
  await big.opened;
  big.ws.send(new Uint8Array(70000));
  expect("a message larger than 64 KiB closes the connection", !!(await until(() => big.closed, 5000)), "");

  // ---- 1. the password login gives a token
  const A = ctx.bots.spawn({ nick: "loginA" });
  await A.connect();
  await A.wait({ conn: "lobby" }, 20000);
  expect("the password login gives LoggedIn and the token is kept", loggedIn(A) && existsSync(tokenOf("loginA")), `token file ${existsSync(tokenOf("loginA"))}`);

  // ---- 2. wrong password
  const W = ctx.bots.spawn({ nick: "loginW" });
  await W.connect({ password_env: "NT_WRONG_PASSWORD" });
  const wr = await waitReject(W);
  expect("a wrong password gives BadPassword", wr === "BadPassword", `${wr}`);

  // ---- 3. nick: empty, and over the limit (BadNick before any password check). Six tries each: the bot sends
  // `Login` right behind `Hello`, the gateway refuses the nick and closes with that frame unread, so the answer
  // can be lost behind a reset; the rate shows it.
  for (const [label, nick] of [["an empty nick", ""], ["a nick over the limit", "n".repeat(200)]]) {
    const outcomes = [];
    for (let i = 0; i < 6; i++) {
      const N = ctx.bots.spawn({ nick: `loginN${label.length}_${i}` });
      await N.connect({ nick });
      outcomes.push((await waitReject(N, 3000)) ?? `no answer (${N.events.find((e) => e.ev === "closed")?.reason || "closed"})`);
    }
    const good = outcomes.filter((o) => o === "BadNick").length;
    numbers[`badnick ${label}`] = outcomes;
    expect(`${label} gives BadNick (${good} of 6 tries got it)`, good === 6, outcomes.join("; "));
  }

  // ---- 4. a foreign token without a password: the token of loginA used by another nick
  const B = ctx.bots.spawn({ nick: "loginB" });
  await B.connect({ token_file: tokenOf("loginA"), password_env: "NT_EMPTY_PASSWORD" });
  const br = await waitReject(B);
  expect("a foreign token without a password gives BadPassword", br === "BadPassword", `${br}`);

  // ---- 5. nick taken: live in another session, the password right, no token
  const T = ctx.bots.spawn({ nick: "loginT" });
  await T.connect({ nick: "loginA" });
  const tr = await waitReject(T);
  expect("a live nick gives NickTaken", tr === "NickTaken", `${tr}`);

  // ---- 6. the same token in a second connection replaces the first one
  const A2 = ctx.bots.spawn({ nick: "loginA2" });
  await A2.connect({ nick: "loginA", token_file: tokenOf("loginA"), password_env: "NT_EMPTY_PASSWORD" });
  await A2.wait({ conn: "lobby" }, 20000);
  const replaced = await until(() => A.events.some((e) => e.ev === "lobby" && e.what === "detached" && e.reason === "Replaced"), 15000);
  expect("the second connection with the token logs in; the first gets Detached { Replaced }", !!replaced && loggedIn(A2), "");

  // ---- 7. six wrong passwords in a row: each answer later than the last
  const delays = [];
  for (let i = 0; i < 6; i++) {
    const D = ctx.bots.spawn({ nick: `loginD${i}` });
    const t0 = Date.now();
    await D.connect({ password_env: "NT_WRONG_PASSWORD" });
    const r = await waitReject(D, 60000);
    delays.push(Date.now() - t0);
    expect(`wrong password ${i + 1} of 6 gives BadPassword`, r === "BadPassword", `${r}`);
  }
  numbers.password_answer_ms = delays;
  expect("the sixth wrong password is answered later than the first (the delay grows)", delays[5] >= delays[0] + 3000, `first ${delays[0]} ms, sixth ${delays[5]} ms, all ${JSON.stringify(delays)}`);

  // ---- 8. restart of the gateway: the token of loginA still logs in
  await A.stop();
  await A2.stop();
  await server.restart();
  const R = ctx.bots.spawn({ nick: "loginR" });
  await R.connect({ nick: "loginA", token_file: tokenOf("loginA"), password_env: "NT_EMPTY_PASSWORD" });
  await R.wait({ conn: "lobby" }, 30000).catch(() => null);
  expect("after a restart of the gateway the token still logs in", loggedIn(R), `${rejectOf(R)}`);

  // ---- 9. the block: ten wrong passwords in the block window block the address (a fresh gateway: the count restarts)
  const blockDelays = [];
  for (let i = 0; i < 10; i++) {
    const K = ctx.bots.spawn({ nick: `loginK${i}` });
    const t0 = Date.now();
    await K.connect({ password_env: "NT_WRONG_PASSWORD" });
    await waitReject(K, 60000);
    blockDelays.push(Date.now() - t0);
  }
  const Z = ctx.bots.spawn({ nick: "loginZ" });
  const tz = Date.now();
  await Z.connect();
  const zr = await waitReject(Z, 60000);
  const zMs = Date.now() - tz;
  expect("the address is blocked: the right password gives BadPassword", zr === "BadPassword", `${zr}`);
  expect("the blocked answer comes without the delay", zMs < 5000, `${zMs} ms`);
  numbers.block_answer_ms = blockDelays;

  const Y = ctx.bots.spawn({ nick: "loginY" });
  await Y.connect({ nick: "loginA", token_file: tokenOf("loginA"), password_env: "NT_EMPTY_PASSWORD" });
  await Y.wait({ conn: "lobby" }, 30000).catch(() => null);
  expect("while the address is blocked a token still logs in", loggedIn(Y), `${rejectOf(Y)}`);

  ctx.pending("max_sessions (session limit 64): needs 65 live sessions; this scenario opens none of them");
  return { numbers };
}
