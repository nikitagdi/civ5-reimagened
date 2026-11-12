//! The skin packages already read in this session, so switching back to a skin is not a download and a parse
//! again: the last [`PACKAGE_CACHE`] packages used, the least recently used one goes first.

use std::sync::Arc;

use ui2_package::Package;

use crate::registry::hash_hex;

/// How many read packages are kept.
pub const PACKAGE_CACHE: usize = 3;

/// Read packages by skin id and hash, most recently used first.
#[derive(Default)]
pub struct PackageCache {
    entries: Vec<Arc<Package>>,
}

impl PackageCache {
    /// The package of skin `id` whose hash starts with `hash` (the index's hex prefix), if it is kept; it becomes
    /// the most recently used.
    pub fn get(&mut self, id: &str, hash: &str) -> Option<Arc<Package>> {
        let i = self
            .entries
            .iter()
            .position(|p| p.manifest().id == id && hash_hex(&p.hash()).starts_with(hash))?;
        let pkg = self.entries.remove(i);
        self.entries.insert(0, Arc::clone(&pkg));
        Some(pkg)
    }

    /// Keeps a package as the most recently used; another build of the same skin is replaced, and past
    /// [`PACKAGE_CACHE`] packages the least recently used goes.
    pub fn put(&mut self, pkg: Arc<Package>) {
        self.entries
            .retain(|p| p.manifest().id != pkg.manifest().id);
        self.entries.insert(0, pkg);
        self.entries.truncate(PACKAGE_CACHE);
    }

    /// The ids of the kept packages, most recently used first.
    pub fn ids(&self) -> Vec<String> {
        self.entries
            .iter()
            .map(|p| p.manifest().id.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use ui2_runtime::testkit::Kit;

    use super::*;

    fn pkg(id: &str) -> Arc<Package> {
        Kit::new().finish_skin(id).expect("package")
    }

    #[test]
    fn the_least_recently_used_goes_first() {
        let mut c = PackageCache::default();
        let a = pkg("a");
        let hash = hash_hex(&a.hash());
        c.put(a);
        c.put(pkg("b"));
        c.put(pkg("c"));
        assert!(c.get("a", &hash[..16]).is_some());
        assert!(c.get("a", "ffff").is_none(), "another build");
        c.put(pkg("d"));
        assert_eq!(c.ids(), ["d", "a", "c"]);
        c.put(pkg("c"));
        assert_eq!(c.ids(), ["c", "d", "a"], "one build per skin");
    }
}
