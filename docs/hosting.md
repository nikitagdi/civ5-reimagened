# Hosting

How to run a server your friends can reach. The server is one Linux x86_64 process (`civ5-server`, the
gateway) that starts one worker process per running game. It is built for Docker (`ubuntu:24.04`, libc++ 18).
A native Windows server has only the stub core.

## What the host needs

- Linux x86_64 with Docker and Compose, reachable over ssh for `cargo xtask deploy`.
- A domain name (or a public IP with a self-signed certificate) and a TLS reverse proxy; Caddy files are in
  `deploy/public/`.
- On the host, a data folder (default `/srv/civ5`) with `assets/` (the pack from
  `civ5-assets build`) and `gamedata/` (from `civ5-assets export-gamedata`), readable by uid 1000.
- On your machine: the full build setup ([install.md](install.md)), Docker CLI, ssh, curl.

## Ports

| Port | Who listens | What |
|---|---|---|
| 8080 | gateway | web client, `/ws` (WebSocket), `/assets/*`, `/replica/*`, `/catalog/*`, `/healthz`, `/netcheck/` |
| 9090 | gateway, loopback only | admin API (`civ5-server admin ...`) |
| 443 (80 for ACME) | your reverse proxy | public HTTPS; forwards to 127.0.0.1:8080 |

The gateway has no TLS of its own. It refuses a non-loopback `listen` unless `allow_plain_nonlocal = true`,
which the container config sets because Compose publishes the port to 127.0.0.1 only
(`deploy/compose.public.yaml`).

## Configuration

`deploy/server.toml` is copied into the image as `/etc/civ5/server.toml`. Edit it before `cargo xtask dist`.
Command-line flags override it (`civ5-server --config <file> --listen ... --max-games ...`; the unknown-flag
message lists all of them).

| Key | Default in `deploy/server.toml` | Meaning |
|---|---|---|
| `listen`, `admin_listen` | `0.0.0.0:8080`, `127.0.0.1:9090` | gateway and admin listeners |
| `state_dir` | `/var/lib/civ5` | accounts (`accounts.json`), saves, logs, recordings; a Docker volume |
| `web_root`, `assets_root`, `gamedata_dir` | `/srv/civ5/web`, `/srv/civ5/assets`, `/srv/civ5/gamedata` | client files, asset pack, game data |
| `core` | `real` | `real` gamecore or `fixture` test world |
| `cross_origin_isolation` | `true` | COOP/COEP/CORP headers so browsers load the threaded client |
| `cookie_secure` | `true` (default) | set `false` only for plain http on a LAN address |
| `max_games`, `max_sessions`, `min_free_mem_mb` | 8, 64, 800 | limits |
| `trusted_proxies` | `127.0.0.1`, `172.16.0.0/12` | where the proxy's forwarded headers are trusted |
| `hibernate_idle_min`, `empty_close_min` | 5, 5 | a game with no human sleeps, then closes (saves stay) |
| `[worker] autosave` | every 60 s, ring of 10, every 10th turn kept | server saves |
| `[retention]`, `[recording]` | see the file | what is kept on disk; game recordings for `tools/civ5-replay` |

## Accounts and invite codes

Players register on the site with a nick and a password of their own. Registration asks for the server's
registration code, `CIV5_PASSWORD` (at least 8 characters): this is your invite code, not a login password.
Share it with your friends. `open_access` (`--open-access`) lets anyone register; do not use it on a public
host. Sessions are an HttpOnly cookie; one login per account at a time (a new login replaces the old one).

After login: create a room in the lobby, pick civilizations and slots (human, AI, closed), set the game
options, and start. Friends join from the lobby, also into a running game (an AI slot or their own seat).

## Option 1: cargo xtask deploy

```sh
cp deploy/targets/example.toml.template deploy/targets/myhost.toml   # fill in every value
cargo xtask deploy --target myhost --dry-run                          # print the plan, touch nothing
CIV5_PASSWORD=<invite code> cargo xtask deploy --target myhost
```

Target fields: `docker_context`, `ssh`, `domain`, `dir`, `caddy_mode` (`host`, `docker` or `none`),
`caddy_dir`, `reload_cmd`, `compose`. Empty values and `example.com` are refused. `deploy/targets/*.toml` is
git-ignored. The deploy builds `cargo xtask dist`, syncs the changed blobs of `--assets <dir>` (default
`../assets-out/b2`) and `--gamedata <dir>` (default `../assets-out/gamedata`), writes `<dir>/civ5.env`
(mode 0600) on the first run, builds and starts the image, waits for `/healthz`, installs the Caddy block,
runs a smoke test and keeps the last 3 image tags. `--rollback` starts the previous tag. A deploy that would
change the gamecore build while games run stops unless `--allow-core-change`. `--tls internal` makes a
self-signed site for a bare IP address. The registration code is never printed or put on a command line.
The smoke test (`cargo xtask nettest --scenario deploy-smoke`) runs a bot and a browser against the new
server, so it needs Node.js 22+ and `npm ci` in `tools/playtest` on the machine you deploy from; without
them pass `--skip-smoke`.

## Option 2: Docker Compose by hand

```sh
cargo xtask dist                      # prints target/dist/civ5-<build>/
export CIV5_BUILD=<build> CIV5_DIST=$PWD/target/dist/civ5-<build> CIV5_DIR=/srv/civ5 CIV5_PASSWORD=<invite code>
docker compose -f deploy/compose.yaml -f deploy/compose.public.yaml up -d --build
```

Copy the pack to `$CIV5_DIR/assets` and the game data to `$CIV5_DIR/gamedata` first. The container runs as
uid 1000, mounts both read-only, keeps state in the `civ5-state` volume and has a healthcheck
(`civ5-server --config /etc/civ5/server.toml healthcheck`). A systemd unit for running without Docker is in
`deploy/systemd/civ5-server.service`.

## TLS and the reverse proxy

`deploy/public/Caddyfile.civ5` (domain) and `Caddyfile.civ5.internal` (IP with `tls internal`) proxy the
dynamic paths to 127.0.0.1:8080 and serve `/assets/*` and the web root straight from the data folder with long
cache headers. Any other proxy works if it:

- forwards WebSocket upgrades on `/ws` and `/netcheck/ws`;
- keeps `Cross-Origin-Opener-Policy: same-origin`, `Cross-Origin-Embedder-Policy: require-corp` and
  `Cross-Origin-Resource-Policy: same-origin` (without them browsers fall back to the single-threaded client);
- sends `Content-Security-Policy: default-src 'none'; script-src 'self' 'unsafe-eval'` with `/ui2/worker.js`
  (the UI v2 script worker: no network for skin scripts), and no policy with the other files;
- serves HTTPS: browsers enable cross-origin isolation and `Secure` cookies only on secure origins
  (`localhost` counts as one).

`civ5-assets precompress --dir <assets> --only-hash-names` writes `.br`/`.zst` sidecars of the blobs for a
proxy that serves precompressed files (`brotli` binary needed for `.br`).

## Operating it

```sh
docker exec civ5 civ5-server --config /etc/civ5/server.toml admin GET /admin/metrics
docker exec civ5 civ5-server --config /etc/civ5/server.toml admin GET /admin/games
docker exec civ5 civ5-server --config /etc/civ5/server.toml admin POST /admin/accounts/<nick>/reset
```

The last one resets a forgotten password. A crashed worker restarts from the last autosave. `SIGTERM` (a
normal `docker stop`) saves every game before exit. Back up the `civ5-state` volume to keep accounts and saves.
`civ5-assets gc --out <assets>` removes blobs older manifests no longer need.
