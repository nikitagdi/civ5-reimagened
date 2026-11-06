//! `GET /mods/{hash}`: the asset pack of an installed mod, by the SHA-256 of the pack file (T32). The hash comes to
//! the client in `ModView.pack`; the client verifies the bytes against it, so the route needs no authentication
//! (a pack is public like any asset blob). Only packs the store found at scan time are served: the path is never
//! built from the URL, so there is nothing to traverse.

use std::sync::Arc;

use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

use super::static_files::isolate;
use crate::Shared;

/// A pack larger than this is not served (and not advertised by the store).
pub const MAX_PACK_BYTES: u64 = mod_host_server::MAX_PACK_BYTES;

pub async fn get(State(shared): State<Arc<Shared>>, UrlPath(hash): UrlPath<String>) -> Response {
    // 64 hex digits of SHA-256, nothing else.
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(info) = shared.mod_store.pack_by_hash(&hash).cloned() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let read = tokio::task::spawn_blocking(move || mod_host_server::read_pack(&info)).await;
    let bytes = match read {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "a mod pack cannot be served");
            return StatusCode::NOT_FOUND.into_response();
        }
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let mut resp = ([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response();
    resp.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    isolate(&shared, resp)
}
