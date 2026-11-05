//! The Lua closure: which Lua files of the install the gamecore contexts and the map script need
//!. Static scan of `include("X")` / `include "X"` and of the
//! `ContextPtr:LoadNewContext("X")` lines of `InGame.lua`, resolved by the rules civhost uses
//! (`HstVfs::ResolveInclude`): case-insensitive, by file stem, first root that has the stem wins;
//! inside a root the files of a folder come before its subfolders, subfolders in name order.
//!
//! Only files under the game directory are read; nothing here writes.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::GameDataError;

/// Includes the original game itself fails to find (civhost reports the same). Lower case, with
/// `.lua`. Any other unresolved include is a build error.
pub const KNOWN_MISSING: &[&str] = &["plotareasweepiterator.lua"];

/// Map scripts of the pack: (name in `map_scripts`, path from the game root).
pub const MAP_SCRIPTS: &[(&str, &str)] = &[(
    "lekmap_pangaea",
    "Assets/Maps/Lekmap v5.2/LekmapPangaeaFractalv5.2.lua",
)];

/// `InGame.lua` is found by stem through the context roots.
pub const CONTEXT_ENTRY: &str = "InGame";

/// Lekmod DLC folder the contexts are taken from (or another spelling of v34.15 that
/// `game::source::is_lekmod_dir` accepts); any other `LEKMOD*` folder is used when there is none
/// (the last one in name order, like civhost).
pub const LEKMOD_DLC: &str = crate::game::source::LEKMOD_DIR;

/// Single normalisation of paths used in ids and in the index: `/` separators, lower case, no
/// leading `./`, no empty or trailing components.
pub fn normalize_path(p: &str) -> String {
    p.replace('\\', "/")
        .to_lowercase()
        .split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .collect::<Vec<_>>()
        .join("/")
}

/// A directory searched for `.lua` files by stem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootDef {
    /// Path from the game root (any case, `/` or `\`).
    pub dir: String,
    /// Subfolders are searched too (files of a folder first, then subfolders in name order).
    pub recursive: bool,
}

impl RootDef {
    pub fn recursive(dir: impl Into<String>) -> Self {
        Self {
            dir: dir.into(),
            recursive: true,
        }
    }
}

/// Everything the closure computation needs to know about the install layout.
#[derive(Debug, Clone)]
pub struct ClosureSpec {
    /// (name, path from the game root).
    pub map_scripts: Vec<(String, String)>,
    /// Roots for `include` inside map scripts, after the script's own folder (non-recursive).
    pub map_roots: Vec<RootDef>,
    /// Name (stem) of the file whose `LoadNewContext` lines name the gameplay contexts.
    pub context_entry: String,
    /// Roots for contexts and the includes inside them.
    pub context_roots: Vec<RootDef>,
    /// The order written to the index as `include_roots` (the first one, the map script's own
    /// folder, is added in front when the index is made). The closure checks that this order
    /// resolves every include to the same file the civhost roots do.
    pub index_roots: Vec<RootDef>,
    /// Lower case file names (with `.lua`) that may stay unresolved.
    pub known_missing: Vec<String>,
    /// Files the gamecore loads by bare name outside any script (`WorldBuilderRandomItems`, found
    /// by stem in `map_roots`); they and their includes join the closure like a map script's.
    pub extra_entries: Vec<String>,
    /// Name in `map_scripts` whose folder is the first include root of the index (`None`: the
    /// first name in key order).
    pub default_script: Option<String>,
}

impl ClosureSpec {
    /// The spec of the b1 pack for the install at `game`: Lekmap Pangaea and the Lekmod contexts.
    pub fn lekmod_b1(game: &Path) -> Result<Self, GameDataError> {
        let dlc = find_lekmod_dlc(game)?;
        let dlc = format!("Assets/DLC/{dlc}");
        let gameplay = [
            "Assets/DLC/Expansion2/Gameplay/Lua",
            "Assets/DLC/Expansion/Gameplay/Lua",
            "Assets/Gameplay/Lua",
        ];
        let ui = [
            "Assets/DLC/Expansion2/UI",
            "Assets/DLC/Expansion/UI",
            "Assets/UI",
        ];
        let roots = |dirs: &[&str]| {
            dirs.iter()
                .map(|d| RootDef::recursive(*d))
                .collect::<Vec<_>>()
        };
        let mut map_roots = roots(&gameplay);
        map_roots.push(RootDef::recursive("Assets/Maps"));
        // civhost: the Lekmod DLC folder, the UI folders, then the gameplay folders.
        let mut context_roots = vec![RootDef::recursive(dlc.clone())];
        context_roots.extend(roots(&ui));
        context_roots.extend(roots(&gameplay));
        // Gameplay folders, Maps, the Lekmod DLC, the UI folders.
        let mut index_roots = roots(&gameplay);
        index_roots.push(RootDef::recursive("Assets/Maps"));
        index_roots.push(RootDef::recursive(dlc));
        index_roots.extend(roots(&ui));
        Ok(Self {
            map_scripts: MAP_SCRIPTS
                .iter()
                .map(|(n, p)| ((*n).to_owned(), (*p).to_owned()))
                .collect(),
            map_roots,
            context_entry: CONTEXT_ENTRY.to_owned(),
            context_roots,
            index_roots,
            known_missing: KNOWN_MISSING.iter().map(|s| (*s).to_owned()).collect(),
            extra_entries: Vec::new(),
            default_script: Some(MAP_SCRIPTS[0].0.to_owned()),
        })
    }

    /// The spec of the server package: every map script of the game DB
    /// (`map_scripts`, from `map_scripts::read`) and the world-builder entry file.
    pub fn lekmod_b2(
        game: &Path,
        map_scripts: Vec<(String, String)>,
    ) -> Result<Self, GameDataError> {
        let mut spec = Self::lekmod_b1(game)?;
        spec.map_scripts = map_scripts;
        spec.extra_entries = vec![super::map_scripts::WORLD_BUILDER_ENTRY.to_owned()];
        Ok(spec)
    }
}

/// Name of the Lekmod DLC folder under `Assets/DLC` (as it is on disk).
pub fn find_lekmod_dlc(game: &Path) -> Result<String, GameDataError> {
    let dlc_dir = resolve_ci(game, "Assets/DLC")
        .ok_or_else(|| invalid(format!("no Assets/DLC folder under {}", game.display())))?;
    let mut names: Vec<String> = read_dir_sorted(&dlc_dir)?
        .into_iter()
        .filter(|(_, p)| p.is_dir())
        .map(|(n, _)| n)
        .filter(|n| n.to_ascii_lowercase().starts_with("lekmod"))
        .collect();
    // The same folder the asset index reads (`LEKMOD_v34.15` or another spelling of v34.15).
    if let Some(n) = crate::game::source::pick_lekmod_dir(names.iter().map(String::as_str)) {
        return Ok(n.to_owned());
    }
    names.pop().ok_or_else(|| {
        invalid(format!(
            "no LEKMOD* folder in {} (install Lekmod {LEKMOD_DLC})",
            dlc_dir.display()
        ))
    })
}

fn invalid(msg: String) -> GameDataError {
    GameDataError::Invalid(msg)
}

fn io_err(path: &Path, source: std::io::Error) -> GameDataError {
    GameDataError::Io {
        path: path.display().to_string(),
        source,
    }
}

/// Directory entries as (name, path), sorted by name (byte order of the name as on disk).
fn read_dir_sorted(dir: &Path) -> Result<Vec<(String, PathBuf)>, GameDataError> {
    let mut v = Vec::new();
    for e in fs::read_dir(dir).map_err(|e| io_err(dir, e))? {
        let e = e.map_err(|e| io_err(dir, e))?;
        v.push((e.file_name().to_string_lossy().into_owned(), e.path()));
    }
    v.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(v)
}

/// `rel` under `base`, each component matched case-insensitively (the install is read on
/// case-sensitive file systems too). `None` if a component is missing.
fn resolve_ci(base: &Path, rel: &str) -> Option<PathBuf> {
    let mut cur = base.to_path_buf();
    for comp in rel.replace('\\', "/").split('/').filter(|c| !c.is_empty()) {
        let direct = cur.join(comp);
        if direct.exists() {
            cur = direct;
            continue;
        }
        let hit = fs::read_dir(&cur)
            .ok()?
            .filter_map(Result::ok)
            .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(comp))?;
        cur = hit.path();
    }
    Some(cur)
}

// ---------------------------------------------------------------------------------------------
// Source scan
// ---------------------------------------------------------------------------------------------

/// What `scan` looks for.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Scan {
    /// Literal names of `include("X")` / `include "X"`, in file order.
    pub includes: Vec<String>,
    /// Literal names of `ContextPtr:LoadNewContext("X")`, in file order.
    pub contexts: Vec<String>,
}

/// Scans Lua source without running it. Comments and long strings are skipped, so commented-out
/// calls do not count. Calls with a computed name (`include("a" .. b)`, `LoadNewContext(name)`)
/// are not literal and are left out.
pub fn scan(src: &[u8]) -> Scan {
    let mut out = Scan::default();
    let n = src.len();
    let mut i = 0;
    // Last significant byte before the current token (to tell `x:include` and `x.include` apart).
    let mut prev = 0u8;
    while i < n {
        let c = src[i];
        match c {
            b'-' if src.get(i + 1) == Some(&b'-') => {
                i += 2;
                i = match long_bracket_level(src, i) {
                    Some(level) => skip_long(src, i, level),
                    None => src[i..]
                        .iter()
                        .position(|&b| b == b'\n')
                        .map_or(n, |p| i + p),
                };
            }
            b'[' if long_bracket_level(src, i).is_some() => {
                let level = long_bracket_level(src, i).unwrap_or(0);
                i = skip_long(src, i, level);
                prev = b']';
            }
            b'"' | b'\'' => {
                i = skip_quoted(src, i).0;
                prev = b'"';
            }
            _ if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i;
                while i < n && (src[i].is_ascii_alphanumeric() || src[i] == b'_') {
                    i += 1;
                }
                let ident = &src[start..i];
                if ident == b"include"
                    && prev != b'.'
                    && prev != b':'
                    && let Some((name, next)) = call_literal(src, i)
                {
                    out.includes.push(name);
                    i = next;
                } else if ident == b"LoadNewContext"
                    && prev == b':'
                    && let Some((name, next)) = call_literal(src, i)
                {
                    out.contexts.push(name);
                    i = next;
                }
                prev = b'a';
            }
            _ => {
                if !c.is_ascii_whitespace() {
                    prev = c;
                }
                i += 1;
            }
        }
    }
    out
}

/// `[` `=`* `[` at `i`: the number of `=`.
fn long_bracket_level(src: &[u8], i: usize) -> Option<usize> {
    if src.get(i) != Some(&b'[') {
        return None;
    }
    let mut j = i + 1;
    while src.get(j) == Some(&b'=') {
        j += 1;
    }
    (src.get(j) == Some(&b'[')).then_some(j - i - 1)
}

/// Index after the closing bracket of the long string/comment that opens at `i`.
fn skip_long(src: &[u8], i: usize, level: usize) -> usize {
    let mut j = i + level + 2;
    while j < src.len() {
        if src[j] == b']' {
            let mut k = j + 1;
            let mut eq = 0;
            while src.get(k) == Some(&b'=') {
                eq += 1;
                k += 1;
            }
            if eq == level && src.get(k) == Some(&b']') {
                return k + 1;
            }
        }
        j += 1;
    }
    src.len()
}

/// A quoted string starting at `i`: (index after it, content). An unterminated one ends at the
/// line break.
fn skip_quoted(src: &[u8], i: usize) -> (usize, Vec<u8>) {
    let q = src[i];
    let mut j = i + 1;
    let mut content = Vec::new();
    while j < src.len() {
        match src[j] {
            b'\\' => {
                if let Some(&b) = src.get(j + 1) {
                    content.push(b);
                }
                j += 2;
            }
            b if b == q => return (j + 1, content),
            b'\n' => return (j, content),
            b => {
                content.push(b);
                j += 1;
            }
        }
    }
    (src.len(), content)
}

/// After the callee name at `i`: `("X")`, `( "X" )` or `"X"`. Returns the name and the index after
/// the literal. The parenthesised form must close right after the literal (a concatenation or a
/// second argument makes the name non-literal).
fn call_literal(src: &[u8], mut i: usize) -> Option<(String, usize)> {
    let skip_ws = |mut j: usize| {
        while src.get(j).is_some_and(u8::is_ascii_whitespace) {
            j += 1;
        }
        j
    };
    i = skip_ws(i);
    let paren = src.get(i) == Some(&b'(');
    if paren {
        i = skip_ws(i + 1);
    }
    if !matches!(src.get(i), Some(b'"' | b'\'')) {
        return None;
    }
    let (after, content) = skip_quoted(src, i);
    if paren && src.get(skip_ws(after)) != Some(&b')') {
        return None;
    }
    Some((String::from_utf8_lossy(&content).into_owned(), after))
}

// ---------------------------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------------------------

/// Stem (lower case, no `.lua`) of an include name: civhost drops the folder part and `.lua`.
pub fn stem_of(name: &str) -> String {
    let base = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .to_lowercase();
    base.strip_suffix(".lua")
        .map_or(base.clone(), str::to_owned)
}

type StemMap = HashMap<String, PathBuf>;

/// Lazily indexed roots of one game directory.
struct Resolver<'a> {
    game: &'a Path,
    cache: RefCell<HashMap<(String, bool), Rc<StemMap>>>,
}

impl<'a> Resolver<'a> {
    fn new(game: &'a Path) -> Self {
        Self {
            game,
            cache: RefCell::new(HashMap::new()),
        }
    }

    fn stems(&self, root: &RootDef) -> Result<Rc<StemMap>, GameDataError> {
        let key = (normalize_path(&root.dir), root.recursive);
        if let Some(m) = self.cache.borrow().get(&key) {
            return Ok(Rc::clone(m));
        }
        let mut map = StemMap::new();
        if let Some(dir) = resolve_ci(self.game, &root.dir).filter(|d| d.is_dir()) {
            index_dir(&dir, root.recursive, &mut map)?;
        }
        let map = Rc::new(map);
        self.cache.borrow_mut().insert(key, Rc::clone(&map));
        Ok(map)
    }

    /// First root with the stem of `name`.
    fn resolve(&self, name: &str, roots: &[RootDef]) -> Result<Option<PathBuf>, GameDataError> {
        let stem = stem_of(name);
        for r in roots {
            if let Some(p) = self.stems(r)?.get(&stem) {
                return Ok(Some(p.clone()));
            }
        }
        Ok(None)
    }

    /// Path of `abs` from the game root, normalised.
    fn rel(&self, abs: &Path) -> String {
        let rel = abs.strip_prefix(self.game).unwrap_or(abs);
        normalize_path(&rel.to_string_lossy())
    }
}

fn index_dir(dir: &Path, recursive: bool, map: &mut StemMap) -> Result<(), GameDataError> {
    let entries = read_dir_sorted(dir)?;
    // Files of a folder win over its subfolders; within the folder the lower-case name decides.
    let mut files: Vec<&(String, PathBuf)> = entries.iter().filter(|(_, p)| p.is_file()).collect();
    files.sort_by_key(|(n, _)| n.to_lowercase());
    for (name, path) in files {
        let lower = name.to_lowercase();
        if let Some(stem) = lower.strip_suffix(".lua") {
            map.entry(stem.to_owned()).or_insert_with(|| path.clone());
        }
    }
    if recursive {
        for (_, path) in entries.iter().filter(|(_, p)| p.is_dir()) {
            index_dir(path, true, map)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Closure
// ---------------------------------------------------------------------------------------------

/// A file of the closure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosureFile {
    pub abs: PathBuf,
    pub size: u64,
}

/// Result of [`compute`].
#[derive(Debug, Default)]
pub struct Closure {
    /// Normalised path from the game root to file.
    pub files: BTreeMap<String, ClosureFile>,
    /// Normalised path of the file that names the contexts (`InGame.lua`).
    pub context_entry: String,
    /// Context files, in the order of the `LoadNewContext` lines.
    pub contexts: Vec<String>,
    /// Map script name to its normalised path.
    pub map_scripts: BTreeMap<String, String>,
    /// Known-missing includes that were actually hit (lower case, with `.lua`).
    pub known_missing: BTreeSet<String>,
    /// `include_roots` as written to the index (the map script's own folder first).
    pub include_roots: Vec<String>,
    /// Includes that the index order would resolve to another file than civhost's roots do.
    pub warnings: Vec<String>,
    /// The files the gameplay contexts reach: the entry, the contexts and what they include. The replica in the
    /// client loads these and no map script (it never builds a map).
    pub context_files: BTreeSet<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Side {
    Map(usize),
    Context,
}

/// Computes the closure for `spec` under `game`. Unresolved includes that are not in
/// `spec.known_missing`, a map script or context that is not found, and an install without the
/// context entry are errors.
pub fn compute(game: &Path, spec: &ClosureSpec) -> Result<Closure, GameDataError> {
    let res = Resolver::new(game);
    let mut closure = Closure::default();
    let mut errors: Vec<String> = Vec::new();
    // Roots per side. A map script searches its own folder first.
    let mut map_roots: Vec<Vec<RootDef>> = Vec::new();
    let mut queue: VecDeque<(PathBuf, Side)> = VecDeque::new();

    for (name, rel) in &spec.map_scripts {
        let abs = resolve_ci(game, rel)
            .filter(|p| p.is_file())
            .ok_or_else(|| {
                invalid(format!(
                    "map script {name}: {rel} not found under {}",
                    game.display()
                ))
            })?;
        let folder = abs.parent().map(|p| res.rel(p)).unwrap_or_default();
        let mut roots = vec![RootDef {
            dir: folder,
            recursive: false,
        }];
        roots.extend(spec.map_roots.iter().cloned());
        let side = Side::Map(map_roots.len());
        map_roots.push(roots);
        closure.map_scripts.insert(name.clone(), res.rel(&abs));
        queue.push_back((abs, side));
    }

    // Contexts: InGame.lua names them.
    let entry = res
        .resolve(&spec.context_entry, &spec.context_roots)?
        .ok_or_else(|| {
            invalid(format!(
                "{}.lua not found in the context roots",
                spec.context_entry
            ))
        })?;
    closure.context_entry = res.rel(&entry);
    let entry_src = fs::read(&entry).map_err(|e| io_err(&entry, e))?;
    queue.push_back((entry, Side::Context));
    for name in scan(&entry_src).contexts {
        match res.resolve(&name, &spec.context_roots)? {
            Some(abs) => {
                let rel = res.rel(&abs);
                if !closure.contexts.contains(&rel) {
                    closure.contexts.push(rel);
                }
                queue.push_back((abs, Side::Context));
            }
            None => errors.push(format!(
                "context {name:?} named in {} is not found",
                closure.context_entry
            )),
        }
    }

    // Entry files the gamecore loads by bare name: resolved like an include of a map script.
    for name in &spec.extra_entries {
        match res.resolve(name, &spec.map_roots)? {
            Some(abs) => {
                let folder = abs.parent().map(|p| res.rel(p)).unwrap_or_default();
                let mut roots = vec![RootDef {
                    dir: folder,
                    recursive: false,
                }];
                roots.extend(spec.map_roots.iter().cloned());
                let side = Side::Map(map_roots.len());
                map_roots.push(roots);
                queue.push_back((abs, side));
            }
            None => errors.push(format!("entry file {name:?} is not found in the map roots")),
        }
    }

    let first_script = spec
        .default_script
        .as_ref()
        .and_then(|k| closure.map_scripts.get(k))
        .or_else(|| closure.map_scripts.values().next());
    let mut index_roots = vec![RootDef {
        dir: first_script
            .and_then(|p| p.rsplit_once('/').map(|(d, _)| d.to_owned()))
            .unwrap_or_default(),
        recursive: false,
    }];
    index_roots.extend(spec.index_roots.iter().cloned());
    closure.include_roots = index_roots
        .iter()
        .filter(|r| !r.dir.is_empty())
        .map(|r| normalize_path(&r.dir))
        .collect();

    let mut done: BTreeSet<(String, Side)> = BTreeSet::new();
    let mut warned: BTreeSet<String> = BTreeSet::new();
    while let Some((abs, side)) = queue.pop_front() {
        let rel = res.rel(&abs);
        if !done.insert((rel.clone(), side)) {
            continue;
        }
        let src = fs::read(&abs).map_err(|e| io_err(&abs, e))?;
        closure
            .files
            .entry(rel.clone())
            .or_insert_with(|| ClosureFile {
                abs: abs.clone(),
                size: src.len() as u64,
            });
        let roots = match side {
            Side::Map(i) => &map_roots[i],
            Side::Context => &spec.context_roots,
        };
        for name in scan(&src).includes {
            match res.resolve(&name, roots)? {
                Some(found) => {
                    let found_rel = res.rel(&found);
                    // A map script is loaded with its own folder in front of the index roots.
                    let via_index = match side {
                        Side::Map(i) => {
                            let mut v = vec![map_roots[i][0].clone()];
                            v.extend(index_roots.iter().skip(1).cloned());
                            res.resolve(&name, &v)?
                        }
                        Side::Context => res.resolve(&name, &index_roots)?,
                    }
                    .map(|p| res.rel(&p));
                    if via_index.as_deref() != Some(found_rel.as_str())
                        && warned.insert(format!("{name}|{found_rel}"))
                    {
                        closure.warnings.push(format!(
                            "include {name:?} in {rel} resolves to {found_rel} by the civhost roots but to {} by include_roots",
                            via_index.as_deref().unwrap_or("nothing")
                        ));
                    }
                    queue.push_back((found, side));
                }
                None => {
                    let missing = format!("{}.lua", stem_of(&name));
                    if spec.known_missing.contains(&missing) {
                        closure.known_missing.insert(missing);
                    } else {
                        errors.push(format!("include {name:?} in {rel} is not found"));
                    }
                }
            }
        }
    }
    closure.context_files = done
        .iter()
        .filter(|(_, side)| *side == Side::Context)
        .map(|(rel, _)| rel.clone())
        .collect();
    if !errors.is_empty() {
        errors.sort();
        errors.dedup();
        return Err(invalid(format!(
            "unresolved Lua includes (add to KNOWN_MISSING only if the original game fails on them too):\n  {}",
            errors.join("\n  ")
        )));
    }
    Ok(closure)
}

// ---------------------------------------------------------------------------------------------
// Check against a civhost log
// ---------------------------------------------------------------------------------------------

/// Compares `closure` with a civhost load log. What the log carries: `LoadFile ... -> <script>`,
/// `include: file not found: <name>` and `N/M gameplay context(s) loaded`. Returns one warning per
/// disagreement (empty if the log agrees).
pub fn check_civhost_log(log: &str, closure: &Closure) -> Vec<String> {
    let mut warnings = Vec::new();
    let mut logged_missing: BTreeSet<String> = BTreeSet::new();
    let mut scripts: Vec<String> = Vec::new();
    let mut contexts_total: Option<usize> = None;
    for line in log.lines() {
        if let Some((_, name)) = line.split_once("include: file not found: ") {
            logged_missing.insert(format!("{}.lua", stem_of(name.trim())));
        } else if let Some((_, path)) = line.split_once("lua: LoadFile ")
            && let Some((_, abs)) = path.split_once(" -> ")
        {
            scripts.push(normalize_path(abs.trim()));
        } else if let Some((counts, _)) = line.split_once(" gameplay context(s)")
            && let Some((_, slash)) = counts.rsplit_once(' ').map(|(a, b)| (a, b.trim()))
            && let Some((loaded, total)) = slash.split_once('/')
            && loaded == total
        {
            contexts_total = total.parse().ok();
        }
    }
    for m in logged_missing.difference(&closure.known_missing) {
        warnings.push(format!(
            "civhost log: include {m} not found, but the closure resolves it or does not know it"
        ));
    }
    for m in closure.known_missing.difference(&logged_missing) {
        warnings.push(format!(
            "closure: {m} is expected missing, but the civhost log does not report it"
        ));
    }
    for s in &scripts {
        if !closure
            .map_scripts
            .values()
            .any(|p| s.ends_with(p.as_str()))
        {
            warnings.push(format!(
                "civhost log loaded map script {s}, which is not a map script of the index"
            ));
        }
    }
    match contexts_total {
        Some(t) if t != closure.contexts.len() => warnings.push(format!(
            "civhost log loaded {t} gameplay contexts, the closure has {}",
            closure.contexts.len()
        )),
        None => warnings.push("civhost log has no `N/N gameplay context(s)` line".into()),
        _ => {}
    }
    warnings
}
