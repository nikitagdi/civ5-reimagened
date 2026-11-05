// Golden report page: builds the UI from the inline DATA JSON using DOM calls only.
(function () {
  "use strict";
  // Create an element. Props: class, on<event> handlers, text, other attributes.
  // Children may be nodes, strings, numbers, arrays; null/false are skipped.
  function $(tag, props, ...kids) {
    const el = document.createElement(tag);
    for (const [k, v] of Object.entries(props || {})) {
      if (v === undefined || v === null || v === false) continue;
      if (k === "class") el.className = v;
      else if (k === "text") el.textContent = String(v);
      else if (k.startsWith("on")) el.addEventListener(k.slice(2), v);
      else el.setAttribute(k, v === true ? "" : String(v));
    }
    for (const kid of kids.flat()) {
      if (kid === null || kid === undefined || kid === false) continue;
      el.append(kid instanceof Node ? kid : document.createTextNode(String(kid)));
    }
    return el;
  }
  const arr = (x) => (Array.isArray(x) ? x : []);
  const txt = (x, d = "") => (x === undefined || x === null ? d : String(x));
  const path = (x) => (typeof x === "string" && x !== "" ? x : null);
  const fmt = (x) => (typeof x !== "number" || !isFinite(x) ? "-" : Number.isInteger(x) ? String(x) : x.toFixed(4));
  const scroll = (el) => $("div", { class: "scroll" }, el);
  const muted = (text) => $("p", { class: "muted", text });
  const VERDICTS = ["accept", "reject", "pending"];
  const STATUSES = ["pass", "fail", "error", "empty", "info"];
  const badge = (s) => $("span", { class: "status st-" + (STATUSES.includes(s) ? s : "other"), text: s || "-" });
  let D = {};
  try {
    D = JSON.parse(document.getElementById("data")?.textContent || "{}") ?? {};
  } catch (err) {
    D = { title: "Report data could not be parsed", generated: String(err) };
  }
  if (typeof D !== "object") D = {};
  let current = null; // viewer that the F key flips
  document.addEventListener("keydown", (e) => {
    if (!current || e.key.toLowerCase() !== "f" || e.ctrlKey || e.metaKey || e.altKey) return;
    if (["INPUT", "TEXTAREA", "SELECT"].includes(e.target.tagName)) return;
    e.preventDefault();
    current.flip();
  });
  // Image viewer for one ref/ours/error set. full adds side-by-side and error-map buttons.
  function viewer(ref, ours, error, label, full) {
    let mode = ref ? "ref" : "ours"; // ref | ours | side | error
    const stage = $("div", { class: "stage" });
    const flipBtn = $("button", { type: "button", class: "btn", onclick: flip });
    const sideBtn = $("button", {
      type: "button", class: "btn", text: "Side by side",
      onclick: () => setMode(mode === "side" ? "ref" : "side"),
    });
    const errBtn = $("button", {
      type: "button", class: "btn", text: "Error map",
      onclick: () => setMode(mode === "error" ? "ref" : "error"),
    });
    const im = (src, alt) => $("img", { src, alt, loading: "lazy" });
    function setMode(m) { mode = m; paint(); }
    function flip() { if (ref && ours) setMode(mode === "ours" ? "ref" : "ours"); }
    function paint() {
      let view;
      if (mode === "side" && ref && ours) {
        view = { shown: "side by side", kids: [im(ref, label + " ref"), im(ours, label + " ours")] };
      } else if (mode === "error" && error) {
        view = { shown: "error map", kids: [im(error, label + " error map")] };
      } else if (ours && (mode === "ours" || !ref)) {
        view = { shown: "ours", kids: [im(ours, label + " ours")] };
      } else if (ref) {
        view = { shown: "ref", kids: [im(ref, label + " ref")] };
      } else {
        view = { shown: "nothing", kids: [muted("No image for this view.")] };
      }
      stage.replaceChildren(...view.kids);
      flipBtn.textContent = `Flip (showing ${view.shown})`;
      flipBtn.disabled = !(ref && ours);
      sideBtn.disabled = !(ref && ours);
      errBtn.disabled = !error;
      sideBtn.setAttribute("aria-pressed", String(mode === "side"));
      errBtn.setAttribute("aria-pressed", String(mode === "error"));
    }
    const el = $("div", { class: "viewer" },
      $("div", { class: "bar" }, flipBtn, ...(full ? [sideBtn, errBtn] : [])),
      stage,
      ours ? null : $("p", { class: "note", text: "no render of ours yet" }));
    paint();
    return { el, flip };
  }
  // Enlarged crop in a modal dialog with the same flip controls; Esc closes.
  function enlarge(label, ref, ours, error) {
    const v = viewer(ref, ours, error, label, false);
    const prev = current;
    const dlg = $("dialog", { class: "big" },
      $("div", { class: "bar" }, $("strong", { text: label }),
        $("button", { type: "button", class: "btn", text: "Close", onclick: () => dlg.close() })),
      v.el);
    dlg.addEventListener("close", () => {
      current = prev;
      dlg.remove();
    });
    document.body.append(dlg);
    current = v;
    dlg.showModal();
  }
  function cropTile(cr, label) {
    const ref = path(cr.ref), ours = path(cr.ours), error = path(cr.error);
    const thumb = ref || ours;
    return $("figure", { class: "crop" },
      $("button", {
        type: "button", class: "crop-open", "aria-label": "Enlarge " + label,
        onclick: () => enlarge(label, ref, ours, error),
      }, thumb ? $("img", { src: thumb, alt: label, loading: "lazy" }) : muted("no image")),
      $("figcaption", { text: label }));
  }
  const cropGrid = (list) => {
    const crops = arr(list);
    return crops.length
      ? $("div", { class: "crops" }, crops.map((cr, k) => cropTile(cr ?? {}, txt(cr?.label, "crop " + (k + 1)))))
      : muted("No crops.");
  };
  const metricsTable = (list) => {
    const rows = arr(list);
    if (!rows.length) return muted("No metrics.");
    const head = ["Mask", "Pixels", "Flip", "dE2000", "SSIM", "Threshold (flip / 1-SSIM / dE)", "Status", "Verdict"];
    const status = (m) => (m?.gated === false ? "info" : m?.exceeds ? "fail" : "pass");
    return scroll($("table", {},
      $("thead", {}, $("tr", {}, head.map((h) => $("th", { text: h })))),
      $("tbody", {}, rows.map((m) => $("tr", {},
        $("td", { text: txt(m?.mask) }), $("td", { text: fmt(m?.pixels) }),
        $("td", { text: fmt(m?.flip) }), $("td", { text: fmt(m?.de2000) }),
        $("td", { text: fmt(m?.ssim) }),
        $("td", { text: [m?.thr_flip, m?.thr_ssim, m?.thr_de2000].map(fmt).join(" / ") }),
        $("td", {}, badge(status(m)), m?.exceeds ? $("span", { text: " " + arr(m.over).join(", ") }) : null),
        $("td", { text: txt(m?.verdict, "-") }))))));
  };
  const notesList = (list) => {
    const notes = arr(list);
    return notes.length
      ? $("ul", { class: "notes" }, notes.map((n) => $("li", { text: txt(n) })))
      : muted("No notes.");
  };
  // Triage state: capture id -> verdict, in page order. tomlOut shows the file text.
  const verdicts = new Map();
  const needed = []; // tables for the masks that exceed their threshold and have no verdict yet
  const tomlOut = $("textarea", { readonly: true, rows: "8", spellcheck: "false", "aria-label": "triage.toml text" });
  const copyBtn = $("button", { type: "button", class: "btn", text: "Copy", onclick: copyToml });
  function tableKey(id) {
    return /^[A-Za-z0-9_-]+$/.test(id) ? `[${id}]` : `["${id.replace(/["\\]/g, "\\$&")}"]`;
  }
  function renderToml() {
    tomlOut.value = [...verdicts]
      .filter(([, v]) => v !== "pending") // no verdict yet is no table
      .map(([id, v]) => `${tableKey(id)}\nverdict = "${v}"\n`)
      .concat(needed.map((n) => `${n}\nverdict = "simp"   # or "bug"\nreference = "SIMP-NNN"   # or the task of the bug\n`))
      .join("\n");
  }
  async function copyToml() {
    try {
      await navigator.clipboard.writeText(tomlOut.value);
    } catch (err) {
      tomlOut.select();
      document.execCommand("copy");
    }
    copyBtn.textContent = "Copied";
    setTimeout(() => { copyBtn.textContent = "Copy"; }, 1500);
  }
  function captureSection(c, idx) {
    const id = txt(c.id, "capture-" + (idx + 1));
    const ref = path(c.ref), ours = path(c.ours), error = path(c.error);
    const v = viewer(ref, ours, error, id, true);
    current = current || v;
    const init = VERDICTS.includes(c.verdict) ? c.verdict : "pending";
    for (const m of arr(c.metrics)) {
      if (m?.exceeds && !m.verdict) needed.push(`${tableKey(id).slice(0, -1)}.masks.${m.key}]`);
    }
    verdicts.set(id, init);
    const radios = $("div", { class: "radios", role: "radiogroup", "aria-label": "Verdict " + id },
      VERDICTS.map((val) => $("label", {},
        $("input", {
          type: "radio", name: "triage-" + idx, value: val, checked: val === init,
          onchange: () => { verdicts.set(id, val); renderToml(); },
        }),
        val)));
    return $("section", {
      class: "card",
      onpointerdown: () => { current = v; },
      onfocusin: () => { current = v; },
    },
      $("h2", { text: id }),
      c.scene ? muted("Scene " + txt(c.scene)) : null,
      v.el,
      $("h3", { text: "Metrics" }), metricsTable(c.metrics),
      $("h3", { text: "Crops" }), cropGrid(c.crops),
      $("h3", { text: "Notes" }), notesList(c.notes),
      $("h3", { text: "Verdict" }), radios);
  }
  function entryRow(e) {
    const calls = arr(e.calls);
    const detail = $("details", {},
      $("summary", { text: txt(e.entry, "(unnamed)") }),
      calls.length ? callsTable(calls) : muted("No calls."));
    return $("tr", {},
      $("td", {}, detail),
      $("td", {}, badge(txt(e.status, "empty"))),
      $("td", { text: txt(e.summary) }));
  }
  function callsTable(calls) {
    const head = ["Capture", "Eid", "Status", "Compared", "Failing", "Max abs", "p99.9", "Image", "Notes"];
    return scroll($("table", {},
      $("thead", {}, $("tr", {}, head.map((h) => $("th", { text: h })))),
      $("tbody", {}, calls.map((c) => {
        const r = c ?? {};
        return $("tr", {},
          $("td", { text: txt(r.capture) }), $("td", { text: fmt(r.eid) }),
          $("td", {}, badge(txt(r.status, "-"))), $("td", { text: fmt(r.compared) }),
          $("td", { text: fmt(r.failing) }), $("td", { text: fmt(r.max_abs) }),
          $("td", { text: fmt(r.p99_9) }),
          $("td", {}, path(r.image)
            ? $("img", { src: r.image, alt: "error image", loading: "lazy", class: "thumb" })
            : "-"),
          $("td", { text: arr(r.notes).map((n) => txt(n)).join("; ") }));
      }))));
  }
  function replaySection(rp) {
    const entries = arr(rp.entries);
    const table = $("table", {},
      $("thead", {}, $("tr", {}, ["Entry", "Status", "Summary"].map((h) => $("th", { text: h })))),
      $("tbody", {}, entries.map((e) => entryRow(e ?? {}))));
    return $("section", { class: "card" },
      $("h2", { text: "Replay" }),
      muted("GPU: " + txt(rp.gpu, "unknown")),
      entries.length ? scroll(table) : muted("No replay entries."));
  }
  function cameraSection(items) {
    const list = arr(items);
    return $("section", { class: "card" }, $("h2", { text: "Camera" }),
      list.length
        ? list.map((it) => $("div", {},
          it?.capture ? $("h3", { text: txt(it.capture) }) : null,
          $("pre", { text: txt(it?.text) })))
        : muted("No camera data."));
  }
  // Assemble the page.
  const title = txt(D.title, "Golden report");
  document.title = title;
  const app = document.getElementById("app") || document.body.appendChild($("div", { id: "app" }));
  const captureEls = arr(D.captures).map((c, i) => captureSection(c ?? {}, i));
  renderToml();
  app.replaceChildren(
    $("header", {}, $("h1", { text: title }),
      D.generated ? muted("Generated " + txt(D.generated)) : null),
    captureEls.length ? captureEls : muted("No captures."),
    $("section", { class: "card" },
      $("h2", { text: "triage.toml" }),
      muted("The verdicts above produce this file."),
      tomlOut,
      $("div", { class: "row" }, copyBtn)),
    replaySection(D.replay ?? {}),
    cameraSection(D.camera));
})();
