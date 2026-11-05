//! The `wv3` files of `golden` scenes: the stream messages of a scene saved to disk, read by
//! `golden` (writes them) and by the client (`client-app/src/golden.rs`, draws a frame from them without a session).
//!
//! `mapbasis.wv3` and `world.wv3` are the magic `WV3` + format byte, then one `GameBatch` (postcard, the codec of the
//! game stream) with a single message: `MapBasis`, and `Keyframe` (with `basis_hash`, without the basis). The client
//! reads the basis first and the keyframe second, the order of `Basis` before `Full`. `catalog.wv3` is the magic
//! `WVC` + format byte, then the `Catalog` the worker sent (postcard).

use crate::{Catalog, Codec, GameBatch, GameMsg, Keyframe, MapBasis, Postcard};

/// `WV3` and the format version.
pub const MAGIC: [u8; 4] = *b"WV3\x01";
/// `WVC` and the format version.
pub const CATALOG_MAGIC: [u8; 4] = *b"WVC\x01";

fn encode(msg: GameMsg) -> Result<Vec<u8>, String> {
    let batch = GameBatch {
        seq: 1,
        msgs: vec![msg],
    };
    let mut out = MAGIC.to_vec();
    out.extend(
        Postcard
            .encode(&batch)
            .map_err(|e| format!("encode the wv3 batch: {e}"))?,
    );
    Ok(out)
}

fn decode(bytes: &[u8]) -> Result<GameMsg, String> {
    let Some(body) = bytes.strip_prefix(&MAGIC) else {
        return Err("not a wv3 file (bad magic)".into());
    };
    let batch: GameBatch = Postcard
        .decode(body)
        .map_err(|e| format!("decode the wv3 batch: {e}"))?;
    let [msg] = <[GameMsg; 1]>::try_from(batch.msgs)
        .map_err(|m| format!("a wv3 file holds one message, found {}", m.len()))?;
    Ok(msg)
}

/// The bytes of `mapbasis.wv3`.
pub fn encode_basis(basis: &MapBasis) -> Result<Vec<u8>, String> {
    encode(GameMsg::MapBasis(basis.clone()))
}

/// The map basis of a `mapbasis.wv3`.
pub fn decode_basis(bytes: &[u8]) -> Result<MapBasis, String> {
    match decode(bytes)? {
        GameMsg::MapBasis(b) => Ok(b),
        other => Err(format!("mapbasis.wv3 holds {other:?}")),
    }
}

/// The bytes of `world.wv3`.
pub fn encode_world(frame: &Keyframe) -> Result<Vec<u8>, String> {
    encode(GameMsg::Keyframe(frame.clone()))
}

/// The keyframe of a `world.wv3`.
pub fn decode_world(bytes: &[u8]) -> Result<Keyframe, String> {
    match decode(bytes)? {
        GameMsg::Keyframe(k) => Ok(k),
        other => Err(format!("world.wv3 holds {other:?}")),
    }
}

/// The bytes of `catalog.wv3` from the postcard bytes of the worker's `Catalog` message.
pub fn encode_catalog_bytes(postcard: &[u8]) -> Vec<u8> {
    let mut out = CATALOG_MAGIC.to_vec();
    out.extend_from_slice(postcard);
    out
}

/// The bytes of `catalog.wv3`.
pub fn encode_catalog(catalog: &Catalog) -> Result<Vec<u8>, String> {
    Ok(encode_catalog_bytes(
        &Postcard
            .encode(catalog)
            .map_err(|e| format!("encode the catalog: {e}"))?,
    ))
}

/// The catalog of a `catalog.wv3`.
pub fn decode_catalog(bytes: &[u8]) -> Result<Catalog, String> {
    let Some(body) = bytes.strip_prefix(&CATALOG_MAGIC) else {
        return Err("not a catalog.wv3 file (bad magic)".into());
    };
    Postcard
        .decode(body)
        .map_err(|e| format!("decode the catalog: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_round_trip_and_refuse_each_other() {
        let basis = MapBasis {
            game: 3,
            basis_hash: 77,
            width: 2,
            height: 1,
            plots: vec![Default::default(), Default::default()],
            ..MapBasis::default()
        };
        let bytes = encode_basis(&basis).unwrap();
        assert_eq!(decode_basis(&bytes).unwrap(), basis);
        assert!(decode_world(&bytes).is_err(), "a basis is not a keyframe");
        assert!(decode_basis(b"nope").is_err());

        let catalog = Catalog::default();
        let bytes = encode_catalog(&catalog).unwrap();
        assert_eq!(decode_catalog(&bytes).unwrap(), catalog);
        assert!(decode_catalog(&encode_basis(&basis).unwrap()).is_err());
    }
}
