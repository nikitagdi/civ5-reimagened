# Limitations

What does not work, or works differently from the original game. This list reflects the current state of
`main`; it will change.

## Scope

- **Multiplayer only.** No single-player campaign without a server, no scenarios, no hot-seat. You can play
  alone against AI on your own server.
- **One ruleset.** Brave New World with Lekmod v34.15 on game version 1.0.3.279. No other mods; the game's
  modding system is stubbed.
- **Up to 8 humans per game**, plus AI players up to the map size's limit (a Huge game with 12 majors and 24
  city-states has been played).
- **Turn modes:** simultaneous and hybrid (default). Sequential turns are not supported.
- **Saves** use our own format. Original `.Civ5Save` files cannot be loaded, and our saves do not load in the
  original game.

## Features that do not exist yet

- Spectators / observers (designed, not built). A defeated player can keep watching from their old seat.
- Teams: every player is a team of one. Per-player difficulty: one difficulty for the whole game.
- Pre-made world-builder maps (`.Civ5Map`) in the lobby; random map size or type. Map scripts work.
- Private rooms; playing one account from two devices at once (a second login replaces the first).
- 3D leader screens; Steam features (achievements, cloud saves, matchmaking) are stubbed.
- Languages: English is the main language; others are partial.
- A changing-password page (an admin can reset a password, see [hosting.md](hosting.md)).

## Known behaviour differences

- **Fog of war is client-side.** Every browser holds the full game state and hides what you should not see
  only when drawing. A modified client could reveal hidden information. Play with people you trust.
- **Turn change.** The AI phase reaches the browsers as a whole snapshot each turn: late in big games the
  screen freezes for about 0.5-3 s at the turn change, and AI units jump to their new positions instead of
  animating their moves.
- **Interface copy reload.** The game itself runs only on the server, so players never desync from each
  other. The browser's read-only copy of the state for the interface can still differ from the server; then
  it reloads from a server snapshot: a few seconds of frozen screen on a big game. One known cause is still
  open.
- **Taking over a seat** mid-turn plays from the next turn.

## Render gaps

The renderer is recreated to resemble the original, not to match it pixel for pixel; several parts were tuned
by eye. Known gaps:

- Strategic view: missing.
- Coastlines and rivers: different noise and meandering; surf timing; ice cracks.
- Natural wonders and landmarks: some layouts differ; the Great Wall drawing; roads inside a tile.
- Fog-of-war patches and border lines are not exact; the minimap is drawn from the view.
- Trade routes are drawn as straight lines.
- Map particle effects run on our own particle system and differ in detail; a unit whose texture is missing
  gets a neutral texture.
- Anti-aliasing is MSAA 4x (the original offers 8x).
- WebGL2: terrain is drawn wrongly in Chrome's WebGL2 (grey, without heights). WebGPU is the supported path.

## Platforms

| Platform | Status |
|---|---|
| Chrome on Windows 11, WebGPU | the tested target: benchmarks, playtests, real games |
| Chrome on Linux or macOS | expected to work, not tested by us |
| Firefox | uses WebGL2 automatically, because its WebGPU ran the GPU process out of memory; `?gfx=webgpu` overrides. Not tested visually |
| Safari, iOS | not tested. iPhone Safari has no Fullscreen API; the client suggests Add to Home Screen instead |
| Android phones and tablets, Chrome | works: touch controls (tap, drag to pan, 500 ms hold for right click), Low preset by default. GPUs without BC texture compression get uncompressed fallback textures, which need more memory; the GPU memory of a Huge game (1.1-1.3 GB) can be a limit on weaker phones |
| Native desktop app (`apps/desktop`) | builds and runs with the stub core only |
| Server | Linux x86_64 (Docker). On Windows the real server runs inside WSL2 |
