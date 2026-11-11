//! `cargo xtask ui gen-model [--check]`: write (or verify) the files generated from the model schema.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use ui2_model::Schema;
use ui2_model::codegen::{GenFile, generate};

const SCHEMA: &str = "crates/shared/ui2-model/schema/ui-model.toml";

/// Parse the schema on disk and produce every generated file.
fn files(root: &Path) -> Result<Vec<GenFile>> {
    let src = fs::read_to_string(root.join(SCHEMA)).with_context(|| format!("reading {SCHEMA}"))?;
    let schema = Schema::parse(&src).map_err(|e| anyhow::anyhow!("{SCHEMA}: {e}"))?;
    Ok(generate(&schema))
}

/// Bring the files under `root` in line with `files`. With `check`, write nothing and return the paths that
/// differ; otherwise write what differs and return those paths.
pub(super) fn sync(root: &Path, files: &[GenFile], check: bool) -> Result<Vec<String>> {
    let mut changed = Vec::new();
    for f in files {
        let path = root.join(f.path);
        // A checkout may have converted line endings: compare the text, not the bytes of the endings.
        let current = fs::read_to_string(&path)
            .map(|s| s.replace("\r\n", "\n"))
            .ok();
        if current.as_deref() == Some(f.content.as_str()) {
            continue;
        }
        changed.push(f.path.to_string());
        if !check {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)?;
            }
            fs::write(&path, f.content.as_bytes())
                .with_context(|| format!("writing {}", f.path))?;
        }
    }
    Ok(changed)
}

/// `cargo xtask ui gen-model [--check]`.
pub(super) fn run(root: &Path, args: &[String]) -> Result<()> {
    let check = match args {
        [] => false,
        [a] if a == "--check" => true,
        _ => bail!("usage: cargo xtask ui gen-model [--check]"),
    };
    let changed = sync(root, &files(root)?, check)?;
    if check {
        if changed.is_empty() {
            println!("ui model: generated files are fresh");
            return Ok(());
        }
        bail!(
            "generated files are stale: {}; run `cargo xtask ui gen-model`",
            changed.join(", ")
        );
    }
    if changed.is_empty() {
        println!("ui model: nothing to write");
    }
    for c in &changed {
        println!("ui model: wrote {c}");
    }
    Ok(())
}

/// The `check` step: the committed generated files equal a fresh run.
pub fn check_generated(root: &Path) -> Result<()> {
    run(root, &["--check".to_string()])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("xtask-ui-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn sample() -> Vec<GenFile> {
        let schema = Schema::v1();
        generate(schema)
    }

    #[test]
    fn generating_twice_is_byte_equal() {
        assert_eq!(sample(), sample());
        let dir = temp_dir("twice");
        let first = sync(&dir, &sample(), false).unwrap();
        assert!(!first.is_empty());
        let before: Vec<Vec<u8>> = sample()
            .iter()
            .map(|f| fs::read(dir.join(f.path)).unwrap())
            .collect();
        assert!(sync(&dir, &sample(), false).unwrap().is_empty());
        let after: Vec<Vec<u8>> = sample()
            .iter()
            .map(|f| fs::read(dir.join(f.path)).unwrap())
            .collect();
        assert_eq!(before, after);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn check_fails_on_a_stale_file() {
        let dir = temp_dir("stale");
        sync(&dir, &sample(), false).unwrap();
        assert!(sync(&dir, &sample(), true).unwrap().is_empty());
        let victim = sample()[0].path;
        fs::write(dir.join(victim), "// edited by hand\n").unwrap();
        let stale = sync(&dir, &sample(), true).unwrap();
        assert_eq!(stale, vec![victim.to_string()]);
        // `--check` does not repair.
        assert_eq!(
            fs::read_to_string(dir.join(victim)).unwrap(),
            "// edited by hand\n"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn committed_files_are_fresh() {
        // The repository root is two levels above this crate.
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        check_generated(&root).expect("run `cargo xtask ui gen-model`");
    }
}
