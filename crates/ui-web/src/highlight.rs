//! Approximate syntax highlighting for code blocks in answers (ADR 0020).
//!
//! A single pass over the text, per language family (C-like, scripts, SQL, JSON/YAML, markup): it
//! finds comments, strings, numbers, keywords, function calls, keys and tags — no parsing, so
//! unusual constructs (heredocs, interpolation, regex literals) may be coloured wrongly. The
//! pieces always concatenate back to the exact input, and an unterminated string or comment simply
//! runs to the end (partial text stays well formed). Pure: no I/O, and the output only names a
//! fixed set of token classes.

/// What a piece of code is; [`Kind::class`] is the CSS class (`tok-*`), `None` for plain text.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Plain,
    Keyword,
    String,
    Number,
    Comment,
    Function,
    Key,
    Tag,
    Attr,
}

impl Kind {
    pub fn class(self) -> Option<&'static str> {
        Some(match self {
            Kind::Plain => return None,
            Kind::Keyword => "tok-kw",
            Kind::String => "tok-str",
            Kind::Number => "tok-num",
            Kind::Comment => "tok-com",
            Kind::Function => "tok-fn",
            Kind::Key => "tok-key",
            Kind::Tag => "tok-tag",
            Kind::Attr => "tok-attr",
        })
    }
}

/// Languages the answers name most often.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Text,
    Kotlin,
    Java,
    Rust,
    Python,
    TypeScript,
    JavaScript,
    Json,
    Sql,
    Bash,
    Shell,
    Yaml,
    Xml,
    Html,
    Css,
    CSharp,
    Swift,
    Markdown,
}

impl Lang {
    /// By fence name or common alias, ignoring case; `""` and the plain-text names are `Text`.
    pub fn from_name(name: &str) -> Option<Lang> {
        Some(match name.to_ascii_lowercase().as_str() {
            "" | "text" | "txt" | "plain" | "plaintext" => Lang::Text,
            "kotlin" | "kt" => Lang::Kotlin,
            "java" => Lang::Java,
            "rust" | "rs" => Lang::Rust,
            "python" | "py" => Lang::Python,
            "typescript" | "ts" | "tsx" => Lang::TypeScript,
            "javascript" | "js" | "jsx" => Lang::JavaScript,
            "json" => Lang::Json,
            "sql" => Lang::Sql,
            "bash" => Lang::Bash,
            "sh" | "shell" | "zsh" | "console" => Lang::Shell,
            "yaml" | "yml" => Lang::Yaml,
            "xml" => Lang::Xml,
            "html" => Lang::Html,
            "css" => Lang::Css,
            "csharp" | "cs" | "c#" => Lang::CSharp,
            "swift" => Lang::Swift,
            "markdown" | "md" => Lang::Markdown,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Lang::Text => "Texto",
            Lang::Kotlin => "Kotlin",
            Lang::Java => "Java",
            Lang::Rust => "Rust",
            Lang::Python => "Python",
            Lang::TypeScript => "TypeScript",
            Lang::JavaScript => "JavaScript",
            Lang::Json => "JSON",
            Lang::Sql => "SQL",
            Lang::Bash => "Bash",
            Lang::Shell => "Shell",
            Lang::Yaml => "YAML",
            Lang::Xml => "XML",
            Lang::Html => "HTML",
            Lang::Css => "CSS",
            Lang::CSharp => "C#",
            Lang::Swift => "Swift",
            Lang::Markdown => "Markdown",
        }
    }
}

/// Code beyond this size is left plain (cost bound).
const MAX_BYTES: usize = 100_000;

/// The code split into classified pieces whose concatenation is exactly `code`.
pub fn highlight(lang: Lang, code: &str) -> Vec<(Kind, &str)> {
    if code.is_empty() {
        return Vec::new();
    }
    if code.len() > MAX_BYTES || matches!(lang, Lang::Text | Lang::Markdown) {
        return vec![(Kind::Plain, code)];
    }
    match lang {
        Lang::Xml | Lang::Html => markup(code),
        _ => generic(&syntax(lang), code),
    }
}

struct Syntax {
    /// Line comment markers (`#` only counts at the start of a word).
    line: &'static [&'static str],
    block: Option<(&'static str, &'static str)>,
    quotes: &'static [char],
    /// `"""` / `'''` strings may span lines.
    triple: bool,
    /// `'x'` is a character literal, otherwise `'` is a lifetime / plain (Rust).
    char_literal: bool,
    keywords: &'static [&'static str],
    ignore_case: bool,
    /// `$name` / `${…}` are highlighted (shells).
    dollar: bool,
    /// Words and strings followed by `:` are keys (JSON, YAML).
    colon_keys: bool,
    /// `name(` is a function call.
    functions: bool,
    /// `-` belongs to words (CSS, YAML).
    dash_words: bool,
}

const NONE: Syntax = Syntax {
    line: &[],
    block: None,
    quotes: &[],
    triple: false,
    char_literal: false,
    keywords: &[],
    ignore_case: false,
    dollar: false,
    colon_keys: false,
    functions: true,
    dash_words: false,
};

const C_BLOCK: Option<(&str, &str)> = Some(("/*", "*/"));

fn syntax(lang: Lang) -> Syntax {
    match lang {
        Lang::Kotlin => Syntax {
            line: &["//"],
            block: C_BLOCK,
            quotes: &['"', '\''],
            triple: true,
            keywords: &[
                "fun",
                "val",
                "var",
                "class",
                "object",
                "interface",
                "if",
                "else",
                "when",
                "for",
                "while",
                "do",
                "return",
                "break",
                "continue",
                "in",
                "is",
                "as",
                "null",
                "true",
                "false",
                "this",
                "super",
                "import",
                "package",
                "private",
                "public",
                "protected",
                "internal",
                "override",
                "open",
                "abstract",
                "sealed",
                "data",
                "enum",
                "companion",
                "suspend",
                "inline",
                "lateinit",
                "const",
                "try",
                "catch",
                "finally",
                "throw",
                "typealias",
                "by",
                "init",
                "where",
            ],
            ..NONE
        },
        Lang::Java => Syntax {
            line: &["//"],
            block: C_BLOCK,
            quotes: &['"', '\''],
            triple: true,
            keywords: &[
                "class",
                "interface",
                "enum",
                "extends",
                "implements",
                "public",
                "private",
                "protected",
                "static",
                "final",
                "abstract",
                "void",
                "int",
                "long",
                "double",
                "float",
                "boolean",
                "char",
                "byte",
                "short",
                "new",
                "return",
                "if",
                "else",
                "for",
                "while",
                "do",
                "switch",
                "case",
                "default",
                "break",
                "continue",
                "try",
                "catch",
                "finally",
                "throw",
                "throws",
                "import",
                "package",
                "this",
                "super",
                "null",
                "true",
                "false",
                "instanceof",
                "var",
                "record",
                "synchronized",
                "volatile",
            ],
            ..NONE
        },
        Lang::Rust => Syntax {
            line: &["//"],
            block: C_BLOCK,
            quotes: &['"', '\''],
            char_literal: true,
            keywords: &[
                "fn", "let", "mut", "const", "static", "struct", "enum", "impl", "trait", "type",
                "pub", "use", "mod", "crate", "self", "Self", "super", "if", "else", "match",
                "for", "while", "loop", "in", "return", "break", "continue", "as", "ref", "move",
                "async", "await", "dyn", "where", "unsafe", "extern", "true", "false", "box",
            ],
            ..NONE
        },
        Lang::Python => Syntax {
            line: &["#"],
            quotes: &['"', '\''],
            triple: true,
            keywords: &[
                "def", "class", "if", "elif", "else", "for", "while", "return", "import", "from",
                "as", "try", "except", "finally", "raise", "with", "pass", "break", "continue",
                "lambda", "yield", "in", "is", "not", "and", "or", "None", "True", "False",
                "global", "nonlocal", "async", "await", "del", "assert", "self", "match", "case",
            ],
            ..NONE
        },
        Lang::TypeScript | Lang::JavaScript => Syntax {
            line: &["//"],
            block: C_BLOCK,
            quotes: &['"', '\'', '`'],
            keywords: &[
                "function",
                "const",
                "let",
                "var",
                "class",
                "extends",
                "new",
                "return",
                "if",
                "else",
                "for",
                "while",
                "do",
                "switch",
                "case",
                "default",
                "break",
                "continue",
                "try",
                "catch",
                "finally",
                "throw",
                "import",
                "export",
                "from",
                "as",
                "async",
                "await",
                "yield",
                "this",
                "super",
                "null",
                "undefined",
                "true",
                "false",
                "typeof",
                "instanceof",
                "in",
                "of",
                "void",
                "delete",
                "interface",
                "type",
                "enum",
                "implements",
                "public",
                "private",
                "protected",
                "readonly",
                "static",
                "abstract",
                "namespace",
                "declare",
                "keyof",
                "get",
                "set",
            ],
            ..NONE
        },
        Lang::CSharp => Syntax {
            line: &["//"],
            block: C_BLOCK,
            quotes: &['"', '\''],
            triple: true,
            keywords: &[
                "class",
                "struct",
                "interface",
                "enum",
                "namespace",
                "using",
                "public",
                "private",
                "protected",
                "internal",
                "static",
                "readonly",
                "const",
                "void",
                "int",
                "long",
                "double",
                "float",
                "bool",
                "string",
                "char",
                "var",
                "new",
                "return",
                "if",
                "else",
                "for",
                "foreach",
                "while",
                "do",
                "switch",
                "case",
                "default",
                "break",
                "continue",
                "try",
                "catch",
                "finally",
                "throw",
                "this",
                "base",
                "null",
                "true",
                "false",
                "async",
                "await",
                "override",
                "virtual",
                "abstract",
                "sealed",
                "partial",
                "record",
                "in",
                "out",
                "ref",
                "is",
                "as",
                "get",
                "set",
            ],
            ..NONE
        },
        Lang::Swift => Syntax {
            line: &["//"],
            block: C_BLOCK,
            quotes: &['"'],
            triple: true,
            keywords: &[
                "func",
                "let",
                "var",
                "class",
                "struct",
                "enum",
                "protocol",
                "extension",
                "import",
                "if",
                "else",
                "guard",
                "switch",
                "case",
                "default",
                "for",
                "while",
                "repeat",
                "return",
                "break",
                "continue",
                "in",
                "is",
                "as",
                "nil",
                "true",
                "false",
                "self",
                "super",
                "init",
                "deinit",
                "public",
                "private",
                "internal",
                "fileprivate",
                "open",
                "static",
                "final",
                "override",
                "try",
                "catch",
                "throw",
                "throws",
                "async",
                "await",
                "where",
                "some",
                "any",
                "lazy",
                "weak",
                "inout",
            ],
            ..NONE
        },
        Lang::Css => Syntax {
            block: C_BLOCK,
            quotes: &['"', '\''],
            keywords: &["important", "none", "auto", "inherit", "initial", "unset"],
            dash_words: true,
            ..NONE
        },
        Lang::Bash | Lang::Shell => Syntax {
            line: &["#"],
            quotes: &['"', '\''],
            keywords: &[
                "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case",
                "esac", "in", "function", "return", "exit", "export", "local", "readonly",
                "source", "echo", "cd", "set", "unset", "alias", "break", "continue",
            ],
            dollar: true,
            functions: false,
            ..NONE
        },
        Lang::Sql => Syntax {
            line: &["--"],
            block: C_BLOCK,
            quotes: &['\'', '"'],
            keywords: &[
                "select",
                "from",
                "where",
                "and",
                "or",
                "not",
                "in",
                "is",
                "null",
                "as",
                "join",
                "inner",
                "left",
                "right",
                "full",
                "outer",
                "cross",
                "on",
                "group",
                "by",
                "order",
                "having",
                "limit",
                "offset",
                "insert",
                "into",
                "values",
                "update",
                "set",
                "delete",
                "create",
                "table",
                "alter",
                "drop",
                "index",
                "view",
                "primary",
                "key",
                "foreign",
                "references",
                "default",
                "distinct",
                "union",
                "all",
                "exists",
                "between",
                "like",
                "case",
                "when",
                "then",
                "else",
                "end",
                "asc",
                "desc",
                "with",
                "count",
                "sum",
                "avg",
                "min",
                "max",
                "begin",
                "commit",
                "rollback",
                "constraint",
                "unique",
                "check",
            ],
            ignore_case: true,
            ..NONE
        },
        Lang::Json => Syntax {
            quotes: &['"'],
            keywords: &["true", "false", "null"],
            colon_keys: true,
            functions: false,
            ..NONE
        },
        Lang::Yaml => Syntax {
            line: &["#"],
            quotes: &['"', '\''],
            keywords: &["true", "false", "null", "yes", "no", "on", "off"],
            colon_keys: true,
            functions: false,
            dash_words: true,
            ..NONE
        },
        // Markup, text and Markdown never reach `generic`.
        Lang::Xml | Lang::Html | Lang::Text | Lang::Markdown => NONE,
    }
}

/// Collects classified pieces; whatever lies between tokens is plain.
struct Out<'a> {
    code: &'a str,
    pieces: Vec<(Kind, &'a str)>,
    plain_from: usize,
}

impl<'a> Out<'a> {
    fn new(code: &'a str) -> Self {
        Out {
            code,
            pieces: Vec::new(),
            plain_from: 0,
        }
    }

    /// `start..end` becomes a token; `start` never lies before the previous token's end.
    fn token(&mut self, start: usize, end: usize, kind: Kind) {
        debug_assert!(start >= self.plain_from && end > start);
        if start > self.plain_from {
            self.pieces
                .push((Kind::Plain, &self.code[self.plain_from..start]));
        }
        self.pieces.push((kind, &self.code[start..end]));
        self.plain_from = end;
    }

    fn finish(mut self) -> Vec<(Kind, &'a str)> {
        if self.plain_from < self.code.len() {
            self.pieces
                .push((Kind::Plain, &self.code[self.plain_from..]));
        }
        self.pieces
    }
}

fn is_word_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_word_char(c: char, dash: bool) -> bool {
    c.is_alphanumeric() || c == '_' || (dash && c == '-')
}

/// Byte offset of the end of the word starting at `from`.
fn word_end(code: &str, from: usize, dash: bool) -> usize {
    code[from..]
        .char_indices()
        .find(|(_, c)| !is_word_char(*c, dash))
        .map_or(code.len(), |(i, _)| from + i)
}

/// End of a string that opens at `start` with quote `q` (`start` is on the opening quote).
/// Unterminated: to the end of the line (to the end of the text for multi-line kinds).
fn string_end(code: &str, start: usize, q: char, multiline: bool) -> usize {
    let mut chars = code[start + q.len_utf8()..].char_indices();
    let base = start + q.len_utf8();
    while let Some((i, c)) = chars.next() {
        if c == '\\' {
            chars.next();
        } else if c == q {
            return base + i + c.len_utf8();
        } else if c == '\n' && !multiline {
            return base + i;
        }
    }
    code.len()
}

fn generic<'a>(s: &Syntax, code: &'a str) -> Vec<(Kind, &'a str)> {
    let mut out = Out::new(code);
    let mut i = 0;
    while i < code.len() {
        let rest = &code[i..];
        let c = rest.chars().next().unwrap_or(' ');

        // Comments.
        if let Some((open, close)) = s.block {
            if let Some(body) = rest.strip_prefix(open) {
                let end = body
                    .find(close)
                    .map_or(code.len(), |p| i + open.len() + p + close.len());
                out.token(i, end, Kind::Comment);
                i = end;
                continue;
            }
        }
        if let Some(marker) = s.line.iter().find(|m| rest.starts_with(**m)) {
            let at_word_start = *marker != "#"
                || code[..i]
                    .chars()
                    .next_back()
                    .is_none_or(|p| p.is_whitespace());
            if at_word_start {
                let end = rest.find('\n').map_or(code.len(), |p| i + p);
                out.token(i, end, Kind::Comment);
                i = end;
                continue;
            }
        }

        // Strings.
        if s.quotes.contains(&c) {
            if s.char_literal && c == '\'' {
                // 'x' or an escape like '\n' / '\u{1F600}'; otherwise a lifetime.
                let mut chars = rest.chars();
                chars.next();
                let literal_end = match (chars.next(), chars.next()) {
                    (Some('\\'), _) => rest[2..]
                        .char_indices()
                        .take(12)
                        .find(|(_, ch)| *ch == '\'')
                        .map(|(p, _)| i + 2 + p + 1),
                    (Some(ch), Some('\'')) if ch != '\'' => Some(i + 1 + ch.len_utf8() + 1),
                    _ => None,
                };
                match literal_end {
                    Some(end) => {
                        out.token(i, end, Kind::String);
                        i = end;
                    }
                    None => i += 1,
                }
                continue;
            }
            let triple = [c, c, c].iter().collect::<String>();
            let end = if s.triple && rest.starts_with(&triple) {
                rest[3..]
                    .find(&triple)
                    .map_or(code.len(), |p| i + 3 + p + 3)
            } else {
                string_end(code, i, c, c == '`')
            };
            let kind =
                if s.colon_keys && code[end..].trim_start_matches([' ', '\t']).starts_with(':') {
                    Kind::Key
                } else {
                    Kind::String
                };
            out.token(i, end, kind);
            i = end;
            continue;
        }

        // `$name`, `${…}`, `$1`, `$?` in shells.
        if s.dollar && c == '$' {
            let after = &rest[1..];
            let end = if after.starts_with('{') {
                after.find('}').map_or(code.len(), |p| i + 1 + p + 1)
            } else {
                match after.chars().next() {
                    Some(n) if is_word_start(n) || n.is_ascii_digit() => {
                        word_end(code, i + 1, false)
                    }
                    Some(n) if "@*#?!$-".contains(n) => i + 2,
                    _ => i + 1,
                }
            };
            if end > i + 1 {
                out.token(i, end, Kind::Keyword);
                i = end;
                continue;
            }
        }

        // Numbers (identifiers are consumed whole, so a digit here starts a number).
        if c.is_ascii_digit() {
            let mut end = i;
            let mut chars = rest.char_indices().peekable();
            while let Some((p, ch)) = chars.next() {
                let dot_in_number =
                    ch == '.' && chars.peek().is_some_and(|(_, n)| n.is_ascii_digit());
                if ch.is_alphanumeric() || ch == '_' || dot_in_number {
                    end = i + p + ch.len_utf8();
                } else {
                    break;
                }
            }
            out.token(i, end, Kind::Number);
            i = end;
            continue;
        }

        // Words and `@annotations`.
        let annotation = c == '@' && rest[1..].chars().next().is_some_and(is_word_start);
        if is_word_start(c) || annotation {
            let start = if annotation { i + 1 } else { i };
            let end = word_end(code, start, s.dash_words);
            let word = &code[i..end];
            let is_keyword = annotation
                || s.keywords.iter().any(|k| {
                    if s.ignore_case {
                        k.eq_ignore_ascii_case(word)
                    } else {
                        *k == word
                    }
                });
            let after = &code[end..];
            let kind = if is_keyword {
                Some(Kind::Keyword)
            } else if s.colon_keys && after.starts_with(':') {
                Some(Kind::Key)
            } else if s.functions && after.starts_with('(') {
                Some(Kind::Function)
            } else {
                None
            };
            if let Some(kind) = kind {
                out.token(i, end, kind);
            }
            i = end;
            continue;
        }

        i += c.len_utf8();
    }
    out.finish()
}

/// XML / HTML: comments, tag names, attribute names and values.
fn markup(code: &str) -> Vec<(Kind, &str)> {
    let name_char = |c: char| c.is_alphanumeric() || matches!(c, '-' | '_' | ':' | '.');
    let mut out = Out::new(code);
    let mut i = 0;
    while i < code.len() {
        let rest = &code[i..];
        if rest.starts_with("<!--") {
            let end = rest.find("-->").map_or(code.len(), |p| i + p + 3);
            out.token(i, end, Kind::Comment);
            i = end;
            continue;
        }
        let opens_tag = rest.starts_with('<')
            && rest[1..]
                .trim_start_matches('/')
                .chars()
                .next()
                .is_some_and(|c| c.is_alphabetic() || c == '!' || c == '?');
        if !opens_tag {
            i += rest.chars().next().map_or(1, char::len_utf8);
            continue;
        }
        // `<name`, `</name`, `<!DOCTYPE`, `<?xml`.
        let head = 1 + usize::from(rest[1..].starts_with(['/', '!', '?']));
        let mut j = i + head;
        while let Some(c) = code[j..].chars().next().filter(|c| name_char(*c)) {
            j += c.len_utf8();
        }
        out.token(i, j, Kind::Tag);
        while j < code.len() {
            let tail = &code[j..];
            let c = tail.chars().next().unwrap_or(' ');
            if c == '>' || tail.starts_with("/>") || tail.starts_with("?>") {
                let len = if c == '>' { 1 } else { 2 };
                out.token(j, j + len, Kind::Tag);
                j += len;
                break;
            }
            if c == '"' || c == '\'' {
                let end = string_end(code, j, c, true);
                out.token(j, end, Kind::String);
                j = end;
            } else if c.is_alphabetic() || matches!(c, '_' | ':' | '@') {
                let mut end = j;
                while let Some(n) = code[end..].chars().next().filter(|n| name_char(*n)) {
                    end += n.len_utf8();
                }
                let end = end.max(j + c.len_utf8());
                out.token(j, end, Kind::Attr);
                j = end;
            } else {
                j += c.len_utf8();
            }
        }
        i = j;
    }
    out.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Lang; 18] = [
        Lang::Text,
        Lang::Kotlin,
        Lang::Java,
        Lang::Rust,
        Lang::Python,
        Lang::TypeScript,
        Lang::JavaScript,
        Lang::Json,
        Lang::Sql,
        Lang::Bash,
        Lang::Shell,
        Lang::Yaml,
        Lang::Xml,
        Lang::Html,
        Lang::Css,
        Lang::CSharp,
        Lang::Swift,
        Lang::Markdown,
    ];

    fn kinds(lang: Lang, code: &str) -> Vec<(Kind, &str)> {
        highlight(lang, code)
            .into_iter()
            .filter(|(k, _)| *k != Kind::Plain)
            .collect()
    }

    fn has(lang: Lang, code: &str, kind: Kind, text: &str) -> bool {
        kinds(lang, code).contains(&(kind, text))
    }

    #[test]
    fn pieces_always_rebuild_the_input() {
        let samples = [
            "",
            " ",
            "\n\n",
            "fun main() {\n    println(\"Olá, ação 🚀\")\n}\n",
            "fn f<'a>(x: &'a str) -> char { '\\n' }",
            "SELECT * FROM t -- fim\nWHERE a = 'x",
            "{\"a\": [1, 2.5e3, true, null], \"b\": \"\\\"\"}",
            "<div class=\"a\" id='b'>texto &amp; <!-- c --></div>",
            "x = \"\"\"aberta\n\\",
            "/* sem fim",
            "\"sem fim",
            "$",
            "${",
            "\r\n\t# c\r\n",
            "<",
            "<a",
            "<a href=\"x",
            "'",
            "日本語 = 'テスト'",
            "a\u{0}b",
        ];
        for lang in ALL {
            for sample in samples {
                let joined: String = highlight(lang, sample).iter().map(|(_, t)| *t).collect();
                assert_eq!(joined, sample, "{lang:?}: {sample:?}");
                assert!(highlight(lang, sample).iter().all(|(_, t)| !t.is_empty()));
            }
        }
    }

    #[test]
    fn kotlin() {
        let code = "fun main() { val n = 42 // soma\n println(\"oi\") }";
        assert!(has(Lang::Kotlin, code, Kind::Keyword, "fun"));
        assert!(has(Lang::Kotlin, code, Kind::Function, "main"));
        assert!(has(Lang::Kotlin, code, Kind::Number, "42"));
        assert!(has(Lang::Kotlin, code, Kind::Comment, "// soma"));
        assert!(has(Lang::Kotlin, code, Kind::String, "\"oi\""));
        assert!(has(Lang::Kotlin, code, Kind::Function, "println"));
    }

    #[test]
    fn rust_lifetimes_chars_and_comments() {
        let code = "fn f<'a>(x: &'a str) -> char { 'x' } // fim\n/* a */ let n = 0xFF_u8;";
        assert!(has(Lang::Rust, code, Kind::Keyword, "fn"));
        assert!(has(Lang::Rust, code, Kind::String, "'x'"));
        assert!(
            !kinds(Lang::Rust, code)
                .iter()
                .any(|(k, t)| *k == Kind::String && t.contains("'a"))
        );
        assert!(has(Lang::Rust, code, Kind::Comment, "// fim"));
        assert!(has(Lang::Rust, code, Kind::Comment, "/* a */"));
        assert!(has(Lang::Rust, code, Kind::Number, "0xFF_u8"));
        assert!(has(Lang::Rust, "let c = '\\n';", Kind::String, "'\\n'"));
        // `--` is a SQL comment only.
        assert!(
            !kinds(Lang::Rust, "a -- b")
                .iter()
                .any(|(k, _)| *k == Kind::Comment)
        );
    }

    #[test]
    fn python_triple_quotes_and_decorators() {
        let code = "@cache\ndef f(x):\n    \"\"\"doc\n    mais\"\"\"\n    return None  # fim";
        assert!(has(Lang::Python, code, Kind::Keyword, "@cache"));
        assert!(has(Lang::Python, code, Kind::Keyword, "def"));
        assert!(has(
            Lang::Python,
            code,
            Kind::String,
            "\"\"\"doc\n    mais\"\"\""
        ));
        assert!(has(Lang::Python, code, Kind::Keyword, "None"));
        assert!(has(Lang::Python, code, Kind::Comment, "# fim"));
    }

    #[test]
    fn bash_variables_and_hash() {
        let code = "echo \"a # b\" $HOME ${PATH} $1 # nota\nls -la | grep a#b";
        assert!(has(Lang::Bash, code, Kind::Keyword, "$HOME"));
        assert!(has(Lang::Bash, code, Kind::Keyword, "${PATH}"));
        assert!(has(Lang::Bash, code, Kind::Keyword, "$1"));
        assert!(has(Lang::Bash, code, Kind::Comment, "# nota"));
        // `#` inside a string or glued to a word is not a comment.
        assert_eq!(
            kinds(Lang::Bash, code)
                .iter()
                .filter(|(k, _)| *k == Kind::Comment)
                .count(),
            1
        );
        assert!(has(Lang::Shell, "export A=1", Kind::Keyword, "export"));
    }

    #[test]
    fn sql_is_case_insensitive() {
        let code = "select Id, count(*) FROM t -- x\nWHERE n >= 10 AND s = 'a''b'";
        assert!(has(Lang::Sql, code, Kind::Keyword, "select"));
        assert!(has(Lang::Sql, code, Kind::Keyword, "FROM"));
        assert!(has(Lang::Sql, code, Kind::Keyword, "count"));
        assert!(has(Lang::Sql, code, Kind::Comment, "-- x"));
        assert!(has(Lang::Sql, code, Kind::Number, "10"));
        assert!(has(Lang::Sql, code, Kind::String, "'a'"));
    }

    #[test]
    fn json_keys_versus_values() {
        let code = "{\n  \"nome\": \"NLMX\",\n  \"n\": -1.5,\n  \"ok\": true,\n  \"x\": null\n}";
        assert!(has(Lang::Json, code, Kind::Key, "\"nome\""));
        assert!(has(Lang::Json, code, Kind::String, "\"NLMX\""));
        assert!(has(Lang::Json, code, Kind::Number, "1.5"));
        assert!(has(Lang::Json, code, Kind::Keyword, "true"));
        assert!(has(Lang::Json, code, Kind::Keyword, "null"));
    }

    #[test]
    fn yaml_keys_and_comments() {
        let code = "# conf\nnome-app: nlmx\nativo: true\nlista:\n  - \"a\"\n";
        assert!(has(Lang::Yaml, code, Kind::Comment, "# conf"));
        assert!(has(Lang::Yaml, code, Kind::Key, "nome-app"));
        assert!(has(Lang::Yaml, code, Kind::Key, "ativo"));
        assert!(has(Lang::Yaml, code, Kind::Keyword, "true"));
        assert!(has(Lang::Yaml, code, Kind::String, "\"a\""));
    }

    #[test]
    fn markup_tags_attributes_and_comments() {
        let code = "<!DOCTYPE html>\n<a href=\"https://x\" data-x='1'>oi &amp; 1 < 2</a><!-- c -->";
        for lang in [Lang::Html, Lang::Xml] {
            assert!(has(lang, code, Kind::Tag, "<!DOCTYPE"));
            assert!(has(lang, code, Kind::Tag, "<a"));
            assert!(has(lang, code, Kind::Attr, "href"));
            assert!(has(lang, code, Kind::Attr, "data-x"));
            assert!(has(lang, code, Kind::String, "\"https://x\""));
            assert!(has(lang, code, Kind::Tag, "</a"));
            assert!(has(lang, code, Kind::Comment, "<!-- c -->"));
            // Text outside tags, including a lone `<`, stays plain.
            assert!(!kinds(lang, code).iter().any(|(_, t)| t.contains("oi")));
        }
    }

    #[test]
    fn css_javascript_csharp_swift_typescript_java() {
        assert!(has(
            Lang::Css,
            "a { color: red !important; } /* x */",
            Kind::Keyword,
            "important"
        ));
        assert!(has(
            Lang::Css,
            "a { width: calc(10px) }",
            Kind::Function,
            "calc"
        ));
        assert!(has(Lang::Css, "a { width: 10px }", Kind::Number, "10px"));
        assert!(has(
            Lang::JavaScript,
            "const s = `a\nb`; // c",
            Kind::String,
            "`a\nb`"
        ));
        assert!(has(
            Lang::TypeScript,
            "interface A { x: number }",
            Kind::Keyword,
            "interface"
        ));
        assert!(has(
            Lang::CSharp,
            "public class A { }",
            Kind::Keyword,
            "class"
        ));
        assert!(has(
            Lang::Swift,
            "guard let x = y else { return }",
            Kind::Keyword,
            "guard"
        ));
        assert!(has(
            Lang::Java,
            "public static void main(String[] a)",
            Kind::Function,
            "main"
        ));
    }

    #[test]
    fn comment_markers_inside_strings_are_not_comments() {
        let code = "val u = \"http://x // y\" // c";
        let comments: Vec<_> = kinds(Lang::Kotlin, code)
            .into_iter()
            .filter(|(k, _)| *k == Kind::Comment)
            .collect();
        assert_eq!(comments, vec![(Kind::Comment, "// c")]);
        assert!(has(
            Lang::Kotlin,
            "\"a\\\"b\" x",
            Kind::String,
            "\"a\\\"b\""
        ));
    }

    #[test]
    fn unterminated_input_runs_to_the_end() {
        assert!(has(
            Lang::Kotlin,
            "val a = \"sem fim\nval b = 1",
            Kind::String,
            "\"sem fim"
        ));
        assert!(has(Lang::Kotlin, "val b = 1", Kind::Number, "1"));
        assert!(has(
            Lang::Kotlin,
            "/* aberto\nainda",
            Kind::Comment,
            "/* aberto\nainda"
        ));
        assert!(has(
            Lang::Python,
            "x = '''aberta\nlinhas",
            Kind::String,
            "'''aberta\nlinhas"
        ));
        assert!(has(Lang::Html, "<a href=\"x", Kind::String, "\"x"));
    }

    #[test]
    fn plain_languages_and_big_inputs_are_not_highlighted() {
        for lang in [Lang::Text, Lang::Markdown] {
            assert_eq!(
                highlight(lang, "fun x() // y"),
                vec![(Kind::Plain, "fun x() // y")]
            );
        }
        let big = "val a = 1\n".repeat(MAX_BYTES / 5);
        assert_eq!(highlight(Lang::Kotlin, &big).len(), 1);
    }

    #[test]
    fn language_names_and_aliases() {
        assert_eq!(Lang::from_name("KT"), Some(Lang::Kotlin));
        assert_eq!(Lang::from_name("c#"), Some(Lang::CSharp));
        assert_eq!(Lang::from_name(""), Some(Lang::Text));
        assert_eq!(Lang::from_name("brainfuck"), None);
        assert_eq!(Lang::Shell.label(), "Shell");
    }

    #[test]
    fn classes_are_a_fixed_set() {
        let all = [
            Kind::Keyword,
            Kind::String,
            Kind::Number,
            Kind::Comment,
            Kind::Function,
            Kind::Key,
            Kind::Tag,
            Kind::Attr,
        ];
        for kind in all {
            assert!(kind.class().is_some_and(|c| c.starts_with("tok-")));
        }
        assert_eq!(Kind::Plain.class(), None);
    }

    #[test]
    fn highlighting_big_code_is_fast() {
        let code = "fun f(x: Int): String { return \"a\" + x /* c */ } // d\n".repeat(1500);
        assert!(code.len() < MAX_BYTES);
        let start = std::time::Instant::now();
        for _ in 0..20 {
            assert!(highlight(Lang::Kotlin, &code).len() > 1000);
        }
        assert!(start.elapsed().as_secs() < 5);
    }
}
