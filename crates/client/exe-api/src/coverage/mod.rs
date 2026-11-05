//! The AC-A1 coverage report: which exe-level names the Lua of the contexts of a
//! list call, and what `exe-api` has for each: a real implementation, a SIMP stub, or nothing.
//!
//! `lua-api.json` of the reverse is empty, so the report is built from the other side. [`Sources`] holds
//! the Lua files of the `ui` set after the file system rules (a later layer hides an earlier file of the
//! same bare name). For every context of the list the scripts it `include`s are followed, the tokens are
//! read ([`lex`]) and the uses of exe-level globals are collected. Then a fresh Lua state gets
//! `ExeApi::base_setup`, and each name is looked up there (`Global.Member ~= nil`), so what the report
//! says is what the scripts would find. Names the game replica or `ui` provide are counted, not checked.

pub mod lex;
#[cfg(not(target_arch = "wasm32"))]
pub mod set;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use lua_sys::Lua;
use platform::MemoryStore;
use serde_json::{Value, json};

use crate::registry::Status;
use crate::simp::{SIMP_ROWS, SimpRow};
use crate::{ExeApi, ExeConfig, ExeHost, SeatInfo};
use lex::{Tok, Use};

/// Who provides a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Owner {
    /// This crate, WO-5: the globals of the client (`OptionsManager`, `Mouse`, ...).
    ExeApi,
    /// This crate, WO-4: `UI`, `Events`, `LuaEvents`, `GameEvents`, `Network`.
    Events,
    /// The `ui` crate (`Controls`, `ContextPtr`, `UIManager`).
    Ui,
    /// The game replica (`Players`, `Game`, `GameInfo`, the enums of the DLL).
    Replica,
}

impl Owner {
    pub fn label(self) -> &'static str {
        match self {
            Owner::ExeApi => "exe-api",
            Owner::Events => "exe-api/events",
            Owner::Ui => "ui",
            Owner::Replica => "replica",
        }
    }

    /// Is the name checked against the Lua state? (`ui` and the replica are not part of this crate.)
    pub fn checked(self) -> bool {
        matches!(self, Owner::ExeApi | Owner::Events)
    }
}

/// What the Lua state has for a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Real,
    Simp(String),
    Missing,
    /// Provided by another module; not looked up.
    NotChecked,
}

impl Verdict {
    pub fn label(&self) -> &'static str {
        match self {
            Verdict::Real => "real",
            Verdict::Simp(_) => "simp",
            Verdict::Missing => "missing",
            Verdict::NotChecked => "other-module",
        }
    }
}

/// Tables the scripts fill themselves (`MapModData.UI_bc1 = ...`): only the table has to exist.
const DATA_TABLES: &[&str] = &["MapModData"];
const WO5_TABLES: &[&str] = &[
    "OptionsManager",
    "Modding",
    "ContentManager",
    "ContentType",
    "Mouse",
    "Keys",
    "KeyEvents",
    "MouseEvents",
    "PopupPriority",
    "Locale",
    "Steam",
    "Matchmaking",
    "Path",
    "NetKicked",
    "MapModData",
];
const EVENT_TABLES: &[&str] = &["UI", "Events", "LuaEvents", "GameEvents", "Network"];
const UI_TABLES: &[&str] = &["Controls", "ContextPtr", "UIManager", "TTManager"];
const REPLICA_TABLES: &[&str] = &[
    "Players",
    "Teams",
    "Game",
    "Map",
    "GameInfo",
    "GameInfoTypes",
    "GameDefines",
    "DB",
    "PreGame",
    "InfoCornerID",
    "TradeableItems",
    "SlotStatus",
    "SlotClaim",
    "InterfaceDirective",
    "InfluenceLevelTrend",
    "ChatTargetTypes",
    "CoopWarStates",
    "SystemUpdateUIType",
    "InterfaceDirtyBits",
];
/// Bare names of the exe: functions the scripts call as globals. The first five are the plain ones of
/// WO-5; the rest are the strategic view, the turn timer and the debug functions.
const EXE_FUNCS: &[&str] = &[
    "InStrategicView",
    "ToHexFromGrid",
    "ToGridFromHex",
    "HexToWorld",
    "GridToWorld",
    "ToggleStrategicView",
    "UnitMoving",
    "SetGameViewRenderType",
    "GetGameViewRenderType",
    "MouseOverStrategicViewResource",
    "StrategicViewShowFeatures",
    "StrategicViewShowFogOfWar",
    "SetStrategicViewIconSetting",
    "SetStrategicViewOverlay",
    "GetStrategicViewIconSetting",
    "GetStrategicViewOverlay",
    "GetOverlayLegend",
    "ProcessStrategicViewMouseClick",
    "getEndTurnTimerLength",
    "IsGameCoreBusy",
    "TogglePauseGameplay",
];
const UI_FUNCS: &[&str] = &["LookUpControl"];
/// The members of `PreGame` the client provides; the other members are the replica's.
const PREGAME_CLIENT: &[&str] = &[
    "GetNickName",
    "IsMultiplayerGame",
    "GameStarted",
    "IsHotSeatGame",
    "IsInternetGame",
    "SetLeaderKey",
    "GetLeaderKey",
];
/// Names that are in the Lua standard library or provided by `lua-rt`.
const KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "if", "in", "local",
    "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];
const STD: &[&str] = &[
    "pairs",
    "ipairs",
    "type",
    "tostring",
    "tonumber",
    "unpack",
    "next",
    "error",
    "setmetatable",
    "getmetatable",
    "rawget",
    "rawset",
    "rawequal",
    "select",
    "pcall",
    "xpcall",
    "assert",
    "loadstring",
    "loadfile",
    "dofile",
    "collectgarbage",
    "getfenv",
    "setfenv",
    "require",
    "module",
    "newproxy",
    "string",
    "table",
    "math",
    "os",
    "debug",
    "io",
    "coroutine",
    "package",
    "bit",
    "print",
    "include",
];

fn is_enum_table(name: &str) -> bool {
    name.ends_with("Types") && name.chars().next().is_some_and(char::is_uppercase)
}

fn owner_of(name: &str) -> Option<Owner> {
    if WO5_TABLES.contains(&name) || EXE_FUNCS.contains(&name) {
        Some(Owner::ExeApi)
    } else if EVENT_TABLES.contains(&name) {
        Some(Owner::Events)
    } else if UI_TABLES.contains(&name) || UI_FUNCS.contains(&name) {
        Some(Owner::Ui)
    } else if REPLICA_TABLES.contains(&name) || is_enum_table(name) {
        Some(Owner::Replica)
    } else {
        None
    }
}

fn is_func(name: &str) -> bool {
    EXE_FUNCS.contains(&name) || UI_FUNCS.contains(&name)
}

fn known(name: &str) -> bool {
    owner_of(name).is_some()
}

/// The Lua files by bare lower-case name (`toppanel.lua`).
#[derive(Default)]
pub struct Sources {
    files: HashMap<String, (String, Vec<u8>)>,
}

impl Sources {
    pub fn new() -> Sources {
        Sources::default()
    }

    /// Adds a file; a later `add` of the same bare name hides the earlier one (layers, lowest first).
    pub fn add(&mut self, path: &str, bytes: Vec<u8>) {
        let name = ui::bare_lower(path);
        self.files.insert(name, (path.to_owned(), bytes));
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    fn get(&self, bare: &str) -> Option<&(String, Vec<u8>)> {
        self.files.get(bare)
    }
}

/// The contexts by cluster (`K1`, `K2`, `K3`, `core`), from the list file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Contexts {
    pub clusters: Vec<(String, Vec<String>)>,
}

impl Contexts {
    /// `[K1]` starts a cluster; one context per line; `#` starts a comment.
    pub fn parse(text: &str) -> Result<Contexts, String> {
        let mut clusters: Vec<(String, Vec<String>)> = Vec::new();
        for (n, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                clusters.push((name.trim().to_owned(), Vec::new()));
            } else if let Some((_, list)) = clusters.last_mut() {
                list.push(line.to_owned());
            } else {
                return Err(format!(
                    "line {}: a context before the first [cluster]",
                    n + 1
                ));
            }
        }
        if clusters.is_empty() {
            return Err("no [cluster] in the list".to_owned());
        }
        Ok(Contexts { clusters })
    }

    pub fn names(&self, cluster: &str) -> &[String] {
        self.clusters
            .iter()
            .find(|(n, _)| n == cluster)
            .map(|(_, l)| l.as_slice())
            .unwrap_or(&[])
    }
}

/// One name the scripts use, with what the state has for it.
#[derive(Debug, Clone)]
pub struct Entry {
    /// `UI.LookAt`, `UIManager:QueuePopup`, `InStrategicView`.
    pub name: String,
    pub owner: Owner,
    pub verdict: Verdict,
    pub clusters: BTreeSet<String>,
    pub uses: u32,
    pub files: BTreeSet<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub contexts: Contexts,
    pub files_scanned: usize,
    /// Contexts without a Lua file (XML only, or not in the set).
    pub no_lua: Vec<String>,
    /// `include`d modules that no file of the set answers to.
    pub unknown_includes: BTreeSet<String>,
    pub entries: Vec<Entry>,
    /// Capitalised names the scripts use that no script defines and no table above owns.
    pub unresolved: BTreeMap<String, u32>,
}

/// A host for the state the report builds.
struct NullHost(MemoryStore);

impl ExeHost for NullHost {
    fn kv(&self) -> &dyn platform::KeyValueStore {
        &self.0
    }
    fn look_at(&self, _x: i32, _y: i32) {}
    fn seat_info(&self) -> SeatInfo {
        SeatInfo::default()
    }
}

fn lua_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !s.starts_with(|c: char| c.is_ascii_digit())
}

/// Does `Global.Member` (or the bare `Global`) answer non-nil in the state?
fn exists(lua: &Lua, global: &str, member: Option<&str>) -> bool {
    if !lua_ident(global) {
        return false;
    }
    let code = match member {
        Some(m) => format!(
            "local ok, r = pcall(function() return {global}[{m:?}] ~= nil end) return ok and r"
        ),
        None => format!("return {global} ~= nil"),
    };
    lua.load(&code)
        .set_name("=coverage")
        .eval::<bool>()
        .unwrap_or(false)
}

fn include_target(arg: &str) -> Option<String> {
    let bare = ui::bare_lower(arg);
    if bare.is_empty() || bare.contains('%') {
        return None;
    }
    Some(if bare.ends_with(".lua") {
        bare
    } else {
        format!("{bare}.lua")
    })
}

/// Builds the report for `contexts` over `sources`.
pub fn run(sources: &Sources, contexts: &Contexts) -> Report {
    let host: Rc<dyn ExeHost> = Rc::new(NullHost(MemoryStore::new()));
    let api = ExeApi::new(ExeConfig::default(), host);
    let lua = Lua::new().expect("a Lua state");
    {
        let g = lua.globals();
        if let Err(e) = api.base_setup()(&lua, &g) {
            tracing::warn!("coverage: base setup failed: {e}");
        }
    }
    let registry = api.registered();

    let mut report = Report {
        contexts: contexts.clone(),
        ..Report::default()
    };
    let mut tokens: HashMap<String, Vec<lex::Spanned>> = HashMap::new();
    let mut scanned: HashSet<String> = HashSet::new();
    // (name, kind-tag) -> entry
    let mut table: BTreeMap<String, Entry> = BTreeMap::new();
    let mut all_defined: HashSet<String> = HashSet::new();
    let mut raw_unresolved: Vec<(String, Vec<String>)> = Vec::new();

    for (cluster, names) in &contexts.clusters {
        let mut queue: Vec<String> = Vec::new();
        for n in names {
            let file = format!("{}.lua", ui::bare_lower(n));
            if sources.get(&file).is_some() {
                queue.push(file);
            } else if !report.no_lua.contains(n) {
                report.no_lua.push(n.clone());
            }
        }
        let mut seen: HashSet<String> = HashSet::new();
        while let Some(file) = queue.pop() {
            if !seen.insert(file.clone()) {
                continue;
            }
            let Some((path, bytes)) = sources.get(&file) else {
                continue;
            };
            let toks = tokens
                .entry(file.clone())
                .or_insert_with(|| lex::lex(bytes));
            if scanned.insert(file.clone()) {
                all_defined.extend(lex::defined(toks));
            }
            for arg in lex::includes(toks) {
                match include_target(&arg) {
                    Some(t) if sources.get(&t).is_some() => queue.push(t),
                    Some(_) => {
                        report.unknown_includes.insert(arg);
                    }
                    None => {}
                }
            }
            for (u, _line) in lex::uses(toks, known) {
                let (global, member, name) = match &u {
                    Use::Member(g, _) | Use::Method(g, _) if DATA_TABLES.contains(&g.as_str()) => {
                        (g.as_str(), None, g.clone())
                    }
                    Use::Member(g, m) => (g.as_str(), Some(m.as_str()), format!("{g}.{m}")),
                    Use::Method(g, m) => (g.as_str(), Some(m.as_str()), format!("{g}:{m}")),
                    // A table global named alone is passed around; only functions count.
                    Use::Bare(g) if is_func(g) || DATA_TABLES.contains(&g.as_str()) => {
                        (g.as_str(), None, g.clone())
                    }
                    Use::Bare(_) => continue,
                };
                let mut owner = owner_of(global).unwrap_or(Owner::Replica);
                if global == "PreGame" {
                    owner = if member.is_some_and(|m| PREGAME_CLIENT.contains(&m)) {
                        Owner::ExeApi
                    } else {
                        Owner::Replica
                    };
                }
                // `Global:Method` is a call on an object of another module.
                let colon = matches!(u, Use::Method(..));
                if colon && owner.checked() {
                    // `Events:...` or `UI:...` is not the exe's way; look up the member all the same.
                }
                let entry = table.entry(name.clone()).or_insert_with(|| Entry {
                    name,
                    owner,
                    verdict: Verdict::NotChecked,
                    clusters: BTreeSet::new(),
                    uses: 0,
                    files: BTreeSet::new(),
                });
                entry.clusters.insert(cluster.clone());
                entry.uses += 1;
                entry
                    .files
                    .insert(path.rsplit(['/', '\\']).next().unwrap_or(path).to_owned());
            }
            // Capitalised globals nobody owns: collect after all files are read (a name defined in
            // an included script is not unresolved).
            let mut names_here: Vec<String> = Vec::new();
            collect_unresolved(toks, &mut names_here);
            if !names_here.is_empty() {
                raw_unresolved.push((cluster.clone(), names_here));
            }
        }
    }

    for entry in table.values_mut() {
        if !entry.owner.checked() {
            continue;
        }
        let (global, member) = match entry.name.split_once(['.', ':']) {
            Some((g, m)) => (g.to_owned(), Some(m.to_owned())),
            None => (entry.name.clone(), None),
        };
        let found = exists(&lua, &global, member.as_deref());
        entry.verdict = if !found {
            Verdict::Missing
        } else {
            match registry.get(&entry.name) {
                Some(Status::Simp(id)) => Verdict::Simp(id.to_owned()),
                _ => Verdict::Real,
            }
        };
    }

    for names in raw_unresolved.iter().flat_map(|(_, n)| n) {
        let base = names.split('.').next().unwrap_or(names);
        if !all_defined.contains(base) {
            *report.unresolved.entry(names.clone()).or_default() += 1;
        }
    }
    report.files_scanned = scanned.len();
    report.entries = table.into_values().collect();
    report
}

/// `Name` or `Name.Member` for capitalised names that are neither owned nor a field of something.
fn collect_unresolved(toks: &[lex::Spanned], out: &mut Vec<String>) {
    for (i, (t, _)) in toks.iter().enumerate() {
        let Tok::Ident(name) = t else { continue };
        let upper = name.starts_with(|c: char| c.is_ascii_uppercase());
        if known(name) || STD.contains(&name.as_str()) || KEYWORDS.contains(&name.as_str()) {
            continue;
        }
        if i > 0 {
            match &toks[i - 1].0 {
                Tok::Punct('.') | Tok::Punct(':') => continue,
                Tok::Ident(k) if k == "local" || k == "function" => continue,
                _ => {}
            }
        }
        match (toks.get(i + 1).map(|t| &t.0), toks.get(i + 2).map(|t| &t.0)) {
            (Some(Tok::Punct('.')), Some(Tok::Ident(m))) if upper => {
                out.push(format!("{name}.{m}"))
            }
            (Some(Tok::Punct('(')), _) | (Some(Tok::Punct('{')), _) | (Some(Tok::Str(_)), _) => {
                out.push(name.clone());
            }
            _ => {}
        }
    }
}

impl Report {
    fn in_clusters(&self, clusters: &[&str]) -> Vec<&Entry> {
        self.entries
            .iter()
            .filter(|e| {
                clusters.is_empty() || e.clusters.iter().any(|c| clusters.contains(&c.as_str()))
            })
            .collect()
    }

    /// Counts of the names this crate must provide (WO-4 and WO-5 together) for the clusters
    /// (`[]`: all): `(real, simp, missing)`.
    pub fn counts(&self, clusters: &[&str], owners: &[Owner]) -> (usize, usize, usize) {
        let mut c = (0, 0, 0);
        for e in self
            .in_clusters(clusters)
            .into_iter()
            .filter(|e| owners.contains(&e.owner))
        {
            match e.verdict {
                Verdict::Real => c.0 += 1,
                Verdict::Simp(_) => c.1 += 1,
                Verdict::Missing => c.2 += 1,
                Verdict::NotChecked => {}
            }
        }
        c
    }

    /// The names of `owners` that are missing in the clusters, most used first.
    pub fn missing(&self, clusters: &[&str], owners: &[Owner]) -> Vec<&Entry> {
        let mut v: Vec<&Entry> = self
            .in_clusters(clusters)
            .into_iter()
            .filter(|e| owners.contains(&e.owner) && e.verdict == Verdict::Missing)
            .collect();
        v.sort_by(|a, b| b.uses.cmp(&a.uses).then(a.name.cmp(&b.name)));
        v
    }

    /// The SIMP rows the stubs in use name.
    pub fn simp_rows(&self) -> Vec<&'static SimpRow> {
        let ids: BTreeSet<&str> = self
            .entries
            .iter()
            .filter_map(|e| match &e.verdict {
                Verdict::Simp(id) => Some(id.as_str()),
                _ => None,
            })
            .collect();
        SIMP_ROWS.iter().filter(|r| ids.contains(r.id)).collect()
    }

    pub fn to_json(&self) -> Value {
        const OWN: &[Owner] = &[Owner::ExeApi, Owner::Events];
        let cluster_names: Vec<&str> = self
            .contexts
            .clusters
            .iter()
            .map(|(n, _)| n.as_str())
            .collect();
        let summary = |clusters: &[&str]| {
            let by = |owners: &[Owner]| {
                let (real, simp, missing) = self.counts(clusters, owners);
                json!({ "real": real, "simp": simp, "missing": missing })
            };
            let other = |o: Owner| {
                self.in_clusters(clusters)
                    .into_iter()
                    .filter(|e| e.owner == o)
                    .count()
            };
            json!({
                "exe_api": by(OWN),
                "wo5": by(&[Owner::ExeApi]),
                "wo4": by(&[Owner::Events]),
                "uses_of_ui_crate": other(Owner::Ui),
                "uses_of_replica": other(Owner::Replica),
            })
        };
        let mut per_cluster = serde_json::Map::new();
        for c in &cluster_names {
            per_cluster.insert((*c).to_owned(), summary(&[c]));
        }
        let k123: Vec<&str> = cluster_names
            .iter()
            .copied()
            .filter(|c| matches!(*c, "K1" | "K2" | "K3"))
            .collect();
        let entries: Vec<Value> = self
            .entries
            .iter()
            .map(|e| {
                json!({
                    "name": e.name,
                    "owner": e.owner.label(),
                    "status": e.verdict.label(),
                    "simp": match &e.verdict { Verdict::Simp(id) => Value::String(id.clone()), _ => Value::Null },
                    "clusters": e.clusters,
                    "uses": e.uses,
                    "files": e.files,
                })
            })
            .collect();
        json!({
            "version": 1,
            "contexts": self.contexts.clusters.iter().map(|(n, l)| (n.clone(), json!(l))).collect::<serde_json::Map<_, _>>(),
            "files_scanned": self.files_scanned,
            "contexts_without_lua": self.no_lua,
            "includes_not_in_set": self.unknown_includes,
            "summary": {
                "all": summary(&[]),
                "k1_k3": summary(&k123),
                "per_cluster": per_cluster,
            },
            "entries": entries,
            "simp": self.simp_rows().iter().map(|r| json!({"id": r.id, "what": r.what, "place": r.place})).collect::<Vec<_>>(),
            "unresolved_globals": self.unresolved,
        })
    }

    /// The table for the terminal.
    pub fn table(&self) -> String {
        use std::fmt::Write;
        const OWN: &[Owner] = &[Owner::ExeApi, Owner::Events];
        let mut s = String::new();
        let _ = writeln!(
            s,
            "exe-api coverage (AC-A1): {} Lua files scanned",
            self.files_scanned
        );
        let _ = writeln!(
            s,
            "{:<8} {:>6} {:>6} {:>8} | {:>8} {:>8} | {:>9} {:>8}",
            "cluster", "real", "simp", "missing", "wo5 miss", "wo4 miss", "ui uses", "replica"
        );
        let mut rows: Vec<(String, Vec<&str>)> = self
            .contexts
            .clusters
            .iter()
            .map(|(n, _)| (n.clone(), vec![n.as_str()]))
            .collect();
        let k123: Vec<&str> = self
            .contexts
            .clusters
            .iter()
            .map(|(n, _)| n.as_str())
            .filter(|c| matches!(*c, "K1" | "K2" | "K3"))
            .collect();
        rows.push(("K1-K3".to_owned(), k123));
        rows.push(("all".to_owned(), Vec::new()));
        for (label, cl) in &rows {
            let (r, p, m) = self.counts(cl, OWN);
            let (_, _, m5) = self.counts(cl, &[Owner::ExeApi]);
            let (_, _, m4) = self.counts(cl, &[Owner::Events]);
            let ui = self
                .in_clusters(cl)
                .into_iter()
                .filter(|e| e.owner == Owner::Ui)
                .count();
            let rep = self
                .in_clusters(cl)
                .into_iter()
                .filter(|e| e.owner == Owner::Replica)
                .count();
            let _ = writeln!(
                s,
                "{label:<8} {r:>6} {p:>6} {m:>8} | {m5:>8} {m4:>8} | {ui:>9} {rep:>8}"
            );
        }
        let k123: Vec<&str> = ["K1", "K2", "K3"].to_vec();
        let miss5 = self.missing(&k123, &[Owner::ExeApi]);
        let _ = writeln!(s, "\nmissing in K1-K3, WO-5 names ({}):", miss5.len());
        for e in &miss5 {
            let _ = writeln!(s, "  {:<44} x{}", e.name, e.uses);
        }
        let miss4 = self.missing(&k123, &[Owner::Events]);
        let _ = writeln!(
            s,
            "\nmissing in K1-K3, WO-4 names ({}); top by uses:",
            miss4.len()
        );
        for e in miss4.iter().take(25) {
            let _ = writeln!(s, "  {:<44} x{}", e.name, e.uses);
        }
        let rows = self.simp_rows();
        if !rows.is_empty() {
            let _ = writeln!(s, "\nSIMP stubs in use:");
            for r in rows {
                let _ = writeln!(s, "  {}  {}", r.id, r.what);
            }
        }
        if !self.unresolved.is_empty() {
            let _ = writeln!(
                s,
                "\nunresolved capitalised globals (not defined by any scanned script, not owned):"
            );
            let mut v: Vec<_> = self.unresolved.iter().collect();
            v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
            for (n, c) in v.iter().take(40) {
                let _ = writeln!(s, "  {n:<44} x{c}");
            }
        }
        if !self.no_lua.is_empty() {
            let _ = writeln!(
                s,
                "\ncontexts without a Lua file: {}",
                self.no_lua.join(", ")
            );
        }
        if !self.unknown_includes.is_empty() {
            let _ = writeln!(
                s,
                "includes not in the set: {}",
                self.unknown_includes
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_lists_parse_with_comments() {
        let c = Contexts::parse(
            "# k\n[K1]\nWorldView # the world\nUnitFlagManager\n\n[K2]\nTopPanel\n",
        )
        .unwrap();
        assert_eq!(c.names("K1"), ["WorldView", "UnitFlagManager"]);
        assert_eq!(c.names("K2"), ["TopPanel"]);
        assert!(Contexts::parse("WorldView").is_err());
        assert!(Contexts::parse("# nothing").is_err());
    }
}
