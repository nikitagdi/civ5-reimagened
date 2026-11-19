//! Check screen bindings without a browser. The scanner handles only the authored quoted HTML fragments.
use std::path::Path;
use ui2_model::{Sample, Schema};
use ui2_package::profile::{
    Tag, TypeSig,
    attrs::{self, AttrCtx, RawAttrs},
    expr::Scope,
};

fn states(scope: Scope, path: &str, value: &toml::Value) -> Scope {
    match value {
        toml::Value::Table(table) => table
            .iter()
            .fold(scope, |s, (k, v)| states(s, &format!("{path}.{k}"), v)),
        toml::Value::Boolean(_) => scope.with_state(path, TypeSig::Bool),
        toml::Value::Integer(_) => scope.with_state(path, TypeSig::Int),
        toml::Value::Float(_) => scope.with_state(path, TypeSig::Float),
        toml::Value::String(_) => scope.with_state(path, TypeSig::Str),
        _ => panic!("unsupported skin state"),
    }
}

#[test]
fn tech_group_bindings_and_all_samples_match_the_schema() {
    let skin = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../skins/engraved");
    let manifest: toml::Value =
        toml::from_str(&std::fs::read_to_string(skin.join("skin.toml")).unwrap()).unwrap();
    let mut scope = Scope::new();
    for (k, v) in manifest["state"].as_table().unwrap() {
        scope = states(scope, k, v);
    }
    for file in [
        "techs",
        "techs_tip",
        "tech_panel",
        "tech_award",
        "tech_award_tip",
    ] {
        let source = std::fs::read_to_string(skin.join(format!("parts/{file}.html"))).unwrap();
        let mut rest = source.as_str();
        let mut stack = vec![(String::new(), scope.clone(), false)];
        while let Some(start) = rest.find('<') {
            rest = &rest[start + 1..];
            if rest.starts_with("!--") {
                rest = &rest[rest.find("-->").unwrap() + 3..];
                continue;
            }
            let mut quote = None;
            let end = rest
                .char_indices()
                .find_map(|(i, c)| {
                    if let Some(q) = quote {
                        if c == q {
                            quote = None;
                        }
                    } else if c == '\'' || c == '"' {
                        quote = Some(c);
                    } else if c == '>' {
                        return Some(i);
                    }
                    None
                })
                .unwrap();
            let mut tag = rest[..end].trim();
            rest = &rest[end + 1..];
            if let Some(close) = tag.strip_prefix('/') {
                let (open, _, _) = stack.pop().unwrap();
                assert_eq!(open, close, "{file}: unbalanced HTML");
                continue;
            }
            let name_end = tag.find(char::is_whitespace).unwrap_or(tag.len());
            let name = tag[..name_end].to_string();
            // An inline svg is a picture, not markup: the converter checks only its symbol references.
            if name == "svg" {
                if !tag.ends_with('/') {
                    rest = &rest[rest.find("</svg>").expect("closed svg") + "</svg>".len()..];
                }
                continue;
            }
            tag = tag[name_end..].trim();
            let mut values = Vec::new();
            while !tag.is_empty() && tag != "/" {
                let at = tag.find(['=', ' ']).unwrap_or(tag.len());
                let key = tag[..at].to_string();
                tag = tag[at..].trim_start();
                let value = if let Some(after) = tag.strip_prefix('=') {
                    let after = after.trim_start();
                    let q = after.chars().next().unwrap();
                    assert!(q == '\'' || q == '"');
                    let end = after[1..].find(q).unwrap() + 1;
                    let value = after[1..end].to_string();
                    tag = after[end + 1..].trim_start();
                    value
                } else {
                    String::new()
                };
                values.push((key, value));
            }
            let parent = stack.last().unwrap();
            let ctx = AttrCtx {
                in_for: parent.2,
                ..AttrCtx::default()
            };
            let raw = RawAttrs {
                tag: Tag::from_name(&name).expect("profile tag"),
                attrs: values,
            };
            let mut parsed =
                attrs::parse(&raw, &ctx).unwrap_or_else(|e| panic!("{file}: {name}: {e:?}"));
            let checked = attrs::check(&mut parsed, &parent.1, Schema::v1(), &ctx)
                .unwrap_or_else(|e| panic!("{file}: {name}: {e:?}"));
            if !["img", "input", "hr"].contains(&name.as_str()) {
                stack.push((name, checked.scope, parent.2 || parsed.for_.is_some()));
            }
        }
        assert_eq!(stack.len(), 1, "{file}: unclosed HTML");
    }
    for file in [
        "techs",
        "techs--free",
        "techs--steal",
        "tech_panel",
        "tech_panel--done",
        "rich_tooltips",
        "tech_award",
    ] {
        Sample::parse(
            Schema::v1(),
            &std::fs::read_to_string(skin.join(format!("sample/{file}.json"))).unwrap(),
        )
        .unwrap_or_else(|e| panic!("{file}: {e:?}"));
    }
}
