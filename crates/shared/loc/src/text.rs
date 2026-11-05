//! `Localization::String` as data: a base string split into forms, each with plurality, gender and traits.

/// `Localization::Gender`; the discriminants are the exe's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Gender {
    Neuter = 0,
    Masculine = 1,
    Feminine = 2,
}

/// One form of a string (`SingleFormString`): the text between `|` separators and its grammar data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    pub text: String,
    /// 1-based plural form index (`SingleFormString::m_Plurality`).
    pub plurality: u32,
    pub gender: Gender,
    /// Noun traits such as `vowel`, `an`, `the`, `no_article` (`m_Traits`).
    pub traits: Vec<String>,
}

impl Form {
    /// A form built from composed text: plurality 1, masculine, no traits (`SingleFormString(base, 1, MASCULINE)`).
    pub fn plain(text: impl Into<String>) -> Form {
        Form {
            text: text.into(),
            plurality: 1,
            gender: Gender::Masculine,
            traits: Vec::new(),
        }
    }

    fn has_trait(&self, t: &str) -> bool {
        self.traits.iter().any(|x| x == t)
    }
}

/// A gender selector: `None` is the `*` wildcard, traits must all be present on the form.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GenderSpec {
    pub gender: Option<Gender>,
    pub traits: Vec<String>,
}

impl GenderSpec {
    /// `StringParser::ParseGenderAndTraits`: `masculine`, `feminine:vowel`, `*:an`, `neuter:a,b`.
    /// Fails on `|` (several forms) and on an unknown gender word.
    pub fn parse(s: &str) -> Option<GenderSpec> {
        // re: 0x08c62c7a StringParser::ParseGenderAndTraits
        if s.contains('|') {
            return None;
        }
        let word_end = s.find(':').unwrap_or(s.len());
        let word = &s[..word_end];
        let gender = if word == "*" {
            None
        } else {
            // The exe compares `strnicmp(word, literal, word.len())`: a case-insensitive prefix of the name.
            let prefix_of = |name: &str| {
                name.len() >= word.len()
                    && name.as_bytes()[..word.len()].eq_ignore_ascii_case(word.as_bytes())
            };
            if prefix_of("masculine") {
                Some(Gender::Masculine)
            } else if prefix_of("feminine") {
                Some(Gender::Feminine)
            } else if prefix_of("neuter") {
                Some(Gender::Neuter)
            } else {
                return None;
            }
        };
        let mut traits = Vec::new();
        if word_end < s.len() {
            for t in s[word_end + 1..].split(',') {
                // Empty tokens and tokens starting with `*` are skipped by the exe.
                if !t.is_empty() && !t.starts_with('*') {
                    traits.push(t.to_string());
                }
            }
        }
        Some(GenderSpec { gender, traits })
    }
}

/// `Localization::String` without its arguments: forms split at every `|` of the base string.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocString {
    base: String,
    forms: Vec<Form>,
}

impl LocString {
    /// `String(const char*)`: empty text has no forms, otherwise one form per `|`-separated part.
    pub fn new(text: &str) -> LocString {
        // re: 0x08c5ceec String::ParseBaseString: a plain split at every '|', braces are not respected
        let forms = if text.is_empty() {
            Vec::new()
        } else {
            text.split('|').map(Form::plain).collect()
        };
        LocString {
            base: text.to_string(),
            forms,
        }
    }

    /// `String(SingleFormString const&)`: one form, keeping its grammar data.
    pub fn from_form(form: Form) -> LocString {
        LocString {
            base: form.text.clone(),
            forms: vec![form],
        }
    }

    /// The whole base string, `|` included (`GetBaseString`).
    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn num_forms(&self) -> usize {
        self.forms.len()
    }

    /// Form `n`, 1-based.
    pub fn form(&self, n: usize) -> Option<&Form> {
        n.checked_sub(1).and_then(|i| self.forms.get(i))
    }

    pub fn is_empty(&self) -> bool {
        self.base.is_empty()
    }

    /// `String::SetGender(const char*, len)`: one specifier for all forms, or one per `|`-separated form.
    pub fn set_gender(&mut self, spec: &str) -> bool {
        // re: 0x08c5dbec String::SetGender(char const*, unsigned int)
        let parts = if spec.is_empty() {
            1
        } else {
            spec.split('|').count()
        };
        if parts != 1 {
            if parts != self.forms.len() {
                return false;
            }
            for (i, part) in spec.split('|').enumerate() {
                if !self.set_form_gender(i + 1, part) {
                    return false;
                }
            }
            return true;
        }
        for n in 1..=self.forms.len() {
            if !self.set_form_gender(n, spec) {
                return false;
            }
        }
        true
    }

    /// `String::SetGender(nForm, const char*, len)`: parses; a failed parse changes nothing but succeeds.
    pub fn set_form_gender(&mut self, n: usize, spec: &str) -> bool {
        // re: 0x08c5dd16 String::SetGender(unsigned int, char const*, unsigned int)
        let Some(form) = n.checked_sub(1).and_then(|i| self.forms.get_mut(i)) else {
            return false;
        };
        if let Some(parsed) = GenderSpec::parse(spec) {
            if let Some(g) = parsed.gender {
                form.gender = g;
            }
            for t in parsed.traits {
                if !form.has_trait(&t) {
                    form.traits.push(t);
                }
            }
        }
        true
    }

    /// `String::SetPlurality(const char*, len)`: `1`, or `1|2|...` with one number per form.
    /// `max_forms` is the plural rule's form count. Zero or too-large numbers fail.
    pub fn set_plurality(&mut self, spec: &str, max_forms: u32) -> bool {
        // re: 0x08c5df04 String::SetPlurality(char const*, unsigned int)
        let parts: Vec<&str> = spec.split('|').collect();
        let parse = |p: &str| -> Option<u32> {
            if p.len() >= 8 {
                return None;
            }
            // atoi: leading digits, anything else stops
            let digits: String = p.chars().take_while(|c| c.is_ascii_digit()).collect();
            let v: u32 = digits.parse().unwrap_or(0);
            (v != 0).then_some(v)
        };
        if parts.len() == 1 {
            let Some(v) = parse(parts[0]) else {
                return false;
            };
            if v > max_forms {
                return false;
            }
            for f in &mut self.forms {
                f.plurality = v;
            }
            return true;
        }
        if parts.len() != self.forms.len() {
            return false;
        }
        for (i, p) in parts.iter().enumerate() {
            let Some(v) = parse(p) else { return false };
            if v > max_forms {
                return false;
            }
            self.forms[i].plurality = v;
        }
        true
    }

    /// `String::GetFormByPlurality(p, offset)`: 1-based index of the matching form, 0 if none.
    pub fn form_by_plurality(&self, p: u32, mut offset: u32) -> usize {
        // re: 0x08c5d89e String::GetFormByPlurality
        for (i, f) in self.forms.iter().enumerate() {
            if f.plurality == p {
                if offset == 0 {
                    return i + 1;
                }
                offset -= 1;
            }
        }
        0
    }

    /// `String::GetFormByGender(Gender, traits)`: first form with that gender (any for the wildcard) and all traits.
    pub fn form_by_gender(&self, spec: &GenderSpec) -> usize {
        // re: 0x08c5d9de String::GetFormByGender(Gender, vector<SharedString> const&)
        for (i, f) in self.forms.iter().enumerate() {
            if (spec.gender.is_none() || spec.gender == Some(f.gender))
                && spec.traits.iter().all(|t| f.has_trait(t))
            {
                return i + 1;
            }
        }
        0
    }

    /// `String::GetFormByPluralityAndGender`.
    pub fn form_by_plurality_and_gender(&self, p: u32, spec: &GenderSpec) -> usize {
        // re: 0x08c5db46 String::GetFormByPluralityAndGender(int, Gender, vector<SharedString> const&)
        for (i, f) in self.forms.iter().enumerate() {
            if f.plurality == p
                && (spec.gender.is_none() || spec.gender == Some(f.gender))
                && spec.traits.iter().all(|t| f.has_trait(t))
            {
                return i + 1;
            }
        }
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_forms_and_defaults() {
        let s = LocString::new("Keshik|Keshiks");
        assert_eq!(s.num_forms(), 2);
        assert_eq!(s.form(2).unwrap().text, "Keshiks");
        assert_eq!(s.form(1).unwrap().gender, Gender::Masculine);
        assert_eq!(s.form(1).unwrap().plurality, 1);
        assert!(LocString::new("").form(1).is_none());
    }

    #[test]
    fn plurality_and_gender_columns() {
        let mut s = LocString::new("a|b|c|d");
        assert!(s.set_plurality("1|1|2|2", 2));
        assert!(s.set_gender("masculine|feminine|masculine|feminine"));
        assert_eq!(s.form_by_plurality(2, 0), 3);
        assert_eq!(s.form_by_plurality(2, 1), 4);
        let fem = GenderSpec::parse("feminine").unwrap();
        assert_eq!(s.form_by_plurality_and_gender(2, &fem), 4);
        assert!(!s.set_plurality("1|2", 2));
        assert!(!s.set_plurality("3", 2));
        assert!(!s.set_plurality("0", 2));
    }

    #[test]
    fn gender_spec_parse() {
        let g = GenderSpec::parse("neuter:an").unwrap();
        assert_eq!(g.gender, Some(Gender::Neuter));
        assert_eq!(g.traits, ["an"]);
        let w = GenderSpec::parse("*:vowel").unwrap();
        assert_eq!(w.gender, None);
        assert!(GenderSpec::parse("a|b").is_none());
        assert!(GenderSpec::parse("robot").is_none());
        assert_eq!(
            GenderSpec::parse("MASC").unwrap().gender,
            Some(Gender::Masculine)
        );
    }

    #[test]
    fn traits_match_only_when_all_present() {
        let mut s = LocString::new("Archer");
        s.set_gender("neuter:an");
        assert_eq!(s.form_by_gender(&GenderSpec::parse("*:an").unwrap()), 1);
        assert_eq!(s.form_by_gender(&GenderSpec::parse("*:the").unwrap()), 0);
        assert_eq!(
            s.form_by_gender(&GenderSpec::parse("masculine").unwrap()),
            0
        );
    }
}
