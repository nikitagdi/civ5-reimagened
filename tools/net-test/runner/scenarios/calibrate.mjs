// `calibrate` (NT-D10, net-test 4.3): the channel gives what it promises.
//
// Link `proxy` (always): a TCP echo server, `netproxy` in front of it, a Node client. For RTT 100, 200, 400 and the
// bandwidth 1000 kbit/s: the median of 30 round trips is within 5 ms (Linux) or 16 ms (Windows) of the profile;
// the bandwidth is within 10 %; `blackhole` hands over what it collected in order; `reset` is an RST, not an EOF.
// Link `netem` (`--link netem`, WSL as root): the same for the real qdisc on the veth pair (RTT by `ping`,
// bandwidth by a TCP transfer, loss by `ping`, `blackhole on/off`).
//
// The controls of NT-D10 (`fog` with the leak, `hash` with `corrupt_view`, `latency` at RTT 0) are checked by those
// scenarios themselves (their `control` part); the calibration does not run them.
import path from "node:path";
import { fileURLToPath } from "node:url";
import { checkProxy } from "../lib/proxy-check.mjs";

export const meta = {
  id: "calibrate",
  ac: [],
  needs: ["proxy"],
  core: ["fixture", "real"],
  server: false,
  commonChecks: false,
  attach: false,
  quick: { rtts: [100, 200] },
  full: { rtts: [100, 200, 400] },
};

const here = path.dirname(fileURLToPath(import.meta.url));
const NETEM_SH = path.resolve(here, "../../netem/netem.sh");
const PROXY_CHECK = path.resolve(here, "../lib/proxy-check.mjs");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const median = (xs) => [...xs].sort((a, b) => a - b)[Math.floor(xs.length / 2)];

// ---------------------------------------------------------------- netem

const SINK_PY = `
import socket, sys, time
s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind((sys.argv[1], int(sys.argv[2]))); s.listen(1)
c, _ = s.accept()
total = int(sys.argv[3]); got = 0; first = None; after = 0
while got < total:
    d = c.recv(65536)
    if not d: break
    got += len(d)
    if first is None: first = time.time()
    else: after += len(d)
print(after * 8 / 1000 / max(time.time() - first, 1e-6))
`;
const SEND_PY = `
import socket, sys
s = socket.create_connection((sys.argv[1], int(sys.argv[2])))
s.sendall(b"x" * int(sys.argv[3])); s.close()
`;

async function netemPart(ctx, host) {
  const { expect } = ctx;
  const sh = host.path(NETEM_SH);
  const run = (...args) => host.exec("bash", [sh, ...args], { timeoutMs: 60000 });
  const numbers = { rtt: {} };
  const up = await run("up");
  if (!expect("netem: up", up.code === 0, up.stderr.trim() || up.stdout.trim())) return numbers;
  const inNs = (cmd, args, timeoutMs = 60000) => host.exec("ip", ["netns", "exec", "ntbots", cmd, ...args], { timeoutMs });
  const pings = async (count, interval, wait = 2) => {
    const r = await inNs("ping", ["-c", String(count), "-i", String(interval), "-W", String(wait), "10.77.0.1"], 120000);
    const times = [...r.stdout.matchAll(/time=([\d.]+) ms/g)].map((m) => Number(m[1]));
    return { times, sent: count };
  };
  try {
    for (const rtt of ctx.opts.rtts) {
      await run("set", String(rtt), "0", "0");
      const { times } = await pings(30, 0.2);
      const med = times.length ? median(times) : NaN;
      numbers.rtt[rtt] = { median_ms: Math.round(med * 10) / 10, error_ms: Math.round((med - rtt) * 10) / 10, n: times.length };
      // The qdisc of the WSL2 kernel adds about 5-6 ms to the delay it is asked for (measured); the scenarios compare with the measured RTT, so the bound here is 8 ms.
      expect(`netem: RTT ${rtt} ms measured within 8 ms`, Math.abs(med - rtt) <= 8, `median ${med.toFixed(1)} ms of ${times.length} pings, error ${(med - rtt).toFixed(1)} ms`);
    }

    // The bandwidth: 100 KB from the namespace to the root, 1000 kbit/s.
    await run("set", "0", "0", "0", "1000");
    {
      const port = 18999;
      const sink = host.exec("python3", ["-c", SINK_PY, "10.77.0.1", String(port), "100000"], { timeoutMs: 60000 });
      await sleep(600);
      await inNs("python3", ["-c", SEND_PY, "10.77.0.1", String(port), "100000"]);
      const r = await sink;
      const kbit = Number(r.stdout.trim());
      numbers.rate_kbit_measured = Math.round(kbit);
      expect("netem: bandwidth 1000 kbit/s within 10 %", Math.abs(kbit - 1000) <= 100, `${r.stdout.trim() || r.stderr.trim()} kbit/s`);
    }

    // The loss: 3 % each way, so about 6 % of the round trips.
    await run("set", "20", "0", "3");
    {
      const { times, sent } = await pings(300, 0.02, 1);
      const lost = (100 * (sent - times.length)) / sent;
      numbers.loss_roundtrip_pct = Math.round(lost * 10) / 10;
      expect("netem: loss 3 % each way gives 2..12 % of lost round trips", lost >= 2 && lost <= 12, `${lost.toFixed(1)} % of ${sent}`);
    }

    // The blackhole: nothing passes, then the channel comes back.
    await run("set", "20", "0", "0");
    await run("blackhole", "on");
    const dead = await pings(3, 0.3, 1);
    await run("blackhole", "off");
    const alive = await pings(3, 0.3, 1);
    expect("netem: blackhole on drops everything", dead.times.length === 0, `${dead.times.length} of 3 answered`);
    expect("netem: blackhole off brings the channel back", alive.times.length === 3, `${alive.times.length} of 3 answered`);
  } finally {
    await run("down");
  }
  return numbers;
}

export async function run(ctx) {
  const out = {};
  if (ctx.opts.link === "netem") {
    if (!ctx.caps.netem) {
      ctx.expect("netem is available (WSL, root, tc)", false, "run with --wsl; this profile has no netem");
    } else {
      out.netem = await netemPart(ctx, ctx.host);
    }
  }
  // The proxy is measured on its own OS: in-process here, or by a Node started in WSL.
  const host = ctx.host;
  const o = { netproxy: host.path(ctx.tools.netproxy), rtts: ctx.opts.rtts, seed: ctx.opts.seed, tolerance_ms: host.os === "win32" ? 16 : 5 };
  let r;
  if (host.kind === "local") r = await checkProxy({ ...o, netproxy: ctx.tools.netproxy });
  else {
    const x = await host.exec("node", [host.path(PROXY_CHECK), JSON.stringify(o)], { timeoutMs: 180000 });
    const line = x.stdout.trim().split("\n").filter((l) => l.startsWith("{")).pop();
    r = line ? JSON.parse(line) : { checks: [{ name: "proxy: measurement ran", pass: false, detail: x.stderr.trim() || x.stdout.trim() }], numbers: {} };
  }
  for (const c of r.checks) ctx.expect(c.name, c.pass, c.detail);
  out.proxy = r.numbers;
  return { numbers: out };
}
