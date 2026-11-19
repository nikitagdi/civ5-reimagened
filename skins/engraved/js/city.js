(function () {
  "use strict";
  ui.on("city.open", key => {
    const id = Number(String(key).split(":").pop());
    if (Number.isInteger(id)) ui.intent("window.open", { "window": "city", a: id});
  });
  let cityId = null;
  let advance = null;
  ui.model.on("city", function (city) {
    if (!city || city.city !== cityId) {
      cityId = city ? city.city : null;
      // The city view looks at its city: its tiles are worked on the map around it.
      if (city && city.map_city != null) ui.intent("camera.to_city", {city: city.map_city});
      ui.state.set("city.sell", -1);
      ui.state.set("city.hover", -1);
      advance = null;
    }
    if (city && advance && city.city === advance.city && JSON.stringify(city.production.queue) !== advance.queue) {
      const next = advance.next;
      advance = null;
      if (next === null || next === undefined) ui.intent("window.close", {"window": "city"});
      else ui.intent("window.open", {"window": "city", a: next, b: 2});
    }
  });
  ui.on("city.push", function (item) {
    const city = ui.model.get("city");
    if (!city) return;
    if (city.choose_mode) advance = {city: city.city, next: city.next_choose_city, queue: JSON.stringify(city.production.queue)};
    ui.intent("screen.act", {screen: "city", action: "push_order", a: item, b: ui.state.get("city.queue_mode"), c: ui.state.get("city.repeat_order") ? 1 : 0, d: city.city});
  });
  ui.on("ready", function () {
    const setting = ui.settings.get("city.buy_tiles");
    if (typeof setting === "boolean") ui.state.set("city.buy_tiles", setting);
  });
  ui.state.on("city.buy_tiles", function (value) { ui.settings.set("city.buy_tiles", value); });
})();
