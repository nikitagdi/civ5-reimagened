//! A state without Lua for layout tests: XML text in, controls out.
#![allow(dead_code)]

use ui::style::StyleBook;
use ui::{ControlId, State, StubHost, StubText, UiConfig};

pub fn state() -> State {
    state_with(StubHost::with_default_size(256, 256), StyleBook::default())
}

pub fn state_with(host: StubHost, styles: StyleBook) -> State {
    State::new(
        UiConfig::default(),
        styles,
        Box::new(host),
        Box::new(StubText::default()),
    )
}

/// Builds the first element of `xml` and initialises it. The tree has no context.
pub fn build(st: &mut State, xml: &str) -> ControlId {
    let el = ui::xml::parse("test.xml", xml.as_bytes()).expect("xml");
    let id = st.build_control(&el, None).expect("a control");
    st.initialize_plain(id);
    id
}

pub fn pos(st: &State, id: ControlId) -> (f32, f32) {
    let n = st.node(id);
    (n.pos.x, n.pos.y)
}

pub fn size(st: &State, id: ControlId) -> (f32, f32) {
    let n = st.node(id);
    (n.size.x, n.size.y)
}

/// The child with this `ID` below `root` (depth first).
pub fn find(st: &State, root: ControlId, name: &str) -> ControlId {
    fn walk(st: &State, id: ControlId, name: &str) -> Option<ControlId> {
        if st.node(id).id == name {
            return Some(id);
        }
        st.node(id).children.iter().find_map(|&c| walk(st, c, name))
    }
    walk(st, root, name).unwrap_or_else(|| panic!("no control {name}"))
}
