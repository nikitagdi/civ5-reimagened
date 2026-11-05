//! Where `include` finds files: a name-indexed set of Lua files with layer priority.

use std::collections::HashMap;

/// Source of Lua files for `include`. The VFS of `ui-assets` implements it; `MemorySource` and
/// `DirSource` are for tests and tools. Paths use `/` or `\`; matching ignores case.
pub trait IncludeSource {
    /// The file `name` stands for: a bare file name with or without `.lua` (found in any
    /// directory, the highest-priority one wins), or a path suffix (`InGame\Foo`).
    fn resolve(&self, name: &str) -> Option<String>;
    /// Paths of all files visible after priority (one per path), sorted. Used by pattern includes.
    fn list(&self) -> Vec<String>;
    /// Lower-case stems of the XML files (contexts) of the file set; a Lua file with an XML twin is a context, not a
    /// layout (default: none known).
    fn xml_stems(&self) -> Vec<String> {
        Vec::new()
    }
    fn read(&self, path: &str) -> Option<Vec<u8>>;
}

enum Content {
    Memory(Vec<u8>),
    #[cfg(not(target_arch = "wasm32"))]
    Disk(std::path::PathBuf),
}

struct Entry {
    path: String,
    content: Content,
}

/// Index shared by the sources: later `add` calls win over earlier ones (higher layer).
#[derive(Default)]
struct Index {
    entries: Vec<Entry>,
    by_path: HashMap<String, usize>,
    by_name: HashMap<String, usize>,
}

fn norm(path: &str) -> String {
    let p = path.replace('\\', "/");
    p.trim_start_matches("./")
        .trim_start_matches('/')
        .to_owned()
}

impl Index {
    fn add(&mut self, path: &str, content: Content) {
        let path = norm(path);
        let key = path.to_ascii_lowercase();
        let name = key.rsplit('/').next().unwrap_or(&key).to_owned();
        let i = match self.by_path.get(&key) {
            Some(&i) => {
                self.entries[i] = Entry { path, content };
                i
            }
            None => {
                self.entries.push(Entry { path, content });
                self.by_path.insert(key, self.entries.len() - 1);
                self.entries.len() - 1
            }
        };
        self.by_name.insert(name, i);
    }

    fn resolve(&self, name: &str) -> Option<String> {
        let mut n = norm(name).to_ascii_lowercase();
        if !n.ends_with(".lua") {
            n.push_str(".lua");
        }
        if !n.contains('/') {
            return self.by_name.get(&n).map(|&i| self.entries[i].path.clone());
        }
        if let Some(&i) = self.by_path.get(&n) {
            return Some(self.entries[i].path.clone());
        }
        let suffix = format!("/{n}");
        self.by_path
            .iter()
            .filter(|(k, _)| k.ends_with(&suffix))
            .map(|(_, &i)| i)
            .max()
            .map(|i| self.entries[i].path.clone())
    }

    fn list(&self) -> Vec<String> {
        let mut v: Vec<String> = self.entries.iter().map(|e| e.path.clone()).collect();
        v.sort();
        v
    }

    fn read(&self, path: &str) -> Option<Vec<u8>> {
        let &i = self.by_path.get(&norm(path).to_ascii_lowercase())?;
        match &self.entries[i].content {
            Content::Memory(b) => Some(b.clone()),
            #[cfg(not(target_arch = "wasm32"))]
            Content::Disk(p) => std::fs::read(p).ok(),
        }
    }
}

/// Files in memory.
#[derive(Default)]
pub struct MemorySource(Index);

impl MemorySource {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a file; a file with the same path replaces the earlier one (a higher layer), and a
    /// later file with the same bare name wins name lookups.
    pub fn add(&mut self, path: &str, bytes: impl Into<Vec<u8>>) -> &mut Self {
        self.0.add(path, Content::Memory(bytes.into()));
        self
    }
}

impl IncludeSource for MemorySource {
    fn resolve(&self, name: &str) -> Option<String> {
        self.0.resolve(name)
    }
    fn list(&self) -> Vec<String> {
        self.0.list()
    }
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.0.read(path)
    }
}

/// `.lua` files of directory trees, read on demand. Layers are given from the lowest priority
/// to the highest; paths are relative to their layer's root. Not available on wasm.
#[cfg(not(target_arch = "wasm32"))]
pub struct DirSource(Index);

#[cfg(not(target_arch = "wasm32"))]
impl DirSource {
    pub fn new<P: AsRef<std::path::Path>>(layers: &[P]) -> std::io::Result<Self> {
        let mut index = Index::default();
        for root in layers {
            let root = root.as_ref();
            let mut files = Vec::new();
            walk(root, root, &mut files)?;
            files.sort();
            for (rel, abs) in files {
                index.add(&rel, Content::Disk(abs));
            }
        }
        Ok(DirSource(index))
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn walk(
    root: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<(String, std::path::PathBuf)>,
) -> std::io::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        if p.is_dir() {
            walk(root, &p, out)?;
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lua")) {
            let rel = p
                .strip_prefix(root)
                .unwrap_or(&p)
                .to_string_lossy()
                .into_owned();
            out.push((rel, p));
        }
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
impl IncludeSource for DirSource {
    fn resolve(&self, name: &str) -> Option<String> {
        self.0.resolve(name)
    }
    fn list(&self) -> Vec<String> {
        self.0.list()
    }
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.0.read(path)
    }
}
