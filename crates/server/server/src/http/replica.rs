//! `GET /replica/{hash:016x}`: the body of a replica snapshot or patch (`wire::replica`,
//! `C5RB`) exactly as the worker packed it. The hash in the URL is the xxh64 of the body, so the answer never
//! changes and is cached forever, like a catalog. The bodies are held in memory for the last few turns of a
//! game (`game::replica::ReplicaBlobs`); one that is gone is a 404, and the client asks for a new base.

use std::sync::Arc;

use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::Shared;

/// Exactly 16 lowercase hex digits.
fn parse_hash(s: &str) -> Option<u64> {
    (s.len() == 16 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
        .then(|| u64::from_str_radix(s, 16).ok())
        .flatten()
}

pub async fn get(State(shared): State<Arc<Shared>>, UrlPath(hash): UrlPath<String>) -> Response {
    let t0 = std::time::Instant::now();
    let Some(body) = parse_hash(&hash).and_then(|h| shared.replica.get(h)) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // `g.blob_serve`: the body goes out of memory (the transfer to the client is the socket's).
    if let Some(h) = parse_hash(&hash)
        && let Some(game) = shared.replica.game_of(h).and_then(|g| shared.games.get(g))
        && let Some(turn) = game.info.trace.blob_turn(h)
    {
        let dur = u64::try_from(t0.elapsed().as_micros()).unwrap_or(0);
        let end = game.info.trace.now_us();
        game.info.trace.record(
            turn,
            "g.blob_serve",
            "http",
            end.saturating_sub(dur),
            dur,
            &[
                ("hash", i64::from_ne_bytes(h.to_ne_bytes())),
                ("bytes", i64::try_from(body.len()).unwrap_or(0)),
            ],
        );
    }
    let resp = (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            ),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=31536000, immutable"),
            ),
        ],
        body,
    )
        .into_response();
    // Under `cross_origin_isolation` the page is COEP `require-corp`: the body says it is same-origin.
    super::static_files::isolate(&shared, resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_syntax_is_strict() {
        assert_eq!(parse_hash("00000000000000ff"), Some(255));
        assert_eq!(parse_hash("ff"), None);
        assert_eq!(parse_hash("00000000000000FF"), None);
        assert_eq!(parse_hash("../../../etc/passwd"), None);
    }
}
