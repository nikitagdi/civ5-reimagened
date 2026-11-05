// The login page: two real <form>s (sign in, create account) posted with fetch to /api/auth/*. The answer sets the
// HttpOnly cookie civ5_session (script never sees the device token); the page remembers only the nick (civ5.nick, not
// secret) and tells the wasm to sign in ({"Login":{nick,password:""}} through window.__civ5ui): the WebSocket upgrade
// carries the cookie. ui.js decides when the page is shown (before the wasm loads when this browser never signed in,
// and whenever the app reports the login screen).
//
// One-time migration: an earlier build kept the token in localStorage (civ5.token.<nick>). `ready` posts that token once
// to /api/auth/adopt (the server turns it into the cookie), then every civ5.token.* key is deleted, whatever the answer.
(function () {
  "use strict";

  const NICK_KEY = "civ5.nick";
  const TOKEN_PREFIX = "civ5.token.";

  function store(op, key, value) {
    try {
      if (op === "get") return localStorage.getItem(key);
      if (op === "set") localStorage.setItem(key, value);
      if (op === "remove") localStorage.removeItem(key);
    } catch (_) {}
    return null;
  }

  const $ = (id) => document.getElementById(id);
  const root = $("civ5-auth");
  if (!root) return;
  const tabs = { login: $("tab-login"), register: $("tab-register") };
  const forms = { login: $("form-login"), register: $("form-register") };
  const appError = $("auth-app-error");
  const appButtons = $("auth-app-buttons");
  let info = { registration: "open", min_password: 8 };
  let infoLoaded = false;

  function selectTab(name) {
    for (const k of Object.keys(forms)) {
      forms[k].hidden = k !== name;
      tabs[k].setAttribute("aria-selected", String(k === name));
    }
  }
  tabs.login.addEventListener("click", () => selectTab("login"));
  tabs.register.addEventListener("click", () => selectTab("register"));

  function showError(form, text, field) {
    const box = form.querySelector(".err");
    box.textContent = text || "";
    box.hidden = !text;
    for (const el of form.elements) el.removeAttribute && el.removeAttribute("aria-invalid");
    if (field && form.elements[field]) form.elements[field].setAttribute("aria-invalid", "true");
  }

  async function loadInfo() {
    if (infoLoaded) return;
    try {
      const r = await fetch("/api/auth/info", { cache: "no-store" });
      if (r.ok) {
        const j = await r.json();
        if (j && (j.registration === "open" || j.registration === "code")) info = j;
        infoLoaded = true;
      }
    } catch (_) {}
    $("reg-code-row").hidden = info.registration !== "code";
    const min = Number(info.min_password) || 8;
    $("reg-min").textContent = "(at least " + min + " characters)";
  }

  /** The text of an error answer. */
  async function errorOf(r) {
    let j = null;
    try {
      j = await r.json();
    } catch (_) {}
    let text = j && j.error ? String(j.error) : "";
    if (r.status === 429) {
      const wait = r.headers.get("Retry-After");
      text = (text || "Too many attempts.") + (wait ? " Try again in " + wait + " s." : " Try again later.");
    }
    if (!text) text = r.status === 404 || r.status >= 500 ? "Cannot reach the server." : "Something went wrong (" + r.status + ").";
    return { text, field: j && j.field };
  }

  function succeed(form, nick) {
    store("set", NICK_KEY, nick);
    try {
      if (navigator.credentials && navigator.credentials.store && window.PasswordCredential) {
        navigator.credentials.store(new PasswordCredential(form)).catch(() => {});
      }
    } catch (_) {}
    form.reset();
    api.hide();
    window.__civ5ui.post({ Login: { nick, password: "" } });
  }

  async function submit(ev, name) {
    ev.preventDefault();
    const form = forms[name];
    const btn = form.querySelector("button[type=submit]");
    const username = form.elements.username.value.trim();
    const password = form.elements.password.value;
    if (!username) return showError(form, "Enter your nick.", "username");
    if (!password) return showError(form, "Enter your password.", "password");
    if (name === "register") {
      const min = Number(info.min_password) || 8;
      if (password.length < min) return showError(form, "The password needs at least " + min + " characters.", "password");
      if (password !== form.elements.password2.value) return showError(form, "The passwords differ.", "password2");
    }
    const body = new URLSearchParams();
    body.set("username", username);
    body.set("password", password);
    if (name === "register") {
      if (info.registration === "code") body.set("code", form.elements.code.value);
    }
    showError(form, "");
    btn.disabled = true;
    try {
      const r = await fetch(form.getAttribute("action"), {
        method: "POST",
        headers: { "content-type": "application/x-www-form-urlencoded" },
        body,
        cache: "no-store",
      });
      if (r.ok) {
        let j = null;
        try {
          j = await r.json();
        } catch (_) {}
        if (j && j.nick) return succeed(form, String(j.nick));
        return showError(form, "Cannot reach the server.");
      }
      const e = await errorOf(r);
      showError(form, e.text, e.field);
    } catch (_) {
      showError(form, "Cannot reach the server.");
    } finally {
      btn.disabled = false;
    }
  }
  forms.login.addEventListener("submit", (ev) => submit(ev, "login"));
  forms.register.addEventListener("submit", (ev) => submit(ev, "register"));

  /** Old tokens of `localStorage` -> the cookie (once), then none are left. Never rejects. */
  async function adoptOld() {
    try {
      const nick = store("get", NICK_KEY);
      const token = nick && store("get", TOKEN_PREFIX + nick);
      if (token && /^[0-9a-f]{32}$/.test(token)) {
        await fetch("/api/auth/adopt", {
          method: "POST",
          headers: { "content-type": "application/x-www-form-urlencoded" },
          body: new URLSearchParams({ username: nick, token }),
          cache: "no-store",
        }).catch(() => {});
      }
    } catch (_) {}
    try {
      const keys = [];
      for (let i = 0; i < localStorage.length; i++) {
        const k = localStorage.key(i);
        if (k && k.startsWith(TOKEN_PREFIX)) keys.push(k);
      }
      for (const k of keys) localStorage.removeItem(k);
    } catch (_) {}
  }

  const api = {
    /** Resolves when the migration is done; bootstrap.js waits for it before the app opens its socket. */
    ready: adoptOld(),
    /** True when this browser signed in before (the cookie itself is HttpOnly and cannot be asked). */
    mayHaveSession() {
      return !!store("get", NICK_KEY);
    },
    /** Shows the page; `view` is the app's LoginView or null (before the wasm loads). */
    show(view) {
      loadInfo();
      root.hidden = false;
      window.__civ5ui.setAuthVisible(true);
      const text = view && view.error ? view.error : "";
      appError.textContent = text;
      appError.hidden = !text;
      appButtons.textContent = "";
      if (view && (view.button === "Reload" || view.button === "Retry")) {
        const b = document.createElement("button");
        b.type = "button";
        b.textContent = view.button;
        b.addEventListener("click", () => window.__civ5ui.post(view.button));
        appButtons.append(b);
      }
      appButtons.hidden = !appButtons.firstChild;
      // "guest" is the client's placeholder nick, not a name to offer to someone who has none.
      const nick = view && view.nick !== "guest" ? view.nick : "";
      for (const f of [forms.login, forms.register]) {
        const u = f.elements.username;
        if (nick && !u.value && document.activeElement !== u) u.value = nick;
      }
    },
    hide() {
      root.hidden = true;
      window.__civ5ui.setAuthVisible(false);
    },
    visible() {
      return !root.hidden;
    },
    /** Drops the device of the cookie on the server and clears the cookie (fire and forget). */
    logout() {
      try {
        fetch("/api/auth/logout", {
          method: "POST",
          headers: { "content-type": "application/x-www-form-urlencoded" },
          body: "",
          cache: "no-store",
          keepalive: true,
        }).catch(() => {});
      } catch (_) {}
    },
  };
  window.__civ5auth = api;
})();
