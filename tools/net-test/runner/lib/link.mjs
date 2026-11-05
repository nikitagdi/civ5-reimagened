// The channel between the bots and the server (`ctx.link`, net-test 4.3, 4.4): a `netproxy` in front of the server
// the runner started. The bots connect to `link.wsUrl` (a mission or `connect` takes `url`); the scenario sets the
// profile on the fly.

/**
 * The profiles the network scenarios (N5.6) put on the channel. `stall_p` is the loss model of the proxy: with that
 * probability a chunk is held back for 200-1000 ms, the way a TCP retransmission would hold it (the proxy does not
 * drop bytes: a dropped byte would be a broken stream). Real packet loss is `netem` (WSL, `--link netem`).
 */
export const PROFILES = Object.freeze({
  latency: { rtt_ms: 200, jitter_ms: 20 },
  weak: { rtt_ms: 200, jitter_ms: 20, stall_p: 0.03, stall_ms: [200, 1000] },
  clean: { rtt_ms: 0, jitter_ms: 0, rate_kbit: 0, stall_p: 0 },
});

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/**
 * @param {{host: import("./host.mjs").LocalHost, netproxy: string, server: {port: number}, seed?: number, logDir?: string}} o
 */
export async function startProxyLink(o) {
  const { host } = o;
  const listen = await host.freePort();
  let control = await host.freePort();
  while (control === listen) control = await host.freePort();
  const child = host.spawn(
    o.netproxy,
    ["--listen", `127.0.0.1:${listen}`, "--upstream", `127.0.0.1:${o.server.port}`, "--control", `127.0.0.1:${control}`, "--seed", String(o.seed ?? 1)],
    { stdio: ["ignore", "pipe", "pipe"] },
  );
  let out = "";
  child.stdout.on("data", (d) => (out += d));
  child.stderr.on("data", (d) => (out += d));
  for (let i = 0; i < 100 && !out.includes("netproxy listen="); i++) await sleep(100);
  if (!out.includes("netproxy listen=")) {
    await host.killTree(child);
    throw new Error(`netproxy did not start: ${out.trim()}`);
  }
  const base = `http://127.0.0.1:${control}`;
  const call = async (method, p, body) => {
    for (let i = 0; ; i++) {
      try {
        const r = await fetch(base + p, { method, headers: { "content-type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body) });
        if (!r.ok) throw new Error(`netproxy ${method} ${p}: HTTP ${r.status} ${await r.text()}`);
        return await r.json();
      } catch (e) {
        // The control port of a proxy in WSL is forwarded a moment late.
        if (e.cause?.code !== "ECONNREFUSED" || i >= 40) throw e;
        await sleep(150);
      }
    }
  };
  let nominalRtt = 0;
  const link = {
    kind: "proxy",
    url: `http://127.0.0.1:${listen}`,
    wsUrl: `ws://127.0.0.1:${listen}/ws`,
    listen,
    control,
    /** The RTT the profile promises (2 x delay), for the scenarios that compare with it. */
    get nominalRtt() {
      return nominalRtt;
    },
    /** `set({rtt_ms: 200, jitter_ms: 20, rate_kbit: 0, stall_p: 0})` or the raw profile fields; others stay as they were. */
    async set(p = {}) {
      const { rtt_ms, ...rest } = p;
      const body = { ...rest };
      if (rtt_ms !== undefined) body.delay_ms = Math.round(rtt_ms / 2);
      const prof = await call("POST", "/profile", body);
      nominalRtt = prof.delay_ms * 2;
      return prof;
    },
    /** A clean channel again. */
    clear: async () => {
      const prof = await call("POST", "/profile", { reset: true });
      nominalRtt = 0;
      return prof;
    },
    blackhole: (ms) => call("POST", "/blackhole", { ms }),
    reset: (body = {}) => call("POST", "/reset", body),
    halfOpen: (dir, ms) => call("POST", "/half-open", { dir, ms }),
    stats: () => call("GET", "/stats"),
    stop: () => host.killTree(child),
  };
  return link;
}
