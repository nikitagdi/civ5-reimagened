// The game cursor at the window's edge (live report 2026-11-02: "in the bottom ~20 px the cursor turns back into the
// system one").
//
// The game's cursors are custom CSS cursors (winit sets `canvas.style.cursor = "url(blob:...) x y, auto"` for each
// frame of the animation). Chrome (and Firefox) refuse to draw a custom cursor image that would reach outside the
// page or over the browser's own bars, and fall back to the `auto` of the declaration, the system arrow. So within
// one cursor size of an edge the browser shows the arrow, however small the picture is.
//
// In that edge zone this file hides the system cursor (`cursor: none` on the canvas) and draws the same picture
// itself: a DOM element that follows the mouse and takes the picture and hot spot from the canvas's own `cursor`
// value, so the cursor the game picked (and its animation) is what shows. Elsewhere the native cursor stays (it
// has no lag). Mouse and pen only; a touch screen has no cursor.
//
// SIMPLIFIED(SIMP-exe-1082): in the edge zone the cursor is a page element, one frame behind the pointer, not the
// system's own cursor; the zone is a fixed 40 px (48 px at the bottom) rather than what a given browser allows.

/** Width of the zone along the sides and the top, and (taller, the browser's bar is there) along the bottom. */
export const EDGE_PX = 40;
export const EDGE_BOTTOM_PX = 48;

/** Whether a pointer at (x, y) in a window of w x h is in the zone where the browser drops custom cursors. */
export function inEdgeZone(x, y, w, h) {
  return x < EDGE_PX || y < EDGE_PX || x > w - EDGE_PX || y > h - EDGE_BOTTOM_PX;
}

/** Whether the page holds a pointer lock (a mod's first-person mode): the cursor is hidden by the browser then. */
export function pointerLocked(doc) {
  return !!(doc && doc.pointerLockElement);
}

/** The picture and hot spot of a CSS cursor value `url(blob:...) 4 2, auto`; null for a keyword cursor. */
export function parseCursor(value) {
  const m = /url\(\s*(?:"([^"]*)"|'([^']*)'|([^)\s]*))\s*\)\s*(\d+)?\s*(\d+)?/.exec(String(value ?? ""));
  if (!m) return null;
  const url = m[1] ?? m[2] ?? m[3];
  if (!url) return null;
  return { url, x: m[4] ? Number(m[4]) : 0, y: m[5] ? Number(m[5]) : 0 };
}

function start() {
  const canvas = document.getElementById("civ5-canvas");
  if (!canvas) return;
  const style = document.createElement("style");
  style.textContent =
    "#civ5-canvas.civ5-soft-cursor{cursor:none!important}" +
    "#civ5-soft-cursor{position:fixed;left:0;top:0;width:32px;height:32px;pointer-events:none;z-index:2147483647;" +
    "background-repeat:no-repeat;background-size:32px 32px;will-change:transform;display:none}";
  document.head.appendChild(style);
  const el = document.createElement("div");
  el.id = "civ5-soft-cursor";
  el.setAttribute("aria-hidden", "true");
  document.body.appendChild(el);

  let on = false;
  let shown = null; // the cursor value the element shows
  let hot = { x: 0, y: 0 };
  let at = { x: 0, y: 0 };
  let raf = 0;

  const place = () => {
    el.style.transform = `translate(${at.x - hot.x}px,${at.y - hot.y}px)`;
  };
  // The game swaps the picture for the animation and the interface modes; follow it while the element shows.
  const sync = () => {
    raf = 0;
    if (!on) return;
    const value = canvas.style.cursor;
    if (value !== shown) {
      shown = value;
      const c = parseCursor(value);
      if (c) {
        el.style.backgroundImage = `url("${c.url}")`;
        hot = { x: c.x, y: c.y };
        el.style.visibility = "visible";
      } else {
        // A keyword cursor (the text bar over an edit box...): let the system draw it.
        el.style.visibility = "hidden";
      }
      canvas.classList.toggle("civ5-soft-cursor", !!c);
      place();
    }
    raf = requestAnimationFrame(sync);
  };
  const enter = () => {
    if (on) return;
    on = true;
    shown = null;
    el.style.display = "block";
    sync();
  };
  const leave = () => {
    if (!on) return;
    on = false;
    el.style.display = "none";
    canvas.classList.remove("civ5-soft-cursor");
    if (raf) cancelAnimationFrame(raf);
    raf = 0;
  };

  const move = (e) => {
    if (e.pointerType === "touch") return;
    // Locked: there is no cursor to draw, and the pointer's position does not move.
    if (pointerLocked(document)) {
      leave();
      return;
    }
    at = { x: e.clientX, y: e.clientY };
    if (inEdgeZone(at.x, at.y, window.innerWidth, window.innerHeight)) {
      enter();
      place();
    } else {
      leave();
    }
  };
  window.addEventListener("pointermove", move, { passive: true, capture: true });
  window.addEventListener("pointerdown", move, { passive: true, capture: true });
  // The pointer left the page: there is no cursor to draw.
  document.documentElement.addEventListener("pointerleave", leave);
  window.addEventListener("blur", leave);
  // The lock begins: the soft cursor goes at once, not at the next pointer move.
  document.addEventListener("pointerlockchange", () => {
    if (pointerLocked(document)) leave();
  });
  window.__civ5CursorEdge = { active: () => on, element: el };
}

if (typeof document !== "undefined" && typeof window !== "undefined") {
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", start);
  else start();
}
