(function () {
  "use strict";
  let submitting = false;
  ui.on("congress.tip", value => ui.state.set("congress.tip", value || ""));
  ui.on("congress.back", () => {
    if (ui.state.get("congress.confirm") !== "none") ui.state.set("congress.confirm", "none");
    else if (ui.state.get("congress.rename")) ui.state.set("congress.rename", false);
    else if (ui.state.get("congress.picker") !== "none") ui.state.set("congress.picker", "none");
    else ui.intent("window.close", { "window": "congress" });
  });
  ui.on("congress.submit", () => {
    const kind = ui.state.get("congress.confirm");
    if (!["votes", "proposals"].includes(kind)) return;
    submitting = true;
    ui.intent("screen.act", { screen: "congress", action: kind === "votes" ? "vote_submit" : "propose_submit" });
    ui.state.set("congress.confirm", "none");
  });
  ui.on("congress.rename", () => {
    const name = ui.state.get("congress.rename_text").trim();
    if (!name || Array.from(name).length > 48) return;
    ui.intent("screen.act", { screen: "congress", action: "rename", text: name });
  });
  ui.on("congress.pick", key => {
    const model = ui.model.get("congress");
    const choices = model && model.league && model.league.choices;
    if (!choices || ui.state.get("congress.picker") === "browse") return;
    const choice = choices.enact.concat(choices.repeal).find(item => item.key === key);
    if (!choice || !choice.enabled) return;
    if (choice.params.length > 1) { ui.state.set("congress.choice", key); return; }
    ui.intent("screen.act", { screen: "congress", action: "propose_set", a: ui.state.get("congress.slot"), b: choice.kind === "repeal" ? 1 : 0, c: choice.id, d: choice.params.length ? choice.params[0].value : -1 });
    ui.state.set("congress.picker", "none");
  });
  ui.model.on("ui.windows.congress.open", open => {
    if (open) {
      submitting = false;
      ui.state.set("congress.picker", "none");
      ui.state.set("congress.confirm", "none");
      ui.state.set("congress.rename", false);
      ui.intent("screen.act", { screen: "congress", action: "open" });
    }
  });
  ui.model.on("congress", model => {
    if (submitting && model && model.league && model.league.mode === "view") {
      submitting = false;
      ui.intent("window.close", { "window": "congress" });
    }
  });
})();
