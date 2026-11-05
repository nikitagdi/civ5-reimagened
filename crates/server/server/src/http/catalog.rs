//! `GET /catalog/{hash:016x}`: the game catalog exactly as the worker sent it,
//! stored in `state_dir/catalog/<hash>.bin`. A catalog is immutable, so the answer is cached forever.
//! Before it goes out, `xxh64(body)` must equal the hash in the URL.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::Shared;
use crate::state::files::{self, FileMode};

/// Catalogs are small (a few hundred KiB at most); anything larger is not read.
const MAX_CATALOG: u64 = 16 * 1024 * 1024;

/// TODO(N1.8): replace by `protocol::hash::xxh64` when `proto` lands it. XXH64, seed 0.
pub fn xxh64(data: &[u8]) -> u64 {
    const P1: u64 = 0x9E37_79B1_85EB_CA87;
    const P2: u64 = 0xC2B2_AE3D_27D4_EB4F;
    const P3: u64 = 0x1656_67B1_9E37_79F9;
    const P4: u64 = 0x85EB_CA77_C2B2_AE63;
    const P5: u64 = 0x27D4_EB2F_1656_67C5;

    fn read64(b: &[u8]) -> u64 {
        u64::from_le_bytes(b[..8].try_into().expect("8 bytes"))
    }
    fn round(acc: u64, input: u64) -> u64 {
        acc.wrapping_add(input.wrapping_mul(P2))
            .rotate_left(31)
            .wrapping_mul(P1)
    }
    fn merge(acc: u64, v: u64) -> u64 {
        (acc ^ round(0, v)).wrapping_mul(P1).wrapping_add(P4)
    }

    let mut rest = data;
    let mut h = if data.len() >= 32 {
        let mut v1 = P1.wrapping_add(P2);
        let mut v2 = P2;
        let mut v3 = 0u64;
        let mut v4 = 0u64.wrapping_sub(P1);
        while rest.len() >= 32 {
            v1 = round(v1, read64(&rest[0..]));
            v2 = round(v2, read64(&rest[8..]));
            v3 = round(v3, read64(&rest[16..]));
            v4 = round(v4, read64(&rest[24..]));
            rest = &rest[32..];
        }
        let mut h = v1
            .rotate_left(1)
            .wrapping_add(v2.rotate_left(7))
            .wrapping_add(v3.rotate_left(12))
            .wrapping_add(v4.rotate_left(18));
        for v in [v1, v2, v3, v4] {
            h = merge(h, v);
        }
        h
    } else {
        P5
    };
    h = h.wrapping_add(data.len() as u64);
    while rest.len() >= 8 {
        h ^= round(0, read64(rest));
        h = h.rotate_left(27).wrapping_mul(P1).wrapping_add(P4);
        rest = &rest[8..];
    }
    if rest.len() >= 4 {
        let k = u32::from_le_bytes(rest[..4].try_into().expect("4 bytes"));
        h ^= u64::from(k).wrapping_mul(P1);
        h = h.rotate_left(23).wrapping_mul(P2).wrapping_add(P3);
        rest = &rest[4..];
    }
    for &b in rest {
        h ^= u64::from(b).wrapping_mul(P5);
        h = h.rotate_left(11).wrapping_mul(P1);
    }
    h ^= h >> 33;
    h = h.wrapping_mul(P2);
    h ^= h >> 29;
    h = h.wrapping_mul(P3);
    h ^ (h >> 32)
}

/// Exactly 16 lowercase hex digits.
fn parse_hash(s: &str) -> Option<u64> {
    (s.len() == 16 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
        .then(|| u64::from_str_radix(s, 16).ok())
        .flatten()
}

fn catalog_path(dir: &Path, hash: u64) -> PathBuf {
    dir.join(format!("{hash:016x}.bin"))
}

/// Stores a catalog under its hash and returns the hash. Idempotent.
pub fn store(catalog_dir: &Path, body: &[u8]) -> anyhow::Result<u64> {
    let hash = xxh64(body);
    let path = catalog_path(catalog_dir, hash);
    if !path.exists() {
        files::write_atomic(&path, body, FileMode::Normal)?;
    }
    Ok(hash)
}

pub async fn get(State(shared): State<Arc<Shared>>, UrlPath(hash): UrlPath<String>) -> Response {
    let (Some(hash), Some(state_dir)) = (parse_hash(&hash), shared.config.state_dir.clone()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let path = catalog_path(&state_dir.join("catalog"), hash);
    let read = tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let mut buf = Vec::new();
        std::fs::File::open(path)?
            .take(MAX_CATALOG + 1)
            .read_to_end(&mut buf)?;
        std::io::Result::Ok(buf)
    })
    .await;
    let Ok(Ok(body)) = read else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if body.len() as u64 > MAX_CATALOG || xxh64(&body) != hash {
        tracing::error!(
            hash = format!("{hash:016x}"),
            "catalog file does not match its hash"
        );
        return StatusCode::NOT_FOUND.into_response();
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
    fn xxh64_reference_vectors() {
        assert_eq!(xxh64(b""), 0xEF46_DB37_51D8_E999);
        assert_eq!(xxh64(b"a"), 0xD24E_C4F1_A98C_6E5B);
        assert_eq!(xxh64(b"abc"), 0x44BC_2CF5_AD77_0999);
    }

    #[test]
    fn hash_syntax_is_strict() {
        assert_eq!(parse_hash("00000000000000ff"), Some(255));
        assert_eq!(parse_hash("ff"), None);
        assert_eq!(parse_hash("00000000000000FF"), None);
        assert_eq!(parse_hash("../../../etc/passwd"), None);
        assert_eq!(parse_hash("+000000000000ff"), None);
    }

    #[test]
    fn store_is_content_addressed_and_idempotent() {
        let dir = std::env::temp_dir().join(format!("civ5-cat-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let h1 = store(&dir, b"catalog bytes").unwrap();
        let h2 = store(&dir, b"catalog bytes").unwrap();
        assert_eq!(h1, h2);
        assert_eq!(
            std::fs::read(catalog_path(&dir, h1)).unwrap(),
            b"catalog bytes"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
