//! ICU `DecimalFormat` subset used by the `number`, `percent` and `currency` transforms, plus Roman
//! numerals, English spell-out and English ordinals.
//!
//! The exe delegates to ICU 4.4 (`FormatNumber` 0x08c5b7c8 and friends); the pattern grammar and the
//! half-even rounding below follow ICU's `DecimalFormat::applyPattern` and `subformat`.

/// Locale symbols of the number formatter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbols {
    pub decimal: char,
    pub grouping: char,
    pub minus: char,
    pub percent: char,
    pub permille: char,
    pub currency_symbol: String,
    pub currency_code: String,
}

impl Symbols {
    /// en_US: `,` grouping, `.` decimal point.
    pub fn en_us() -> Symbols {
        Symbols {
            decimal: '.',
            grouping: ',',
            minus: '-',
            percent: '%',
            permille: '\u{2030}',
            currency_symbol: "$".into(),
            currency_code: "USD".into(),
        }
    }

    /// ru_RU: no-break space grouping, `,` decimal point.
    pub fn ru_ru() -> Symbols {
        Symbols {
            decimal: ',',
            grouping: '\u{a0}',
            minus: '-',
            percent: '%',
            permille: '\u{2030}',
            currency_symbol: "\u{440}.".into(),
            currency_code: "RUB".into(),
        }
    }

    /// Symbols for a language type such as `en_US` or `ru_RU`; unknown languages use English symbols.
    pub fn for_language(language_type: &str) -> Symbols {
        if language_type.to_ascii_lowercase().starts_with("ru") {
            Symbols::ru_ru()
        } else {
            Symbols::en_us()
        }
    }
}

/// A pattern the ICU grammar rejects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternError(pub &'static str);

impl std::fmt::Display for PatternError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "bad number pattern: {}", self.0)
    }
}

impl std::error::Error for PatternError {}

/// A parsed decimal format pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecimalFormat {
    pos_prefix: String,
    pos_suffix: String,
    neg_prefix: String,
    neg_suffix: String,
    grouping: usize,
    min_int: usize,
    min_frac: usize,
    max_frac: usize,
    multiplier: i64,
    decimal_always_shown: bool,
    symbols: Symbols,
}

struct SubPattern {
    prefix: String,
    suffix: String,
    digit_left: usize,
    zero_digits: usize,
    digit_right: usize,
    decimal_pos: i64,
    grouping_count: i64,
    multiplier: i64,
}

fn parse_sub(
    chars: &[char],
    pos: &mut usize,
    sym: &Symbols,
    first: bool,
) -> Result<SubPattern, PatternError> {
    let mut sub = SubPattern {
        prefix: String::new(),
        suffix: String::new(),
        digit_left: 0,
        zero_digits: 0,
        digit_right: 0,
        decimal_pos: -1,
        grouping_count: -1,
        multiplier: 1,
    };
    // Phase 0: prefix, 1: number, 2: suffix.
    let mut phase = 0;
    let mut in_quote = false;
    while *pos < chars.len() {
        let c = chars[*pos];
        if phase == 1 {
            match c {
                '#' => {
                    if sub.zero_digits > 0 {
                        sub.digit_right += 1;
                    } else {
                        sub.digit_left += 1;
                    }
                    if sub.grouping_count >= 0 && sub.decimal_pos < 0 {
                        sub.grouping_count += 1;
                    }
                }
                '0' => {
                    if sub.digit_right > 0 {
                        return Err(PatternError("'0' after '#' in the fraction"));
                    }
                    sub.zero_digits += 1;
                    if sub.grouping_count >= 0 && sub.decimal_pos < 0 {
                        sub.grouping_count += 1;
                    }
                }
                ',' => sub.grouping_count = 0,
                '.' => {
                    if sub.decimal_pos >= 0 {
                        return Err(PatternError("two decimal separators"));
                    }
                    sub.decimal_pos = (sub.digit_left + sub.zero_digits + sub.digit_right) as i64;
                }
                'E' => return Err(PatternError("exponent patterns are not supported")),
                ';' => {
                    *pos += 1;
                    return finish_sub(sub, in_quote);
                }
                _ => {
                    phase = 2;
                    continue;
                }
            }
            *pos += 1;
            continue;
        }
        // Affix handling (prefix or suffix).
        if c == '\'' {
            if chars.get(*pos + 1) == Some(&'\'') {
                affix(&mut sub, phase).push('\'');
                *pos += 2;
                continue;
            }
            in_quote = !in_quote;
            *pos += 1;
            continue;
        }
        if in_quote {
            affix(&mut sub, phase).push(c);
            *pos += 1;
            continue;
        }
        match c {
            '#' | '0' | ',' | '.' => {
                if phase == 0 {
                    phase = 1;
                    continue;
                }
                return Err(PatternError("unquoted digit symbol in the suffix"));
            }
            ';' => {
                if !first {
                    return Err(PatternError("more than two sub-patterns"));
                }
                *pos += 1;
                return finish_sub(sub, in_quote);
            }
            '%' => {
                sub.multiplier = 100;
                affix(&mut sub, phase).push(sym.percent);
            }
            '\u{2030}' => {
                sub.multiplier = 1000;
                affix(&mut sub, phase).push(sym.permille);
            }
            '\u{a4}' => {
                if chars.get(*pos + 1) == Some(&'\u{a4}') {
                    *pos += 1;
                    affix(&mut sub, phase).push_str(&sym.currency_code);
                } else {
                    affix(&mut sub, phase).push_str(&sym.currency_symbol);
                }
            }
            '-' => affix(&mut sub, phase).push(sym.minus),
            _ => affix(&mut sub, phase).push(c),
        }
        *pos += 1;
    }
    finish_sub(sub, in_quote)
}

fn affix(sub: &mut SubPattern, phase: i32) -> &mut String {
    if phase == 0 {
        &mut sub.prefix
    } else {
        &mut sub.suffix
    }
}

fn finish_sub(mut sub: SubPattern, in_quote: bool) -> Result<SubPattern, PatternError> {
    if in_quote {
        return Err(PatternError("unterminated quote"));
    }
    if sub.zero_digits == 0 && sub.digit_left > 0 && sub.decimal_pos >= 0 {
        // "###.###", "###." and ".###": the digit next to the decimal point is a required zero.
        let mut n = sub.decimal_pos as usize;
        if n == 0 {
            n += 1;
        }
        sub.digit_right = sub.digit_left - n;
        sub.digit_left = n - 1;
        sub.zero_digits = 1;
    }
    let dp = sub.decimal_pos;
    if (dp < 0 && sub.digit_right > 0)
        || (dp >= 0
            && (dp < sub.digit_left as i64 || dp > (sub.digit_left + sub.zero_digits) as i64))
        || sub.grouping_count == 0
    {
        return Err(PatternError("misplaced digit symbols"));
    }
    Ok(sub)
}

impl DecimalFormat {
    /// `DecimalFormat::applyPattern` with `positive[;negative]`.
    pub fn parse(pattern: &str, symbols: &Symbols) -> Result<DecimalFormat, PatternError> {
        // re: 0x08c5b7c8 ParameterTransforms::FormatNumber (ICU applyPattern on the style argument)
        let chars: Vec<char> = pattern.chars().collect();
        let mut pos = 0;
        let pos_sub = parse_sub(&chars, &mut pos, symbols, true)?;
        let mut neg_prefix = format!("{}{}", symbols.minus, pos_sub.prefix);
        let mut neg_suffix = pos_sub.suffix.clone();
        if pos < chars.len() {
            let neg = parse_sub(&chars, &mut pos, symbols, false)?;
            neg_prefix = neg.prefix;
            neg_suffix = neg.suffix;
        }
        let total = pos_sub.digit_left + pos_sub.zero_digits + pos_sub.digit_right;
        let dp = pos_sub.decimal_pos;
        let eff_dec = if dp >= 0 { dp as usize } else { total };
        let (max_frac, min_frac) = if dp >= 0 {
            (
                total - dp as usize,
                pos_sub.digit_left + pos_sub.zero_digits - dp as usize,
            )
        } else {
            (0, 0)
        };
        Ok(DecimalFormat {
            pos_prefix: pos_sub.prefix,
            pos_suffix: pos_sub.suffix,
            neg_prefix,
            neg_suffix,
            grouping: if pos_sub.grouping_count > 0 {
                pos_sub.grouping_count as usize
            } else {
                0
            },
            min_int: eff_dec - pos_sub.digit_left,
            min_frac,
            max_frac,
            multiplier: pos_sub.multiplier,
            decimal_always_shown: dp == 0 || dp == total as i64,
            symbols: symbols.clone(),
        })
    }

    /// `NumberFormat::createInstance(locale)`: `#,##0.###`.
    pub fn number(symbols: &Symbols) -> DecimalFormat {
        // INVARIANT: the literal pattern is valid.
        DecimalFormat::parse("#,##0.###", symbols).expect("built-in pattern")
    }

    /// `NumberFormat::createPercentInstance`: `#,##0%`.
    pub fn percent(symbols: &Symbols) -> DecimalFormat {
        DecimalFormat::parse("#,##0%", symbols).expect("built-in pattern")
    }

    /// `NumberFormat::createCurrencyInstance`: `¤#,##0.00;(¤#,##0.00)`.
    pub fn currency(symbols: &Symbols) -> DecimalFormat {
        DecimalFormat::parse("\u{a4}#,##0.00;(\u{a4}#,##0.00)", symbols).expect("built-in pattern")
    }

    pub fn format_i64(&self, v: i64) -> String {
        let neg = v < 0;
        let mut mag = (v as i128).unsigned_abs() * self.multiplier as u128;
        let mut digits = Vec::new();
        if mag == 0 {
            digits.push(0u8);
        }
        while mag > 0 {
            digits.push((mag % 10) as u8);
            mag /= 10;
        }
        digits.reverse();
        let dp = if digits == [0] {
            0
        } else {
            digits.len() as i32
        };
        if digits == [0] {
            digits.clear();
        }
        self.assemble(neg, digits, dp)
    }

    pub fn format_f64(&self, v: f64) -> String {
        if v.is_nan() {
            return "NaN".to_string();
        }
        let neg = v.is_sign_negative();
        if v.is_infinite() {
            let body = "\u{221e}";
            return if neg {
                format!("{}{}{}", self.neg_prefix, body, self.neg_suffix)
            } else {
                format!("{}{}{}", self.pos_prefix, body, self.pos_suffix)
            };
        }
        let scaled = v.abs() * self.multiplier as f64;
        let (digits, dp) = shortest_digits(scaled);
        self.assemble(neg, digits, dp)
    }

    fn assemble(&self, neg: bool, mut digits: Vec<u8>, mut dp: i32) -> String {
        round_half_even(&mut digits, &mut dp, self.max_frac);
        let int_digits: Vec<u8> = if dp > 0 {
            (0..dp as usize)
                .map(|i| digits.get(i).copied().unwrap_or(0))
                .collect()
        } else {
            Vec::new()
        };
        let mut frac: Vec<u8> = Vec::new();
        let frac_len = digits.len() as i32 - dp.max(0);
        if frac_len > 0 || dp < 0 {
            let lead_zeros = (-dp).max(0) as usize;
            frac.extend(std::iter::repeat_n(0u8, lead_zeros));
            frac.extend(digits.iter().skip(dp.max(0) as usize));
        }
        while frac.len() > self.min_frac && frac.last() == Some(&0) {
            frac.pop();
        }
        while frac.len() < self.min_frac {
            frac.push(0);
        }
        frac.truncate(self.max_frac.max(self.min_frac));

        let mut int_str: Vec<char> = int_digits.iter().map(|d| (b'0' + d) as char).collect();
        while int_str.len() < self.min_int {
            int_str.insert(0, '0');
        }
        if int_str.is_empty() && frac.is_empty() {
            int_str.push('0');
        }
        let mut body = String::new();
        let n = int_str.len();
        for (i, ch) in int_str.iter().enumerate() {
            if self.grouping > 0 && i > 0 && (n - i).is_multiple_of(self.grouping) {
                body.push(self.symbols.grouping);
            }
            body.push(*ch);
        }
        if !frac.is_empty() || self.decimal_always_shown {
            body.push(self.symbols.decimal);
        }
        body.extend(frac.iter().map(|d| (b'0' + d) as char));
        if neg {
            format!("{}{}{}", self.neg_prefix, body, self.neg_suffix)
        } else {
            format!("{}{}{}", self.pos_prefix, body, self.pos_suffix)
        }
    }
}

/// Shortest round-trip decimal digits of a non-negative finite double (ICU's `DigitList::set(double)`),
/// as digits `d1 d2 ...` and the position of the decimal point: value = 0.d1d2... x 10^dp.
fn shortest_digits(v: f64) -> (Vec<u8>, i32) {
    if v == 0.0 {
        return (Vec::new(), 0);
    }
    let s = format!("{v:e}");
    let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let digits: Vec<u8> = mant
        .bytes()
        .filter(u8::is_ascii_digit)
        .map(|b| b - b'0')
        .collect();
    (digits, exp + 1)
}

/// Round to `max_frac` fraction digits, half-even on the decimal digits; trims trailing zeros.
fn round_half_even(digits: &mut Vec<u8>, dp: &mut i32, max_frac: usize) {
    let keep = *dp as i64 + max_frac as i64;
    if keep < 0 {
        digits.clear();
        *dp = 0;
        return;
    }
    let keep = keep as usize;
    if keep < digits.len() {
        let first_dropped = digits[keep];
        let rest_nonzero = digits[keep + 1..].iter().any(|&d| d != 0);
        let prev_odd = keep > 0 && digits[keep - 1] % 2 == 1;
        let up = first_dropped > 5 || (first_dropped == 5 && (rest_nonzero || prev_odd));
        digits.truncate(keep);
        if up {
            let mut i = keep;
            loop {
                if i == 0 {
                    digits.insert(0, 1);
                    *dp += 1;
                    break;
                }
                i -= 1;
                if digits[i] == 9 {
                    digits[i] = 0;
                } else {
                    digits[i] += 1;
                    break;
                }
            }
        }
    }
    while digits.last() == Some(&0) {
        digits.pop();
    }
    if digits.is_empty() {
        *dp = 0;
    }
}

/// Roman numerals, `%roman-upper`: 1..=3999; `None` outside.
pub fn roman_upper(n: i64) -> Option<String> {
    // re: 0x08c5bc40 ParameterTransforms::FormatRoman uses the ICU ruleset "%roman-upper"
    if !(1..=3999).contains(&n) {
        return None;
    }
    const TABLE: [(i64, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut n = n;
    let mut out = String::new();
    for (v, s) in TABLE {
        while n >= v {
            out.push_str(s);
            n -= v;
        }
    }
    Some(out)
}

const ONES: [&str; 20] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];
const TENS: [&str; 10] = [
    "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];
const SCALES: [&str; 7] = [
    "",
    "thousand",
    "million",
    "billion",
    "trillion",
    "quadrillion",
    "quintillion",
];

fn spell_below_1000(n: u64, out: &mut Vec<String>) {
    let h = n / 100;
    let r = n % 100;
    if h > 0 {
        out.push(format!("{} hundred", ONES[h as usize]));
    }
    if r > 0 {
        if r < 20 {
            out.push(ONES[r as usize].to_string());
        } else if r.is_multiple_of(10) {
            out.push(TENS[(r / 10) as usize].to_string());
        } else {
            out.push(format!(
                "{}-{}",
                TENS[(r / 10) as usize],
                ONES[(r % 10) as usize]
            ));
        }
    }
}

/// English cardinal for an integer (ICU `%simple` of the en spell-out rules).
pub fn spellout_en_i64(n: i64) -> String {
    // SIMPLIFIED(SIMP-txt-3): English only, no `and`-style variants
    if n == 0 {
        return "zero".into();
    }
    let mut parts: Vec<String> = Vec::new();
    if n < 0 {
        parts.push("minus".into());
    }
    let mag = n.unsigned_abs();
    let mut groups = Vec::new();
    let mut m = mag;
    while m > 0 {
        groups.push(m % 1000);
        m /= 1000;
    }
    for (i, g) in groups.iter().enumerate().rev() {
        if *g == 0 {
            continue;
        }
        spell_below_1000(*g, &mut parts);
        if i > 0 {
            parts.push(SCALES[i].to_string());
        }
    }
    parts.join(" ")
}

/// English cardinal for a double: integer part, `point`, then each fractional digit.
pub fn spellout_en_f64(v: f64) -> String {
    if v.is_nan() {
        return "not a number".into();
    }
    if v.is_infinite() {
        return if v < 0.0 {
            "minus infinity".into()
        } else {
            "infinity".into()
        };
    }
    if v.fract() == 0.0 && v.abs() < 9.0e18 {
        return spellout_en_i64(v as i64);
    }
    let (digits, dp) = shortest_digits(v.abs());
    let int_part: i64 = (0..dp.max(0) as usize)
        .map(|i| digits.get(i).copied().unwrap_or(0) as i64)
        .fold(0i64, |a, d| a.saturating_mul(10).saturating_add(d));
    let mut out = String::new();
    if v < 0.0 {
        out.push_str("minus ");
    }
    out.push_str(&spellout_en_i64(int_part));
    out.push_str(" point");
    for _ in 0..(-dp).max(0) {
        out.push_str(" zero");
    }
    for d in digits.iter().skip(dp.max(0) as usize) {
        out.push(' ');
        out.push_str(ONES[*d as usize]);
    }
    out
}

/// English ordinal in digits (ICU `%digits-ordinal`): `1st`, `2nd`, `3rd`, `4th`, `11th`, `1,001st`.
pub fn ordinal_en(n: i64, symbols: &Symbols) -> String {
    // SIMPLIFIED(SIMP-txt-3): English only
    let mag = n.unsigned_abs();
    let suffix = match (mag % 100, mag % 10) {
        (11..=13, _) => "th",
        (_, 1) => "st",
        (_, 2) => "nd",
        (_, 3) => "rd",
        _ => "th",
    };
    let f = DecimalFormat::number(symbols);
    format!("{}{}", f.format_i64(n), suffix)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(p: &str, v: f64) -> String {
        DecimalFormat::parse(p, &Symbols::en_us())
            .unwrap()
            .format_f64(v)
    }
    fn fmti(p: &str, v: i64) -> String {
        DecimalFormat::parse(p, &Symbols::en_us())
            .unwrap()
            .format_i64(v)
    }

    #[test]
    fn basic_patterns() {
        assert_eq!(fmt("#", 12.6), "13");
        assert_eq!(fmt("#", 0.0), "0");
        assert_eq!(fmt("#.##", 3.14649), "3.15");
        assert_eq!(fmt("#.##", 0.5), "0.5");
        assert_eq!(fmt("#.##", 2.0), "2");
        assert_eq!(fmt("#.#", 2.25), "2.2");
        assert_eq!(fmt("#.#", 2.35), "2.4");
        assert_eq!(fmt("0.00", 2.0), "2.00");
        assert_eq!(fmt("#,###,###,###", 1234567890.0), "1,234,567,890");
        assert_eq!(
            fmti("#,###,###,###", 9_007_199_254_740_993),
            "9,007,199,254,740,993"
        );
        assert_eq!(
            fmti("#,###,###,###", i64::MIN),
            "-9,223,372,036,854,775,808"
        );
    }

    #[test]
    fn half_even_rounding() {
        assert_eq!(fmt("#", 0.5), "0");
        assert_eq!(fmt("#", 1.5), "2");
        assert_eq!(fmt("#", 2.5), "2");
        assert_eq!(fmt("#", 3.5), "4");
        assert_eq!(fmt("#.##", 0.125), "0.12");
        assert_eq!(fmt("#.##", 0.135), "0.14");
        assert_eq!(fmt("#", 0.51), "1");
        assert_eq!(fmt("#", 0.4), "0");
        assert_eq!(fmt("#.#", 9.96), "10");
        assert_eq!(fmt("#.#", 0.05), "0");
    }

    #[test]
    fn negative_subpattern_and_markup() {
        let p = "'+'##.##;'[COLOR_RED]-'##.##";
        assert_eq!(fmt(p, 3.5), "+3.5");
        assert_eq!(fmt(p, -3.5), "[COLOR_RED]-3.5");
        assert_eq!(fmt("'+'##;'[COLOR_RED]-'##", 0.0), "+0");
        let q = "'(+'##')';'[COLOR:255:60:60:255](-'##')'";
        assert_eq!(fmt(q, 5.0), "(+5)");
        assert_eq!(fmt(q, -5.0), "[COLOR:255:60:60:255](-5)");
    }

    #[test]
    fn default_negative_prefix() {
        assert_eq!(fmt("#,##0", -1234.0), "-1,234");
        assert_eq!(fmt("#.##", -0.5), "-0.5");
    }

    #[test]
    fn percent_and_quoted_percent() {
        let s = Symbols::en_us();
        assert_eq!(DecimalFormat::percent(&s).format_f64(0.256), "26%");
        assert_eq!(DecimalFormat::percent(&s).format_i64(2), "200%");
        assert_eq!(fmt("#'%'", 25.0), "25%");
        assert_eq!(fmt("#%", 0.5), "50%");
        assert_eq!(fmt("#.#\u{2030}", 0.0123), "12.3\u{2030}");
    }

    #[test]
    fn defaults() {
        let s = Symbols::en_us();
        let n = DecimalFormat::number(&s);
        assert_eq!(n.format_i64(1234567), "1,234,567");
        assert_eq!(n.format_f64(1234.5678), "1,234.568");
        assert_eq!(n.format_f64(0.1f32 as f64), "0.1");
        assert_eq!(n.format_f64(-0.0004), "-0");
        let c = DecimalFormat::currency(&s);
        assert_eq!(c.format_f64(1234.5), "$1,234.50");
        assert_eq!(c.format_f64(-3.0), "($3.00)");
        let ru = Symbols::ru_ru();
        assert_eq!(
            DecimalFormat::number(&ru).format_f64(1234.5),
            "1\u{a0}234,5"
        );
    }

    #[test]
    fn bad_patterns() {
        let s = Symbols::en_us();
        assert!(DecimalFormat::parse("#.#.#", &s).is_err());
        assert!(DecimalFormat::parse("'abc#", &s).is_err());
        assert!(DecimalFormat::parse("0.0#0", &s).is_err());
        assert!(DecimalFormat::parse("#E0", &s).is_err());
    }

    #[test]
    fn roman() {
        assert_eq!(roman_upper(1994).unwrap(), "MCMXCIV");
        assert_eq!(roman_upper(4).unwrap(), "IV");
        assert_eq!(roman_upper(0), None);
        assert_eq!(roman_upper(4000), None);
    }

    #[test]
    fn spellout_and_ordinal() {
        assert_eq!(spellout_en_i64(0), "zero");
        assert_eq!(spellout_en_i64(21), "twenty-one");
        assert_eq!(spellout_en_i64(123), "one hundred twenty-three");
        assert_eq!(spellout_en_i64(1001), "one thousand one");
        assert_eq!(spellout_en_i64(-5), "minus five");
        assert_eq!(spellout_en_f64(2.5), "two point five");
        let s = Symbols::en_us();
        assert_eq!(ordinal_en(1, &s), "1st");
        assert_eq!(ordinal_en(12, &s), "12th");
        assert_eq!(ordinal_en(22, &s), "22nd");
        assert_eq!(ordinal_en(103, &s), "103rd");
        assert_eq!(ordinal_en(1001, &s), "1,001st");
    }
}
