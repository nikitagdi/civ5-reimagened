//! DX11 shader blocks: the DXBC container, its signatures and declarations, the text of the
//! instructions, the twins of the SM3 blocks and the map of the constants (SR-D2,
//! SR-D8).
//!
//! Only the container and the declarations are decoded here (SR-D2): chunks, `ISGN`/`OSGN`/`PCSG`,
//! the program version and stage, `dcl_*`. The instruction text is the one of `D3DDisassemble`
//! (`asm_text`, Windows); the twins (`twins`) and the constant map (`cbmap`) are built on both.

pub mod asm_text;
pub mod cbmap;
pub mod chunks;
pub mod decl;
pub mod twins;

use crate::scan::Stage;
use chunks::{Chunk, Signature};
use decl::{Decls, Program};

#[derive(Debug, thiserror::Error)]
pub enum DxbcError {
    #[error("not a DXBC container")]
    NotDxbc,
    #[error("DXBC: truncated {0}")]
    Truncated(&'static str),
    #[error("DXBC: {0}")]
    Container(String),
    #[error("DXBC {chunk}: {why}")]
    Signature { chunk: &'static str, why: String },
    #[error("DXBC program: {0}")]
    Program(String),
    #[error("DXBC: no {0} chunk")]
    Missing(&'static str),
    #[error("DX11 text: {0}")]
    Text(String),
    #[error("{0}")]
    Other(String),
}

/// A parsed DX11 block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dxbc {
    /// Size of the container, from its header.
    pub size: usize,
    pub chunks: Vec<Chunk>,
    pub program: Program,
    pub isgn: Signature,
    pub osgn: Signature,
    /// Patch constants of a hull or domain shader.
    pub pcsg: Option<Signature>,
}

impl Dxbc {
    pub fn parse(d: &[u8]) -> Result<Dxbc, DxbcError> {
        let (size, chunks) = chunks::read_chunks(d)?;
        let find = |f: [u8; 4]| chunks.iter().find(|c| c.fourcc == f);
        let program_chunk = find(chunks::SHDR)
            .or_else(|| find(chunks::SHEX))
            .ok_or(DxbcError::Missing("SHDR/SHEX"))?;
        let program = decl::parse_program(program_chunk.payload(d))?;
        let sig = |f: [u8; 4], name: &'static str| -> Result<Option<Signature>, DxbcError> {
            find(f)
                .map(|c| Signature::parse(c.payload(d), name))
                .transpose()
        };
        // Compute shaders have no signatures; every other stage has both.
        let isgn = sig(chunks::ISGN, "ISGN")?;
        let osgn = sig(chunks::OSGN, "OSGN")?;
        if program.stage != Stage::Cs && (isgn.is_none() || osgn.is_none()) {
            return Err(DxbcError::Missing("ISGN/OSGN"));
        }
        Ok(Dxbc {
            size,
            chunks: chunks.clone(),
            program,
            isgn: isgn.unwrap_or_default(),
            osgn: osgn.unwrap_or_default(),
            pcsg: sig(chunks::PCSG, "PCSG")?,
        })
    }

    pub fn stage(&self) -> Stage {
        self.program.stage
    }

    /// `ps_5_0`, ...
    pub fn profile(&self) -> String {
        self.program.profile()
    }

    pub fn decls(&self) -> &Decls {
        &self.program.decls
    }

    /// FourCCs of the chunks, in file order.
    pub fn chunk_names(&self) -> Vec<String> {
        self.chunks.iter().map(Chunk::name).collect()
    }
}

#[cfg(test)]
pub(crate) mod synth {
    //! Synthetic containers for tests.
    use super::chunks::{HEADER_SIZE, MAGIC};

    pub fn container(chunks: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let table_end = HEADER_SIZE + chunks.len() * 4;
        let mut body = Vec::new();
        let mut offs = Vec::new();
        for (cc, payload) in chunks {
            offs.push((table_end + body.len()) as u32);
            body.extend_from_slice(*cc);
            body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            body.extend_from_slice(payload);
        }
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&[0xAB; 16]);
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&((table_end + body.len()) as u32).to_le_bytes());
        out.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
        for o in offs {
            out.extend_from_slice(&o.to_le_bytes());
        }
        out.extend_from_slice(&body);
        out
    }

    /// Signature payload with these `(name, index, sysval, register, mask)` elements.
    pub fn sig(els: &[(&str, u32, u32, u32, u8)]) -> Vec<u8> {
        let mut head = Vec::new();
        head.extend_from_slice(&(els.len() as u32).to_le_bytes());
        head.extend_from_slice(&8u32.to_le_bytes());
        let mut names = Vec::new();
        let names_at = 8 + els.len() * 24;
        for (name, index, sv, reg, mask) in els {
            head.extend_from_slice(&((names_at + names.len()) as u32).to_le_bytes());
            head.extend_from_slice(&index.to_le_bytes());
            head.extend_from_slice(&sv.to_le_bytes());
            head.extend_from_slice(&3u32.to_le_bytes());
            head.extend_from_slice(&reg.to_le_bytes());
            head.extend_from_slice(&[*mask, *mask, 0, 0]);
            names.extend_from_slice(name.as_bytes());
            names.push(0);
        }
        head.extend_from_slice(&names);
        head
    }
}

#[cfg(test)]
mod tests {
    use super::decl::synth::*;
    use super::synth::*;
    use super::*;

    fn ps_block() -> Vec<u8> {
        let mut body = Vec::new();
        body.extend(dcl_cb(0, 3, false));
        body.extend(dcl_sampler(0, 0));
        body.extend(dcl_resource(3, 0));
        body.extend(dcl_input_ps(1, 3));
        body.extend(dcl_output(0, 15));
        body.extend(sample());
        body.extend([op(62, 0, 1)]);
        container(&[
            (
                b"ISGN",
                sig(&[("SV_Position", 0, 1, 0, 15), ("TEXCOORD", 0, 0, 1, 3)]),
            ),
            (b"OSGN", sig(&[("SV_Target", 0, 64, 0, 15)])),
            (b"SHEX", program(0, 5, 0, &body)),
        ])
    }

    #[test]
    fn a_container_is_parsed() {
        let d = Dxbc::parse(&ps_block()).unwrap();
        assert_eq!(d.profile(), "ps_5_0");
        assert_eq!(d.stage(), Stage::Ps);
        assert_eq!(d.chunk_names(), ["ISGN", "OSGN", "SHEX"]);
        assert_eq!(d.isgn.elements.len(), 2);
        assert_eq!(d.osgn.elements[0].name, "SV_Target");
        assert!(d.pcsg.is_none());
        let x = d.decls();
        assert_eq!(x.cbuffers.len(), 1);
        assert_eq!(x.resources.len(), 1);
        assert_eq!(x.samplers.len(), 1);
        assert_eq!(x.inputs.len(), 1);
        assert_eq!(x.instructions, 2);
        assert_eq!(x.fetches, 1);
    }

    #[test]
    fn missing_pieces_are_errors() {
        // No program.
        let b = container(&[(b"ISGN", sig(&[])), (b"OSGN", sig(&[]))]);
        assert!(matches!(Dxbc::parse(&b), Err(DxbcError::Missing(_))));
        // No signatures on a pixel shader.
        let b = container(&[(b"SHDR", program(0, 4, 0, &[op(62, 0, 1)]))]);
        assert!(matches!(Dxbc::parse(&b), Err(DxbcError::Missing(_))));
        // A compute shader needs none.
        let b = container(&[(b"SHEX", program(5, 5, 0, &[op(62, 0, 1)]))]);
        assert_eq!(Dxbc::parse(&b).unwrap().stage(), Stage::Cs);
        // Truncated.
        let b = ps_block();
        assert!(Dxbc::parse(&b[..b.len() - 3]).is_err());
        assert!(Dxbc::parse(b"nothing").is_err());
    }

    #[test]
    fn hull_shader_has_patch_constants() {
        let b = container(&[
            (b"ISGN", sig(&[("TEXCOORD", 0, 0, 0, 15)])),
            (b"OSGN", sig(&[("TEXCOORD", 0, 0, 0, 15)])),
            (b"PCSG", sig(&[("SV_TessFactor", 0, 11, 0, 1)])),
            (b"SHEX", program(3, 5, 0, &[op(62, 0, 1)])),
        ]);
        let d = Dxbc::parse(&b).unwrap();
        assert_eq!(d.profile(), "hs_5_0");
        assert_eq!(d.pcsg.unwrap().elements[0].name, "SV_TessFactor");
    }
}
