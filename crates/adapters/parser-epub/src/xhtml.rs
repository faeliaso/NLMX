//! A chapter's XHTML as typed blocks: headings, paragraphs, list items, code and tables.

use nlmx_domain::parsed::ContentKind;
use quick_xml::events::Event;

use crate::xml::{attr, collapse, entity, lower, reader};

/// A block of a chapter before it is located.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawBlock {
    pub kind: ContentKind,
    pub text: String,
}

/// Elements whose whole content is left out.
fn is_skipped(name: &str) -> bool {
    matches!(name, "script" | "style" | "head" | "nav" | "svg" | "math")
}

/// Elements that start and end a paragraph-like block.
fn is_block(name: &str) -> bool {
    matches!(
        name,
        "p" | "div"
            | "blockquote"
            | "section"
            | "article"
            | "aside"
            | "figure"
            | "figcaption"
            | "body"
            | "dd"
            | "dt"
            | "header"
            | "footer"
            | "address"
            | "details"
            | "summary"
            | "main"
            | "hr"
    )
}

fn heading_level(name: &str) -> Option<u8> {
    match name.as_bytes() {
        [b'h', digit @ b'1'..=b'6'] => Some(digit - b'0'),
        _ => None,
    }
}

fn is_void(name: &str) -> bool {
    matches!(
        name,
        "br" | "img" | "meta" | "link" | "input" | "col" | "wbr"
    )
}

#[derive(Default)]
struct Table {
    header: Vec<String>,
    rows: Vec<Vec<String>>,
    row: Vec<String>,
    row_has_th: bool,
    in_thead: bool,
    cell: Option<String>,
    /// Nested tables are read as part of the cell they are in.
    nested: usize,
}

#[derive(Default)]
struct Reading {
    blocks: Vec<RawBlock>,
    text: String,
    pending: Option<ContentKind>,
    skipped: usize,
    lists: Vec<bool>,
    pre: bool,
    table: Option<Table>,
}

impl Reading {
    fn flush(&mut self) {
        let kind = self.pending.take().unwrap_or(ContentKind::Paragraph);
        let raw = std::mem::take(&mut self.text);
        let text = if matches!(kind, ContentKind::CodeBlock { .. }) {
            raw.trim_matches(['\n', '\r'])
                .lines()
                .map(str::trim_end)
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            collapse(&raw)
        };
        if !text.trim().is_empty() {
            self.blocks.push(RawBlock { kind, text });
        }
    }

    fn push_text(&mut self, text: &str) {
        if self.skipped > 0 {
            return;
        }
        match self.table.as_mut() {
            Some(table) => {
                if let Some(cell) = table.cell.as_mut() {
                    cell.push_str(text);
                }
            }
            None => self.text.push_str(text),
        }
    }

    fn start(&mut self, name: &str, class: Option<String>) {
        if self.skipped > 0 {
            if is_skipped(name) {
                self.skipped += 1;
            }
            return;
        }
        if is_skipped(name) {
            if self.table.is_none() {
                self.flush();
            }
            self.skipped = 1;
            return;
        }
        if self.table.is_some() {
            self.start_in_table(name);
            return;
        }
        if let Some(level) = heading_level(name) {
            self.flush();
            self.pending = Some(ContentKind::Heading { level });
            return;
        }
        match name {
            "ul" | "ol" => {
                self.flush();
                self.lists.push(name == "ol");
            }
            "li" => {
                self.flush();
                self.pending = Some(ContentKind::ListItem {
                    ordered: self.lists.last().copied().unwrap_or(false),
                    depth: self.lists.len().saturating_sub(1).min(u8::MAX as usize) as u8,
                });
            }
            "pre" => {
                self.flush();
                self.pre = true;
                self.pending = Some(ContentKind::CodeBlock { language: None });
            }
            "code" if self.pre => {
                if let Some(language) = class.as_deref().and_then(language_of) {
                    self.pending = Some(ContentKind::CodeBlock {
                        language: Some(language),
                    });
                }
            }
            "table" => {
                self.flush();
                self.table = Some(Table::default());
            }
            "br" => self.text.push(if self.pre { '\n' } else { ' ' }),
            _ if is_block(name) => {
                self.flush();
            }
            _ => {}
        }
    }

    fn start_in_table(&mut self, name: &str) {
        let Some(table) = self.table.as_mut() else {
            return;
        };
        match name {
            "table" => table.nested += 1,
            _ if table.nested > 0 => {}
            "thead" => table.in_thead = true,
            "tr" => {
                table.row.clear();
                table.row_has_th = false;
            }
            "td" | "th" => {
                table.row_has_th |= name == "th";
                table.cell = Some(String::new());
            }
            "br" => {
                if let Some(cell) = table.cell.as_mut() {
                    cell.push(' ');
                }
            }
            _ => {}
        }
    }

    fn end(&mut self, name: &str) {
        if self.skipped > 0 {
            if is_skipped(name) {
                self.skipped -= 1;
            }
            return;
        }
        if self.table.is_some() {
            self.end_in_table(name);
            return;
        }
        if heading_level(name).is_some() {
            self.flush();
            return;
        }
        match name {
            "ul" | "ol" => {
                self.flush();
                self.lists.pop();
            }
            "li" => self.flush(),
            "pre" => {
                self.flush();
                self.pre = false;
            }
            _ if is_block(name) => self.flush(),
            _ => {}
        }
    }

    fn end_in_table(&mut self, name: &str) {
        let Some(table) = self.table.as_mut() else {
            return;
        };
        if name == "table" && table.nested > 0 {
            table.nested -= 1;
            return;
        }
        if table.nested > 0 {
            return;
        }
        match name {
            "thead" => table.in_thead = false,
            "td" | "th" => {
                if let Some(cell) = table.cell.take() {
                    table.row.push(collapse(&cell));
                }
            }
            "tr" => {
                let row = std::mem::take(&mut table.row);
                if !row.is_empty() {
                    let is_header = table.header.is_empty()
                        && table.rows.is_empty()
                        && (table.row_has_th || table.in_thead);
                    if is_header {
                        table.header = row;
                    } else {
                        table.rows.push(row);
                    }
                }
            }
            "table" => self.finish_table(),
            _ => {}
        }
    }

    fn finish_table(&mut self) {
        let Some(mut table) = self.table.take() else {
            return;
        };
        // A row still open (no `</tr>`) counts.
        if !table.row.is_empty() {
            table.rows.push(std::mem::take(&mut table.row));
        }
        let lines: Vec<String> = std::iter::once(&table.header)
            .filter(|h| !h.is_empty())
            .chain(table.rows.iter())
            .map(|row| row.join(" | "))
            .collect();
        let text = lines.join("\n");
        if !text.trim().is_empty() {
            self.blocks.push(RawBlock {
                kind: ContentKind::Table {
                    header: table.header,
                    rows: table.rows,
                },
                text,
            });
        }
    }
}

/// `language-rust` / `lang-rust` in a class list.
fn language_of(class: &str) -> Option<String> {
    class.split_whitespace().find_map(|c| {
        c.strip_prefix("language-")
            .or_else(|| c.strip_prefix("lang-"))
            .filter(|l| !l.is_empty())
            .map(str::to_string)
    })
}

/// The blocks of a chapter in reading order. `Err` when the XML cannot be read.
pub(crate) fn parse_chapter(bytes: &[u8]) -> Result<Vec<RawBlock>, ()> {
    let mut reader = reader(bytes);
    let mut reading = Reading::default();
    loop {
        match reader.read_event().map_err(drop)? {
            Event::Start(e) => {
                let name = lower(e.local_name().as_ref());
                if is_void(&name) && name != "br" {
                    continue;
                }
                reading.start(&name, attr(&e, "class"));
            }
            Event::End(e) => {
                let name = lower(e.local_name().as_ref());
                if !is_void(&name) {
                    reading.end(&name);
                }
            }
            Event::Text(t) => {
                let text = t.decode().map_err(drop)?;
                reading.push_text(&text);
            }
            Event::CData(t) => {
                let text = t.decode().map_err(drop)?;
                reading.push_text(&text);
            }
            Event::GeneralRef(r) => {
                if let Some(text) = entity(&r) {
                    reading.push_text(&text);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if reading.table.is_some() {
        reading.finish_table();
    }
    reading.flush();
    Ok(reading.blocks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocks(body: &str) -> Vec<(ContentKind, String)> {
        let xhtml = format!(
            r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><head><title>Ignorado</title><style>p {{ color: red }}</style></head><body>{body}</body></html>"#
        );
        parse_chapter(xhtml.as_bytes())
            .unwrap()
            .into_iter()
            .map(|b| (b.kind, b.text))
            .collect()
    }

    #[test]
    fn headings_and_paragraphs_keep_their_kind() {
        let got = blocks(
            "<h1>Título</h1><p>Um   texto\ncom <em>ênfase</em> e&nbsp;espaço.</p><h3>Sub</h3><p>Fim</p>",
        );
        assert_eq!(
            got,
            [
                (ContentKind::Heading { level: 1 }, "Título".to_string()),
                (
                    ContentKind::Paragraph,
                    "Um texto com ênfase e espaço.".to_string()
                ),
                (ContentKind::Heading { level: 3 }, "Sub".to_string()),
                (ContentKind::Paragraph, "Fim".to_string()),
            ]
        );
    }

    #[test]
    fn head_script_style_and_nav_are_left_out() {
        let got = blocks(
            "<nav><ol><li><a href='x'>Sumário</a></li></ol></nav><script>var x = 1;</script><p>Corpo</p>",
        );
        assert_eq!(got, [(ContentKind::Paragraph, "Corpo".to_string())]);
    }

    #[test]
    fn lists_keep_order_and_depth() {
        let got = blocks("<ul><li>a</li><li>b<ol><li>b1</li></ol></li></ul><p>depois</p>");
        assert_eq!(
            got,
            [
                (
                    ContentKind::ListItem {
                        ordered: false,
                        depth: 0
                    },
                    "a".to_string()
                ),
                (
                    ContentKind::ListItem {
                        ordered: false,
                        depth: 0
                    },
                    "b".to_string()
                ),
                (
                    ContentKind::ListItem {
                        ordered: true,
                        depth: 1
                    },
                    "b1".to_string()
                ),
                (ContentKind::Paragraph, "depois".to_string()),
            ]
        );
    }

    #[test]
    fn code_blocks_keep_their_lines_and_language() {
        let got = blocks(
            "<pre><code class=\"language-rust\">fn main() {\n    println!(\"oi\");\n}\n</code></pre>",
        );
        assert_eq!(
            got,
            [(
                ContentKind::CodeBlock {
                    language: Some("rust".into())
                },
                "fn main() {\n    println!(\"oi\");\n}".to_string()
            )]
        );
        let plain = blocks("<pre>a\n  b</pre>");
        assert_eq!(
            plain,
            [(
                ContentKind::CodeBlock { language: None },
                "a\n  b".to_string()
            )]
        );
    }

    #[test]
    fn tables_keep_header_rows_and_a_pipe_text() {
        let got = blocks(
            "<table><thead><tr><th>Item</th><th>Preço</th></tr></thead><tbody><tr><td>Café</td><td>12,50</td></tr><tr><td>Chá</td><td>8</td></tr></tbody></table><p>depois</p>",
        );
        assert_eq!(got.len(), 2);
        assert_eq!(
            got[0].0,
            ContentKind::Table {
                header: vec!["Item".into(), "Preço".into()],
                rows: vec![
                    vec!["Café".into(), "12,50".into()],
                    vec!["Chá".into(), "8".into()]
                ],
            }
        );
        assert_eq!(got[0].1, "Item | Preço\nCafé | 12,50\nChá | 8");
        assert_eq!(got[1].1, "depois");
    }

    #[test]
    fn a_table_without_a_header_has_only_rows() {
        let got = blocks("<table><tr><td>a</td><td>b</td></tr></table>");
        assert_eq!(
            got,
            [(
                ContentKind::Table {
                    header: vec![],
                    rows: vec![vec!["a".into(), "b".into()]]
                },
                "a | b".to_string()
            )]
        );
    }

    #[test]
    fn blockquotes_and_loose_text_become_paragraphs() {
        let got = blocks("texto solto<blockquote><p>citação</p></blockquote>mais<br/>texto");
        let texts: Vec<_> = got.iter().map(|b| b.1.as_str()).collect();
        assert_eq!(texts, ["texto solto", "citação", "mais texto"]);
        assert!(got.iter().all(|b| b.0 == ContentKind::Paragraph));
    }

    #[test]
    fn real_world_html_is_tolerated() {
        // Unclosed void tags, wrong end tags and unknown entities.
        let got = blocks("<p>a<br>b <b>negrito</i> &foo; fim</p>");
        assert_eq!(
            got,
            [(ContentKind::Paragraph, "a b negrito fim".to_string())]
        );
    }

    #[test]
    fn unreadable_xml_is_an_error() {
        assert!(parse_chapter(b"<p>texto \xff\xfe inv\xe1lido</p>").is_err());
    }
}
