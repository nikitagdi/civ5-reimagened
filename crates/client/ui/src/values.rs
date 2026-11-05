//! Attribute values the way the exe reads them: `ExtractCoords`, `LoadAnchorFlags`,
//! `LoadAnchorSideFlags`, booleans and numbers. The helpers carry the exe function names.

use crate::flags;

/// Separators of `FStringA::Extract` in every pair, anchor and colour value.
const SEPARATORS: [char; 6] = [' ', '.', ',', 'x', '*', '|'];

fn trimmed(text: &str) -> &str {
    text.trim_matches(|c: char| c == ' ' || c == '\t' || c == '\r' || c == '\n')
}

/// Tokens of a value split at the separators, empty tokens included.
fn raw_tokens(text: &str) -> Vec<&str> {
    trimmed(text).split(SEPARATORS).collect()
}

/// The first token (even when empty) and the next non-empty one: the `ExtractStrings` and
/// `ExtractCoords` rule (the second token skips empty pieces such as the gap in `60, +10`).
fn first_two(text: &str) -> (&str, Option<&str>) {
    let t = raw_tokens(text);
    let first = t.first().copied().unwrap_or("");
    let second = t.iter().skip(1).copied().find(|s| !s.is_empty());
    (first, second)
}

/// `FStringA::ConvertToFloat(10)`: a decimal number, anything else is 0. A sign is allowed.
pub fn to_float(token: &str) -> f32 {
    let t = trimmed(token);
    if let Ok(v) = t.parse::<f32>() {
        return v;
    }
    // Leading numeric prefix (`12px`), else 0.
    let end = t
        .char_indices()
        .take_while(|&(i, c)| c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+')))
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    t[..end].parse::<f32>().unwrap_or(0.0)
}

/// `FStringA::ConvertToInt(10, true)`.
pub fn to_int(token: &str) -> i32 {
    to_float(token) as i32
}

/// A value that starts with `t`/`T` or `1` is true (`True`, `true`, `1`).
pub fn parse_bool(text: &str) -> bool {
    let t = trimmed(text);
    t.starts_with(['t', 'T']) || t.starts_with('1')
}

/// Result of `ControlBase::ExtractCoords`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coords {
    pub x: f32,
    pub y: f32,
    /// `SIZE_FULL_X | SIZE_FULL_Y` for the axes written as `F`/`Full`.
    pub flags: u32,
}

/// `ControlBase::ExtractCoords`: two numbers; a token that starts with `F` takes the screen size.
/// `previous` is kept for an axis that has no token (the exe does not define that case).
pub fn extract_coords(text: &str, screen: (i32, i32), previous: (f32, f32)) -> Coords {
    let mut out = Coords {
        x: previous.0,
        y: previous.1,
        flags: 0,
    };
    if trimmed(text).is_empty() {
        return out;
    }
    let (first, second) = first_two(text);
    if first.starts_with(['F', 'f']) {
        out.x = screen.0 as f32;
        out.flags = flags::SIZE_FULL_X;
    } else {
        out.x = to_float(first);
    }
    if let Some(s) = second {
        if s.starts_with(['F', 'f']) {
            out.y = screen.1 as f32;
            out.flags |= flags::SIZE_FULL_Y;
        } else {
            out.y = to_float(s);
        }
    }
    out
}

/// `ControlBase::LoadAnchorFlags`: first token `R`ight or `C`enter, second `B`ottom or `C`enter.
pub fn anchor_flags(text: &str) -> u32 {
    let (first, second) = first_two(text);
    let mut f = if first.starts_with(['R', 'r']) {
        flags::H_RIGHT
    } else if first.starts_with(['C', 'c']) {
        flags::H_CENTER
    } else {
        0
    };
    let second = second.unwrap_or("");
    if second.starts_with(['B', 'b']) {
        f |= flags::V_BOTTOM;
    } else if second.starts_with(['C', 'c']) {
        f |= flags::V_CENTER;
    }
    f
}

/// `ControlBase::LoadAnchorSideFlags`: `O`ut on an axis outers the control on that axis.
pub fn anchor_side_flags(text: &str) -> u32 {
    let (first, second) = first_two(text);
    let mut f = 0;
    if first.starts_with(['O', 'o']) {
        f |= flags::H_OUTER;
    }
    if second.unwrap_or("").starts_with(['O', 'o']) {
        f |= flags::V_OUTER;
    }
    f
}

/// `StackControl::BuildFromXML`: `StackGrowth`/`WrapGrowth` by the first letter.
/// Bottom/Down 0, Top/Up 1, Left 2, Right 3; anything else keeps the old value.
pub fn growth_dir(text: &str) -> Option<i32> {
    let t = trimmed(text);
    match t.chars().next()?.to_ascii_lowercase() {
        'b' | 'd' => Some(0),
        't' | 'u' => Some(1),
        'l' => Some(2),
        'r' => Some(3),
        _ => None,
    }
}

/// Colour text split into 0..1 floats by `cvUISkinColors::ColorFromXML`: `r,g,b[,a]` in 0..255.
pub fn color_from_xml(text: &str) -> Option<[f32; 4]> {
    let t = trimmed(text);
    if t.is_empty() {
        return None;
    }
    let mut c = [0.0f32, 0.0, 0.0, 255.0];
    for (slot, tok) in c.iter_mut().zip(t.split(SEPARATORS)) {
        if !tok.is_empty() {
            let v = to_int(tok).max(0);
            *slot = if v < 256 { v as f32 } else { 255.0 };
        }
    }
    Some([c[0] / 255.0, c[1] / 255.0, c[2] / 255.0, c[3] / 255.0])
}

/// Splits a colour value into its name token and the optional alpha token
/// (`UIManager::ExtractColor`): `Black.128` is ("Black", 128).
pub fn color_name_alpha(text: &str) -> Option<(&str, Option<i32>)> {
    let t = trimmed(text);
    if t.is_empty() {
        return None;
    }
    let (name, alpha) = first_two(t);
    Some((name, alpha.map(to_int)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: (i32, i32) = (1920, 1080);

    fn xy(text: &str) -> (f32, f32, u32) {
        let c = extract_coords(text, SCREEN, (0.0, 0.0));
        (c.x, c.y, c.flags)
    }

    #[test]
    fn extract_coords_separators() {
        // ControlBase::ExtractCoords: any of " .,x*|" separates the two numbers.
        assert_eq!(xy("256,30"), (256.0, 30.0, 0));
        assert_eq!(xy("256 30"), (256.0, 30.0, 0));
        assert_eq!(xy("256.30"), (256.0, 30.0, 0));
        assert_eq!(xy("256x30"), (256.0, 30.0, 0));
        assert_eq!(xy("256*30"), (256.0, 30.0, 0));
        assert_eq!(xy("256|30"), (256.0, 30.0, 0));
        assert_eq!(xy("  -3, -1 "), (-3.0, -1.0, 0));
    }

    #[test]
    fn extract_coords_skips_empty_second_tokens() {
        assert_eq!(xy("60,+10"), (60.0, 10.0, 0));
        assert_eq!(xy("60, +10"), (60.0, 10.0, 0));
        assert_eq!(xy("15;1"), (15.0, 0.0, 0));
    }

    #[test]
    fn extract_coords_full_takes_screen_size() {
        assert_eq!(xy("Full.32"), (1920.0, 32.0, flags::SIZE_FULL_X));
        assert_eq!(
            xy("F,F"),
            (1920.0, 1080.0, flags::SIZE_FULL_X | flags::SIZE_FULL_Y)
        );
        assert_eq!(xy("100,full"), (100.0, 1080.0, flags::SIZE_FULL_Y));
    }

    #[test]
    fn extract_coords_empty_keeps_previous() {
        let c = extract_coords("", SCREEN, (7.0, 8.0));
        assert_eq!((c.x, c.y), (7.0, 8.0));
        // A single token sets x only.
        let c = extract_coords("30", SCREEN, (7.0, 8.0));
        assert_eq!((c.x, c.y), (30.0, 8.0));
    }

    #[test]
    fn anchors_by_first_letter() {
        // ControlBase::LoadAnchorFlags
        assert_eq!(anchor_flags("L,T"), 0);
        assert_eq!(anchor_flags("R,B"), flags::H_RIGHT | flags::V_BOTTOM);
        assert_eq!(anchor_flags("C,C"), flags::H_CENTER | flags::V_CENTER);
        assert_eq!(anchor_flags("c.c"), flags::H_CENTER | flags::V_CENTER);
        assert_eq!(
            anchor_flags("Right,Center"),
            flags::H_RIGHT | flags::V_CENTER
        );
        assert_eq!(anchor_flags("L,B"), flags::V_BOTTOM);
        assert_eq!(anchor_flags(""), 0);
    }

    #[test]
    fn anchor_side_outer() {
        // ControlBase::LoadAnchorSideFlags
        assert_eq!(anchor_side_flags("O.O"), flags::H_OUTER | flags::V_OUTER);
        assert_eq!(anchor_side_flags("O.I"), flags::H_OUTER);
        assert_eq!(anchor_side_flags("i.O"), flags::V_OUTER);
        assert_eq!(anchor_side_flags("I,I"), 0);
    }

    #[test]
    fn booleans() {
        assert!(parse_bool("True"));
        assert!(parse_bool("true"));
        assert!(parse_bool("1"));
        assert!(!parse_bool("0"));
        assert!(!parse_bool("False"));
        assert!(!parse_bool(""));
    }

    #[test]
    fn colors() {
        // cvUISkinColors::ColorFromXML
        assert_eq!(color_from_xml("255,0,0,255"), Some([1.0, 0.0, 0.0, 1.0]));
        assert_eq!(color_from_xml("0,0,0"), Some([0.0, 0.0, 0.0, 1.0]));
        assert_eq!(
            color_from_xml("300,128,0"),
            Some([1.0, 128.0 / 255.0, 0.0, 1.0])
        );
        assert_eq!(color_from_xml("  "), None);
        // UIManager::ExtractColor: name and alpha
        assert_eq!(color_name_alpha("Black.128"), Some(("Black", Some(128))));
        assert_eq!(color_name_alpha("Red,220"), Some(("Red", Some(220))));
        assert_eq!(color_name_alpha("Red"), Some(("Red", None)));
    }

    #[test]
    fn growth() {
        assert_eq!(growth_dir("Bottom"), Some(0));
        assert_eq!(growth_dir("Down"), Some(0));
        assert_eq!(growth_dir("B"), Some(0));
        assert_eq!(growth_dir("Top"), Some(1));
        assert_eq!(growth_dir("Left"), Some(2));
        assert_eq!(growth_dir("Right"), Some(3));
        assert_eq!(growth_dir("Reflect percent"), Some(3));
        assert_eq!(growth_dir("???"), None);
    }
}
