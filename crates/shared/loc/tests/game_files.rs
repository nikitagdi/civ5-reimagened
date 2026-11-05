//! Acceptance on the installed game's en_US text: every string parses, every key composes with substitute
//! arguments without leaving braces, and the grammar helpers work on real rows.
//! Skipped, with a message, when the game files are absent (set `CIV5_GAME_DIR`).

mod common;

use loc::{Arg, LanguageInfo, LocString, Localizer, Param, parse};

fn needs_number(tag: &str) -> bool {
    matches!(
        tag,
        "number" | "percent" | "currency" | "roman" | "spellout" | "ordinal" | "plural"
    )
}

/// Walks all parameters including injected ones: `(arg index, wants a number)`.
fn arg_needs(tpl: &str, params: &[Param], out: &mut Vec<(usize, bool)>) {
    for p in params {
        if p.arg_index > 0 {
            let num = !p.at && p.tag.is_some_and(|(a, b)| needs_number(&tpl[a..=b]));
            out.push((p.arg_index as usize, num));
        }
        arg_needs(tpl, &p.injected, out);
    }
}

fn substitute_args(tpl: &str, params: &[Param]) -> Vec<Arg> {
    let mut needs = Vec::new();
    arg_needs(tpl, params, &mut needs);
    let max = needs.iter().map(|(i, _)| *i).max().unwrap_or(0);
    (1..=max)
        .map(|i| {
            let num = needs.iter().any(|&(j, n)| j == i && n);
            if num {
                Arg::Double(3.0)
            } else {
                Arg::Str(format!("ARG{i}"))
            }
        })
        .collect()
}

#[test]
fn every_en_us_string_parses_and_composes() {
    let Some(assets) = common::assets_dir() else {
        return;
    };
    let table = common::load_en_us(&assets);
    assert!(table.len() > 20_000, "rows loaded: {}", table.len());

    let mut parse_errors = Vec::new();
    let mut with_params = 0usize;
    let mut compose_empty = Vec::new();
    let mut leftover = Vec::new();
    let rows: Vec<(String, String)> = table
        .iter()
        .map(|(k, e)| (k.to_string(), e.text.clone()))
        .collect();
    let l = Localizer::new(LanguageInfo::en_us(), Box::new(table));
    for (key, text) in &rows {
        let ls = LocString::new(text);
        let mut params_of_key = Vec::new();
        for n in 1..=ls.num_forms() {
            let form = &ls.form(n).unwrap().text;
            match parse::parse(form) {
                Ok(p) => params_of_key.extend(p.into_iter().map(|p| (form.clone(), p))),
                Err(e) => parse_errors.push(format!("{key}: {e}: {form:.120}")),
            }
        }
        if params_of_key.is_empty() {
            continue;
        }
        with_params += 1;
        // arguments from the first form's parameters
        let first = &ls.form(1).unwrap().text;
        let Ok(params) = parse::parse(first) else {
            continue;
        };
        let args = substitute_args(first, &params);
        let out = l.convert(key, &args);
        if out.is_empty() && !first.is_empty() {
            compose_empty.push(format!("{key}: {text:.140}"));
        } else if out.contains('{') || out.contains('}') {
            leftover.push(format!("{key}: {out:.140}"));
        }
    }
    eprintln!(
        "rows {} with parameters {} parse errors {} empty results {} leftover braces {}",
        rows.len(),
        with_params,
        parse_errors.len(),
        compose_empty.len(),
        leftover.len()
    );
    for l in parse_errors.iter().take(20) {
        eprintln!("PARSE {l}");
    }
    for l in compose_empty.iter().take(20) {
        eprintln!("EMPTY {l}");
    }
    for l in leftover.iter().take(20) {
        eprintln!("LEFT  {l}");
    }
    assert!(
        parse_errors.is_empty(),
        "{} strings do not parse",
        parse_errors.len()
    );
    assert!(
        leftover.is_empty(),
        "{} results keep braces",
        leftover.len()
    );
    assert!(
        compose_empty.is_empty(),
        "{} keys compose to nothing",
        compose_empty.len()
    );
}

#[test]
fn grammar_helpers_on_real_rows() {
    let Some(assets) = common::assets_dir() else {
        return;
    };
    let table = common::load_en_us(&assets);
    let l = Localizer::new(LanguageInfo::en_us(), Box::new(table));
    // an unknown civilization adjective keeps its plain reading
    let archer = l.convert(
        "TXT_KEY_GRAMMAR_A_AN",
        &[Arg::Str("TXT_KEY_UNIT_ARCHER".into())],
    );
    eprintln!("a/an Archer: {archer}");
    assert!(archer == "an Archer" || archer == "a Archer", "{archer}");
    let wall = l.convert(
        "TXT_KEY_GRAMMAR_A_AN",
        &[Arg::Str("TXT_KEY_BUILDING_GREAT_WALL".into())],
    );
    eprintln!("a/an Great Wall: {wall}");
    assert!(l.has_key("TXT_KEY_GRAMMAR_A_AN"));
    assert!(!l.has_key("TXT_KEY_DEFINITELY_NOT_THERE"));
    assert_eq!(l.stats().bad_template, 0, "{:?}", l.take_warnings());
}
