use std::cell::RefCell;
use std::rc::Rc;

use gamecore::NetMessage;
use lua_sys::{Function, Lua, LuaError, LuaValue, Table, Variadic};
use serde_json::Value as Json;

const MAX_JSON: usize = 512 * 1024;
const MAX_MESSAGES: usize = 256;
/// The longest text of an action or of a net message sent with it, in bytes.
const MAX_TEXT: usize = 256;
/// The net messages that carry a string: renames, a religion's own name, a league's name.
const TEXT_MESSAGES: &[&str] = &[
    "SendRenameCity",
    "SendRenameUnit",
    "SendFoundReligion",
    "SendEnhanceReligion",
    "SendLeagueEditName",
];

pub(crate) struct ActionOutput {
    pub messages: Vec<NetMessage>,
    pub done: Option<i32>,
    pub open: Option<protocol::ScreenOpen>,
    pub close: bool,
}

pub(crate) struct Screens {
    lua: Lua,
}

impl Screens {
    pub fn new() -> lua_sys::Result<Self> {
        let lua = Lua::new()?;
        lua.sandbox()?;
        lua.set_instruction_limit(Some(20_000_000));
        Ok(Self { lua })
    }

    pub fn lua(&self) -> &Lua {
        &self.lua
    }

    pub fn load(&self) -> lua_sys::Result<()> {
        self.lua
            .exec(include_str!("_lib.lua"), "=screens/_lib.lua")?;
        let popups: Table = self.lua.load(include_str!("_popups.lua")).eval()?;
        self.lua.globals().raw_set("__popups", popups)?;
        let registry = self.lua.create_table();
        for (name, source) in super::REGISTRY {
            let screen: Table = self
                .lua
                .load(source)
                .set_name(&format!("=screens/{name}.lua"))
                .eval()?;
            let _: Function = screen.get("data")?;
            registry.raw_set(*name, screen)?;
        }
        self.lua.globals().raw_set("__screens", registry)
    }

    /// `__screen_call(f, ...)` of `_lib.lua`: a screen function run inside the core's read scope, which puts back
    /// the synced state a Lua getter may change on this machine only.
    fn scoped(&self) -> lua_sys::Result<Function<'_>> {
        self.lua.globals().get("__screen_call")
    }

    /// The seat's client reads the HUD's popup head, so it answers the core's war question before a move: the core may
    /// ask it (`GcUiSetMoveQuestions`; without it, or with the stub, nothing to do).
    pub fn answers_move_questions(&self, player: i8) -> lua_sys::Result<()> {
        if let LuaValue::Function(f) = self.lua.globals().raw_get("GcUiSetMoveQuestions")? {
            let _: LuaValue = f.call((i32::from(player), true))?;
        }
        Ok(())
    }

    /// Where a seat's unit stands: `Some(Some((x, y)))`, `Some(None)` when it is gone, `None` without the core.
    pub fn unit_plot(&self, player: i8, unit: i32) -> lua_sys::Result<Option<Option<(i32, i32)>>> {
        let f: Function = self.lua.globals().get("__unit_plot")?;
        let (known, x, y): (bool, Option<i32>, Option<i32>) =
            self.scoped()?.call((f, i32::from(player), unit))?;
        Ok(known.then_some(x.zip(y)))
    }

    pub fn popup_kind(&self, number: i32) -> lua_sys::Result<Option<String>> {
        let enums: Table = self.lua.globals().get("ButtonPopupTypes")?;
        for pair in enums.pairs::<String, LuaValue>() {
            let (name, value) = pair?;
            if matches!(value, LuaValue::Number(n) if n == f64::from(number)) {
                return Ok(Some(name));
            }
        }
        Ok(None)
    }

    pub fn context(&self, entry: Option<&crate::popups::Entry>) -> lua_sys::Result<()> {
        let value = match entry {
            Some(entry) => LuaValue::Table(self.popup_table(&entry.popup, entry.head.id)?),
            None => LuaValue::Nil,
        };
        self.lua.globals().raw_set("__popup", value)
    }

    fn popup_table(&self, p: &crate::popups::Popup, id: i32) -> lua_sys::Result<Table<'_>> {
        let t = self.lua.create_table();
        t.raw_set("id", id)?;
        t.raw_set("kind", p.kind.as_str())?;
        t.raw_set("data1", p.data1)?;
        t.raw_set("data2", p.data2)?;
        t.raw_set("data3", p.data3)?;
        if let Some(v) = p.data4 {
            t.raw_set("data4", v)?;
        }
        if let Some(v) = p.data5 {
            t.raw_set("data5", v)?;
        }
        t.raw_set("flag", p.flag)?;
        t.raw_set("option1", p.option1)?;
        t.raw_set("option2", p.option2)?;
        if let Some(text) = &p.text {
            t.raw_set("text", text.as_str())?;
        }
        Ok(t)
    }

    pub fn map_popup(
        &self,
        popup: &crate::popups::Popup,
        id: i32,
    ) -> lua_sys::Result<Option<crate::popups::PopupHead>> {
        let mappings: Table = self.lua.globals().raw_get("__popups")?;
        let LuaValue::Table(mapping) = mappings.raw_get(popup.kind.as_str())? else {
            return Ok(None);
        };
        let screen: String = mapping.get("screen")?;
        if !screen
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_lowercase)
            || !screen
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            return Err(LuaError::msg("invalid popup screen name"));
        }
        let payload = self.popup_table(popup, id)?;
        let argument = |name| -> lua_sys::Result<Option<i32>> {
            match mapping.raw_get(name)? {
                LuaValue::Nil => Ok(None),
                LuaValue::Number(n)
                    if n.fract() == 0.0 && n >= i32::MIN as f64 && n <= i32::MAX as f64 =>
                {
                    Ok(Some(n as i32))
                }
                LuaValue::Function(f) => self.scoped()?.call((f, payload.clone())),
                _ => Err(LuaError::msg("invalid popup argument expression")),
            }
        };
        Ok(Some(crate::popups::PopupHead {
            id,
            screen,
            a: argument("a")?,
            b: argument("b")?,
            decision: mapping.get("decision")?,
        }))
    }

    /// Why a queued decision can no longer be answered (its city or unit is gone, the choice was made another way):
    /// the `lapsed(player, popup)` of its mapping, run in the read scope. `None`: still pending, or the mapping has
    /// no such test. A decision stays until its screen action completes it, so a lapsed one would otherwise block
    /// the seat's queue for good.
    pub fn popup_lapsed(
        &self,
        player: i8,
        entry: &crate::popups::Entry,
    ) -> lua_sys::Result<Option<String>> {
        let mappings: Table = self.lua.globals().raw_get("__popups")?;
        let LuaValue::Table(mapping) = mappings.raw_get(entry.popup.kind.as_str())? else {
            return Ok(None);
        };
        let LuaValue::Function(f) = mapping.raw_get("lapsed")? else {
            return Ok(None);
        };
        let payload = self.popup_table(&entry.popup, entry.head.id)?;
        let value: LuaValue = self.scoped()?.call((f, i32::from(player), payload))?;
        Ok(match value {
            LuaValue::Nil | LuaValue::Boolean(false) => None,
            LuaValue::String(s) => Some(s.to_str()?.to_owned()),
            _ => Some("the decision lapsed".into()),
        })
    }

    fn screen(&self, name: &str) -> lua_sys::Result<Table<'_>> {
        let registry: Table = self.lua.globals().raw_get("__screens")?;
        registry
            .raw_get(name)
            .map_err(|_| LuaError::msg(format!("unknown screen {name}")))
    }

    fn args(&self, args: &[(String, i64)]) -> lua_sys::Result<Table<'_>> {
        if args.len() > 32 {
            return Err(LuaError::msg("too many screen arguments"));
        }
        let table = self.lua.create_table();
        for (key, value) in args {
            if key.len() > 64
                || table.contains_key(key.as_str())?
                // SIMPLIFIED(SIMP-323): Lua arguments reject integers outside the exact double range.
                || value.unsigned_abs() > (1u64 << 53)
            {
                return Err(LuaError::msg("invalid or duplicate screen argument"));
            }
            table.raw_set(key.as_str(), *value as f64)?;
        }
        if let LuaValue::Table(popup) = self.lua.globals().raw_get("__popup")? {
            table.raw_set("popup_id", popup.raw_get::<_, i32>("id")?)?;
            table.raw_set("popup", popup)?;
        }
        Ok(table)
    }

    pub fn data(&self, player: i8, name: &str, args: &[(String, i64)]) -> lua_sys::Result<String> {
        let data: Function = self.screen(name)?.get("data")?;
        let value: LuaValue = self
            .scoped()?
            .call((data, i32::from(player), self.args(args)?))?;
        let mut remaining = MAX_JSON;
        let value = json(value, 0, &mut remaining)?;
        if !value.is_object() {
            return Err(LuaError::msg("screen data must return an object"));
        }
        let text = serde_json::to_string(&value).map_err(|e| LuaError::msg(e.to_string()))?;
        if text.len() > MAX_JSON {
            return Err(LuaError::msg("screen response exceeds 512 KiB"));
        }
        self.lua.gc_collect()?;
        Ok(text)
    }

    pub fn action(
        &self,
        player: i8,
        name: &str,
        action: &str,
        args: &[(String, i64)],
        text: Option<&str>,
    ) -> lua_sys::Result<ActionOutput> {
        if text.is_some_and(|s| s.len() > MAX_TEXT || s.chars().any(char::is_control)) {
            return Err(LuaError::msg("invalid screen action text"));
        }
        if args.iter().any(|(key, _)| key == "text") {
            return Err(LuaError::msg("text is a reserved screen argument"));
        }
        let arguments = self.args(args)?;
        if let Some(text) = text {
            arguments.raw_set("text", text)?;
        }
        let actions: Table = self.screen(name)?.get("actions")?;
        let function: Function = actions.get(action)?;
        let messages = Rc::new(RefCell::new(Vec::new()));
        let output = Rc::clone(&messages);
        let net = self.lua.create_table();
        net.raw_set(
            "send",
            self.lua
                .create_function(move |_, (name, args): (String, Variadic<i32>)| {
                    let message = NetMessage::new(&name, &args.0)
                        .filter(|m| gamecore::replica::net_message_from_ui(m.id))
                        .ok_or_else(|| LuaError::msg("unknown or forbidden UI net message"))?;
                    let mut output = output.borrow_mut();
                    if output.len() >= MAX_MESSAGES {
                        return Err(LuaError::msg("too many screen net messages"));
                    }
                    output.push(message);
                    Ok(())
                }),
        )?;
        let done = Rc::new(RefCell::new(None));
        let done_output = Rc::clone(&done);
        net.raw_set(
            "popup_done",
            self.lua.create_function(move |_, id: i32| {
                if done_output.borrow().is_some() {
                    return Err(LuaError::msg("popup already finished"));
                }
                *done_output.borrow_mut() = Some(id);
                Ok(())
            }),
        )?;
        // `net.send_text(name, text, ...)`: the whitelisted messages that carry a string (a rename, a religion's
        // own name), the numbers after the text being the message's numeric arguments, as for `net.send`.
        let output = Rc::clone(&messages);
        net.raw_set(
            "send_text",
            self.lua
                .create_function(move |_, all: Variadic<LuaValue<'_>>| {
                    let text_of = |value: Option<&LuaValue<'_>>| match value {
                        Some(LuaValue::String(s)) => Ok(String::from_utf8_lossy(&s.0).into_owned()),
                        _ => Err(LuaError::msg("send_text expects a name and a text")),
                    };
                    let name = text_of(all.0.first())?;
                    let text = text_of(all.0.get(1))?;
                    let mut args = Vec::new();
                    for value in all.0.iter().skip(2) {
                        match value {
                            LuaValue::Number(n)
                                if n.fract() == 0.0
                                    && *n >= f64::from(i32::MIN)
                                    && *n <= f64::from(i32::MAX) =>
                            {
                                args.push(*n as i32);
                            }
                            _ => {
                                return Err(LuaError::msg("send_text arguments are whole numbers"));
                            }
                        }
                    }
                    if !TEXT_MESSAGES.contains(&name.as_str()) {
                        return Err(LuaError::msg("this net message carries no text"));
                    }
                    if text.len() > MAX_TEXT || text.chars().any(char::is_control) {
                        return Err(LuaError::msg("invalid screen net message text"));
                    }
                    let mut message = NetMessage::new(&name, &args)
                        .filter(|m| gamecore::replica::net_message_from_ui(m.id))
                        .ok_or_else(|| LuaError::msg("unknown or forbidden UI net message"))?;
                    message.text = text;
                    let mut output = output.borrow_mut();
                    if output.len() >= MAX_MESSAGES {
                        return Err(LuaError::msg("too many screen net messages"));
                    }
                    output.push(message);
                    Ok(())
                }),
        )?;
        let value: LuaValue = self
            .scoped()?
            .call((function, i32::from(player), arguments, net))?;
        let (open, close) = match value {
            LuaValue::Nil => (None, false),
            LuaValue::Table(t) => {
                let close = t.raw_get::<_, Option<bool>>("close")?.unwrap_or(false);
                let open = match t.raw_get("open")? {
                    LuaValue::Nil => None,
                    LuaValue::Table(o) => {
                        let screen: String = o.get("screen")?;
                        self.screen(&screen)?;
                        if matches!(screen.as_str(), "hud" | "rich_tooltips" | "civ_ribbon") {
                            return Err(LuaError::msg("service screen has no window"));
                        }
                        Some(protocol::ScreenOpen {
                            screen,
                            a: o.raw_get("a")?,
                            b: o.raw_get("b")?,
                        })
                    }
                    _ => return Err(LuaError::msg("invalid action open result")),
                };
                (open, close)
            }
            _ => return Err(LuaError::msg("action result must be a table")),
        };
        let result = ActionOutput {
            messages: messages.borrow().clone(),
            done: *done.borrow(),
            open,
            close,
        };
        self.lua.gc_collect()?;
        Ok(result)
    }
}

fn json(value: LuaValue<'_>, depth: usize, remaining: &mut usize) -> lua_sys::Result<Json> {
    if depth > 32 || *remaining == 0 {
        return Err(LuaError::msg("screen response exceeds structural limits"));
    }
    *remaining -= 1;
    Ok(match value {
        LuaValue::Nil => Json::Null,
        LuaValue::Boolean(v) => Json::Bool(v),
        LuaValue::Number(n) => {
            if !n.is_finite() {
                return Err(LuaError::msg("non-finite screen number"));
            }
            if n.fract() == 0.0 && n.abs() <= (1u64 << 53) as f64 {
                Json::from(n as i64)
            } else {
                Json::from(n)
            }
        }
        LuaValue::String(s) => {
            let s = s.to_str()?.to_owned();
            *remaining = remaining
                .checked_sub(s.len())
                .ok_or_else(|| LuaError::msg("screen response exceeds 512 KiB"))?;
            Json::String(s)
        }
        LuaValue::Table(t) => {
            let array = t.raw_len() > 0
                || t.metatable()
                    .is_some_and(|m| m.raw_get::<_, bool>("__screen_array").unwrap_or(false));
            if array {
                let len = t.raw_len();
                let mut count = 0;
                for pair in t.pairs::<LuaValue, LuaValue>() {
                    let (key, _) = pair?;
                    if !matches!(key, LuaValue::Number(n) if n >= 1.0 && n <= len as f64 && n.fract() == 0.0)
                    {
                        return Err(LuaError::msg("screen arrays must be dense"));
                    }
                    count += 1;
                }
                if count != len {
                    return Err(LuaError::msg("screen arrays must be dense"));
                }
                let mut values = Vec::new();
                for i in 1..=len {
                    values.push(json(t.raw_get(i)?, depth + 1, remaining)?);
                }
                Json::Array(values)
            } else {
                let mut values = serde_json::Map::new();
                for pair in t.pairs::<String, LuaValue>() {
                    let (key, value) = pair?;
                    *remaining = remaining
                        .checked_sub(key.len())
                        .ok_or_else(|| LuaError::msg("screen response exceeds 512 KiB"))?;
                    values.insert(key, json(value, depth + 1, remaining)?);
                }
                Json::Object(values)
            }
        }
        _ => return Err(LuaError::msg("screen data contains a non-JSON value")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime() -> Screens {
        let runtime = Screens::new().unwrap();
        runtime.load().unwrap();
        runtime.lua.exec(r#"
            __screens.test = {
                data = function(player, args) return {player = player, a = args.a, empty = array()} end,
                actions = {
                    research = function(player, args, net) net.send("sendResearch", args.a, 0, -1, args.b or 0) end,
                    forbidden = function(player, args, net) net.send("sendTurnComplete") end,
                    fail = function(player, args, net) net.send("sendResearch", 1); error("failed") end,
                },
            }
        "#, "=test").unwrap();
        runtime
    }

    #[test]
    fn popup_mapping_names_and_arguments_are_valid() {
        let r = runtime();
        let mappings: Table = r.lua.globals().raw_get("__popups").unwrap();
        for pair in mappings.pairs::<String, Table>() {
            let (kind, _) = pair.unwrap();
            assert!(kind.starts_with("BUTTONPOPUP_"));
            let popup = crate::popups::Popup {
                kind,
                data1: 3,
                data2: 4,
                data3: 5,
                ..Default::default()
            };
            let head = r.map_popup(&popup, 7).unwrap().unwrap();
            assert!(!head.screen.is_empty());
        }
        let popup = crate::popups::Popup {
            kind: "BUTTONPOPUP_TECH_AWARD".into(),
            data3: 5,
            ..Default::default()
        };
        assert_eq!(r.map_popup(&popup, 7).unwrap().unwrap().a, Some(5));
        assert!(
            r.map_popup(&crate::popups::Popup::default(), 8)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn actions_return_transitions_and_stage_popup_completion() {
        let r = runtime();
        r.lua
            .exec(
                r#"
            __screens.test.actions.next = function(player, args, net)
                net.popup_done(9)
                net.send("sendResearch", 3, 0, -1, 0)
                return {open = {screen = "techs", a = 2, b = 3}, close = true}
            end
            __screens.test.actions.invalid = function(player, args, net)
                net.popup_done(9)
                return {open = {screen = "missing"}}
            end
            __screens.test.actions.fail_done = function(player, args, net)
                net.popup_done(9)
                error("discard staged completion")
            end
        "#,
                "=transitions",
            )
            .unwrap();
        let result = r.action(0, "test", "next", &[], None).unwrap();
        assert_eq!(result.done, Some(9));
        assert!(result.close);
        assert_eq!(
            result.open,
            Some(protocol::ScreenOpen {
                screen: "techs".into(),
                a: Some(2),
                b: Some(3)
            })
        );
        assert_eq!(result.messages.len(), 1);
        assert!(r.action(0, "test", "invalid", &[], None).is_err());
        assert!(r.action(0, "test", "fail_done", &[], None).is_err());
        let result = r
            .action(0, "test", "research", &[("a".into(), 3)], None)
            .unwrap();
        assert_eq!(result.done, None);
        assert!(!result.close);
        assert_eq!(result.open, None);
    }

    #[test]
    fn data_and_messages_keep_the_explicit_seat_and_arguments() {
        let r = runtime();
        let json: Json =
            serde_json::from_str(&r.data(3, "test", &[("a".into(), 9)]).unwrap()).unwrap();
        assert_eq!(json, serde_json::json!({"player":3,"a":9,"empty":[]}));
        let messages = r
            .action(
                3,
                "test",
                "research",
                &[("a".into(), 12), ("b".into(), 1)],
                None,
            )
            .unwrap();
        assert_eq!(
            messages.messages,
            vec![NetMessage::new("sendResearch", &[12, 0, -1, 1]).unwrap()]
        );
        assert!(r.action(3, "test", "forbidden", &[], None).is_err());
        assert!(r.action(3, "test", "fail", &[], None).is_err());
        assert_eq!(
            r.action(3, "test", "research", &[("a".into(), 2)], None)
                .unwrap()
                .messages
                .len(),
            1
        );
        assert!(r.data(0, "missing", &[]).is_err());
        assert!(
            r.data(0, "test", &[("a".into(), 1), ("a".into(), 2)])
                .is_err()
        );
    }

    #[test]
    fn errors_and_limits_leave_the_interpreter_usable() {
        let r = runtime();
        for source in [
            "function() while true do end end",
            "function() return {text = string.rep('x', 524289)} end",
            "function() local t = {}; t.self = t; return t end",
            "function() return {invalid = function() end} end",
            "function() error('screen failed') end",
        ] {
            r.lua
                .exec(&format!("__screens.bad = {{data = {source}}}"), "=bad")
                .unwrap();
            assert!(r.data(0, "bad", &[]).is_err());
            assert!(r.data(0, "test", &[]).is_ok());
        }
    }

    #[test]
    fn action_text_is_optional_utf8_bounded_and_free_of_controls() {
        let r = runtime();
        r.lua
            .exec(
                r#"
            __screens.test.actions.text = function(player, args, net)
                assert(args.text == expected_text)
            end
        "#,
                "=text-test",
            )
            .unwrap();
        r.action(3, "test", "text", &[], None).unwrap();
        for value in ["", "New city", "\u{e9}", &"\u{e9}".repeat(128)] {
            r.lua.globals().raw_set("expected_text", value).unwrap();
            r.action(3, "test", "text", &[], Some(value)).unwrap();
        }
        for value in [
            "\n",
            "\r",
            "\0",
            "\t",
            "\u{7f}",
            "\u{85}",
            &"x".repeat(257),
            &"\u{e9}".repeat(129),
        ] {
            assert!(r.action(3, "test", "text", &[], Some(value)).is_err());
        }
        assert!(
            r.action(3, "test", "text", &[("text".into(), 1)], None)
                .is_err()
        );
        r.lua
            .globals()
            .raw_set("expected_text", "Valid again")
            .unwrap();
        r.action(3, "test", "text", &[], Some("Valid again"))
            .unwrap();
    }

    #[test]
    fn send_text_attaches_a_bounded_text_to_whitelisted_string_messages() {
        let r = runtime();
        r.lua
            .exec(
                r#"
            __screens.test.actions.rename = function(player, args, net)
                net.send_text("SendRenameCity", args.text, args.a)
            end
            __screens.test.actions.religion = function(player, args, net)
                net.send_text("SendFoundReligion", args.text, 1, 2)
            end
            __screens.test.actions.plain = function(player, args, net)
                net.send_text("sendResearch", "x", 1)
            end
            __screens.test.actions.turn = function(player, args, net)
                net.send_text("sendTurnComplete", "x")
            end
        "#,
                "=send-text-test",
            )
            .unwrap();
        let m = r
            .action(
                3,
                "test",
                "rename",
                &[("a".into(), 7)],
                Some("New \u{e9} city"),
            )
            .unwrap();
        let mut expected = NetMessage::new("SendRenameCity", &[7]).unwrap();
        expected.text = "New \u{e9} city".into();
        assert_eq!(m.messages, vec![expected]);
        let m = r.action(3, "test", "religion", &[], Some("")).unwrap();
        assert_eq!(m.messages[0].name(), "SendFoundReligion");
        assert_eq!(
            (
                m.messages[0].args[0],
                m.messages[0].args[1],
                m.messages[0].text.as_str()
            ),
            (1, 2, "")
        );
        for bad in ["\n", "\t", &"x".repeat(257)] {
            assert!(
                r.action(3, "test", "rename", &[("a".into(), 7)], Some(bad))
                    .is_err()
            );
        }
        // A message that carries no text, and the ones a screen may not send at all, stay closed.
        assert!(r.action(3, "test", "plain", &[], None).is_err());
        assert!(r.action(3, "test", "turn", &[], None).is_err());
    }

    #[test]
    fn built_in_scripts_only_read_and_emit_net_messages() {
        for (name, source) in super::super::REGISTRY {
            for forbidden in [
                "GetActivePlayer",
                "GetActiveTeam",
                ":Set",
                ":Change",
                ":Do",
                ":Push",
                ":Pop",
                "Game.Set",
                "Game.Do",
            ] {
                assert!(!source.contains(forbidden), "{name} uses {forbidden}");
            }
        }
    }

    #[test]
    fn hud_script_matches_schema_scopes_focus_and_rechecks_action_availability() {
        let r = runtime();
        r.lua
            .exec(include_str!("../tests/fixtures/hud_api.lua"), "=hud-api")
            .unwrap();
        let global: Json = serde_json::from_str(&r.data(3, "hud", &[]).unwrap()).unwrap();
        assert!(global.get("city").is_none() && global.get("unit").is_none());
        let json: Json = serde_json::from_str(
            &r.data(3, "hud", &[("city".into(), 3), ("unit".into(), 4)])
                .unwrap(),
        )
        .unwrap();
        let sample = ui2_model::Sample::parse(
            ui2_model::Schema::v1(),
            &serde_json::json!({"model":{"hud":json}}).to_string(),
        )
        .unwrap();
        let mut store = ui2_model::ModelStore::new(ui2_model::Schema::v1());
        sample.apply(&mut store).unwrap();
        let hud = store
            .get::<Option<ui2_model::generated::Hud>>()
            .unwrap()
            .as_ref()
            .unwrap();
        let city = hud.city.as_ref().unwrap();
        assert_eq!(city.queue.len(), 2);
        assert_eq!(city.overflow, 7);
        assert_eq!(city.reasons.len(), 1);
        assert_eq!(
            hud.research
                .techs
                .iter()
                .map(|t| t.queue)
                .collect::<Vec<_>>(),
            [Some(0), Some(1)]
        );
        assert_eq!(hud.top.resources.len(), 2);
        assert_eq!(hud.top.policies.iter().next().unwrap().owned, 1);
        assert_eq!(hud.unit.as_ref().unwrap().promotions.len(), 1);
        assert_eq!(
            r.action(3, "hud", "alert", &[("a".into(), 4)], None)
                .unwrap()
                .messages,
            [NetMessage::new("sendPushMission", &[4, 10, 0, 0, 0, 0]).unwrap()]
        );
        assert_eq!(
            r.action(3, "hud", "upgrade", &[("a".into(), 4)], None)
                .unwrap()
                .messages,
            [NetMessage::new("sendDoCommand", &[4, 12, 0, 0, 0]).unwrap()]
        );
        assert!(
            r.action(3, "hud", "pillage", &[("a".into(), 4)], None)
                .is_err()
        );
        for args in [vec![("city".into(), 3)], vec![("unit".into(), 4)]] {
            assert!(r.data(1, "hud", &args).is_err());
        }
        assert!(
            r.action(1, "hud", "alert", &[("a".into(), 4)], None)
                .is_err()
        );
        assert!(r.action(3, "hud", "alert", &[], None).is_err());
        r.lua.exec("hud_busy = true", "=busy").unwrap();
        assert!(
            r.action(3, "hud", "alert", &[("a".into(), 4)], None)
                .is_err()
        );
    }
}
