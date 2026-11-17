use lua_sys::LuaValue;
pub fn json(v: LuaValue<'_>) -> serde_json::Value {
    match v {
        LuaValue::Nil => serde_json::Value::Null,
        LuaValue::Boolean(b) => b.into(),
        LuaValue::Number(n) => {
            if n.fract() == 0.0 {
                (n as i64).into()
            } else {
                serde_json::json!(n)
            }
        }
        LuaValue::String(s) => s.to_str().unwrap().into(),
        LuaValue::Table(t) => {
            if t.raw_len() > 0
                || t.metatable()
                    .is_some_and(|m| m.raw_get::<_, bool>("__screen_array").unwrap_or(false))
            {
                serde_json::Value::Array(
                    (1..=t.raw_len())
                        .map(|i| json(t.raw_get(i).unwrap()))
                        .collect(),
                )
            } else {
                serde_json::Value::Object(
                    t.pairs::<String, LuaValue>()
                        .map(|p| {
                            let (k, v) = p.unwrap();
                            (k, json(v))
                        })
                        .collect(),
                )
            }
        }
        _ => panic!("not JSON"),
    }
}
