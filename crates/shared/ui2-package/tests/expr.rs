//! The expression language against `profile/expr-vectors.json`, the file the preview script and
//! the runtime run too.
#![cfg(feature = "profile")]

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde_json::Value as Json;
use ui2_package::profile::expr::{
    self, EvalHost, Expr, Scope, Template, TplSeg, Value, check_template, codes, eval, fmt_float,
    render, typecheck,
};
use ui2_package::profile::{Profile, SchemaView, TypeSig};

#[derive(Deserialize)]
struct VarDef {
    #[serde(rename = "type")]
    ty: String,
    origin: Option<String>,
}

#[derive(Deserialize)]
struct Vector {
    expr: Option<String>,
    tpl: Option<String>,
    #[serde(rename = "type")]
    ty: Option<String>,
    value: Option<Json>,
    text: Option<String>,
    error: Option<String>,
    #[serde(default)]
    warns: usize,
    #[serde(default)]
    model: BTreeMap<String, Json>,
}

#[derive(Deserialize)]
struct File {
    types: BTreeMap<String, String>,
    vars: BTreeMap<String, VarDef>,
    state: BTreeMap<String, String>,
    extra: BTreeMap<String, String>,
    model: BTreeMap<String, Json>,
    vectors: Vec<Vector>,
}

fn file() -> File {
    serde_json::from_str(include_str!("../profile/expr-vectors.json")).expect("vectors parse")
}

/// `int`, `list<struct a.b[]>`, `enum(a|b)`, `key<city>`, `struct p?` as `TypeSig`.
fn parse_type(s: &str) -> TypeSig {
    if let Some(inner) = s.strip_suffix('?') {
        return TypeSig::Opt(Box::new(parse_type(inner)));
    }
    match s {
        "bool" => TypeSig::Bool,
        "int" => TypeSig::Int,
        "float" => TypeSig::Float,
        "str" => TypeSig::Str,
        "text" => TypeSig::Text,
        "color" => TypeSig::Color,
        "asset" => TypeSig::Asset,
        _ => {
            if let Some(name) = s.strip_prefix("struct ") {
                TypeSig::Struct(name.to_string())
            } else if let Some(kind) = s.strip_prefix("key<").and_then(|r| r.strip_suffix('>')) {
                TypeSig::Key(kind.to_string())
            } else if let Some(list) = s.strip_prefix("list<").and_then(|r| r.strip_suffix('>')) {
                TypeSig::List(Box::new(parse_type(list)))
            } else if let Some(values) = s.strip_prefix("enum(").and_then(|r| r.strip_suffix(')')) {
                TypeSig::Enum(values.split('|').map(str::to_string).collect())
            } else {
                panic!("unknown type {s:?}")
            }
        }
    }
}

/// A schema made of the flat type map of the vector file.
struct MapView(BTreeMap<String, TypeSig>);

impl SchemaView for MapView {
    fn root(&self, name: &str) -> Option<TypeSig> {
        let prefix = format!("{name}.");
        self.0
            .keys()
            .any(|k| k.starts_with(&prefix))
            .then(|| TypeSig::Struct(name.to_string()))
    }

    fn field(&self, of: &TypeSig, name: &str) -> Option<TypeSig> {
        let of = match of {
            TypeSig::Opt(inner) => inner.as_ref(),
            other => other,
        };
        let TypeSig::Struct(path) = of else {
            return None;
        };
        self.0.get(&format!("{path}.{name}")).cloned()
    }

    fn list_key(&self, _list: &TypeSig) -> Option<&str> {
        None
    }

    fn intent(&self, _name: &str) -> Option<Vec<(String, TypeSig)>> {
        None
    }

    fn js_writable(&self, _path: &str) -> bool {
        false
    }

    fn version(&self) -> (u16, u16) {
        (1, 0)
    }
}

struct TestHost<'a> {
    model: &'a BTreeMap<String, Json>,
    warns: usize,
}

fn to_value(j: &Json) -> Value {
    match j {
        Json::Null => Value::Null,
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => match n.as_i64() {
            Some(i) => Value::Int(i),
            None => Value::Float(n.as_f64().expect("finite")),
        },
        Json::String(s) => Value::Str(s.clone()),
        Json::Array(a) => Value::List(a.len()),
        Json::Object(o) => match o.get("$color").and_then(Json::as_str) {
            Some(hex) => {
                let digits = hex.trim_start_matches('#');
                let v = u32::from_str_radix(digits, 16).expect("hex color");
                Value::Color(if digits.len() == 6 {
                    (v << 8) | 0xff
                } else {
                    v
                })
            }
            None => Value::Struct,
        },
    }
}

impl EvalHost for TestHost<'_> {
    fn path(&mut self, segs: &[String]) -> Value {
        self.model
            .get(&segs.join("."))
            .map_or(Value::Null, to_value)
    }

    fn loc(&mut self, key: &str, args: &[Value]) -> String {
        let mut out = key.to_string();
        for a in args {
            out.push('|');
            out.push_str(&render(a));
        }
        out
    }

    fn fmt_int(&mut self, n: i64) -> String {
        let digits = n.unsigned_abs().to_string();
        let mut grouped = String::new();
        for (i, c) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i).is_multiple_of(3) {
                grouped.push(',');
            }
            grouped.push(c);
        }
        if n < 0 {
            format!("-{grouped}")
        } else {
            grouped
        }
    }

    fn warn(&mut self, _message: &str) {
        self.warns += 1;
    }
}

fn expected_json(v: &Value) -> Json {
    match v {
        Value::Null => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        Value::Int(i) => Json::from(*i),
        Value::Float(f) => Json::from(*f),
        Value::Str(s) | Value::Text(s) => Json::from(s.clone()),
        Value::Color(c) => serde_json::json!({ "$color": format!("#{c:08x}") }),
        Value::List(n) => serde_json::json!({ "$list": n }),
        Value::Struct => serde_json::json!({}),
    }
}

fn same(a: &Json, b: &Json) -> bool {
    match (a, b) {
        (Json::Number(x), Json::Number(y)) => x.as_f64() == y.as_f64(),
        _ => a == b,
    }
}

struct Setup {
    view: MapView,
    scope: Scope,
}

fn setup(f: &File) -> Setup {
    let view = MapView(
        f.types
            .iter()
            .map(|(k, v)| (k.clone(), parse_type(v)))
            .collect(),
    );
    let mut scope = Scope::new();
    for (name, v) in &f.vars {
        scope = scope.with_var(name, parse_type(&v.ty), v.origin.clone());
    }
    for (path, ty) in &f.state {
        scope = scope.with_state(path, parse_type(ty));
    }
    for (path, ty) in &f.extra {
        scope = scope.with_extra(path, parse_type(ty));
    }
    Setup { view, scope }
}

/// Runs one vector and describes the first thing that is wrong.
fn run(f: &File, s: &Setup, v: &Vector) -> Result<(), String> {
    let mut model = f.model.clone();
    model.extend(v.model.clone());
    let mut host = TestHost {
        model: &model,
        warns: 0,
    };
    let (what, result): (&str, Result<(String, Value), expr::ExprError>) =
        if let Some(src) = &v.expr {
            (src, run_expr(s, src, &mut host))
        } else if let Some(src) = &v.tpl {
            (src, run_tpl(s, src, &mut host))
        } else {
            return Err("a vector without expr or tpl".into());
        };
    match (result, &v.error) {
        (Err(e), Some(code)) => {
            if e.code == code {
                Ok(())
            } else {
                Err(format!("{what:?}: got {e}, want {code}"))
            }
        }
        (Err(e), None) => Err(format!("{what:?}: unexpected error {e}")),
        (Ok((ty, _)), Some(code)) => Err(format!("{what:?}: typed as {ty}, want error {code}")),
        (Ok((ty, value)), None) => {
            if v.expr.is_some() && v.ty.as_deref() != Some(ty.as_str()) {
                return Err(format!("{what:?}: type {ty}, want {:?}", v.ty));
            }
            if v.expr.is_some() {
                let want = v.value.clone().unwrap_or(Json::Null);
                if !same(&expected_json(&value), &want) {
                    return Err(format!("{what:?}: value {value:?}, want {want}"));
                }
            } else if let Value::Str(text) = &value
                && Some(text) != v.text.as_ref()
            {
                return Err(format!("{what:?}: text {text:?}, want {:?}", v.text));
            }
            if host.warns != v.warns {
                return Err(format!(
                    "{what:?}: {} warnings, want {}",
                    host.warns, v.warns
                ));
            }
            Ok(())
        }
    }
}

fn run_expr(s: &Setup, src: &str, host: &mut TestHost) -> Result<(String, Value), expr::ExprError> {
    let e = expr::parse(src)?;
    let ty = typecheck(&e, &s.scope, &s.view)?;
    Ok((ty.to_string(), eval(&e, host)))
}

fn run_tpl(s: &Setup, src: &str, host: &mut TestHost) -> Result<(String, Value), expr::ExprError> {
    let t = expr::parse_template(src)?;
    check_template(&t, &s.scope, &s.view)?;
    Ok(("text".into(), Value::Str(render_template(&t, host))))
}

fn render_template(t: &Template, host: &mut TestHost) -> String {
    let mut out = String::new();
    for seg in &t.segs {
        match seg {
            TplSeg::Lit(s) => out.push_str(s),
            TplSeg::Expr(e) => out.push_str(&render(&eval(e, host))),
        }
    }
    out
}

#[test]
fn vectors_are_green() {
    let f = file();
    let s = setup(&f);
    assert!(f.vectors.len() >= 150, "{} vectors", f.vectors.len());
    let failures: Vec<String> = f
        .vectors
        .iter()
        .filter_map(|v| run(&f, &s, v).err())
        .collect();
    assert!(failures.is_empty(), "{}\n", failures.join("\n"));
}

#[test]
fn vectors_cover_every_error_code_and_every_function() {
    let f = file();
    let used: BTreeSet<&str> = f
        .vectors
        .iter()
        .filter_map(|v| v.error.as_deref())
        .collect();
    for code in codes::ALL {
        // E4-010 (a loop variable name) is checked with `for` nodes, see tests/attrs.rs.
        if *code != codes::VAR_NAME && *code != codes::NOT_LIST {
            assert!(used.contains(code), "no vector for {code}");
        }
    }
    let text: String = f
        .vectors
        .iter()
        .filter_map(|v| v.expr.as_deref().or(v.tpl.as_deref()))
        .collect::<Vec<_>>()
        .join("\n");
    for func in expr::Func::ALL {
        assert!(
            text.contains(&format!("{}(", func.name())),
            "{}",
            func.name()
        );
    }
    let kinds = ["int", "float", "str", "bool", "null"];
    for k in kinds {
        assert!(f.vectors.iter().any(|v| v.ty.as_deref() == Some(k)), "{k}");
    }
}

#[test]
fn every_expression_code_has_a_text() {
    let profile = Profile::v1();
    for code in codes::ALL {
        let def = profile
            .error(code)
            .unwrap_or_else(|| panic!("no text for {code}"));
        assert!(!def.what.is_empty() && !def.hint.is_empty(), "{code}");
    }
}

#[test]
fn half_a_unit_rounds_away_from_zero() {
    assert_eq!(fmt_float(0.03125), "0.0313");
    assert_eq!(fmt_float(-0.03125), "-0.0313");
}

#[test]
fn paths_read_by_an_expression_are_reported_with_the_schema_types() {
    let f = file();
    let s = setup(&f);
    let e = expr::parse("c.pop + len(game.cities) + game.player.score").expect("parse");
    let a = expr::analyze(&e, &s.scope, &s.view).expect("analyze");
    let paths: Vec<(&str, String)> = a
        .paths
        .iter()
        .map(|(p, t)| (p.as_str(), t.to_string()))
        .collect();
    assert_eq!(
        paths,
        [
            ("game.cities", "list<struct game.cities[]>".to_string()),
            ("game.cities[].pop", "int".to_string()),
            ("game.player.score", "int".to_string()),
        ]
    );
    // The paths resolve back through the schema view the way the package checks them.
    for (p, t) in &a.paths {
        assert_eq!(
            ui2_package::profile::resolve_path(&s.view, p).as_ref(),
            Some(t),
            "{p}"
        );
    }
    assert_eq!(a.origin, None);
    let list = expr::parse("game.cities").expect("parse");
    let a = expr::analyze(&list, &s.scope, &s.view).expect("analyze");
    assert_eq!(a.origin.as_deref(), Some("game.cities"));
}

#[test]
fn a_for_scope_names_the_item_and_refuses_taken_names() {
    let f = file();
    let s = setup(&f);
    let list = expr::analyze(
        &expr::parse("game.cities").expect("parse"),
        &Scope::new(),
        &s.view,
    )
    .expect("analyze");
    let inner = Scope::new()
        .for_scope("u", Some("n"), &list, &s.view)
        .expect("scope");
    assert_eq!(inner.vars["u"].origin.as_deref(), Some("game.cities[]"));
    assert_eq!(inner.vars["n"].ty, TypeSig::Int);
    let e = expr::parse("u.pop + n").expect("parse");
    let a = expr::analyze(&e, &inner, &s.view).expect("analyze");
    assert_eq!(a.paths[0].0, "game.cities[].pop");
    for taken in ["game", "state", "mods", "null", "u", "9x", ""] {
        let err = inner
            .for_scope(taken, None, &list, &s.view)
            .expect_err(taken);
        assert_eq!(err.code, codes::VAR_NAME, "{taken}");
    }
    let not_list = expr::analyze(
        &expr::parse("game.turn").expect("parse"),
        &Scope::new(),
        &s.view,
    )
    .expect("analyze");
    assert_eq!(
        Scope::new()
            .for_scope("x", None, &not_list, &s.view)
            .expect_err("int")
            .code,
        codes::NOT_LIST
    );
}

#[test]
fn trees_survive_postcard_and_are_deterministic() {
    let e: Expr =
        expr::parse("game.alive ? fmt.int(game.gold / 2) : loc('k', 1.5, 'x')").expect("parse");
    let a = postcard::to_allocvec(&e).expect("encode");
    let b = postcard::to_allocvec(&e).expect("encode");
    assert_eq!(a, b);
    assert_eq!(postcard::from_bytes::<Expr>(&a), Ok(e));
}
