//! A small, safe Markdown renderer for model answers: everything is HTML-escaped first, then
//! only paragraphs, bullet/numbered lists, `**bold**`, headings (as bold paragraphs) and
//! citation markers `[n]` are turned into markup. Raw HTML from the model never reaches the page.

/// How markers become links to the viewer: `[n]` → source `n`, `[página N]` → page `N`.
pub struct Citations<'a> {
    /// Label ("Relatório, pp. 2–3") and viewer URL of source `n`.
    pub source: &'a dyn Fn(usize) -> Option<(String, String)>,
    /// Viewer URL of a `[página N]` reference.
    pub page: &'a dyn Fn(u32) -> Option<String>,
}

pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
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
    out
}

enum Block {
    Paragraph(Vec<String>),
    Bullets(Vec<String>),
    Numbered(Vec<String>),
}

pub fn render(text: &str, citations: &Citations<'_>) -> String {
    let mut blocks: Vec<Block> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            blocks.push(Block::Paragraph(Vec::new()));
            continue;
        }
        if let Some(item) = bullet(trimmed) {
            match blocks.last_mut() {
                Some(Block::Bullets(items)) => items.push(item.to_string()),
                _ => blocks.push(Block::Bullets(vec![item.to_string()])),
            }
        } else if let Some(item) = numbered(trimmed) {
            match blocks.last_mut() {
                Some(Block::Numbered(items)) => items.push(item.to_string()),
                _ => blocks.push(Block::Numbered(vec![item.to_string()])),
            }
        } else {
            let line = heading(trimmed)
                .map(|h| format!("**{h}**"))
                .unwrap_or_else(|| trimmed.to_string());
            match blocks.last_mut() {
                Some(Block::Paragraph(lines)) if !lines.is_empty() => lines.push(line),
                _ => blocks.push(Block::Paragraph(vec![line])),
            }
        }
    }
    let mut html = String::new();
    for block in blocks {
        match block {
            Block::Paragraph(lines) if lines.is_empty() => {}
            Block::Paragraph(lines) => {
                let body: Vec<String> = lines.iter().map(|l| inline(l, citations)).collect();
                html.push_str(&format!("<p>{}</p>", body.join("<br>")));
            }
            Block::Bullets(items) => list(&mut html, "ul", &items, citations),
            Block::Numbered(items) => list(&mut html, "ol", &items, citations),
        }
    }
    html
}

fn list(html: &mut String, tag: &str, items: &[String], citations: &Citations<'_>) {
    html.push_str(&format!("<{tag}>"));
    for item in items {
        html.push_str(&format!("<li>{}</li>", inline(item, citations)));
    }
    html.push_str(&format!("</{tag}>"));
}

fn bullet(line: &str) -> Option<&str> {
    ["- ", "* ", "• ", "– "]
        .iter()
        .find_map(|p| line.strip_prefix(p))
        .map(str::trim)
}

fn numbered(line: &str) -> Option<&str> {
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 || digits > 3 {
        return None;
    }
    let rest = &line[digits..];
    rest.strip_prefix(". ")
        .or_else(|| rest.strip_prefix(") "))
        .map(str::trim)
}

fn heading(line: &str) -> Option<&str> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    (1..=6)
        .contains(&hashes)
        .then(|| line[hashes..].strip_prefix(' '))
        .flatten()
        .map(str::trim)
}

/// Escapes a line, then applies `**bold**` and citation markers.
fn inline(line: &str, citations: &Citations<'_>) -> String {
    let escaped = escape(line);
    // Bold: pairs of ** only.
    let mut bold = String::with_capacity(escaped.len());
    let parts: Vec<&str> = escaped.split("**").collect();
    let pairs = (parts.len() - 1) / 2 * 2;
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            if i <= pairs {
                bold.push_str(if i % 2 == 1 { "<strong>" } else { "</strong>" });
            } else {
                bold.push_str("**");
            }
        }
        bold.push_str(part);
    }
    // Citations: [n] with a number that has a source; [página N] with a known page.
    let mut out = String::with_capacity(bold.len());
    let mut rest = bold.as_str();
    while let Some(open) = rest.find('[') {
        let (before, from) = rest.split_at(open);
        out.push_str(before);
        let Some(close) = from.find(']') else {
            out.push_str(from);
            rest = "";
            break;
        };
        let inner = &from[1..close];
        let button = if let Ok(n) = inner.parse::<usize>() {
            (citations.source)(n).map(|(label, url)| {
                format!(
                    r##"<button type="button" class="citation" hx-get="{url}" hx-target="#viewer" aria-label="Abrir a fonte {n} no PDF: {label}" title="Abrir no PDF · {label}">{n}</button>"##,
                    url = escape(&url),
                    label = escape(&label),
                )
            })
        } else if let Some(page) = inner
            .strip_prefix("página ")
            .and_then(|p| p.parse::<u32>().ok())
        {
            (citations.page)(page).map(|url| {
                format!(
                    r##"<button type="button" class="page-ref" hx-get="{url}" hx-target="#viewer" aria-label="Abrir a página {page} no PDF" title="Abrir a página {page} no PDF">página {page}</button>"##,
                    url = escape(&url),
                )
            })
        } else {
            None
        };
        match button {
            Some(html) => {
                out.push_str(&html);
                rest = &from[close + 1..];
            }
            None => {
                out.push('[');
                rest = &from[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The answer as plain text, for copying: markers kept, Markdown symbols removed.
pub fn plain(text: &str) -> String {
    text.lines()
        .map(|l| {
            let t = l.trim_start();
            let t = heading(t).unwrap_or(t);
            t.replace("**", "")
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cites(n: usize) -> Option<(String, String)> {
        (n <= 2).then(|| {
            (
                format!("Relatório, p. {n}"),
                format!("/viewer/1?cite=7-{n}"),
            )
        })
    }

    fn page(p: u32) -> Option<String> {
        (p == 4).then(|| "/viewer/1?ref=7-4".to_string())
    }

    fn html(text: &str) -> String {
        render(
            text,
            &Citations {
                source: &cites,
                page: &page,
            },
        )
    }

    #[test]
    fn renders_paragraphs_lists_bold_and_citations() {
        let out = html(
            "## Visão geral\nO prazo é **180 dias** [1].\n\n- Consultas [2]\n- Exames\n\n1. Primeiro\n2. Segundo",
        );
        assert_eq!(
            out,
            concat!(
                "<p><strong>Visão geral</strong><br>O prazo é <strong>180 dias</strong> ",
                r##"<button type="button" class="citation" hx-get="/viewer/1?cite=7-1" hx-target="#viewer" aria-label="Abrir a fonte 1 no PDF: Relatório, p. 1" title="Abrir no PDF · Relatório, p. 1">1</button>.</p>"##,
                "<ul><li>Consultas ",
                r##"<button type="button" class="citation" hx-get="/viewer/1?cite=7-2" hx-target="#viewer" aria-label="Abrir a fonte 2 no PDF: Relatório, p. 2" title="Abrir no PDF · Relatório, p. 2">2</button></li><li>Exames</li></ul>"##,
                "<ol><li>Primeiro</li><li>Segundo</li></ol>"
            )
        );
    }

    #[test]
    fn model_html_is_escaped_and_unknown_markers_stay_text() {
        let out = html("<script>alert(1)</script> <img src=x onerror=y> [9] [a] **sem par");
        assert!(!out.contains("<script") && !out.contains("<img"));
        assert!(out.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(out.contains("[9] [a] **sem par"));
        assert!(!out.contains("citation"));
    }

    #[test]
    fn page_references_become_viewer_links() {
        let out = html("Ver [página 4] e [página 9].");
        assert_eq!(
            out,
            r##"<p>Ver <button type="button" class="page-ref" hx-get="/viewer/1?ref=7-4" hx-target="#viewer" aria-label="Abrir a página 4 no PDF" title="Abrir a página 4 no PDF">página 4</button> e [página 9].</p>"##
        );
    }

    #[test]
    fn citation_labels_are_escaped() {
        let evil = |_: usize| {
            Some((
                "\"><script>x</script>".to_string(),
                "/v?a=1&b=2".to_string(),
            ))
        };
        let out = render(
            "Ver [1].",
            &Citations {
                source: &evil,
                page: &|_| None,
            },
        );
        assert!(out.contains("/v?a=1&amp;b=2"));
        assert!(!out.contains("<script>"));
        assert!(out.contains("&quot;&gt;&lt;script&gt;"));
    }

    #[test]
    fn plain_text_for_copying() {
        assert_eq!(
            plain("## Título\nTexto **forte** [1].\n- item"),
            "Título\nTexto forte [1].\n- item"
        );
    }
}
