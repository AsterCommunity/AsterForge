//! HTML and inline-script escaping helpers.
//!
//! These helpers are intentionally small and dependency-free. They are meant for server-side
//! placeholder injection into already generated HTML, not for sanitizing untrusted rich HTML.

/// Escapes text for HTML text or attribute placeholder insertion.
pub fn escape_html(value: impl AsRef<str>) -> String {
    value
        .as_ref()
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Escapes a JSON string fragment before embedding it in an inline `<script>` context.
///
/// Call this after JSON serialization when the resulting text is inserted into a script block.
/// It prevents HTML parser breakouts through `</script>`-relevant characters and preserves the
/// JavaScript line terminator semantics of U+2028 and U+2029.
pub fn escape_script_json(value: impl AsRef<str>) -> String {
    value
        .as_ref()
        .replace('&', "\\u0026")
        .replace('<', "\\u003C")
        .replace('>', "\\u003E")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

#[cfg(test)]
mod tests {
    use super::{escape_html, escape_script_json};

    #[test]
    fn escape_html_replaces_markup_and_quote_characters() {
        assert_eq!(
            escape_html(r#"<meta title="A&B's">"#),
            "&lt;meta title=&quot;A&amp;B&#39;s&quot;&gt;"
        );
    }

    #[test]
    fn escape_script_json_replaces_html_parser_breakout_characters() {
        assert_eq!(
            escape_script_json("</script>&\u{2028}\u{2029}"),
            "\\u003C/script\\u003E\\u0026\\u2028\\u2029"
        );
    }
}
