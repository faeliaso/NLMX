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
