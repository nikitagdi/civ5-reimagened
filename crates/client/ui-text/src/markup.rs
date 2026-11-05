//! `TextControl::ParseTextIntoBlocks` (Civ5XP 0x08699e96): the string becomes text, icon and newline blocks.
//!
//! Tags (only in parse mode): `[NEWLINE]`, `[ICON_<name>]`, `[COLOR_<name>]`, `[COLOR:r:g:b:a]` (0-255),
//! `[ENDCOLOR]`, `[/COLOR]`, `[[` for a literal `[`. Any other tag, and an icon or color that is not in the
//! tables, is dropped with no output. (`[SPACE]` and `[TAB]` never get here: `TextSystem` expands them first.)

use crate::FontId;
use crate::icons::Rgba;

/// `TextControlBlock`: a run of text, an icon, or a line break.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    /// Byte range of the run in the source string.
    pub offset: usize,
    pub len: usize,
    /// Colors of the three layers at this point.
    pub colors: [Rgba; 3],
    /// A line starts at this block.
    pub newline: bool,
    pub style: u8,
    /// Icon font and glyph code; `None` for text.
    pub icon: Option<(FontId, u32)>,
}

/// Looks up `ICON_*` names (loading the icon font on demand) and `COLOR_*` names.
pub trait TagResolver {
    fn icon(&mut self, name: &str) -> Option<(FontId, u32)>;
    fn color(&mut self, name: &str) -> Option<Rgba>;
}

fn atoi3(s: &str) -> i32 {
    // strncpy(buf, s, min(len, 3)); atoi(buf)
    let t: String = s.chars().take(3).collect();
    let digits: String = t
        .trim_start()
        .chars()
        .enumerate()
        .take_while(|(i, c)| c.is_ascii_digit() || (*i == 0 && (*c == '-' || *c == '+')))
        .map(|(_, c)| c)
        .collect();
    digits.parse().unwrap_or(0)
}

/// `[COLOR:r:g:b:a]`: each field is the text up to the next `:` (the last up to `]`), at most 3 characters.
fn parse_color_fields(rest: &str) -> Rgba {
    let mut fields = rest.splitn(4, ':');
    let mut next = || atoi3(fields.next().unwrap_or("")) as u8;
    [next(), next(), next(), next()]
}

/// Splits `text` into blocks.
///
/// `colors` are the control's three layer colors; `style` the style row of every text block.
pub fn parse_blocks(
    text: &str,
    colors: [Rgba; 3],
    style: u8,
    parse_mode: bool,
    resolver: &mut dyn TagResolver,
) -> Vec<Block> {
    let s = text.as_bytes();
    let len = s.len();
    let mut blocks: Vec<Block> = Vec::new();
    // Layer colors in force; `[COLOR]` changes layer 0 only.
    let mut cur = colors;
    // Color stack: slot 0 is the control's layer-0 color (the exe seeds it before the loop).
    let mut stack: Vec<Rgba> = vec![colors[0]];
    let mut sp: usize = 0;

    let mut run_start = 0usize;
    let mut run_len = 0usize;
    let mut i = 0usize;
    let text_block = |offset: usize, len: usize, cur: [Rgba; 3]| Block {
        offset,
        len,
        colors: cur,
        newline: false,
        style,
        icon: None,
    };

    while i <= len {
        let c = if i < len { s[i] } else { 0 };
        if c == b'[' && parse_mode {
            if i + 1 < len && s[i + 1] == b'[' {
                // "[[": the text so far plus one '[', then the second '[' is skipped
                blocks.push(text_block(run_start, run_len + 1, cur));
                run_start = i + 2;
                run_len = 0;
                i += 2;
                continue;
            }
            if run_len != 0 {
                blocks.push(text_block(run_start, run_len, cur));
            }
            let tag_end = s[i..]
                .iter()
                .position(|&b| b == b']')
                .map_or(len, |p| i + p);
            let name = &text[i + 1..tag_end.min(len).max(i + 1)];
            if name == "NEWLINE" {
                blocks.push(Block {
                    offset: tag_end + 1,
                    len: 0,
                    colors: cur,
                    newline: true,
                    style,
                    icon: None,
                });
            } else if name.starts_with("COLOR") && name.len() >= 5 {
                match name.as_bytes().get(5) {
                    Some(b':') => {
                        let c = parse_color_fields(&name[6..]);
                        push_color(&mut stack, &mut sp, c);
                        cur[0] = c;
                    }
                    Some(b'_') => {
                        if let Some(c) = resolver.color(name) {
                            push_color(&mut stack, &mut sp, c);
                            cur[0] = c;
                        }
                    }
                    // RE-PENDING(0x08699e96): [COLOR0..3:r:g:b:a] writes into a local table the decompiler
                    // does not resolve; no text in the game data uses it.
                    _ => {}
                }
            } else if name == "ENDCOLOR" {
                cur = colors;
            } else if name == "/COLOR" {
                sp = sp.saturating_sub(1);
                cur[0] = stack[sp];
            } else if name.starts_with("ICON")
                && name.as_bytes().get(4) == Some(&b'_')
                && let Some(icon) = resolver.icon(name)
            {
                blocks.push(Block {
                    offset: 0,
                    len: 0,
                    colors: cur,
                    newline: false,
                    style,
                    icon: Some(icon),
                });
            }
            run_len = 0;
            run_start = tag_end + 1;
            i = tag_end + 1;
            continue;
        }
        if i == len {
            // the last run is always pushed, empty or not
            blocks.push(text_block(run_start, run_len, cur));
            break;
        }
        run_len += 1;
        i += 1;
    }
    blocks
}

fn push_color(stack: &mut Vec<Rgba>, sp: &mut usize, c: Rgba) {
    *sp += 1;
    if stack.len() <= *sp {
        stack.resize(*sp + 1, c);
    }
    stack[*sp] = c;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct R;
    impl TagResolver for R {
        fn icon(&mut self, name: &str) -> Option<(FontId, u32)> {
            (name == "ICON_GOLD").then_some((FontId(7), 4))
        }
        fn color(&mut self, name: &str) -> Option<Rgba> {
            (name == "COLOR_RED").then_some([255, 0, 0, 255])
        }
    }

    const W: [Rgba; 3] = [[255; 4], [0, 0, 0, 255], [10, 20, 30, 255]];

    fn texts(text: &str, blocks: &[Block]) -> Vec<String> {
        blocks
            .iter()
            .map(|b| text[b.offset..b.offset + b.len].to_string())
            .collect()
    }

    #[test]
    fn plain_text_is_one_block() {
        let b = parse_blocks("hello", W, 2, true, &mut R);
        assert_eq!(b.len(), 1);
        assert_eq!((b[0].offset, b[0].len, b[0].style), (0, 5, 2));
        assert_eq!(parse_blocks("", W, 0, true, &mut R).len(), 1);
    }

    #[test]
    fn newline_and_icon_blocks() {
        let t = "a[NEWLINE]b[ICON_GOLD]c";
        let b = parse_blocks(t, W, 0, true, &mut R);
        assert_eq!(texts(t, &b), ["a", "", "b", "", "c"]);
        assert!(b[1].newline && !b[0].newline && !b[2].newline);
        assert_eq!(b[3].icon, Some((FontId(7), 4)));
    }

    #[test]
    fn unknown_and_unresolved_tags_vanish() {
        let t = "a[NOPE]b[ICON_NOPE]c[COLOR_NOPE]d";
        let b = parse_blocks(t, W, 0, true, &mut R);
        assert_eq!(texts(t, &b).concat(), "abcd");
        assert!(b.iter().all(|b| b.icon.is_none()));
    }

    #[test]
    fn colors_change_layer_zero_only() {
        let t = "a[COLOR_RED]b[ENDCOLOR]c[COLOR:1:2:3:4]d[/COLOR]e";
        let b = parse_blocks(t, W, 0, true, &mut R);
        assert_eq!(texts(t, &b), ["a", "b", "c", "d", "e"]);
        assert_eq!(b[0].colors, W);
        assert_eq!(b[1].colors, [[255, 0, 0, 255], W[1], W[2]]);
        assert_eq!(b[2].colors, W);
        assert_eq!(b[3].colors, [[1, 2, 3, 4], W[1], W[2]]);
        // [/COLOR] pops to the previous stack slot, which holds the earlier [COLOR_RED]
        assert_eq!(b[4].colors[0], [255, 0, 0, 255]);
    }

    #[test]
    fn pop_without_push_gives_the_base_color() {
        let t = "[/COLOR]x";
        let b = parse_blocks(t, W, 0, true, &mut R);
        assert_eq!(b.last().unwrap().colors, W);
    }

    #[test]
    fn double_bracket_is_a_literal() {
        let t = "a[[b";
        let b = parse_blocks(t, W, 0, true, &mut R);
        assert_eq!(texts(t, &b).concat(), "a[b");
    }

    #[test]
    fn parse_mode_off_keeps_tags() {
        let t = "a[NEWLINE]b";
        let b = parse_blocks(t, W, 0, false, &mut R);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].len, t.len());
    }

    #[test]
    fn color_fields_clip_to_three_characters() {
        let t = "[COLOR:255:60:60:255]x";
        let b = parse_blocks(t, W, 0, true, &mut R);
        assert_eq!(b.last().unwrap().colors[0], [255, 60, 60, 255]);
    }
}
