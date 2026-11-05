//! The M1 fixture: the root context that holds the four EUI contexts, and the Lua stubs of the
//! exe API and the game core they call (our files, not the original's).

/// The fixture root of M1: the tip context, the EUI context and the two panels.
pub const M1_ROOT_XML: &str = r#"<Context ID="M1Root">
  <LuaContext ID="ToolTips" FileName="Assets/UI/FrontEnd/ToolTips"/>
  <LuaContext ID="EUI_context" FileName="Assets/UI/Core/EUI_context"/>
  <LuaContext ID="TopPanel" FileName="Assets/UI/InGame/TopPanel"/>
  <LuaContext ID="NotificationPanel" FileName="Assets/UI/InGame/NotificationPanel"/>
</Context>"#;

/// Name the fixture root is loaded under (`UiCore::load_root`).
pub const M1_ROOT_NAME: &str = "M1Root";

/// Lua of the stubs (`tests/fixtures/m1_stubs.lua`).
pub const M1_STUBS: &str = include_str!("../tests/fixtures/m1_stubs.lua");

/// The event the panels wait for before they fill in their values.
pub const M1_START_EVENT: &str = "SequenceGameInitComplete";
