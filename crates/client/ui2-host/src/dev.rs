//! Hot reload of a skin from its sources (`?ui2dev=<url>`): the client side of `cargo xtask ui watch`.
//!
//! The watch server rebuilds the package on every saved change and serves
//!
//! ```text
//! GET <url>/pkg/<id>.rev           the number of packages built so far (grows with every good build)
//! GET <url>/pkg/<id>.ui2           the last good package
//! GET <url>/pkg/<id>.errors.json   the errors of the last build ([] when it built)
//! ```
//!
//! [`DevReload`] asks for `.rev` and `errors.json` every [`POLL_MS`]; a number above the one applied brings the
//! package, read and handed to the host as [`DevEvent::Package`] (the host applies it with `V2Ui::reload`, which
//! keeps the model, the open windows and the skin's state). New converter errors become [`DevEvent::Errors`] (the
//! old package stays). Nothing here touches the mod set or the assets: this path is the skin's alone.
//!
//! No clock and no threads: the host calls [`DevReload::poll`] once a frame with its time.

use std::sync::Arc;

use platform::{HttpClient, HttpError, HttpOp, HttpResponse};
use ui2_package::Package;

/// How often the watch server is asked for a new build, ms.
// SIMPLIFIED(SIMP-210): the hot reload polls the watch server's `.rev` every 500 ms instead of being told of a new
// build (no socket between the watch server and the page).
pub const POLL_MS: f64 = 500.0;

/// What a poll brought.
pub enum DevEvent {
    /// A new build of the skin: its number and the read package (the hash and the client checks are the host's).
    Package { rev: u64, pkg: Arc<Package> },
    /// The last build failed: the converter's errors, as one line for a notice. The running package stays.
    Errors(String),
    /// The watch server could not be asked or answered badly (logged; asked again at the next poll).
    Unreachable(String),
}

/// The hot reload of one skin from one watch server.
pub struct DevReload {
    base: String,
    id: String,
    /// The last build number taken (0: none yet).
    rev: u64,
    next_ms: f64,
    rev_op: Option<Box<dyn HttpOp>>,
    errors_op: Option<Box<dyn HttpOp>>,
    /// The package being fetched and its number.
    pkg_op: Option<(u64, Box<dyn HttpOp>)>,
    /// The text of the last `errors.json` seen (a notice only when it changes).
    last_errors: String,
}

impl DevReload {
    /// Hot reload of skin `id` from the watch server at `base` (`http://127.0.0.1:8095`; a trailing `/` is
    /// dropped).
    pub fn new(base: &str, id: &str) -> DevReload {
        DevReload {
            base: base.trim().trim_end_matches('/').to_string(),
            id: id.to_string(),
            rev: 0,
            next_ms: f64::NEG_INFINITY,
            rev_op: None,
            errors_op: None,
            pkg_op: None,
            last_errors: String::new(),
        }
    }

    /// The skin it reloads.
    pub fn skin(&self) -> &str {
        &self.id
    }

    /// The last build number taken.
    pub fn rev(&self) -> u64 {
        self.rev
    }

    fn url(&self, ext: &str) -> String {
        format!("{}/pkg/{}.{ext}", self.base, self.id)
    }

    /// One step at `now_ms`: answers that came are taken, a poll is started when one is due.
    pub fn poll(&mut self, http: &dyn HttpClient, now_ms: f64) -> Vec<DevEvent> {
        let mut out = Vec::new();
        if let Some((rev, op)) = self.pkg_op.as_mut()
            && let Some(answer) = op.poll()
        {
            let rev = *rev;
            self.pkg_op = None;
            match body(answer).and_then(|b| {
                Package::read(Arc::from(b)).map_err(|e| format!("the package does not read: {e}"))
            }) {
                Ok(pkg) => {
                    self.rev = rev;
                    out.push(DevEvent::Package {
                        rev,
                        pkg: Arc::new(pkg),
                    });
                }
                Err(why) => out.push(DevEvent::Unreachable(format!("{}: {why}", self.url("ui2")))),
            }
        }
        if let Some(op) = self.rev_op.as_mut()
            && let Some(answer) = op.poll()
        {
            self.rev_op = None;
            let rev = body(answer).and_then(|b| {
                String::from_utf8_lossy(&b)
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| "not a number".to_string())
            });
            match rev {
                Ok(rev) if rev > self.rev && self.pkg_op.is_none() => {
                    let url = self.url("ui2");
                    tracing::info!(%url, rev, "UI v2 dev: a new build");
                    self.pkg_op = Some((rev, http.get(&url)));
                }
                Ok(_) => {}
                Err(why) => out.push(DevEvent::Unreachable(format!("{}: {why}", self.url("rev")))),
            }
        }
        if let Some(op) = self.errors_op.as_mut()
            && let Some(answer) = op.poll()
        {
            self.errors_op = None;
            if let Ok(b) = body(answer) {
                let text = String::from_utf8_lossy(&b).trim().to_string();
                if text != self.last_errors {
                    self.last_errors.clone_from(&text);
                    if let Some(line) = summary(&text) {
                        out.push(DevEvent::Errors(line));
                    }
                }
            }
        }
        if now_ms >= self.next_ms && self.rev_op.is_none() && self.errors_op.is_none() {
            self.next_ms = now_ms + POLL_MS;
            self.rev_op = Some(http.get(&self.url("rev")));
            self.errors_op = Some(http.get(&self.url("errors.json")));
        }
        out
    }
}

/// The body of a successful answer, or why there is none.
fn body(answer: Result<HttpResponse, HttpError>) -> Result<Vec<u8>, String> {
    match answer {
        Ok(r) if r.is_success() => Ok(r.body),
        Ok(r) => Err(format!("HTTP {}", r.status)),
        Err(e) => Err(e.to_string()),
    }
}

/// One line for the errors of `errors.json`: how many, and the first one's code, place and text. `None` when
/// there are none (or the text is not the converter's list).
fn summary(text: &str) -> Option<String> {
    let list: Vec<serde_json::Value> = serde_json::from_str(text).ok()?;
    let first = list.first()?;
    let s = |k: &str| first.get(k).and_then(|v| v.as_str()).unwrap_or("");
    let at = match (
        s("file"),
        first.get("line").and_then(serde_json::Value::as_u64),
    ) {
        ("", _) => String::new(),
        (f, Some(l)) => format!(" {f}:{l}"),
        (f, None) => format!(" {f}"),
    };
    let what = [s("what"), s("why")]
        .into_iter()
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(": ");
    let more = if list.len() > 1 {
        format!(" (and {} more)", list.len() - 1)
    } else {
        String::new()
    };
    Some(format!(
        "the skin did not build: {}{at} {what}{more}",
        s("code")
    ))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    use ui2_runtime::testkit::Kit;

    use super::*;

    /// Answers by URL, each ready at once; the answers can change between polls.
    #[derive(Default)]
    struct Server {
        files: RefCell<BTreeMap<String, Vec<u8>>>,
        asked: RefCell<Vec<String>>,
    }

    struct Ready(Option<Result<HttpResponse, HttpError>>);

    impl HttpOp for Ready {
        fn poll(&mut self) -> Option<Result<HttpResponse, HttpError>> {
            self.0.take()
        }
    }

    impl HttpClient for Server {
        fn get(&self, url: &str) -> Box<dyn HttpOp> {
            self.asked.borrow_mut().push(url.to_string());
            let answer = match self.files.borrow().get(url) {
                Some(b) => Ok(HttpResponse {
                    status: 200,
                    body: b.clone(),
                }),
                None => Ok(HttpResponse {
                    status: 404,
                    body: Vec::new(),
                }),
            };
            Box::new(Ready(Some(answer)))
        }
    }

    const BASE: &str = "http://127.0.0.1:8095";

    impl Server {
        fn put(&self, ext: &str, body: &[u8]) {
            self.files
                .borrow_mut()
                .insert(format!("{BASE}/pkg/alt.{ext}"), body.to_vec());
        }
    }

    fn packages(ev: &[DevEvent]) -> Vec<u64> {
        ev.iter()
            .filter_map(|e| match e {
                DevEvent::Package { rev, .. } => Some(*rev),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_new_rev_brings_the_package_once_and_the_same_rev_nothing() {
        let s = Server::default();
        let pkg = Kit::new().finish_skin("alt").expect("package");
        s.put("rev", b"0");
        s.put("errors.json", b"[]\n");
        let mut d = DevReload::new(&format!("{BASE}/"), "alt");
        // Rev 0: nothing built yet.
        assert!(d.poll(&s, 0.0).is_empty());
        assert!(d.poll(&s, 1.0).is_empty());
        s.put("rev", b"3\n");
        s.put("ui2", &pkg.bytes()[..]);
        // Not before the next poll is due.
        assert!(d.poll(&s, 100.0).is_empty());
        let asked = s.asked.borrow().len();
        assert!(d.poll(&s, 500.0).is_empty(), "the rev is asked for");
        assert_eq!(s.asked.borrow().len(), asked + 2, ".rev and errors.json");
        assert!(
            d.poll(&s, 516.0).is_empty(),
            "the rev came: the package is asked for"
        );
        assert_eq!(packages(&d.poll(&s, 532.0)), [3]);
        assert_eq!(d.rev(), 3);
        // The same number again: nothing.
        for t in [1000.0, 1016.0, 1032.0, 1500.0, 1516.0] {
            assert!(packages(&d.poll(&s, t)).is_empty());
        }
        assert_eq!(
            s.asked
                .borrow()
                .iter()
                .filter(|u| u.ends_with(".ui2"))
                .count(),
            1
        );
    }

    #[test]
    fn converter_errors_become_one_notice_each_time_they_change() {
        let s = Server::default();
        s.put("rev", b"0");
        s.put(
            "errors.json",
            br#"[{"code":"E2-002","file":"css/hud.css","line":12,"what":"not in the profile","why":"display: table"},{"code":"E1-001"}]"#,
        );
        let mut d = DevReload::new(BASE, "alt");
        d.poll(&s, 0.0);
        let ev = d.poll(&s, 1.0);
        let lines: Vec<_> = ev
            .iter()
            .filter_map(|e| match e {
                DevEvent::Errors(l) => Some(l.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].contains("E2-002") && lines[0].contains("css/hud.css:12"),
            "{}",
            lines[0]
        );
        assert!(lines[0].contains("1 more"));
        // The same errors again: no new notice.
        d.poll(&s, 600.0);
        assert!(
            d.poll(&s, 601.0)
                .iter()
                .all(|e| !matches!(e, DevEvent::Errors(_)))
        );
        // Fixed: `[]` gives no notice.
        s.put("errors.json", b"[]\n");
        d.poll(&s, 1200.0);
        assert!(
            d.poll(&s, 1201.0)
                .iter()
                .all(|e| !matches!(e, DevEvent::Errors(_)))
        );
    }

    #[test]
    fn a_server_that_is_not_there_is_reported_and_asked_again() {
        let s = Server::default();
        let mut d = DevReload::new(BASE, "alt");
        d.poll(&s, 0.0);
        let ev = d.poll(&s, 1.0);
        assert!(ev.iter().any(|e| matches!(e, DevEvent::Unreachable(_))));
        d.poll(&s, 600.0);
        assert_eq!(
            s.asked
                .borrow()
                .iter()
                .filter(|u| u.ends_with(".rev"))
                .count(),
            2
        );
    }
}
