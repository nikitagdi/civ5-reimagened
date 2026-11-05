//! The page: one HTML file with the CSS, the script and the data inlined (images stay files).

use anyhow::Result;
use serde_json::Value;

const CSS: &str = include_str!("page.css");
const JS: &str = include_str!("page.js");

/// The data goes into a `<script type="application/json">`: `<` is escaped so that no text in it can
/// close the element.
fn json_for_script(data: &Value) -> Result<String> {
    Ok(serde_json::to_string(data)?.replace('<', "\\u003c"))
}

pub fn page(data: &Value) -> Result<String> {
    Ok(format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>Golden report</title>\n<style>\n{CSS}\n</style>\n</head>\n<body>\n\
         <div id=\"app\"></div>\n\
         <script id=\"data\" type=\"application/json\">{}</script>\n\
         <script>\n{JS}\n</script>\n</body>\n</html>\n",
        json_for_script(data)?
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_cannot_close_the_script() {
        let p = page(&serde_json::json!({"x": "</script><b>"})).unwrap();
        assert_eq!(p.matches("</script>").count(), 2);
    }
}
