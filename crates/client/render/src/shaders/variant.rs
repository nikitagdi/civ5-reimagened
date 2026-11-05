//! Variants of a shader module through `override` constants: skinning,
//! alpha, shadow pass and `_CODE` variants are one text with `override` declarations, and the
//! pipeline cache gives the values. No text is copied. A [`Variant`] is that list of values.

/// Values of pipeline-overridable constants, in the order they were set.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Variant {
    constants: Vec<(String, f64)>,
}

/// A name that is not declared with `override` in the module.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("`{0}` is not an override constant of the module")]
pub struct UnknownOverride(pub String);

impl Variant {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets a constant; a later value for the same name wins.
    pub fn with(mut self, name: &str, value: f64) -> Self {
        match self.constants.iter_mut().find(|(n, _)| n == name) {
            Some(c) => c.1 = value,
            None => self.constants.push((name.to_owned(), value)),
        }
        self
    }

    /// A boolean constant (`override USE_SHADOW: bool`).
    pub fn with_flag(self, name: &str, on: bool) -> Self {
        self.with(name, if on { 1.0 } else { 0.0 })
    }

    /// For `PipelineCompilationOptions::constants`.
    pub fn constants(&self) -> Vec<(&str, f64)> {
        self.constants
            .iter()
            .map(|(n, v)| (n.as_str(), *v))
            .collect()
    }

    /// A key for the pipeline cache: the same values give the same key whatever the order they
    /// were set in.
    pub fn key(&self) -> String {
        let mut sorted: Vec<_> = self.constants.iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        sorted
            .iter()
            .map(|(n, v)| format!("{n}={v}"))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Fails for a name `source` does not declare (a typo would otherwise be an ignored value or a
    /// pipeline error).
    pub fn check(&self, source: &str) -> Result<(), UnknownOverride> {
        let declared = declared_overrides(source);
        match self.constants.iter().find(|(n, _)| !declared.contains(n)) {
            Some((n, _)) => Err(UnknownOverride(n.clone())),
            None => Ok(()),
        }
    }
}

/// The names declared with `override` in WGSL `source` (`override NAME: type = ...;`, also with
/// an `@id(n)` attribute before it).
pub fn declared_overrides(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in source.lines() {
        let line = line.split("//").next().unwrap_or("").trim();
        let mut rest = line;
        // `@id(3) override name: f32;`
        while let Some(after) = rest.strip_prefix('@') {
            match after.find(')') {
                Some(i) => rest = after[i + 1..].trim_start(),
                None => break,
            }
        }
        if let Some(decl) = rest.strip_prefix("override") {
            if !decl.starts_with(char::is_whitespace) {
                continue;
            }
            let name: String = decl
                .trim_start()
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                names.push(name);
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "
override USE_SKIN: bool = false;   // a comment with override FAKE
@id(7) override ALPHA_REF: f32 = 0.5;
const NOT_AN_OVERRIDE: f32 = 1.0;
fn overrides() {}
";

    #[test]
    fn the_declared_names_are_found() {
        assert_eq!(declared_overrides(SRC), ["USE_SKIN", "ALPHA_REF"]);
    }

    #[test]
    fn a_variant_is_checked_against_the_module() {
        let v = Variant::new()
            .with_flag("USE_SKIN", true)
            .with("ALPHA_REF", 0.25);
        assert_eq!(v.check(SRC), Ok(()));
        assert_eq!(v.constants(), [("USE_SKIN", 1.0), ("ALPHA_REF", 0.25)]);
        let typo = Variant::new().with("USE_SKINN", 1.0);
        assert_eq!(typo.check(SRC), Err(UnknownOverride("USE_SKINN".into())));
        assert_eq!(Variant::new().check(SRC), Ok(()));
    }

    #[test]
    fn the_key_ignores_the_order_and_the_last_value_wins() {
        let a = Variant::new().with("B", 2.0).with("A", 1.0);
        let b = Variant::new().with("A", 1.0).with("B", 2.0);
        assert_eq!(a.key(), b.key());
        assert_eq!(a.key(), "A=1,B=2");
        let c = a.clone().with("A", 3.0);
        assert_eq!(c.constants().len(), 2);
        assert_eq!(c.key(), "A=3,B=2");
    }
}
