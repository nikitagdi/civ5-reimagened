//! `Locale.Compare`: a simplified collation.
//!
//! SIMPLIFIED(SIMP-txt-2): the exe compares with an ICU collator (`Language::Compare`, 0x08c5affa).
//! This compares letters case- and accent-insensitively first, then accents, then case (lower before
//! upper, as ICU's default tertiary order does). Punctuation and spaces sort before digits, digits before
//! letters. Good enough for Latin-script sorting of the EUI lists; revisit for ru_RU.

use std::cmp::Ordering;

/// Base letter and accent flag for Latin-1 supplement and Latin Extended-A letters.
fn strip_accent(c: char) -> (char, bool) {
    let base = match c {
        '\u{c0}'..='\u{c5}' | '\u{e0}'..='\u{e5}' | '\u{100}'..='\u{105}' => 'a',
        '\u{c7}' | '\u{e7}' | '\u{106}'..='\u{10d}' => 'c',
        '\u{10e}'..='\u{111}' => 'd',
        '\u{c8}'..='\u{cb}' | '\u{e8}'..='\u{eb}' | '\u{112}'..='\u{11b}' => 'e',
        '\u{11c}'..='\u{123}' => 'g',
        '\u{124}'..='\u{127}' => 'h',
        '\u{cc}'..='\u{cf}' | '\u{ec}'..='\u{ef}' | '\u{128}'..='\u{131}' => 'i',
        '\u{134}' | '\u{135}' => 'j',
        '\u{136}' | '\u{137}' => 'k',
        '\u{139}'..='\u{142}' => 'l',
        '\u{d1}' | '\u{f1}' | '\u{143}'..='\u{148}' => 'n',
        '\u{d2}'..='\u{d6}' | '\u{d8}' | '\u{f2}'..='\u{f6}' | '\u{f8}' | '\u{14c}'..='\u{151}' => {
            'o'
        }
        '\u{154}'..='\u{159}' => 'r',
        '\u{15a}'..='\u{161}' => 's',
        '\u{162}'..='\u{167}' => 't',
        '\u{d9}'..='\u{dc}' | '\u{f9}'..='\u{fc}' | '\u{168}'..='\u{173}' => 'u',
        '\u{174}' | '\u{175}' => 'w',
        '\u{dd}' | '\u{fd}' | '\u{ff}' | '\u{176}'..='\u{178}' => 'y',
        '\u{179}'..='\u{17e}' => 'z',
        _ => return (c, false),
    };
    (base, true)
}

/// Primary key: class (punctuation < digit < letter) and the folded character.
fn primary(c: char) -> (u8, char) {
    let (base, _) = strip_accent(c);
    let lower = base.to_lowercase().next().unwrap_or(base);
    let class = if c.is_alphabetic() {
        2
    } else if c.is_numeric() {
        1
    } else {
        0
    };
    (class, lower)
}

/// -1, 0 or 1, like the integer `Locale.Compare` pushes.
pub fn compare(a: &str, b: &str) -> i32 {
    let key = |s: &str| s.chars().map(primary).collect::<Vec<_>>();
    match key(a).cmp(&key(b)) {
        Ordering::Less => return -1,
        Ordering::Greater => return 1,
        Ordering::Equal => {}
    }
    // Secondary: unaccented before accented.
    let accents = |s: &str| s.chars().map(|c| strip_accent(c).1).collect::<Vec<_>>();
    match accents(a).cmp(&accents(b)) {
        Ordering::Less => return -1,
        Ordering::Greater => return 1,
        Ordering::Equal => {}
    }
    // Tertiary: lower case before upper case.
    let upper = |s: &str| s.chars().map(char::is_uppercase).collect::<Vec<_>>();
    match upper(a).cmp(&upper(b)) {
        Ordering::Less => -1,
        Ordering::Greater => 1,
        Ordering::Equal => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_ignore_case_first() {
        assert_eq!(compare("apple", "Banana"), -1);
        assert_eq!(compare("Banana", "apple"), 1);
        assert_eq!(compare("abc", "abc"), 0);
    }

    #[test]
    fn case_breaks_ties_lower_first() {
        assert_eq!(compare("a", "A"), -1);
        assert_eq!(compare("A", "a"), 1);
    }

    #[test]
    fn accents_after_plain_letters() {
        assert_eq!(compare("resume", "r\u{e9}sum\u{e9}"), -1);
        assert_eq!(compare("\u{e9}", "f"), -1);
    }

    #[test]
    fn punctuation_digits_letters() {
        assert_eq!(compare("-a", "1"), -1);
        assert_eq!(compare("1", "a"), -1);
        assert_eq!(compare("ab", "abc"), -1);
    }
}
