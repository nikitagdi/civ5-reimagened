// Turn loop of the M4 WO-E scenario and its dev REPL: END TURN with what blocks it handled the way a player does (production: the
// city screen, units: fortify). A research block is answered with the message the tech tree sends (`Network.SendResearch`),
// the tree itself is the subject of K9 (WO-B), not of this work order.
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const BLOCK_RESEARCH = 1;
const BLOCK_STACKED = 7;
const BLOCK_POLICY = 0;

/** @param {{page: import("playwright").Page}} b  @param {ReturnType<typeof import("./ui-driver.mjs").uiDriver>} ui
 *  @param {{research?: () => Promise<string>, policy?: () => Promise<string>, block?: (bl: {type: number, turn: number}) => Promise<string|null>,
 *    [type: number]: (bl: {type: number, turn: number}) => Promise<string>}} [hooks]  (WO-J) research / policy blocks answered through
 *  the windows instead of the messages; (WO-L) a number key handles that blocking type (by `GetEndTurnBlockingType`) */
export function turnDriver(b, ui, shot = async () => {}, hooks = {}) {
  const handled = [];
  /** One entry per END TURN that advanced the turn: {turn, ms} = the click to the new turn on the replica (WO-J, polled every 250 ms). */
  const timings = [];
  const turnNow = async () => (await b.page.evaluate(() => window.__civ5stats?.replica?.turn ?? 0));

  async function chooseResearch() {
    if (hooks.research) return hooks.research();
    const [r] = await ui.lua(`
      local p = Players[Game.GetActivePlayer()]
      local pick
      for t in GameInfo.Technologies() do if p:CanResearch(t.ID) then pick = t break end end
      if pick then Network.SendResearch(pick.ID, 0, -1, false) end
      print(TAG .. tostring(pick and pick.Type))`);
    return `research: ${r}`;
  }

  /** The warriors the capital keeps building pile up on its plot ("move stacked unit"): all but one are disbanded by the
   *  command message the unit panel's delete button sends. */
  async function thinWarriors() {
    const r = await ui.lua(`
      local p = Players[Game.GetActivePlayer()]
      local seen, n = false, 0
      for u in p:Units() do
        if u:GetUnitType() == GameInfoTypes.UNIT_WARRIOR then
          if seen then Network.SendDoCommand(u:GetID(), CommandTypes.COMMAND_DELETE, 0, 0, false) n = n + 1 else seen = true end
        end
      end
      print(TAG .. n)`);
    return `stacked: disbanded ${r[0]} warriors`;
  }

  /** A policy to adopt blocks the turn (culture piles up over 25 turns): the first adoptable one, by the tree's own message. */
  async function adoptPolicy() {
    if (hooks.policy) return hooks.policy();
    const r = await ui.lua(`
      local ok, err = pcall(function()
      local p = Players[Game.GetActivePlayer()]
      local pick
      for t in GameInfo.Policies() do
        if not pick and t.PolicyBranchType ~= nil and p:CanAdoptPolicy(t.ID) then pick = t end
      end
      if not pick then
        for t in GameInfo.PolicyBranchTypes() do
          if not pick and p.CanUnlockPolicyBranch and p:CanUnlockPolicyBranch(t.ID) then
            Network.SendUpdatePolicies(t.ID, false, true)
            print(TAG .. "branch " .. t.Type) return
          end
        end
      end
      if pick then Network.SendUpdatePolicies(pick.ID, true, true) end
      print(TAG .. tostring(pick and pick.Type) .. " culture=" .. p:GetJONSCulture() .. " cost=" .. p:GetNextPolicyCost() .. " free=" .. tostring(p.GetNumFreePolicies and p:GetNumFreePolicies()))
      end)
      if not ok then print(TAG .. "ERR " .. tostring(err)) end`);
    return `policy: ${r[0]}`;
  }

  /** One END TURN; resolves with the new turn number once the turn advanced. */
  async function endTurn() {
    const at = await turnNow();
    for (let step = 0; step < 24; step++) {
      const bl = await ui.blocking();
      let note = null;
      if (bl.type === -1) {
        await ui.clickEndTurn();
        const t0 = Date.now();
        while (Date.now() - t0 < 60000) {
          await sleep(250);
          if ((await turnNow()) > at) { timings.push({ turn: at, ms: Date.now() - t0 }); await sleep(3000); return at + 1; }
        }
        handled.push(`turn ${at}: END TURN did not advance`);
        continue;
      }
      if (bl.type === BLOCK_RESEARCH) note = await chooseResearch();
      else if (bl.type === BLOCK_POLICY) note = await adoptPolicy();
      else if (bl.type === BLOCK_STACKED) note = await thinWarriors();
      else if (hooks[bl.type]) note = await hooks[bl.type](bl);
      else note = (hooks.block && (await hooks.block(bl))) || (await ui.handleBlock(bl));
      handled.push(`turn ${at}: ${note}`);
      await sleep(2500);
      if ((await turnNow()) > at) { await sleep(3000); return at + 1; }
    }
    await shot(`stuck-${at}`);
    throw new Error(`the turn did not advance from ${at}: ${handled.slice(-4).join("; ")}`);
  }

  /** Clears the blockers a player would clear first (policy, research, production, units, stacked units); returns the one left. */
  async function clearOrdinary() {
    for (let i = 0; i < 12; i++) {
      const bl = await ui.blocking();
      let n = null;
      if (bl.type === BLOCK_RESEARCH) n = await chooseResearch();
      else if (bl.type === BLOCK_POLICY) n = await adoptPolicy();
      else if (bl.type === BLOCK_STACKED) n = await thinWarriors();
      else if (bl.type === 2 || bl.type === 3) n = (hooks.block && (await hooks.block(bl))) || (await ui.handleBlock(bl));
      else return bl;
      handled.push(`turn ${bl.turn}: ${n}`);
      await sleep(2500);
    }
    return ui.blocking();
  }

  async function toTurn(n) {
    while ((await turnNow()) < n) await endTurn();
  }
  return { endTurn, toTurn, clearOrdinary, turnNow, handled, timings };
}
