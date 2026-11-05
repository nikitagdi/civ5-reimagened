//! Dependency graph model and rules R1–R10 as pure functions (tested on synthetic graphs).
//! Transitive rules follow `normal` edges only; direct rules also follow `build`; `dev` is ignored.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fmt;

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Layer {
    Shared,
    Build,
    Client,
    PlatformWeb,
    PlatformNative,
    AppWeb,
    AppDesktop,
    Server,
    Tool,
}

impl Layer {
    pub fn parse(s: &str) -> Option<Layer> {
        Some(match s {
            "shared" => Layer::Shared,
            "build" => Layer::Build,
            "client" => Layer::Client,
            "platform-web" => Layer::PlatformWeb,
            "platform-native" => Layer::PlatformNative,
            "app-web" => Layer::AppWeb,
            "app-desktop" => Layer::AppDesktop,
            "server" => Layer::Server,
            "tool" => Layer::Tool,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Layer::Shared => "shared",
            Layer::Build => "build",
            Layer::Client => "client",
            Layer::PlatformWeb => "platform-web",
            Layer::PlatformNative => "platform-native",
            Layer::AppWeb => "app-web",
            Layer::AppDesktop => "app-desktop",
            Layer::Server => "server",
            Layer::Tool => "tool",
        }
    }
}

impl fmt::Display for Layer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EdgeKind {
    Normal,
    Build,
    Dev,
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub to: String,
    pub kinds: BTreeSet<EdgeKindOrd>,
}

/// `EdgeKind` with an `Ord` impl, so it can live in a `BTreeSet`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct EdgeKindOrd(pub u8);

impl From<EdgeKind> for EdgeKindOrd {
    fn from(k: EdgeKind) -> Self {
        EdgeKindOrd(match k {
            EdgeKind::Normal => 0,
            EdgeKind::Build => 1,
            EdgeKind::Dev => 2,
        })
    }
}

impl Edge {
    fn has(&self, k: EdgeKind) -> bool {
        self.kinds.contains(&k.into())
    }
}

#[derive(Debug, Clone, Default)]
pub struct Node {
    /// `None` for external crates.
    pub layer: Option<Layer>,
    pub deps: Vec<Edge>,
}

#[derive(Debug, Clone, Default)]
pub struct Graph {
    pub nodes: BTreeMap<String, Node>,
}

impl Graph {
    pub fn add(&mut self, name: &str, layer: Option<Layer>) -> &mut Node {
        let n = self.nodes.entry(name.to_string()).or_default();
        if layer.is_some() {
            n.layer = layer;
        }
        n
    }

    pub fn edge(&mut self, from: &str, to: &str, kind: EdgeKind) {
        self.add(to, None);
        let n = self.add(from, None);
        if let Some(e) = n.deps.iter_mut().find(|e| e.to == to) {
            e.kinds.insert(kind.into());
        } else {
            n.deps.push(Edge {
                to: to.to_string(),
                kinds: BTreeSet::from([kind.into()]),
            });
        }
    }

    pub fn layer(&self, name: &str) -> Option<Layer> {
        self.nodes.get(name).and_then(|n| n.layer)
    }

    pub fn workspace(&self) -> impl Iterator<Item = (&String, Layer)> {
        self.nodes
            .iter()
            .filter_map(|(k, n)| n.layer.map(|l| (k, l)))
    }

    fn direct(&self, name: &str, with_build: bool) -> Vec<&str> {
        self.nodes
            .get(name)
            .map(|n| {
                n.deps
                    .iter()
                    .filter(|e| e.has(EdgeKind::Normal) || (with_build && e.has(EdgeKind::Build)))
                    .map(|e| e.to.as_str())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Shortest `normal`-edge path from `from` to the first node matching `goal`.
    fn find_path(&self, from: &str, goal: impl Fn(&str) -> bool) -> Option<Vec<String>> {
        self.find_path_avoiding(from, goal, |_| false)
    }

    /// Like `find_path`, but never enters a node for which `avoid` holds.
    fn find_path_avoiding(
        &self,
        from: &str,
        goal: impl Fn(&str) -> bool,
        avoid: impl Fn(&str) -> bool,
    ) -> Option<Vec<String>> {
        let mut prev: HashMap<&str, &str> = HashMap::new();
        let mut seen: BTreeSet<&str> = BTreeSet::from([from]);
        let mut q: VecDeque<&str> = VecDeque::from([from]);
        while let Some(cur) = q.pop_front() {
            for next in self.direct(cur, false) {
                if avoid(next) || !seen.insert(next) {
                    continue;
                }
                prev.insert(next, cur);
                if goal(next) {
                    let mut path = vec![next.to_string()];
                    let mut at = next;
                    while let Some(&p) = prev.get(at) {
                        path.push(p.to_string());
                        at = p;
                    }
                    path.reverse();
                    return Some(path);
                }
                q.push_back(next);
            }
        }
        None
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct DepsConfig {
    pub r2: R2,
    pub r5: R5,
    pub r10: R10,
}

/// R10 (the client holds a replica, never the rules). `forbidden` crates are unreachable from the client builds;
/// `direct` lists crates that the layers `client`, `platform-*` and `app-*` may depend on directly only through
/// the named crates (B4: `gamecore` only through `replica`). `warn` reports violations without failing the check;
/// `deny` fails. Exceptions are tolerated only in `warn`.
#[derive(Debug, Clone, Deserialize)]
pub struct R10 {
    pub mode: R10Mode,
    pub forbidden: Vec<String>,
    #[serde(default)]
    pub exceptions: Vec<R10Exception>,
    #[serde(default)]
    pub direct: Vec<R10Direct>,
}

/// A crate the client layers may name directly only from `allowed` crates.
#[derive(Debug, Clone, Deserialize)]
pub struct R10Direct {
    pub target: String,
    #[serde(default)]
    pub allowed: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum R10Mode {
    Warn,
    Deny,
}

/// A known reach `app → forbidden` that `warn` mode tolerates silently.
#[derive(Debug, Clone, Deserialize)]
pub struct R10Exception {
    pub app: String,
    pub forbidden: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct R2 {
    pub allowed_web_paths: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct R5 {
    pub protocol_allowed: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub rule: &'static str,
    pub message: String,
    /// A warning is reported but does not fail the check (R10 in `warn` mode).
    pub warning: bool,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.rule, self.message)
    }
}

fn arrow(path: &[String]) -> String {
    path.join(" → ")
}

fn viol(rule: &'static str, message: String) -> Violation {
    Violation {
        rule,
        message,
        warning: false,
    }
}

pub fn check_all(g: &Graph, cfg: &DepsConfig) -> Vec<Violation> {
    let mut v = Vec::new();
    v.extend(r1(g));
    v.extend(r2(g, cfg));
    v.extend(r3(g));
    v.extend(r4(g));
    v.extend(r5(g, cfg));
    v.extend(r6(g));
    v.extend(r7(g));
    v.extend(r9(g));
    v.extend(r10(g, cfg));
    v
}

const WEB_DIRECT: [&str; 4] = ["web-sys", "js-sys", "wasm-bindgen", "wasm-bindgen-futures"];

/// R1: direct web-sys/js-sys/wasm-bindgen deps only in platform-web and app-web.
pub fn r1(g: &Graph) -> Vec<Violation> {
    let mut out = Vec::new();
    for (name, layer) in g.workspace() {
        if matches!(layer, Layer::PlatformWeb | Layer::AppWeb) {
            continue;
        }
        for d in g.direct(name, true) {
            if WEB_DIRECT.contains(&d) {
                out.push(viol(
                    "R1",
                    format!(
                        "crate `{name}` (layer {layer}) depends directly on `{d}`: {name} → {d}; \
                         allowed only in the platform-web and app-web layers"
                    ),
                ));
            }
        }
    }
    out
}

/// Bad web-path intermediate: a non-allow-listed external crate, or a platform-web/app-web crate.
fn is_bad_waypoint(g: &Graph, n: &str, allowed: &BTreeSet<&str>) -> bool {
    match g.layer(n) {
        None => !allowed.contains(n),
        Some(l) => matches!(l, Layer::PlatformWeb | Layer::AppWeb),
    }
}

/// R2: web-sys/js-sys reachable from shared/client only via allow-listed crates (direct deps are R1).
pub fn r2(g: &Graph, cfg: &DepsConfig) -> Vec<Violation> {
    let allowed: BTreeSet<&str> = cfg
        .r2
        .allowed_web_paths
        .iter()
        .map(String::as_str)
        .collect();
    let is_target = |n: &str| n == "web-sys" || n == "js-sys";
    let mut out = Vec::new();
    for (start, layer) in g.workspace() {
        if !matches!(layer, Layer::Shared | Layer::Client) {
            continue;
        }
        // BFS over (node, saw a bad intermediate).
        type State<'a> = (&'a str, bool);
        let mut prev: HashMap<State, State> = HashMap::new();
        let init: State = (start.as_str(), false);
        let mut seen: BTreeSet<State> = BTreeSet::from([init]);
        let mut q: VecDeque<State> = VecDeque::from([init]);
        let mut found: Option<State> = None;
        'bfs: while let Some((cur, bad)) = q.pop_front() {
            for next in g.direct(cur, false) {
                if is_target(next) {
                    if bad {
                        prev.insert((next, true), (cur, bad));
                        found = Some((next, true));
                        break 'bfs;
                    }
                    continue;
                }
                let nbad = bad || is_bad_waypoint(g, next, &allowed);
                let st = (next, nbad);
                if seen.insert(st) {
                    prev.insert(st, (cur, bad));
                    q.push_back(st);
                }
            }
        }
        if let Some(end) = found {
            let mut path = vec![end.0.to_string()];
            let mut at = end;
            while let Some(&p) = prev.get(&at) {
                path.push(p.0.to_string());
                at = p;
            }
            path.reverse();
            // Already reported at the shared/client intermediate crate.
            if g.layer(&path[1])
                .is_some_and(|l| matches!(l, Layer::Shared | Layer::Client))
            {
                continue;
            }
            let bad_crates: Vec<&String> = path[1..path.len() - 1]
                .iter()
                .filter(|n| is_bad_waypoint(g, n, &allowed))
                .collect();
            out.push(viol(
                "R2",
                format!(
                    "`{start}` (layer {layer}) reaches {} outside the allow-list: {} \
                     (not permitted by R2: not in deps.toml r2.allowed_web_paths, \
                     or it is a platform-web/app-web crate: {})",
                    path.last().map(String::as_str).unwrap_or("web-sys"),
                    arrow(&path),
                    bad_crates
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
    }
    out
}

fn is_client_side(l: Layer) -> bool {
    matches!(
        l,
        Layer::Client
            | Layer::PlatformWeb
            | Layer::PlatformNative
            | Layer::AppWeb
            | Layer::AppDesktop
    )
}

/// R3: server and client-side layers are separated; apps are leaves.
pub fn r3(g: &Graph) -> Vec<Violation> {
    use Layer::*;
    let mut out = Vec::new();
    for (name, layer) in g.workspace() {
        let (bad_side, descr): (fn(Layer) -> bool, &str) = match layer {
            Server => (is_client_side, "client, platform-* and app-*"),
            Client => (
                |l| matches!(l, Server | AppWeb | AppDesktop),
                "server and app-*",
            ),
            PlatformWeb => (
                |l| matches!(l, Server | AppWeb | AppDesktop | PlatformNative),
                "server, app-* and platform-native",
            ),
            PlatformNative => (
                |l| matches!(l, Server | AppWeb | AppDesktop | PlatformWeb),
                "server, app-* and platform-web",
            ),
            AppWeb => (
                |l| matches!(l, Server | AppDesktop | PlatformNative),
                "server, app-desktop and platform-native",
            ),
            AppDesktop => (
                |l| matches!(l, Server | AppWeb | PlatformWeb),
                "server, app-web and platform-web",
            ),
            _ => continue,
        };
        if let Some(path) = g.find_path(name, |n| g.layer(n).is_some_and(bad_side)) {
            out.push(viol(
                "R3",
                format!(
                    "crate `{name}` (layer {layer}) depends on a layer the rule forbids ({descr}): {}",
                    arrow(&path)
                ),
            ));
        }
    }
    out
}

/// R4: shared depends only on shared (`build` only as a build-dependency).
pub fn r4(g: &Graph) -> Vec<Violation> {
    let mut out = Vec::new();
    for (name, layer) in g.workspace() {
        if layer != Layer::Shared {
            continue;
        }
        let bad = |n: &str| g.layer(n).is_some_and(|l| l != Layer::Shared);
        if let Some(path) = g.find_path(name, bad) {
            let last = path.last().map(String::as_str).unwrap_or("");
            let last_layer = g.layer(last).map(Layer::as_str).unwrap_or("?");
            out.push(viol(
                "R4",
                format!(
                    "shared crate `{name}` depends on layer {last_layer} (`{last}`); shared may \
                     depend only on shared (and on build via a build-dependency): {}",
                    arrow(&path)
                ),
            ));
        }
    }
    out
}

/// R5: direct deps of protocol only from `r5.protocol_allowed`.
pub fn r5(g: &Graph, cfg: &DepsConfig) -> Vec<Violation> {
    let mut out = Vec::new();
    if g.layer("protocol").is_none() {
        return out;
    }
    for d in g.direct("protocol", true) {
        if !cfg.r5.protocol_allowed.iter().any(|a| a == d) {
            out.push(viol(
                "R5",
                format!(
                    "`protocol` directly depends on `{d}`: protocol → {d}; \
                     allowed: {}",
                    cfg.r5.protocol_allowed.join(", ")
                ),
            ));
        }
    }
    out
}

/// R6: client-layer boundaries. client-app reaches platform-* only through app-*.
pub fn r6(g: &Graph) -> Vec<Violation> {
    let mut out = Vec::new();
    let mut names: BTreeSet<&str> = ["client-core", "ui", "render", "assets"]
        .into_iter()
        .filter(|n| g.layer(n).is_some())
        .collect();
    names.extend(
        g.workspace()
            .filter(|(_, l)| *l == Layer::Client)
            .map(|(n, _)| n.as_str()),
    );
    for &name in &names {
        let bad = |n: &str| matches!(g.layer(n), Some(Layer::PlatformWeb | Layer::PlatformNative));
        if let Some(path) = g.find_path(name, bad) {
            // Already reported at an intermediate crate that this rule checks itself.
            if names.contains(path[1].as_str()) {
                continue;
            }
            out.push(viol(
                "R6",
                format!(
                    "`{name}` depends on platform-web/platform-native, not on `platform` \
                     (the client layer reaches platform-* only through app-*): {}",
                    arrow(&path)
                ),
            ));
        }
    }
    for (name, layer) in g.workspace() {
        if name != "client-app" && g.direct(name, true).contains(&"winit") {
            out.push(viol(
                "R6",
                format!("crate `{name}` (layer {layer}) directly depends on `winit`: {name} → winit; allowed only in client-app"),
            ));
        }
    }
    for (a, b) in [("ui", "render"), ("render", "ui")] {
        if g.layer(a).is_some()
            && let Some(path) = g.find_path(a, |n| n == b)
        {
            out.push(viol(
                "R6",
                format!(
                    "`{a}` depends on `{b}`; the shared language is the draw-list: {}",
                    arrow(&path)
                ),
            ));
        }
    }
    if g.layer("ui").is_some()
        && let Some(path) = g.find_path("ui", |n| n == "wgpu")
    {
        out.push(viol(
            "R6",
            format!("`ui` depends on `wgpu`: {}", arrow(&path)),
        ));
    }
    out
}

/// R7: server runtime (tokio, axum, hyper) only in server/tool. Dev-deps don't count.
pub fn r7(g: &Graph) -> Vec<Violation> {
    let mut out = Vec::new();
    for (name, layer) in g.workspace() {
        if matches!(layer, Layer::Server | Layer::Tool) {
            continue;
        }
        if let Some(path) = g.find_path(name, |n| matches!(n, "tokio" | "axum" | "hyper")) {
            // Already reported at a workspace intermediate outside server/tool.
            if g.layer(&path[1])
                .is_some_and(|l| !matches!(l, Layer::Server | Layer::Tool))
            {
                continue;
            }
            out.push(viol(
                "R7",
                format!(
                    "crate `{name}` (layer {layer}) pulls the server runtime outside the server/tool layers: {}",
                    arrow(&path)
                ),
            ));
        }
    }
    out
}

/// The interim UI crate: the only one that may depend on egui (SIMP-001, play-ui §3.4).
pub const INTERIM_UI: &str = "interim-ui";

/// The egui family: `egui*` (egui, egui-wgpu, egui_extras, ...), `epaint`, `emath`, `ecolor`.
fn is_egui_family(n: &str) -> bool {
    n.starts_with("egui") || matches!(n, "epaint" | "emath" | "ecolor")
}

/// R9: egui isolation. Only `interim-ui` depends on the egui family; only `client-app` (and the
/// tool layer) depends on `interim-ui`. Everything else, `ui`, `render`, `assets`, `client-core`,
/// `platform`, `draw-list` and so on, reaches egui neither directly nor transitively. The
/// apps reach it through `client-app`, which is allowed, so paths through `interim-ui` are not
/// followed.
pub fn r9(g: &Graph) -> Vec<Violation> {
    let mut out = Vec::new();
    for (name, layer) in g.workspace() {
        if name == INTERIM_UI {
            continue;
        }
        if g.direct(name, true).contains(&INTERIM_UI)
            && name != "client-app"
            && layer != Layer::Tool
        {
            out.push(viol(
                "R9",
                format!(
                    "crate `{name}` (layer {layer}) depends on `{INTERIM_UI}`: {name} → {INTERIM_UI}; \
                     only client-app and the tool layer may"
                ),
            ));
        }
        let direct = g.direct(name, true).into_iter().find(|d| is_egui_family(d));
        if let Some(d) = direct {
            out.push(viol(
                "R9",
                format!(
                    "crate `{name}` (layer {layer}) depends directly on `{d}`: {name} → {d}; \
                     only `{INTERIM_UI}` may depend on egui"
                ),
            ));
            continue;
        }
        if let Some(path) = g.find_path_avoiding(name, is_egui_family, |n| n == INTERIM_UI) {
            // Already reported at a workspace intermediate crate.
            if g.layer(&path[1]).is_some() {
                continue;
            }
            out.push(viol(
                "R9",
                format!(
                    "crate `{name}` (layer {layer}) reaches egui outside `{INTERIM_UI}`: {}",
                    arrow(&path)
                ),
            ));
        }
    }
    out
}

/// R10: the client holds a replica of the game and never runs its rules. (1) None of `r10.forbidden` (`game-host`)
/// is reachable from the client builds (`app-web`, `app-desktop`) over `normal` edges. (2) A crate of the layers
/// `client`, `platform-*`, `app-*` depends directly (`normal` or `build`) on a crate of `r10.direct` only if it is
/// in that entry's `allowed` list (`gamecore`: `replica`; `gamecore-sys`: nobody).
/// In `warn` mode the violations are warnings, and `r10.exceptions` (known reaches) are silent;
/// in `deny` mode every violation fails and exceptions are ignored.
pub fn r10(g: &Graph, cfg: &DepsConfig) -> Vec<Violation> {
    let mut out = r10_reach(g, cfg);
    out.extend(r10_direct(g, cfg));
    out
}

/// R10, direct part: see [`r10`].
pub fn r10_direct(g: &Graph, cfg: &DepsConfig) -> Vec<Violation> {
    let mut out = Vec::new();
    let warn = cfg.r10.mode == R10Mode::Warn;
    for (name, layer) in g.workspace() {
        if !matches!(
            layer,
            Layer::Client
                | Layer::PlatformWeb
                | Layer::PlatformNative
                | Layer::AppWeb
                | Layer::AppDesktop
        ) {
            continue;
        }
        for rule in &cfg.r10.direct {
            if rule.allowed.iter().any(|a| a == name) {
                continue;
            }
            if g.direct(name, true).contains(&rule.target.as_str()) {
                let who = if rule.allowed.is_empty() {
                    "no client crate may".to_owned()
                } else {
                    format!("only {} may", rule.allowed.join(", "))
                };
                let mut v = viol(
                    "R10",
                    format!(
                        "crate `{name}` (layer {layer}) depends directly on `{}`; {who}",
                        rule.target
                    ),
                );
                v.warning = warn;
                out.push(v);
            }
        }
    }
    out
}

/// R10, reach part: see [`r10`].
pub fn r10_reach(g: &Graph, cfg: &DepsConfig) -> Vec<Violation> {
    let mut out = Vec::new();
    let warn = cfg.r10.mode == R10Mode::Warn;
    for (app, layer) in g.workspace() {
        if !matches!(layer, Layer::AppWeb | Layer::AppDesktop) {
            continue;
        }
        for target in &cfg.r10.forbidden {
            if g.layer(target).is_none() {
                continue;
            }
            if warn
                && cfg
                    .r10
                    .exceptions
                    .iter()
                    .any(|e| e.app == *app && e.forbidden == *target)
            {
                continue;
            }
            if let Some(path) = g.find_path(app, |n| n == target) {
                let mut v = viol(
                    "R10",
                    format!(
                        "client build `{app}` (layer {layer}) reaches `{target}`, which holds the game brain: {}",
                        arrow(&path)
                    ),
                );
                v.warning = warn;
                out.push(v);
            }
        }
    }
    out
}

/// Stores each identical violation once, with every graph label it was found in.
pub fn merge_violations(
    found: &mut Vec<(Violation, Vec<String>)>,
    label: &str,
    new: impl IntoIterator<Item = Violation>,
) {
    for v in new {
        match found.iter_mut().find(|(x, _)| *x == v) {
            Some((_, labels)) => {
                if !labels.iter().any(|l| l == label) {
                    labels.push(label.to_string());
                }
            }
            None => found.push((v, vec![label.to_string()])),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildPlan {
    pub wasm: Vec<String>,
    pub native: Vec<String>,
}

/// R8: which crates the build check covers per target. `build` is host-only.
pub fn r8_plan(g: &Graph) -> BuildPlan {
    let mut plan = BuildPlan {
        wasm: Vec::new(),
        native: Vec::new(),
    };
    for (name, layer) in g.workspace() {
        if matches!(
            layer,
            Layer::Shared | Layer::Client | Layer::PlatformWeb | Layer::AppWeb
        ) {
            plan.wasm.push(name.clone());
        }
        if matches!(
            layer,
            Layer::Shared
                | Layer::Client
                | Layer::PlatformNative
                | Layer::AppDesktop
                | Layer::Server
                | Layer::Tool
        ) {
            plan.native.push(name.clone());
        }
    }
    plan
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn cfg() -> DepsConfig {
        DepsConfig {
            r2: R2 {
                allowed_web_paths: vec![
                    "wgpu".into(),
                    "winit".into(),
                    "getrandom".into(),
                    "egui-wgpu".into(),
                ],
            },
            r5: R5 {
                protocol_allowed: vec!["serde".into(), "postcard".into(), "thiserror".into()],
            },
            r10: R10 {
                mode: R10Mode::Deny,
                forbidden: ["gamecore", "gamecore-sys", "lua-sys", "game-host", "ui"]
                    .map(String::from)
                    .to_vec(),
                exceptions: Vec::new(),
                direct: vec![
                    R10Direct {
                        target: "gamecore".into(),
                        allowed: vec!["replica".into()],
                    },
                    R10Direct {
                        target: "gamecore-sys".into(),
                        allowed: Vec::new(),
                    },
                ],
            },
        }
    }

    fn clean() -> Graph {
        use EdgeKind::Normal as N;
        let mut g = Graph::default();
        for (n, l) in [
            ("protocol", Layer::Shared),
            ("lua-sys", Layer::Shared),
            ("gamecore", Layer::Shared),
            ("gamecore-sys", Layer::Shared),
            ("game-host", Layer::Shared),
            ("build-support", Layer::Build),
            ("platform", Layer::Client),
            ("client-core", Layer::Client),
            ("replica", Layer::Client),
            ("assets", Layer::Client),
            ("draw-list", Layer::Client),
            ("ui", Layer::Client),
            ("render", Layer::Client),
            ("interim-ui", Layer::Client),
            ("client-app", Layer::Client),
            ("platform-web", Layer::PlatformWeb),
            ("platform-native", Layer::PlatformNative),
            ("app-web", Layer::AppWeb),
            ("app-desktop", Layer::AppDesktop),
            ("server", Layer::Server),
        ] {
            g.add(n, Some(l));
        }
        g.edge("protocol", "serde", N);
        g.edge("protocol", "postcard", N);
        g.edge("lua-sys", "build-support", EdgeKind::Build);
        g.edge("gamecore", "lua-sys", N);
        g.edge("gamecore", "gamecore-sys", N);
        g.edge("game-host", "gamecore", N);
        g.edge("client-core", "protocol", N);
        g.edge("assets", "platform", N);
        g.edge("ui", "lua-sys", N);
        g.edge("ui", "assets", N);
        g.edge("ui", "draw-list", N);
        g.edge("render", "draw-list", N);
        g.edge("render", "wgpu", N);
        g.edge("wgpu", "web-sys", N);
        g.edge("web-sys", "js-sys", N);
        g.edge("interim-ui", "render", N);
        g.edge("interim-ui", "platform", N);
        g.edge("interim-ui", "egui", N);
        g.edge("interim-ui", "egui-wgpu", N);
        g.edge("egui-wgpu", "egui", N);
        g.edge("egui-wgpu", "wgpu", N);
        g.edge("egui", "epaint", N);
        g.edge("epaint", "emath", N);
        g.edge("epaint", "ecolor", N);
        g.edge("client-app", "client-core", N);
        g.edge("client-app", "interim-ui", N);
        g.edge("client-app", "render", N);
        g.edge("client-app", "winit", N);
        g.edge("winit", "web-sys", N);
        g.edge("platform-web", "platform", N);
        g.edge("platform-web", "web-sys", N);
        g.edge("platform-native", "platform", N);
        g.edge("app-web", "client-app", N);
        g.edge("app-web", "platform-web", N);
        g.edge("app-web", "wasm-bindgen", N);
        g.edge("app-desktop", "client-app", N);
        g.edge("app-desktop", "platform-native", N);
        g.edge("server", "protocol", N);
        g.edge("server", "gamecore", N);
        g.edge("server", "game-host", N);
        g.edge("server", "tokio", N);
        g.edge("server", "axum", N);
        g
    }

    #[test]
    fn clean_graph_passes() {
        let v = check_all(&clean(), &cfg());
        assert!(v.is_empty(), "{v:?}");
    }

    fn rules(v: &[Violation]) -> Vec<&'static str> {
        v.iter().map(|x| x.rule).collect()
    }

    #[test]
    fn r1_direct_web_sys_in_render() {
        let mut g = clean();
        g.edge("render", "web-sys", EdgeKind::Normal);
        let v = r1(&g);
        assert_eq!(rules(&v), ["R1"]);
        assert!(v[0].message.contains("render → web-sys"));
    }

    #[test]
    fn r1_ignores_dev_dependencies() {
        let mut g = clean();
        g.edge("render", "web-sys", EdgeKind::Dev);
        assert!(r1(&g).is_empty());
    }

    #[test]
    fn r2_unlisted_path_to_web_sys() {
        let mut g = clean();
        g.edge("client-core", "fancy-lib", EdgeKind::Normal);
        g.edge("fancy-lib", "web-sys", EdgeKind::Normal);
        let v = r2(&g, &cfg());
        assert_eq!(rules(&v), ["R2"]);
        assert!(
            v[0].message.contains("client-core → fancy-lib → web-sys"),
            "{}",
            v[0]
        );
    }

    #[test]
    fn r2_listed_path_is_ok_and_dev_ignored() {
        let mut g = clean();
        g.edge("client-core", "fancy-lib", EdgeKind::Dev);
        g.edge("fancy-lib", "js-sys", EdgeKind::Normal);
        assert!(r2(&g, &cfg()).is_empty());
    }

    #[test]
    fn r3_server_depends_on_client() {
        let mut g = clean();
        g.edge("server", "platform", EdgeKind::Normal);
        let v = r3(&g);
        assert_eq!(rules(&v), ["R3"]);
        assert!(v[0].message.contains("server → platform"));
    }

    #[test]
    fn r3_client_depends_on_server() {
        let mut g = clean();
        g.edge("client-app", "server", EdgeKind::Normal);
        let v = r3(&g);
        assert!(
            v.iter().any(|x| x.message.contains("client-app → server")),
            "{v:?}"
        );
    }

    #[test]
    fn r4_shared_depends_on_client() {
        let mut g = clean();
        g.edge("protocol", "platform", EdgeKind::Normal);
        let v = r4(&g);
        assert_eq!(rules(&v), ["R4"]);
    }

    #[test]
    fn r4_protocol_to_platform_web() {
        let mut g = clean();
        g.edge("protocol", "platform-web", EdgeKind::Normal);
        let v = r4(&g);
        assert_eq!(rules(&v), ["R4"]);
        assert!(v[0].message.contains("protocol → platform-web"), "{}", v[0]);
    }

    #[test]
    fn r4_gamecore_to_app_desktop_and_native() {
        let mut g = clean();
        g.edge("gamecore", "app-desktop", EdgeKind::Normal);
        let v = r4(&g);
        assert!(
            v.iter()
                .any(|x| x.rule == "R4" && x.message.contains("gamecore → app-desktop")),
            "{v:?}"
        );
        let mut g = clean();
        g.edge("gamecore", "platform-native", EdgeKind::Normal);
        let v = r4(&g);
        // gamecore itself, and game-host, which depends on it.
        assert!(!v.is_empty() && v.iter().all(|x| x.rule == "R4"), "{v:?}");
    }

    #[test]
    fn r4_build_dependency_is_ok_but_normal_is_not() {
        assert!(r4(&clean()).is_empty()); // lua-sys → build-support (build)
        let mut g = clean();
        g.edge("protocol", "build-support", EdgeKind::Normal);
        assert_eq!(rules(&r4(&g)), ["R4"]);
    }

    #[test]
    fn r3_extra_layer_separation() {
        for (a, b) in [
            ("client-core", "app-web"),
            ("platform-web", "app-web"),
            ("app-desktop", "platform-web"),
            ("platform-native", "platform-web"),
            ("app-web", "app-desktop"),
        ] {
            let mut g = clean();
            g.edge(a, b, EdgeKind::Normal);
            let v = r3(&g);
            assert!(
                v.iter().any(|x| x.message.contains(&format!("{a} → {b}"))),
                "{a} → {b}: {v:?}"
            );
        }
    }

    #[test]
    fn r6_draw_list_and_client_app_on_platform_layers() {
        let mut g = clean();
        g.edge("draw-list", "platform-web", EdgeKind::Normal);
        let v = r6(&g);
        assert_eq!(rules(&v), ["R6"]);
        assert!(
            v[0].message.contains("draw-list → platform-web"),
            "{}",
            v[0]
        );
        let mut g = clean();
        g.edge("client-app", "platform-native", EdgeKind::Normal);
        let v = r6(&g);
        assert!(
            v.iter()
                .any(|x| x.message.contains("client-app → platform-native")),
            "{v:?}"
        );
    }

    #[test]
    fn draw_list_to_platform_web_hits_r2_and_r6() {
        let mut g = clean();
        g.edge("draw-list", "platform-web", EdgeKind::Normal);
        let v = check_all(&g, &cfg());
        let rs = rules(&v);
        assert!(rs.contains(&"R2") && rs.contains(&"R6"), "{v:?}");
    }

    #[test]
    fn r2_shared_via_platform_web_to_web_sys() {
        let mut g = clean();
        g.edge("protocol", "platform-web", EdgeKind::Normal);
        let v = r2(&g, &cfg());
        assert_eq!(rules(&v), ["R2"]);
        assert!(
            v[0].message.contains("protocol → platform-web → web-sys"),
            "{}",
            v[0]
        );
    }

    #[test]
    fn merge_dedupes_and_keeps_all_features_only_violations() {
        let mut base = clean();
        base.edge("render", "web-sys", EdgeKind::Normal);
        let mut all = base.clone();
        all.edge("protocol", "platform-web", EdgeKind::Normal);
        let mut found = Vec::new();
        merge_violations(&mut found, "host", check_all(&base, &cfg()));
        assert!(!found.iter().any(|(v, _)| v.rule == "R4"));
        merge_violations(&mut found, "host+all-features", check_all(&all, &cfg()));
        let r1s: Vec<_> = found.iter().filter(|(v, _)| v.rule == "R1").collect();
        assert_eq!(r1s.len(), 1);
        assert_eq!(r1s[0].1, ["host", "host+all-features"]);
        let r4s: Vec<_> = found.iter().filter(|(v, _)| v.rule == "R4").collect();
        assert_eq!(r4s.len(), 1);
        assert_eq!(r4s[0].1, ["host+all-features"]);
        merge_violations(&mut found, "host+all-features", check_all(&all, &cfg()));
        let r4s: Vec<_> = found.iter().filter(|(v, _)| v.rule == "R4").collect();
        assert_eq!(r4s[0].1.len(), 1);
    }

    #[test]
    fn r5_protocol_extra_dependency() {
        let mut g = clean();
        g.edge("protocol", "rand", EdgeKind::Normal);
        let v = r5(&g, &cfg());
        assert_eq!(rules(&v), ["R5"]);
        assert!(v[0].message.contains("rand"));
    }

    #[test]
    fn r6_client_core_on_platform_web() {
        let mut g = clean();
        g.edge("client-core", "platform-web", EdgeKind::Normal);
        assert_eq!(rules(&r6(&g)), ["R6"]);
    }

    #[test]
    fn r6_winit_outside_client_app() {
        let mut g = clean();
        g.edge("render", "winit", EdgeKind::Normal);
        let v = r6(&g);
        assert_eq!(rules(&v), ["R6"]);
        assert!(v[0].message.contains("render → winit"));
    }

    #[test]
    fn r6_ui_render_and_wgpu() {
        let mut g = clean();
        g.edge("ui", "render", EdgeKind::Normal);
        let v = r6(&g);
        assert_eq!(rules(&v), ["R6", "R6"]);
        let mut g2 = clean();
        g2.edge("render", "ui", EdgeKind::Normal);
        assert_eq!(rules(&r6(&g2)), ["R6"]);
    }

    #[test]
    fn r7_tokio_in_client_but_not_dev() {
        let mut g = clean();
        g.edge("assets", "tokio", EdgeKind::Dev);
        assert!(r7(&g).is_empty());
        g.edge("assets", "hyper", EdgeKind::Normal);
        let v = r7(&g);
        assert_eq!(rules(&v), ["R7"]);
        assert!(v[0].message.contains("assets → hyper"));
    }

    #[test]
    fn r9_clean_graph_reaches_egui_only_through_interim_ui() {
        let g = clean();
        assert!(r9(&g).is_empty());
        // The apps do reach egui, through client-app.
        assert!(g.find_path("app-desktop", is_egui_family).is_some());
    }

    #[test]
    fn r9_direct_egui_in_render_and_ui() {
        for who in ["render", "ui", "client-app", "platform"] {
            let mut g = clean();
            g.edge(who, "egui", EdgeKind::Normal);
            let v = r9(&g);
            assert_eq!(rules(&v), ["R9"], "{who}: {v:?}");
            assert!(v[0].message.contains(&format!("{who} → egui")), "{}", v[0]);
        }
        let mut g = clean();
        g.edge("render", "egui-wgpu", EdgeKind::Build);
        assert_eq!(rules(&r9(&g)), ["R9"], "build edges count");
    }

    #[test]
    fn r9_transitive_egui_through_a_third_party_crate() {
        let mut g = clean();
        g.edge("assets", "egui_extras", EdgeKind::Normal);
        g.edge("egui_extras", "epaint", EdgeKind::Normal);
        let v = r9(&g);
        assert_eq!(rules(&v), ["R9"], "{v:?}");
        assert!(v[0].message.contains("assets → egui_extras"), "{}", v[0]);
        let mut g = clean();
        g.edge("draw-list", "fancy-lib", EdgeKind::Normal);
        g.edge("fancy-lib", "ecolor", EdgeKind::Normal);
        let v = r9(&g);
        assert!(
            v.iter()
                .any(|x| x.message.contains("draw-list → fancy-lib → ecolor")),
            "{v:?}"
        );
    }

    #[test]
    fn r9_only_client_app_and_tools_depend_on_interim_ui() {
        for who in [
            "render",
            "ui",
            "client-core",
            "assets",
            "platform",
            "app-desktop",
            "server",
        ] {
            let mut g = clean();
            g.edge(who, "interim-ui", EdgeKind::Normal);
            let v = r9(&g);
            assert!(
                v.iter()
                    .any(|x| x.rule == "R9" && x.message.contains(&format!("{who} → interim-ui"))),
                "{who}: {v:?}"
            );
        }
        let mut g = clean();
        g.add("asset-pipeline", Some(Layer::Tool));
        g.edge("asset-pipeline", "interim-ui", EdgeKind::Normal);
        assert!(r9(&g).is_empty(), "tools may");
    }

    #[test]
    fn r9_dev_dependencies_do_not_count() {
        let mut g = clean();
        g.edge("render", "egui", EdgeKind::Dev);
        assert!(r9(&g).is_empty());
    }

    #[test]
    fn r10_clean_graph_has_no_brain_in_the_apps() {
        let g = clean();
        // The brain exists, on the server side only.
        assert!(g.find_path("server", |n| n == "gamecore").is_some());
        assert!(r10(&g, &cfg()).is_empty());
    }

    #[test]
    fn r10_every_forbidden_crate_is_caught_in_both_apps() {
        for target in cfg().r10.forbidden {
            for app in ["app-web", "app-desktop"] {
                // Directly from the app and through a third-party crate.
                for via in [None, Some("fancy-lib")] {
                    let mut g = clean();
                    match via {
                        None => g.edge(app, &target, EdgeKind::Normal),
                        Some(mid) => {
                            g.edge(app, mid, EdgeKind::Normal);
                            g.edge(mid, &target, EdgeKind::Normal);
                        }
                    }
                    let v = r10(&g, &cfg());
                    // The target itself is always reported (crates behind it may be too).
                    let hit = v
                        .iter()
                        .find(|x| x.message.contains(&format!("`{target}`")));
                    let hit = hit.unwrap_or_else(|| panic!("{app} -> {via:?} -> {target}: {v:?}"));
                    assert!(hit.rule == "R10" && !hit.warning);
                    assert!(hit.message.contains(&format!("{app} → ")), "{hit}");
                }
            }
        }
    }

    #[test]
    fn r10_client_app_to_ui_and_game_host_is_the_b1_shape() {
        let mut g = clean();
        g.edge("client-app", "ui", EdgeKind::Normal);
        g.edge("client-app", "game-host", EdgeKind::Normal);
        let v = r10(&g, &cfg());
        // ui, lua-sys (through ui), game-host, gamecore, gamecore-sys: five per app.
        assert_eq!(v.len(), 10, "{v:?}");
        assert!(
            v.iter()
                .any(|x| x.message.contains("app-web → client-app → ui")),
            "{v:?}"
        );
        assert!(
            v.iter().any(|x| x
                .message
                .contains("app-desktop → client-app → game-host → gamecore")),
            "{v:?}"
        );
    }

    #[test]
    fn r10_ignores_dev_edges_and_crates_outside_the_closure() {
        let mut g = clean();
        g.edge("client-app", "game-host", EdgeKind::Dev);
        g.edge("app-web", "gamecore", EdgeKind::Dev);
        g.edge("client-core", "game-host", EdgeKind::Dev);
        assert!(r10(&g, &cfg()).is_empty());
        // `server` is not a client build, `client-core` is only checked through the apps.
        g.edge("server", "ui", EdgeKind::Normal);
        assert!(r10(&g, &cfg()).is_empty());
        // A client crate that no app reaches: a brain behind it is not the reach rule's concern (the direct
        // rule still names it).
        g.add("orphan", Some(Layer::Client));
        g.edge("orphan", "gamecore", EdgeKind::Normal);
        assert!(r10_reach(&g, &cfg()).is_empty());
        assert_eq!(r10_direct(&g, &cfg()).len(), 1);
    }

    #[test]
    fn r10_warn_mode_warns_and_exceptions_are_silent() {
        let mut g = clean();
        g.edge("app-web", "game-host", EdgeKind::Normal);
        g.edge("app-web", "ui", EdgeKind::Normal);
        let mut c = cfg();
        c.r10.mode = R10Mode::Warn;
        let v = r10(&g, &c);
        assert!(!v.is_empty() && v.iter().all(|x| x.rule == "R10" && x.warning));
        assert!(v.iter().any(|x| x.message.contains("app-web → ui")));
        for forbidden in ["game-host", "gamecore", "gamecore-sys"] {
            c.r10.exceptions.push(R10Exception {
                app: "app-web".into(),
                forbidden: forbidden.into(),
            });
        }
        // `ui` and `lua-sys` stay reported, the three excepted crates are silent.
        let v = r10(&g, &c);
        assert_eq!(v.len(), 2, "{v:?}");
        assert!(v.iter().all(|x| x.warning));
        assert!(v.iter().any(|x| x.message.contains("`ui`")), "{v:?}");
        assert!(v.iter().any(|x| x.message.contains("`lua-sys`")), "{v:?}");
        // An exception for another app does not help.
        let mut c2 = cfg();
        c2.r10.mode = R10Mode::Warn;
        c2.r10.exceptions.push(R10Exception {
            app: "app-desktop".into(),
            forbidden: "ui".into(),
        });
        assert!(
            r10(&g, &c2)
                .iter()
                .any(|x| x.message.contains("app-web") && x.message.contains("`ui`"))
        );
    }

    #[test]
    fn r10_deny_mode_ignores_exceptions() {
        let mut g = clean();
        g.edge("app-desktop", "gamecore", EdgeKind::Normal);
        let mut c = cfg();
        c.r10.exceptions.push(R10Exception {
            app: "app-desktop".into(),
            forbidden: "gamecore".into(),
        });
        let v = r10(&g, &c);
        assert!(v.iter().any(|x| !x.warning && x.rule == "R10"), "{v:?}");
    }

    /// The B4 shape: the client reaches `gamecore` through `replica` only.
    fn with_replica() -> Graph {
        let mut g = clean();
        g.edge("replica", "gamecore", EdgeKind::Normal);
        g.edge("replica", "client-core", EdgeKind::Normal);
        g.edge("client-app", "replica", EdgeKind::Normal);
        g
    }

    /// The deps.toml form of R10 (B4): only `game-host` is unreachable from the apps.
    fn b4_cfg() -> DepsConfig {
        let mut c = cfg();
        c.r10.forbidden = vec!["game-host".into()];
        c
    }

    #[test]
    fn r10_replica_may_hold_gamecore_and_the_apps_reach_it_through_it() {
        let g = with_replica();
        assert!(g.find_path("app-web", |n| n == "gamecore").is_some());
        let v = r10(&g, &b4_cfg());
        assert!(v.is_empty(), "{v:?}");
        // The same graph is red in the old form, where `gamecore` was unreachable.
        assert!(!r10(&g, &cfg()).is_empty());
    }

    #[test]
    fn r10_client_app_depending_on_gamecore_directly_is_red() {
        let mut g = with_replica();
        g.edge("client-app", "gamecore", EdgeKind::Normal);
        let v = r10(&g, &b4_cfg());
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].rule, "R10");
        assert!(!v[0].warning);
        assert!(
            v[0].message.contains(
                "crate `client-app` (layer client) depends directly on `gamecore`; only replica may"
            ),
            "{}",
            v[0]
        );
        // The check as a whole fails with it.
        assert!(check_all(&g, &b4_cfg()).iter().any(|x| x.rule == "R10"));
    }

    #[test]
    fn r10_every_client_layer_is_covered_and_build_edges_count() {
        for who in [
            "client-core",
            "ui",
            "platform-web",
            "platform-native",
            "app-web",
            "app-desktop",
        ] {
            for target in ["gamecore", "gamecore-sys"] {
                for kind in [EdgeKind::Normal, EdgeKind::Build] {
                    let mut g = with_replica();
                    g.edge(who, target, kind);
                    let v = r10_direct(&g, &b4_cfg());
                    assert_eq!(v.len(), 1, "{who} -> {target} {kind:?}: {v:?}");
                    assert!(v[0].message.contains(&format!("`{who}`")), "{}", v[0]);
                }
            }
        }
    }

    #[test]
    fn r10_gamecore_sys_is_closed_even_to_replica() {
        let mut g = with_replica();
        g.edge("replica", "gamecore-sys", EdgeKind::Normal);
        let v = r10_direct(&g, &b4_cfg());
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].message.contains("no client crate may"), "{}", v[0]);
    }

    #[test]
    fn r10_dev_edges_and_the_shared_and_server_layers_are_not_its_concern() {
        let mut g = with_replica();
        g.edge("client-app", "gamecore", EdgeKind::Dev);
        g.edge("client-core", "game-host", EdgeKind::Dev);
        g.edge("server", "gamecore", EdgeKind::Normal);
        g.edge("game-host", "gamecore-sys", EdgeKind::Normal);
        let v = r10(&g, &b4_cfg());
        assert!(v.is_empty(), "{v:?}");
    }

    #[test]
    fn r10_game_host_stays_unreachable_through_replica_too() {
        let mut g = with_replica();
        g.edge("replica", "game-host", EdgeKind::Normal);
        let v = r10(&g, &b4_cfg());
        assert!(
            v.iter().any(|x| x
                .message
                .contains("app-web → client-app → replica → game-host")),
            "{v:?}"
        );
    }

    #[test]
    fn check_all_includes_r10() {
        let mut g = clean();
        g.edge("app-web", "game-host", EdgeKind::Normal);
        let v = check_all(&g, &cfg());
        assert!(v.iter().any(|x| x.rule == "R10"), "{v:?}");
    }

    /// The real workspace on both targets: `deps.toml` must list exactly the reaches that exist.
    /// With exceptions ignored, the violations are precisely the excepted pairs; with them
    /// applied, nothing is left. After N4.5 (mode `deny`, no exceptions) both sides are empty.
    #[test]
    fn r10_real_workspace_matches_deps_toml() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("workspace root")
            .to_path_buf();
        let text = std::fs::read_to_string(root.join("tools/xtask/deps.toml")).expect("deps.toml");
        let real: DepsConfig = toml::from_str(&text).expect("deps.toml parses");
        let host = crate::meta::host_triple().expect("host triple");
        for triple in [host.as_str(), "wasm32-unknown-unknown"] {
            let md = crate::meta::load(&root, triple, false).expect("cargo metadata");
            let (g, layer_errors) = crate::meta::build_graph(&md).expect("graph");
            assert!(layer_errors.is_empty(), "{layer_errors:?}");
            assert!(r10(&g, &real).is_empty(), "{triple}: unlisted reaches");
            let mut strict = real.clone();
            strict.r10.mode = R10Mode::Deny;
            strict.r10.exceptions.clear();
            let reached: BTreeSet<(String, String)> = r10_reach(&g, &strict)
                .into_iter()
                .map(|v| {
                    // Message form: client build `app` (layer ...) reaches `target`, ...
                    let tick: Vec<&str> = v.message.split('`').collect();
                    (tick[1].to_string(), tick[3].to_string())
                })
                .collect();
            let listed: BTreeSet<(String, String)> = real
                .r10
                .exceptions
                .iter()
                .filter(|e| g.layer(&e.app).is_some() && g.layer(&e.forbidden).is_some())
                .map(|e| (e.app.clone(), e.forbidden.clone()))
                .collect();
            if real.r10.mode == R10Mode::Warn {
                assert!(
                    reached.is_subset(&listed),
                    "{triple}: reached but not listed: {:?}",
                    reached.difference(&listed).collect::<Vec<_>>()
                );
            } else {
                assert!(reached.is_empty(), "{triple}: {reached:?}");
            }
        }
    }

    #[test]
    fn r8_selects_crates_per_target() {
        let plan = r8_plan(&clean());
        let has = |l: &[String], n: &str| l.iter().any(|x| x == n);
        assert!(
            has(&plan.wasm, "protocol")
                && has(&plan.wasm, "platform-web")
                && has(&plan.wasm, "app-web")
        );
        assert!(
            !has(&plan.wasm, "server")
                && !has(&plan.wasm, "platform-native")
                && !has(&plan.wasm, "app-desktop")
        );
        assert!(!has(&plan.wasm, "build-support"));
        assert!(
            has(&plan.native, "server")
                && has(&plan.native, "platform-native")
                && has(&plan.native, "app-desktop")
        );
        assert!(!has(&plan.native, "platform-web") && !has(&plan.native, "app-web"));
        assert!(has(&plan.native, "client-core") && has(&plan.native, "lua-sys"));
    }
}
