//! The body of `GET /catalog/{hash:016x}` (net-protocol NP-D14): `C5CT`, a `u16` version (little-endian), then
//! the postcard `Catalog`. The hash of the body is its xxh64.

/// First bytes of the body.
pub const CATALOG_MAGIC: [u8; 4] = *b"C5CT";

/// The version of the body's layout. The server writes it, the client refuses any other. A change of the
/// `Catalog` layout bumps it together with the protocol version.
pub const CATALOG_VERSION: u16 = 1;

/// Length of the header: the magic and the version.
pub const CATALOG_HEADER_LEN: usize = 6;

/// Why a body is not a catalog of this version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CatalogHeaderError {
    #[error("the body is not a catalog")]
    NotACatalog,
    #[error("catalog version {0}, this client reads {CATALOG_VERSION}")]
    Version(u16),
}

/// The header of a catalog body.
pub fn catalog_header() -> [u8; CATALOG_HEADER_LEN] {
    let mut h = [0u8; CATALOG_HEADER_LEN];
    h[..4].copy_from_slice(&CATALOG_MAGIC);
    h[4..].copy_from_slice(&CATALOG_VERSION.to_le_bytes());
    h
}

/// Checks the magic and the version; returns the postcard part.
pub fn strip_catalog_header(body: &[u8]) -> Result<&[u8], CatalogHeaderError> {
    if body.len() < CATALOG_HEADER_LEN || body[..4] != CATALOG_MAGIC {
        return Err(CatalogHeaderError::NotACatalog);
    }
    let version = u16::from_le_bytes([body[4], body[5]]);
    if version != CATALOG_VERSION {
        return Err(CatalogHeaderError::Version(version));
    }
    Ok(&body[CATALOG_HEADER_LEN..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_round_trips() {
        let mut body = catalog_header().to_vec();
        body.extend_from_slice(b"rest");
        assert_eq!(strip_catalog_header(&body), Ok(&b"rest"[..]));
    }

    #[test]
    fn the_version_is_little_endian() {
        assert_eq!(catalog_header()[4..], [1, 0]);
        let mut body = b"C5CT".to_vec();
        body.extend_from_slice(&1u16.to_be_bytes());
        assert_eq!(
            strip_catalog_header(&body),
            Err(CatalogHeaderError::Version(256))
        );
    }

    #[test]
    fn short_or_foreign_bodies_are_refused() {
        assert_eq!(
            strip_catalog_header(b"C5C"),
            Err(CatalogHeaderError::NotACatalog)
        );
        assert_eq!(
            strip_catalog_header(b"C5CT\x01"),
            Err(CatalogHeaderError::NotACatalog)
        );
        assert_eq!(
            strip_catalog_header(b"XXXX\x01\x00"),
            Err(CatalogHeaderError::NotACatalog)
        );
        assert_eq!(strip_catalog_header(b"C5CT\x01\x00"), Ok(&[][..]));
    }
}
