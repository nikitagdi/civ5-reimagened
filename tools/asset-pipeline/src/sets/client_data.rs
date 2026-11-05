//! Set `client-data`: the small data blobs the client reads once at start, made from the user's own
//! install (nothing of it is in the repository):
//!
//! - `data:cursors` ([`asset_format::CursorSet`]): the animated cursors of `Assets/UI/Cursors`
//!   ([`crate::cursors`]); without it the client shows the browser's default cursor;
//! - `data:dge-graphs` (text): the unit blend graphs of `Resource/Common/UnitModels.fpk`
//!   ([`crate::anim::dge`]); without it the units play the clips of their action.
//!
//! Part of the composite `b2`.

use std::path::Path;
use std::time::Instant;

use asset_format::{CURSORS_ID, DGE_GRAPHS_ID, LogicalId, PrefetchGroup, Variant, VariantFormat};

use crate::anim::dge::{UNIT_GRAPH_PACK, graphs_table};
use crate::cursors::build_cursor_set;
use crate::game::GameSource;
use crate::models::ModelError;
use crate::store::{BlobStore, ManifestBuilder};

pub const SET_NAME: &str = "client-data";

/// Fetched by id at start, not by the group table.
const PREFETCH: PrefetchGroup = PrefetchGroup(0);

#[derive(Debug, Default)]
pub struct ClientDataSummary {
    pub cursors: usize,
    pub cursor_bytes: u64,
    pub graphs: usize,
    pub graph_bytes: u64,
    /// `(what, why)` of the cursors and graphs left out.
    pub failed: Vec<(String, String)>,
    pub seconds: f64,
}

/// The `.dge` files of the unit models pack: `(file name, bytes)`.
fn unit_graphs(game: &GameSource) -> Result<Vec<(String, Vec<u8>)>, ModelError> {
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let all = game.iter_ext("dge").chain(
        game.shadowed()
            .iter()
            .filter(|e| e.extension().eq_ignore_ascii_case("dge")),
    );
    for e in all {
        let from_pack = game
            .origin_path(e)
            .and_then(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().to_ascii_lowercase())
            })
            .is_some_and(|n| n == UNIT_GRAPH_PACK);
        if from_pack && seen.insert(e.name.clone()) {
            out.push((e.name.clone(), game.read(e)?));
        }
    }
    Ok(out)
}

/// Builds the set into `out`.
pub fn build_client_data(game: &GameSource, out: &Path) -> Result<ClientDataSummary, ModelError> {
    let started = Instant::now();
    let store = BlobStore::open(out)?;
    let mut manifest = ManifestBuilder::open(&store)?;
    let mut s = ClientDataSummary::default();
    let mut add = |id: &str, bytes: &[u8]| -> Result<u64, ModelError> {
        let (hash, size) = store.put(bytes)?;
        manifest.add(
            LogicalId::new(id),
            Variant {
                format: VariantFormat::Generic,
                hash,
                size,
                prefetch: PREFETCH,
            },
        );
        Ok(size)
    };

    let (cursors, failed) = build_cursor_set(|stem| {
        game.lookup(&format!("{stem}.ani"))
            .and_then(|e| game.read(e).ok())
    });
    s.failed.extend(
        failed
            .into_iter()
            .map(|(n, why)| (format!("cursor {n}"), why)),
    );
    if !cursors.cursors.is_empty() {
        s.cursors = cursors.cursors.len();
        s.cursor_bytes = add(CURSORS_ID, &cursors.encode())?;
    }

    let graphs = unit_graphs(game)?;
    let (text, n, failed) = graphs_table(&graphs);
    s.failed.extend(
        failed
            .into_iter()
            .map(|(n, why)| (format!("graph {n}"), why)),
    );
    if n > 0 {
        s.graphs = n;
        s.graph_bytes = add(DGE_GRAPHS_ID, text.as_bytes())?;
    }
    manifest.finish(&store)?;
    s.seconds = started.elapsed().as_secs_f64();
    Ok(s)
}
