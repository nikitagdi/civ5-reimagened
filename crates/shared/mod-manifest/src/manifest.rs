//! `mod.toml`: the typed manifest (D3) with strict parsing and semantic validation.
//!
//! No I/O: the input is the text of the file. Unknown keys and invalid values are errors that carry the key path.
//! The part x sync-class matrix and the channel / capability consistency rules live here; the list of known
//! capabilities does not (it belongs to `mod-api`), only their shape is checked.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;

use toml::Value;

use crate::error::ManifestError;
use crate::reader::{Res, Tbl, wrong_type};
use crate::semver::{Version, VersionReq};

/// Hard ceiling of a channel payload (below the 64 KiB gateway frame, D7).
pub const CHANNEL_PAYLOAD_CEILING: u32 = 56 * 1024;
/// Hard ceiling of `state_max`.
pub const STATE_MAX_CEILING: u64 = 64 * 1024 * 1024;
pub const DEFAULT_STATE_MAX: u64 = 1024 * 1024;
pub const DEFAULT_C2S_MAX: u32 = 4096;
pub const DEFAULT_S2C_MAX: u32 = 16 * 1024;
pub const DEFAULT_RATE_PER_S: u32 = 30;
/// Highest `[rust] tick_hz`: the game actor calls a server half's `on_tick` at most this often.
pub const TICK_HZ_MAX: u32 = 30;
/// The capability that gives a mod its network channels.
pub const CAP_NET_CHANNEL: &str = "net.channel";

/// A mod id: `^[a-z][a-z0-9-]{1,31}$`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModId(String);

impl ModId {
    pub fn new(s: &str) -> Result<Self, &'static str> {
        let b = s.as_bytes();
        if b.len() < 2 || b.len() > 32 {
            return Err("a mod id has 2 to 32 characters");
        }
        if !b[0].is_ascii_lowercase() {
            return Err("a mod id starts with a lowercase letter a-z");
        }
        if !b
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
        {
            return Err("a mod id contains only a-z, 0-9 and '-'");
        }
        Ok(Self(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for ModId {
    type Err = &'static str;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

/// Sync class (SPEC "Классы синхронизации"): G, S, C.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SyncClass {
    /// G: changes core state, part of the digest; version and hash are pinned.
    Gameplay,
    /// S: state lives outside the core, relayed on the mod's channels; may be optional for clients.
    Cosmetic,
    /// C: client-only, a per-player preference.
    Client,
}

impl SyncClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gameplay => "gameplay",
            Self::Cosmetic => "cosmetic",
            Self::Client => "client",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "gameplay" => Some(Self::Gameplay),
            "cosmetic" => Some(Self::Cosmetic),
            "client" => Some(Self::Client),
            _ => None,
        }
    }
}

/// Whether clients must have the mod.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ClientPolicy {
    Optional,
    Required,
}

impl ClientPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Optional => "optional",
            Self::Required => "required",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "optional" => Some(Self::Optional),
            "required" => Some(Self::Required),
            _ => None,
        }
    }
}

/// A part of a mod. The discriminant is the bit in the stable `parts` mask of the mod set hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum Part {
    Data = 1,
    LuaGameplay = 2,
    LuaUi = 4,
    Assets = 8,
    CorePatch = 16,
    ServerRs = 32,
    ClientRs = 64,
}

impl Part {
    pub const ALL: [Part; 7] = [
        Part::Data,
        Part::LuaGameplay,
        Part::LuaUi,
        Part::Assets,
        Part::CorePatch,
        Part::ServerRs,
        Part::ClientRs,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Data => "data",
            Self::LuaGameplay => "lua-gameplay",
            Self::LuaUi => "lua-ui",
            Self::Assets => "assets",
            Self::CorePatch => "core-patch",
            Self::ServerRs => "server-rs",
            Self::ClientRs => "client-rs",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.as_str() == s)
    }

    pub fn bit(self) -> u16 {
        self as u16
    }

    /// The part x class matrix (design-mod-system.md section 1). `server-rs` in a G mod may act on the game only
    /// through the closed command list; that is enforced by the host, not here.
    pub fn allowed_in(self, class: SyncClass) -> bool {
        match self {
            Part::Data | Part::LuaGameplay | Part::CorePatch => class == SyncClass::Gameplay,
            Part::ServerRs => class != SyncClass::Client,
            Part::LuaUi | Part::Assets | Part::ClientRs => true,
        }
    }
}

/// One network channel of a mod. Limits are what the host enforces (D7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    pub id: u8,
    pub name: String,
    pub c2s_max: u32,
    pub s2c_max: u32,
    /// Token bucket refill, messages per second.
    pub rate_per_s: u32,
    pub burst: u32,
}

/// `{ crate = "...", entry = "path::to::register" }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustEntry {
    pub crate_name: String,
    pub entry: String,
}

/// `[rust]`. `state_max` caps the server half's `persist()` and the Fresh replay. `tick_hz` is how often the game
/// actor calls the server half's `on_tick`: `1..=TICK_HZ_MAX` times a second, `0` (the default) never.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustSection {
    pub server: Option<RustEntry>,
    pub client: Option<RustEntry>,
    pub caps: BTreeSet<String>,
    pub state_max: u64,
    pub tick_hz: u32,
}

impl Default for RustSection {
    fn default() -> Self {
        Self {
            server: None,
            client: None,
            caps: BTreeSet::new(),
            state_max: DEFAULT_STATE_MAX,
            tick_hz: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaUiEntry {
    pub entry: String,
    pub file: String,
}

/// `[content]`: runtime-delivered bundle, paths relative to the mod root (globs allowed).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Content {
    pub data: Vec<String>,
    pub loc: Vec<String>,
    pub lua_gameplay: Vec<String>,
    pub lua_ui: Vec<LuaUiEntry>,
    pub assets: Option<String>,
}

/// `[core]`: C++ core patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreSection {
    pub base: String,
    pub patches: Vec<String>,
    pub defines: Vec<String>,
}

/// `[order]`: ordering only, no dependency.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Order {
    pub after: BTreeSet<ModId>,
    pub before: BTreeSet<ModId>,
}

/// A parsed and validated `mod.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub id: ModId,
    pub name: String,
    pub version: Version,
    /// Requirement on the host's `MOD_API_VERSION`.
    pub api: VersionReq,
    /// SPDX expression as written (shape-checked only).
    pub license: String,
    pub authors: Vec<String>,
    pub sync: SyncClass,
    /// As written; `None` when omitted. See [`Manifest::client_policy`].
    pub client: Option<ClientPolicy>,
    pub parts: BTreeSet<Part>,
    pub deps: BTreeMap<ModId, VersionReq>,
    pub optional: BTreeMap<ModId, VersionReq>,
    pub conflicts: BTreeMap<ModId, VersionReq>,
    pub order: Order,
    pub rust: RustSection,
    pub channels: Vec<Channel>,
    pub content: Content,
    pub core: Option<CoreSection>,
}

impl Manifest {
    /// Parses and validates the text of a `mod.toml`. Returns the first problem found.
    pub fn parse(src: &str) -> Result<Self, ManifestError> {
        let table: toml::Table = src
            .parse()
            .map_err(|e: toml::de::Error| ManifestError::Syntax(e.to_string()))?;
        let mut root = Tbl::new(&table, "");
        let mut mod_t = Tbl::from_value(root.require("mod")?, "mod")?;

        let id =
            ModId::new(mod_t.str_req("id")?).map_err(|m| ManifestError::invalid("mod.id", m))?;
        let name = parse_name(mod_t.str_req("name")?)?;
        let version = mod_t
            .str_req("version")?
            .parse::<Version>()
            .map_err(|e| ManifestError::invalid("mod.version", e.to_string()))?;
        let api = mod_t
            .str_req("api")?
            .parse::<VersionReq>()
            .map_err(|e| ManifestError::invalid("mod.api", e.to_string()))?;
        let license = parse_license(mod_t.str_req("license")?)?;
        let authors = parse_authors(&mut mod_t)?;
        let sync_s = mod_t.str_req("sync")?;
        let sync = SyncClass::parse(sync_s).ok_or_else(|| {
            ManifestError::invalid(
                "mod.sync",
                format!("{sync_s:?} is not one of gameplay, cosmetic, client"),
            )
        })?;
        let client = match mod_t.str_opt("client")? {
            None => None,
            Some(s) => Some(ClientPolicy::parse(s).ok_or_else(|| {
                ManifestError::invalid(
                    "mod.client",
                    format!("{s:?} is not one of optional, required"),
                )
            })?),
        };
        let parts = parse_parts(&mut mod_t)?;
        mod_t.finish()?;

        let deps = parse_reqs(root.table_opt("deps")?)?;
        let optional = parse_reqs(root.table_opt("optional")?)?;
        let conflicts = parse_reqs(root.table_opt("conflicts")?)?;
        let order = parse_order(root.table_opt("order")?)?;
        let (rust, state_max_given) = parse_rust(root.table_opt("rust")?)?;
        let channels = parse_channels(root.get("channels"))?;
        let content = parse_content(root.table_opt("content")?)?;
        let core = parse_core(root.table_opt("core")?)?;
        root.finish()?;

        let m = Manifest {
            id,
            name,
            version,
            api,
            license,
            authors,
            sync,
            client,
            parts,
            deps,
            optional,
            conflicts,
            order,
            rust,
            channels,
            content,
            core,
        };
        m.validate(state_max_given)?;
        Ok(m)
    }

    pub fn has_part(&self, p: Part) -> bool {
        self.parts.contains(&p)
    }

    /// Effective client policy: G mods are always required, a cosmetic mod defaults to `required` (fail closed),
    /// a client-only mod is a per-player preference and never blocks anybody.
    pub fn client_policy(&self) -> ClientPolicy {
        match self.sync {
            SyncClass::Gameplay => ClientPolicy::Required,
            SyncClass::Cosmetic => self.client.unwrap_or(ClientPolicy::Required),
            SyncClass::Client => ClientPolicy::Optional,
        }
    }

    fn validate(&self, state_max_given: bool) -> Res<()> {
        let bad = |path: &str, msg: String| Err(ManifestError::invalid(path, msg));
        let class = self.sync.as_str();

        // Part x class matrix.
        for p in &self.parts {
            if !p.allowed_in(self.sync) {
                return bad(
                    "mod.parts",
                    format!("part `{}` is not allowed in a `{class}` mod", p.as_str()),
                );
            }
        }
        if self.client.is_some() && self.sync != SyncClass::Cosmetic {
            return bad(
                "mod.client",
                format!("`client` applies only to sync = \"cosmetic\" (this mod is `{class}`)"),
            );
        }

        // [rust] entries match parts.
        for (part, entry, key) in [
            (Part::ServerRs, &self.rust.server, "rust.server"),
            (Part::ClientRs, &self.rust.client, "rust.client"),
        ] {
            match (self.has_part(part), entry) {
                (true, None) => {
                    return Err(ManifestError::Missing {
                        path: key.to_string(),
                    });
                }
                (false, Some(_)) => {
                    return bad(
                        key,
                        format!("declared, but `parts` has no `{}`", part.as_str()),
                    );
                }
                _ => {}
            }
        }
        if !self.rust.caps.is_empty()
            && !self.has_part(Part::ServerRs)
            && !self.has_part(Part::ClientRs)
        {
            return bad(
                "rust.caps",
                "capabilities need a `server-rs` or `client-rs` part".to_string(),
            );
        }
        if self.rust.tick_hz > 0 && !self.has_part(Part::ServerRs) {
            return bad(
                "rust.tick_hz",
                "only a mod with a `server-rs` part has a server tick".to_string(),
            );
        }
        if state_max_given && !self.has_part(Part::ServerRs) {
            return bad(
                "rust.state_max",
                "only a mod with a `server-rs` part has server state".to_string(),
            );
        }

        // Channels and the net.channel capability.
        let has_cap = self.rust.caps.contains(CAP_NET_CHANNEL);
        if self.sync == SyncClass::Client {
            if has_cap {
                return bad(
                    "rust.caps",
                    format!("a client-only mod cannot declare `{CAP_NET_CHANNEL}`"),
                );
            }
            if !self.channels.is_empty() {
                return bad(
                    "channels",
                    "a client-only mod cannot declare channels".to_string(),
                );
            }
        }
        if has_cap && !self.has_part(Part::ServerRs) {
            return bad(
                "rust.caps",
                format!("`{CAP_NET_CHANNEL}` needs a `server-rs` part"),
            );
        }
        if !self.channels.is_empty() && !has_cap {
            return bad(
                "channels",
                format!("channels need the `{CAP_NET_CHANNEL}` capability in rust.caps"),
            );
        }
        if has_cap && self.channels.is_empty() {
            return bad(
                "rust.caps",
                format!("`{CAP_NET_CHANNEL}` is declared but there is no [[channels]]"),
            );
        }
        let mut ids = BTreeSet::new();
        let mut names = BTreeSet::new();
        for (i, c) in self.channels.iter().enumerate() {
            if !ids.insert(c.id) {
                return bad(
                    &format!("channels[{i}].id"),
                    format!("channel id {} is used twice", c.id),
                );
            }
            if !names.insert(c.name.as_str()) {
                return bad(
                    &format!("channels[{i}].name"),
                    format!("channel name {:?} is used twice", c.name),
                );
            }
        }

        // [content] needs the matching part.
        for (nonempty, part, key) in [
            (!self.content.data.is_empty(), Part::Data, "content.data"),
            (!self.content.loc.is_empty(), Part::Data, "content.loc"),
            (
                !self.content.lua_gameplay.is_empty(),
                Part::LuaGameplay,
                "content.lua_gameplay",
            ),
            (
                !self.content.lua_ui.is_empty(),
                Part::LuaUi,
                "content.lua_ui",
            ),
            (
                self.content.assets.is_some(),
                Part::Assets,
                "content.assets",
            ),
        ] {
            if nonempty && !self.has_part(part) {
                return bad(
                    key,
                    format!("declared, but `parts` has no `{}`", part.as_str()),
                );
            }
        }

        // [core] <-> core-patch.
        match (self.has_part(Part::CorePatch), &self.core) {
            (true, None) => {
                return Err(ManifestError::Missing {
                    path: "core".to_string(),
                });
            }
            (false, Some(_)) => {
                return bad(
                    "core",
                    "declared, but `parts` has no `core-patch`".to_string(),
                );
            }
            _ => {}
        }

        // Relations.
        for (table, key) in [
            (&self.deps, "deps"),
            (&self.optional, "optional"),
            (&self.conflicts, "conflicts"),
        ] {
            if table.contains_key(&self.id) {
                return bad(
                    &format!("{key}.{}", self.id),
                    "a mod cannot refer to itself".to_string(),
                );
            }
        }
        for (a, a_key, b, b_key) in [
            (&self.deps, "deps", &self.optional, "optional"),
            (&self.deps, "deps", &self.conflicts, "conflicts"),
            (&self.optional, "optional", &self.conflicts, "conflicts"),
        ] {
            if let Some(id) = a.keys().find(|k| b.contains_key(*k)) {
                return bad(
                    &format!("{b_key}.{id}"),
                    format!("`{id}` is already listed in [{a_key}]"),
                );
            }
        }
        for (set, key) in [
            (&self.order.after, "order.after"),
            (&self.order.before, "order.before"),
        ] {
            if set.contains(&self.id) {
                return bad(key, "a mod cannot be ordered against itself".to_string());
            }
        }
        if let Some(id) = self.order.after.intersection(&self.order.before).next() {
            return bad(
                "order.before",
                format!("`{id}` is in both order.after and order.before"),
            );
        }
        Ok(())
    }
}

fn parse_name(s: &str) -> Res<String> {
    if s.trim().is_empty() || s.chars().count() > 64 || s.chars().any(char::is_control) {
        return Err(ManifestError::invalid(
            "mod.name",
            "must be 1 to 64 characters without control characters",
        ));
    }
    Ok(s.to_string())
}

/// Shape check only: SPDX ids, `LicenseRef-...`, `AND`/`OR`/`WITH`, parentheses and `+`.
fn parse_license(s: &str) -> Res<String> {
    let ok = !s.is_empty()
        && s.len() <= 128
        && s == s.trim()
        && s.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+' | '(' | ')' | ' ' | ':')
        });
    if ok {
        Ok(s.to_string())
    } else {
        Err(ManifestError::invalid(
            "mod.license",
            "expected an SPDX expression such as \"MIT\" or \"LicenseRef-Proprietary\"",
        ))
    }
}

fn parse_authors(t: &mut Tbl<'_>) -> Res<Vec<String>> {
    let mut out = Vec::new();
    for (path, s) in t.strings_opt("authors")?.unwrap_or_default() {
        if s.trim().is_empty() || s.chars().count() > 128 || s.chars().any(char::is_control) {
            return Err(ManifestError::invalid(
                path,
                "must be 1 to 128 characters without control characters",
            ));
        }
        out.push(s.to_string());
    }
    Ok(out)
}

fn parse_parts(t: &mut Tbl<'_>) -> Res<BTreeSet<Part>> {
    let items = t
        .strings_opt("parts")?
        .ok_or_else(|| ManifestError::Missing {
            path: "mod.parts".to_string(),
        })?;
    if items.is_empty() {
        return Err(ManifestError::invalid(
            "mod.parts",
            "a mod has at least one part",
        ));
    }
    let mut parts = BTreeSet::new();
    for (path, s) in items {
        let p = Part::parse(s).ok_or_else(|| {
            ManifestError::invalid(
                path.clone(),
                format!(
                    "unknown part {s:?} (known: {})",
                    Part::ALL.map(Part::as_str).join(", ")
                ),
            )
        })?;
        if !parts.insert(p) {
            return Err(ManifestError::invalid(
                path,
                format!("part {s:?} is listed twice"),
            ));
        }
    }
    Ok(parts)
}

fn parse_reqs(t: Option<Tbl<'_>>) -> Res<BTreeMap<ModId, VersionReq>> {
    let mut out = BTreeMap::new();
    let Some(t) = t else { return Ok(out) };
    for (path, key, v) in t.entries() {
        let id = ModId::new(key).map_err(|m| ManifestError::invalid(path.clone(), m))?;
        let Value::String(s) = v else {
            return Err(wrong_type(&path, "string (version requirement)", v));
        };
        let req = s
            .parse::<VersionReq>()
            .map_err(|e| ManifestError::invalid(path, e.to_string()))?;
        out.insert(id, req);
    }
    Ok(out)
}

fn parse_id_list(t: &mut Tbl<'_>, key: &'static str) -> Res<BTreeSet<ModId>> {
    let mut out = BTreeSet::new();
    for (path, s) in t.strings_opt(key)?.unwrap_or_default() {
        let id = ModId::new(s).map_err(|m| ManifestError::invalid(path.clone(), m))?;
        if !out.insert(id) {
            return Err(ManifestError::invalid(
                path,
                format!("{s:?} is listed twice"),
            ));
        }
    }
    Ok(out)
}

fn parse_order(t: Option<Tbl<'_>>) -> Res<Order> {
    let Some(mut t) = t else {
        return Ok(Order::default());
    };
    let order = Order {
        after: parse_id_list(&mut t, "after")?,
        before: parse_id_list(&mut t, "before")?,
    };
    t.finish()?;
    Ok(order)
}

/// `"4096"`-style integers are bytes; strings take `B`, `KiB`, `MiB` suffixes.
fn parse_size(v: &Value, path: &str, max: u64) -> Res<u64> {
    let bytes = match v {
        Value::Integer(n) => u64::try_from(*n).ok(),
        Value::String(s) => {
            let (digits, mult) = if let Some(d) = s.strip_suffix("MiB") {
                (d, 1024 * 1024)
            } else if let Some(d) = s.strip_suffix("KiB") {
                (d, 1024)
            } else if let Some(d) = s.strip_suffix('B') {
                (d, 1)
            } else {
                (s.as_str(), 1)
            };
            let digits = digits.trim();
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return Err(ManifestError::invalid(
                    path,
                    format!("{s:?} is not a size (use a number of bytes, \"64KiB\" or \"4MiB\")"),
                ));
            }
            digits.parse::<u64>().ok().and_then(|n| n.checked_mul(mult))
        }
        other => return Err(wrong_type(path, "integer or string", other)),
    };
    match bytes {
        Some(n) if (1..=max).contains(&n) => Ok(n),
        _ => Err(ManifestError::invalid(
            path,
            format!("size must be between 1 and {max} bytes"),
        )),
    }
}

fn is_ident(s: &str) -> bool {
    let mut it = s.chars();
    it.next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && it.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn parse_rust_entry(t: Option<Tbl<'_>>) -> Res<Option<RustEntry>> {
    let Some(mut t) = t else { return Ok(None) };
    let crate_path = t.path_of("crate");
    let entry_path = t.path_of("entry");
    let crate_name = t.str_req("crate")?;
    let entry = t.str_req("entry")?;
    t.finish()?;
    let crate_ok = {
        let mut cs = crate_name.chars();
        cs.next().is_some_and(|c| c.is_ascii_lowercase())
            && cs.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    };
    if !crate_ok {
        return Err(ManifestError::invalid(
            crate_path,
            "expected a crate name such as \"my-mod-server\"",
        ));
    }
    let segs: Vec<&str> = entry.split("::").collect();
    if segs.len() < 2 || !segs.iter().all(|s| is_ident(s)) {
        return Err(ManifestError::invalid(
            entry_path,
            "expected a function path such as \"my_mod_server::register\"",
        ));
    }
    Ok(Some(RustEntry {
        crate_name: crate_name.to_string(),
        entry: entry.to_string(),
    }))
}

fn valid_cap(s: &str) -> bool {
    !s.is_empty()
        && s.split('.').all(|seg| {
            let mut c = seg.chars();
            c.next().is_some_and(|f| f.is_ascii_lowercase())
                && c.all(|x| x.is_ascii_lowercase() || x.is_ascii_digit() || x == '_')
        })
}

/// Returns the section and whether `state_max` was written.
fn parse_rust(t: Option<Tbl<'_>>) -> Res<(RustSection, bool)> {
    let Some(mut t) = t else {
        return Ok((RustSection::default(), false));
    };
    let server = parse_rust_entry(t.table_opt("server")?)?;
    let client = parse_rust_entry(t.table_opt("client")?)?;
    let mut caps = BTreeSet::new();
    for (path, s) in t.strings_opt("caps")?.unwrap_or_default() {
        if !valid_cap(s) {
            return Err(ManifestError::invalid(
                path,
                format!("{s:?} is not a capability name (like \"net.channel\")"),
            ));
        }
        if !caps.insert(s.to_string()) {
            return Err(ManifestError::invalid(
                path,
                format!("capability {s:?} is listed twice"),
            ));
        }
    }
    let state_max_path = t.path_of("state_max");
    let (state_max, given) = match t.get("state_max") {
        None => (DEFAULT_STATE_MAX, false),
        Some(v) => (parse_size(v, &state_max_path, STATE_MAX_CEILING)?, true),
    };
    let tick_hz = t
        .int_opt("tick_hz", 0, i64::from(TICK_HZ_MAX))?
        .map_or(0, |n| u32::try_from(n).unwrap_or(0));
    t.finish()?;
    Ok((
        RustSection {
            server,
            client,
            caps,
            state_max,
            tick_hz,
        },
        given,
    ))
}

fn parse_channels(v: Option<&Value>) -> Res<Vec<Channel>> {
    let Some(v) = v else { return Ok(Vec::new()) };
    let Value::Array(items) = v else {
        return Err(wrong_type("channels", "array of tables ([[channels]])", v));
    };
    let ceiling = u64::from(CHANNEL_PAYLOAD_CEILING);
    let mut out = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let path = format!("channels[{i}]");
        let mut t = Tbl::from_value(item, &path)?;
        if t.get("state_max").is_some() {
            return Err(ManifestError::invalid(
                format!("{path}.state_max"),
                "state_max is a key of [rust], not of a channel",
            ));
        }
        let id = t
            .int_opt("id", 0, 255)?
            .ok_or_else(|| ManifestError::Missing {
                path: format!("{path}.id"),
            })?;
        let name = t.str_req("name")?;
        let name_ok = {
            let mut cs = name.chars();
            name.len() <= 32
                && cs.next().is_some_and(|c| c.is_ascii_lowercase())
                && cs.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        };
        if !name_ok {
            return Err(ManifestError::invalid(
                format!("{path}.name"),
                "expected 1 to 32 characters: a-z first, then a-z, 0-9, '_' or '-'",
            ));
        }
        let size = |t: &mut Tbl<'_>, key: &'static str, default: u32| -> Res<u32> {
            let p = t.path_of(key);
            match t.get(key) {
                None => Ok(default),
                // parse_size bounds the value by the ceiling, so it fits u32.
                Some(v) => Ok(parse_size(v, &p, ceiling)? as u32),
            }
        };
        let c2s_max = size(&mut t, "c2s_max", DEFAULT_C2S_MAX)?;
        let s2c_max = size(&mut t, "s2c_max", DEFAULT_S2C_MAX)?;
        let rate_path = t.path_of("rate");
        let rate_per_s = match t.str_opt("rate")? {
            None => DEFAULT_RATE_PER_S,
            Some(s) => {
                let n = s.strip_suffix("/s").and_then(|d| {
                    (!d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
                        .then(|| d.parse::<u32>().ok())?
                });
                match n {
                    Some(n) if (1..=10_000).contains(&n) => n,
                    _ => {
                        return Err(ManifestError::invalid(
                            rate_path,
                            format!("{s:?} is not a rate: use \"N/s\" with N from 1 to 10000"),
                        ));
                    }
                }
            }
        };
        let burst = match t.int_opt("burst", 1, 100_000)? {
            // int_opt bounds the value, so it fits u32.
            Some(b) => b as u32,
            None => rate_per_s.saturating_mul(2),
        };
        t.finish()?;
        out.push(Channel {
            id: id as u8,
            name: name.to_string(),
            c2s_max,
            s2c_max,
            rate_per_s,
            burst,
        });
    }
    Ok(out)
}

/// A path relative to the mod root; glob characters are allowed, escapes are not.
fn rel_path(path: String, s: &str) -> Res<String> {
    // One trailing '/' marks a directory ("assets/").
    let body = s.strip_suffix('/').unwrap_or(s);
    let bad = body.is_empty()
        || s.starts_with('/')
        || s.contains('\\')
        || s.contains(':')
        || body.split('/').any(|seg| seg == ".." || seg.is_empty());
    if bad {
        return Err(ManifestError::invalid(
            path,
            "expected a relative path with '/' separators, without '..' or empty segments",
        ));
    }
    Ok(s.to_string())
}

fn rel_paths(t: &mut Tbl<'_>, key: &'static str) -> Res<Vec<String>> {
    t.strings_opt(key)?
        .unwrap_or_default()
        .into_iter()
        .map(|(p, s)| rel_path(p, s))
        .collect()
}

fn parse_content(t: Option<Tbl<'_>>) -> Res<Content> {
    let Some(mut t) = t else {
        return Ok(Content::default());
    };
    let data = rel_paths(&mut t, "data")?;
    let loc = rel_paths(&mut t, "loc")?;
    let lua_gameplay = rel_paths(&mut t, "lua_gameplay")?;
    let mut lua_ui = Vec::new();
    let ui_path = t.path_of("lua_ui");
    if let Some(v) = t.get("lua_ui") {
        let Value::Array(items) = v else {
            return Err(wrong_type(&ui_path, "array of tables", v));
        };
        for (i, item) in items.iter().enumerate() {
            let p = format!("{ui_path}[{i}]");
            let mut e = Tbl::from_value(item, &p)?;
            let entry = e.str_req("entry")?;
            let file_path = e.path_of("file");
            let file = rel_path(file_path, e.str_req("file")?)?;
            e.finish()?;
            if !entry.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || entry.is_empty() {
                return Err(ManifestError::invalid(
                    format!("{p}.entry"),
                    "expected an identifier such as \"InGameUIAddin\"",
                ));
            }
            lua_ui.push(LuaUiEntry {
                entry: entry.to_string(),
                file,
            });
        }
    }
    let assets_path = t.path_of("assets");
    let assets = t
        .str_opt("assets")?
        .map(|s| rel_path(assets_path, s))
        .transpose()?;
    t.finish()?;
    Ok(Content {
        data,
        loc,
        lua_gameplay,
        lua_ui,
        assets,
    })
}

fn parse_core(t: Option<Tbl<'_>>) -> Res<Option<CoreSection>> {
    let Some(mut t) = t else { return Ok(None) };
    let base_path = t.path_of("base");
    let base = t.str_req("base")?;
    if base.trim().is_empty() {
        return Err(ManifestError::invalid(base_path, "must not be empty"));
    }
    let patches = rel_paths(&mut t, "patches")?;
    let mut defines = Vec::new();
    for (path, s) in t.strings_opt("defines")?.unwrap_or_default() {
        let ok = s.chars().next().is_some_and(|c| c.is_ascii_uppercase())
            && s.chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        if !ok {
            return Err(ManifestError::invalid(
                path,
                "expected an upper-case macro name such as \"LEKMOD_MODCORE_FOO\"",
            ));
        }
        defines.push(s.to_string());
    }
    t.finish()?;
    Ok(Some(CoreSection {
        base: base.to_string(),
        patches,
        defines,
    }))
}
