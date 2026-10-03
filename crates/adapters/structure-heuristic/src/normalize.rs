//! Text normalization applied to every extracted span.

/// Expands ligatures, turns special spaces into spaces, drops soft hyphens, zero-width and control
/// characters, and collapses runs of whitespace.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for c in text.chars() {
        let expanded: &str = match c {
            'ﬀ' => "ff",
            'ﬁ' => "fi",
            'ﬂ' => "fl",
            'ﬃ' => "ffi",
            'ﬄ' => "ffl",
            'ﬅ' | 'ﬆ' => "st",
            '\u{00AD}' | '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}' | '\u{FEFF}' => continue,
            c if c.is_whitespace() || c == '\u{00A0}' => {
                pending_space = true;
                continue;
            }
            c if c.is_control() => continue,
            _ => {
                if pending_space && !out.is_empty() {
                    out.push(' ');
                }
                pending_space = false;
                out.push(c);
                continue;
            }
        };
        if pending_space && !out.is_empty() {
            out.push(' ');
        }
        pending_space = false;
        out.push_str(expanded);
    }
    out
}
