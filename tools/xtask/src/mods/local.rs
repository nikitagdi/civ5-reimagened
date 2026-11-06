//! The list file of a distribution (`mods.local.toml`, public example `examples/mods/hello/mods.toml`).
//!
//! ```toml
//! mods_dir = "installed"            # optional: <dir>/<id>/mod.toml are extra candidates for dependencies
//!
//! [[mod]]
//! path = "../mods/some-mod"         # a checkout, relative to this file
//!
//! [[mod]]
//! git = "https://example.org/x.git" # or a git repository at a fixed revision
//! rev = "0123abc"
//! req = "^1"                        # optional: the versions accepted (default `*`)
//! ```
//!
//! The listed mods are the picks of the resolver; `mods_dir` only adds installed candidates.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use mod_manifest::VersionReq;
use serde::Deserialize;

pub const MANIFEST: &str = "mod.toml";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListFile {
    /// Extra candidates: every `<mods_dir>/<name>/mod.toml`. Relative to the list file.
    pub mods_dir: Option<String>,
    #[serde(default, rename = "mod")]
    pub mods: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub path: Option<String>,
    pub git: Option<String>,
    pub rev: Option<String>,
    /// A `VersionReq` on the mod, default `*`.
    pub req: Option<String>,
}

/// One listed mod with its directory on disk.
#[derive(Debug, Clone)]
pub struct Source {
    pub dir: PathBuf,
    /// For messages: `path "../x"` or `git <url> @ <rev>`.
    pub origin: String,
    pub req: VersionReq,
}

pub fn parse(text: &str) -> Result<ListFile> {
    let list: ListFile = toml::from_str(text).context("not a valid mods list")?;
    for (i, e) in list.mods.iter().enumerate() {
        let n = i + 1;
        match (&e.path, &e.git, &e.rev) {
            (Some(_), None, None) | (None, Some(_), Some(_)) => {}
            (Some(_), _, _) => bail!("mod #{n}: `path` cannot be combined with `git` or `rev`"),
            (None, Some(_), None) => bail!("mod #{n}: `git` needs a `rev` (a commit or a tag)"),
            (None, None, Some(_)) => bail!("mod #{n}: `rev` needs `git`"),
            (None, None, None) => bail!("mod #{n}: give `path`, or `git` with `rev`"),
        }
        if let Some(g) = &e.git
            && (g.is_empty() || g.starts_with('-') || g.chars().any(char::is_whitespace))
        {
            bail!("mod #{n}: `git` {g:?} is not a repository address");
        }
        if let Some(r) = &e.rev
            && (r.is_empty()
                || r.starts_with('-')
                || !r
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._/-".contains(c)))
        {
            bail!("mod #{n}: `rev` {r:?} is not a commit or a tag name");
        }
        if let Some(req) = &e.req {
            req.parse::<VersionReq>()
                .map_err(|err| anyhow::anyhow!("mod #{n}: `req` {req:?}: {err}"))?;
        }
    }
    Ok(list)
}

/// The directories of the listed mods. `base` is the directory of the list file; git checkouts go below `git_cache`.
pub fn sources(list: &ListFile, base: &Path, git_cache: &Path) -> Result<Vec<Source>> {
    let mut out = Vec::new();
    for (i, e) in list.mods.iter().enumerate() {
        let req = match &e.req {
            Some(r) => r.parse().map_err(|err| anyhow::anyhow!("{r:?}: {err}"))?,
            None => "*".parse().map_err(|err| anyhow::anyhow!("{err}"))?,
        };
        let (dir, origin) = if let Some(p) = &e.path {
            (base.join(p), format!("path {p:?}"))
        } else {
            let (url, rev) = (
                e.git.as_deref().unwrap_or(""),
                e.rev.as_deref().unwrap_or(""),
            );
            let dir = checkout(git_cache, url, rev)
                .with_context(|| format!("mod #{}: git {url} @ {rev}", i + 1))?;
            (dir, format!("git {url} @ {rev}"))
        };
        out.push(Source { dir, origin, req });
    }
    Ok(out)
}

/// `<mods_dir>/<name>` for every subdirectory that holds a `mod.toml`, by name.
pub fn extra_dirs(list: &ListFile, base: &Path) -> Result<Vec<PathBuf>> {
    let Some(d) = &list.mods_dir else {
        return Ok(Vec::new());
    };
    let dir = base.join(d);
    let mut out = Vec::new();
    for item in std::fs::read_dir(&dir)
        .with_context(|| format!("`mods_dir` {d:?} ({}) cannot be read", dir.display()))?
    {
        let p = item?.path();
        if p.join(MANIFEST).is_file() {
            out.push(p);
        }
    }
    out.sort();
    Ok(out)
}

/// A shallow checkout of `rev`, reused when it is already there.
fn checkout(cache: &Path, url: &str, rev: &str) -> Result<PathBuf> {
    let name: String = url
        .rsplit(['/', ':'])
        .next()
        .unwrap_or("repo")
        .trim_end_matches(".git")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let rev_part: String = rev
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let dir = cache.join(format!("{name}-{rev_part}"));
    if dir.join(MANIFEST).is_file() {
        return Ok(dir);
    }
    std::fs::create_dir_all(&dir)?;
    let git = |args: &[&str]| -> Result<()> {
        let st = Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(args)
            .status()
            .context("failed to start git")?;
        if !st.success() {
            bail!("`git {}` failed ({st})", args.join(" "));
        }
        Ok(())
    };
    git(&["init", "-q"])?;
    git(&["fetch", "-q", "--depth", "1", "--", url, rev])?;
    git(&["checkout", "-q", "FETCH_HEAD"])?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(text: &str) -> String {
        format!("{:#}", parse(text).unwrap_err())
    }

    #[test]
    fn a_list_of_paths_and_a_git_entry_parses() {
        let l = parse(
            "mods_dir = \"x\"\n[[mod]]\npath = \"a\"\n[[mod]]\ngit = \"https://h/r.git\"\nrev = \"abc123\"\nreq = \"^1\"\n",
        )
        .unwrap();
        assert_eq!(l.mods.len(), 2);
        assert_eq!(l.mods_dir.as_deref(), Some("x"));
        assert_eq!(l.mods[1].rev.as_deref(), Some("abc123"));
    }

    #[test]
    fn an_empty_list_is_allowed() {
        assert!(parse("").unwrap().mods.is_empty());
    }

    #[test]
    fn entries_are_checked() {
        assert!(err("[[mod]]\n").contains("mod #1: give `path`"));
        assert!(
            err("[[mod]]\npath = \"a\"\n[[mod]]\ngit = \"u\"\n")
                .contains("mod #2: `git` needs a `rev`")
        );
        assert!(err("[[mod]]\nrev = \"a\"\n").contains("`rev` needs `git`"));
        assert!(err("[[mod]]\npath = \"a\"\nrev = \"a\"\n").contains("cannot be combined"));
        assert!(err("[[mod]]\ngit = \"-x\"\nrev = \"a\"\n").contains("not a repository address"));
        assert!(err("[[mod]]\ngit = \"u\"\nrev = \"a b\"\n").contains("not a commit or a tag"));
        assert!(err("[[mod]]\npath = \"a\"\nreq = \"1.2\"\n").contains("mod #1: `req`"));
        assert!(err("[[mod]]\npath = \"a\"\nbogus = 1\n").contains("bogus"));
    }

    #[test]
    fn paths_are_relative_to_the_list() {
        let l = parse("[[mod]]\npath = \"../m\"\n").unwrap();
        let s = sources(&l, Path::new("/base/dir"), Path::new("/cache")).unwrap();
        assert_eq!(s[0].dir, Path::new("/base/dir").join("../m"));
        assert_eq!(s[0].req.to_string(), "*");
    }
}
