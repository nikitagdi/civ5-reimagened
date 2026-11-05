//! Chat of a game: the text is cleaned here, the lines go out through the seat pipelines
//! (each keeps the 200 lines its seat saw, NP-D16). There is no chat in a room before the start.

use protocol::{ChatTo, PlayerId};

/// A line is cut to this many characters.
pub const MAX_CHARS: usize = 500;

/// Unicode category Cf (format characters): they hide or reorder text.
fn is_format(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{0600}'..='\u{0605}'
            | '\u{061C}'
            | '\u{06DD}'
            | '\u{070F}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{110BD}'
            | '\u{E0001}'
            | '\u{E0020}'..='\u{E007F}'
    )
}

/// The text of a line as the game sends it: no Cc and Cf characters, cut to [`MAX_CHARS`], blanks at
/// the ends trimmed. `None` for a line with nothing left.
pub fn clean(text: &str) -> Option<String> {
    let kept: String = text
        .chars()
        .filter(|c| !c.is_control() && !is_format(*c))
        .take(MAX_CHARS)
        .collect();
    let kept = kept.trim();
    (!kept.is_empty()).then(|| kept.to_owned())
}

/// Seats that get a line sent by `from` to `to`; `humans` are the human seats of the game. `Err`: the
/// text for the sender's journal (an AI or a closed slot is no addressee).
pub fn recipients(
    to: ChatTo,
    from: PlayerId,
    humans: &[PlayerId],
) -> Result<Vec<PlayerId>, &'static str> {
    match to {
        ChatTo::All => Ok(humans.to_vec()),
        ChatTo::Player(p) if p == from => Ok(vec![from]),
        ChatTo::Player(p) if humans.contains(&p) => Ok(vec![from, p]),
        ChatTo::Player(_) => Err("That player is not a human in this game"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_removes_control_and_format_characters() {
        assert_eq!(
            clean("  hi\u{7}\u{200B} there\n").as_deref(),
            Some("hi there")
        );
        assert_eq!(clean("a\u{202E}b").as_deref(), Some("ab"));
        assert_eq!(clean("привет").as_deref(), Some("привет"));
    }

    #[test]
    fn clean_cuts_and_drops_empty() {
        let long = "я".repeat(600);
        assert_eq!(clean(&long).unwrap().chars().count(), MAX_CHARS);
        assert_eq!(clean(" \u{200B}\t "), None);
        assert_eq!(clean(""), None);
    }

    #[test]
    fn recipients_follow_the_addressee() {
        let humans = [0, 2, 3];
        assert_eq!(recipients(ChatTo::All, 0, &humans), Ok(vec![0, 2, 3]));
        assert_eq!(recipients(ChatTo::Player(2), 0, &humans), Ok(vec![0, 2]));
        assert_eq!(recipients(ChatTo::Player(0), 0, &humans), Ok(vec![0]));
        assert!(recipients(ChatTo::Player(1), 0, &humans).is_err());
    }
}
