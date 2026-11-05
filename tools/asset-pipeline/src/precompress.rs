//! `civ5-assets precompress`: writes `<file>.zst` and `<file>.br` next to the files of a folder, for
//! the gateway and Caddy to serve by `Accept-Encoding`.
//!
//! zstd comes from the `zstd` crate, brotli from the external `brotli` binary (`-q 11`). A sidecar
//! is kept only if the file is at least [`MIN_SIZE`] bytes and the sidecar at most 95 % of it; a
//! stale one that fails the rule is deleted. A sidecar newer than its file is not redone.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

use rayon::prelude::*;

/// Smaller files are not worth a `Content-Encoding`.
pub const MIN_SIZE: u64 = 256;
/// A sidecar must be at most this share (percent) of the original.
pub const MAX_RATIO_PERCENT: u64 = 95;
pub const DEFAULT_ZSTD_LEVEL: i32 = 19;

#[derive(Debug, Clone)]
pub struct Options {
    pub dir: PathBuf,
    /// `brotli` binary; `None` looks it up in PATH.
    pub brotli_bin: Option<PathBuf>,
    pub zstd_level: i32,
    /// Only files named by 64 hex digits (blobs).
    pub only_hash_names: bool,
}

/// Bytes of one group of files.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Sizes {
    pub files: u64,
    pub original: u64,
    /// Bytes served with a br sidecar (the original size where there is none).
    pub br: u64,
    pub zst: u64,
}

impl Sizes {
    fn add(&mut self, o: &Sizes) {
        self.files += o.files;
        self.original += o.original;
        self.br += o.br;
        self.zst += o.zst;
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Summary {
    pub total: Sizes,
    /// `.wasm` and `.js` files.
    pub web: Sizes,
    pub br_made: u64,
    pub zst_made: u64,
    pub removed_stale: u64,
    /// brotli was wanted but the binary was not found.
    pub brotli_missing: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum PrecompressError {
    #[error("i/o error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{0}")]
    Failed(String),
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> PrecompressError + '_ {
    move |source| PrecompressError::Io {
        path: path.to_owned(),
        source,
    }
}

fn is_hash_name(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Files that are not content for a client: pointers, history, caches, sidecars themselves.
fn skipped(name: &str) -> bool {
    name.ends_with(".br")
        || name.ends_with(".zst")
        || name.starts_with(".")
        || matches!(name, "manifest" | "manifests.log")
}

fn collect(dir: &Path, only_hash: bool, out: &mut Vec<PathBuf>) -> Result<(), PrecompressError> {
    for e in fs::read_dir(dir).map_err(io_err(dir))? {
        let e = e.map_err(io_err(dir))?;
        let name = e.file_name().to_string_lossy().into_owned();
        let ty = e.file_type().map_err(io_err(&e.path()))?;
        if ty.is_dir() {
            // `.cache` and other dot folders hold build state.
            if !name.starts_with('.') {
                collect(&e.path(), only_hash, out)?;
            }
        } else if ty.is_file() && !skipped(&name) && (!only_hash || is_hash_name(&name)) {
            out.push(e.path());
        }
    }
    Ok(())
}

fn sidecar(path: &Path, ext: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".");
    s.push(ext);
    PathBuf::from(s)
}

/// Finds `brotli` for `-q 11` runs: the given path, else PATH.
pub fn find_brotli(given: Option<&Path>) -> Option<PathBuf> {
    let probe = |p: &Path| {
        Command::new(p)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    match given {
        Some(p) => probe(p).then(|| p.to_owned()),
        None => {
            let p = PathBuf::from("brotli");
            probe(&p).then_some(p)
        }
    }
}

fn brotli_compress(bin: &Path, file: &Path) -> Result<Vec<u8>, PrecompressError> {
    // The file goes in through stdin: some builds of `brotli` fail on long Windows paths.
    let input = fs::File::open(file).map_err(io_err(file))?;
    let out = Command::new(bin)
        .args(["-q", "11", "-f", "-c"])
        .stdin(Stdio::from(input))
        .stderr(Stdio::piped())
        .output()
        .map_err(io_err(bin))?;
    if !out.status.success() {
        return Err(PrecompressError::Failed(format!(
            "brotli failed on {}: {}",
            file.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(out.stdout)
}

/// Result for one sidecar: its size if it exists after the call, and whether it was written.
struct Side {
    size: Option<u64>,
    made: bool,
    removed: bool,
}

/// Brings `<file>.<ext>` in line with the rule; `make` produces the compressed bytes.
fn ensure(
    file: &Path,
    ext: &str,
    len: u64,
    modified: Option<SystemTime>,
    make: impl FnOnce() -> Result<Vec<u8>, PrecompressError>,
) -> Result<Side, PrecompressError> {
    let side = sidecar(file, ext);
    let existing = fs::metadata(&side).ok();
    if len < MIN_SIZE {
        let removed = existing.is_some() && fs::remove_file(&side).is_ok();
        return Ok(Side {
            size: None,
            made: false,
            removed,
        });
    }
    if let Some(m) = &existing
        && let (Ok(side_time), Some(file_time)) = (m.modified(), modified)
        && side_time >= file_time
    {
        return Ok(Side {
            size: Some(m.len()),
            made: false,
            removed: false,
        });
    }
    let bytes = make()?;
    if bytes.len() as u64 * 100 > len * MAX_RATIO_PERCENT {
        let removed = existing.is_some() && fs::remove_file(&side).is_ok();
        return Ok(Side {
            size: None,
            made: false,
            removed,
        });
    }
    let tmp = sidecar(file, &format!("{ext}.tmp"));
    fs::write(&tmp, &bytes).map_err(io_err(&tmp))?;
    fs::rename(&tmp, &side).map_err(io_err(&side))?;
    Ok(Side {
        size: Some(bytes.len() as u64),
        made: true,
        removed: false,
    })
}

fn process(
    file: &Path,
    zstd_level: i32,
    brotli: Option<&Path>,
) -> Result<Summary, PrecompressError> {
    let meta = fs::metadata(file).map_err(io_err(file))?;
    let len = meta.len();
    let modified = meta.modified().ok();
    let mut s = Summary::default();

    let zst = ensure(file, "zst", len, modified, || {
        let data = fs::read(file).map_err(io_err(file))?;
        zstd::bulk::compress(&data, zstd_level).map_err(io_err(file))
    })?;
    let br = match brotli {
        Some(bin) => ensure(file, "br", len, modified, || brotli_compress(bin, file))?,
        None => Side {
            size: fs::metadata(sidecar(file, "br")).ok().map(|m| m.len()),
            made: false,
            removed: false,
        },
    };
    s.zst_made += u64::from(zst.made);
    s.br_made += u64::from(br.made);
    s.removed_stale += u64::from(zst.removed) + u64::from(br.removed);
    let sizes = Sizes {
        files: 1,
        original: len,
        br: br.size.unwrap_or(len),
        zst: zst.size.unwrap_or(len),
    };
    s.total = sizes;
    let ext = file
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    if matches!(ext.as_deref(), Some("wasm" | "js" | "mjs")) {
        s.web = sizes;
    }
    Ok(s)
}

pub fn run(opts: &Options) -> Result<Summary, PrecompressError> {
    let mut files = Vec::new();
    collect(&opts.dir, opts.only_hash_names, &mut files)?;
    files.sort();
    let brotli = find_brotli(opts.brotli_bin.as_deref());
    let results: Vec<Result<Summary, PrecompressError>> = files
        .par_iter()
        .map(|f| process(f, opts.zstd_level, brotli.as_deref()))
        .collect();
    let mut total = Summary {
        brotli_missing: brotli.is_none(),
        ..Summary::default()
    };
    for r in results {
        let r = r?;
        total.total.add(&r.total);
        total.web.add(&r.web);
        total.br_made += r.br_made;
        total.zst_made += r.zst_made;
        total.removed_stale += r.removed_stale;
    }
    Ok(total)
}

impl Summary {
    pub fn to_text(&self) -> String {
        use std::fmt::Write;
        let mb = |b: u64| b as f64 / 1e6;
        let mut s = String::new();
        for (name, z) in [("all files", &self.total), ("wasm/js", &self.web)] {
            let _ = writeln!(
                s,
                "{name}: {} file(s), original {:.2} MB, br {:.2} MB, zstd {:.2} MB",
                z.files,
                mb(z.original),
                mb(z.br),
                mb(z.zst)
            );
        }
        let _ = writeln!(
            s,
            "written: {} br, {} zstd; stale sidecars removed: {}",
            self.br_made, self.zst_made, self.removed_stale
        );
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("civ5-pc-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn opts(d: &Dir, only_hash: bool) -> Options {
        Options {
            dir: d.0.clone(),
            brotli_bin: None,
            zstd_level: 3,
            only_hash_names: only_hash,
        }
    }

    fn compressible() -> Vec<u8> {
        b"civ5 asset delivery "
            .iter()
            .copied()
            .cycle()
            .take(4000)
            .collect()
    }

    /// Pseudo-random bytes that do not compress.
    fn noise(n: usize) -> Vec<u8> {
        let mut x = 0x2545_f491_4f6c_dd1du64;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect()
    }

    #[test]
    fn zstd_sidecars_follow_the_rule() {
        let d = Dir::new("rule");
        let hash = "ab".repeat(32);
        fs::write(d.0.join(&hash), compressible()).unwrap();
        fs::write(d.0.join("app.wasm"), compressible()).unwrap();
        fs::write(d.0.join("tiny.js"), b"x").unwrap();
        fs::write(d.0.join("noise.bin"), noise(4000)).unwrap();
        fs::write(d.0.join("manifest"), "x".repeat(300)).unwrap();
        fs::write(d.0.join("manifests.log"), compressible()).unwrap();
        fs::create_dir_all(d.0.join(".cache")).unwrap();
        fs::write(d.0.join(".cache").join("c"), compressible()).unwrap();
        fs::create_dir_all(d.0.join("sub")).unwrap();
        fs::write(d.0.join("sub").join("a.css"), compressible()).unwrap();
        // A stale sidecar of a file that no longer compresses is removed.
        let stale = d.0.join("noise.bin.zst");
        fs::write(&stale, b"stale").unwrap();
        fs::File::options()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_modified(SystemTime::now() - std::time::Duration::from_secs(100))
            .unwrap();

        let s = run(&opts(&d, false)).unwrap();
        for ok in [hash.as_str(), "app.wasm", "sub/a.css"] {
            let side = sidecar(&d.0.join(ok), "zst");
            let packed = fs::read(&side).unwrap();
            assert!(packed.len() < 400, "{ok}");
            assert_eq!(zstd::decode_all(&packed[..]).unwrap(), compressible());
        }
        for none in [
            "tiny.js.zst",
            "noise.bin.zst",
            "manifest.zst",
            "manifests.log.zst",
            ".cache/c.zst",
        ] {
            assert!(!d.0.join(none).exists(), "{none}");
        }
        assert_eq!(s.total.files, 5, "tiny, noise, app.wasm, hash, a.css");
        assert_eq!(s.zst_made, 3);
        assert_eq!(s.removed_stale, 1);
        assert_eq!(s.web.files, 2, "tiny.js and app.wasm");
        assert!(s.total.zst < s.total.original);

        // A second run skips the finished sidecars; sidecars are not compressed again.
        let again = run(&opts(&d, false)).unwrap();
        assert_eq!(again.zst_made, 0);
        assert_eq!(again.total.files, 5);
        assert!(!d.0.join(format!("{hash}.zst.zst")).exists());
    }

    #[test]
    fn only_hash_names_leaves_other_files_alone() {
        let d = Dir::new("hash");
        let hash = "0f".repeat(32);
        fs::write(d.0.join(&hash), compressible()).unwrap();
        fs::write(d.0.join("app.wasm"), compressible()).unwrap();
        fs::write(d.0.join("F".repeat(64)), compressible()).unwrap();
        let s = run(&opts(&d, true)).unwrap();
        assert_eq!(s.total.files, 1);
        assert!(d.0.join(format!("{hash}.zst")).is_file());
        assert!(!d.0.join("app.wasm.zst").exists());
    }

    #[test]
    fn a_changed_file_gets_a_fresh_sidecar() {
        let d = Dir::new("fresh");
        let f = d.0.join("a.js");
        fs::write(&f, compressible()).unwrap();
        run(&opts(&d, false)).unwrap();
        let old = sidecar(&f, "zst");
        // Make the sidecar older than the file.
        let t = SystemTime::now() - std::time::Duration::from_secs(100);
        fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(t)
            .unwrap();
        let s = run(&opts(&d, false)).unwrap();
        assert_eq!(s.zst_made, 1);
    }

    #[test]
    fn brotli_sidecar_when_the_binary_exists() {
        let Some(bin) = find_brotli(None) else {
            eprintln!("brotli binary not found, part skipped");
            return;
        };
        let d = Dir::new("br");
        fs::write(d.0.join("a.wasm"), compressible()).unwrap();
        let s = run(&Options {
            brotli_bin: Some(bin),
            ..opts(&d, false)
        })
        .unwrap();
        assert!(!s.brotli_missing);
        assert_eq!(s.br_made, 1);
        let br = fs::metadata(d.0.join("a.wasm.br")).unwrap().len();
        assert!(br > 0 && br < 400);
        assert_eq!(s.total.br, br);
    }

    #[test]
    fn a_missing_brotli_binary_is_reported_and_zstd_still_runs() {
        let d = Dir::new("nobr");
        fs::write(d.0.join("a.js"), compressible()).unwrap();
        let s = run(&Options {
            brotli_bin: Some(PathBuf::from("no-such-brotli-binary")),
            ..opts(&d, false)
        })
        .unwrap();
        assert!(s.brotli_missing);
        assert_eq!(s.zst_made, 1);
        assert!(!d.0.join("a.js.br").exists());
    }
}
