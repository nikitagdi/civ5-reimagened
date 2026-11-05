// `lobby` (N4.2, net-test 4.5): the room of a host, its settings, the slots, "Ready", the start. Two bots, raw lobby
// commands (`LobbyCmd`), the fixture's pick lists (`WORLDSIZE_DUEL`/`WORLDSIZE_HUGE`, `fixture.lua`,
// `GAMESPEED_STANDARD`, `ERA_ANCIENT`, `HANDICAP_PRINCE`, `TURNTIMER_FAST`/`SLOW`, option `GAMEOPTION_FIXTURE`):
//   - every refused setting is a `LobbyError` with its text (validate.rs), and the room keeps its old settings;
//   - Duel -> Huge with a human in slot 5 (opened by the host first: slots beyond the default start closed);
//     the refusal of a shrink with a human above the new size's max_players (Duel max 4);
//   - `Slot { Ai }` on a seat with a human is a LobbyError; `Slot { Open }` on it takes the human out of the room;
//   - `Start` with an open slot, or with a guest not ready, is a LobbyError; a settings change clears "Ready";
//   - once everybody is ready the game starts: the admin metrics show the chosen world size, speed, map script,
//     turn mode, name, and every seat has a concrete civilization ("random" is replaced).
// Pending: the lists of the real core (the Huge and the Duel with all map scripts, the sizes of the table in 4.8),
// the readback of the room (`RoomState`) by the bot (the bot keeps it but does not report it, so the timer and the
// options of the room cannot be read back), the `Starting` status of the room (only `Running` is seen, through the
// admin API), and the closed slots beyond `default_players` (the bot does not report the slots).
// The room is the first room of a fresh server (id 1; a wrong id shows as a failed `in_room` check).
export const meta = {
  id: "lobby",
  ac: ["AC-N4.2"],
  needs: ["admin", "hooks"],
  core: ["fixture", "real"],
  quick: {},
  full: {},
};

const ROOM = 1;
const AUTO = {
  research: true, policy: true, ideology: true, religion: true, promotion: true, great_person: true,
  captured_city: true, league: true, archaeology: true, goody_hut: true, steal_tech: true,
};

/** A `GameSettings` (the wire JSON) with the fixture's first choices, changed by `over`. */
const settings = (over = {}) => ({
  name: "lobby-test",
  world_size: "WORLDSIZE_DUEL",
  map_script: "fixture.lua",
  map_options: [],
  speed: "GAMESPEED_STANDARD",
  start_era: "ERA_ANCIENT",
  handicap: "HANDICAP_PRINCE",
  minor_civs: 0,
  turn_mode: "Hybrid",
  timer: "Off",
  game_options: [],
  seed: null,
  map_seed: null,
  auto_choices: { ...AUTO },
  diplo_auto_decline: true,
  ...over,
});

export async function run(ctx) {
  const { server, expect, sleep } = ctx;
  const numbers = {};
  const H = ctx.bots.spawn({ nick: "lobH" });
  const G = ctx.bots.spawn({ nick: "lobG" });
  await H.connect();
  await H.wait({ conn: "lobby" }, 20000);

  /** The `LobbyError` texts that came after `cmd` (the answer comes in a moment). */
  const sendAndCollect = async (h, cmd) => {
    const before = (await h.stats()).lobby_errors.length;
    await h.lobby(cmd);
    await sleep(700);
    return (await h.stats()).lobby_errors.slice(before);
  };
  const refuses = async (h, label, cmd, re) => {
    const got = await sendAndCollect(h, cmd);
    expect(label, got.length === 1 && re.test(got[0]), JSON.stringify(got));
  };
  const accepts = async (h, label, cmd) => {
    const got = await sendAndCollect(h, cmd);
    expect(label, got.length === 0, JSON.stringify(got));
  };

  // ---- 1. the room, and the refused settings (one LobbyError each, the room keeps its settings)
  await accepts(H, "the host creates a Duel room", { CreateRoom: { settings: settings() } });
  expect("the host sits in its room", (await H.state()).in_room, "");
  await refuses(H, "an unknown world size is refused", { Settings: settings({ world_size: "WORLDSIZE_NOPE" }) }, /unknown world size/);
  await refuses(H, "an unknown game speed is refused", { Settings: settings({ speed: "GAMESPEED_NOPE" }) }, /unknown game speed/);
  await refuses(H, "an unknown map script is refused", { Settings: settings({ map_script: "nope.lua" }) }, /unknown map script/);
  await refuses(H, "an unknown start era is refused", { Settings: settings({ start_era: "ERA_NOPE" }) }, /unknown start era/);
  await refuses(H, "an unknown difficulty is refused", { Settings: settings({ handicap: "HANDICAP_NOPE" }) }, /unknown difficulty/);
  await refuses(H, "city-states above the size's maximum (Duel: 0) are refused", { Settings: settings({ minor_civs: 1 }) }, /city-states/);
  await refuses(H, "a forbidden option is refused", { Settings: settings({ game_options: [["QUICK_COMBAT", 1]] }) }, /not allowed/);
  await refuses(H, "an unknown option is refused", { Settings: settings({ game_options: [["GAMEOPTION_NOPE", 0]] }) }, /unknown option/);
  await refuses(H, "a fixed timer below 10 s is refused", { Settings: settings({ timer: { Fixed: { seconds: 9 } } }) }, /10 to 3600/);
  await refuses(H, "a timer row that is not in the list is refused", { Settings: settings({ timer: { Original: { row: "TURNTIMER_NOPE" } } }) }, /turn timer/);
  await refuses(H, "a room name over 40 characters is refused", { Settings: settings({ name: "n".repeat(41) }) }, /40 characters/);
  await accepts(H, "a timer row of the list is accepted", { Settings: settings({ timer: { Original: { row: "TURNTIMER_FAST" } } }) });
  await accepts(H, "the timer goes back to Off", { Settings: settings() });

  // ---- 2. an open slot at the start is a LobbyError (slot 1 is open: no guest yet)
  await refuses(H, "start with an open slot is a LobbyError", "Start", /./);
  expect("no game started with an open slot", !(await H.state()).attached, "");


  // ---- 3. Huge and the vanishing slot: a human above the Duel's two slots blocks the change to Duel
  await accepts(H, "Huge is accepted", { Settings: settings({ world_size: "WORLDSIZE_HUGE" }) });
  await G.connect();
  await G.wait({ conn: "lobby" }, 20000);
  await accepts(G, "the guest joins the room", { JoinRoom: { room: ROOM } });
  expect("the guest sits in the room", (await G.state()).in_room, "");
  // The slots beyond the size's default are closed (Huge: 8 default, slot 5 is inside it, but it starts closed): the
  // host opens it, then the guest takes it.
  await accepts(H, "the host opens slot 5 of the Huge room", { Slot: { index: 5, occupant: "Open", civ: "Random" } });
  await accepts(G, "the guest takes slot 5", { TakeSlot: { index: 5 } });
  // Duel has max_players 4 (the fixture as the real core): a human in slot 5 would vanish, so the change is refused.
  await refuses(H, "a change to Duel with a human in slot 5 (above max_players 4) is refused", { Settings: settings({ world_size: "WORLDSIZE_DUEL" }) }, /./);
  await accepts(G, "the guest moves to slot 1", { TakeSlot: { index: 1 } });
  await accepts(H, "Slot Closed on the empty slot 5 is accepted", { Slot: { index: 5, occupant: "Closed", civ: "Random" } });
  await accepts(H, "Duel is accepted once every human is in slots 0 and 1", { Settings: settings({ world_size: "WORLDSIZE_DUEL" }) });

  // ---- 4. a slot with a human: Slot Ai is refused, Slot Open takes the guest out of the room
  await refuses(H, "Slot Ai on the guest's seat is a LobbyError", { Slot: { index: 1, occupant: "Ai", civ: "Random" } }, /./);
  await accepts(H, "Slot Open on the guest's seat is accepted", { Slot: { index: 1, occupant: "Open", civ: "Random" } });
  await sleep(500);
  expect("the guest is out of the room (no room in its LobbyState)", !(await G.state()).in_room, "");

  // ---- 5. the start: the guest must be ready; a settings change clears the guest's Ready. A guest that joins a
  // room is seated in its first open slot (slot 1 here) by the server.
  await accepts(G, "the guest joins again (seated in the first open slot)", { JoinRoom: { room: ROOM } });
  expect("the guest is back in the room", (await G.state()).in_room, "");
  await refuses(H, "start before the guest is ready is a LobbyError (not ready)", "Start", /ready/);
  await accepts(G, "the guest is ready", { Ready: true });
  await accepts(H, "the host changes the timer to Fixed 30 s", { Settings: settings({ timer: { Fixed: { seconds: 30 } } }) });
  await refuses(H, "a settings change cleared the guest's Ready: start is a LobbyError (not ready)", "Start", /ready/);
  await accepts(G, "the guest is ready again", { Ready: true });
  await accepts(H, "start with everybody ready", "Start");

  // ---- 6. the game runs: the admin metrics show what was chosen, and the civilizations are concrete
  const gotGame = await H.wait({ conn: "attached" }, 60000).then(() => true, () => false);
  expect("the host is attached to its seat after the start", gotGame, "");
  const game = (await H.state()).attached?.game;
  const g = game !== undefined ? await server.admin.game(game) : null;
  expect("the game runs", g?.status === "Running", `${g?.status}`);
  expect("the game has the chosen world size, speed, map script and turn mode",
    g?.settings?.world_size === "WORLDSIZE_DUEL" && g?.settings?.speed === "GAMESPEED_STANDARD" &&
    g?.settings?.map_script === "fixture.lua" && g?.settings?.turn_mode === "Hybrid" && g?.settings?.name === "lobby-test",
    JSON.stringify(g?.settings));
  // The civilization of a seat is in the list endpoint (`GET /admin/games`), not in the per-game metrics.
  const listed = (await server.admin.games()).games?.find((x) => x.id === game);
  const civs = (listed?.seats ?? []).map((s) => s.civ);
  expect("every seat has a concrete civilization (random was replaced)", civs.length > 0 && civs.every((c) => c && !/random/i.test(c)), JSON.stringify(civs));
  numbers.game = { game, status: g?.status, settings: g?.settings, civs };
  await G.wait({ conn: "attached" }, 30000).catch(() => null);
  expect("the guest is attached to its seat", !!(await G.state()).attached, "");

  ctx.pending("the lists of the real core (Duel and Huge with every map script, the sizes of the table in 4.8): the fixture has two sizes and one map script");
  ctx.pending("the room's readback: the bot keeps RoomState but does not report it, so the timer and the game options of the room cannot be read back, and the Starting status is not seen (only Running, from the admin API)");
  ctx.pending("closed slots beyond default_players: the bot does not report the slots, so the count of open and closed seats is not checked");
  return { numbers };
}
