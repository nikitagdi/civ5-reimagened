// The scripted player of the browser scenarios (H6 `autoplay`, shared by `saveload`, `smoke`,
// `simp`): plays turns the way a player does it on the screens. Next turn; when something
// blocks, the button goes to it and the policy answers on the panel that opens (a unit's order, a
// city's build).
import { state, act, until, settle } from "./browser.mjs";

/** gc-abi-v1 unit commands (`unit_panel::action_info`). */
export const FOUND_CITY = 3;
export const FORTIFY = 4;
export const SLEEP = 5;
export const SKIP = 6;
export const AUTOMATE = 8;
/** The orders tried for a unit that needs one, first enabled wins. */
const ORDER_PREFERENCE = [FOUND_CITY, AUTOMATE, FORTIFY, SLEEP, SKIP];
/** Builds the policy prefers, else the first one that is not the current build. */
const BUILD_PREFERENCE = ["Settler", "Warrior", "Monument", "Worker", "Scout"];
/** `EndTurnBlockingTypes` the policy answers with a move: units stacked on one plot. */
const BLOCKING_STACKED_UNITS = 7;
/** `EndTurnBlockingTypes` of a city without production. */
export const BLOCKING_PRODUCTION = 2;
/** Forced ends refused in one turn before the game counts as stuck. */
const MAX_FORCED_PER_TURN = 3;
const STEPS_PER_TURN = 40;

/** The six neighbours of an offset-coordinate plot (odd rows shifted right, as the game's grid). */
export function neighbours(x, y) {
  const odd = y & 1;
  const d = odd
    ? [[1, 0], [-1, 0], [0, 1], [1, 1], [0, -1], [1, -1]]
    : [[1, 0], [-1, 0], [-1, 1], [0, 1], [-1, -1], [0, -1]];
  return d.map(([dx, dy]) => [x + dx, y + dy]);
}

export function pickBuild(panel) {
  for (const name of BUILD_PREFERENCE) {
    const b = panel.builds.find((x) => x.name === name && !x.current);
    if (b) return b;
  }
  return panel.builds.find((x) => !x.current) ?? null;
}

function pickOrder(panel) {
  for (const a of ORDER_PREFERENCE) {
    const b = panel.actions.find((x) => x.action === a && x.enabled);
    if (b) return b;
  }
  return null;
}

const sameScreen = (a, b) =>
  a.worlds === b.worlds &&
  a.selectedUnit === b.selectedUnit &&
  !!a.cityPanel === !!b.cityPanel &&
  JSON.stringify(a.blocker) === JSON.stringify(b.blocker) &&
  a.unitPanel?.loading === b.unitPanel?.loading &&
  a.cityPanel?.loading === b.cityPanel?.loading;

/**
 * A player on `page`. `player.play(target)` plays until the human's turn `target` (or the game
 * is over / stuck) and returns the last state. `player.observe` (optional) sees every state the
 * policy looked at (the scenarios collect end-turn texts and journal lines with it).
 * @param {import("playwright").Page} page
 * @param {{turnTimeoutMs: number, verbose?: boolean, observe?: (s: object) => void}} o
 */
export function createPlayer(page, o) {
  const player = {
    policy: { builds: 0, orders: {}, moves: { tried: 0, moved: 0 }, forced: 0, confirms: 0 },
    turns: [],
    stuck: null,
    explored: false,
  };
  const look = (s) => {
    if (s && o.observe) o.observe(s);
    return s;
  };

  /** A right click on a neighbouring plot, the next one until the unit stands elsewhere. */
  player.moveAway = async (unit) => {
    for (const [x, y] of neighbours(unit.x, unit.y)) {
      player.policy.moves.tried++;
      await act(page, { kind: "click", x, y, secondary: true });
      const next = look(
        await settle(
          page,
          (v) => {
            const u = v.units.find((w) => w.id === unit.id);
            return v.status !== "human" || !!v.question || !u || u.x !== unit.x || u.y !== unit.y;
          },
          4000,
        ),
      );
      if (next?.question) {
        player.policy.confirms++;
        await act(page, { kind: "confirm", yes: false });
        continue;
      }
      const u = next?.units.find((w) => w.id === unit.id);
      if (u && (u.x !== unit.x || u.y !== unit.y)) {
        player.policy.moves.moved++;
        return true;
      }
      if (next?.status !== "human") return false;
    }
    return false;
  };

  player.play = async (target) => {
    let last = null;
    turns: while (true) {
      let s = look(await until(page, (x) => x.status === "human" || x.status === "over", o.turnTimeoutMs, "human turn"));
      last = s;
      if (s.status === "over" || s.turn >= target) break;
      const turn = s.turn;
      const record = { turn, actions: 0, forced: false };
      let lastBlocker = null;
      let sameBlocker = 0;
      let forced = 0;
      for (let step = 0; ; step++) {
        s = look(await state(page));
        if (!s) {
          await new Promise((r) => setTimeout(r, 50));
          continue;
        }
        if (s.status !== "human" || s.turn !== turn) break;
        if (step >= STEPS_PER_TURN || sameBlocker >= 3) {
          if (forced >= MAX_FORCED_PER_TURN) {
            // STUCK: neither the policy nor a forced end moves the turn on.
            player.stuck = { turn, blocker: s.blocker, endTurnNote: s.endTurnNote };
            player.turns.push(record);
            break turns;
          }
          // The policy cannot clear it: end the turn anyway (Shift+click).
          forced++;
          record.forced = true;
          player.policy.forced++;
          await act(page, { kind: "endTurn", force: true });
          look(await settle(page, (x) => x.status !== "human" || x.turn !== turn, 5000));
          continue;
        }
        record.actions++;
        let next;
        if (s.question) {
          player.policy.confirms++;
          await act(page, { kind: "confirm", yes: false });
          next = await settle(page, (x) => !x.question, 5000);
        } else if (s.cityPanel && s.cityPanel.loading) {
          next = await settle(page, (x) => !x.cityPanel || !x.cityPanel.loading, 10000);
        } else if (s.cityPanel && s.blocker?.kind === BLOCKING_PRODUCTION) {
          const build = pickBuild(s.cityPanel);
          if (build) {
            player.policy.builds++;
            await act(page, { kind: "chooseBuild", index: build.index });
          } else {
            await act(page, { kind: "closeCity" });
          }
          next = await settle(page, (x) => !sameScreen(s, x), 10000);
        } else if (s.cityPanel) {
          await act(page, { kind: "closeCity" });
          next = await settle(page, (x) => !x.cityPanel, 5000);
        } else if (s.unitPanel && s.unitPanel.loading) {
          next = await settle(page, (x) => !x.unitPanel || !x.unitPanel.loading, 10000);
        } else if (s.unitPanel && s.selectedUnit !== null && s.blocker) {
          const unit = s.units.find((u) => u.id === s.selectedUnit);
          const order = pickOrder(s.unitPanel);
          const stacked = s.blocker.kind === BLOCKING_STACKED_UNITS;
          const military = order && order.action !== FOUND_CITY && order.action !== AUTOMATE;
          if (unit && unit.movesLeft > 0 && (stacked || (!player.explored && military))) {
            // Stacked units: one steps aside. Once per game a military unit walks a step too (the
            // map's move path).
            player.explored ||= !stacked;
            await player.moveAway(unit);
            next = await state(page);
          } else if (order) {
            player.policy.orders[order.label] = (player.policy.orders[order.label] ?? 0) + 1;
            await act(page, { kind: "unitAction", action: order.action });
            next = await settle(page, (x) => !sameScreen(s, x), 10000);
          } else {
            sameBlocker = 3; // nothing to order: forced next step
            continue;
          }
        } else {
          // Next turn: ends it, or goes to what blocks it (selects the unit, opens the city).
          await act(page, { kind: "endTurn", force: false });
          next = await settle(page, (x) => !sameScreen(s, x) || x.status !== "human", 10000);
        }
        look(next);
        const key = JSON.stringify(next?.blocker ?? null);
        if (next?.blocker && key === lastBlocker) sameBlocker++;
        else sameBlocker = 0;
        lastBlocker = key;
      }
      if (!player.stuck) player.turns.push(record);
      if (o.verbose) console.log(`turn ${turn}: ${record.actions} actions${record.forced ? ", forced" : ""}`);
    }
    return (await state(page)) ?? last;
  };
  return player;
}

/** The per-turn times of `metrics()` merged into the player's turn records. */
export function mergeTurnTimes(turns, metrics) {
  const times = new Map((metrics?.turns ?? []).map((t) => [t.turn, t]));
  for (const t of turns) {
    t.wallMs = times.get(t.turn)?.wallMs ?? null;
    t.aiMs = times.get(t.turn)?.aiMs ?? null;
  }
}
