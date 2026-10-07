//! Safety net for logs: fields that may carry document content or user data are replaced, and
//! user paths inside free text are masked. The first line of defence is not logging such data.

pub const REDACTED: &str = "[redigido]";
/// Longest free-text value kept in a log record.
pub const MAX_TEXT: usize = 300;

/// Field names whose value is never written.
const SENSITIVE_FIELDS: &[&str] = &[
    "text",
    "content",
    "quote",
    "query",
    "question",
    "answer",
    "prompt",
    "path",
    "file",
    "filename",
    "file_name",
    "title",
    "body",
    "chunk",
    "passage",
];

/// Prefixes of paths that belong to the user (home, external volumes, temp dirs).
const PATH_PREFIXES: &[&str] = &[
    "/Users/",
    "/Volumes/",
    "/private/",
    "/var/folders/",
    "/tmp/",
];

pub fn is_sensitive(field: &str) -> bool {
    let field = field.to_ascii_lowercase();
    SENSITIVE_FIELDS
        .iter()
        .any(|s| field == *s || field.ends_with(&format!("_{s}")))
}

/// Masks user paths and limits the length of a free-text value.
pub fn sanitize(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(MAX_TEXT + 8));
    let mut rest = text;
    while let Some((at, prefix_len)) = PATH_PREFIXES
        .iter()
        .filter_map(|p| rest.find(p).map(|i| (i, p.len())))
        .min_by_key(|(i, _)| *i)
    {
        out.push_str(&rest[..at]);
        out.push_str("<path>");
        let after = &rest[at + prefix_len..];
        // A path ends at whitespace or a quote/bracket.
        let end = after
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ')' | ']' | '}' | ','))
            .unwrap_or(after.len());
        rest = &after[end..];
    }
    out.push_str(rest);
    if out.chars().count() > MAX_TEXT {
        let cut: String = out.chars().take(MAX_TEXT).collect();
        format!("{cut}…")
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitive_field_names() {
        for f in [
            "text",
            "question",
            "file_path",
            "document_title",
            "Prompt",
            "path",
        ] {
            assert!(is_sensitive(f), "{f}");
        }
        for f in ["document_id", "pages", "total_ms", "kind", "model", "pid"] {
            assert!(!is_sensitive(f), "{f}");
        }
    }

    #[test]
    fn masks_user_paths_and_truncates() {
        assert_eq!(
            sanitize("falha ao abrir /Users/ana/Laudos/João Silva.pdf: permissão"),
            "falha ao abrir <path> Silva.pdf: permissão"
        );
        assert_eq!(
            sanitize("db at \"/Volumes/X/app.sqlite3\" ok"),
            "db at \"<path>\" ok"
        );
        assert_eq!(
            sanitize("tmp /var/folders/ab/T/x and /tmp/y"),
            "tmp <path> and <path>"
        );
        assert_eq!(sanitize("/usr/bin/fm exited"), "/usr/bin/fm exited");
        let long = "a".repeat(1000);
        assert_eq!(sanitize(&long).chars().count(), MAX_TEXT + 1);
    }
}
