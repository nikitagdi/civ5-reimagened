// `selftest`: the runner's own check (not a scenario of the 4.5 table). The server starts on free ports with
// a temporary state dir, `/healthz` is 200, the admin API answers with its token and refuses without it,
// and the metrics are `schema: 1`. Needs no bots.
export const meta = {
  id: "selftest",
  ac: [],
  needs: ["admin"],
  core: ["fixture", "real"],
  commonChecks: false,
  full: {},
  quick: {},
};

export async function run(ctx) {
  const { server, expect } = ctx;
  const health = await fetch(`${server.url}/healthz`);
  expect("healthz is 200", health.status === 200, `HTTP ${health.status}`);
  const m = await server.admin.metrics();
  expect("metrics schema is 1", m.schema === 1, `schema ${m.schema}`);
  expect("protocol is 3", m.protocol === 3, `protocol ${m.protocol}`);
  expect("no game yet", Object.keys(m.games ?? {}).length === 0, `${Object.keys(m.games ?? {}).length} games`);
  if (!server.attached) {
    const status = await server.admin.statusWithoutToken();
    expect("admin without a token is refused", status === 401, `HTTP ${status}`);
    expect("the ports are not the defaults", server.port !== 8080 && server.adminPort !== 9090, `${server.port}, ${server.adminPort}`);
  }
  return { numbers: { build: m.build, core_build: m.core_build, uptime_s: m.uptime_s, ports: server.attached ? null : [server.port, server.adminPort] } };
}
