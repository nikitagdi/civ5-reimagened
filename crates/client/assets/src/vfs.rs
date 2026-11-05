//! Civ5 logical path VFS with overlays.

use std::collections::HashMap;

use crate::{AssetError, LogicalId};

/// Overlay id; precedence comes from [`Vfs::add_overlay`]'s priority, not from the id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VfsOverlayId(pub u32);

#[derive(Debug)]
struct Overlay {
    id: VfsOverlayId,
    priority: i32,
    /// On equal priority, the later-added overlay wins.
    order: u64,
    files: HashMap<String, LogicalId>,
}

/// Civ5 path → [`LogicalId`]. Paths are case-insensitive, and higher-priority overlays shadow lower ones.
#[derive(Debug, Default)]
pub struct Vfs {
    /// Sorted by descending `(priority, order)`; first match wins.
    overlays: Vec<Overlay>,
    next_order: u64,
}

impl Vfs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Re-adding an existing `id` only updates its priority.
    pub fn add_overlay(&mut self, id: VfsOverlayId, priority: i32) {
        if let Some(o) = self.overlays.iter_mut().find(|o| o.id == id) {
            o.priority = priority;
        } else {
            let order = self.next_order;
            self.next_order += 1;
            self.overlays.push(Overlay {
                id,
                priority,
                order,
                files: HashMap::new(),
            });
        }
        self.overlays
            .sort_by_key(|o| std::cmp::Reverse((o.priority, o.order)));
    }

    pub fn has_overlay(&self, id: VfsOverlayId) -> bool {
        self.overlays.iter().any(|o| o.id == id)
    }

    /// Returns the id it replaced in that overlay, if any.
    pub fn insert(
        &mut self,
        overlay: VfsOverlayId,
        path: &str,
        id: LogicalId,
    ) -> Result<Option<LogicalId>, AssetError> {
        let o = self
            .overlays
            .iter_mut()
            .find(|o| o.id == overlay)
            .ok_or(AssetError::UnknownOverlay(overlay))?;
        Ok(o.files.insert(normalize_path(path), id))
    }

    pub fn resolve(&self, path: &str) -> Option<LogicalId> {
        let key = normalize_path(path);
        self.overlays
            .iter()
            .find_map(|o| o.files.get(&key))
            .cloned()
    }

    /// Only within `active` overlays.
    pub fn resolve_in(&self, path: &str, active: &[VfsOverlayId]) -> Option<LogicalId> {
        let key = normalize_path(path);
        self.overlays
            .iter()
            .filter(|o| active.contains(&o.id))
            .find_map(|o| o.files.get(&key))
            .cloned()
    }

    /// Includes shadowed entries.
    pub fn file_count(&self) -> usize {
        self.overlays.iter().map(|o| o.files.len()).sum()
    }
}

/// Civ5 path → VFS key: `\` to `/`, lowercase, `..` never climbs above the root.
pub fn normalize_path(path: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    for seg in path.split(['/', '\\']) {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s.to_lowercase()),
        }
    }
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: VfsOverlayId = VfsOverlayId(0);
    const DLC: VfsOverlayId = VfsOverlayId(1);
    const MOD: VfsOverlayId = VfsOverlayId(2);

    fn lid(s: &str) -> LogicalId {
        LogicalId::new(s)
    }

    #[test]
    fn normalizes_separators_and_case() {
        assert_eq!(
            normalize_path("Assets\\UI\\InGame\\TopPanel.lua"),
            "assets/ui/ingame/toppanel.lua"
        );
        assert_eq!(normalize_path("/a//b/./c/../D"), "a/b/d");
        assert_eq!(normalize_path("../x"), "x");
    }

    #[test]
    fn case_insensitive_resolve() {
        let mut vfs = Vfs::new();
        vfs.add_overlay(BASE, 0);
        vfs.insert(BASE, "Assets/UI/TopPanel.lua", lid("ui.toppanel"))
            .unwrap();
        assert_eq!(
            vfs.resolve("assets\\ui\\TOPPANEL.LUA"),
            Some(lid("ui.toppanel"))
        );
        assert_eq!(vfs.resolve("assets/ui/other.lua"), None);
    }

    #[test]
    fn overlay_priority_wins_regardless_of_insert_order() {
        let mut vfs = Vfs::new();
        vfs.add_overlay(MOD, 20);
        vfs.add_overlay(BASE, 0);
        vfs.add_overlay(DLC, 10);
        vfs.insert(BASE, "a.xml", lid("base")).unwrap();
        vfs.insert(DLC, "A.XML", lid("dlc")).unwrap();
        vfs.insert(BASE, "b.xml", lid("base-b")).unwrap();
        vfs.insert(DLC, "b.xml", lid("dlc-b")).unwrap();
        vfs.insert(MOD, "B.xml", lid("mod-b")).unwrap();
        assert_eq!(vfs.resolve("a.xml"), Some(lid("dlc")));
        assert_eq!(vfs.resolve("b.xml"), Some(lid("mod-b")));
    }

    #[test]
    fn equal_priority_later_overlay_wins() {
        let mut vfs = Vfs::new();
        vfs.add_overlay(BASE, 5);
        vfs.add_overlay(DLC, 5);
        vfs.insert(BASE, "x", lid("base")).unwrap();
        vfs.insert(DLC, "x", lid("dlc")).unwrap();
        assert_eq!(vfs.resolve("x"), Some(lid("dlc")));
    }

    #[test]
    fn resolve_in_restricts_overlays() {
        let mut vfs = Vfs::new();
        vfs.add_overlay(BASE, 0);
        vfs.add_overlay(MOD, 20);
        vfs.insert(BASE, "x", lid("base")).unwrap();
        vfs.insert(MOD, "x", lid("mod")).unwrap();
        assert_eq!(vfs.resolve_in("x", &[BASE]), Some(lid("base")));
        assert_eq!(vfs.resolve_in("x", &[BASE, MOD]), Some(lid("mod")));
        assert_eq!(vfs.resolve_in("x", &[]), None);
    }

    #[test]
    fn insert_into_unknown_overlay_fails() {
        let mut vfs = Vfs::new();
        assert!(matches!(
            vfs.insert(BASE, "x", lid("y")),
            Err(AssetError::UnknownOverlay(_))
        ));
    }
}
