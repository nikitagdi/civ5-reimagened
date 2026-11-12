//! A small semver subset (no `semver` crate, plan §1): versions `MAJOR.MINOR.PATCH` without pre-release or build
//! metadata, and requirements made of comparators joined by commas (logical AND):
//!
//! | comparator | meaning |
//! |---|---|
//! | `=1.2.3` | exactly 1.2.3; `=1.2` is `>=1.2.0, <1.3.0`; `=1` is `>=1.0.0, <2.0.0` |
//! | `^1.2.3` | `>=1.2.3, <2.0.0`; `^0.2.3` is `<0.3.0`; `^0.0.3` is `<0.0.4`; `^1.2`, `^1` as in Cargo |
//! | `~1.2.3` | `>=1.2.3, <1.3.0`; `~1.2` is `<1.3.0`; `~1` is `<2.0.0` |
//! | `>1.2.3` `>=1.2.3` `<1.2.3` `<=1.2.3` | plain comparison; a missing component means the whole range |
//! | `*` | any version (only alone) |
//!
//! Every comparator is an interval `[lo, hi)`; a version matches when it lies in all of them.

use std::fmt;
use std::str::FromStr;

use crate::error::SemverError;

/// Largest accepted version component. Keeps "next version" arithmetic free of overflow.
pub const MAX_COMPONENT: u32 = 1_000_000_000;

/// `MAJOR.MINOR.PATCH`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

fn parse_component(s: &str) -> Result<u32, &'static str> {
    if s.is_empty() {
        return Err("empty version component");
    }
    if !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err("version components must be decimal numbers");
    }
    if s.len() > 1 && s.starts_with('0') {
        return Err("leading zero in a version component");
    }
    match s.parse::<u32>() {
        Ok(n) if n <= MAX_COMPONENT => Ok(n),
        _ => Err("version component is too large"),
    }
}

impl FromStr for Version {
    type Err = SemverError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = |reason| SemverError::Version {
            input: s.to_string(),
            reason,
        };
        if s.contains(['-', '+']) {
            return Err(err("pre-release and build metadata are not supported"));
        }
        let parts: Vec<&str> = s.split('.').collect();
        let [major, minor, patch] = parts.as_slice() else {
            return Err(err("expected MAJOR.MINOR.PATCH"));
        };
        Ok(Self {
            major: parse_component(major).map_err(err)?,
            minor: parse_component(minor).map_err(err)?,
            patch: parse_component(patch).map_err(err)?,
        })
    }
}

/// A version with optional minor and patch (`1`, `1.2`, `1.2.3`), used inside requirements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Partial {
    major: u32,
    minor: Option<u32>,
    patch: Option<u32>,
}

impl Partial {
    fn lower(self) -> Version {
        Version::new(self.major, self.minor.unwrap_or(0), self.patch.unwrap_or(0))
    }

    /// The first version after everything that has this prefix: `1.2.3` -> `1.2.4`, `1.2` -> `1.3.0`, `1` -> `2.0.0`.
    fn upper(self) -> Version {
        match (self.minor, self.patch) {
            (Some(mi), Some(pa)) => Version::new(self.major, mi, pa + 1),
            (Some(mi), None) => Version::new(self.major, mi + 1, 0),
            (None, _) => Version::new(self.major + 1, 0, 0),
        }
    }

    fn next_minor(self) -> Version {
        Version::new(self.major, self.minor.unwrap_or(0) + 1, 0)
    }

    fn next_major(self) -> Version {
        Version::new(self.major + 1, 0, 0)
    }
}

impl fmt::Display for Partial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.major)?;
        if let Some(mi) = self.minor {
            write!(f, ".{mi}")?;
            if let Some(pa) = self.patch {
                write!(f, ".{pa}")?;
            }
        }
        Ok(())
    }
}

fn parse_partial(s: &str) -> Result<Partial, &'static str> {
    if s.contains(['-', '+']) {
        return Err("pre-release and build metadata are not supported");
    }
    let mut it = s.split('.');
    let major = parse_component(it.next().unwrap_or(""))?;
    let minor = it.next().map(parse_component).transpose()?;
    let patch = it.next().map(parse_component).transpose()?;
    if it.next().is_some() {
        return Err("too many version components");
    }
    Ok(Partial {
        major,
        minor,
        patch,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Exact,
    Caret,
    Tilde,
    Greater,
    GreaterEq,
    Less,
    LessEq,
}

impl Op {
    fn symbol(self) -> &'static str {
        match self {
            Op::Exact => "=",
            Op::Caret => "^",
            Op::Tilde => "~",
            Op::Greater => ">",
            Op::GreaterEq => ">=",
            Op::Less => "<",
            Op::LessEq => "<=",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Comparator {
    op: Op,
    p: Partial,
}

impl Comparator {
    /// The interval `[lo, hi)` of matching versions; `None` is unbounded.
    fn bounds(self) -> (Option<Version>, Option<Version>) {
        let p = self.p;
        let lo = p.lower();
        match self.op {
            Op::Exact => (Some(lo), Some(p.upper())),
            Op::Caret => {
                let hi = if p.major > 0 {
                    p.next_major()
                } else {
                    match (p.minor, p.patch) {
                        (None, _) => p.next_major(),
                        (Some(mi), _) if mi > 0 => p.next_minor(),
                        (Some(_), None) => p.next_minor(),
                        (Some(mi), Some(pa)) => Version::new(0, mi, pa + 1),
                    }
                };
                (Some(lo), Some(hi))
            }
            Op::Tilde => {
                let hi = if p.minor.is_some() {
                    p.next_minor()
                } else {
                    p.next_major()
                };
                (Some(lo), Some(hi))
            }
            Op::Greater => (Some(p.upper()), None),
            Op::GreaterEq => (Some(lo), None),
            Op::Less => (None, Some(lo)),
            Op::LessEq => (None, Some(p.upper())),
        }
    }

    fn matches(self, v: &Version) -> bool {
        let (lo, hi) = self.bounds();
        lo.is_none_or(|lo| *v >= lo) && hi.is_none_or(|hi| *v < hi)
    }
}

/// A version requirement: comparators joined by commas, or `*`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionReq {
    /// Empty means `*`.
    comparators: Vec<Comparator>,
}

impl VersionReq {
    /// `*`: any version.
    pub const STAR: VersionReq = VersionReq {
        comparators: Vec::new(),
    };

    pub fn is_any(&self) -> bool {
        self.comparators.is_empty()
    }

    pub fn matches(&self, v: &Version) -> bool {
        self.comparators.iter().all(|c| c.matches(v))
    }

    /// True when some version below `v` satisfies the requirement (`^1` admits 1.0.0, so it admits
    /// versions below 1.3.0; `^1.3` does not).
    pub fn admits_below(&self, v: &Version) -> bool {
        let mut lo = Version::new(0, 0, 0);
        let mut hi: Option<Version> = None;
        for c in &self.comparators {
            let (l, h) = c.bounds();
            if let Some(l) = l {
                lo = lo.max(l);
            }
            if let Some(h) = h {
                hi = Some(hi.map_or(h, |x| x.min(h)));
            }
        }
        lo < *v && hi.is_none_or(|h| lo < h)
    }
}

impl fmt::Display for VersionReq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.comparators.is_empty() {
            return f.write_str("*");
        }
        for (i, c) in self.comparators.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{}{}", c.op.symbol(), c.p)?;
        }
        Ok(())
    }
}

impl FromStr for VersionReq {
    type Err = SemverError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = |reason| SemverError::Req {
            input: s.to_string(),
            reason,
        };
        let s_trim = s.trim();
        if s_trim == "*" {
            return Ok(Self::STAR);
        }
        if s_trim.is_empty() {
            return Err(err("empty requirement"));
        }
        let mut comparators = Vec::new();
        for piece in s_trim.split(',') {
            let piece = piece.trim();
            if piece.is_empty() {
                return Err(err("empty comparator (stray comma)"));
            }
            if piece == "*" {
                return Err(err("`*` cannot be combined with other comparators"));
            }
            // Two-character operators first.
            let (op, rest) = if let Some(r) = piece.strip_prefix(">=") {
                (Op::GreaterEq, r)
            } else if let Some(r) = piece.strip_prefix("<=") {
                (Op::LessEq, r)
            } else if let Some(r) = piece.strip_prefix('>') {
                (Op::Greater, r)
            } else if let Some(r) = piece.strip_prefix('<') {
                (Op::Less, r)
            } else if let Some(r) = piece.strip_prefix('=') {
                (Op::Exact, r)
            } else if let Some(r) = piece.strip_prefix('^') {
                (Op::Caret, r)
            } else if let Some(r) = piece.strip_prefix('~') {
                (Op::Tilde, r)
            } else {
                return Err(err(
                    "missing operator (use =, ^, ~, >, >=, <, <= or a single `*`)",
                ));
            };
            let p = parse_partial(rest.trim()).map_err(err)?;
            comparators.push(Comparator { op, p });
        }
        Ok(Self { comparators })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        s.parse().unwrap()
    }
    fn req(s: &str) -> VersionReq {
        s.parse().unwrap()
    }
    fn ok(r: &str, version: &str) -> bool {
        req(r).matches(&v(version))
    }

    #[test]
    fn version_parse_and_display() {
        assert_eq!(v("1.2.3"), Version::new(1, 2, 3));
        assert_eq!(v("0.0.0").to_string(), "0.0.0");
        assert_eq!(v("10.20.30").to_string(), "10.20.30");
    }

    #[test]
    fn version_parse_rejects_garbage() {
        for bad in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "a.b.c",
            "1.2.x",
            "01.2.3",
            "1.02.3",
            "1.2.03",
            "-1.2.3",
            "1..3",
            " 1.2.3",
            "1.2.3 ",
            "1.2.3-alpha",
            "1.2.3+build",
            "99999999999.0.0",
            "1000000001.0.0",
            "+1.2.3",
        ] {
            assert!(bad.parse::<Version>().is_err(), "{bad:?} must be rejected");
        }
        assert!(
            "1.2.3-rc1"
                .parse::<Version>()
                .unwrap_err()
                .to_string()
                .contains("pre-release")
        );
    }

    #[test]
    fn version_ordering() {
        assert!(v("1.2.3") < v("1.2.4"));
        assert!(v("1.2.10") > v("1.2.9"));
        assert!(v("1.10.0") > v("1.9.99"));
        assert!(v("2.0.0") > v("1.99.99"));
        let mut all = vec![v("1.10.0"), v("1.2.0"), v("0.9.9"), v("1.2.0")];
        all.sort();
        assert_eq!(all, [v("0.9.9"), v("1.2.0"), v("1.2.0"), v("1.10.0")]);
    }

    #[test]
    fn star_matches_everything() {
        assert!(req("*").is_any());
        assert!(ok("*", "0.0.0"));
        assert!(ok("*", "999.9.9"));
        assert!(ok(" * ", "1.0.0"));
        assert_eq!(req("*").to_string(), "*");
    }

    #[test]
    fn exact() {
        assert!(ok("=1.2.3", "1.2.3"));
        assert!(!ok("=1.2.3", "1.2.4"));
        assert!(!ok("=1.2.3", "1.2.2"));
        assert!(ok("=1.2", "1.2.0") && ok("=1.2", "1.2.9"));
        assert!(!ok("=1.2", "1.3.0") && !ok("=1.2", "1.1.9"));
        assert!(ok("=1", "1.9.9") && !ok("=1", "2.0.0") && !ok("=1", "0.9.9"));
    }

    #[test]
    fn caret() {
        assert!(ok("^1.2.3", "1.2.3") && ok("^1.2.3", "1.9.0") && ok("^1.2.3", "1.2.4"));
        assert!(!ok("^1.2.3", "1.2.2") && !ok("^1.2.3", "2.0.0") && !ok("^1.2.3", "0.9.9"));
        assert!(
            ok("^1", "1.0.0") && ok("^1", "1.99.0") && !ok("^1", "2.0.0") && !ok("^1", "0.9.9")
        );
        assert!(
            ok("^1.2", "1.2.0")
                && ok("^1.2", "1.3.0")
                && !ok("^1.2", "1.1.9")
                && !ok("^1.2", "2.0.0")
        );
        // Zero major: the leftmost non-zero component is the compatibility boundary.
        assert!(
            ok("^0.2.3", "0.2.3")
                && ok("^0.2.3", "0.2.9")
                && !ok("^0.2.3", "0.3.0")
                && !ok("^0.2.3", "0.2.2")
        );
        assert!(ok("^0.0.3", "0.0.3") && !ok("^0.0.3", "0.0.4") && !ok("^0.0.3", "0.0.2"));
        assert!(ok("^0.0", "0.0.9") && !ok("^0.0", "0.1.0"));
        assert!(ok("^0", "0.9.9") && !ok("^0", "1.0.0"));
        assert!(ok("^0.2", "0.2.7") && !ok("^0.2", "0.3.0"));
    }

    #[test]
    fn tilde() {
        assert!(ok("~1.2.3", "1.2.3") && ok("~1.2.3", "1.2.99"));
        assert!(!ok("~1.2.3", "1.3.0") && !ok("~1.2.3", "1.2.2"));
        assert!(ok("~1.2", "1.2.0") && !ok("~1.2", "1.3.0"));
        assert!(ok("~1", "1.9.9") && !ok("~1", "2.0.0") && !ok("~1", "0.9.9"));
    }

    #[test]
    fn comparisons() {
        assert!(ok(">1.2.3", "1.2.4") && !ok(">1.2.3", "1.2.3"));
        assert!(ok(">1.2", "1.3.0") && !ok(">1.2", "1.2.9"));
        assert!(ok(">1", "2.0.0") && !ok(">1", "1.9.9"));
        assert!(ok(">=1.2.3", "1.2.3") && !ok(">=1.2.3", "1.2.2"));
        assert!(ok(">=1.2", "1.2.0") && !ok(">=1.2", "1.1.9"));
        assert!(ok("<1.2.3", "1.2.2") && !ok("<1.2.3", "1.2.3"));
        assert!(ok("<1.2", "1.1.9") && !ok("<1.2", "1.2.0"));
        assert!(ok("<=1.2.3", "1.2.3") && !ok("<=1.2.3", "1.2.4"));
        assert!(ok("<=1.2", "1.2.9") && !ok("<=1.2", "1.3.0"));
        assert!(ok("<=1", "1.9.9") && !ok("<=1", "2.0.0"));
    }

    #[test]
    fn comma_is_and() {
        let r = req(">=1.2, <2");
        assert!(r.matches(&v("1.2.0")) && r.matches(&v("1.9.9")));
        assert!(!r.matches(&v("1.1.9")) && !r.matches(&v("2.0.0")));
        let r = req(">=1.0.0,<1.5.0 ");
        assert!(r.matches(&v("1.4.9")) && !r.matches(&v("1.5.0")));
        // Contradictory ranges match nothing.
        let r = req(">=2, <1");
        assert!(!r.matches(&v("0.5.0")) && !r.matches(&v("1.5.0")) && !r.matches(&v("2.5.0")));
    }

    #[test]
    fn whitespace_after_operator() {
        assert!(ok("^ 1.2.3", "1.5.0"));
        assert!(ok(">= 1.2", "1.2.0"));
    }

    #[test]
    fn req_parse_rejects_garbage() {
        for bad in [
            "",
            " ",
            ",",
            "1.2.3",
            "1",
            "^",
            "^x",
            "^1.2.3.4",
            "^1.2.3-beta",
            "~",
            ">=",
            "=>1.0",
            "==1.0",
            "!=1.0",
            "^1,",
            ",^1",
            "^1,,^2",
            "*,^1",
            "^1,*",
            "**",
            "^01.2",
            "^1.x",
            "^1.2.",
            "1.2.x",
        ] {
            assert!(
                bad.parse::<VersionReq>().is_err(),
                "{bad:?} must be rejected"
            );
        }
        let e = "1.2.3".parse::<VersionReq>().unwrap_err().to_string();
        assert!(e.contains("missing operator"), "{e}");
    }

    #[test]
    fn req_display_round_trips() {
        for s in [
            "*",
            "^1",
            "^1.2",
            "^1.2.3",
            "~0.2",
            "=1.4.0",
            ">=1.2, <2",
            ">1",
            "<=3.0.0",
            ">=1.0.0, <1.5, >1.1",
        ] {
            let r = req(s);
            assert_eq!(r.to_string(), s);
            assert_eq!(req(&r.to_string()), r);
        }
        // Normalised spacing.
        assert_eq!(req(">=1.2,<2").to_string(), ">=1.2, <2");
        assert_eq!(req("^ 1").to_string(), "^1");
    }

    #[test]
    fn largest_component_does_not_overflow() {
        let max = format!("{MAX_COMPONENT}.{MAX_COMPONENT}.{MAX_COMPONENT}");
        let top: Version = max.parse().unwrap();
        for r in [
            "^1000000000",
            ">1000000000",
            "<=1000000000.1000000000.1000000000",
            "~1000000000.1000000000",
            "=1000000000",
        ] {
            let _ = req(r).matches(&top);
        }
        assert!(!req(">1000000000.1000000000.1000000000").matches(&top));
    }
}
