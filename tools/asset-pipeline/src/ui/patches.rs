//! Deliberate layout improvements over the original UI files, applied to the bytes of a UI file when the set is built
//! (the game files are never touched). Every patch names its file and the exact text it replaces; a patch whose text
//! is not found is reported, so a changed game file does not drop one silently.

/// One patch: the bare file name, the text it replaces and the new text.
pub struct Patch {
    pub file: &'static str,
    pub old: &'static str,
    pub new: &'static str,
}

/// SIMPLIFIED(SIMP-uic-20): the left civilization and city-state list of the Lekmod NotificationPanel (`CivPanelL`) is
/// moved 80 px to the right. In the original it starts at x = 0, over the city ribbon of the UnitPanel (a long city
/// list runs under the list of city states). Deliberate improvement requested by the players.
pub const PATCHES: &[Patch] = &[Patch {
    file: "NotificationPanel.xml",
    old: r#"<Container ID="CivPanelL" Anchor="L,B" Offset="0,0" >"#,
    new: r#"<Container ID="CivPanelL" Anchor="L,B" Offset="80,0" >"#,
}];

/// Applies the patches of `file` to `bytes`. Returns the changed bytes and the patches that found no text.
pub fn apply(file: &str, bytes: Vec<u8>) -> (Vec<u8>, Vec<&'static Patch>) {
    let mut missed = Vec::new();
    let mut text: Option<String> = None;
    for p in PATCHES.iter().filter(|p| p.file == file) {
        let t = text.get_or_insert_with(|| String::from_utf8_lossy(&bytes).into_owned());
        if t.contains(p.old) {
            *t = t.replacen(p.old, p.new, 1);
        } else {
            missed.push(p);
        }
    }
    match text {
        Some(t) => (t.into_bytes(), missed),
        None => (bytes, missed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_left_civ_panel_moves_right() {
        let src = "<a/>\r\n\t<Container ID=\"CivPanelL\" Anchor=\"L,B\" Offset=\"0,0\" >\r\n";
        let (out, missed) = apply("NotificationPanel.xml", src.as_bytes().to_vec());
        assert!(missed.is_empty());
        assert!(String::from_utf8(out).unwrap().contains("Offset=\"80,0\""));
    }

    #[test]
    fn a_missing_text_is_reported_and_other_files_are_untouched() {
        let (out, missed) = apply("NotificationPanel.xml", b"<a/>".to_vec());
        assert_eq!(missed.len(), 1);
        assert_eq!(out, b"<a/>");
        let (out, missed) = apply("Other.xml", b"x".to_vec());
        assert!(missed.is_empty());
        assert_eq!(out, b"x");
    }
}
