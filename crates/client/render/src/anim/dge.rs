//! The original's blend graphs (`.dge`, Granny DAG2) of the unit states, and what the port plays of
//! them. The table (one expression per graph, decoded from `Resource/Common/UnitModels.fpk` of the
//! user's install by `tools/asset-pipeline`, set `client-data`) is not part of the program: the client
//! installs it from the asset set (`data:dge-graphs`, [`install`]); a native run can name a file with
//! `CIV5_DGE_GRAPHS`. Without a table every state plays the clips of its action (the fallback of
//! `graph`).
//!
//! The nodes (re-b19 0.1): `ecN(loop, cont)` plays the clip of event code N (`loop=0` endless,
//! `loop=1` once then the last frame held; `cont=1` keeps the phase of the same clip of the state
//! before); `Select(sel){i: ..}` picks input `int(sel + 0.001)` clamped to the inputs (an
//! unconnected selector takes its default); `Blend(p, from, to, mask)` mixes two poses by `p`.
//! Every state crossfades from the pose it had to its own by `p = S(min(1, stateTime / T))`
//! (`Curve3(BlendTime(T))`, the ease-in/out curve) or linearly (`BlendTime(T)`).
//!
//! What the port keeps (SIMPLIFIED(SIMP-exe-1271)): the `to` side of the entry blend, evaluated by
//! the path options, crossfaded from whatever the figure played before (the original's `from`
//! tree names that same clip by the options the entering socket set); the `AIM_` overlay (the
//! turret clips 1700/1705/1710 on the bones of `TrackMaskAIM_LAYER`) is left out, which is the
//! original's pose on every other bone; a blend by a path option takes the side the option is
//! nearer to.
// re: granny::SampleDAG_Selection 0x08e0b4b6, ScalarCallback table 0x0a301950,
// FBlockCurveLibrary 0x08d7b196

use std::collections::BTreeMap;
use std::sync::OnceLock;

/// Environment variable naming a table file (native runs and tests with real data).
pub const TABLE_ENV: &str = "CIV5_DGE_GRAPHS";

/// The table the client installed from the asset set.
static INSTALLED: OnceLock<BTreeMap<String, Node>> = OnceLock::new();

/// A scalar input of a node.
#[derive(Debug, Clone, PartialEq)]
pub enum Scalar {
    Const(f32),
    /// A path option of the machine.
    Path(u16),
    /// `min(1, stateTime / T)`.
    BlendTime(f32),
    /// `min(1, stateTime / duration(clip ec))`.
    BlendEc(u32),
    /// The S-curve of `FBlockCurveLibrary` (type 3) over the input.
    Curve(Box<Scalar>),
    /// No edge: 0.
    Unconnected,
}

/// A pose node.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Clip {
        ec: u32,
        once: bool,
        cont: bool,
    },
    /// An input with no edge: no pose.
    Nothing,
    Select {
        sel: Scalar,
        default: u16,
        arms: Vec<Node>,
    },
    Blend {
        p: Scalar,
        from: Box<Node>,
        to: Box<Node>,
        masked: bool,
    },
}

/// The graphs by name (`FORTIFY_GRAPH`, `AIM_ENTER_FORTIFY`: upper case, no extension).
/// The installed table; before (or without) one, the table of `CIV5_DGE_GRAPHS`, else none (in
/// this crate's unit tests: the synthetic `dge_fixture.txt`).
pub fn graphs() -> &'static BTreeMap<String, Node> {
    if let Some(g) = INSTALLED.get() {
        return g;
    }
    static FALLBACK: OnceLock<BTreeMap<String, Node>> = OnceLock::new();
    FALLBACK.get_or_init(fallback_table)
}

fn fallback_table() -> BTreeMap<String, Node> {
    if let Some(path) = std::env::var_os(TABLE_ENV) {
        match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| parse_table(&t))
        {
            Ok(t) => return t,
            Err(e) => tracing::warn!("{TABLE_ENV}={}: {e}", std::path::Path::new(&path).display()),
        }
    }
    #[cfg(test)]
    return parse_table(include_str!("dge_fixture.txt")).expect("dge_fixture.txt");
    #[cfg(not(test))]
    BTreeMap::new()
}

/// Installs the table of the asset set (`data:dge-graphs`); the first one stays. Returns the number
/// of graphs.
pub fn install(text: &str) -> Result<usize, String> {
    let table = parse_table(text)?;
    let n = table.len();
    if INSTALLED.set(table).is_err() {
        tracing::debug!("a blend graph table is installed already; the new one is ignored");
    }
    Ok(n)
}

/// True once a table is in use (installed, or named by `CIV5_DGE_GRAPHS`).
pub fn has_table() -> bool {
    !graphs().is_empty()
}

/// The graph a state's `asset` names: folder and extension dropped, Lekmod's `EDITED_` copies
/// read as the vanilla graph they edit (re-b19 0.3: the body is the same).
pub fn graph_of(asset: &str) -> Option<&'static Node> {
    let file = asset.rsplit(['/', '\\']).next().unwrap_or(asset);
    let up = file.to_ascii_uppercase();
    let stem = up.strip_suffix(".DGE").unwrap_or(&up);
    let stem = stem.strip_prefix("EDITED_").unwrap_or(stem);
    graphs().get(stem)
}

pub fn parse_table(text: &str) -> Result<BTreeMap<String, Node>, String> {
    let mut out = BTreeMap::new();
    let mut name: Option<String> = None;
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("Sub[") {
            let n = name
                .take()
                .ok_or_else(|| format!("expression without a name: {line}"))?;
            let body = rest
                .strip_suffix(']')
                .ok_or_else(|| format!("{n}: no closing ]"))?;
            let mut p = Parser {
                s: body.as_bytes(),
                i: 0,
            };
            let node = p.node().map_err(|e| format!("{n}: {e} at {}", p.i))?;
            if p.i != p.s.len() {
                return Err(format!("{n}: trailing text at {}", p.i));
            }
            out.insert(n, node);
        } else {
            let n = line.strip_suffix(".DGE").unwrap_or(line);
            name = Some(n.to_ascii_uppercase());
        }
    }
    Ok(out)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn rest(&self) -> &[u8] {
        &self.s[self.i..]
    }

    fn eat(&mut self, t: &str) -> bool {
        if self.rest().starts_with(t.as_bytes()) {
            self.i += t.len();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, t: &str) -> Result<(), String> {
        if self.eat(t) {
            Ok(())
        } else {
            Err(format!("expected {t:?}"))
        }
    }

    /// A number up to the next `,`, `)`, `]`, `}`, `;` or `-` that does not start it.
    fn number(&mut self) -> Result<f64, String> {
        let start = self.i;
        while self.i < self.s.len() {
            let c = self.s[self.i];
            let sign_ok = self.i == start || matches!(self.s[self.i - 1], b'e' | b'E');
            if c.is_ascii_digit()
                || c == b'.'
                || c == b'e'
                || c == b'E'
                || ((c == b'-' || c == b'+') && sign_ok)
            {
                self.i += 1;
            } else {
                break;
            }
        }
        std::str::from_utf8(&self.s[start..self.i])
            .ok()
            .and_then(|t| t.parse().ok())
            .ok_or_else(|| "a number".to_owned())
    }

    fn kv(&mut self, key: &str) -> Result<f64, String> {
        self.expect(key)?;
        self.expect("=")?;
        self.number()
    }

    fn scalar(&mut self) -> Result<Scalar, String> {
        if self.eat("-") {
            return Ok(Scalar::Unconnected);
        }
        if self.eat("Curve3(") {
            let s = self.scalar()?;
            self.expect(")")?;
            return Ok(Scalar::Curve(Box::new(s)));
        }
        if self.eat("BlendTime(") {
            let v = self.number()?;
            self.expect(")")?;
            return Ok(Scalar::BlendTime(v as f32));
        }
        if self.eat("BlendEC(") {
            let v = self.number()?;
            self.expect(")")?;
            return Ok(Scalar::BlendEc(v as u32));
        }
        if self.eat("Path(") {
            let v = self.number()?;
            self.expect(")")?;
            // An index stored as a float; a denormal (an authoring slip) is option 0.
            return Ok(Scalar::Path((v + 0.001).max(0.0) as u16));
        }
        if self.eat("const[") {
            let v = self.number()?;
            self.expect("]")?;
            return Ok(Scalar::Const(v as f32));
        }
        Err("a scalar".into())
    }

    fn node(&mut self) -> Result<Node, String> {
        if self.eat("-") {
            return Ok(Node::Nothing);
        }
        if self.eat("Blend(") {
            self.expect("p=")?;
            let p = self.scalar()?;
            self.expect(", from=")?;
            let from = self.node()?;
            self.expect(", to=")?;
            let to = self.node()?;
            self.expect(", mask=")?;
            let masked = if self.eat("-") {
                false
            } else {
                self.expect("Mask(")?;
                while self.i < self.s.len() && self.s[self.i] != b')' {
                    self.i += 1;
                }
                self.expect(")")?;
                true
            };
            self.expect(")")?;
            return Ok(Node::Blend {
                p,
                from: Box::new(from),
                to: Box::new(to),
                masked,
            });
        }
        if self.eat("Select(") {
            self.expect("sel=")?;
            let sel = self.scalar()?;
            self.expect(", ")?;
            let default = self.kv("default")? as u16;
            self.expect("){")?;
            let mut arms = Vec::new();
            loop {
                let k = self.number()? as usize;
                if k != arms.len() {
                    return Err(format!("arm {k} out of order"));
                }
                self.expect(":")?;
                arms.push(self.node()?);
                if self.eat("; ") {
                    continue;
                }
                self.expect("}")?;
                break;
            }
            return Ok(Node::Select { sel, default, arms });
        }
        if self.eat("ec") {
            let ec = self.number()? as u32;
            self.expect("(")?;
            let once = self.kv("loop")? != 0.0;
            self.expect(",")?;
            self.kv("clamp")?;
            self.expect(",")?;
            let cont = self.kv("cont")? != 0.0;
            self.expect(",")?;
            self.kv("spd")?;
            self.expect(",")?;
            self.kv("off")?;
            self.expect(")")?;
            // ` IN[Path(6)]`: an input of the clip node the port does not read.
            if self.eat(" IN[") {
                self.scalar()?;
                self.expect("]")?;
            }
            return Ok(Node::Clip { ec, once, cont });
        }
        Err("a node".into())
    }
}

/// `Selection`: input `int(v + 0.001)`, clamped to the inputs; below 0 the first.
pub fn select_index(v: f32, arms: usize) -> usize {
    if arms == 0 || v.is_nan() || v < 0.0 {
        return 0;
    }
    ((v + 0.001) as usize).min(arms - 1)
}

/// The ease-in/ease-out curve of `FBlockCurveLibrary` type 3: Catmull-Rom through (0, 0),
/// (0.25, 0.12), (0.75, 0.88), (1, 1), the ends repeated.
// SIMPLIFIED(SIMP-exe-1271): a uniform Catmull-Rom on both coordinates, x solved by bisection;
// the library's parametrisation was read only at call level.
pub fn s_curve(x: f32) -> f32 {
    const P: [(f32, f32); 6] = [
        (0.0, 0.0),
        (0.0, 0.0),
        (0.25, 0.12),
        (0.75, 0.88),
        (1.0, 1.0),
        (1.0, 1.0),
    ];
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let cr = |a: f32, b: f32, c: f32, d: f32, u: f32| {
        0.5 * (2.0 * b
            + (c - a) * u
            + (2.0 * a - 5.0 * b + 4.0 * c - d) * u * u
            + (3.0 * b - a - 3.0 * c + d) * u * u * u)
    };
    for s in 0..3 {
        let (p0, p1, p2, p3) = (P[s], P[s + 1], P[s + 2], P[s + 3]);
        if x > p2.0 && s < 2 {
            continue;
        }
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..30 {
            let mid = 0.5 * (lo + hi);
            if cr(p0.0, p1.0, p2.0, p3.0, mid) < x {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let u = 0.5 * (lo + hi);
        return cr(p0.1, p1.1, p2.1, p3.1, u).clamp(0.0, 1.0);
    }
    x
}

/// A clip the port plays for a graph, under the selections that lead to it.
#[derive(Debug, Clone, PartialEq)]
pub struct Leaf {
    pub ec: u32,
    pub once: bool,
    pub cont: bool,
    /// `(path, arm, arms)`: the selections that must pick this clip.
    pub when: Vec<(u16, u16, u16)>,
    /// The entry crossfade: seconds, and whether the S-curve shapes it (`None`: a hard cut).
    pub blend: Option<(f32, bool)>,
    /// The blend time is the length of this clip (`BlendEC`).
    pub blend_ec: Option<u32>,
}

/// The clips the port plays of `node`, in the order of the graph (see the module text).
pub fn leaves(node: &Node) -> Vec<Leaf> {
    let mut out = Vec::new();
    walk(node, &mut Vec::new(), None, None, &mut out);
    out
}

/// The entry blend of a `p`: (seconds, S-curve), and the clip whose length times it.
type EntryBlend = (Option<(f32, bool)>, Option<u32>);

fn blend_of(p: &Scalar) -> Option<EntryBlend> {
    match p {
        Scalar::BlendTime(t) => Some((Some((*t, false)), None)),
        Scalar::BlendEc(ec) => Some((Some((0.0, false)), Some(*ec))),
        Scalar::Curve(inner) => match blend_of(inner)? {
            (Some((t, _)), e) => Some((Some((t, true)), e)),
            other => Some(other),
        },
        _ => None,
    }
}

fn walk(
    node: &Node,
    when: &mut Vec<(u16, u16, u16)>,
    blend: Option<(f32, bool)>,
    blend_ec: Option<u32>,
    out: &mut Vec<Leaf>,
) {
    match node {
        Node::Nothing => {}
        Node::Clip { ec, once, cont } => {
            if *ec != 0 {
                out.push(Leaf {
                    ec: *ec,
                    once: *once,
                    cont: *cont,
                    when: when.clone(),
                    blend,
                    blend_ec,
                });
            }
        }
        Node::Select { sel, default, arms } => match sel {
            Scalar::Path(p) => {
                let n = arms.len() as u16;
                for (i, a) in arms.iter().enumerate() {
                    when.push((*p, i as u16, n));
                    walk(a, when, blend, blend_ec, out);
                    when.pop();
                }
            }
            // A selector of time (BlendEC) is 0 at the state entry: its first input; no edge:
            // the default input.
            Scalar::BlendEc(_) | Scalar::BlendTime(_) | Scalar::Curve(_) => {
                if let Some(a) = arms.first() {
                    walk(a, when, blend, blend_ec, out);
                }
            }
            Scalar::Const(v) => walk(
                &arms[select_index(*v, arms.len())],
                when,
                blend,
                blend_ec,
                out,
            ),
            Scalar::Unconnected => {
                let i = usize::from(*default).min(arms.len().saturating_sub(1));
                if let Some(a) = arms.get(i) {
                    walk(a, when, blend, blend_ec, out);
                }
            }
        },
        Node::Blend {
            p,
            from,
            to,
            masked,
        } => {
            // The AIM overlay on its masked bones: the body only.
            if *masked {
                return walk(from, when, blend, blend_ec, out);
            }
            match p {
                Scalar::Const(v) if *v >= 0.5 => walk(to, when, blend, blend_ec, out),
                Scalar::Const(_) | Scalar::Unconnected => walk(from, when, blend, blend_ec, out),
                // A blend by a path option: the side the option is nearer to.
                Scalar::Path(path) => {
                    when.push((*path, 0, 2));
                    walk(from, when, blend, blend_ec, out);
                    when.pop();
                    when.push((*path, 1, 2));
                    walk(to, when, blend, blend_ec, out);
                    when.pop();
                }
                timed => {
                    let (b, e) = blend_of(timed).unwrap_or((None, None));
                    // The outermost entry blend is the one the state starts with.
                    let (b, e) = if blend.is_some() {
                        (blend, blend_ec)
                    } else {
                        (b, e)
                    };
                    walk(to, when, b, e, out);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_graph_of_the_table_parses() {
        let g = graphs();
        assert!(g.len() >= 17, "{}", g.len());
        for (name, node) in g {
            assert!(!leaves(node).is_empty(), "{name} plays nothing");
        }
        // The table of the install (made by the asset pipeline), when the run names one: its 123
        // graphs all parse and play something.
        match std::env::var_os(TABLE_ENV) {
            Some(path) => {
                let text = std::fs::read_to_string(path).expect("the table file");
                let t = parse_table(&text).expect("the install's table");
                assert_eq!(t.len(), 123);
                for (name, node) in &t {
                    assert!(!leaves(node).is_empty(), "{name} plays nothing");
                }
            }
            None => eprintln!("SKIPPED: the install's table ({TABLE_ENV} not set)"),
        }
    }

    #[test]
    fn every_form_of_the_table_reads() {
        let fixture = parse_table(include_str!("dge_fixture.txt")).unwrap();
        let l = leaves(&fixture["PROBE_EVERY_FORM"]);
        // BlendEC times the entry; a blend by a path option gives both sides; a constant selector
        // picks its arm; an unconnected selector takes the default arm.
        assert_eq!(
            l.iter().map(|x| (x.ec, x.when.clone())).collect::<Vec<_>>(),
            [(1400, vec![(0, 0, 2)]), (1420, vec![(0, 1, 2)])]
        );
        assert!(l.iter().all(|x| x.blend_ec == Some(1420)));
        assert!(parse_table("NAME.DGE\nSub[ec1(loop=0)]").is_err());
        assert!(parse_table("Sub[-]").is_err());
    }

    #[test]
    fn the_selection_index_is_the_option_rounded_down_and_clamped() {
        assert_eq!(select_index(0.0, 3), 0);
        assert_eq!(select_index(0.999, 3), 1);
        assert_eq!(select_index(0.95, 3), 0);
        assert_eq!(select_index(7.0, 3), 2);
        assert_eq!(select_index(-1.0, 3), 0);
    }

    #[test]
    fn the_s_curve_eases_in_and_out() {
        assert_eq!(s_curve(0.0), 0.0);
        assert!((s_curve(1.0) - 1.0).abs() < 1e-4);
        assert!((s_curve(0.5) - 0.5).abs() < 0.02);
        assert!((s_curve(0.25) - 0.12).abs() < 0.01);
        assert!(s_curve(0.1) < 0.1 && s_curve(0.9) > 0.9);
        let mut last = 0.0;
        for i in 0..=100 {
            let y = s_curve(i as f32 / 100.0);
            assert!(y + 1e-5 >= last, "monotone at {i}");
            last = y;
        }
    }

    #[test]
    fn the_names_of_the_install_and_of_lekmod_find_their_graph() {
        assert!(graph_of("FORTIFY_GRAPH.dge").is_some());
        assert!(graph_of("Art/aim_fortify_graph.DGE").is_some());
        assert_eq!(
            graph_of("EDITED_AIM_IDLE_GRAPH.dge"),
            graph_of("AIM_IDLE_GRAPH.dge")
        );
        assert!(graph_of("MYSTERY.dge").is_none());
    }
}
