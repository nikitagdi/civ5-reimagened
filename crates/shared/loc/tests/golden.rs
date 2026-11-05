//! Golden tests of the template grammar on hand-written tables (no game files needed).

use loc::{Arg, LanguageInfo, Localizer, MapTable};

fn table() -> MapTable {
    let mut t = MapTable::new();
    t.insert_text("TXT_KEY_HELLO", "Hello");
    t.insert_text("TXT_KEY_NUM", "{1_Num} turns");
    t.insert_text("TXT_KEY_NAME", "Name: {1_Name}");
    t.insert_text("TXT_KEY_ATNAME", "Name: {@1_Name}");
    t.insert_text("TXT_KEY_NESTED", "[{TXT_KEY_HELLO}] and {TXT_KEY_NUM}");
    t.insert_text("TXT_KEY_PLURAL", "{1_N} {1_N: plural 1?Rank; other?Ranks;}");
    t.insert_text("TXT_KEY_HAS", "{1_Num: plural 1?has; 2?have;}");
    t.insert_text("TXT_KEY_FMT", "{1_Num: number \"#.##\"}");
    t.insert_text(
        "TXT_KEY_FMT_SIGNED",
        "{1_Num: number \"'+'##.##;'[COLOR_RED]-'##.##\"} Gold",
    );
    t.insert_text("TXT_KEY_PERCENT", "{1: percent}");
    t.insert_text("TXT_KEY_UPPER", "{1: upper} {1: lower} {1: title}");
    t.insert_text("TXT_KEY_TEXTKEY", "{1_Civ:textkey}");
    t.insert_text("TXT_KEY_ROMAN", "{1: roman}");
    t.insert_text("TXT_KEY_ORD", "{1: ordinal} {1: spellout}");
    // the real grammar helper
    t.insert_text(
        "TXT_KEY_GRAMMAR_A_AN",
        "{1: gender *:an?an {1}; *:the?the {1}; *:no_article?{1}; other?a {1};}",
    );
    t.insert_text(
        "TXT_KEY_GRAMMAR_UPPER_A_AN",
        "{1: gender *:an?An {1}; *:the?The {1}; *:no_article?{1}; other?A {1};}",
    );
    t.insert_text(
        "TXT_KEY_BUILT",
        "You built {TXT_KEY_GRAMMAR_A_AN << {@1_Name}}.",
    );
    t.insert_text(
        "TXT_KEY_BUILT_RAW",
        "You built {TXT_KEY_GRAMMAR_A_AN << {1_Name}}.",
    );
    t.insert(
        "TXT_KEY_UNIT_ARCHER",
        "Archer",
        Some("neuter:an"),
        Some("1"),
    );
    t.insert(
        "TXT_KEY_UNIT_SPEARMAN",
        "Spearman",
        Some("masculine"),
        Some("1"),
    );
    t.insert(
        "TXT_KEY_WONDER_GREAT_WALL",
        "Great Wall",
        Some("neuter:the"),
        Some("1"),
    );
    t.insert(
        "TXT_KEY_CITY_ROME",
        "Rome",
        Some("neuter:no_article"),
        Some("1"),
    );
    t.insert("TXT_KEY_UNIT_KESHIK", "Keshik|Keshiks", None, Some("1|2"));
    t.insert("TXT_KEY_LEADER_X", "Queen", Some("feminine"), None);
    t.insert_text(
        "TXT_KEY_TITLE",
        "{@1: gender feminine?Empress; other?Emperor;}",
    );
    t.insert_text("TXT_KEY_BAD_ARG", "{3_Missing}");
    t.insert_text("TXT_KEY_BAD_BRACE", "oops {1");
    t.insert_text("TXT_KEY_CYCLE", "{TXT_KEY_CYCLE}");
    t.insert_text("TXT_KEY_FORMS_EXPLICIT", "{TXT_KEY_UNIT_KESHIK[2]}");
    t.insert_text("TXT_KEY_FORMS_PLURAL", "{TXT_KEY_UNIT_KESHIK *{1}}");
    t.insert_text("TXT_KEY_ICON", "[ICON_GOLD] {1_Num} [NEWLINE]x");
    t
}

fn loc() -> Localizer {
    Localizer::new(LanguageInfo::en_us(), Box::new(table()))
}

fn d(v: f64) -> Arg {
    Arg::Double(v)
}

fn s(v: &str) -> Arg {
    Arg::Str(v.to_string())
}

#[test]
fn plain_and_positional() {
    let l = loc();
    assert_eq!(l.convert("TXT_KEY_HELLO", &[]), "Hello");
    assert_eq!(l.convert("TXT_KEY_NUM", &[d(5.0)]), "5 turns");
    assert_eq!(
        l.convert("TXT_KEY_NUM", &[Arg::Int(1234567)]),
        "1,234,567 turns"
    );
    assert_eq!(l.convert("TXT_KEY_NUM", &[d(2.5)]), "2.5 turns");
    assert_eq!(l.convert("TXT_KEY_NAME", &[s("Bob")]), "Name: Bob");
    assert_eq!(l.convert("TXT_KEY_ATNAME", &[s("Bob")]), "Name: Bob");
    assert_eq!(
        l.convert("TXT_KEY_NESTED", &[d(2.0)]),
        "[Hello] and 2 turns"
    );
}

#[test]
fn missing_key_is_its_own_text() {
    let l = loc();
    assert_eq!(l.convert("TXT_KEY_NOPE", &[]), "TXT_KEY_NOPE");
    assert_eq!(l.stats().missing, 1);
    assert_eq!(l.convert("TXT_KEY_NOPE", &[]), "TXT_KEY_NOPE");
    assert_eq!(l.take_warnings().len(), 1, "one warning per key");
    assert_eq!(l.convert("plain text, no key", &[]), "plain text, no key");
}

#[test]
fn errors_give_the_empty_string_without_panic() {
    let l = loc();
    // missing argument
    assert_eq!(l.convert("TXT_KEY_BAD_ARG", &[d(1.0)]), "");
    // broken braces
    assert_eq!(l.convert("TXT_KEY_BAD_BRACE", &[d(1.0)]), "");
    // '@' on a number
    assert_eq!(l.convert("TXT_KEY_ATNAME", &[d(1.0)]), "");
    // self-reference stops at the depth limit
    assert_eq!(l.convert("TXT_KEY_CYCLE", &[]), "");
    assert!(l.stats().bad_template >= 4);
    assert!(l.try_convert("TXT_KEY_BAD_BRACE", &[]).is_err());
}

#[test]
fn plural_transform() {
    let l = loc();
    assert_eq!(l.convert("TXT_KEY_PLURAL", &[d(1.0)]), "1 Rank");
    assert_eq!(l.convert("TXT_KEY_PLURAL", &[d(2.0)]), "2 Ranks");
    assert_eq!(l.convert("TXT_KEY_PLURAL", &[d(0.0)]), "0 Ranks");
    assert_eq!(l.convert("TXT_KEY_HAS", &[d(1.0)]), "has");
    assert_eq!(l.convert("TXT_KEY_HAS", &[d(5.0)]), "have");
}

#[test]
fn number_formats() {
    let l = loc();
    assert_eq!(l.convert("TXT_KEY_FMT", &[d(3.14649)]), "3.15");
    assert_eq!(l.convert("TXT_KEY_FMT_SIGNED", &[d(3.5)]), "+3.5 Gold");
    assert_eq!(
        l.convert("TXT_KEY_FMT_SIGNED", &[d(-3.5)]),
        "[COLOR_RED]-3.5 Gold"
    );
    assert_eq!(l.convert("TXT_KEY_PERCENT", &[d(0.256)]), "26%");
    assert_eq!(l.to_number(1234.5678, "#,###.#"), "1,234.6");
    assert_eq!(l.to_number(0.5, "#"), "0");
    assert_eq!(l.to_currency(12.5), "$12.50");
    assert_eq!(l.to_percent(0.5), "50%");
    assert_eq!(l.to_roman(14.0), "XIV");
    assert_eq!(l.to_ordinal(3.0), "3rd");
    assert_eq!(l.to_spellout(42.0), "forty-two");
}

#[test]
fn case_transforms_and_textkey() {
    let l = loc();
    assert_eq!(
        l.convert("TXT_KEY_UPPER", &[s("hELLO wORLD")]),
        "HELLO WORLD hello world Hello World"
    );
    // a text key argument is translated first
    assert_eq!(
        l.convert("TXT_KEY_UPPER", &[s("TXT_KEY_UNIT_ARCHER")]),
        "ARCHER archer Archer"
    );
    assert_eq!(
        l.convert("TXT_KEY_TEXTKEY", &[s("TXT_KEY_UNIT_SPEARMAN")]),
        "Spearman"
    );
    assert_eq!(
        l.convert("TXT_KEY_TEXTKEY", &[s("TXT_KEY_NOPE_AT_ALL")]),
        "TXT_KEY_NOPE_AT_ALL"
    );
    assert_eq!(l.to_upper("abc"), "ABC");
    assert_eq!(l.to_lower("ABC"), "abc");
    // a numeric argument to a string transform is an error
    assert_eq!(l.compose("{1: upper}", &[d(1.0)]), "");
}

#[test]
fn grammar_articles_via_injection() {
    let l = loc();
    // `an Archer`, `a Spearman`, `the Great Wall`, `Rome` (no article), unknown text falls to `a`
    assert_eq!(
        l.convert("TXT_KEY_BUILT", &[s("TXT_KEY_UNIT_ARCHER")]),
        "You built an Archer."
    );
    assert_eq!(
        l.convert("TXT_KEY_BUILT", &[s("TXT_KEY_UNIT_SPEARMAN")]),
        "You built a Spearman."
    );
    assert_eq!(
        l.convert("TXT_KEY_BUILT", &[s("TXT_KEY_WONDER_GREAT_WALL")]),
        "You built the Great Wall."
    );
    assert_eq!(
        l.convert("TXT_KEY_BUILT", &[s("TXT_KEY_CITY_ROME")]),
        "You built Rome."
    );
    assert_eq!(
        l.convert("TXT_KEY_BUILT", &[s("Swordsman")]),
        "You built a Swordsman."
    );
    // without '@' the argument is passed through and translated inside the helper
    assert_eq!(
        l.convert("TXT_KEY_BUILT_RAW", &[s("TXT_KEY_UNIT_ARCHER")]),
        "You built an Archer."
    );
    // called directly with the key as the argument
    assert_eq!(
        l.convert("TXT_KEY_GRAMMAR_UPPER_A_AN", &[s("TXT_KEY_UNIT_ARCHER")]),
        "An Archer"
    );
}

#[test]
fn gender_branch() {
    let l = loc();
    assert_eq!(
        l.convert("TXT_KEY_TITLE", &[s("TXT_KEY_LEADER_X")]),
        "Empress"
    );
    assert_eq!(
        l.convert("TXT_KEY_TITLE", &[s("TXT_KEY_UNIT_SPEARMAN")]),
        "Emperor"
    );
}

#[test]
fn forms_by_pipe() {
    let l = loc();
    // default: the first form
    assert_eq!(l.convert("TXT_KEY_UNIT_KESHIK", &[]), "Keshik");
    assert_eq!(l.convert("TXT_KEY_FORMS_EXPLICIT", &[]), "Keshiks");
    assert_eq!(l.convert("TXT_KEY_FORMS_PLURAL", &[d(1.0)]), "Keshik");
    assert_eq!(l.convert("TXT_KEY_FORMS_PLURAL", &[d(7.0)]), "Keshiks");
}

#[test]
fn markup_is_left_alone() {
    let l = loc();
    assert_eq!(
        l.convert("TXT_KEY_ICON", &[d(3.0)]),
        "[ICON_GOLD] 3 [NEWLINE]x"
    );
}

#[test]
fn nested_gender_and_plural() {
    let mut t = MapTable::new();
    t.insert_text(
        "K",
        "{2:gender masculine?{2: plural 1?el ; other?los ;}; feminine?{2: plural 1?la ; other?las ;}; other?x;}",
    );
    t.insert("N_M1", "gato", Some("masculine"), Some("1"));
    t.insert("N_F2", "casas", Some("feminine"), Some("2"));
    let l = Localizer::new(LanguageInfo::en_us(), Box::new(t));
    // the plural transform inspects the string's own plurality
    assert_eq!(l.convert("K", &[d(0.0), s("N_M1")]), "el ");
    assert_eq!(l.convert("K", &[d(0.0), s("N_F2")]), "las ");
}

#[test]
fn boolean_and_integer_kinds() {
    let l = loc();
    assert_eq!(
        l.compose(
            "{1}-{2}",
            &[Arg::from_bool(true), Arg::Int64(10_000_000_000)]
        ),
        "1-10,000,000,000"
    );
    assert_eq!(l.compose("{1}", &[Arg::Float(0.1)]), "0.1");
}

#[test]
fn language_switch_and_lookup_language() {
    let mut l = loc();
    let mut ru = MapTable::new();
    ru.insert_text(
        "TXT_KEY_HELLO",
        "\u{41f}\u{440}\u{438}\u{432}\u{435}\u{442}",
    );
    ru.insert_text("TXT_KEY_ITEMS", "{1_N} {1_N: plural 1?\u{44f}\u{449}\u{438}\u{43a}; 2?\u{44f}\u{449}\u{438}\u{43a}\u{430}; other?\u{44f}\u{449}\u{438}\u{43a}\u{43e}\u{432};}");
    l.add_language(LanguageInfo::new("ru_RU", "Russian", 8), Box::new(ru));
    assert_eq!(
        l.lookup_language("ru_RU", "TXT_KEY_HELLO", &[]).as_deref(),
        Some("\u{41f}\u{440}\u{438}\u{432}\u{435}\u{442}")
    );
    assert_eq!(l.lookup_language("xx_XX", "TXT_KEY_HELLO", &[]), None);
    assert!(l.set_language("ru_RU"));
    assert_eq!(l.current_language().type_, "ru_RU");
    // ru_RU number symbols: no-break space grouping
    assert_eq!(l.compose("{1}", &[d(1234567.0)]), "1\u{a0}234\u{a0}567");
    let items = |n: f64| l.convert("TXT_KEY_ITEMS", &[d(n)]);
    assert!(items(1.0).ends_with("\u{44f}\u{449}\u{438}\u{43a}"));
    assert!(items(2.0).ends_with("\u{430}"));
    assert!(items(5.0).ends_with("\u{43e}\u{432}"));
    assert!(items(21.0).ends_with("\u{44f}\u{449}\u{438}\u{43a}"));
    assert!(items(11.0).ends_with("\u{43e}\u{432}"));
    assert!(items(101.0).ends_with("\u{44f}\u{449}\u{438}\u{43a}"));
    assert!(l.set_language("en_US"));
}
