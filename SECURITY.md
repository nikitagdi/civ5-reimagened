# Security policy

## Reporting a vulnerability

Please report security problems privately through GitHub: open the repository's **Security** tab and choose
**Report a vulnerability** (a private security advisory). Do not open a public issue for them.

Include what is affected, how to reproduce it, and what an attacker could do. We will acknowledge the report,
keep you informed while we work on a fix, and credit you in the advisory if you want.

## Supported versions

Only the `main` branch is supported. There are no releases with separate security fixes yet.

## Scope

In scope: the server (`civ5-server`: accounts, sessions and cookies, the registration code, the WebSocket
and HTTP endpoints, the admin API, file serving), the deploy tooling (`cargo xtask dist` and `deploy`, the
configs in `deploy/`), and the client where a malicious server or player could harm a player's machine or
account.

Known and documented, not a vulnerability report by itself: every browser holds the full game state, so a
modified client can see what fog of war hides ([docs/limitations.md](docs/limitations.md)). Ideas to change that
are welcome as normal issues.

The gamecore fork has its own repository; report gamecore problems there the same way.
