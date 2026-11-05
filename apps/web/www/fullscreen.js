// Full screen on a phone or tablet (live report 2026-11-02: the browser's address bar and status bar cover much of
// the game).
//
// Three ways, each degrading to the next:
//  1. A "full screen" button (shown on touch devices that have the Fullscreen API: Android Chrome, Firefox, iPadOS
//     Safari) that calls `requestFullscreen()` on the user's tap (the API refuses anything but a user gesture), then
//     tries to lock the landscape orientation (works in full screen on Android). The choice is remembered: the next
//     visit enters full screen on the first tap, as a page cannot do it on its own.
//  2. The web app manifest (`manifest.webmanifest`: `display: fullscreen`, falling back to `standalone`): "Add to home
//     screen" (Android Chrome: "Install app") then opens the game without any browser bars.
//  3. iPhone Safari has no Fullscreen API for pages at all; there the button shows how to add the page to the home
//     screen (`apple-mobile-web-app-capable` in index.html makes that launch bar-less).
//
// SIMPLIFIED(SIMP-exe-1081): the original is a desktop game and has no such thing. The button is a page element,
// not part of the game's interface; it hides once the page is full screen or installed.

const KEY = "civ5.fullscreen";

export const fsApi = (doc = document) => {
  const el = doc.documentElement;
  return {
    request: el.requestFullscreen
      ? (o) => el.requestFullscreen(o)
      : el.webkitRequestFullscreen
        ? () => el.webkitRequestFullscreen()
        : null,
    current: () => doc.fullscreenElement || doc.webkitFullscreenElement || null,
  };
};

/** What to offer: "button" (the API works), "ios-hint" (add to the home screen), or "none" (nothing to offer). */
export function plan({ touch, hasApi, ios, standalone, fullscreen }) {
  if (!touch || standalone || fullscreen) return "none";
  if (hasApi) return "button";
  return ios ? "ios-hint" : "none";
}

/** The page runs as an installed app (home screen shortcut), without browser bars. */
function isStandalone() {
  try {
    if (navigator.standalone) return true;
    return ["fullscreen", "standalone", "minimal-ui"].some((m) => matchMedia(`(display-mode: ${m})`).matches);
  } catch (_) {
    return false;
  }
}

const isIos = () =>
  /iPad|iPhone|iPod/.test(navigator.userAgent) ||
  // iPadOS reports a Mac.
  (navigator.platform === "MacIntel" && navigator.maxTouchPoints > 1);

function remembered() {
  try {
    return localStorage.getItem(KEY) === "1";
  } catch (_) {
    return false;
  }
}
function remember(on) {
  try {
    if (on) localStorage.setItem(KEY, "1");
    else localStorage.removeItem(KEY);
  } catch (_) {}
}

async function enter(api) {
  try {
    await api.request({ navigationUI: "hide" });
  } catch (_) {
    // Safari's webkit prefix takes no options and returns nothing; a refusal leaves the page as it was.
  }
  try {
    await screen.orientation.lock("landscape");
  } catch (_) {}
}

function start() {
  const api = fsApi();
  const touch = matchMedia("(pointer: coarse)").matches || navigator.maxTouchPoints > 0;
  const state = () => ({
    touch,
    hasApi: !!api.request && (document.fullscreenEnabled ?? document.webkitFullscreenEnabled ?? true),
    ios: isIos(),
    standalone: isStandalone(),
    fullscreen: !!api.current(),
  });
  if (plan(state()) === "none") return;

  const btn = document.createElement("button");
  btn.id = "civ5-fs-button";
  btn.type = "button";
  btn.hidden = true;
  btn.setAttribute("aria-label", "Full screen");
  btn.innerHTML =
    '<svg viewBox="0 0 24 24" width="22" height="22" aria-hidden="true"><path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>';
  const hint = document.createElement("div");
  hint.id = "civ5-fs-hint";
  hint.hidden = true;
  hint.textContent =
    "To play without the browser bars: tap Share, then Add to Home Screen, and open the game from the icon.";
  const style = document.createElement("style");
  style.textContent =
    "#civ5-fs-button{position:fixed;left:max(6px,env(safe-area-inset-left));top:50%;transform:translateY(-50%);" +
    "z-index:2147483000;width:40px;height:56px;border:1px solid rgba(255,255,255,.35);border-radius:10px;" +
    "background:rgba(10,14,22,.62);color:#e8edf6;opacity:.5;display:flex;align-items:center;justify-content:center;" +
    "padding:0;-webkit-tap-highlight-color:transparent;touch-action:manipulation}" +
    "#civ5-fs-button:active{opacity:1}#civ5-fs-button[hidden],#civ5-fs-hint[hidden]{display:none}" +
    "#civ5-fs-hint{position:fixed;left:56px;right:16px;top:50%;transform:translateY(-50%);z-index:2147483000;" +
    "padding:10px 12px;border-radius:10px;background:rgba(10,14,22,.92);color:#e8edf6;font:14px/1.4 system-ui,sans-serif}";
  document.head.appendChild(style);
  document.body.append(btn, hint);

  const refresh = () => {
    const p = plan(state());
    btn.hidden = p === "none";
    if (p === "none") hint.hidden = true;
  };
  btn.addEventListener("click", () => {
    if (plan(state()) === "ios-hint") {
      hint.hidden = !hint.hidden;
      return;
    }
    remember(true);
    enter(api);
  });
  const changed = () => {
    // Leaving full screen on purpose (the back gesture) forgets the choice; entering it hides the button.
    if (!api.current()) remember(false);
    refresh();
  };
  document.addEventListener("fullscreenchange", changed);
  document.addEventListener("webkitfullscreenchange", changed);
  try {
    matchMedia("(display-mode: standalone)").addEventListener("change", refresh);
  } catch (_) {}
  refresh();

  // The next visit: the first touch is the gesture the API needs.
  if (remembered() && plan(state()) === "button") {
    const once = () => {
      document.removeEventListener("pointerup", once, true);
      if (!api.current()) enter(api);
    };
    document.addEventListener("pointerup", once, true);
  }
}

if (typeof document !== "undefined" && typeof window !== "undefined") {
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", start);
  else start();
}
