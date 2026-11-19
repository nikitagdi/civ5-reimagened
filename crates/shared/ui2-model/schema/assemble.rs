// Shared by the build script and the schema generator. Screen files extend named sections.
pub fn assemble(base: &str, screens: &[(String, String)]) -> Result<String, String> {
    let mut source = base.replace("\r\n", "\n");
    let mut enums = String::new();
    let mut extra = String::new();
    let mut windows = String::new();
    for (name, text) in screens {
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            return Err(format!("invalid screen filename {name}"));
        }
        if !text.contains(&format!("[model.{name}]")) || !text.contains("_optional = true") {
            return Err(format!(
                "screen {name} must declare its optional model root"
            ));
        }
        if !matches!(
            name.as_str(),
            "hud" | "rich_tooltips" | "civ_ribbon" | "plot_help" | "targets"
        ) {
            windows.push_str(&format!("{name} = \"WinArgs\"\n"));
        }
        let mut in_enum = false;
        for line in text.lines() {
            if line.trim() == "[enum]" {
                in_enum = true;
                continue;
            }
            if line.starts_with('[') {
                in_enum = false;
            }
            if in_enum {
                enums.push_str(line);
                enums.push('\n');
            } else {
                extra.push_str(line);
                extra.push('\n');
            }
        }
    }
    source = source.replacen("[type]", &format!("{enums}\n[type]"), 1);
    source = source.replacen("[type.Windows]", &format!("[type.Windows]\n{windows}"), 1);
    let start = source.find("WindowId = [").ok_or("missing WindowId enum")?;
    let end = start + source[start..].find(']').ok_or("invalid WindowId enum")?;
    let names = screens
        .iter()
        .filter(|(n, _)| {
            !matches!(
                n.as_str(),
                "hud" | "rich_tooltips" | "civ_ribbon" | "plot_help" | "targets"
            )
        })
        .map(|(n, _)| format!(", \"{n}\""))
        .collect::<String>();
    source.insert_str(end, &names);
    source.push('\n');
    source.push_str(&extra);
    Ok(source)
}
