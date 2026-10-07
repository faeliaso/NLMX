//! Safe Markdown renderer for model answers (ADR 0020).
//!
//! The model's text is untrusted. It goes through: cleanup (line endings, control characters) →
//! `pulldown-cmark` parser → an event renderer that builds every tag itself from an allow list.
//! Raw HTML from the model is shown as text, images become their alt text, and only `http`,
//! `https` and `mailto` links become anchors. Citation markers `[n]` / `[página N]` are turned
//! into buttons, only in running text (never inside code or links).

use crate::highlight::{self, Lang};
use nlmx_i18n::{t, t_args};
use pulldown_cmark::{
    Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd, TextMergeStream,
};

/// Where the marker of a source leads: its label and the URL that opens it in the side panel — the
/// PDF viewer for a PDF, the source information for any other format.
pub struct SourceLink {
    /// "relatorio.pdf · pp. 2–3".
    pub label: String,
    pub url: String,
    /// Opens the PDF viewer (otherwise: the information panel).
    pub previewable: bool,
}

pub struct Citations<'a> {
    /// Where marker `[n]` leads.
    pub source: &'a dyn Fn(usize) -> Option<SourceLink>,
    /// Viewer URL of a `[página N]` reference.
    pub page: &'a dyn Fn(u32) -> Option<String>,
}

pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    push_escaped(&mut out, text);
    out
}

fn push_escaped(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
}

/// Line endings normalized to `\n`; control characters (NUL, escapes, …) dropped; everything else
/// — accents, emoji, any script, tabs, repeated spaces — kept as is.
fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            '\n' | '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// A link target that may become an anchor: `http`, `https` or `mailto`, with no whitespace or
/// control characters. Everything else (`javascript:`, `data:`, `file:`, relative paths…) is not.
pub fn is_safe_url(url: &str) -> bool {
    let url = url.trim();
    if url.is_empty()
        || url.len() > 2048
        || url.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return false;
    }
    let lower = url.to_ascii_lowercase();
    ["http://", "https://", "mailto:"]
        .iter()
        .any(|scheme| lower.starts_with(scheme) && lower.len() > scheme.len())
}

/// The language of a code fence and its display name; an unknown language keeps a sanitized form
/// of its name and is not highlighted.
fn language(info: &str) -> (Option<Lang>, String) {
    let raw: String = info
        .split(|c: char| c.is_whitespace() || c == '{' || c == ',')
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '#' | '.' | '-' | '_'))
        .take(20)
        .collect();
    match Lang::from_name(&raw) {
        Some(Lang::Text) => (Some(Lang::Text), t("chat-code-plain")),
        Some(lang) => (Some(lang), lang.label().to_string()),
        None => (None, raw),
    }
}

#[derive(Default)]
struct Renderer {
    out: String,
    /// Open code block: (language, label, accumulated text).
    code: Option<(Option<Lang>, String, String)>,
    /// One entry per open link: whether it became an anchor.
    links: Vec<bool>,
    aligns: Vec<Alignment>,
    in_head: bool,
    body_open: bool,
    cell: usize,
}

pub fn render(text: &str, citations: &Citations<'_>) -> String {
    let text = clean(text);
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let events = TextMergeStream::new(Parser::new_ext(&text, options));
    let mut r = Renderer {
        out: String::with_capacity(text.len() * 2),
        ..Renderer::default()
    };
    for event in events {
        r.event(event, citations);
    }
    r.out
}

impl Renderer {
    fn event(&mut self, event: Event<'_>, citations: &Citations<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => {
                if let Some((_, _, code)) = &mut self.code {
                    code.push_str(&t);
                } else if self.links.iter().any(|a| *a) {
                    push_escaped(&mut self.out, &t);
                } else {
                    self.text_with_citations(&t, citations);
                }
            }
            Event::Code(t) => {
                self.out.push_str("<code>");
                push_escaped(&mut self.out, &t);
                self.out.push_str("</code>");
            }
            // Raw HTML (and math, which is not enabled) is shown as text, never as markup.
            Event::Html(t) | Event::InlineHtml(t) => {
                for (i, line) in t.trim_end_matches('\n').split('\n').enumerate() {
                    if i > 0 {
                        self.out.push_str("<br>");
                    }
                    push_escaped(&mut self.out, line);
                }
            }
            Event::SoftBreak | Event::HardBreak => self.out.push_str("<br>"),
            Event::Rule => self.out.push_str("<hr>"),
            Event::TaskListMarker(done) => {
                self.out.push_str(&if done {
                    format!(
                        r#"<input type="checkbox" disabled checked aria-label="{}"> "#,
                        escape(&t("chat-task-done"))
                    )
                } else {
                    format!(
                        r#"<input type="checkbox" disabled aria-label="{}"> "#,
                        escape(&t("chat-task-pending"))
                    )
                });
            }
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.out.push_str("<p>"),
            Tag::Heading { level, .. } => {
                // The page owns h1/h2: answer headings start at h3.
                let n = match level {
                    HeadingLevel::H1 => 3,
                    HeadingLevel::H2 => 4,
                    HeadingLevel::H3 => 5,
                    _ => 6,
                };
                self.out.push_str(&format!("<h{n}>"));
            }
            Tag::BlockQuote(_) => self.out.push_str("<blockquote>"),
            Tag::CodeBlock(kind) => {
                let info = match &kind {
                    CodeBlockKind::Fenced(info) => info.to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                let (lang, label) = language(&info);
                self.code = Some((lang, label, String::new()));
            }
            Tag::HtmlBlock => self.out.push_str("<p>"),
            Tag::List(None) => self.out.push_str("<ul>"),
            Tag::List(Some(1)) => self.out.push_str("<ol>"),
            Tag::List(Some(n)) => self.out.push_str(&format!("<ol start=\"{n}\">")),
            Tag::Item => self.out.push_str("<li>"),
            Tag::Emphasis => self.out.push_str("<em>"),
            Tag::Strong => self.out.push_str("<strong>"),
            Tag::Strikethrough => self.out.push_str("<del>"),
            Tag::Link { dest_url, .. } => {
                let safe = is_safe_url(&dest_url);
                if safe {
                    let url = escape(dest_url.trim());
                    self.out.push_str(&format!(
                        r#"<a class="md-link" href="{url}" title="{url}" data-external rel="noopener noreferrer">"#
                    ));
                }
                self.links.push(safe);
            }
            Tag::Table(aligns) => {
                self.aligns = aligns;
                self.body_open = false;
                self.out.push_str(&format!(
                    r#"<div class="table-scroll" role="region" tabindex="0" aria-label="{}"><table>"#,
                    escape(&t("chat-table"))
                ));
            }
            Tag::TableHead => {
                self.in_head = true;
                self.cell = 0;
                self.out.push_str("<thead><tr>");
            }
            Tag::TableRow => {
                self.cell = 0;
                if !self.body_open {
                    self.out.push_str("<tbody>");
                    self.body_open = true;
                }
                self.out.push_str("<tr>");
            }
            Tag::TableCell => {
                let class = match self.aligns.get(self.cell) {
                    Some(Alignment::Center) => r#" class="align-center""#,
                    Some(Alignment::Right) => r#" class="align-right""#,
                    _ => "",
                };
                self.cell += 1;
                if self.in_head {
                    self.out.push_str(&format!(r#"<th scope="col"{class}>"#));
                } else {
                    self.out.push_str(&format!("<td{class}>"));
                }
            }
            // Images (alt text flows as text), footnotes, metadata: nothing to open.
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::HtmlBlock => self.out.push_str("</p>"),
            TagEnd::Heading(level) => {
                let n = match level {
                    HeadingLevel::H1 => 3,
                    HeadingLevel::H2 => 4,
                    HeadingLevel::H3 => 5,
                    _ => 6,
                };
                self.out.push_str(&format!("</h{n}>"));
            }
            TagEnd::BlockQuote(_) => self.out.push_str("</blockquote>"),
            TagEnd::CodeBlock => {
                if let Some((lang, label, code)) = self.code.take() {
                    self.code_block(lang, &label, code.strip_suffix('\n').unwrap_or(&code));
                }
            }
            TagEnd::List(ordered) => self.out.push_str(if ordered { "</ol>" } else { "</ul>" }),
            TagEnd::Item => self.out.push_str("</li>"),
            TagEnd::Emphasis => self.out.push_str("</em>"),
            TagEnd::Strong => self.out.push_str("</strong>"),
            TagEnd::Strikethrough => self.out.push_str("</del>"),
            TagEnd::Link => {
                if self.links.pop() == Some(true) {
                    self.out.push_str("</a>");
                }
            }
            TagEnd::Table => {
                if self.body_open {
                    self.out.push_str("</tbody>");
                }
                self.out.push_str("</table></div>");
            }
            TagEnd::TableHead => {
                self.in_head = false;
                self.out.push_str("</tr></thead>");
            }
            TagEnd::TableRow => self.out.push_str("</tr>"),
            TagEnd::TableCell => self
                .out
                .push_str(if self.in_head { "</th>" } else { "</td>" }),
            _ => {}
        }
    }

    fn code_block(&mut self, lang: Option<Lang>, label: &str, code: &str) {
        let copy = escape(&t("chat-code-copy"));
        let copy_aria = escape(&t_args(
            "chat-code-copy-aria",
            &[("language", label.into())],
        ));
        let label = escape(label);
        self.out.push_str(&format!(
            r#"<figure class="code-block"><figcaption><span class="code-lang">{label}</span><button type="button" class="btn btn-ghost btn-sm" data-copy-code aria-label="{copy_aria}">{copy}</button></figcaption><pre tabindex="0"><code>"#
        ));
        match lang {
            Some(lang) => {
                for (kind, piece) in highlight::highlight(lang, code) {
                    match kind.class() {
                        Some(class) => {
                            self.out.push_str(&format!(r#"<span class="{class}">"#));
                            push_escaped(&mut self.out, piece);
                            self.out.push_str("</span>");
                        }
                        None => push_escaped(&mut self.out, piece),
                    }
                }
            }
            None => push_escaped(&mut self.out, code),
        }
        self.out.push_str("</code></pre></figure>");
    }

    /// Escapes running text; `[n]` with a source and `[página N]` with a known page become buttons.
    fn text_with_citations(&mut self, text: &str, citations: &Citations<'_>) {
        let mut rest = text;
        while let Some(open) = rest.find('[') {
            let (before, from) = rest.split_at(open);
            push_escaped(&mut self.out, before);
            let Some(close) = from.find(']') else {
                rest = from;
                break;
            };
            let inner = &from[1..close];
            match citation_button(inner, citations) {
                Some(html) => {
                    self.out.push_str(&html);
                    rest = &from[close + 1..];
                }
                None => {
                    self.out.push('[');
                    rest = &from[1..];
                }
            }
        }
        push_escaped(&mut self.out, rest);
    }
}

fn citation_button(inner: &str, citations: &Citations<'_>) -> Option<String> {
    if let Ok(n) = inner.parse::<usize>() {
        return (citations.source)(n).map(|link| {
            let (what, tip) = if link.previewable {
                (
                    t_args("chat-source-open-title", &[("n", n.into())]),
                    t("chat-source-open-pdf"),
                )
            } else {
                (
                    t_args("chat-source-info-title", &[("n", n.into())]),
                    t("chat-source-info-tip"),
                )
            };
            format!(
                r##"<button type="button" class="citation" hx-get="{url}" hx-target="#viewer" aria-label="{what}: {label}" title="{tip} · {label}">{n}</button>"##,
                what = escape(&what),
                tip = escape(&tip),
                url = escape(&link.url),
                label = escape(&link.label),
            )
        });
    }
    let page = inner.strip_prefix("página ")?.parse::<u32>().ok()?;
    (citations.page)(page).map(|url| {
        let open = escape(&t_args("chat-page-open", &[("page", u64::from(page).into())]));
        let text = escape(&t_args("chat-page-ref", &[("page", u64::from(page).into())]));
        format!(
            r##"<button type="button" class="page-ref" hx-get="{url}" hx-target="#viewer" aria-label="{open}" title="{open}">{text}</button>"##,
            url = escape(&url),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> Citations<'static> {
        Citations {
            source: &|_| None,
            page: &|_| None,
        }
    }

    fn md(text: &str) -> String {
        render(text, &none())
    }

    /// The text of the first code block, as the browser would copy it (tags dropped, entities decoded).
    fn code_text(html: &str) -> String {
        let pre = html.find("<pre").unwrap();
        let start = pre + html[pre..].find("<code>").unwrap() + "<code>".len();
        let end = start + html[start..].find("</code>").unwrap();
        let mut text = String::new();
        let mut in_tag = false;
        for c in html[start..end].chars() {
            match c {
                '<' => in_tag = true,
                '>' => in_tag = false,
                c if !in_tag => text.push(c),
                _ => {}
            }
        }
        text.replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#39;", "'")
            .replace("&amp;", "&")
    }

    /// Tags that open and close must balance (void tags excluded).
    fn balanced(html: &str) -> bool {
        let mut stack: Vec<String> = Vec::new();
        let mut rest = html;
        while let Some(i) = rest.find('<') {
            let tail = &rest[i + 1..];
            let end = tail.find('>').expect("tag never closed");
            let tag = &tail[..end];
            rest = &tail[end + 1..];
            let name: String = tag
                .trim_start_matches('/')
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            if matches!(name.as_str(), "br" | "hr" | "input") {
                continue;
            }
            if tag.starts_with('/') {
                if stack.pop().as_deref() != Some(name.as_str()) {
                    return false;
                }
            } else {
                stack.push(name);
            }
        }
        stack.is_empty()
    }

    #[test]
    fn plain_text_is_a_paragraph() {
        assert_eq!(md("Olá, mundo."), "<p>Olá, mundo.</p>");
    }

    #[test]
    fn bold_italic_strike() {
        let html = md("**forte**, *itálico* e ~~riscado~~");
        assert_eq!(
            html,
            "<p><strong>forte</strong>, <em>itálico</em> e <del>riscado</del></p>"
        );
    }

    #[test]
    fn headings_start_below_the_page_titles() {
        let html = md("# Título\n\n## Sub\n\n###### Fim");
        assert!(html.contains("<h3>Título</h3>"));
        assert!(html.contains("<h4>Sub</h4>"));
        assert!(html.contains("<h6>Fim</h6>"));
    }

    #[test]
    fn lists_ordered_unordered_and_nested() {
        let html = md("- a\n- b\n  - b1\n  - b2\n\n3. três\n4. quatro");
        assert!(html.contains("<ul><li>a</li><li>b<ul><li>b1</li><li>b2</li></ul></li></ul>"));
        assert!(html.contains(r#"<ol start="3">"#));
        assert!(balanced(&html));
    }

    #[test]
    fn single_newlines_are_kept_as_breaks() {
        assert_eq!(md("linha 1\nlinha 2"), "<p>linha 1<br>linha 2</p>");
    }

    #[test]
    fn safe_links_become_anchors() {
        let html = md("[site](https://exemplo.com/a?b=1&c=2) e [mail](mailto:a@b.com)");
        assert!(html.contains(r#"href="https://exemplo.com/a?b=1&amp;c=2""#));
        assert!(html.contains("data-external"));
        assert!(html.contains(r#"rel="noopener noreferrer""#));
        assert!(html.contains(r#"href="mailto:a@b.com""#));
    }

    #[test]
    fn dangerous_links_are_plain_text() {
        for url in [
            "javascript:alert(1)",
            "JaVaScRiPt:alert(1)",
            "data:text/html,<script>1</script>",
            "file:///etc/passwd",
            "vbscript:x",
            "/relativo",
            "//evil.com",
        ] {
            let html = md(&format!("[clique]({url})"));
            assert!(!html.contains("<a "), "{url}: {html}");
            assert!(!html.contains("href"), "{url}: {html}");
            assert!(html.contains("clique"));
        }
    }

    #[test]
    fn autolinks_follow_the_same_rule() {
        assert!(md("<https://exemplo.com>").contains("<a "));
        assert!(!md("<javascript:alert(1)>").contains("<a "));
    }

    #[test]
    fn blockquote() {
        assert_eq!(
            md("> Esta é uma observação."),
            "<blockquote><p>Esta é uma observação.</p></blockquote>"
        );
    }

    #[test]
    fn inline_code_is_escaped() {
        assert_eq!(
            md("use `Vec<u8>` aqui"),
            "<p>use <code>Vec&lt;u8&gt;</code> aqui</p>"
        );
    }

    #[test]
    fn code_blocks_keep_indentation_and_name_the_language() {
        let cases = [
            (
                "kotlin",
                "Kotlin",
                "fun main() {\n    println(\"Hello NLMX\")\n}",
            ),
            (
                "rust",
                "Rust",
                "fn main() {\n    let v: Vec<u8> = vec![];\n}",
            ),
            ("python", "Python", "def f():\n    return 1"),
            ("json", "JSON", "{\n  \"a\": [1, 2]\n}"),
            ("sql", "SQL", "SELECT * FROM t\n  WHERE a < 3;"),
            ("bash", "Bash", "echo \"$HOME\" && ls -la"),
        ];
        for (lang, label, code) in cases {
            let html = md(&format!("```{lang}\n{code}\n```"));
            assert!(html.contains(&format!(r#"<span class="code-lang">{label}</span>"#)));
            assert!(html.contains("data-copy-code"));
            assert_eq!(code_text(&html), code, "{html}");
            assert!(balanced(&html));
        }
    }

    #[test]
    fn unknown_or_missing_language_works() {
        let plain = t("chat-code-plain");
        assert!(md("```\nx\n```").contains(&format!(r#"<span class="code-lang">{plain}</span>"#)));
        assert!(md("```text\nx\n```").contains(&plain));
        assert!(md("```brainfuck\n+\n```").contains(">brainfuck<"));
        assert!(md("```<script>\nx\n```").contains(">script<"));
        assert!(md("    indentado\n").contains(&plain));
    }

    #[test]
    fn code_blocks_do_not_activate_citations() {
        let cit = Citations {
            source: &|n| {
                Some(SourceLink {
                    label: "a.pdf".into(),
                    url: format!("/s/{n}"),
                    previewable: true,
                })
            },
            page: &|_| None,
        };
        let html = render("```\n[1]\n```\n\n`[1]`", &cit);
        assert!(!html.contains("citation"), "{html}");
    }

    #[test]
    fn tables_scroll_and_are_semantic() {
        let html = md("| Nome | Status |\n|:-----|-------:|\n| PDF  | OK     |\n| DOCX | OK     |");
        assert!(html.starts_with(r#"<div class="table-scroll" role="region" tabindex="0""#));
        assert!(html.contains(r#"<th scope="col">Nome</th>"#));
        assert!(html.contains(r#"<th scope="col" class="align-right">Status</th>"#));
        assert!(html.contains("<tbody><tr><td>PDF</td>"));
        assert!(balanced(&html));
    }

    #[test]
    fn rule_and_task_list() {
        let html = md("---\n\n- [x] feito\n- [ ] falta");
        assert!(html.contains("<hr>"));
        assert!(html.contains("checked"));
        assert!(html.contains("disabled"));
    }

    #[test]
    fn incomplete_markdown_stays_well_formed() {
        for partial in [
            "```kotlin",
            "```kotlin\nfun main() {",
            "```kotlin\nfun main() {\n}\n``",
            "**negrito sem fim",
            "*itálico",
            "| a | b |\n|---|",
            "| a | b |\n|---|---|\n| 1",
            "[texto](https://exemplo",
            "- item\n  - sub",
            "> citação\n> ",
            "`código",
            "<div",
            "",
        ] {
            let html = md(partial);
            assert!(balanced(&html), "{partial:?} → {html}");
        }
        assert!(md("```kotlin\nfun").contains("<pre"));
    }

    #[test]
    fn utf8_survives_untouched() {
        let text = "A inteligência artificial é capaz de analisar documentos — ação, coração, não, “aspas”, 日本語, العربية, 🚀👍🏽";
        let html = md(text);
        assert_eq!(html, format!("<p>{text}</p>"));
        for mojibake in ["Ã©", "Ã£", "â€œ", "â€�", "\u{fffd}"] {
            assert!(!html.contains(mojibake));
        }
    }

    #[test]
    fn special_characters_are_escaped() {
        assert_eq!(
            md("a & b < c > d \"e\" 'f'"),
            "<p>a &amp; b &lt; c &gt; d &quot;e&quot; &#39;f&#39;</p>"
        );
    }

    #[test]
    fn control_characters_and_crlf_are_cleaned() {
        assert_eq!(md("a\r\nb\u{0}\u{1b}[31m"), "<p>a<br>b[31m</p>");
    }

    #[test]
    fn raw_html_is_text_never_markup() {
        for evil in [
            "<script>alert(1)</script>",
            "<img src=x onerror=alert(1)>",
            "<a href=\"javascript:alert(1)\" onclick=\"x()\">x</a>",
            "<iframe src=\"https://evil\"></iframe>",
            "<svg onload=alert(1)>",
            "texto <b onmouseover=x>negrito</b> fim",
            "<style>*{display:none}</style>",
            "<div>\n<script>alert(1)</script>\n</div>",
        ] {
            let html = md(evil);
            for tag in [
                "<script", "<img", "<iframe", "<svg", "<style", "<div", "<b ", "<a ",
            ] {
                assert!(!html.contains(tag), "{evil} → {html}");
            }
            assert!(
                !html.contains(" onerror=") || html.contains("&lt;"),
                "{html}"
            );
        }
    }

    #[test]
    fn images_are_never_loaded() {
        let html = md("![descrição](https://evil.example/pixel.png)");
        assert!(!html.contains("<img"));
        assert!(html.contains("descrição"));
    }

    #[test]
    fn injection_inside_markdown_constructs_is_escaped() {
        let html = md("**<script>x</script>** [<img src=x onerror=y>](https://a.com) `<b>`");
        assert!(!html.contains("<script"));
        assert!(!html.contains("<img"));
        assert!(!html.contains("<b>"));
        assert!(balanced(&html));
    }

    #[test]
    fn citations_become_buttons_only_with_a_source() {
        let cit = Citations {
            source: &|n| {
                (n == 1).then(|| SourceLink {
                    label: "relatorio.pdf · p. 12".into(),
                    url: "/viewer/1?page=12&cite=5-1".into(),
                    previewable: true,
                })
            },
            page: &|p| (p == 12).then(|| "/viewer/1?page=12&ref=5-12".to_string()),
        };
        let html = render(
            "Segundo o documento [1], veja [2] e [página 12], [página 99].",
            &cit,
        );
        assert!(html.contains(r#"class="citation""#));
        assert!(html.contains("hx-get=\"/viewer/1?page=12&amp;cite=5-1\""));
        assert!(html.contains("[2]"));
        assert!(html.contains(r#"class="page-ref""#));
        assert!(html.contains("[página 99]"));
        assert_eq!(html.matches("<button").count(), 2);
    }

    #[test]
    fn citations_inside_formatting_still_work() {
        let cit = Citations {
            source: &|n| {
                Some(SourceLink {
                    label: "x".into(),
                    url: format!("/s/{n}"),
                    previewable: false,
                })
            },
            page: &|_| None,
        };
        let html = render("- **importante** [1]\n- outro [2]", &cit);
        assert_eq!(html.matches("class=\"citation\"").count(), 2);
        assert!(balanced(&html));
    }

    #[test]
    fn very_long_messages_render_in_bounded_time() {
        let paragraph = "Texto **longo** com `código`, [1] e acentuação — ação.\n\n";
        let text = paragraph.repeat(20_000);
        let start = std::time::Instant::now();
        let html = md(&text);
        assert!(html.len() > text.len());
        assert!(start.elapsed().as_secs() < 5);
        assert!(balanced(&html));
    }

    #[test]
    fn many_messages_render_in_bounded_time() {
        let answer =
            "# T\n\n- a\n- b\n\n```rust\nfn x() {}\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";
        for count in [10, 50, 150] {
            let start = std::time::Instant::now();
            for _ in 0..count {
                assert!(balanced(&md(answer)));
            }
            assert!(start.elapsed().as_secs() < 5, "{count} messages");
        }
    }

    #[test]
    fn safe_url_rules() {
        assert!(is_safe_url("https://a.com"));
        assert!(is_safe_url(" HTTP://a.com "));
        assert!(is_safe_url("mailto:a@b.com"));
        assert!(!is_safe_url("https://a .com"));
        assert!(!is_safe_url("https://a.com/\u{0}"));
        assert!(!is_safe_url("https://"));
        assert!(!is_safe_url("ftp://a.com"));
        assert!(!is_safe_url(""));
    }

    #[test]
    fn highlighted_code_keeps_its_text_and_adds_only_token_spans() {
        let code = "fun main() { // <b>x</b>\n    println(\"</code><script>alert(1)</script>\")\n}";
        let html = md(&format!("```kotlin\n{code}\n```"));
        assert!(
            html.contains(r#"<span class="tok-kw">fun</span>"#),
            "{html}"
        );
        assert!(html.contains(r#"<span class="tok-com">"#));
        assert_eq!(code_text(&html), code);
        assert!(!html.contains("<script"));
        assert!(!html.contains("<b>"));
        assert!(balanced(&html));
        // Only the fixed token classes appear.
        let mut rest = html.as_str();
        while let Some(p) = rest.find("<span class=\"") {
            rest = &rest[p + 13..];
            let class = &rest[..rest.find('"').unwrap()];
            assert!(class == "code-lang" || class.starts_with("tok-"), "{class}");
        }
    }

    #[test]
    fn unknown_languages_and_text_are_not_highlighted() {
        for fence in [
            "```brainfuck\nfun x() // y\n```",
            "```\nfun x() // y\n```",
            "```markdown\n# fun\n```",
        ] {
            let html = md(fence);
            assert!(!html.contains("tok-"), "{fence}: {html}");
        }
    }

    #[test]
    fn partial_highlighted_blocks_stay_well_formed() {
        for partial in [
            "```kotlin\nval s = \"aberta",
            "```sql\nselect /* x",
            "```html\n<a href=\"",
            "```python\nx = '''",
        ] {
            assert!(balanced(&md(partial)), "{partial}");
        }
    }
}
