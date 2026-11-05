//! Reads the Lua files of a built `ui` set from disk (`civ5-assets build --set ui --out <dir>`): the
//! pointer file `manifest`, the manifest blob, the index `ui:index`, and the blob of every Lua file the
//! index lists (after the layering the build applied).

use std::path::Path;

use asset_format::{
    ContentHash, LogicalId, Manifest, PlatformProfile, UI_INDEX_ID, UiFileKind, UiIndex,
};

use super::Sources;

#[derive(Debug)]
pub struct SetError(pub String);

impl std::fmt::Display for SetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SetError {}

fn blob(dir: &Path, hash: &ContentHash) -> Result<Vec<u8>, SetError> {
    let path = dir.join(hash.to_string());
    std::fs::read(&path).map_err(|e| SetError(format!("{}: {e}", path.display())))
}

/// The Lua files of the set in `dir`, and the number of Lua files the index lists.
pub fn load_lua(dir: &Path) -> Result<(Sources, UiIndex), SetError> {
    let pointer = std::fs::read_to_string(dir.join("manifest")).map_err(|e| {
        SetError(format!(
            "{}: {e} (is this a built set?)",
            dir.join("manifest").display()
        ))
    })?;
    let hash: ContentHash = pointer
        .trim()
        .parse()
        .map_err(|e| SetError(format!("manifest pointer: {e}")))?;
    let manifest =
        Manifest::decode(&blob(dir, &hash)?).map_err(|e| SetError(format!("manifest: {e}")))?;
    let profile = PlatformProfile::uncompressed_only();
    let fetch = |id: &str| -> Result<Vec<u8>, SetError> {
        let id = LogicalId::new(id);
        let v = manifest
            .resolve(&id, &profile)
            .ok_or_else(|| SetError(format!("{} is not in the manifest", id.as_str())))?;
        blob(dir, &v.hash)
    };
    let index =
        UiIndex::decode(&fetch(UI_INDEX_ID)?).map_err(|e| SetError(format!("ui index: {e}")))?;
    let mut sources = Sources::new();
    // Files are sorted by bare name and each name is the visible copy of its layers already.
    for f in index.files.iter().filter(|f| f.kind == UiFileKind::Lua) {
        let bytes = fetch(&f.id)?;
        sources.add(&f.path, bytes);
    }
    Ok((sources, index))
}
