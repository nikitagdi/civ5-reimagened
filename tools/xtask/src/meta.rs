//! Populates the graph model from `cargo metadata`.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use cargo_metadata::{DependencyKind, Metadata, MetadataCommand};

use crate::graph::{EdgeKind, Graph, Layer};

// Without all_features, optional deps behind non-default features are not resolved.
pub fn load(root: &Path, triple: &str, all_features: bool) -> Result<Metadata> {
    load_manifest(&root.join("Cargo.toml"), triple, all_features)
}

/// Like [`load`] for any `Cargo.toml` (the generated dist workspace).
pub fn load_manifest(manifest: &Path, triple: &str, all_features: bool) -> Result<Metadata> {
    let mut opts = vec!["--filter-platform".to_string(), triple.to_string()];
    if all_features {
        opts.push("--all-features".to_string());
    }
    MetadataCommand::new()
        .manifest_path(manifest)
        .other_options(opts)
        .exec()
        .with_context(|| {
            format!(
                "cargo metadata --filter-platform {triple} (all_features={all_features}) failed"
            )
        })
}

/// Returns the graph and layer errors (crate without a layer, or with an unknown one).
pub fn build_graph(md: &Metadata) -> Result<(Graph, Vec<String>)> {
    build_graph_of(md, md.workspace_members.iter().collect())
}

/// The graph of a dist workspace: its crates are path dependencies outside the dist workspace root, not
/// workspace members, so every package without a registry or git source counts as a crate of ours (each needs a
/// layer).
pub fn build_graph_local(md: &Metadata) -> Result<(Graph, Vec<String>)> {
    let local = md
        .packages
        .iter()
        .filter(|p| p.source.is_none())
        .map(|p| &p.id)
        .collect();
    build_graph_of(md, local)
}

fn build_graph_of(
    md: &Metadata,
    members: Vec<&cargo_metadata::PackageId>,
) -> Result<(Graph, Vec<String>)> {
    let resolve = md
        .resolve
        .as_ref()
        .context("cargo metadata returned no resolve")?;
    let by_id: HashMap<_, _> = md.packages.iter().map(|p| (&p.id, p)).collect();
    let mut g = Graph::default();
    let mut errors = Vec::new();

    for id in members {
        let Some(p) = by_id.get(id) else { continue };
        let layer = p
            .metadata
            .get("civ5")
            .and_then(|c| c.get("layer"))
            .and_then(|l| l.as_str());
        match layer {
            None => errors.push(format!(
                "crate `{}` ({}) does not declare a layer: add `[package.metadata.civ5] layer = \"...\"` \
                 (shared, build, client, platform-web, platform-native, app-web, app-desktop, server, tool)",
                p.name, p.manifest_path
            )),
            Some(s) => match Layer::parse(s) {
                Some(l) => {
                    g.add(&p.name, Some(l));
                }
                None => errors.push(format!("crate `{}`: unknown layer {s:?}", p.name)),
            },
        }
    }

    for node in &resolve.nodes {
        let Some(from) = by_id.get(&node.id) else {
            continue;
        };
        g.add(&from.name, None);
        for d in &node.deps {
            let Some(to) = by_id.get(&d.pkg) else {
                continue;
            };
            for k in &d.dep_kinds {
                let kind = match k.kind {
                    DependencyKind::Normal => EdgeKind::Normal,
                    DependencyKind::Build => EdgeKind::Build,
                    DependencyKind::Development => EdgeKind::Dev,
                    _ => continue,
                };
                g.edge(&from.name, &to.name, kind);
            }
        }
    }
    Ok((g, errors))
}

pub fn host_triple() -> Result<String> {
    let out = std::process::Command::new(std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into()))
        .arg("-vV")
        .output()
        .context("failed to run rustc -vV")?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .find_map(|l| l.strip_prefix("host: "))
        .map(|s| s.trim().to_string())
        .context("rustc -vV output has no host: line")
}
