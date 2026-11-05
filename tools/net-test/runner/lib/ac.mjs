// The acceptance criteria of track N and their status in a run (net-test 4.8).
// A row is **pass** when every scenario that checks it (its `meta.ac`, among the scenarios that exist)
// ran and passed; **fail** when any of them failed; **not-covered** otherwise, with the reason.
// A skipped scenario never counts as a pass (NT-D5).

const A = (id, priority, criterion, method, manual = "") => ({ id, priority, criterion, method, manual });

export const AC_LIST = [
  A("AC-N1.1", "Must", "Only the server computes the game: no brain in the client; solo play is a server game with one human", "auto (build)"),
  A("AC-N1.2", "Must", "Every diff carries the view hash; 0 mismatches in 200 turns with 4 bots; corruption heals by a keyframe in RTT + 60 ms", "auto"),
  A("AC-N1.3", "Must", "Fog: no unit or city invisible to the team in the messages, 100 turns (worker audit and the bot's client audit)", "auto"),
  A("AC-N1.4", "Must", "Queries have no side effects", "auto (gc-abi-v2 `purity`, game-host-mp `worker_real`)", "not here"),
  A("AC-N1.5", "Must", "A repeated order is not applied twice; a lost order is not lost silently", "auto"),
  A("AC-N2.1", "Must", "Order answer at RTT 200: p50 <= RTT + 60 ms, p95 <= RTT + 150 ms", "auto (500 bot orders)"),
  A("AC-N2.2", "Must", "A simple order is visible in 1 frame; on refusal the view returns and the reason is in the journal", "auto (browser)"),
  A("AC-N2.3", "Must", "Selecting a unit with a preloaded context needs no query; the first select after a change <= RTT + 50 ms", "auto (query counter)"),
  A("AC-N2.4", "Must", "The next order can be given without waiting for the previous answer", "auto"),
  A("AC-N2.5", "Must", "Server step per order: p95 <= 20 ms (Duel x 4), <= 60 ms (Huge x 8)", "auto"),
  A("AC-N3.1", "Must", "RTT 200 and 3 % loss: 30 turns, p95 <= 1 s, 0 false drops", "auto"),
  A("AC-N3.2", "Must", "10 s outage: view current within 2 s of the return; no queue while offline", "auto"),
  A("AC-N3.3", "Must", "Tab reload: game on screen <= 5 s, view hash equals the server's", "auto (browser)"),
  A("AC-N3.4", "Must", "Tab in the background for 10 minutes: the slot lives, view current within 2 s", "auto + manual", "H10"),
  A("AC-N3.5", "Must", "Traffic: Duel <= 50 KB/turn, keyframe <= 15 KB; Huge <= 200 KB/turn, keyframe <= 80 KB; up <= 5 KB/turn", "auto (counters)"),
  A("AC-N4.1", "Must", "Login: nick + shared password, token", "auto"),
  A("AC-N4.2", "Must", "Lobby: rooms, settings, slots, ready, start", "auto + manual", "H10"),
  A("AC-N4.3", "Must", "Huge game with 8 human bots and AI starts; everyone gets their own view", "auto"),
  A("AC-N4.4", "Must", "Loading a saved game from the lobby: humans take their slots", "auto"),
  A("AC-N4.5", "Must", "Joining a running game on an AI seat <= 10 s at RTT 200; the others stall <= 1 s", "auto"),
  A("AC-N4.6", "Must", "Drop: seat stays human, turn waits, timer stands; kick; host handover after 60 s", "auto"),
  A("AC-N4.7", "Must", "Simultaneous and hybrid turn modes", "auto (bots)"),
  A("AC-N4.8", "Must", "Turn timer: remaining time visible to all, expiry ends the turn, host pause", "auto"),
  A("AC-N4.9", "Must", "Turn status: who finished; cancel end of turn; an order after one's end of turn cancels it", "manual", "UI part"),
  A("AC-N4.10", "Must", "Chat to all and private; declare war, white peace", "auto + manual", "H10"),
  A("AC-N4.11", "Must", "Orders during the AI phase are queued or refused with a reason", "auto"),
  A("AC-N5.1", "Must", "One game, one worker; several games at once", "auto"),
  A("AC-N5.2", "Must", "`kill -9` of a worker: the game goes on within 30 s, at most a minute of orders lost", "auto"),
  A("AC-N5.3", "Must", "Autosave: start of the human turn, once a minute with orders, ring + every 10th turn", "auto"),
  A("AC-N5.4", "Must", "AI turn time: Duel median <= 0.3 s; Standard x 8 median <= 2 s, max <= 5 s; late Huge median <= 5 s", "auto (measure)"),
  A("AC-N5.5", "Must", "Deployment with one command; an update does not lose games", "manual", "deploy by the user"),
  A("AC-N5.6", "Must", "Ping to the public server measured from Spain and Kazakhstan", "manual", "public deploy step"),
  A("AC-N5.7", "Should", "Server journal per game: logins, orders, step times, bytes", "auto"),
  A("AC-N6.1", "Must", "Endurance: 6 bots, 200 turns, random delays and drops: 0 crashes, 0 stuck turns", "auto (nightly)"),
  A("AC-N6.2", "Must", "B1 playtest scenarios pass through the server", "auto"),
];

/** Milestones are shown with the criteria. */
export const MILESTONES = [
  { id: "M1", priority: "Milestone", criterion: "Two bots play one server game on the test world", method: "auto (`m1`)", manual: "" },
  { id: "M2", priority: "Milestone", criterion: "The same on the real core", method: "auto (`m2`)", manual: "" },
];

/**
 * The AC table of a run.
 * @param {{id: string, ac?: string[], milestone?: string}[]} known metas of every scenario that exists
 * @param {{id: string, status: "pass"|"fail"|"skip", skip?: string}[]} ran results of the selected scenarios
 */
export function acTable(known, ran) {
  const byId = new Map(ran.map((r) => [r.id, r]));
  const rows = [];
  for (const a of [...AC_LIST, ...MILESTONES]) {
    const by = known.filter((m) => (m.ac ?? []).includes(a.id) || m.milestone === a.id).map((m) => m.id);
    const results = by.map((id) => byId.get(id)).filter(Boolean);
    let status = "not-covered";
    let evidence;
    if (a.method === "manual") {
      evidence = "manual acceptance";
    } else if (by.length === 0) {
      evidence = "no scenario yet";
    } else if (results.length === 0) {
      evidence = `not run (${by.join(", ")})`;
    } else if (results.some((r) => r.status === "fail")) {
      status = "fail";
      evidence = results.filter((r) => r.status === "fail").map((r) => r.id).join(", ");
    } else if (results.every((r) => r.status === "pass" && !r.pending?.length) && results.length === by.length) {
      status = "pass";
      evidence = by.join(", ");
    } else {
      const part = results.filter((r) => r.status === "pass").map((r) => r.id);
      const why = results.filter((r) => r.status === "skip").map((r) => `${r.id}: ${r.skip}`);
      why.push(...results.filter((r) => r.status === "pass" && r.pending?.length).map((r) => `${r.id}: pending ${r.pending.join("; ")}`));
      const missing = by.filter((id) => !byId.has(id)).map((id) => `${id}: not run`);
      evidence = [part.length ? `passed ${part.join(", ")}` : "", ...why, ...missing].filter(Boolean).join("; ");
    }
    rows.push({ ...a, by, status, evidence });
  }
  return rows;
}

export function acCounts(rows) {
  const c = { all: { pass: 0, fail: 0, "not-covered": 0 }, must: { pass: 0, fail: 0, "not-covered": 0 } };
  for (const r of rows) {
    if (r.priority === "Milestone") continue;
    c.all[r.status]++;
    if (r.priority === "Must") c.must[r.status]++;
  }
  return c;
}
