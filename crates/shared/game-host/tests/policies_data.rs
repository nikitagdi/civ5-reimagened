use lua_sys::{Function, Lua, LuaValue, Table};
use serde_json::{Value, json};
use ui2_model::{Sample, Schema};

fn value(v: LuaValue<'_>) -> Value {
    match v {
        LuaValue::Nil => Value::Null,
        LuaValue::Boolean(b) => b.into(),
        LuaValue::Number(n) => {
            assert!(n.is_finite() && n.fract() == 0.0);
            (n as i64).into()
        }
        LuaValue::String(s) => s.to_str().unwrap().into(),
        LuaValue::Table(t) => {
            if t.raw_len() > 0
                || t.metatable()
                    .is_some_and(|m| m.raw_get::<_, bool>("__screen_array").unwrap_or(false))
            {
                Value::Array(
                    (1..=t.raw_len())
                        .map(|i| value(t.raw_get(i).unwrap()))
                        .collect(),
                )
            } else {
                Value::Object(
                    t.pairs::<String, LuaValue>()
                        .map(|pair| {
                            let (k, v) = pair.unwrap();
                            (k, value(v))
                        })
                        .collect(),
                )
            }
        }
        _ => panic!("non-JSON data"),
    }
}

fn data(lua: &Lua, name: &str, setup: &str) -> Value {
    lua.exec(setup, "=seat-state").unwrap();
    let source = game_host::screens::REGISTRY
        .iter()
        .find(|(n, _)| *n == name)
        .unwrap()
        .1;
    let screen: Table = lua.load(source).eval().unwrap();
    let f: Function = screen.get("data").unwrap();
    let args = lua.create_table();
    args.set("a", 4).unwrap();
    let root = value(f.call((3, args)).unwrap());
    Sample::parse(Schema::v1(), &json!({"model":{name:root}}).to_string())
        .unwrap_or_else(|e| panic!("{name}: {e:?}"));
    root
}

#[test]
fn policy_group_data_validates_and_masks_unmet_identity() {
    let lua = Lua::new().unwrap();
    lua.exec(include_str!("../screens/_lib.lua"), "=helpers")
        .unwrap();
    lua.exec(include_str!("fixtures/policies_api.lua"), "=policy-api")
        .unwrap();
    let root = data(&lua, "policies", "");
    assert_eq!(root["branches"][0]["state"], "era_locked");
    let root = data(&lua, "policies", "era=1; blocked=true");
    assert_eq!(root["branches"][0]["state"], "blocked");
    let root = data(&lua, "policies", "cities=0");
    assert_eq!(root["branches"][0]["reason"], "Found a city first");
    let root = data(&lua, "policies", "cities=1; ideology=9");
    // The ideology tab as the original: the opinion's own text, the core's pressure tooltip, the switch tooltip.
    let opinion = &root["ideology"]["public_opinion"];
    assert_eq!(opinion["label"], "TXT_KEY_CO_PUBLIC_OPINION_CONTENT");
    assert_eq!(opinion["tooltip"], "Content");
    assert_eq!(opinion["unhappiness_tooltip"], "No unhappiness");
    assert_eq!(
        root["ideology"]["title"],
        "TXT_KEY_POLICYSCREEN_IDEOLOGY_TITLE"
    );
    assert_eq!(
        root["ideology"]["switch"]["reason"],
        "TXT_KEY_POLICYSCREEN_CHANGE_IDEOLOGY_DISABLED_TT"
    );
    let root = data(&lua, "ideology", "");
    let unmet = root["options"][0]["adopted_by"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["key"] == 4)
        .unwrap();
    assert_eq!(unmet["known"], false);
    assert!(unmet.get("civ").is_none());
    data(&lua, "pantheon", "");
    data(&lua, "religion", "");
    let root = data(&lua, "religion_overview", "");
    assert!(root["world"][0].get("founder").is_none());
    assert!(root["world"][0].get("holy_city").is_none());
    let serialized = root.to_string();
    assert!(!serialized.contains("Hidden city") && !serialized.contains("Foreign civilization"));
    let root = data(&lua, "religion", "ownreligion=true");
    // "Founded in" with the current year, as the original popup labels it.
    assert_eq!(
        root["religion"]["founded"],
        "TXT_KEY_CHOOSE_RELIGION_FOUNDED_IN"
    );
    assert_eq!(root["date"], "TXT_KEY_CHOOSE_RELIGION_FOUNDED_IN");
    data(&lua, "religion_overview", "");
}
