//! `tracing` to the browser console (by level), plus panic capture. Every line carries the absolute UTC time: the console
//! lines start with an RFC 3339 stamp (as the server's `tracing` output) and the ring's lines have `ts`, unix ms. The time
//! comes from `Date.now()`: `SystemTime` panics on wasm32.

use std::cell::{Cell, RefCell};
use std::io;

use tracing::{Level, Metadata};
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::{Layer, Registry};
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::wasm_bindgen;

#[derive(Debug, Default, Clone, Copy)]
pub struct ConsoleMakeWriter;

/// Flushed to the console on drop.
pub struct ConsoleWriter {
    level: Level,
    buf: Vec<u8>,
}

impl io::Write for ConsoleWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for ConsoleWriter {
    fn drop(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        let text = String::from_utf8_lossy(&self.buf);
        // The server's console form: an RFC 3339 UTC stamp first (SIMP-exe-1002).
        let stamped = format!(
            "{} {}",
            rfc3339_utc(js_sys::Date::now() as i64),
            text.trim_end()
        );
        let msg = wasm_bindgen::JsValue::from_str(&stamped);
        match self.level {
            Level::ERROR => web_sys::console::error_1(&msg),
            Level::WARN => web_sys::console::warn_1(&msg),
            Level::INFO => web_sys::console::info_1(&msg),
            _ => web_sys::console::debug_1(&msg),
        }
    }
}

impl<'a> MakeWriter<'a> for ConsoleMakeWriter {
    type Writer = ConsoleWriter;

    fn make_writer(&'a self) -> Self::Writer {
        ConsoleWriter {
            level: Level::INFO,
            buf: Vec::new(),
        }
    }

    fn make_writer_for(&'a self, meta: &Metadata<'_>) -> Self::Writer {
        ConsoleWriter {
            level: *meta.level(),
            buf: Vec::new(),
        }
    }
}

// ------------------------------------------------------------------------------------------ the log ring

/// Level numbers of the ring (`clientlog.js`): 1 error, 2 warn, 3 info, 4 debug, 5 trace.
fn level_number(level: &Level) -> u8 {
    match *level {
        Level::ERROR => 1,
        Level::WARN => 2,
        Level::INFO => 3,
        Level::DEBUG => 4,
        Level::TRACE => 5,
    }
}

fn level_name(n: u8) -> &'static str {
    match n {
        1 => "error",
        2 => "warn",
        3 => "info",
        4 => "debug",
        _ => "trace",
    }
}

/// A JSON string literal.
fn json_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Unix ms as an RFC 3339 UTC stamp with milliseconds (`2026-11-01T10:00:00.123Z`), the form of the server's `tracing`
/// lines.
pub fn rfc3339_utc(unix_ms: i64) -> String {
    let days = unix_ms.div_euclid(86_400_000);
    let ms_of_day = unix_ms.rem_euclid(86_400_000);
    // Civil date from the day count (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        ms_of_day / 3_600_000,
        ms_of_day / 60_000 % 60,
        ms_of_day / 1000 % 60,
        ms_of_day % 1000
    )
}

/// `name` when the message starts with a dotted event name and a colon (`gc.blob: a snapshot is loaded`).
fn event_name(msg: &str) -> Option<&str> {
    let head = msg.split_once(':')?.0;
    let ok = head.contains('.')
        && head.len() <= 40
        && head
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_');
    ok.then_some(head)
}

/// Longest message kept in a ring line, in bytes (a Lua traceback is up to 2000 characters).
const MAX_MSG: usize = 4000;

#[derive(Default)]
struct LineVisitor {
    msg: String,
    fields: String,
    json: Option<String>,
}

impl LineVisitor {
    fn field(&mut self, name: &str, value: &str, quoted: bool) {
        if !self.fields.is_empty() {
            self.fields.push(',');
        }
        json_str(name, &mut self.fields);
        self.fields.push(':');
        if quoted {
            let cut: String = value.chars().take(500).collect();
            json_str(&cut, &mut self.fields);
        } else {
            self.fields.push_str(value);
        }
    }
}

impl tracing::field::Visit for LineVisitor {
    fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
        let text = format!("{v:?}");
        match f.name() {
            "message" => self.msg = text,
            "json" => self.json = Some(text),
            name => self.field(name, &text, true),
        }
    }

    fn record_str(&mut self, f: &tracing::field::Field, v: &str) {
        match f.name() {
            "message" => self.msg = v.to_owned(),
            "json" => self.json = Some(v.to_owned()),
            name => self.field(name, v, true),
        }
    }

    fn record_i64(&mut self, f: &tracing::field::Field, v: i64) {
        self.field(f.name(), &v.to_string(), false);
    }

    fn record_u64(&mut self, f: &tracing::field::Field, v: u64) {
        self.field(f.name(), &v.to_string(), false);
    }

    fn record_f64(&mut self, f: &tracing::field::Field, v: f64) {
        let text = if v.is_finite() {
            format!("{v}")
        } else {
            "null".into()
        };
        self.field(f.name(), &text, false);
    }

    fn record_bool(&mut self, f: &tracing::field::Field, v: bool) {
        self.field(f.name(), if v { "true" } else { "false" }, false);
    }
}

thread_local! {
    /// Lines made since the last hand-over to the ring, with the most important level among them.
    static QUEUE: RefCell<(Vec<String>, u8)> = const { RefCell::new((Vec::new(), 9)) };
    /// Whether this thread is the page's (a worker has no `window`; its lines are not kept).
    static ON_PAGE: Cell<Option<bool>> = const { Cell::new(None) };
}

/// Lines waiting on the page at most (a page that never hands them over cannot grow without end).
const QUEUE_MAX: usize = 2000;

fn on_page() -> bool {
    ON_PAGE.with(|c| {
        c.get().unwrap_or_else(|| {
            let page = web_sys::window().is_some();
            c.set(Some(page));
            page
        })
    })
}

#[wasm_bindgen]
extern "C" {
    /// `window.__civ5log.push(level, text)` (`apps/web/www/clientlog.js`): `text` holds lines joined by `\n`.
    #[wasm_bindgen(js_namespace = ["window", "__civ5log"], js_name = push, catch)]
    fn ring_push(level: u8, text: &str) -> Result<(), JsValue>;
    /// `window.__civ5log.flush(urgent)`: writes the ring's lines to IndexedDB now (`urgent`: in the next task, no gzip).
    #[wasm_bindgen(js_namespace = ["window", "__civ5log"], js_name = flush, catch)]
    fn ring_flush(urgent: bool) -> Result<(), JsValue>;
}

/// Hands the queued lines to the ring (a failed call, no ring on the page, drops them).
pub fn flush_ring() {
    let (lines, level) = QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        let level = std::mem::replace(&mut q.1, 9);
        (std::mem::take(&mut q.0), level)
    });
    if lines.is_empty() {
        return;
    }
    let _ = ring_push(level.min(5), &lines.join("\n"));
}

/// A line made by hand (the panic hook): queued and handed over at once, then flushed to storage.
pub fn ring_line_now(level: u8, target: &str, msg: &str) {
    if !on_page() {
        return;
    }
    let mut line = String::new();
    line.push_str(&format!(
        "{{\"ts\":{},\"kind\":\"log\",\"src\":\"client\",\"lvl\":\"{}\",\"target\":",
        js_sys::Date::now() as i64,
        level_name(level)
    ));
    json_str(target, &mut line);
    line.push_str(",\"msg\":");
    let cut: String = msg.chars().take(MAX_MSG).collect();
    json_str(&cut, &mut line);
    line.push('}');
    queue_line(line, level);
    flush_ring();
    let _ = ring_flush(true);
}

fn queue_line(line: String, level: u8) {
    QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        if q.0.len() >= QUEUE_MAX {
            q.0.remove(0);
        }
        q.0.push(line);
        q.1 = q.1.min(level);
    });
}

/// The `tracing` layer of the log ring: every event the filter lets through becomes one JSON line of the shared
/// envelope (`{"ts","kind":"log","src":"client","lvl","target","name"?,"msg","f":{..}}`); an event with a `json` field and the target
/// `civ5::span` is a finished envelope line (the turn timeline's spans) and goes in as it is. A warn or an error
/// is handed over at once, the rest by the timer ([`start_ring_timer`]).
pub struct RingLayer;

/// The target of the lines that are finished envelope lines (kept out of the console).
pub const SPAN_TARGET: &str = "civ5::span";

impl<S: tracing::Subscriber> Layer<S> for RingLayer {
    fn on_event(&self, ev: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        if !on_page() {
            return;
        }
        let meta = ev.metadata();
        let mut v = LineVisitor::default();
        ev.record(&mut v);
        let level = level_number(meta.level());
        let line = if meta.target() == SPAN_TARGET
            && let Some(json) = v.json.take()
        {
            json
        } else {
            let mut line = format!(
                "{{\"ts\":{},\"kind\":\"log\",\"src\":\"client\",\"lvl\":\"{}\",\"target\":",
                js_sys::Date::now() as i64,
                level_name(level)
            );
            json_str(meta.target(), &mut line);
            if let Some(name) = event_name(&v.msg) {
                line.push_str(",\"name\":");
                json_str(name, &mut line);
            }
            line.push_str(",\"msg\":");
            let cut: String = v.msg.chars().take(MAX_MSG).collect();
            json_str(&cut, &mut line);
            if !v.fields.is_empty() {
                line.push_str(",\"f\":{");
                line.push_str(&v.fields);
                line.push('}');
            }
            line.push('}');
            line
        };
        queue_line(line, level);
        if level <= 2 {
            flush_ring();
        }
    }
}

/// Hands the queued lines to the ring four times a second (a page that does not draw frames still logs). The timer
/// lives as long as the page.
///
/// SIMPLIFIED(SIMP-uic-53): a timer, not once per frame.
pub fn start_ring_timer() {
    let Some(window) = web_sys::window() else {
        return;
    };
    let tick = Closure::<dyn FnMut()>::new(flush_ring);
    let _ = window
        .set_interval_with_callback_and_timeout_and_arguments_0(tick.as_ref().unchecked_ref(), 250);
    tick.forget();
}

/// The ring keeps debug lines only when asked (`?log=debug`, or `civ5.log` = `debug` in localStorage).
fn ring_level() -> LevelFilter {
    let from_url = web_sys::window()
        .and_then(|w| w.location().search().ok())
        .and_then(|s| web_sys::UrlSearchParams::new_with_str(&s).ok())
        .and_then(|p| p.get("log"));
    let from_store = web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item("civ5.log").ok().flatten());
    match from_url.or(from_store).as_deref() {
        Some("debug") => LevelFilter::DEBUG,
        Some("trace") => LevelFilter::TRACE,
        _ => LevelFilter::INFO,
    }
}

/// Installs the console subscriber, the log ring's layer and the panic hook. Repeat calls do nothing. A panic is
/// logged, kept in the ring (flushed to IndexedDB before the overlay shows) and shown over the canvas (`show_crash`):
/// after it the frame loop is dead, the page would stay black. The console shows `max_level` and above; the ring
/// keeps info (debug with `?log=debug`) and the finished span lines the console does not show.
pub fn init_logging(max_level: Level) {
    std::panic::set_hook(Box::new(|info| {
        let text = format!("panic: {info}");
        web_sys::console::error_1(&text.as_str().into());
        ring_line_now(1, "panic", &text);
        show_crash("The game stopped because of an internal error.", &text);
    }));
    let console = tracing_subscriber::fmt::layer()
        .with_writer(ConsoleMakeWriter)
        .without_time()
        .with_ansi(false)
        .with_filter(
            Targets::new()
                .with_default(LevelFilter::from_level(max_level))
                .with_target(SPAN_TARGET, LevelFilter::OFF),
        );
    let ring = RingLayer.with_filter(ring_level());
    let subscriber = Registry::default().with(console).with(ring);
    if tracing::subscriber::set_global_default(subscriber).is_ok() {
        start_ring_timer();
    }
}

/// The id of the crash overlay (`show_crash`).
pub const CRASH_ID: &str = "civ5-crash";

/// Shows a fatal error over the page, plain DOM (no wasm runs after it): `title`, `detail` (the
/// panic or the device's message) and a Reload link. The first call wins; the text also goes to
/// `window.__civ5crash` for the tests. Works only on the main thread (a worker has no document).
pub fn show_crash(title: &str, detail: &str) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let _ = js_sys::Reflect::set(
        &window,
        &"__civ5crash".into(),
        &format!("{title} {detail}").into(),
    );
    let Some(doc) = window.document() else {
        return;
    };
    if doc.get_element_by_id(CRASH_ID).is_some() {
        return;
    }
    let Some(body) = doc.body() else {
        return;
    };
    let make = |tag: &str, style: &str, text: &str| -> Option<web_sys::Element> {
        let e = doc.create_element(tag).ok()?;
        let _ = e.set_attribute("style", style);
        e.set_text_content(Some(text));
        Some(e)
    };
    let Some(panel) = make(
        "div",
        "position:fixed;inset:0;z-index:2147483647;display:flex;flex-direction:column;align-items:center;justify-content:center;gap:14px;padding:24px;box-sizing:border-box;background:rgba(5,8,15,0.94);color:#e6ebf2;font:15px/1.5 system-ui,sans-serif;text-align:center",
        "",
    ) else {
        return;
    };
    let _ = panel.set_attribute("id", CRASH_ID);
    let _ = panel.set_attribute("role", "alert");
    let detail: String = detail.chars().take(2000).collect();
    let parts = [
        make("div", "font-size:20px;font-weight:600", title),
        make(
            "pre",
            "max-width:min(900px,100%);max-height:40vh;overflow:auto;margin:0;padding:10px 12px;background:#0d1320;border:1px solid #26324a;border-radius:6px;color:#aab6c8;font:12px/1.4 ui-monospace,monospace;text-align:left;white-space:pre-wrap;word-break:break-word",
            &detail,
        ),
        make(
            "div",
            "color:#8a96aa;font-size:13px",
            "The console of the browser (F12) has the details. Your game goes on on the server.",
        ),
        make(
            "a",
            "padding:8px 22px;border-radius:6px;background:#2f6fd6;color:#fff;text-decoration:none;font-weight:600",
            "Reload",
        ),
    ];
    for part in parts.into_iter().flatten() {
        if part.tag_name() == "A" {
            let href = window.location().href().unwrap_or_default();
            let _ = part.set_attribute("href", &href);
        }
        let _ = panel.append_child(&part);
    }
    // The full report (the logs and a note, `clientlog.js`); the panel is plain DOM, so it works with the module dead.
    if let Some(full) = make(
        "button",
        "padding:6px 16px;border-radius:6px;border:1px solid #2f6fd6;background:#2f6fd6;color:#fff;cursor:pointer",
        "Send a full report",
    ) {
        let _ = full.set_attribute(
            "onclick",
            "window.__civ5log && window.__civ5log.openReport && window.__civ5log.openReport()",
        );
        let _ = panel.append_child(&full);
    }
    let _ = body.append_child(&panel);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_stamps_are_rfc3339_utc() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339_utc(951_782_400_000), "2000-02-29T00:00:00.000Z");
        assert_eq!(rfc3339_utc(1_761_909_000_123), "2025-10-31T11:10:00.123Z");
        assert_eq!(rfc3339_utc(-1), "1969-12-31T23:59:59.999Z");
    }

    #[test]
    fn a_dotted_name_before_a_colon_is_the_event_name() {
        assert_eq!(event_name("gc.blob: a snapshot is loaded"), Some("gc.blob"));
        assert_eq!(event_name("net.login: logged in"), Some("net.login"));
        assert_eq!(event_name("replica: diverged"), None, "no dot, not a name");
        assert_eq!(event_name("the UI: it said so. ok"), None);
        assert_eq!(event_name("no colon at all"), None);
        assert_eq!(event_name("Gc.Blob: capitals are not names"), None);
    }

    #[test]
    fn json_strings_escape_quotes_slashes_and_control_characters() {
        let mut out = String::new();
        json_str("a\"b\\c\nd\u{1}e", &mut out);
        assert_eq!(out, "\"a\\\"b\\\\c\\nd\\u0001e\"");
    }

    #[test]
    fn levels_have_the_numbers_the_ring_uses() {
        assert_eq!(level_number(&Level::ERROR), 1);
        assert_eq!(level_number(&Level::DEBUG), 4);
        assert_eq!(level_name(2), "warn");
        assert_eq!(level_name(3), "info");
    }
}
