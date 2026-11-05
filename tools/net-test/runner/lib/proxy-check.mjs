// The measurements of `calibrate` for `netproxy` (net-test 4.3): RTT, bandwidth, blackhole, reset. The module
// runs on the OS of the proxy: the runner imports it on a local host and starts it with `node proxy-check.mjs
// '<json>'` inside WSL, so the client, the echo server and the proxy share one clock and one loopback (the port
// forwarding of WSL would smear the timings and turn an RST into a FIN). It prints `{checks, numbers}`.
import { spawn } from "node:child_process";
import net from "node:net";
import { fileURLToPath } from "node:url";
import { hist } from "./report.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const median = (xs) => [...xs].sort((a, b) => a - b)[Math.floor(xs.length / 2)];

function listenOn(server) {
  return new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server.address().port)));
}

async function freePort() {
  const s = net.createServer();
  const port = await listenOn(s);
  await new Promise((r) => s.close(r));
  return port;
}

async function startEcho() {
  const server = net.createServer((c) => {
    c.setNoDelay(true);
    c.on("data", (d) => c.write(d));
    c.on("error", () => {});
  });
  const port = await listenOn(server);
  return { port, stop: () => server.close() };
}

async function startProxy(exe, upstreamPort, seed) {
  const listen = await freePort();
  let control = await freePort();
  while (control === listen) control = await freePort();
  const child = spawn(exe, ["--listen", `127.0.0.1:${listen}`, "--upstream", `127.0.0.1:${upstreamPort}`, "--control", `127.0.0.1:${control}`, "--seed", String(seed)], {
    stdio: ["ignore", "pipe", "pipe"],
    windowsHide: true,
  });
  let out = "";
  child.stdout.on("data", (d) => (out += d));
  child.stderr.on("data", (d) => (out += d));
  for (let i = 0; i < 100 && !out.includes("netproxy listen="); i++) await sleep(50);
  if (!out.includes("netproxy listen=")) {
    child.kill();
    throw new Error(`netproxy did not start: ${out.trim()}`);
  }
  const base = `http://127.0.0.1:${control}`;
  const call = async (method, p, body) => {
    const r = await fetch(base + p, { method, headers: { "content-type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body) });
    if (!r.ok) throw new Error(`netproxy ${method} ${p}: HTTP ${r.status} ${await r.text()}`);
    return r.json();
  };
  return {
    listen,
    profile: (p) => call("POST", "/profile", { reset: true, connect_delay_ms: 0, ...p }),
    blackhole: (ms) => call("POST", "/blackhole", { ms }),
    reset: (body = {}) => call("POST", "/reset", body),
    stats: () => call("GET", "/stats"),
    stop: () => child.kill(),
  };
}

function connect(port) {
  return new Promise((resolve, reject) => {
    const s = net.connect(port, "127.0.0.1", () => {
      s.setNoDelay(true);
      resolve(s);
    });
    s.once("error", reject);
  });
}

/** Reads exactly `n` bytes (rejects on a close, an error or a timeout). */
function readBytes(s, n, timeoutMs = 30000) {
  return new Promise((resolve, reject) => {
    let got = Buffer.alloc(0);
    const timer = setTimeout(() => done(new Error(`timeout after ${got.length} of ${n} bytes`)), timeoutMs);
    const onData = (d) => {
      got = Buffer.concat([got, d]);
      if (got.length >= n) done(null, got);
    };
    const onEnd = () => done(new Error(`closed (EOF) after ${got.length} of ${n} bytes`));
    const onErr = (e) => done(e);
    function done(err, value) {
      clearTimeout(timer);
      s.off("data", onData);
      s.off("end", onEnd);
      s.off("error", onErr);
      err ? reject(err) : resolve(value);
    }
    s.on("data", onData);
    s.on("end", onEnd);
    s.on("error", onErr);
  });
}

async function pingMs(s) {
  const t = performance.now();
  s.write("ping");
  await readBytes(s, 4);
  return performance.now() - t;
}

/**
 * @param {{netproxy: string, rtts: number[], seed: number, tolerance_ms: number}} o
 * @returns {Promise<{checks: {name: string, pass: boolean, detail: string}[], numbers: object}>}
 */
export async function checkProxy(o) {
  const checks = [];
  const expect = (name, pass, detail = "") => checks.push({ name, pass: !!pass, detail: String(detail) });
  const tol = o.tolerance_ms;
  const numbers = { rtt: {}, tolerance_ms: tol };
  const echo = await startEcho();
  const px = await startProxy(o.netproxy, echo.port, o.seed);
  try {
    for (const rtt of o.rtts) {
      await px.profile({ delay_ms: rtt / 2 });
      const s = await connect(px.listen);
      await pingMs(s);
      const xs = [];
      for (let i = 0; i < (rtt >= 400 ? 15 : 30); i++) xs.push(await pingMs(s));
      s.destroy();
      const med = median(xs);
      numbers.rtt[rtt] = { median_ms: Math.round(med * 10) / 10, error_ms: Math.round((med - rtt) * 10) / 10, ...hist(xs) };
      expect(`proxy: RTT ${rtt} ms measured within ${tol} ms`, Math.abs(med - rtt) <= tol, `median ${med.toFixed(1)} ms, error ${(med - rtt).toFixed(1)} ms`);
    }

    // The bandwidth: 60 KB echoed through 1000 kbit/s in both directions; the rate between the first and the last byte.
    await px.profile({ rate_kbit: 1000 });
    {
      const s = await connect(px.listen);
      const total = 60000;
      s.write(Buffer.alloc(total, 7));
      let got = 0;
      let first = null;
      let afterFirst = 0;
      await new Promise((resolve, reject) => {
        const timer = setTimeout(() => reject(new Error("bandwidth: timeout")), 30000);
        s.on("data", (d) => {
          got += d.length;
          if (first === null) first = performance.now();
          else afterFirst += d.length;
          if (got >= total) {
            clearTimeout(timer);
            resolve();
          }
        });
        s.on("error", reject);
      });
      const kbit = (afterFirst * 8) / (performance.now() - first);
      s.destroy();
      numbers.rate_kbit_measured = Math.round(kbit);
      expect("proxy: bandwidth 1000 kbit/s within 10 %", Math.abs(kbit - 1000) <= 100, `${kbit.toFixed(0)} kbit/s`);
    }

    // The blackhole: nothing for 500 ms, then everything in order.
    await px.profile({});
    {
      const s = await connect(px.listen);
      await pingMs(s);
      await px.blackhole(500);
      const t0 = performance.now();
      const tags = ["aaaa", "bbbb", "cccc"];
      for (const t of tags) {
        s.write(t);
        await sleep(40);
      }
      const data = (await readBytes(s, 12)).toString();
      const dt = performance.now() - t0;
      s.destroy();
      expect("proxy: blackhole hands over the collected bytes in order", data === tags.join(""), JSON.stringify(data));
      expect("proxy: blackhole holds them until its end", dt >= 480, `${dt.toFixed(0)} ms of 500`);
    }

    // The reset: RST, not a clean end.
    {
      const s = await connect(px.listen);
      await pingMs(s);
      const r = await px.reset({ side: "client" });
      const outcome = await readBytes(s, 1, 5000).then(() => "data", (e) => (e.code === "ECONNRESET" ? "RST" : e.message));
      s.destroy();
      expect("proxy: reset gives RST", r.reset === 1 && outcome === "RST", `reset ${r.reset}, client saw ${outcome}`);
    }
    numbers.stats = await px.stats();
  } finally {
    px.stop();
    echo.stop();
  }
  return { checks, numbers };
}

// `node proxy-check.mjs '<json of the options>'`: prints the result as one JSON line.
if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1] && process.argv[2]) {
  checkProxy(JSON.parse(process.argv[2])).then(
    (r) => {
      console.log(JSON.stringify(r));
      process.exit(0);
    },
    (e) => {
      console.log(JSON.stringify({ checks: [{ name: "proxy: measurement ran", pass: false, detail: e.stack ?? String(e) }], numbers: {} }));
      process.exit(0);
    },
  );
}
