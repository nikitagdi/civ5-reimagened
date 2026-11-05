// A local plain front for an https server (`deploy-smoke` on a public host): `net-bot` speaks plain ws:// only
// (tungstenite without TLS, NT-D3), so the bot connects to 127.0.0.1:<port> and every connection goes on over
// TLS to the public address, certificate checked. The `Host` header of the request head is rewritten to the
// public name (the edge routes by it); after the head the bytes pass unchanged both ways, so the WebSocket
// upgrade and every frame travel the real public path (edge, its timeouts, the TLS of the site).
import net from "node:net";
import tls from "node:tls";

const HEAD_LIMIT = 16 * 1024;

/** Rewrites (or adds) the `Host` header of an HTTP/1.1 request head. */
export function rewriteHost(head, host) {
  const lines = head.split("\r\n");
  const at = lines.findIndex((l, i) => i > 0 && /^host\s*:/i.test(l));
  if (at > 0) lines[at] = `Host: ${host}`;
  else lines.splice(1, 0, `Host: ${host}`);
  return lines.join("\r\n");
}

/**
 * @param {{url: string}} o `https://name[:port]` of the server
 * @returns {Promise<{wsUrl: string, port: number, connections: () => number, tlsErrors: string[], stop: () => Promise<void>}>}
 */
export async function startTlsFront(o) {
  const target = new URL(o.url);
  if (target.protocol !== "https:") throw new Error(`tls-front: ${o.url} is not https`);
  const host = target.hostname;
  const port = Number(target.port || 443);
  const authority = target.port ? `${host}:${target.port}` : host;
  const sockets = new Set();
  const tlsErrors = [];
  let count = 0;

  const server = net.createServer((local) => {
    count++;
    sockets.add(local);
    let buf = Buffer.alloc(0);
    let remote = null;
    const closeBoth = () => {
      local.destroy();
      remote?.destroy();
    };
    local.on("close", () => sockets.delete(local));
    local.on("error", closeBoth);
    const onData = (chunk) => {
      buf = Buffer.concat([buf, chunk]);
      const end = buf.indexOf("\r\n\r\n");
      if (end < 0) {
        if (buf.length > HEAD_LIMIT) closeBoth();
        return;
      }
      local.off("data", onData);
      local.pause();
      const head = rewriteHost(buf.subarray(0, end).toString("latin1"), authority);
      const rest = buf.subarray(end);
      remote = tls.connect({ host, port, servername: host, ALPNProtocols: ["http/1.1"] }, () => {
        remote.write(Buffer.concat([Buffer.from(head, "latin1"), rest]));
        local.pipe(remote);
        remote.pipe(local);
        local.resume();
      });
      sockets.add(remote);
      remote.on("close", () => {
        sockets.delete(remote);
        local.destroy();
      });
      remote.on("error", (e) => {
        tlsErrors.push(String(e.message ?? e));
        closeBoth();
      });
    };
    local.on("data", onData);
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const lport = server.address().port;
  return {
    wsUrl: `ws://127.0.0.1:${lport}/ws`,
    port: lport,
    connections: () => count,
    tlsErrors,
    stop: async () => {
      for (const s of sockets) s.destroy();
      await new Promise((r) => server.close(() => r()));
    },
  };
}
