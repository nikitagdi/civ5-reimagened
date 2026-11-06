//! The generated dist workspace `target/dist/` and the graph rules on it (R1–R11).
//!
//! ```text
//! target/dist/Cargo.toml    its own [workspace]; the profiles of the public root with panic = "unwind"
//! target/dist/Cargo.lock    a copy of the public Cargo.lock (the dist resolves from the same versions)
//! target/dist/mods.lock     the resolved set (the same bytes `mods check` writes next to the list)
//! target/dist/server/       package `dist-server`, bin `civ5-server`: path deps on the public server crate and the mod crates
//! target/dist/graph/        `mods check` only: a package that depends on every mod crate, so the graph holds them all
//! ```
//!
//! Every dependency is a path dependency; there is no `[patch]`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use cargo_metadata::Metadata;
use mod_manifest::Part;

use super::check::{Prepared, find_crate};
use super::{gen_client, gen_server};
use crate::graph::{self, ModCrates, Violation};

/// What the generated workspace contains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// `mods check`: the server package plus the graph package that holds the client halves.
    Check,
    /// `mods build --target server`: the server package only.
    Server,
    /// `mods build --target client`: the client workspace (`client/`: web and desktop), no server.
    Client,
    /// Server and client.
    All,
}

impl Mode {
    fn server(self) -> bool {
        matches!(self, Mode::Check | Mode::Server | Mode::All)
    }
    fn client(self) -> bool {
        matches!(self, Mode::Client | Mode::All)
    }
}

/// A crate of a mod: its package name, its directory and (for a half) the `rust.*.entry` path.
#[derive(Debug, Clone)]
pub struct CrateRef {
    pub name: String,
    pub dir: PathBuf,
    pub entry: String,
}

#[derive(Debug, Clone)]
pub struct DistMod {
    pub id: String,
    pub version: String,
    pub version_parts: (u32, u32, u32),
    pub dir: PathBuf,
    pub server: Option<CrateRef>,
    pub client: Option<CrateRef>,
    /// `rust.caps` of the manifest: the capabilities the client half is built with.
    pub caps: Vec<String>,
    /// The content hash of the lock (the hash the server computes for the installed directory).
    pub content_hash: [u8; 32],
}

#[derive(Debug)]
pub struct Dist {
    pub dir: PathBuf,
    pub manifest: PathBuf,
    /// The mods of the set, in load order.
    pub mods: Vec<DistMod>,
}

/// Absolute path without the `\\?\` prefix of Windows (cargo and TOML paths do not take it).
pub fn norm(p: &Path) -> Result<PathBuf> {
    let c = p
        .canonicalize()
        .with_context(|| format!("cannot resolve {}", p.display()))?;
    let s = c.to_string_lossy();
    Ok(match s.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => c,
    })
}

/// `/`-separated form for generated manifests.
pub fn toml_path(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// The mods of the resolved set with the directories of their Rust halves.
pub fn dist_mods(p: &Prepared) -> Result<Vec<DistMod>> {
    let mut out = Vec::new();
    for rm in p.resolution.set.mods() {
        let dir = p
            .dir_of(&rm.id, rm.version)
            .ok_or_else(|| anyhow!("mod `{}` has no directory", rm.id))?;
        let dir = norm(dir)?;
        let manifest = &p
            .mods
            .iter()
            .find(|m| {
                m.installed.manifest.id == rm.id && m.installed.manifest.version == rm.version
            })
            .ok_or_else(|| anyhow!("mod `{}` is not loaded", rm.id))?
            .installed
            .manifest;
        let half = |part: Part,
                    entry: &Option<mod_manifest::RustEntry>|
         -> Result<Option<CrateRef>> {
            if !manifest.has_part(part) {
                return Ok(None);
            }
            let e = entry.as_ref().ok_or_else(|| {
                anyhow!(
                    "mod `{}`: part `{}` without a rust entry",
                    rm.id,
                    part.as_str()
                )
            })?;
            let cdir = find_crate(&dir, &e.crate_name)
                .with_context(|| format!("mod `{}`: rust entry of `{}`", rm.id, part.as_str()))?;
            Ok(Some(CrateRef {
                name: e.crate_name.clone(),
                dir: norm(&cdir)?,
                entry: e.entry.clone(),
            }))
        };
        let server = half(Part::ServerRs, &manifest.rust.server)?;
        let client = half(Part::ClientRs, &manifest.rust.client)?;
        out.push(DistMod {
            id: rm.id.to_string(),
            version: rm.version.to_string(),
            version_parts: (rm.version.major, rm.version.minor, rm.version.patch),
            dir,
            server,
            client,
            caps: manifest.rust.caps.iter().cloned().collect(),
            content_hash: rm.content_hash.0,
        });
    }
    Ok(out)
}

/// The profiles of the public root, with the panic strategy a mod-bearing server needs (plan decision 7: a panic
/// in a mod is caught by the host, which needs `unwind`).
fn profiles(root: &Path, unwind: bool) -> Result<toml::Table> {
    let text = std::fs::read_to_string(root.join("Cargo.toml"))
        .context("cannot read the root Cargo.toml")?;
    let doc: toml::Table = text.parse().context("cannot parse the root Cargo.toml")?;
    let mut profile = match doc.get("profile") {
        Some(toml::Value::Table(t)) => t.clone(),
        _ => toml::Table::new(),
    };
    for name in ["dev", "release"].into_iter().filter(|_| unwind) {
        let entry = profile
            .entry(name.to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if let toml::Value::Table(t) = entry {
            t.insert("panic".into(), "unwind".into());
        }
    }
    Ok(profile)
}

fn write_if_changed(path: &Path, text: &str) -> Result<()> {
    if std::fs::read_to_string(path).ok().as_deref() != Some(text) {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(path, text).with_context(|| format!("cannot write {}", path.display()))?;
    }
    Ok(())
}

/// Copies the public `Cargo.lock` into `dir`: refreshed when the public lock changed since the last run; between
/// those, cargo's own additions for the mod crates stay (so third-party versions of a mod do not drift from run to
/// run).
fn sync_lock(root: &Path, dir: &Path) -> Result<()> {
    let lock =
        std::fs::read_to_string(root.join("Cargo.lock")).context("cannot read Cargo.lock")?;
    let seen = dir.join("Cargo.lock.public");
    if !dir.join("Cargo.lock").exists()
        || std::fs::read_to_string(&seen).ok().as_deref() != Some(lock.as_str())
    {
        std::fs::write(dir.join("Cargo.lock"), &lock)?;
        std::fs::write(&seen, &lock)?;
    }
    Ok(())
}

/// The `Cargo.toml` of a generated workspace.
fn workspace_manifest(root: &Path, members: &[&str], unwind: bool, note: &str) -> Result<String> {
    let mut ws = toml::Table::new();
    let mut w = toml::Table::new();
    w.insert("resolver".into(), "3".into());
    w.insert(
        "members".into(),
        toml::Value::Array(
            members
                .iter()
                .map(|m| toml::Value::String((*m).to_string()))
                .collect(),
        ),
    );
    ws.insert("workspace".into(), toml::Value::Table(w));
    ws.insert(
        "profile".into(),
        toml::Value::Table(profiles(root, unwind)?),
    );
    Ok(format!(
        "# Generated by `cargo xtask mods`; rewritten on every run. Do not edit.
{note}{}",
        toml::to_string(&ws).context("dist Cargo.toml")?
    ))
}

/// The pinned `wasm-bindgen` requirement of the public workspace (the generated web package needs the crate for the
/// start function; the CLI of the bindgen step is checked against the same version).
fn wasm_bindgen_req(root: &Path) -> Result<String> {
    let text = std::fs::read_to_string(root.join("Cargo.toml"))
        .context("cannot read the root Cargo.toml")?;
    let doc: toml::Table = text.parse().context("cannot parse the root Cargo.toml")?;
    doc.get("workspace")
        .and_then(|w| w.get("dependencies"))
        .and_then(|d| d.get("wasm-bindgen"))
        .and_then(|v| {
            v.as_str()
                .or_else(|| v.get("version").and_then(|x| x.as_str()))
        })
        .map(str::to_string)
        .ok_or_else(|| anyhow!("the root Cargo.toml has no workspace dependency `wasm-bindgen`"))
}

/// Writes (or refreshes) `target/dist/`. Files whose bytes are unchanged are left alone, so cargo's fingerprints hold.
pub fn generate(root: &Path, p: &Prepared, mode: Mode) -> Result<Dist> {
    let root = norm(root)?;
    let dir = crate::target_dir(&root)?.join("dist");
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let mods = dist_mods(p)?;
    write_if_changed(&dir.join("mods.lock"), &p.lock_text())?;

    if mode.server() {
        let mut members = vec!["server"];
        if mode == Mode::Check {
            members.push("graph");
        }
        write_if_changed(
            &dir.join("Cargo.toml"),
            &workspace_manifest(
                &root,
                &members,
                true,
                "# The profiles are the public ones, but `panic = \"unwind\"`: a panic in a mod is caught by the host.
",
            )?,
        )?;
        // The dist resolves from the versions of the public lock.
        sync_lock(&root, &dir)?;

        let server_deps: Vec<(&str, &Path)> = mods
            .iter()
            .filter_map(|m| m.server.as_ref())
            .map(|c| (c.name.as_str(), c.dir.as_path()))
            .collect();
        write_if_changed(
            &dir.join("server/Cargo.toml"),
            &gen_server::server_manifest(&root, &server_deps),
        )?;
        write_if_changed(&dir.join("server/src/main.rs"), &gen_server::main_rs(&mods))?;

        if mode == Mode::Check {
            let all: Vec<(&str, &Path)> = mods
                .iter()
                .flat_map(|m| [m.server.as_ref(), m.client.as_ref()])
                .flatten()
                .map(|c| (c.name.as_str(), c.dir.as_path()))
                .collect();
            write_if_changed(
                &dir.join("graph/Cargo.toml"),
                &gen_server::graph_manifest(&all),
            )?;
            write_if_changed(
                &dir.join("graph/src/lib.rs"),
                "//! Generated: holds every mod crate in the graph.
",
            )?;
        }
    }
    if mode.client() {
        generate_client(&root, &dir.join("client"), &mods)?;
    }
    let manifest = dir.join("Cargo.toml");
    Ok(Dist {
        dir,
        manifest,
        mods,
    })
}

/// `target/dist/client/`: its own workspace with the public profiles (the client stays `panic = "abort"`), the
/// packages `dist-client-web` and `dist-desktop`.
fn generate_client(root: &Path, dir: &Path, mods: &[DistMod]) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    write_if_changed(
        &dir.join("Cargo.toml"),
        &workspace_manifest(
            root,
            &["web", "desktop"],
            false,
            "# The profiles are the public ones (the client keeps `panic = \"abort\"`).
",
        )?,
    )?;
    sync_lock(root, dir)?;
    let client_deps: Vec<(&str, &Path)> = mods
        .iter()
        .filter_map(|m| m.client.as_ref())
        .map(|c| (c.name.as_str(), c.dir.as_path()))
        .collect();
    write_if_changed(
        &dir.join("web/Cargo.toml"),
        &gen_client::web_manifest(root, &client_deps, &wasm_bindgen_req(root)?),
    )?;
    write_if_changed(&dir.join("web/src/lib.rs"), &gen_client::web_lib_rs(mods))?;
    write_if_changed(
        &dir.join("desktop/Cargo.toml"),
        &gen_client::desktop_manifest(root, &client_deps),
    )?;
    write_if_changed(
        &dir.join("desktop/src/main.rs"),
        &gen_client::desktop_main_rs(mods),
    )?;
    Ok(())
}

/// The crates of each mod in the graph of `md`: the halves, and every other local crate below the mod's directory.
pub fn mod_crates(md: &Metadata, dist: &Dist) -> Vec<ModCrates> {
    let locals: Vec<(String, PathBuf)> = md
        .packages
        .iter()
        .filter(|p| p.source.is_none())
        .filter_map(|p| {
            let dir = p.manifest_path.parent()?.as_std_path();
            Some((p.name.to_string(), norm(dir).ok()?))
        })
        .collect();
    dist.mods
        .iter()
        .map(|m| {
            let halves: BTreeSet<&str> = [m.server.as_ref(), m.client.as_ref()]
                .into_iter()
                .flatten()
                .map(|c| c.name.as_str())
                .collect();
            ModCrates {
                id: m.id.clone(),
                server: m.server.as_ref().map(|c| c.name.clone()),
                client: m.client.as_ref().map(|c| c.name.clone()),
                shared: locals
                    .iter()
                    .filter(|(n, d)| d.starts_with(&m.dir) && !halves.contains(n.as_str()))
                    .map(|(n, _)| n.clone())
                    .collect(),
            }
        })
        .collect()
}

/// R1–R10 on the dist graph and R11 on the mod crates, on the host and wasm32 graphs, default features and all.
/// Returns the violations (errors); warnings are printed.
pub fn check_graph(root: &Path, dist: &Dist) -> Result<Vec<String>> {
    let cfg = crate::load_deps_config(root)?;
    let host = crate::meta::host_triple()?;
    let mut layer_problems: Vec<String> = Vec::new();
    let mut found: Vec<(Violation, Vec<String>)> = Vec::new();
    for (label, triple) in [("host", host.as_str()), ("wasm32", crate::WASM_TARGET)] {
        for all_features in [false, true] {
            let full = if all_features {
                format!("{label}+all-features")
            } else {
                label.to_string()
            };
            let md = crate::meta::load_manifest(&dist.manifest, triple, all_features)?;
            let (g, layer_errors) = crate::meta::build_graph_local(&md)?;
            for e in layer_errors {
                let line = format!("layer: {e}");
                if !layer_problems.contains(&line) {
                    layer_problems.push(line);
                }
            }
            let mut v = graph::check_all(&g, &cfg);
            v.extend(graph::r11(&g, &cfg, &mod_crates(&md, dist)));
            graph::merge_violations(&mut found, &full, v);
            println!("dist graph {full} ({triple}): {} nodes", g.nodes.len());
        }
    }
    let mut problems = layer_problems;
    for (v, labels) in found {
        let line = format!("{v} [graph: {}]", labels.join(", "));
        if v.warning {
            println!("WARNING {line}");
        } else {
            problems.push(line);
        }
    }
    Ok(problems)
}

/// `mods check` step: nothing to check without Rust halves.
pub fn graph_step(root: &Path, p: &Prepared) -> Result<()> {
    let mods = dist_mods(p)?;
    if mods
        .iter()
        .all(|m| m.server.is_none() && m.client.is_none())
    {
        println!("dist graph: no mod has Rust code, nothing to check");
        return Ok(());
    }
    let dist = generate(root, p, Mode::Check)?;
    let problems = check_graph(root, &dist)?;
    if problems.is_empty() {
        println!("dist graph: rules R1–R11 satisfied");
        return Ok(());
    }
    for line in &problems {
        println!("VIOLATION {line}");
    }
    bail!("dist graph: violations found: {}", problems.len());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::check::{self, tests::Scratch};

    fn repo() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .to_path_buf()
    }

    fn prepared(list: &str, s: &Scratch) -> Prepared {
        check::prepare(&repo().join(list), &s.0.join("work")).unwrap()
    }

    #[test]
    fn the_client_workspace_registers_each_client_half_with_the_facts_of_its_mod() {
        let s = Scratch::new();
        let p = prepared("tools/xtask/tests/fixtures/client-mod/mods.toml", &s);
        let mods = dist_mods(&p).unwrap();
        assert_eq!(mods.len(), 1);
        assert!(mods[0].server.is_none());
        let c = mods[0].client.as_ref().unwrap();
        assert_eq!(c.name, "fixture-client");
        assert_eq!(mods[0].version_parts, (0, 3, 1));
        assert_eq!(mods[0].caps, vec!["camera", "input.claim"]);

        let dir = s.0.join("client");
        generate_client(&repo(), &dir, &mods).unwrap();
        let lib = std::fs::read_to_string(dir.join("web/src/lib.rs")).unwrap();
        assert!(lib.contains("register_fixture_client(&mut registry);"));
        assert!(lib.contains("fixture_client::register(r);"));
        assert!(lib.contains("mod_api::Version::new(0, 3, 1)"));
        assert!(lib.contains("from_names([\"camera\", \"input.claim\"])"));
        // The code hash is the first 16 bytes of the hash in `mods.lock`.
        let hash = p.resolution.set.mods()[0].content_hash.0;
        let hex: Vec<String> = hash[..16].iter().map(|b| format!("0x{b:02x}")).collect();
        assert!(lib.contains(&format!("code_hash: [{}]", hex.join(", "))));
        let main = std::fs::read_to_string(dir.join("desktop/src/main.rs")).unwrap();
        assert!(main.contains("fixture_client::register(r);"));

        // The client keeps the public profiles: no `panic = "unwind"`. Its lock is the public one.
        let ws: toml::Table = std::fs::read_to_string(dir.join("Cargo.toml"))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            ws["workspace"]["members"].as_array().unwrap().len(),
            2,
            "{ws:?}"
        );
        assert!(
            ws.get("profile")
                .and_then(|p| p.get("release"))
                .and_then(|r| r.get("panic"))
                .is_none_or(|v| v.as_str() != Some("unwind"))
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("Cargo.lock")).unwrap(),
            std::fs::read_to_string(repo().join("Cargo.lock")).unwrap()
        );
        let web: toml::Table = std::fs::read_to_string(dir.join("web/Cargo.toml"))
            .unwrap()
            .parse()
            .unwrap();
        assert!(
            web["dependencies"]["wasm-bindgen"]
                .as_str()
                .unwrap()
                .starts_with('=')
        );
    }

    #[test]
    fn a_mod_without_a_client_half_registers_nothing() {
        let s = Scratch::new();
        let p = prepared("examples/mods/hello/mods.toml", &s);
        let mods = dist_mods(&p).unwrap();
        let dir = s.0.join("client");
        generate_client(&repo(), &dir, &mods).unwrap();
        let lib = std::fs::read_to_string(dir.join("web/src/lib.rs")).unwrap();
        assert!(lib.contains("//! - hello 0.1.0 (no client code)"));
        assert!(!lib.contains("ClientRegistry"));
        assert!(lib.contains("app_web::start_with(mod_hooks())"));
    }

    #[test]
    fn the_server_workspace_unwinds_and_the_modes_pick_their_parts() {
        assert!(Mode::Check.server() && !Mode::Check.client());
        assert!(Mode::Client.client() && !Mode::Client.server());
        assert!(Mode::All.server() && Mode::All.client());
        let root = repo();
        let ws = workspace_manifest(&root, &["server"], true, "").unwrap();
        let t: toml::Table = ws.parse().unwrap();
        assert_eq!(t["profile"]["release"]["panic"].as_str(), Some("unwind"));
        assert_eq!(t["profile"]["dev"]["panic"].as_str(), Some("unwind"));
    }
}
