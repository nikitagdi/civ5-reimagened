(function () {
  "use strict";
  let choice = null;
  ui.on("techs.track", function (id, append) {
    const root = ui.model.get("techs");
    const row = root && root.techs.find(t => t.id === id);
    choice = row && append === 0 ? {id, progress: row.progress, mode: root.mode, opened: root.opened_by} : null;
  });
  ui.model.on("techs", function () {
    const root = ui.model.get("techs");
    if (!root) { choice = null; return; }
    if (!choice) return;
    const row = root.techs.find(t => t.id === choice.id);
    if (!row) return;
    const applied = row.state === "current" || row.state === "researched" || row.progress > choice.progress || root.queue.some(q => q.key === row.key);
    if (!applied) return;
    const close = choice.mode === "steal" || (choice.opened === "request" && ui.settings.get("tech_auto_close") !== false);
    choice = null;
    if (close) ui.intent("window.close", {window: "techs"});
  });
}());
