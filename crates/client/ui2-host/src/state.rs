//! The skin's own state (`state.*`): the package lists its paths with their defaults (`[state]` of `skin.toml`,
//! typed by the converter); the host turns that list into a model fragment, mounts it in the store through the
//! actions module and writes the defaults. From then on the markup's and the scripts' writes go through
//! `UiActions::state_ops`, the bindings and the scripts read the store, and a skin change sets the state aside.
//!
//! A path is identifiers separated by dots (`prod.sort`): every name but the last is a struct of the fragment. The
//! value's kind is the path's type: a bool `bool`, an integer `i32`, a float `f32`, a string `str`. The package
//! format also has lists of scalars, which the model has no type for; a skin never declares one (the converter
//! refuses them), and a package that does has those paths left out with a warning.

use std::collections::BTreeMap;
use std::sync::Arc;

use ui2_actions::{StateError, UiActions};
use ui2_model::{Fragment, ModelStore, Value};
use ui2_package::{Lit, Package};

/// The state of a skin as the actions module mounts it: the fragment, and the defaults by full path
/// (`state.prod.sort`).
pub(crate) struct SkinState {
    pub(crate) fragment: Fragment,
    pub(crate) defaults: Vec<(String, Value)>,
}

/// A node of the path tree: a leaf with its type and default, or a struct of named children.
enum Tree {
    Leaf(&'static str, Value),
    Struct(BTreeMap<String, Tree>),
}

/// The model type and value of a default; `None` for a kind the model cannot hold.
fn typed(lit: &Lit) -> Result<(&'static str, Value), String> {
    Ok(match lit {
        Lit::Bool(b) => ("bool", Value::Bool(*b)),
        Lit::Int(i) => {
            let v = i32::try_from(*i)
                .map_err(|_| format!("the default {i} does not fit a 32-bit integer"))?;
            ("i32", Value::I32(v))
        }
        Lit::Float(f) => ("f32", Value::F32(*f as f32)),
        Lit::Str(s) => ("str", Value::Str(Arc::from(s.as_str()))),
        Lit::List(_) => return Err("a list is not a state type of the model".into()),
    })
}

/// Puts a leaf at `path` in the tree; `Err` when the path runs into a leaf or a struct already there.
fn insert(tree: &mut BTreeMap<String, Tree>, path: &str, leaf: Tree) -> Result<(), String> {
    let (head, rest) = match path.split_once('.') {
        Some((h, r)) => (h, Some(r)),
        None => (path, None),
    };
    match rest {
        None => {
            if tree.contains_key(head) {
                return Err("another path already uses this name".into());
            }
            tree.insert(head.to_string(), leaf);
            Ok(())
        }
        Some(rest) => {
            let node = tree
                .entry(head.to_string())
                .or_insert_with(|| Tree::Struct(BTreeMap::new()));
            match node {
                Tree::Struct(children) => insert(children, rest, leaf),
                Tree::Leaf(..) => Err(format!("`{head}` is a value, not a group")),
            }
        }
    }
}

/// Writes the fields of one struct (the root or a group), then its groups; the types of groups are `S<n>`, `n`
/// counting every group of the fragment.
fn write_struct(
    tree: &BTreeMap<String, Tree>,
    prefix: &str,
    table: &str,
    out: &mut Vec<String>,
    groups_seen: &mut usize,
    defaults: &mut Vec<(String, Value)>,
) {
    let mut body = format!("[{table}]\n");
    let mut groups = Vec::new();
    for (name, node) in tree {
        let path = format!("{prefix}.{name}");
        match node {
            Tree::Leaf(ty, v) => {
                body.push_str(&format!("{name} = \"{ty}\"\n"));
                defaults.push((path, v.clone()));
            }
            Tree::Struct(children) => {
                *groups_seen += 1;
                let ty = format!("S{groups_seen}");
                body.push_str(&format!("{name} = \"{ty}\"\n"));
                groups.push((ty, path, children));
            }
        }
    }
    out.push(body);
    for (ty, path, children) in groups {
        write_struct(
            children,
            &path,
            &format!("type.{ty}"),
            out,
            groups_seen,
            defaults,
        );
    }
}

/// The state of a skin package: `Ok(None)` when it declares none.
///
/// # Errors
/// The declared paths do not make a model fragment (a name that is not lower-case snake case, a path that is
/// both a value and a group): the skin's state cannot be mounted.
pub(crate) fn skin_state(pkg: &Package) -> Result<Option<SkinState>, String> {
    let mut tree = BTreeMap::new();
    for (path, lit) in pkg.state_defaults() {
        let (ty, value) = match typed(lit) {
            Ok(t) => t,
            Err(why) => {
                tracing::warn!(target: "ui2_host", "state.{path} is left out: {why}");
                continue;
            }
        };
        insert(&mut tree, path, Tree::Leaf(ty, value))
            .map_err(|why| format!("state.{path}: {why}"))?;
    }
    if tree.is_empty() {
        return Ok(None);
    }
    let mut tables = Vec::new();
    let mut defaults = Vec::new();
    write_struct(&tree, "state", "root", &mut tables, &mut 0, &mut defaults);
    // `[root]` first, then the group types; the fragment language takes them in any order.
    let src = tables.join("\n");
    let fragment = Fragment::parse(&src).map_err(|e| e.to_string())?;
    Ok(Some(SkinState { fragment, defaults }))
}

/// Mounts the skin state of `pkg` in `store` for the skin `id` (and sets aside the state of the skin before it).
/// A package without state leaves `state.*` empty; a state that cannot be mounted is logged and left out (the
/// bindings then read the package's defaults and writes are refused): the skin itself still works.
pub(crate) fn mount(pkg: &Package, actions: &mut UiActions, store: &mut ModelStore) -> Vec<String> {
    let id: Arc<str> = Arc::from(pkg.manifest().id.as_str());
    let mut problems = Vec::new();
    let errors: Vec<StateError> = match skin_state(pkg) {
        Ok(Some(s)) => actions.set_skin_with_defaults(Some((id, &s.fragment)), &s.defaults, store),
        Ok(None) => actions.set_skin(None, store),
        Err(why) => {
            problems.push(format!("the skin state cannot be mounted: {why}"));
            actions.set_skin(None, store)
        }
    };
    problems.extend(errors.iter().map(ToString::to_string));
    for p in &problems {
        tracing::warn!(target: "ui2_host", "skin {}: {p}", pkg.manifest().id);
    }
    problems
}

#[cfg(test)]
mod tests {
    use ui2_package::Lit;
    use ui2_runtime::testkit::Kit;

    use super::*;

    #[test]
    fn nested_paths_become_groups_with_typed_defaults() {
        let mut kit = Kit::new();
        kit.state_default("prod.sort", Lit::Str("name".into()));
        kit.state_default("prod.page", Lit::Int(2));
        kit.state_default("zoom", Lit::Float(1.5));
        kit.state_default("open", Lit::Bool(true));
        kit.state_default("big", Lit::Int(i64::MAX));
        let pkg = kit.finish().expect("package");
        let s = skin_state(&pkg).expect("state").expect("some state");
        let mut store = ModelStore::new(ui2_model::Schema::v1());
        store
            .mount(ui2_model::Namespace::State, &s.fragment)
            .expect("mount");
        for (path, v) in &s.defaults {
            store
                .writer(ui2_model::Owner::Skin)
                .set_value(path, v.clone())
                .expect("default fits");
        }
        store.take_frame();
        let read = |p: &str| {
            let f = store.resolve(p).expect(p);
            store.value(f.id, &[]).cloned()
        };
        assert_eq!(read("state.prod.sort"), Some(Value::Str(Arc::from("name"))));
        assert_eq!(read("state.prod.page"), Some(Value::I32(2)));
        assert_eq!(read("state.zoom"), Some(Value::F32(1.5)));
        assert_eq!(read("state.open"), Some(Value::Bool(true)));
        assert!(
            store.resolve("state.big").is_err(),
            "out of range: left out"
        );
    }

    #[test]
    fn a_path_that_is_both_a_value_and_a_group_is_refused() {
        let mut kit = Kit::new();
        kit.state_default("prod", Lit::Int(1));
        kit.state_default("prod.sort", Lit::Str("name".into()));
        let pkg = kit.finish().expect("package");
        assert!(skin_state(&pkg).is_err());
        let mut kit = Kit::new();
        kit.state_default("Sort", Lit::Str("name".into()));
        let pkg = kit.finish().expect("package");
        assert!(skin_state(&pkg).is_err(), "not snake case");
    }

    #[test]
    fn no_state_is_none() {
        let pkg = Kit::new().finish().expect("package");
        assert!(skin_state(&pkg).expect("fine").is_none());
    }
}
