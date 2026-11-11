//! Generators: everything derived from the schema. `cargo xtask ui gen-model` writes the result to the
//! repository and `--check` compares it with what is committed.

mod js;
mod rust;

use crate::schema::Schema;

/// One generated file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenFile {
    /// Path relative to the repository root, with `/` separators.
    pub path: &'static str,
    pub content: String,
}

/// All generated files for a schema. Deterministic: the same schema gives the same bytes.
#[must_use]
pub fn generate(schema: &Schema) -> Vec<GenFile> {
    vec![
        GenFile {
            path: "crates/shared/ui2-model/src/gen/model.rs",
            content: rust::generate(schema),
        },
        GenFile {
            path: "crates/shared/ui2-model/gen/schema.gen.json",
            content: schema.to_json(),
        },
        GenFile {
            path: "apps/web/www/ui2/model.gen.js",
            content: js::generate_js(schema),
        },
        GenFile {
            path: "apps/web/www/ui2/model.gen.d.ts",
            content: js::generate_dts(schema),
        },
    ]
}
