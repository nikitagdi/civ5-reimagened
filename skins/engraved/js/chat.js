// The chat dropdown of the strip: the button opens and closes it; the badge counts the lines of the others that came
// while it was closed (opening it reads them).
(function () {
  "use strict";
  let seen = null;
  function seqOf(row) {
    const n = Number(String(row && row.seq).split(":").pop());
    return Number.isFinite(n) ? n : 0;
  }
  function last(rows) {
    let top = 0;
    for (const r of rows || []) top = Math.max(top, seqOf(r));
    return top;
  }
  function update() {
    const rows = ui.model.get("chat") || [];
    if (seen === null || ui.state.get("chat.open")) seen = last(rows);
    let unread = 0;
    for (const r of rows) if (!r.mine && seqOf(r) > seen) unread += 1;
    if (ui.state.get("chat.unread") !== unread) ui.state.set("chat.unread", unread);
  }
  ui.on("chat.toggle", function () {
    ui.state.set("chat.open", !ui.state.get("chat.open"));
    ui.state.set("chat.pick", false);
    update();
  });
  ui.on("ready", update);
  ui.model.on("chat", update);
}());
