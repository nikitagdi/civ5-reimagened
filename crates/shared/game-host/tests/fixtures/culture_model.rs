use lua_sys::{Function, Lua, LuaValue, Table};
use serde_json::Value;
use ui2_model::{Sample, Schema};

fn json(value: LuaValue<'_>) -> Value {
    match value {
        LuaValue::Nil => Value::Null,
        LuaValue::Boolean(value) => Value::Bool(value),
        LuaValue::Number(value) if value.fract() == 0.0 => Value::from(value as i64),
        LuaValue::Number(value) => Value::from(value),
        LuaValue::String(value) => Value::String(value.to_str().unwrap().into()),
        LuaValue::Table(table) => {
            let array = table.raw_len() > 0
                || table
                    .metatable()
                    .is_some_and(|meta| meta.raw_get::<_, bool>("__screen_array").unwrap_or(false));
            if array {
                Value::Array(
                    (1..=table.raw_len())
                        .map(|index| json(table.raw_get(index).unwrap()))
                        .collect(),
                )
            } else {
                Value::Object(
                    table
                        .pairs::<String, LuaValue>()
                        .map(|pair| {
                            let (key, value) = pair.unwrap();
                            (key, json(value))
                        })
                        .collect(),
                )
            }
        }
        _ => panic!("screen returned a non-JSON value"),
    }
}

pub fn check(lua: &Lua, global: &str, screen: &str) {
    let table: Table = lua.globals().get(global).unwrap();
    let data: Function = table.get("data").unwrap();
    let value: LuaValue = data.call((0, lua.create_table())).unwrap();
    let root = json(value);
    let text = serde_json::json!({"model":{screen: root}}).to_string();
    Sample::parse(Schema::v1(), &text).unwrap_or_else(|errors| panic!("{}", errors.join("\n")));
}
