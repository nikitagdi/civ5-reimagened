// The lobby list and the room as plain DOM (web only; the native client draws them with egui).
//
// Contract with the wasm (client-app, `dom_ui.rs`):
//   window.__civ5ui.render(screen, json)  screen: "login" | "lobby" | "room" | "none"; called when the view changed
//                                         "art": the game art of these screens, once (`lobby_art.rs`):
//                                         {"urls": {img id: url}, "art": {colors, icons, civs}} (asset set `web:lobby`)
//   window.__civ5ui.drain()               -> array of JSON texts posted since the last call (one call per frame)
//   window.__civ5ui.post(obj)             queues one action (the shapes of interim-ui `DomAction`)
// The views are the serde output of interim-ui's LoginView, LobbyListView and RoomView. Rendering builds a fresh
// tree and morphs it into the page in place, so typing, focus, open selects and scroll survive a redraw.
//
// P15 (lobby UI): game text markup ([COLOR_*]...[ENDCOLOR], [ICON_*], [NEWLINE]) is drawn as the game draws it
// (the colours of the game's Colors table, the glyphs of its icon font); the civilization tip is a framed tooltip
// with the leader portrait, the civ icon and the unique items from the game's icon atlases; the lobby has tabs, an
// online players panel and the saves grouped per game.
(function () {
  "use strict";

  const egui = new URLSearchParams(location.search).get("egui") === "1";
  const queue = [];
  const overlay = document.getElementById("civ5-ui");
  const main = document.getElementById("civ5-main");
  const statusEl = document.getElementById("civ5-status");
  const state = { auth: false, main: false };

  function sync() {
    const show = !egui && (state.auth || state.main);
    overlay.hidden = !show;
    if (statusEl) statusEl.hidden = show;
    if (!show) hideTip();
  }

  const ui = {
    post(obj) {
      if (!egui) queue.push(JSON.stringify(obj));
    },
    drain() {
      return queue.length ? queue.splice(0, queue.length) : [];
    },
    setAuthVisible(v) {
      state.auth = !!v;
      sync();
    },
    render(screen, json) {
      if (egui) return;
      try {
        renderScreen(screen, json);
      } catch (e) {
        console.error("civ5 ui render:", e);
      }
    },
  };
  window.__civ5ui = ui;

  // ------------------------------------------------------------------------------------------ DOM helpers

  /** Element builder. Keys starting with "on" are handlers; `value` and `checked` are set as properties. */
  function h(tag, attrs, ...kids) {
    const el = document.createElement(tag);
    let value, checked;
    for (const k in attrs || {}) {
      const v = attrs[k];
      if (v == null || v === false) continue;
      if (k === "value") value = v;
      else if (k === "checked") checked = true;
      else if (k.startsWith("on")) el[k] = v;
      else if (k === "class") el.className = v;
      else el.setAttribute(k, v === true ? "" : String(v));
    }
    add(el, kids);
    if (value !== undefined) el.value = value;
    if (checked) el.checked = true;
    return el;
  }
  function add(el, kids) {
    for (const k of kids) {
      if (k == null || k === false) continue;
      if (Array.isArray(k)) add(el, k);
      else el.append(k instanceof Node ? k : document.createTextNode(String(k)));
    }
  }

  const HANDLERS = ["onclick", "onchange", "ontoggle", "onkeydown"];

  /** Makes `o` (in the page) look like `n` (fresh); returns the node that stays. */
  function morph(o, n) {
    if (o.nodeType !== n.nodeType || o.nodeName !== n.nodeName || (o.dataset && o.dataset.k) !== (n.dataset && n.dataset.k)) {
      o.replaceWith(n);
      return n;
    }
    if (n.nodeType === 3) {
      if (o.nodeValue !== n.nodeValue) o.nodeValue = n.nodeValue;
      return o;
    }
    const keep = o.nodeName === "DETAILS" ? "open" : "";
    for (const a of Array.from(o.attributes)) if (a.name !== keep && !n.hasAttribute(a.name)) o.removeAttribute(a.name);
    for (const a of Array.from(n.attributes)) if (a.name !== keep && o.getAttribute(a.name) !== a.value) o.setAttribute(a.name, a.value);
    for (const hk of HANDLERS) o[hk] = n[hk] || null;
    morphKids(o, n);
    const focused = document.activeElement === o;
    if (o.nodeName === "INPUT") {
      if (o.type === "checkbox" || o.type === "radio") {
        if (o.checked !== n.checked) o.checked = n.checked;
      } else if (!focused && o.value !== n.value) o.value = n.value;
    } else if (o.nodeName === "SELECT" && !focused && o.value !== n.value) o.value = n.value;
    return o;
  }
  function morphKids(o, n) {
    const fresh = Array.from(n.childNodes);
    fresh.forEach((c, i) => {
      const old = o.childNodes[i];
      if (old) morph(old, c);
      else o.append(c);
    });
    while (o.childNodes.length > fresh.length) o.lastChild.remove();
  }

  const store = {
    get(k, d) {
      try {
        const v = localStorage.getItem("civ5ui." + k);
        return v == null ? d : JSON.parse(v);
      } catch (_) {
        return d;
      }
    },
    set(k, v) {
      try {
        localStorage.setItem("civ5ui." + k, JSON.stringify(v));
      } catch (_) {}
    },
  };

  // ------------------------------------------------------------------------------------------- game art
  //
  // The table of the asset set (`web:lobby`): colours, icon-font glyph rects, per civ the leader portrait, civ icon
  // and unique items as cells of small PNG sheets. The sheets are fetched once each and kept as object URLs (typed
  // image/png, whatever the server's content type is). Without the table the page shows plain text.

  const ART = { art: null, urls: {}, sheets: new Map() };

  function setArt(json) {
    const v = JSON.parse(json);
    ART.art = v && v.art ? v.art : null;
    ART.urls = (v && v.urls) || {};
    redraw();
  }

  let redrawQueued = false;
  function redraw() {
    if (redrawQueued) return;
    redrawQueued = true;
    requestAnimationFrame(() => {
      redrawQueued = false;
      draw();
      refreshTip();
    });
  }

  /** The object URL of a sheet, or null while it loads (a redraw follows) or when there is none. */
  function sheet(id) {
    const got = ART.sheets.get(id);
    if (got !== undefined) return got || null;
    const url = ART.urls[id];
    if (!url) {
      ART.sheets.set(id, false);
      return null;
    }
    ART.sheets.set(id, null);
    fetch(url)
      .then((r) => (r.ok ? r.arrayBuffer() : Promise.reject(new Error("HTTP " + r.status))))
      .then((b) => {
        ART.sheets.set(id, URL.createObjectURL(new Blob([b], { type: "image/png" })));
        redraw();
      })
      .catch((e) => {
        console.warn("civ5 ui: picture " + id + " unavailable:", e && e.message);
        ART.sheets.set(id, false);
      });
    return null;
  }

  /** A cell of a sheet drawn `size` px wide (height by the cell's aspect); an empty box of that size while it loads. */
  function sprite(cell, size, cls) {
    if (!cell) return null;
    const hgt = Math.round((size * cell.h) / cell.w);
    const box = "width:" + size + "px;height:" + hgt + "px";
    const url = sheet(cell.img);
    if (!url) return h("span", { class: "spr " + (cls || ""), style: box, "aria-hidden": "true" });
    return h(
      "span",
      { class: "spr " + (cls || ""), style: box, "aria-hidden": "true" },
      h("span", {
        class: "cell",
        style:
          "width:" + cell.w + "px;height:" + cell.h + "px;background:url(" + url + ") -" + cell.x + "px -" + cell.y +
          "px no-repeat;transform:scale(" + size / cell.w + ")",
      }),
    );
  }

  const civArt = (key) => (ART.art && ART.art.civs && key ? ART.art.civs[key] : null) || null;
  const civIcon = (key, size) => {
    const a = civArt(key);
    return a && a.icon ? sprite(a.icon, size, "civicon") : h("span", { class: "civicon none", style: "width:" + size + "px;height:" + size + "px" });
  };

  // ------------------------------------------------------------------------------------------ text markup
  //
  // As the game's text control: [NEWLINE] breaks, [COLOR_X] ... [ENDCOLOR] colours from the Colors table (nested),
  // [ICON_X] is a glyph of the icon font, [TAB] a space; every other tag is dropped. SIMPLIFIED(SIMP-exe-1054): no
  // [STYLE_*] fonts and no [COLOR:r:g:b:a] alpha blend over the control colour; an unknown colour keeps the text colour.

  const TAG = /\[([A-Za-z][A-Za-z0-9_:.]{0,63})\]/g;

  function colorCss(name) {
    const up = name.toUpperCase();
    if (up.startsWith("COLOR:")) {
      const p = up.slice(6).split(":").map(Number);
      if (p.length >= 3 && p.every((x) => Number.isFinite(x))) return "rgba(" + p[0] + "," + p[1] + "," + p[2] + "," + (p.length > 3 ? p[3] / 255 : 1) + ")";
      return null;
    }
    const c = ART.art && ART.art.colors ? ART.art.colors[up] : null;
    return c ? "rgba(" + c[0] + "," + c[1] + "," + c[2] + "," + (c[3] / 255).toFixed(3) + ")" : null;
  }

  /** The game text `text` as nodes. */
  function markup(text) {
    if (text == null) return [];
    const s = String(text);
    const root = { kids: [] };
    const stack = [root];
    const top = () => stack[stack.length - 1];
    const pushText = (t) => {
      const lines = t.split("\n");
      lines.forEach((l, i) => {
        if (i) top().kids.push(h("br"));
        if (l) top().kids.push(l);
      });
    };
    let last = 0;
    let m;
    TAG.lastIndex = 0;
    while ((m = TAG.exec(s))) {
      if (m.index > last) pushText(s.slice(last, m.index));
      last = TAG.lastIndex;
      const tag = m[1];
      const up = tag.toUpperCase();
      if (up === "NEWLINE") top().kids.push(h("br"));
      else if (up === "TAB" || up === "SPACE") top().kids.push(" ");
      else if (up === "ENDCOLOR") {
        if (stack.length > 1) stack.pop();
      } else if (up.startsWith("COLOR_") || up.startsWith("COLOR:")) {
        const node = { color: colorCss(up), kids: [] };
        top().kids.push(node);
        stack.push(node);
      } else if (up.startsWith("ICON_")) {
        const cell = ART.art && ART.art.icons ? ART.art.icons[up] : null;
        if (cell) top().kids.push(h("span", { class: "gicon", title: up.slice(5).toLowerCase().replace(/_/g, " ") }, sprite(cell, Math.round((18 * cell.w) / Math.max(cell.w, cell.h)))));
      }
      // any other tag: dropped
    }
    if (last < s.length) pushText(s.slice(last));
    const build = (n) => n.kids.map((k) => (k && k.kids ? h("span", { class: "gc", style: k.color ? "color:" + k.color : null }, build(k)) : k));
    return build(root);
  }

  /** The game text without its markup (for native <option> labels and titles). */
  function plain(text) {
    return String(text == null ? "" : text)
      .replace(/\[NEWLINE\]/gi, " ")
      .replace(TAG, "")
      .replace(/\s+/g, " ")
      .trim();
  }

  // --------------------------------------------------------------------------------------------- widgets

  const post = (a) => ui.post(a);
  const edit = (e) => ui.post({ RoomEdit: e });

  const openState = new Map();
  const wide = () => window.matchMedia("(min-width: 900px)").matches;

  /** A <details> whose open state survives redraws. `dflt`: open the first time on a phone. */
  function details(id, cls, title, dflt, body, extra) {
    if (!openState.has(id)) openState.set(id, wide() || dflt);
    return h(
      "details",
      { class: cls, "data-k": id, open: openState.get(id), ontoggle: (e) => openState.set(id, e.target.open) },
      h("summary", null, h("span", { class: "sumtitle" }, title), extra || null),
      h("div", { class: "body" }, body),
    );
  }

  // An option may carry a third item: its hover tip.
  function sel(opts, cur, on, dis, label) {
    const list = opts.slice();
    if (!list.some(([v]) => String(v) === String(cur))) list.unshift([cur, String(cur)]);
    const chosen = list.find(([v]) => String(v) === String(cur));
    return h(
      "select",
      { "aria-label": label, title: (chosen && plain(chosen[2])) || "", disabled: dis, value: String(cur), onchange: (e) => on(e.target.value) },
      list.map(([v, l, tip]) => h("option", { value: String(v), title: plain(tip) || "" }, plain(l))),
    );
  }
  const keyed = (list) => list.map((c) => [c.key, c.name]);

  function field(label, control, sub) {
    return h("div", { class: "field" }, h("span", { class: "lab" }, markup(label)), control, sub ? h("div", { class: "sub" }, markup(sub)) : null);
  }
  function check(label, on, onchange, dis, help, k) {
    const same = help && plain(help) === plain(label);
    return h(
      "label",
      { class: "check", "data-k": k },
      h("input", { type: "checkbox", checked: on, disabled: dis, onchange: (e) => onchange(e.target.checked) }),
      h("span", { class: "txt" }, h("span", { class: "name" }, markup(label)), help && !same ? h("div", { class: "sub" }, markup(help)) : null),
    );
  }
  function num(val, min, max, on, dis, label) {
    return h("input", {
      type: "number",
      inputmode: "numeric",
      min,
      max,
      value: val == null ? "" : String(val),
      disabled: dis,
      "aria-label": label,
      onchange: (e) => on(e.target.value.trim() === "" ? null : Math.min(max, Math.max(min, Math.round(Number(e.target.value) || 0)))),
    });
  }
  const btn = (text, onclick, opts) => h("button", Object.assign({ type: "button", onclick }, opts || {}), text);

  /** The emblem: a hexagon (the map's cell) with a laurel-like chevron, drawn in SVG. */
  function emblem(size) {
    const ns = "http://www.w3.org/2000/svg";
    const svg = document.createElementNS(ns, "svg");
    svg.setAttribute("viewBox", "0 0 40 40");
    svg.setAttribute("width", size);
    svg.setAttribute("height", size);
    svg.setAttribute("class", "emblem");
    svg.setAttribute("aria-hidden", "true");
    svg.innerHTML =
      '<path d="M20 2 35.6 11v18L20 38 4.4 29V11z" fill="none" stroke="currentColor" stroke-width="1.6"/>' +
      '<path d="M20 7.5 30.8 13.7v12.6L20 32.5 9.2 26.3V13.7z" fill="currentColor" opacity=".14"/>' +
      '<path d="M13 23.5c2.2 2.6 4.6 3.9 7 3.9s4.8-1.3 7-3.9M14.5 18.5c1.6 1.9 3.5 2.8 5.5 2.8s3.9-.9 5.5-2.8M20 11v16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>';
    return svg;
  }

  // ------------------------------------------------------------------------------------------------ render

  let last = null;
  const CONN = { Green: "Online", Yellow: "Poor connection", Red: "No connection", Grey: "Closed" };

  function renderScreen(screen, json) {
    if (screen === "art") {
      setArt(json);
      return;
    }
    if (screen === "login") {
      state.main = false;
      window.__civ5auth.show(json ? JSON.parse(json) : null);
    } else if (screen === "lobby" || screen === "room") {
      window.__civ5auth.hide();
      state.main = true;
      if (!last || last.screen !== screen) {
        picker = null;
        hideTip();
      }
      last = { screen, view: JSON.parse(json) };
      draw();
      refreshTip();
    } else {
      // "none": the canvas shows something else. The login page that waits for a first sign-in stays.
      state.main = false;
    }
    sync();
  }

  function draw() {
    if (!last) return;
    const tree = h("div", { id: "civ5-main" }, last.screen === "lobby" ? lobbyPage(last.view) : roomPage(last.view));
    morph(main, tree) === main || console.error("civ5 ui: root replaced");
  }

  // --------------------------------------------------------------------------------------------------- lobby

  function topBar(title, sub, v) {
    return h(
      "header",
      { class: "top" },
      h("div", { class: "brand" }, emblem(36), h("div", null, h("h1", null, title), sub ? h("div", { class: "kicker" }, sub) : null)),
      h(
        "div",
        { class: "who-bar" },
        h("span", { class: "conn", title: CONN[v.conn.dot] || "" }, h("span", { class: "dot " + v.conn.dot }), h("span", { class: "conn-txt" }, CONN[v.conn.dot] || "")),
        v.nick ? h("span", { class: "me" }, v.nick) : null,
        btn(
          "Sign out",
          () => {
            if (window.__civ5auth) window.__civ5auth.logout();
            post("SignOut");
          },
          { class: "quiet" },
        ),
      ),
    );
  }

  let tab = store.get("tab", "rooms");

  function lobbyPage(v) {
    const seat = v.my_seat;
    const groups = saveGroups(v.saves);
    const tabs = [
      ["rooms", "Rooms", "Rooms", v.rooms.length],
      ["games", "Games in progress", "Games", v.games.length],
      ["saves", "Saved games", "Saves", groups.length],
    ];
    if (!tabs.some((t) => t[0] === tab)) tab = "rooms";
    const setTab = (t) => {
      tab = t;
      store.set("tab", t);
      draw();
    };
    return h(
      "div",
      { class: "page lobby" },
      topBar("Multiplayer", "civ5-reimagened", v),
      v.conn.status ? h("div", { class: "err" }, v.conn.status) : null,
      v.note ? h("div", { class: "note" }, v.note) : null,
      v.error ? h("div", { class: "err", role: "alert" }, v.error) : null,
      seat ? seatLine(seat, v.confirm_give_up) : null,
      h(
        "div",
        { class: "lobby-grid" },
        h(
          "main",
          { class: "lobby-main" },
          h(
            "section",
            { class: "hero frame" },
            h("div", { class: "hero-txt" }, h("h2", null, "Start a game"), h("p", null, "Open a room for friends and AI, or jump straight into a game against one AI.")),
            h(
              "div",
              { class: "hero-act" },
              btn("Create room", () => post("CreateRoom"), { class: "primary big" }),
              btn("Quick start", () => post("QuickStart"), { class: "big", title: "A game against one AI, no waiting" }),
            ),
          ),
          h(
            "nav",
            { class: "ltabs", role: "tablist", "aria-label": "Lists" },
            tabs.map(([k, label, short, n]) =>
              h(
                "button",
                { type: "button", role: "tab", "aria-selected": tab === k ? "true" : "false", "data-k": "tab-" + k, onclick: () => setTab(k) },
                h("span", { class: "long" }, label),
                h("span", { class: "short" }, short),
                h("span", { class: "count" }, String(n)),
              ),
            ),
          ),
          h(
            "div",
            { class: "tabpanel", role: "tabpanel", "data-k": "panel-" + tab },
            tab === "rooms" ? roomsList(v) : tab === "games" ? gamesList(v) : savesList(groups, (s) => post({ LoadSave: s.id }), "lobby"),
          ),
        ),
        onlinePanel(v),
      ),
    );
  }

  function empty(title, text) {
    return h("div", { class: "empty" }, emblem(44), h("div", { class: "empty-title" }, title), h("div", { class: "empty-txt" }, text));
  }

  function roomsList(v) {
    if (!v.rooms.length) return empty("No open rooms", "Create one and invite your friends, or take a quick game against the AI.");
    return v.rooms.map((r) =>
      h(
        "article",
        { class: "card room-card", "data-k": "r" + r.id },
        h(
          "div",
          { class: "card-main" },
          h("div", { class: "title" }, r.name),
          h("div", { class: "facts" }, h("span", null, "Host ", h("b", null, r.host)), h("span", { class: "chip" }, r.players + " players"), h("span", null, r.map)),
        ),
        r.starting ? h("span", { class: "status warn" }, "Starting") : btn("Join", () => post({ JoinRoom: r.id }), { class: "primary" }),
      ),
    );
  }

  function gamesList(v) {
    if (!v.games.length) return empty("No games in progress", "Running and sleeping games show here; you can take an AI seat in one.");
    return v.games.map(gameCard);
  }

  function seatLine(seat, confirm) {
    if (confirm) {
      return h(
        "div",
        { class: "banner frame" },
        h("div", { class: "banner-txt" }, "Give up your seat in ", h("b", null, seat.game_name), "? The AI plays it from now on."),
        h("div", { class: "row" }, btn("Yes, give up", () => post({ ConfirmGiveUp: true }), { class: "danger" }), btn("Keep the seat", () => post({ ConfirmGiveUp: false }))),
      );
    }
    return h(
      "div",
      { class: "banner frame" },
      h("div", { class: "banner-txt" }, "You have a seat in ", h("b", null, seat.game_name), ", turn " + seat.turn),
      h(
        "div",
        { class: "row" },
        seat.can_return ? btn("Return to the game", () => post({ TakeSeat: { game: seat.game, player: seat.player } }), { class: "primary" }) : null,
        btn("Give up seat", () => post("GiveUpSeat"), { class: "quiet" }),
      ),
    );
  }

  const STATUS_CLASS = (s) => (/^running/.test(s) ? "ok" : /finished/.test(s) ? "" : "warn");

  function gameCard(g) {
    return h(
      "article",
      { class: "card game-card", "data-k": "g" + g.id },
      h(
        "div",
        { class: "card-head" },
        h("div", { class: "title" }, g.name),
        h("div", { class: "facts" }, h("span", { class: "turn" }, "Turn " + g.turn), h("span", null, g.map), h("span", { class: "status " + STATUS_CLASS(g.status) }, g.status)),
      ),
      h(
        "div",
        { class: "seats" },
        g.seats.map((s) =>
          h(
            "div",
            { class: "seat" + (s.yours ? " yours" : ""), "data-k": "p" + s.player },
            h("span", { class: "dot " + (s.online ? "Green" : "Grey"), title: s.online ? "online" : "offline" }),
            h("span", { class: "seat-civ" }, plain(s.civ)),
            h("span", { class: "seat-who" }, s.who + (s.yours ? " (yours)" : "")),
            s.takeable ? btn("Take", () => post({ TakeSeat: { game: g.id, player: s.player } }), { class: "small", title: "Take " + plain(s.civ) }) : null,
          ),
        ),
      ),
    );
  }

  /** The saves per game, newest game first (the server lists the saves newest first). */
  function saveGroups(saves) {
    const by = new Map();
    for (const s of saves || []) {
      const key = s.game != null ? "g" + s.game : "n" + s.name;
      if (!by.has(key)) by.set(key, { key, name: s.name, host: s.host || "", saves: [] });
      by.get(key).saves.push(s);
    }
    return Array.from(by.values());
  }

  function savesList(groups, onLoad, where) {
    if (!groups.length) return empty("No saved games", "Games are saved every turn; their saves show here grouped by game.");
    return groups.map((g, i) => {
      const newest = g.saves[0];
      const id = where + "-sg-" + g.key;
      if (!openState.has(id)) openState.set(id, i === 0);
      return h(
        "details",
        { class: "save-group card", "data-k": id, open: openState.get(id), ontoggle: (e) => openState.set(id, e.target.open) },
        h(
          "summary",
          null,
          h(
            "div",
            { class: "card-main" },
            h("div", { class: "title" }, g.name),
            h(
              "div",
              { class: "facts" },
              g.host ? h("span", null, "Host ", h("b", null, g.host)) : null,
              h("span", { class: "turn" }, "Turn " + newest.turn),
              h("span", null, newest.saved),
              h("span", { class: "chip" }, g.saves.length + (g.saves.length === 1 ? " save" : " saves")),
            ),
            newest.civs ? h("div", { class: "civs" }, plain(newest.civs)) : null,
          ),
          btn("Load latest", (e) => {
            e.preventDefault();
            onLoad(newest);
          }),
        ),
        h(
          "ol",
          { class: "save-rows" },
          g.saves.map((s) =>
            h(
              "li",
              { "data-k": "s" + s.id },
              h("span", { class: "turn" }, "Turn " + s.turn),
              h("span", { class: "when" }, s.saved),
              btn("Load", () => onLoad(s), { class: "small" }),
            ),
          ),
        ),
      );
    });
  }

  const PLACE_LABEL = { lobby: "In the lobby", room: "In a room", game: "Playing" };

  function onlinePanel(v) {
    const list = v.online || [];
    const body = list.length
      ? h(
          "ul",
          { class: "online-list" },
          list.map((p) =>
            h(
              "li",
              { "data-k": "o" + p.nick, class: "place-" + p.place },
              h("span", { class: "hexdot", title: PLACE_LABEL[p.place] || "" }),
              h(
                "div",
                { class: "o-main" },
                h("div", { class: "o-nick" }, p.nick, p.me ? h("span", { class: "tag" }, "you") : null, p.host ? h("span", { class: "tag" }, "host") : null),
                h("div", { class: "o-status" }, p.status),
              ),
              p.ready == null ? null : h("span", { class: "ready-mark" + (p.ready ? " on" : ""), title: p.ready ? "Ready" : "Not ready" }, p.ready ? "Ready" : "Not ready"),
            ),
          ),
        )
      : h("div", { class: "sub", style: "padding:4px 0 8px" }, "Nobody else is signed in.");
    return h(
      "aside",
      { class: "online frame", "aria-label": "Players online" },
      details("online", "plain", "Online", true, body, h("span", { class: "count" }, String(list.length))),
    );
  }

  // ---------------------------------------------------------------------------------------------------- room

  const human = (s) => (s.who && typeof s.who === "object" ? s.who.Human : null);
  const kindOf = (s) => (s.who === "Ai" ? "Ai" : s.who === "Closed" ? "Closed" : "Open");

  function roomPage(v) {
    const st = v.settings;
    const dis = !v.me_host || v.starting;
    const mine = v.slots.find((s) => human(s) && human(s).me);
    return h(
      "div",
      null,
      h(
        "div",
        { class: "page room" },
        h(
          "header",
          { class: "top" },
          h("div", { class: "brand" }, emblem(36), h("div", null, h("h1", null, st.name || "Room"), h("div", { class: "kicker" }, "Staging room · host " + v.host))),
        ),
        v.note ? h("div", { class: "note" }, v.note) : null,
        v.from_save ? h("div", { class: "note" }, v.from_save) : null,
        v.me_host ? null : h("div", { class: "sub", style: "margin:0 0 8px" }, "Only the host (" + v.host + ") can change the settings."),
        h(
          "div",
          { class: "cols" },
          h("div", { class: "players-col" }, playersSection(v, dis)),
          h("div", { class: "settings-col" }, mapSection(st, dis), gameSection(st, dis), victorySection(st, dis), optionsSection(st, dis), advancedSection(st, dis)),
        ),
      ),
      bottomBar(v, mine),
    );
  }

  function readyBox(v, mine) {
    return check("Ready", human(mine).ready, (on) => post({ Ready: on }), v.starting, null, "ready");
  }

  // The civilization picker: a list with the civ icons; hovering (or focusing) an entry shows the civ's tip, as the
  // original staging room does. One picker is open at a time.
  let picker = null; // slot index
  function tipsOf(v) {
    const tips = {};
    (v.civ_tips || []).forEach((t) => (tips[t.key] = t));
    return tips;
  }

  function civPicker(v, s, on, dis) {
    const open = picker === s.index && !dis;
    const cur = v.civs.find((c) => c.key === s.civ);
    const label = cur ? plain(cur.name) : "Random civilization";
    const toggle = () => {
      picker = open ? null : s.index;
      hideTip();
      draw();
    };
    return h(
      "div",
      { class: "civpick" + (open ? " open" : ""), "data-k": "pick" + s.index },
      h(
        "button",
        {
          type: "button",
          class: "civpick-btn",
          disabled: dis,
          "aria-haspopup": "listbox",
          "aria-expanded": open ? "true" : "false",
          "aria-label": "Civilization of slot " + (s.index + 1) + ": " + label,
          "data-civtip": s.civ || null,
          onclick: toggle,
        },
        s.civ ? civIcon(s.civ, 40) : h("span", { class: "civicon random", "aria-hidden": "true" }, "?"),
        h("span", { class: "civpick-name" }, label),
        h("span", { class: "caret", "aria-hidden": "true" }),
      ),
      open
        ? h(
            "div",
            {
              class: "civpick-list",
              role: "listbox",
              "aria-label": "Civilizations",
              onkeydown: (e) => {
                if (e.key === "Escape") {
                  picker = null;
                  hideTip();
                  draw();
                }
              },
            },
            [{ key: "", name: "Random civilization" }].concat(v.civs).map((c) =>
              h(
                "button",
                {
                  type: "button",
                  role: "option",
                  class: "civpick-opt" + ((s.civ || "") === c.key ? " on" : ""),
                  "aria-selected": (s.civ || "") === c.key ? "true" : "false",
                  "data-civtip": c.key || null,
                  "data-k": "o" + c.key,
                  onclick: () => {
                    picker = null;
                    hideTip();
                    on(c.key);
                    draw();
                  },
                },
                c.key ? civIcon(c.key, 32) : h("span", { class: "civicon random small", "aria-hidden": "true" }, "?"),
                h("span", null, plain(c.name)),
              ),
            ),
          )
        : null,
    );
  }

  function playersSection(v, dis) {
    const firstOpen = v.slots.find((s) => s.who === "Open");
    const lastAi = v.slots.slice().reverse().find((s) => s.who === "Ai");
    const setSlot = (s, kind) => post({ SlotSet: { index: s.index, kind, civ: s.civ } });
    const tips = tipsOf(v);
    return details(
      "players",
      "section frame",
      "Players",
      true,
      [
        v.slots.map((s) => {
          const hu = human(s);
          let civ = null;
          if (s.who !== "Closed") {
            if (hu) civ = civPicker(v, s, (val) => post({ PickCiv: val || null }), !hu.me || v.starting);
            else civ = civPicker(v, s, (val) => post({ SlotSet: { index: s.index, kind: kindOf(s), civ: val || null } }), dis);
          }
          const tip = s.civ ? tips[s.civ] : null;
          const tipId = "civinfo" + s.index;
          return h(
            "div",
            { class: "slot" + (hu && hu.me ? " me" : "") + (s.who === "Closed" ? " closed" : ""), "data-k": "slot" + s.index },
            h(
              "div",
              { class: "who" },
              h("span", { class: "num" }, String(s.index + 1)),
              hu ? h("strong", { class: "nick" }, hu.nick) : h("span", { class: "kind" }, s.who === "Open" ? "Open slot" : s.who === "Ai" ? "AI" : "Closed"),
              hu && hu.nick.toLowerCase() === v.host.toLowerCase() ? h("span", { class: "tag" }, "host") : null,
              hu && hu.me ? h("span", { class: "tag" }, "you") : null,
              hu ? h("span", { class: "ready-mark" + (hu.ready ? " on" : "") }, hu.ready ? "Ready" : "Not ready") : null,
              s.yours ? h("span", { class: "note" }, "Your civilization") : null,
              s.was ? h("span", { class: "sub" }, "was: " + s.was) : null,
            ),
            h(
              "div",
              { class: "slot-ctl" },
              civ,
              hu ? null : sel([["Open", "Open"], ["Ai", "AI"], ["Closed", "Closed"]], kindOf(s), (k) => setSlot(s, k), dis, "Slot " + (s.index + 1) + " is"),
            ),
            tip
              ? (() => {
                  if (!openState.has(tipId)) openState.set(tipId, false);
                  return h(
                    "details",
                    { class: "civinfo", "data-k": tipId, open: openState.get(tipId), ontoggle: (e) => openState.set(tipId, e.target.open) },
                    h("summary", null, "About this civilization"),
                    tipCard(s.civ, v, true),
                  );
                })()
              : null,
            s.who === "Open" && !v.starting && (!v.has_slot || s.yours)
              ? h("div", { class: "row slot-act" }, btn("Take this slot", () => post({ TakeSlot: s.index }), { class: "primary" }))
              : hu && !hu.me && !dis
                ? h("div", { class: "row slot-act" }, btn("Remove", () => setSlot(s, "Open"), { class: "quiet" }))
                : null,
          );
        }),
        v.me_host
          ? h(
              "div",
              { class: "row" },
              btn("Add AI", () => setSlot(firstOpen, "Ai"), { disabled: dis || !firstOpen }),
              btn("Remove AI", () => setSlot(lastAi, "Open"), { disabled: dis || !lastAi, class: "quiet" }),
            )
          : null,
      ],
    );
  }

  // ------------------------------------------------------------------------------------- civilization tip

  /** The framed tip of a civilization: portrait, names, traits (game markup) and unique items with their icons, each
   *  with what it replaces, its numbers and its help text (the original's unique-item tooltips: the asset set's
   *  `uniques[]` carry `replaces`, `stats[]` and `help` in game markup, SIMP-exe-1062).
   *  SIMPLIFIED(SIMP-exe-1055): our own card layout, not the original staging room tooltip. */
  function tipCard(key, v, inline) {
    const t = tipsOf(v)[key];
    const civ = v.civs.find((c) => c.key === key);
    const a = civArt(key);
    if (!t && !a) return null;
    const leader = (t && t.leader) || (a && a.leader && a.leader.name) || "";
    const traits = (t && t.traits) || [];
    const uniques =
      a && a.uniques && a.uniques.length
        ? a.uniques.map((u) => ({ kind: u.kind, name: u.name, cell: u.cell, replaces: u.replaces, stats: u.stats || [], help: u.help }))
        : ((t && t.unique) || []).map((u) => {
            const i = u.indexOf(": ");
            return i > 0 ? { kind: u.slice(0, i), name: u.slice(i + 2), cell: null } : { kind: "", name: u, cell: null };
          });
    return h(
      "div",
      { class: "tipcard" + (inline ? " embedded" : "") },
      h(
        "div",
        { class: "tip-head" },
        a && a.leader && a.leader.cell ? h("div", { class: "portrait" }, sprite(a.leader.cell, inline ? 72 : 96)) : null,
        h(
          "div",
          { class: "tip-names" },
          h("div", { class: "tip-civ" }, a && a.icon ? sprite(a.icon, 24, "civicon") : null, h("span", null, plain(civ ? civ.name : (a && a.name) || key))),
          leader ? h("div", { class: "tip-leader" }, markup(leader)) : null,
        ),
      ),
      traits.length
        ? h(
            "div",
            { class: "tip-sec" },
            traits.map((tr) => {
              const i = tr.indexOf(": ");
              return h("div", { class: "trait" }, i > 0 ? [h("div", { class: "trait-name" }, markup(tr.slice(0, i))), h("div", { class: "trait-desc" }, markup(tr.slice(i + 2)))] : h("div", { class: "trait-desc" }, markup(tr)));
            }),
          )
        : null,
      uniques.length
        ? h(
            "div",
            { class: "tip-sec uniques" },
            uniques.map((u) =>
              h(
                "div",
                { class: "unique" },
                u.cell ? sprite(u.cell, 40, "uicon") : h("span", { class: "uicon none" }),
                h(
                  "div",
                  { class: "u-body" },
                  h("div", { class: "u-kind" }, u.kind ? "Unique " + u.kind.toLowerCase() : "Unique"),
                  h("div", { class: "u-name" }, markup(u.name)),
                  u.replaces ? h("div", { class: "u-replaces" }, markup(u.replaces)) : null,
                  u.stats && u.stats.length ? h("div", { class: "u-stats" }, u.stats.map((st) => h("span", { class: "u-stat" }, markup(st)))) : null,
                  u.help ? h("div", { class: "u-help" }, markup(u.help)) : null,
                ),
              ),
            ),
          )
        : null,
    );
  }

  // One floating tooltip outside the morphed tree. Mouse (and keyboard focus) only: on touch the slot's
  // "About this civilization" fold shows the same card.
  const tipEl = h("div", { class: "civtip-float", role: "tooltip", hidden: true });
  overlay.append(tipEl);
  let tipFor = null; // { key, el }

  function showTip(el) {
    if (!last || last.screen !== "room") return;
    const key = el.getAttribute("data-civtip");
    if (!key) return hideTip();
    tipFor = { key, el };
    refreshTip();
  }
  function hideTip() {
    tipFor = null;
    tipEl.hidden = true;
  }
  function refreshTip() {
    if (!tipFor) return;
    if (!tipFor.el.isConnected || !last || last.screen !== "room") return hideTip();
    const card = tipCard(tipFor.key, last.view, false);
    if (!card) return hideTip();
    morphKids(tipEl, h("div", null, card));
    tipEl.hidden = false;
    const r = tipFor.el.getBoundingClientRect();
    const list = tipFor.el.closest(".civpick-list");
    const anchor = list ? list.getBoundingClientRect() : r;
    const w = tipEl.offsetWidth;
    const hgt = tipEl.offsetHeight;
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    let x = anchor.right + 12;
    if (x + w > vw - 8) x = anchor.left - w - 12;
    if (x < 8) x = Math.max(8, Math.min(vw - w - 8, r.left));
    let y = list ? r.top - 8 : r.bottom + 8;
    if (!list && x !== anchor.right + 12 && x !== anchor.left - w - 12) y = r.bottom + 8;
    y = Math.max(8, Math.min(vh - hgt - 8, y));
    tipEl.style.left = Math.round(x) + "px";
    tipEl.style.top = Math.round(y) + "px";
  }
  overlay.addEventListener("pointerover", (e) => {
    if (e.pointerType === "touch") return;
    const el = e.target.closest && e.target.closest("[data-civtip]");
    if (el) showTip(el);
  });
  overlay.addEventListener("pointerout", (e) => {
    const el = e.target.closest && e.target.closest("[data-civtip]");
    if (el && !(e.relatedTarget && el.contains(e.relatedTarget))) hideTip();
  });
  overlay.addEventListener("focusin", (e) => {
    const el = e.target.closest && e.target.closest("[data-civtip]");
    if (el && e.target.matches(":focus-visible")) showTip(el);
    else hideTip();
  });
  overlay.addEventListener("scroll", hideTip, true);
  document.addEventListener("pointerdown", (e) => {
    if (picker != null && !(e.target.closest && e.target.closest(".civpick"))) {
      picker = null;
      hideTip();
      draw();
    }
  });

  // ------------------------------------------------------------------------------------------- settings

  function mapSection(st, dis) {
    return details(
      "map",
      "section frame",
      "Map",
      true,
      [
        h(
          "div",
          { class: "grid2" },
          field("Map type", sel(keyed(st.scripts), st.map_script, (k) => edit({ MapScript: k }), dis, "Map type")),
          field("Map size", sel(keyed(st.sizes), st.world_size, (k) => edit({ WorldSize: k }), dis, "Map size"), st.size_info),
        ),
        st.map_options.map((o) =>
          o.values.length
            ? h("div", { "data-k": "mo" + o.id }, field(o.name, sel(o.values, o.value, (x) => edit({ MapOption: { id: o.id, value: Number(x) } }), dis || o.read_only, plain(o.name)), o.help))
            : h("div", { "data-k": "mo" + o.id }, check(o.name, o.value !== 0, (on) => edit({ MapOption: { id: o.id, value: on ? 1 : 0 } }), dis || o.read_only, o.help)),
        ),
        field(
          "City-states",
          h(
            "div",
            { class: "inline" },
            num(st.minor_civs, 0, st.max_minor, (n) => edit({ MinorCivs: n == null ? 0 : n }), dis, "City-states"),
            h("span", { class: "sub" }, "of " + st.max_minor),
          ),
        ),
      ],
    );
  }

  function gameSection(st, dis) {
    const kind = st.timer === "Off" ? "Off" : st.timer.Original !== undefined ? "Original" : "Fixed";
    const setTimer = (k) => {
      if (k === "Off") edit({ Timer: "Off" });
      else if (k === "Original") edit({ Timer: { Original: st.timers[0].key } });
      else edit({ Timer: { Fixed: 60 } });
    };
    const kinds = [["Off", "Off"]];
    if (st.timers.length) kinds.push(["Original", "Original"]);
    kinds.push(["Fixed", "Fixed"]);
    return details(
      "game",
      "section frame",
      "Game",
      false,
      [
        h(
          "div",
          { class: "grid2" },
          field("Game speed", sel(keyed(st.speeds), st.speed, (k) => edit({ Speed: k }), dis, "Game speed")),
          field("Start era", sel(keyed(st.eras), st.era, (k) => edit({ Era: k }), dis, "Start era")),
          field("Difficulty", sel(keyed(st.handicaps), st.handicap, (k) => edit({ Handicap: k }), dis, "Difficulty")),
          field("Turn mode", sel([["Simultaneous", "Simultaneous"], ["Hybrid", "Hybrid"]], st.turn_mode, (k) => edit({ TurnMode: k }), dis, "Turn mode")),
        ),
        field(
          "Turn timer",
          h(
            "div",
            null,
            sel(kinds, kind, setTimer, dis, "Turn timer"),
            kind === "Original" ? h("div", { style: "margin-top:8px" }, sel(keyed(st.timers), st.timer.Original, (k) => edit({ Timer: { Original: k } }), dis, "Timer row")) : null,
            kind === "Fixed"
              ? h(
                  "div",
                  { class: "inline", style: "margin-top:8px" },
                  num(st.timer.Fixed, 10, 600, (n) => edit({ Timer: { Fixed: n == null ? 60 : n } }), dis, "Seconds per turn"),
                  h("span", { class: "sub" }, "seconds (10-600)"),
                )
              : null,
          ),
        ),
        field(
          "Max turns",
          h(
            "div",
            null,
            check("Limit the game", st.max_turns > 0, (on) => edit({ MaxTurns: on ? st.max_turns || 500 : 0 }), dis, null),
            st.max_turns > 0 ? num(st.max_turns, 1, 2000, (n) => edit({ MaxTurns: n == null ? 0 : n }), dis, "Max turns") : null,
          ),
          "Off: the speed's own limit.",
        ),
      ],
    );
  }

  function victorySection(st, dis) {
    return details("victory", "section frame", "Victory", false, h("div", { class: "checks" }, st.victories.map((x) => check(x.name, x.on, (on) => edit({ Victory: { key: x.key, on } }), dis, null, x.key))));
  }

  function optionRow(o, dis) {
    return o.values.length
      ? h("div", { "data-k": o.key }, field(o.name, sel(o.values, o.value, (x) => edit({ Option: { key: o.key, value: Number(x) } }), dis, plain(o.name)), o.help))
      : check(o.name, o.value !== 0, (on) => edit({ Option: { key: o.key, value: on ? 1 : 0 } }), dis, o.help, o.key);
  }

  // Lekmod's MP options screen (MPGameOptions.lua): "AI Options" and "Additional Options" are check boxes with a
  // coloured header label (AIOptionsCheck ...) whose check shows the group of options under them. They are drawn as
  // group headers with a switch, the group below while on.
  const GROUP_HEADS = [
    ["GAMEOPTION_AI_OPTIONS", "Ai", "AI options"],
    ["GAMEOPTION_ADDITIONAL", "Additional", "Additional options"],
  ];
  function optionsSection(st, dis) {
    const heads = new Set(GROUP_HEADS.map((g) => g[0]));
    const group = (g) => st.options.filter((o) => o.group === g && !heads.has(o.key));
    const fold = (id, title, rows) => (rows.length ? h("details", { class: "fold", "data-k": id, open: !!openState.get(id), ontoggle: (e) => openState.set(id, e.target.open) }, h("summary", null, title), rows.map((o) => optionRow(o, dis))) : null);
    return details("options", "section frame", "Options", false, [
      h("div", { class: "checks" }, group("General").map((o) => optionRow(o, dis))),
      GROUP_HEADS.map(([key, g, fallback]) => {
        const o = st.options.find((x) => x.key === key);
        if (!o) return null;
        const on = o.value !== 0;
        const rows = st.options.filter((x) => x.group === g);
        return h(
          "div",
          { class: "opt-group" + (on ? " on" : ""), "data-k": "grp" + key },
          h(
            "label",
            { class: "opt-head" },
            h("input", { type: "checkbox", checked: on, disabled: dis, onchange: (e) => edit({ Option: { key, value: e.target.checked ? 1 : 0 } }) }),
            h("span", { class: "opt-head-title" }, plain(o.name) ? markup(o.name) : fallback),
            h("span", { class: "sub" }, rows.length + " options"),
          ),
          on ? h("div", { class: "checks" }, rows.map((x) => optionRow(x, dis))) : null,
        );
      }),
      fold("fold-more", "More options", group("Hidden")),
    ]);
  }

  function advancedSection(st, dis) {
    return details("advanced", "section frame", "Advanced", false, [
      h(
        "div",
        { class: "grid2" },
        field("Seed", num(st.seed, 0, 4294967295, (n) => edit({ Seed: n }), dis, "Seed"), "Empty: random."),
        field("Map seed", num(st.map_seed, 0, 4294967295, (n) => edit({ MapSeed: n }), dis, "Map seed"), "Empty: random."),
      ),
      h("div", { class: "sub", style: "margin-top:12px" }, "The AI decides for you:"),
      h("div", { class: "checks" }, st.auto_choices.map((a) => check(a.name, a.on, (on) => edit({ AutoChoice: { key: a.key, on } }), dis, null, "a" + a.key))),
      h("div", { class: "sub", style: "margin-top:12px" }, "The AI makes research, policy and similar choices for you; AI offers are declined."),
    ]);
  }

  let showSaves = false;
  function bottomBar(v, mine) {
    const host = v.me_host;
    return h(
      "div",
      { class: "bar" },
      h(
        "div",
        { class: "inner" },
        btn("Leave room", () => post("LeaveRoom"), { disabled: v.starting, class: "quiet" }),
        mine ? readyBox(v, mine) : null,
        h(
          "div",
          { class: "why" },
          v.starting ? "Starting the game..." : host ? (v.can_start ? "Everyone is ready" : v.why_not || "") : "Waiting for " + v.host + " to start the game",
        ),
        host ? btn("Load save", () => { showSaves = !showSaves; draw(); }, { disabled: v.starting, "aria-expanded": showSaves ? "true" : "false" }) : null,
        host ? btn("Start game", () => post("Start"), { class: "primary big", disabled: !v.can_start || v.starting }) : null,
        v.error ? h("div", { class: "err", role: "alert", style: "flex-basis:100%;margin:0" }, v.error) : null,
        host && showSaves
          ? h(
              "div",
              { class: "saves" },
              savesList(saveGroups(v.saves), (s) => {
                showSaves = false;
                post({ LoadSave: s.id });
              }, "room"),
            )
          : null,
      ),
    );
  }

  // The login page waits for a first sign-in when this browser never signed in (no remembered nick; the session cookie is
  // HttpOnly, so the page cannot see it: a returning visitor gets the form only if the app reports the login screen).
  if (!egui && window.__civ5auth && !window.__civ5auth.mayHaveSession()) window.__civ5auth.show(null);
})();
