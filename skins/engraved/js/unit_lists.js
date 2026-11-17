(function () {
  "use strict";
  ui.on("unit_lists.open", function (section) {
    ui.state.set("tech_panel.open", section === "tech");
    ui.state.set("unit_lists.section", section === "tech" ? "" : section);
    ui.state.set("unit_lists.sort", "name");
    ui.state.set("unit_lists.desc", false);
  });
}());
