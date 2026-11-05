//! The text of a DXBC block: `D3DDisassemble` through `py/d3d_disasm.py` (Windows, SR-D2), written to `<cache>/<sha256-12 of the DX11 exe>/dx11/asm/<sha1>.txt`, and the
//! few things Rust reads out of it: the reads of `cb<N>[<k>].<swizzle>` and the texture fetches,
//! in program order.
//!
//! The listing is the Microsoft text, unchanged:
//!
//! ```text
//! ps_5_0
//! dcl_globalFlags refactoringAllowed
//! dcl_constantbuffer CB1[12], immediateIndexed
//! dcl_resource_texture2d (float,float,float,float) t5
//! add r0.xyz, v3.xyzx, -cb1[11].xyzx
//! sample_indexable(texture2d)(float,float,float,float) r0.y, r1.xyxx, t5.yxzw, s5
//! ```

use std::path::{Path, PathBuf};

use super::DxbcError;

/// How the constant-buffer index is written: `cb1[9]` or `cb0[r0.x + 12]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CbIndex {
    Imm(u32),
    /// Dynamic index: the constant part and the register that is added (`r0.x`).
    Rel {
        base: u32,
        reg: String,
    },
}

/// One read of a constant buffer by an instruction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CbRead {
    pub cb: u32,
    pub index: CbIndex,
    /// Source component of each used lane, as written (`xyzx` is `[0, 1, 2, 0]`); a scalar read
    /// of one component has one entry.
    pub swizzle: Vec<u8>,
}

/// A texture fetch: the opcode and the resource and sampler registers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetch {
    pub tex: u32,
    pub smp: Option<u32>,
}

/// One instruction of the listing (declarations are counted, not kept).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextInst {
    /// `mad`, `sample_indexable(texture2d)(float,float,float,float)`, `mul_sat`, ...
    pub op: String,
    pub operands: Vec<String>,
    pub cb_reads: Vec<CbRead>,
    pub fetch: Option<Fetch>,
}

/// Declarations counted from the text, for the cross-check against the container reader.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeclCounts {
    pub constant_buffers: usize,
    pub samplers: usize,
    /// `dcl_resource_*` (textures, buffers, structured and raw buffers).
    pub resources: usize,
    pub inputs: usize,
    pub outputs: usize,
    /// `dcl_indexableTemp`.
    pub indexable_temps: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    /// `ps_5_0`, `vs_4_1`, ... from the first non-comment line.
    pub profile: String,
    pub decl_counts: DeclCounts,
    pub insts: Vec<TextInst>,
}

/// Splits `s` at commas that are outside parentheses and brackets.
fn split_operands(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for c in s.chars() {
        match c {
            '(' | '[' => {
                depth += 1;
                cur.push(c);
            }
            ')' | ']' => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 => {
                out.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

fn swizzle_of(s: &str) -> Vec<u8> {
    s.bytes()
        .map_while(|b| match b {
            b'x' | b'r' => Some(0),
            b'y' | b'g' => Some(1),
            b'z' | b'b' => Some(2),
            b'w' | b'a' => Some(3),
            _ => None,
        })
        .collect()
}

/// The constant-buffer read in one operand (`-cb1[11].xyzx`, `cb0[r0.x + 2].z`), if it has one.
pub fn cb_read(operand: &str) -> Option<CbRead> {
    let lower = operand.to_ascii_lowercase();
    // `cb` at the start of the operand or after a modifier (`-`, `|`), followed by digits and `[`.
    let mut from = 0;
    let at = loop {
        let i = from + lower[from..].find("cb")?;
        let before_ok = i == 0 || matches!(lower.as_bytes()[i - 1], b'-' | b'|' | b'(' | b' ');
        let rest = &lower[i + 2..];
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if before_ok && digits > 0 && rest.as_bytes().get(digits) == Some(&b'[') {
            break i;
        }
        from = i + 2;
    };
    let rest = &lower[at + 2..];
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let cb: u32 = rest[..digits].parse().ok()?;
    let inner_start = digits + 1;
    let close = rest[inner_start..].find(']')? + inner_start;
    let inner = rest[inner_start..close].trim();
    let index = match inner.parse::<u32>() {
        Ok(n) => CbIndex::Imm(n),
        Err(_) => {
            // `r0.x + 12`, `r0.x`
            let (reg, base) = match inner.split_once('+') {
                Some((reg, n)) => (reg.trim(), n.trim().parse::<u32>().ok()?),
                None => (inner, 0),
            };
            CbIndex::Rel {
                base,
                reg: reg.to_string(),
            }
        }
    };
    let tail = &rest[close + 1..];
    let swizzle = tail.strip_prefix('.').map(swizzle_of).unwrap_or_default();
    Some(CbRead { cb, index, swizzle })
}

/// Register number of `t5.yxzw` / `s5` / `t5`.
fn reg_number(operand: &str, prefix: char) -> Option<u32> {
    let s = operand.trim().strip_prefix(prefix)?;
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    s[..digits].parse().ok()
}

fn is_fetch(op: &str) -> bool {
    let base = op.split(['(', '_']).next().unwrap_or(op);
    matches!(base, "sample" | "ld" | "gather4" | "lod" | "sampleinfo")
}

/// Parses a listing. Lines of comments (`//`) and blank lines are skipped; the first other line is
/// the profile; `dcl_*` lines are counted; everything else is an instruction.
pub fn parse_listing(text: &str) -> Result<Listing, DxbcError> {
    let mut l = Listing::default();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        if l.profile.is_empty() {
            if !line
                .split('_')
                .next()
                .is_some_and(|s| matches!(s, "vs" | "ps" | "gs" | "hs" | "ds" | "cs"))
            {
                return Err(DxbcError::Text(format!(
                    "expected the profile, got {line:?}"
                )));
            }
            l.profile = line.to_string();
            continue;
        }
        if let Some(rest) = line.strip_prefix("dcl_") {
            let c = &mut l.decl_counts;
            if rest.starts_with("constantbuffer") {
                c.constant_buffers += 1;
            } else if rest.starts_with("sampler") {
                c.samplers += 1;
            } else if rest.starts_with("resource") {
                c.resources += 1;
            } else if rest.starts_with("input") && !rest.starts_with("input_control_point") {
                c.inputs += 1;
            } else if rest.starts_with("output") && !rest.starts_with("output_control_point") {
                c.outputs += 1;
            } else if rest.starts_with("indexabletemp") {
                c.indexable_temps += 1;
            }
            continue;
        }
        // Phase markers and `ret` have no operands.
        let (op, args) = match line.split_once(char::is_whitespace) {
            Some((op, args)) => (op.to_string(), args.trim()),
            None => (line.to_string(), ""),
        };
        let operands = split_operands(args);
        let cb_reads: Vec<CbRead> = operands.iter().filter_map(|o| cb_read(o)).collect();
        let fetch = is_fetch(&op).then(|| {
            // Operands: dst, address, t<N>[.swz], s<N> (`ld` has no sampler).
            let tex = operands
                .iter()
                .find_map(|o| reg_number(o, 't'))
                .unwrap_or(0);
            let smp = operands.iter().find_map(|o| reg_number(o, 's'));
            Fetch { tex, smp }
        });
        l.insts.push(TextInst {
            op,
            operands,
            cb_reads,
            fetch,
        });
    }
    if l.profile.is_empty() {
        return Err(DxbcError::Text("no profile line".to_string()));
    }
    Ok(l)
}

impl Listing {
    /// Reads of the constant buffers in program order, with the index of the instruction.
    pub fn cb_reads(&self) -> impl Iterator<Item = (usize, &CbRead)> {
        self.insts
            .iter()
            .enumerate()
            .flat_map(|(i, inst)| inst.cb_reads.iter().map(move |r| (i, r)))
    }
}

/// `<dx11 dir>/asm/<sha1>.txt`.
pub fn listing_path(dx11_dir: &Path, sha1: &str) -> PathBuf {
    dx11_dir.join("asm").join(format!("{sha1}.txt"))
}

/// Reads and parses the listing of a block.
pub fn read_listing(dx11_dir: &Path, sha1: &str) -> Result<Listing, DxbcError> {
    let p = listing_path(dx11_dir, sha1);
    let text = std::fs::read_to_string(&p)
        .map_err(|e| DxbcError::Text(format!("{}: {e} (run `dx11 asm`)", p.display())))?;
    parse_listing(&text)
}

/// Runs `py/d3d_disasm.py` (Windows, `python -I`) on every block file in `blocks_dir`, writing
/// the listings into `<dx11 dir>/asm/`. Skipped when every block already has its listing.
pub fn ensure_listings(
    python: &str,
    script: &Path,
    dx11_dir: &Path,
    blocks_dir: &Path,
    sha1s: &[String],
) -> Result<(), DxbcError> {
    if sha1s.iter().all(|s| listing_path(dx11_dir, s).is_file()) {
        return Ok(());
    }
    if !cfg!(windows) {
        return Err(DxbcError::Text(
            "the DX11 listings need Windows (d3dcompiler_47.dll) and are not in the cache"
                .to_string(),
        ));
    }
    let out = dx11_dir.join("asm");
    let status = std::process::Command::new(python)
        .arg("-I")
        .arg(script)
        .arg(&out)
        .arg(blocks_dir)
        .status()
        .map_err(|e| DxbcError::Text(format!("running {python} -I {}: {e}", script.display())))?;
    if !status.success() {
        return Err(DxbcError::Text(format!("d3d_disasm.py failed ({status})")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "//\n// Generated by Microsoft (R) D3D Shader Disassembler\n//\nps_5_0\n\
        dcl_globalFlags refactoringAllowed\n\
        dcl_constantbuffer CB1[12], immediateIndexed\n\
        dcl_constantbuffer CB0[4], immediateIndexed\n\
        dcl_sampler s5, mode_default\n\
        dcl_resource_texture2d (float,float,float,float) t5\n\
        dcl_input_ps linear v3.xyz\n\
        dcl_output o0.xyzw\n\
        dcl_temps 2\n\
        add r0.xyz, v3.xyzx, -cb1[11].xyzx\n\
        mul r0.w, r0.z, cb1[9].x\n\
        mov r1.y, cb1[9].w\n\
        sample_indexable(texture2d)(float,float,float,float) r0.y, r1.xyxx, t5.yxzw, s5\n\
        mad r2.xyz, -cb0[3].xyzx, cb0[r0.x + 2].zzyx, l(1.000000, 1.000000, 1.000000, 0.000000)\n\
        ret \n";

    #[test]
    fn listing_is_parsed() {
        let l = parse_listing(SAMPLE).unwrap();
        assert_eq!(l.profile, "ps_5_0");
        let c = &l.decl_counts;
        assert_eq!(
            (
                c.constant_buffers,
                c.samplers,
                c.resources,
                c.inputs,
                c.outputs
            ),
            (2, 1, 1, 1, 1)
        );
        assert_eq!(l.insts.len(), 6);
        assert_eq!(l.insts[0].op, "add");
        assert_eq!(
            l.insts[0].cb_reads,
            [CbRead {
                cb: 1,
                index: CbIndex::Imm(11),
                swizzle: vec![0, 1, 2, 0]
            }]
        );
        assert_eq!(l.insts[1].cb_reads[0].swizzle, [0]);
        let f = l.insts[3].fetch.as_ref().unwrap();
        assert_eq!((f.tex, f.smp), (5, Some(5)));
        // Two reads in one instruction, the second with a dynamic index; the immediate with its
        // commas inside the parentheses is one operand.
        let m = &l.insts[4];
        assert_eq!(m.operands.len(), 4);
        assert_eq!(m.cb_reads.len(), 2);
        assert_eq!(
            m.cb_reads[1],
            CbRead {
                cb: 0,
                index: CbIndex::Rel {
                    base: 2,
                    reg: "r0.x".into()
                },
                swizzle: vec![2, 2, 1, 0]
            }
        );
        assert_eq!(l.cb_reads().count(), 5);
        assert!(l.insts[5].operands.is_empty());
    }

    #[test]
    fn cb_read_spellings() {
        assert_eq!(cb_read("cb0[3].x").unwrap().index, CbIndex::Imm(3));
        assert_eq!(cb_read("|cb2[7].y|").unwrap().cb, 2);
        assert_eq!(cb_read("-|cb2[7].y|").unwrap().cb, 2);
        assert_eq!(
            cb_read("cb0[r1.y + 0].xyzw").unwrap().index,
            CbIndex::Rel {
                base: 0,
                reg: "r1.y".into()
            }
        );
        assert_eq!(
            cb_read("cb0[r1.y].xyzw").unwrap().index,
            CbIndex::Rel {
                base: 0,
                reg: "r1.y".into()
            }
        );
        assert!(cb_read("r0.xyz").is_none());
        // `icb[..]` and `x0[..]` are not constant buffers.
        assert!(cb_read("icb[3].x").is_none());
        assert!(cb_read("l(1.0, 2.0)").is_none());
    }

    #[test]
    fn bad_listings_are_errors() {
        assert!(parse_listing("").is_err());
        assert!(parse_listing("mov r0, r1\n").is_err());
    }
}
