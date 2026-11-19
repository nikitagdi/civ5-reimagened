import { withHud, probe, texts, waitFor, intent } from "./ui2-screen-top_panel.mjs";
import { state } from "../lib/browser.mjs";
import { clickPart } from "./ui2-v1.mjs";

export async function run(context) {
  return withHud(context, "ui2-screen-end_turn", async (page, check) => {
    await waitFor(page, p => texts(p, "end-turn.queue").length > 1, "turn queue");
    await intent(page, "unit.deselect");
    const beforeUnit = (await state(page)).selectedUnit;
    const before = await probe(page);
    if (!await clickPart(page, "helm.dial")) throw new Error("Turn dial missing");
    await waitFor(page, p => p.frames > before.frames + 2, "dial action frame");
    // The HUD root is cleared for a moment when the selection changes (the focus of its query), and comes back with
    // the answer: wait for the queue to come back.
    const after = await waitFor(page, p => texts(p, "end-turn.queue").length > 1, "turn queue after the dial action");
    check("turn queue uses live data", texts(after, "end-turn.queue").length > 1);
    check("dial resolves blocker or changes readiness", after.ui.windows.length > before.ui.windows.length || (await state(page)).selectedUnit !== beforeUnit || texts(after, "helm.state").join() !== texts(before, "helm.state").join());
    // The list of what else blocks the turn (the starting units wait for orders): its entries name the blocker, and
    // `turn.blocker` goes to one (here: selects its unit).
    const listed = await waitFor(page, p => texts(p, "helm.blockers").length > 0, "the list of blockers");
    check("the list of blockers names what waits", texts(listed, "helm.blockers").every(t => t.length > 0));
    await intent(page, "unit.deselect");
    await waitFor(page, p => texts(p, "unit.title").length === 0, "no unit selected");
    // The rows past the first blocker are seq 1, 2, ... in order: the row of a unit that waits selects it.
    const row = texts(listed, "helm.blockers").indexOf("Unit needs orders");
    check("a unit that waits is in the list", row >= 0);
    await intent(page, "turn.blocker", { seq: row + 1 });
    const picked = await waitFor(page, p => texts(p, "unit.title").length > 0, "the entry's unit selected").catch(() => null);
    check("an entry of the list selects its unit", picked != null);
  });
}
