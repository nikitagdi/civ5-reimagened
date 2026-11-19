(function () {
  "use strict";
  ui.on("unit_lists.open", function (section) {
    ui.intent("window.close", {"window": "city_list"});
    ui.state.set("tech_panel.open", section === "tech" && !ui.state.get("tech_panel.open"));
    ui.state.set("unit_lists.section", section === "tech" ? "" : section);
    ui.state.set("unit_lists.sort", "name");
    ui.state.set("unit_lists.desc", false);
  });
  // The strip's lists button: opens the info corner on the unit list, or closes whatever it shows.
  ui.on("unit_lists.toggle", function () {
    const open = ui.state.get("unit_lists.section") !== "" || !!ui.state.get("tech_panel.open");
    ui.intent("window.close", {"window": "city_list"});
    ui.state.set("tech_panel.open", false);
    ui.state.set("unit_lists.section", open ? "" : "units");
    ui.state.set("unit_lists.sort", "name");
    ui.state.set("unit_lists.desc", false);
  });
}());
