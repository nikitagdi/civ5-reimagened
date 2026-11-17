(function () {
  "use strict";
  let lastId = -1, wasOpen = false, concessionCount = null;
  function vote(action) {
    const model = ui.model.get("mp_vote");
    if (!model || !model.proposal || !model.proposal.can_vote || ui.state.get("mp_vote.pending")) return;
    ui.state.set("mp_vote.pending", true);
    ui.intent("screen.act", { screen: "mp_vote", action, a: model.proposal.id });
  }
  ui.on("mp-vote.yes", () => vote("vote_yes"));
  ui.on("mp-vote.no", () => vote("vote_no"));
  ui.on("mp-vote.concede", id => {
    const model = ui.model.get("mp_vote");
    if (ui.state.get("mp_vote.pending") || !model || !model.concede || !model.concede.candidates.some(item => item.player_id === id)) return;
    concessionCount = model.open_count;
    ui.state.set("mp_vote.pending", true);
    ui.intent("screen.act", { screen: "mp_vote", action: "concede", a: id });
  });
  ui.on("mp-vote.diplo", id => {
    const model = ui.model.get("mp_vote");
    if (ui.state.get("mp_vote.pending") || !model || !model.diplo_vote || !model.diplo_vote.blocking || !model.diplo_vote.candidates.some(item => item.player_id === id)) return;
    ui.state.set("mp_vote.pending", true);
    ui.intent("screen.act", { screen: "mp_vote", action: "diplo_vote", a: id });
  });
  ui.model.on("ui.pending", rows => ui.state.set("mp_vote.pending", (rows || []).some(row => row.intent === "screen.act")));
  ui.model.on("turn.finished", finished => {
    if (finished && ui.model.get("ui.windows.mp_vote.open")) ui.intent("window.close", { "window": "mp_vote" });
  });
  function refresh(model) {
    if (!model) { lastId = -1; wasOpen = false; concessionCount = null; return; }
    if (model.proposal) {
      if (lastId === model.proposal.id && wasOpen && model.proposal.status !== "open") ui.intent("window.close", { "window": "mp_vote" });
      lastId = model.proposal.id;
      wasOpen = model.proposal.status === "open";
    }
    if (concessionCount !== null && model.open_count > concessionCount) ui.intent("window.close", { "window": "mp_vote" });
    if (model.diplo_vote && !model.diplo_vote.blocking) ui.intent("window.close", { "window": "mp_vote" });
  }
  ui.model.on("mp_vote", refresh);
  ui.on("ready", () => {
    refresh(ui.model.get("mp_vote"));
    const pending = ui.model.get("ui.pending") || [];
    ui.state.set("mp_vote.pending", pending.some(row => row.intent === "screen.act"));
  });
})();
