//! `mapbasis.wv3` and `world.wv3`: the two stream messages of a scene, as the game
//! stream (net-protocol 4.4) carries them, and `catalog.wv3`, the worker's catalog. The format and its decoder
//! are in `protocol::wv3`, shared with the client loader (`client-app/src/golden.rs`); this is the `anyhow`
//! face of it for the tool.

use anyhow::{Context, Result};
use protocol::{Catalog, Keyframe, MapBasis, wv3};

pub use wv3::MAGIC;

/// The bytes of `mapbasis.wv3`.
pub fn encode_basis(basis: &MapBasis) -> Result<Vec<u8>> {
    wv3::encode_basis(basis).map_err(anyhow::Error::msg)
}

/// The map basis of a `mapbasis.wv3`.
pub fn decode_basis(bytes: &[u8]) -> Result<MapBasis> {
    wv3::decode_basis(bytes)
        .map_err(anyhow::Error::msg)
        .context("mapbasis.wv3")
}

/// The bytes of `world.wv3`.
pub fn encode_world(frame: &Keyframe) -> Result<Vec<u8>> {
    wv3::encode_world(frame).map_err(anyhow::Error::msg)
}

/// The keyframe of a `world.wv3`.
pub fn decode_world(bytes: &[u8]) -> Result<Keyframe> {
    wv3::decode_world(bytes)
        .map_err(anyhow::Error::msg)
        .context("world.wv3")
}

/// The bytes of `catalog.wv3` from the postcard bytes of the worker's `Catalog` message.
pub fn encode_catalog_bytes(postcard: &[u8]) -> Vec<u8> {
    wv3::encode_catalog_bytes(postcard)
}

/// The bytes of `catalog.wv3`.
pub fn encode_catalog(catalog: &Catalog) -> Result<Vec<u8>> {
    wv3::encode_catalog(catalog).map_err(anyhow::Error::msg)
}

/// The catalog of a `catalog.wv3`.
pub fn decode_catalog(bytes: &[u8]) -> Result<Catalog> {
    wv3::decode_catalog(bytes).map_err(anyhow::Error::msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{PlotBasisView, PlotType, TurnStatus, WorldView};

    fn basis() -> MapBasis {
        MapBasis {
            game: 3,
            basis_hash: 77,
            width: 2,
            height: 1,
            wrap_x: true,
            wrap_y: false,
            terrain_seed: 12345,
            plots: vec![
                PlotBasisView::default(),
                PlotBasisView {
                    plot_type: PlotType::Hill,
                    terrain: 4,
                    feature: -1,
                    river_flow: [-1; 3],
                    ..PlotBasisView::default()
                },
            ],
        }
    }

    fn keyframe() -> Keyframe {
        Keyframe {
            view: WorldView {
                turn: 0,
                year: -4000,
                human: 0,
                human_team: 0,
                status: TurnStatus::HumanTurn,
                blocker: None,
                map: Default::default(),
                units: vec![],
                cities: vec![],
                players: vec![],
                top_bar: Default::default(),
                notifications: vec![],
                peace_offers: vec![],
                trade_routes: vec![],
            },
            hash: 5,
            basis_hash: 77,
        }
    }

    #[test]
    fn round_trip() {
        let b = basis();
        assert_eq!(decode_basis(&encode_basis(&b).unwrap()).unwrap(), b);
        let k = keyframe();
        assert_eq!(decode_world(&encode_world(&k).unwrap()).unwrap(), k);
        let c = Catalog::default();
        assert_eq!(decode_catalog(&encode_catalog(&c).unwrap()).unwrap(), c);
    }

    #[test]
    fn bad_files_are_refused() {
        assert!(decode_basis(b"XX").is_err());
        assert!(decode_basis(&encode_world(&keyframe()).unwrap()).is_err());
        assert!(decode_world(&encode_basis(&basis()).unwrap()).is_err());
        let mut bytes = encode_basis(&basis()).unwrap();
        bytes[0] = b'X';
        assert!(decode_basis(&bytes).is_err());
        assert!(decode_catalog(&encode_basis(&basis()).unwrap()).is_err());
    }
}
