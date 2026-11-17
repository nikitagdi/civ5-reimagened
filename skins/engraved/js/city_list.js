(function () {
  "use strict";
  ui.on("city_list.sort", function (column) {
    const current = ui.state.get("city_list.sort");
    ui.state.set("city_list.desc", current === column ? !ui.state.get("city_list.desc") : false);
    ui.state.set("city_list.sort", column);
  });
  ui.on("ready", function () {
    const sort = ui.settings.get("city_list.sort");
    const desc = ui.settings.get("city_list.desc");
    if (["population", "name", "strength", "production"].includes(sort)) ui.state.set("city_list.sort", sort);
    if (typeof desc === "boolean") ui.state.set("city_list.desc", desc);
  });
  ui.state.on("city_list.sort", value => ui.settings.set("city_list.sort", value));
  ui.state.on("city_list.desc", value => ui.settings.set("city_list.desc", value));
})();
