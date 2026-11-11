//! Localization: the skin's own texts (the package's `L10N` table) and the game's texts (the host's
//! localizer).
//!
//! A skin text is a template in the syntax of the game's text engine (`{1_Num} {1_Num: plural 1?turn;
//! other?turns;}`), looked up by key in the current language, else in the skin's first language. It is
//! composed by the game's localizer when the host lent one (so numbers and plurals follow the game's
//! language), otherwise by a localizer of the runtime's own with no game texts. A model text that is a key
//! (`Text::Key`) and `loc(key, ...)` in an expression read the skin's table first, then the game's.

use std::rc::Rc;

use ::loc::{Arg, LanguageInfo, Localizer, MapTable};
use ui2_model::Text;
use ui2_package::Package;
use ui2_package::profile::expr::Value;

/// A piece of a text with the game's markup taken out: its text (`[NEWLINE]` is a line feed) and the color
/// tag around it (`POSITIVE_TEXT` of `[COLOR_POSITIVE_TEXT]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Piece {
    pub text: String,
    pub color: Option<String>,
}

/// The game's markup in a shown text: `[NEWLINE]` breaks the line, `[COLOR_X]...[ENDCOLOR]` colors a piece,
/// `[ICON_X]` is cut out (the flag says one was). Other bracketed words stay as they are. A text without
/// markup is one piece.
pub(crate) fn split_markup(s: &str) -> (Vec<Piece>, bool) {
    let mut pieces = Vec::new();
    let mut cur = String::new();
    let mut color: Option<String> = None;
    let mut icons = false;
    let mut rest = s;
    while let Some(open) = rest.find('[') {
        cur.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find(']') else {
            cur.push_str(&rest[open..]);
            rest = "";
            break;
        };
        let tag = &after[..close];
        let word = tag
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
        match tag {
            "NEWLINE" => cur.push('\n'),
            "ENDCOLOR" => {
                if !cur.is_empty() {
                    pieces.push(Piece {
                        text: std::mem::take(&mut cur),
                        color: color.take(),
                    });
                }
                color = None;
            }
            t if word && t.starts_with("COLOR_") => {
                if !cur.is_empty() {
                    pieces.push(Piece {
                        text: std::mem::take(&mut cur),
                        color: color.clone(),
                    });
                }
                color = Some(t["COLOR_".len()..].to_string());
            }
            t if word && t.starts_with("ICON_") => {
                // SIMPLIFIED(SIMP-228): icons in game texts ([ICON_*]) are cut out and reported once; the profile allows no pictures inside text
                icons = true;
            }
            _ => cur.push_str(&rest[open..open + close + 2]),
        }
        rest = &after[close + 1..];
    }
    cur.push_str(rest);
    if !cur.is_empty() || pieces.is_empty() {
        pieces.push(Piece { text: cur, color });
    }
    (pieces, icons)
}

/// The color token of a markup color: `--ui-loc-<x>` (lower case, `_` as `-`), as a root token or a
/// synonym; `None` when the skin has none (the text keeps its color).
pub(crate) fn markup_color(pkg: &Package, tag: &str) -> Option<ui2_package::style::ColorV> {
    let name = format!("--ui-loc-{}", tag.to_ascii_lowercase().replace('_', "-"));
    let id = pkg.str_id(&name)?;
    let tokens = pkg.tokens();
    let tok = tokens.alias(id).or_else(|| {
        tokens
            .roots
            .iter()
            .position(|(n, _)| *n == id)
            .map(|i| ui2_package::TokenId(i as u32))
    })?;
    Some(ui2_package::style::ColorV::Tok(tok))
}

/// The localization state of a runtime.
pub(crate) struct SkinLoc {
    /// The host's localizer.
    game: Option<Rc<Localizer>>,
    /// Composes skin templates when the host lent no localizer.
    own: Localizer,
    /// The index of the current language in the package's languages.
    lang: usize,
}

impl std::fmt::Debug for SkinLoc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkinLoc")
            .field("game", &self.game.is_some())
            .field("lang", &self.lang)
            .finish()
    }
}

/// An expression value as an argument of the text engine.
fn arg(v: &Value) -> Arg {
    match v {
        Value::Int(i) => match i32::try_from(*i) {
            Ok(n) => Arg::Int(n),
            Err(_) => Arg::Int64(*i),
        },
        Value::Float(f) => Arg::Double(*f),
        other => Arg::Str(ui2_package::profile::expr::render(other)),
    }
}

impl SkinLoc {
    /// The localization of a package, in the language of the game's localizer when the skin has it.
    pub fn new(pkg: &Package, game: Option<Rc<Localizer>>) -> Self {
        let mut s = Self {
            game: None,
            own: Localizer::new(LanguageInfo::en_us(), Box::new(MapTable::new())),
            lang: 0,
        };
        s.set_game(pkg, game);
        s
    }

    /// Changes the game's localizer (and with it the language).
    pub fn set_game(&mut self, pkg: &Package, game: Option<Rc<Localizer>>) {
        let want = game.as_ref().map(|l| l.current_language().type_.clone());
        self.lang = want
            .and_then(|w| pkg.languages().iter().position(|l| *l == w))
            .unwrap_or(0);
        self.game = game;
    }

    fn composer(&self) -> &Localizer {
        self.game.as_deref().unwrap_or(&self.own)
    }

    /// The template of a skin key: the current language, else the first one.
    fn template<'a>(&self, pkg: &'a Package, key: &str) -> Option<&'a str> {
        let langs = pkg.languages();
        langs
            .get(self.lang)
            .and_then(|l| pkg.l10n(l, key))
            .or_else(|| langs.first().and_then(|l| pkg.l10n(l, key)))
    }

    /// A skin text with its arguments; `Err` with the key itself when the skin has no such key.
    pub fn skin_text(&self, pkg: &Package, key: &str, args: &[Value]) -> Result<String, String> {
        match self.template(pkg, key) {
            Some(t) => {
                let args: Vec<Arg> = args.iter().map(arg).collect();
                Ok(self.composer().compose(t, &args))
            }
            None => Err(key.to_string()),
        }
    }

    /// `loc(key, args...)` of an expression: a skin text, else a game text (a missing game key shows the
    /// key, as the game does).
    pub fn call(&self, pkg: &Package, key: &str, args: &[Value]) -> String {
        if let Ok(s) = self.skin_text(pkg, key, args) {
            return s;
        }
        let args: Vec<Arg> = args.iter().map(arg).collect();
        match &self.game {
            Some(g) => g.convert(key, &args),
            None => key.to_string(),
        }
    }

    /// The shown text of a model text.
    pub fn model_text(&self, pkg: &Package, t: &Text) -> String {
        match t {
            Text::Raw(s) => s.to_string(),
            Text::Key { key, args } => {
                let args: Vec<Value> = args.iter().map(|a| Value::Str(a.to_string())).collect();
                self.call(pkg, key, &args)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(text: &str, color: Option<&str>) -> Piece {
        Piece {
            text: text.into(),
            color: color.map(str::to_string),
        }
    }

    #[test]
    fn markup_splits_colors_breaks_lines_and_cuts_icons() {
        assert_eq!(split_markup("plain"), (vec![p("plain", None)], false));
        assert_eq!(split_markup(""), (vec![p("", None)], false));
        let (pieces, icons) =
            split_markup("Gold[NEWLINE][COLOR_POSITIVE_TEXT]+5[ENDCOLOR] per turn [ICON_GOLD]");
        assert!(icons);
        assert_eq!(
            pieces,
            vec![
                p("Gold\n", None),
                p("+5", Some("POSITIVE_TEXT")),
                p(" per turn ", None)
            ]
        );
        // Unknown or unclosed brackets stay as text.
        assert_eq!(
            split_markup("[Esc] to close [x"),
            (vec![p("[Esc] to close [x", None)], false)
        );
    }
}
