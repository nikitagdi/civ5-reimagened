//! Plural rules 1-16 of `Localization::PluralRuleN` (Civ5XP 0x08c655be..0x08c65953).
//!
//! The form numbers are 1-based, exactly as the exe returns them. Division and remainder are signed C
//! `int` operations, so negative counts behave as in the exe.

/// Number of plural forms of rule `rule` (`PluralRuleN::GetMaxPluralForms`); `None` for an unknown rule.
pub fn max_forms(rule: u32) -> Option<u32> {
    // re: 0x08c655be PluralRule1..16 GetMaxPluralForms
    Some(match rule {
        1 => 1,
        2 | 3 | 16 => 2,
        4..=10 | 15 => 3,
        11 | 14 => 4,
        12 => 5,
        13 => 6,
        _ => return None,
    })
}

/// Plural form (1-based) of `n` under `rule` (`Localization::GetPluralityRule(rule)->GetPluralForm(n)`).
/// An unknown rule has no rule object in the exe; callers see `None`.
pub fn plural_form(rule: u32, n: i32) -> Option<u32> {
    // re: 0x08c65430 GetPluralityRule: rule number -> PluralRule<number>, anything else -> null
    let m10 = n.wrapping_rem(10);
    let m100 = n.wrapping_rem(100);
    let f = match rule {
        // re: 0x08c655c4 PluralRule1::GetPluralForm
        1 => 1,
        // re: 0x08c655d0 PluralRule2::GetPluralForm: English, 1 -> 1, everything else -> 2
        2 => {
            if n == 1 {
                1
            } else {
                2
            }
        }
        // re: 0x08c655e4 PluralRule3::GetPluralForm: French, 0 and 1 -> 1
        3 => {
            if n == 0 || n == 1 {
                1
            } else {
                2
            }
        }
        // re: 0x08c65602 PluralRule4::GetPluralForm: Latvian
        4 => {
            if n == 0 {
                1
            } else if m10 == 1 && n != 11 {
                2
            } else {
                3
            }
        }
        // re: 0x08c65648 PluralRule5::GetPluralForm
        5 => match n {
            1 => 1,
            2 => 2,
            _ => 3,
        },
        // re: 0x08c6566e PluralRule6::GetPluralForm: Romanian
        6 => {
            if n == 0 {
                2
            } else if n == 1 {
                1
            } else if m100 > 19 {
                3
            } else {
                2
            }
        }
        // re: 0x08c656ae PluralRule7::GetPluralForm
        7 => {
            if m10 == 1 {
                1
            } else if m10 == 0 {
                2
            } else {
                3
            }
        }
        // re: 0x08c656ea PluralRule8::GetPluralForm: Russian-like; the exe tests the whole count against
        // 12..=14, not the remainder of 100 (112 selects form 2)
        8 => {
            if m10 == 1 && n != 11 {
                1
            } else if (2..=4).contains(&m10) && !(12..=14).contains(&n) {
                2
            } else {
                3
            }
        }
        // re: 0x08c6573c PluralRule9::GetPluralForm: Czech
        9 => {
            if n == 1 {
                1
            } else if (2..=4).contains(&n) {
                2
            } else {
                3
            }
        }
        // re: 0x08c65760 PluralRule10::GetPluralForm: Polish
        10 => {
            if n == 1 {
                1
            } else if !(2..=4).contains(&m10) || (12..=14).contains(&n) {
                3
            } else {
                2
            }
        }
        // re: 0x08c657aa PluralRule11::GetPluralForm
        11 => {
            if m10 == 1 {
                1
            } else if m10 == 2 {
                2
            } else if (3..=4).contains(&m10) {
                3
            } else {
                4
            }
        }
        // re: 0x08c657f2 PluralRule12::GetPluralForm
        12 => {
            if n == 1 {
                1
            } else if n == 2 {
                2
            } else if (3..=6).contains(&n) {
                3
            } else if (7..=10).contains(&n) {
                4
            } else {
                5
            }
        }
        // re: 0x08c6582e PluralRule13::GetPluralForm: Arabic
        13 => {
            if n == 1 {
                1
            } else if n == 2 {
                2
            } else if (3..=10).contains(&m100) {
                3
            } else if m100 > 10 {
                4
            } else if n == 0 {
                6
            } else {
                5
            }
        }
        // re: 0x08c65892 PluralRule14::GetPluralForm: Maltese
        14 => {
            if n == 1 {
                1
            } else if n == 0 || m100 < 11 {
                2
            } else if m100 <= 19 {
                3
            } else {
                4
            }
        }
        // re: 0x08c658de PluralRule15::GetPluralForm
        15 => match m10 {
            1 => 1,
            2 => 2,
            _ => 3,
        },
        // re: 0x08c6591c PluralRule16::GetPluralForm
        16 => {
            if m10 == 1 && n != 11 {
                1
            } else {
                2
            }
        }
        _ => return None,
    };
    Some(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_rule() {
        assert_eq!(plural_form(2, 1), Some(1));
        assert_eq!(plural_form(2, 0), Some(2));
        assert_eq!(plural_form(2, 2), Some(2));
        assert_eq!(plural_form(2, -1), Some(2));
    }

    #[test]
    fn russian_like_rule_8_quirk() {
        let forms: Vec<u32> = [1, 2, 5, 11, 21, 101, 112, 22]
            .iter()
            .map(|&n| plural_form(8, n).unwrap())
            .collect();
        // 112 -> remainder 2 and the exe excludes only the literal 12..=14: form 2.
        assert_eq!(forms, [1, 2, 3, 3, 1, 1, 2, 2]);
    }

    #[test]
    fn every_rule_stays_within_max_forms() {
        for rule in 1..=16 {
            let max = max_forms(rule).unwrap();
            for n in -50..=1000 {
                let f = plural_form(rule, n).unwrap();
                assert!((1..=max).contains(&f), "rule {rule} n {n} -> {f}");
            }
        }
        assert_eq!(plural_form(0, 1), None);
        assert_eq!(plural_form(17, 1), None);
        assert_eq!(max_forms(17), None);
    }

    #[test]
    fn sample_values_of_other_rules() {
        assert_eq!(plural_form(3, 0), Some(1));
        assert_eq!(plural_form(3, 2), Some(2));
        assert_eq!(plural_form(4, 21), Some(2));
        assert_eq!(plural_form(5, 2), Some(2));
        assert_eq!(plural_form(6, 0), Some(2));
        assert_eq!(plural_form(6, 20), Some(3));
        assert_eq!(plural_form(9, 3), Some(2));
        assert_eq!(plural_form(10, 22), Some(2));
        assert_eq!(plural_form(10, 12), Some(3));
        assert_eq!(plural_form(11, 4), Some(3));
        assert_eq!(plural_form(12, 8), Some(4));
        assert_eq!(plural_form(13, 0), Some(6));
        assert_eq!(plural_form(13, 5), Some(3));
        assert_eq!(plural_form(13, 11), Some(4));
        assert_eq!(plural_form(14, 15), Some(3));
        assert_eq!(plural_form(14, 25), Some(4));
        assert_eq!(plural_form(15, 2), Some(2));
        assert_eq!(plural_form(16, 21), Some(1));
    }
}
