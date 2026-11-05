//! A text dump of the control tree (the `ControlBase::GetText` / `GetDataMembers` view).

use std::fmt::Write;

use crate::flags::*;
use crate::state::State;
use crate::tree::*;

/// One line per control: class, ID, position, size, flags; indented by depth.
pub fn dump(st: &State) -> String {
    let mut out = String::new();
    for &r in st.roots.iter().rev() {
        dump_node(st, r, 0, &mut out);
    }
    out
}

pub fn dump_node(st: &State, id: ControlId, depth: usize, out: &mut String) {
    let Some(n) = st.tree.get(id) else { return };
    let _ = write!(
        out,
        "{:indent$}{} \"{}\" pos=({},{}) size=({},{})",
        "",
        n.kind.name(),
        n.id,
        n.pos.x,
        n.pos.y,
        n.size.x,
        n.size.y,
        indent = depth * 2
    );
    if n.has(HIDDEN) {
        out.push_str(" hidden");
    }
    if n.has(MOUSE_OVER_HIDDEN) {
        out.push_str(" mouse-over-hidden");
    }
    if let Kind::Label(l) = &n.kind {
        let _ = write!(out, " text={:?}", l.text);
    }
    // Textures of buttons and images: name, source window, "noquad" when the button draws no quad.
    let quad = match &n.kind {
        Kind::Button(b) => Some(&b.quad),
        Kind::Image(i) => Some(&i.quad),
        _ => None,
    };
    if let Some(q) = quad
        && let Some(t) = &q.texture
    {
        let _ = write!(
            out,
            " tex={} src=({},{},{},{})",
            t.name, q.src_x, q.src_y, q.src_w, q.src_h
        );
        if matches!(n.kind, Kind::Button(_)) && n.flags & button_flag::NO_QUAD != 0 {
            out.push_str(" noquad");
        }
    }
    out.push('\n');
    for &c in &n.children {
        dump_node(st, c, depth + 1, out);
    }
}

/// The controls on the screen (shown, with a size), one line each: `path<TAB>class<TAB>x y w h<TAB>flags<TAB>text`.
/// The path is the IDs of the control and its parents joined by `/` (the contexts' IDs start it), `flags` has `D`
/// for a disabled control, then `v<void1>` for a button, then `a<alpha>` when the control's alpha is not 1. For the playtests: find a control by its path and click the middle of its rectangle.
pub fn dump_screen(st: &State, path_filter: &str) -> String {
    fn walk(
        st: &State,
        id: ControlId,
        path: &str,
        filter: &str,
        top: Option<ControlId>,
        out: &mut String,
    ) {
        let Some(n) = st.tree.get(id) else { return };
        // The popup or modal that is up is flagged hidden-by-mouse-over in the tree (it is drawn on its own).
        if (Some(id) == top && n.has(HIDDEN)) || (Some(id) != top && n.has(NOT_ACTIVE)) {
            return;
        }
        let path = format!("{path}/{}", n.id);
        if let Some((r, true)) = st.screen_rect_in(id, top)
            && r[2] > 0.0
            && r[3] > 0.0
            && path.contains(filter)
        {
            let text = match &n.kind {
                Kind::Label(l) => l.text.clone(),
                // The tip string of any other control (`tip:` + text): the icon buttons of the corner have no other label.
                _ => n
                    .tt_string
                    .as_ref()
                    .map_or_else(String::new, |t| format!("tip:{t}")),
            };
            let void1 = n
                .kind
                .button_base()
                .map_or_else(String::new, |b| format!("v{}", b.void1));
            // The control's own alpha when it is not 1 (the players panel dims who has ended the turn).
            let alpha = if (n.alpha - 1.0).abs() > f32::EPSILON {
                format!("a{:.2}", n.alpha)
            } else {
                String::new()
            };
            let _ = writeln!(
                out,
                "{path}\t{}\t{} {} {} {}\t{}{void1}{alpha}\t{}",
                n.kind.name(),
                r[0],
                r[1],
                r[2],
                r[3],
                if n.has(DISABLED) { "D" } else { "" },
                text.replace(['\n', '\t'], " ")
            );
        }
        for &c in &n.children {
            walk(st, c, &path, filter, top, out);
        }
    }
    let mut out = String::new();
    for &r in st.roots.iter().rev() {
        walk(st, r, "", path_filter, None, &mut out);
    }
    // The popup that is up and the modals: their path is the ids of their parents (the contexts of the game) and their own.
    let tops: Vec<ControlId> = st
        .popup_current
        .into_iter()
        .chain(st.modals.iter().copied())
        .collect();
    for c in tops {
        if !st.alive(c) {
            continue;
        }
        let mut ids = Vec::new();
        let mut cur = st.tree[c].parent;
        while let Some(p) = cur {
            ids.push(st.tree[p].id.clone());
            cur = st.tree.get(p).and_then(|n| n.parent);
        }
        ids.reverse();
        let prefix: String = ids.iter().map(|i| format!("/{i}")).collect();
        walk(st, c, &prefix, path_filter, Some(c), &mut out);
    }
    // The same tops again under `#top` (the m4-menus-endgame scenario finds the game menu's buttons by that prefix).
    for &c in st.popup_current.iter().chain(st.modals.iter()) {
        if st.alive(c) {
            walk(st, c, "#top", path_filter, Some(c), &mut out);
        }
    }
    out
}

/// Controls the user can see (not hidden on the path from a root) that have no size.
pub fn zero_sized_visible(st: &State) -> Vec<ControlId> {
    fn walk(st: &State, id: ControlId, out: &mut Vec<ControlId>) {
        let n = &st.tree[id];
        if n.has(NOT_ACTIVE) {
            return;
        }
        let drawable = matches!(
            n.kind,
            Kind::Image(_) | Kind::Button(_) | Kind::GridButton(_) | Kind::Grid(_) | Kind::Label(_)
        );
        // A label with only blanks has no extent.
        let blank = matches!(&n.kind, Kind::Label(l) if l.text.trim().is_empty());
        if drawable && !blank && (n.size.x == 0.0 || n.size.y == 0.0) {
            out.push(id);
        }
        for &c in &n.children {
            walk(st, c, out);
        }
    }
    let mut v = Vec::new();
    for &r in &st.roots {
        walk(st, r, &mut v);
    }
    v
}
