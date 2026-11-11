//! `Schema` as a `SchemaView`: the types the package and the expression checker see.

use std::collections::BTreeMap;

use serde_json::Value as Json;
use std::sync::Arc;

use ui2_model::Schema;
use ui2_package::profile::{SchemaView, TypeSig, resolve_path};
use ui2_package::{
    FaceFamily, FontRoles, Kind, Manifest, Package, PackageBuilder, PackageError, Tool,
};

const REFERENCE: &str = r#"
version = "1.3"
[enum]
Yield = ["food", "production", "gold"]
UnitActionKind = ["move", "fortify"]
[type]
Plot = { x = "i16", y = "i16" }
[type.CityBanner]
key  = "key<city>"
name = "str"
production = "BannerProd?"
color = "color"
[type.BannerProd]
item = "key<item>"
turns = "i16?"
[type.Resource]
id = "key<item>"
name = "text"
available = "u32"
share = "f32"
[model.top]
gold = "i32"
tourism = "i32?"
resources = { list = "Resource", key = "id" }
[model.ui]
pos = "Plot"
tab = { type = "Yield", js = true }
zoom = { type = "f32", js = true }
cities = { list = "CityBanner", key = "key" }
[intent."unit.action"]
unit = "key<unit>"
action = "UnitActionKind"
[intent."city.rename"]
city = "key<city>"
name = "str"
note = "text?"
"#;

fn reference() -> Schema {
    Schema::parse(REFERENCE).expect("reference schema")
}

fn st(name: &str) -> TypeSig {
    TypeSig::Struct(name.to_string())
}

#[test]
fn roots_fields_and_scalars() {
    let schema = reference();
    let view: &dyn SchemaView = &schema;
    assert_eq!(view.version(), (1, 3));
    assert_eq!(view.root("top"), Some(st("Top")));
    assert_eq!(view.root("ui"), Some(st("Ui")));
    assert_eq!(view.root("nope"), None);

    let top = st("Top");
    assert_eq!(view.field(&top, "gold"), Some(TypeSig::Int));
    assert_eq!(
        view.field(&top, "tourism"),
        Some(TypeSig::Opt(Box::new(TypeSig::Int)))
    );
    assert_eq!(view.field(&top, "nope"), None);
    assert_eq!(view.field(&TypeSig::Int, "x"), None);
    assert_eq!(view.field(&st("Missing"), "x"), None);

    let res = st("Resource");
    assert_eq!(view.field(&res, "id"), Some(TypeSig::Key("item".into())));
    assert_eq!(view.field(&res, "name"), Some(TypeSig::Text));
    assert_eq!(view.field(&res, "available"), Some(TypeSig::Int));
    assert_eq!(view.field(&res, "share"), Some(TypeSig::Float));
    assert_eq!(view.field(&st("CityBanner"), "color"), Some(TypeSig::Color));
}

#[test]
fn records_lists_enums_and_options() {
    let schema = reference();
    let view: &dyn SchemaView = &schema;
    let resources = view.field(&st("Top"), "resources").expect("list");
    assert_eq!(resources, TypeSig::List(Box::new(st("Resource"))));
    assert_eq!(view.list_key(&resources), Some("id"));
    let cities = view.field(&st("Ui"), "cities").expect("list");
    assert_eq!(view.list_key(&cities), Some("key"));
    assert_eq!(view.list_key(&TypeSig::Int), None);
    assert_eq!(view.list_key(&TypeSig::List(Box::new(TypeSig::Int))), None);
    assert_eq!(view.list_key(&TypeSig::List(Box::new(st("Plot")))), None);

    assert_eq!(view.field(&st("Ui"), "pos"), Some(st("Plot")));
    assert_eq!(
        view.field(&st("Ui"), "tab"),
        Some(TypeSig::Enum(vec![
            "food".into(),
            "production".into(),
            "gold".into()
        ]))
    );
    // A record that may be absent is read through its option.
    let production = view
        .field(&st("CityBanner"), "production")
        .expect("optional record");
    assert_eq!(production, TypeSig::Opt(Box::new(st("BannerProd"))));
    assert_eq!(
        view.field(&production, "turns"),
        Some(TypeSig::Opt(Box::new(TypeSig::Int)))
    );
}

#[test]
fn paths_resolve_the_way_the_package_stores_them() {
    let schema = reference();
    let view: &dyn SchemaView = &schema;
    assert_eq!(resolve_path(view, "top.gold"), Some(TypeSig::Int));
    assert_eq!(
        resolve_path(view, "top.resources[].name"),
        Some(TypeSig::Text)
    );
    assert_eq!(
        resolve_path(view, "ui.cities[].production.turns"),
        Some(TypeSig::Opt(Box::new(TypeSig::Int)))
    );
    assert_eq!(resolve_path(view, "top"), Some(st("Top")));
    assert_eq!(resolve_path(view, "top.gold[]"), None);
    assert_eq!(resolve_path(view, "top.resources.name"), None);
    assert_eq!(resolve_path(view, "top.nope"), None);
    assert_eq!(resolve_path(view, "nope"), None);
    assert_eq!(resolve_path(view, ""), None);
}

#[test]
fn intents_have_named_typed_arguments_in_declaration_order() {
    let schema = reference();
    let view: &dyn SchemaView = &schema;
    assert_eq!(
        view.intent("unit.action"),
        Some(vec![
            ("unit".to_string(), TypeSig::Key("unit".into())),
            (
                "action".to_string(),
                TypeSig::Enum(vec!["move".into(), "fortify".into()])
            ),
        ])
    );
    let rename = view.intent("city.rename").expect("intent");
    assert_eq!(
        rename[2],
        ("note".to_string(), TypeSig::Opt(Box::new(TypeSig::Text)))
    );
    assert_eq!(view.intent("unit.dance"), None);
}

#[test]
fn only_js_fields_are_writable() {
    let schema = reference();
    let view: &dyn SchemaView = &schema;
    assert!(view.js_writable("ui.tab"));
    assert!(view.js_writable("ui.zoom"));
    assert!(!view.js_writable("ui.pos"));
    assert!(!view.js_writable("ui.pos.x"));
    assert!(!view.js_writable("top.gold"));
    assert!(!view.js_writable("ui.nope"));
    assert!(!view.js_writable(""));
}

/// The type of the schema source's notation (`i16?`, `list<Resource>`, `key<item>`) as the
/// package sees it.
fn from_source(ty: &str, enums: &BTreeMap<String, Vec<String>>) -> TypeSig {
    if let Some(inner) = ty.strip_suffix('?') {
        return TypeSig::Opt(Box::new(from_source(inner, enums)));
    }
    if let Some(elem) = ty.strip_prefix("list<").and_then(|r| r.strip_suffix('>')) {
        return TypeSig::List(Box::new(st(elem)));
    }
    if let Some(kind) = ty.strip_prefix("key<").and_then(|r| r.strip_suffix('>')) {
        return TypeSig::Key(kind.to_string());
    }
    match ty {
        "bool" => TypeSig::Bool,
        "i8" | "i16" | "i32" | "u32" => TypeSig::Int,
        "f32" => TypeSig::Float,
        "str" => TypeSig::Str,
        "text" => TypeSig::Text,
        "asset" => TypeSig::Asset,
        "color" => TypeSig::Color,
        other => match enums.get(other) {
            Some(variants) => TypeSig::Enum(variants.clone()),
            None => st(other),
        },
    }
}

#[test]
fn every_path_of_the_generated_schema_has_the_type_the_json_gives_it() {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/gen/schema.gen.json"))
        .expect("schema.gen.json");
    let json: Json = serde_json::from_str(&text).expect("valid json");
    let enums: BTreeMap<String, Vec<String>> = json["enums"]
        .as_array()
        .expect("enums")
        .iter()
        .map(|e| {
            let variants = e["variants"]
                .as_array()
                .expect("variants")
                .iter()
                .map(|v| v.as_str().expect("variant").to_string())
                .collect();
            (e["name"].as_str().expect("name").to_string(), variants)
        })
        .collect();
    let schema = Schema::v1();
    let view: &dyn SchemaView = schema;
    let paths = json["paths"].as_array().expect("paths");
    assert!(paths.len() > 50, "{} paths", paths.len());
    for p in paths {
        let path = p["path"].as_str().expect("path");
        let want = from_source(p["type"].as_str().expect("type"), &enums);
        assert_eq!(resolve_path(view, path), Some(want), "{path}");
        if let Some(key) = p["list_key"].as_str() {
            let list = resolve_path(view, path).expect("list type");
            assert_eq!(view.list_key(&list), Some(key), "{path}");
        }
    }
    let (major, minor) = view.version();
    assert_eq!(
        format!("{major}.{minor}"),
        json["version"].as_str().expect("version")
    );
    for root in json["roots"].as_array().expect("roots") {
        let name = root["name"].as_str().expect("name");
        assert_eq!(
            view.root(name),
            Some(st(root["type"].as_str().expect("type"))),
            "{name}"
        );
    }
}

fn manifest() -> Manifest {
    Manifest {
        kind: Kind::Skin,
        id: "probe".into(),
        version: "1.0.0".into(),
        title_key: "skin.title".into(),
        profile: (1, 0),
        model: (1, 0),
        api: (1, 0),
        default: false,
        tool: Tool {
            converter: "test".into(),
            chrome: "1".into(),
            chrome_flags: "00".into(),
            fonttools: "1".into(),
        },
        themes: vec!["dark".into()],
        languages: vec!["en_US".into()],
        font_roles: FontRoles {
            sans: FaceFamily("Sans".into()),
            display: FaceFamily("Sans".into()),
        },
        schema_hash: Schema::v1().hash(),
    }
}

#[test]
fn a_package_checks_its_paths_against_the_real_schema() {
    let read = |b: PackageBuilder| {
        let bytes = b.finish(manifest()).expect("finish");
        Package::read(Arc::from(bytes.as_slice())).expect("read")
    };
    let mut b = PackageBuilder::new();
    b.used_path("top.gold", TypeSig::Int);
    b.used_path("top.resources[].icon", TypeSig::Asset);
    b.used_path("top.resources", TypeSig::List(Box::new(st("Resource"))));
    b.used_path("research.current", TypeSig::Opt(Box::new(st("Tech"))));
    let pkg = read(b);
    pkg.check_paths(Schema::v1())
        .expect("the paths exist with these types");

    let mut b = PackageBuilder::new();
    b.used_path("top.gold", TypeSig::Str);
    assert!(matches!(
        read(b).check_paths(Schema::v1()),
        Err(PackageError::Incompatible { .. })
    ));
    let mut b = PackageBuilder::new();
    b.used_path("top.nope", TypeSig::Int);
    assert!(matches!(
        read(b).check_paths(Schema::v1()),
        Err(PackageError::Incompatible { .. })
    ));
}
