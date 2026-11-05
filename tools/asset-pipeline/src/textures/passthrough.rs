//! World textures keep the bytes of the original: DXT blocks as
//! they are, L8 as `R8`, RGB32A as `Rgba8`, no re-encoding, no sRGB, mips as the file has them.

use asset_format::{TexDim, TextureBlob, TextureFormat, VariantFormat};

use super::{TextureError, read_dds};

/// A DDS file as a blob of the same pixels. The mip chain is the one of the file, also when the
/// file has none ("no mips" stays so: the original does not have them either).
pub fn dds_passthrough(bytes: &[u8]) -> Result<TextureBlob, TextureError> {
    read_dds(bytes)?.into_blob()
}

/// Keeps level 0 only. For the small data textures of the table in 4.2 ("Малые данные"): ramps
/// and colour tables the shader reads without mip filtering. Cubes and volumes are left alone.
pub fn level0_only(mut blob: TextureBlob) -> TextureBlob {
    if blob.dim == TexDim::D2 {
        blob.mips.truncate(1);
    }
    blob
}

/// The manifest variant a blob is filed under (last bullet): the block
/// formats under `Bc`, the plain ones under `Uncompressed`.
pub fn variant_format(format: TextureFormat) -> VariantFormat {
    match format {
        TextureFormat::Bc1
        | TextureFormat::Bc2
        | TextureFormat::Bc3
        | TextureFormat::Bc4
        | TextureFormat::Bc5
        | TextureFormat::Bc7 => VariantFormat::Bc,
        TextureFormat::Astc4x4 => VariantFormat::Astc,
        TextureFormat::Rgba8
        | TextureFormat::R8
        | TextureFormat::Rg8
        | TextureFormat::R16F
        | TextureFormat::Rgba16F => VariantFormat::Uncompressed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ddsfile::{D3DFormat, Dds, NewD3dParams};

    fn dds(format: D3DFormat, w: u32, h: u32, levels: u32, cube: bool, fill: u8) -> Vec<u8> {
        let mut d = Dds::new_d3d(NewD3dParams {
            height: h,
            width: w,
            depth: None,
            format,
            mipmap_levels: (levels > 1).then_some(levels),
            caps2: cube.then_some(ddsfile::Caps2::CUBEMAP | ddsfile::Caps2::CUBEMAP_ALLFACES),
        })
        .unwrap();
        if cube {
            // ddsfile allocates one face for a new cubemap; the file has six.
            d.data.resize(d.data.len() * 6, 0);
        }
        for (i, b) in d.data.iter_mut().enumerate() {
            *b = fill.wrapping_add(i as u8);
        }
        let mut out = Vec::new();
        d.write(&mut out).unwrap();
        out
    }

    #[test]
    fn dxt_blocks_are_kept_byte_for_byte() {
        let file = dds(D3DFormat::DXT1, 16, 16, 5, false, 3);
        let src = Dds::read(&file[..]).unwrap();
        let t = dds_passthrough(&file).unwrap();
        assert_eq!(
            (t.format, t.dim, t.srgb),
            (TextureFormat::Bc1, TexDim::D2, false)
        );
        assert_eq!(t.mips.len(), 5);
        assert_eq!(t.mips.concat(), src.data);
    }

    #[test]
    fn dxt3_and_dxt5_map_to_bc2_and_bc3() {
        for (f, want) in [
            (D3DFormat::DXT3, TextureFormat::Bc2),
            (D3DFormat::DXT5, TextureFormat::Bc3),
        ] {
            let t = dds_passthrough(&dds(f, 8, 8, 4, false, 0)).unwrap();
            assert_eq!(t.format, want);
        }
    }

    #[test]
    fn no_mips_stays_no_mips() {
        let t = dds_passthrough(&dds(D3DFormat::DXT5, 64, 64, 1, false, 0)).unwrap();
        assert_eq!(t.mips.len(), 1);
    }

    #[test]
    fn cube_keeps_faces_and_all_levels() {
        // 32x32 DXT1 cube with 6 levels: 6 faces per level, face-major in the file.
        let file = dds(D3DFormat::DXT1, 32, 32, 6, true, 1);
        let t = dds_passthrough(&file).unwrap();
        assert_eq!(
            (t.dim, t.width, t.height, t.mips.len()),
            (TexDim::Cube, 32, 32, 6)
        );
        assert_eq!(t.mips[0].len(), 6 * TextureFormat::Bc1.mip_size(32, 32));
        assert_eq!(TextureBlob::decode(&t.encode()).unwrap(), t);
    }

    #[test]
    fn rgba_cube_is_swizzled_to_rgba8() {
        let file = dds(D3DFormat::A8R8G8B8, 4, 4, 1, true, 0);
        let t = dds_passthrough(&file).unwrap();
        assert_eq!((t.format, t.dim), (TextureFormat::Rgba8, TexDim::Cube));
        // File bytes B, G, R, A = 0, 1, 2, 3 become R, G, B, A = 2, 1, 0, 3.
        assert_eq!(&t.mips[0][..4], [2, 1, 0, 3]);
    }

    #[test]
    fn level0_only_cuts_2d_chains_and_spares_cubes() {
        let t = dds_passthrough(&dds(D3DFormat::A8R8G8B8, 8, 2, 4, false, 0)).unwrap();
        assert_eq!(level0_only(t).mips.len(), 1);
        let c = dds_passthrough(&dds(D3DFormat::DXT1, 8, 8, 4, true, 0)).unwrap();
        assert_eq!(level0_only(c).mips.len(), 4);
    }

    #[test]
    fn variant_formats() {
        assert_eq!(variant_format(TextureFormat::Bc1), VariantFormat::Bc);
        assert_eq!(
            variant_format(TextureFormat::Rgba8),
            VariantFormat::Uncompressed
        );
        assert_eq!(
            variant_format(TextureFormat::R8),
            VariantFormat::Uncompressed
        );
    }
}
