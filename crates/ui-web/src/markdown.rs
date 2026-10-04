//! A small, safe Markdown renderer for model answers: everything is HTML-escaped first, then
//! only paragraphs, bullet/numbered lists, `**bold**`, headings (as bold paragraphs) and
//! citation markers `[n]` are turned into markup. Raw HTML from the model never reaches the page.

/// How markers become links to the viewer: `[n]` → source `n`, `[página N]` → page `N`.
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
            (citations.source)(n).map(|link| {
                let (what, tip) = if link.previewable {
                    (format!("Abrir a fonte {n} no PDF"), "Abrir no PDF")
                } else {
                    (format!("Ver informações da fonte {n}"), "Ver informações da fonte")
                };
                format!(
                    r##"<button type="button" class="citation" hx-get="{url}" hx-target="#viewer" aria-label="{what}: {label}" title="{tip} · {label}">{n}</button>"##,
                    url = escape(&link.url),
                    label = escape(&link.label),
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

    /// Sources 1 and 2 are PDF pages (they open the viewer); source 3 is a CSV (it opens the
    /// information panel).
    fn cites(n: usize) -> Option<SourceLink> {
        match n {
            1 | 2 => Some(SourceLink {
                label: format!("Relatório, p. {n}"),
                url: format!("/viewer/1?cite=7-{n}"),
                previewable: true,
            }),
            3 => Some(SourceLink {
                label: "dados.csv · linhas 2–9".to_string(),
                url: "/sources/3?cite=7-3".to_string(),
                previewable: false,
            }),
            _ => None,
        }
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
    fn every_format_is_a_button_that_leads_to_its_own_panel() {
        let out = html("A tabela diz isso [3] e o PDF também [1].");
        assert_eq!(out.matches("<button").count(), 2, "{out}");
        // The PDF opens the viewer; the CSV opens the source information, never the viewer.
        assert!(
            out.contains(r##"hx-get="/viewer/1?cite=7-1" hx-target="#viewer""##),
            "{out}"
        );
        assert!(
            out.contains(r##"hx-get="/sources/3?cite=7-3" hx-target="#viewer""##),
            "{out}"
        );
        assert!(out.contains(r#"aria-label="Abrir a fonte 1 no PDF: Relatório, p. 1""#));
        assert!(out.contains(r#"aria-label="Ver informações da fonte 3: dados.csv · linhas 2–9""#));
        assert!(!out.contains("viewer/3"));
    }

    #[test]
    fn citation_labels_are_escaped() {
        let evil = |_: usize| {
            Some(SourceLink {
                label: "\"><script>x</script>".to_string(),
                url: "/v?a=1&b=2".to_string(),
                previewable: true,
            })
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
