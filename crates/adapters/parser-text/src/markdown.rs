//! Markdown: headings give the hierarchy; paragraphs, list items, code blocks and tables are
//! kept as typed blocks located by their (1-based) source lines.

use nlmx_application::ports::{BoxFuture, DocumentParser, DocumentSource};
use nlmx_domain::{
    document_type::DocumentType,
    parsed::{
        ContentBlock, ContentKind, DocumentMetadata, DocumentSection, ParseError, ParseWarning,
        ParsedDocument,
    },
    source::SourceLocation,
};
use pulldown_cmark::{
    CodeBlockKind, Event, HeadingLevel, MetadataBlockKind, Options, Parser, Tag, TagEnd,
};

use crate::{decode::decode, lines::LineIndex, parse_file};

const VERSION: u32 = 1;
const KIND: DocumentType = DocumentType::Markdown;

/// Markdown (CommonMark with tables, task lists, strikethrough and YAML front matter).
/// Raw HTML is ignored; links, emphasis and inline code read as their text.
#[derive(Debug, Clone, Copy, Default)]
pub struct MarkdownDocumentParser;

impl MarkdownDocumentParser {
    pub fn parse_bytes(bytes: &[u8]) -> Result<ParsedDocument, ParseError> {
        let decoded = decode(bytes, KIND)?;
        if decoded.text.trim().is_empty() {
            return Err(ParseError::Empty);
        }
        let mut builder = Builder::new(&decoded.text);
        builder.run();
        let (metadata, sections) = builder.finish();
        if !sections
            .iter()
            .any(|s| s.blocks.iter().any(|b| !b.text.trim().is_empty()))
        {
            return Err(ParseError::Empty);
        }
        let warnings = if decoded.fallback {
            vec![ParseWarning::FallbackEncoding]
        } else {
            Vec::new()
        };
        ParsedDocument::new(KIND, metadata, Vec::new(), sections, warnings)
            .map_err(|_| ParseError::Invalid(KIND))
    }
}

impl DocumentParser for MarkdownDocumentParser {
    fn document_type(&self) -> DocumentType {
        KIND
    }

    fn version(&self) -> u32 {
        VERSION
    }

    fn parse<'a>(
        &'a self,
        source: &'a DocumentSource,
    ) -> BoxFuture<'a, Result<ParsedDocument, ParseError>> {
        parse_file(KIND, source, Self::parse_bytes)
    }
}

enum FrameKind {
    Heading(u8),
    Paragraph,
    Item { ordered: bool, depth: u8 },
    Code { language: Option<String> },
    Cell,
    FrontMatter,
}

/// A block being read: its text so far and the byte range it covers.
struct Frame {
    kind: FrameKind,
    text: String,
    start: usize,
    end: usize,
}

impl Frame {
    fn new(kind: FrameKind, start: usize) -> Self {
        Self {
            kind,
            text: String::new(),
            start,
            end: start,
        }
    }
}

#[derive(Default)]
struct TableState {
    start: usize,
    end: usize,
    header: Vec<String>,
    rows: Vec<Vec<String>>,
    row: Vec<String>,
}

struct SectionBuf {
    title: Option<String>,
    level: u8,
    path: Vec<String>,
    blocks: Vec<ContentBlock>,
}

struct Builder<'a> {
    text: &'a str,
    lines: LineIndex,
    /// Open headings, outermost first.
    headings: Vec<(u8, String)>,
    frames: Vec<Frame>,
    lists: Vec<bool>,
    table: Option<TableState>,
    current: Option<SectionBuf>,
    sections: Vec<DocumentSection>,
    metadata: DocumentMetadata,
    first_h1: Option<String>,
}

fn level_number(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

impl<'a> Builder<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            lines: LineIndex::new(text),
            headings: Vec::new(),
            frames: Vec::new(),
            lists: Vec::new(),
            table: None,
            current: None,
            sections: Vec::new(),
            metadata: DocumentMetadata::default(),
            first_h1: None,
        }
    }

    fn path(&self) -> Vec<String> {
        self.headings
            .iter()
            .map(|(_, title)| title.clone())
            .collect()
    }

    fn flush_section(&mut self) {
        if let Some(section) = self.current.take() {
            self.sections.extend(DocumentSection::new(
                section.title,
                section.level,
                section.path,
                section.blocks,
            ));
        }
    }

    fn start_section(&mut self, title: Option<String>, level: u8) {
        self.flush_section();
        self.current = Some(SectionBuf {
            title,
            level,
            path: self.path(),
            blocks: Vec::new(),
        });
    }

    /// Adds a block to the current section (a preamble section if there is none yet),
    /// located by the lines of `start..end` (bytes).
    fn emit(&mut self, kind: ContentKind, text: &str, start: usize, end: usize) {
        if text.trim().is_empty() {
            return;
        }
        let first = self.lines.line_of(start);
        let last = self
            .lines
            .line_of(end.saturating_sub(1).max(start))
            .max(first);
        let path = self.path();
        let Ok(location) = SourceLocation::markdown(path, Some((first, last))) else {
            return;
        };
        if self.current.is_none() {
            self.start_section(None, 0);
        }
        if let Some(section) = self.current.as_mut() {
            section.blocks.push(ContentBlock {
                kind,
                text: text.to_string(),
                location,
            });
        }
    }

    fn run(&mut self) {
        let mut options = Options::ENABLE_TABLES
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS;
        options.remove(Options::empty());
        let text = self.text;
        for (event, range) in Parser::new_ext(text, options).into_offset_iter() {
            self.handle(event, range);
        }
        self.flush_section();
    }

    fn append(&mut self, piece: &str, end: usize) {
        if let Some(frame) = self.frames.last_mut() {
            frame.text.push_str(piece);
            frame.end = frame.end.max(end);
        }
    }

    fn handle(&mut self, event: Event<'_>, range: std::ops::Range<usize>) {
        match event {
            Event::Start(Tag::MetadataBlock(MetadataBlockKind::YamlStyle)) => {
                self.frames
                    .push(Frame::new(FrameKind::FrontMatter, range.start));
            }
            Event::Start(Tag::Heading { level, .. }) => {
                self.frames.push(Frame::new(
                    FrameKind::Heading(level_number(level)),
                    range.start,
                ));
            }
            Event::Start(Tag::Paragraph) => {
                self.frames
                    .push(Frame::new(FrameKind::Paragraph, range.start));
            }
            Event::Start(Tag::List(first)) => {
                self.flush_item_text();
                self.lists.push(first.is_some());
            }
            Event::End(TagEnd::List(_)) => {
                self.lists.pop();
            }
            Event::Start(Tag::Item) => {
                let kind = FrameKind::Item {
                    ordered: self.lists.last().copied().unwrap_or(false),
                    depth: self.lists.len().saturating_sub(1).min(u8::MAX as usize) as u8,
                };
                self.frames.push(Frame::new(kind, range.start));
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                let language = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().map(str::to_string)
                    }
                    CodeBlockKind::Indented => None,
                };
                self.frames
                    .push(Frame::new(FrameKind::Code { language }, range.start));
            }
            Event::Start(Tag::Table(_)) => {
                self.table = Some(TableState {
                    start: range.start,
                    end: range.start,
                    ..TableState::default()
                });
            }
            Event::Start(Tag::TableCell) => {
                self.frames.push(Frame::new(FrameKind::Cell, range.start));
            }
            Event::End(TagEnd::TableCell) => {
                if let Some(frame) = self.frames.pop() {
                    if let Some(table) = self.table.as_mut() {
                        table.row.push(frame.text.trim().to_string());
                        table.end = table.end.max(frame.end);
                    }
                }
            }
            Event::End(TagEnd::TableHead) => {
                if let Some(table) = self.table.as_mut() {
                    table.header = std::mem::take(&mut table.row);
                }
            }
            Event::End(TagEnd::TableRow) => {
                if let Some(table) = self.table.as_mut() {
                    let row = std::mem::take(&mut table.row);
                    table.rows.push(row);
                }
            }
            Event::End(TagEnd::Table) => self.finish_table(),
            Event::Text(piece) | Event::Code(piece) => self.append(&piece, range.end),
            Event::SoftBreak => self.append(" ", range.end),
            Event::HardBreak => self.append("\n", range.end),
            Event::TaskListMarker(done) => {
                self.append(if done { "[x] " } else { "[ ] " }, range.end);
            }
            Event::End(TagEnd::Heading(_)) => self.finish_heading(),
            Event::End(TagEnd::Paragraph) => self.finish_paragraph(),
            Event::End(TagEnd::Item) => self.finish_item(),
            Event::End(TagEnd::CodeBlock) => {
                if let Some(Frame {
                    kind: FrameKind::Code { language },
                    text,
                    start,
                    ..
                }) = self.frames.pop()
                {
                    let literal = text.trim_end_matches('\n');
                    self.emit(
                        ContentKind::CodeBlock { language },
                        literal,
                        start,
                        range.end,
                    );
                }
            }
            Event::End(TagEnd::MetadataBlock(_)) => {
                if let Some(frame) = self.frames.pop() {
                    self.read_front_matter(&frame.text);
                }
            }
            _ => {}
        }
    }

    /// Emits the text of an open list item before a nested list starts.
    fn flush_item_text(&mut self) {
        let Some(frame) = self.frames.last_mut() else {
            return;
        };
        let FrameKind::Item { ordered, depth } = frame.kind else {
            return;
        };
        let text = std::mem::take(&mut frame.text);
        let (start, end) = (frame.start, frame.end);
        self.emit(
            ContentKind::ListItem { ordered, depth },
            text.trim(),
            start,
            end,
        );
    }

    fn finish_item(&mut self) {
        if let Some(Frame {
            kind: FrameKind::Item { ordered, depth },
            text,
            start,
            end,
        }) = self.frames.pop()
        {
            self.emit(
                ContentKind::ListItem { ordered, depth },
                text.trim(),
                start,
                end,
            );
        }
    }

    fn finish_paragraph(&mut self) {
        let Some(frame) = self.frames.pop() else {
            return;
        };
        // A paragraph inside a list item is part of the item.
        if let Some(parent) = self.frames.last_mut() {
            if matches!(parent.kind, FrameKind::Item { .. }) {
                if !parent.text.is_empty() {
                    parent.text.push('\n');
                }
                parent.text.push_str(frame.text.trim());
                parent.end = parent.end.max(frame.end);
                return;
            }
        }
        self.emit(
            ContentKind::Paragraph,
            frame.text.trim(),
            frame.start,
            frame.end,
        );
    }

    fn finish_heading(&mut self) {
        let Some(frame) = self.frames.pop() else {
            return;
        };
        let FrameKind::Heading(level) = frame.kind else {
            return;
        };
        let title = frame.text.trim().to_string();
        if title.is_empty() {
            return;
        }
        while self.headings.last().is_some_and(|(open, _)| *open >= level) {
            self.headings.pop();
        }
        self.headings.push((level, title.clone()));
        if level == 1 && self.first_h1.is_none() {
            self.first_h1 = Some(title.clone());
        }
        self.start_section(Some(title.clone()), level);
        self.emit(
            ContentKind::Heading { level },
            &title,
            frame.start,
            frame.end,
        );
    }

    fn finish_table(&mut self) {
        let Some(table) = self.table.take() else {
            return;
        };
        let mut lines = Vec::new();
        if !table.header.is_empty() {
            lines.push(table.header.join(" | "));
        }
        lines.extend(table.rows.iter().map(|row| row.join(" | ")));
        let text = lines.join("\n");
        let (start, end) = (table.start, table.end);
        self.emit(
            ContentKind::Table {
                header: table.header,
                rows: table.rows,
            },
            &text,
            start,
            end,
        );
    }

    fn read_front_matter(&mut self, text: &str) {
        for line in text.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim().trim_matches(|c| c == '"' || c == '\'').trim();
            if value.is_empty() {
                continue;
            }
            let slot = match key.trim().to_lowercase().as_str() {
                "title" => &mut self.metadata.title,
                "author" | "authors" => &mut self.metadata.author,
                "language" | "lang" => &mut self.metadata.language,
                _ => continue,
            };
            slot.get_or_insert_with(|| value.to_string());
        }
    }

    fn finish(mut self) -> (DocumentMetadata, Vec<DocumentSection>) {
        self.flush_section();
        if self.metadata.title.is_none() {
            self.metadata.title = self.first_h1.take();
        }
        (self.metadata, self.sections)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "---
title: Guia de Teste
author: Maria
language: pt-BR
---
Prefácio antes dos títulos.

# Guia

Introdução com **negrito**, [link](http://x.y) e `código`.

## Instalação

1. Baixe
2. Instale
   - sub item
3. Rode

- solto

```rust
fn main() {}
```

| a | b |
|---|---|
| 1 | 2 |

> citação aqui

### Detalhe

Texto final.

## Uso

Última seção.
";

    fn parse(text: &str) -> ParsedDocument {
        MarkdownDocumentParser::parse_bytes(text.as_bytes()).unwrap()
    }

    fn lines_of(block: &ContentBlock) -> (u32, u32) {
        match &block.location {
            SourceLocation::Markdown {
                line_start: Some(a),
                line_end: Some(b),
                ..
            } => (*a, *b),
            other => panic!("unexpected location {other:?}"),
        }
    }

    #[test]
    fn metadata_comes_from_the_front_matter() {
        let parsed = parse(DOC);
        let metadata = parsed.metadata();
        assert_eq!(metadata.title.as_deref(), Some("Guia de Teste"));
        assert_eq!(metadata.author.as_deref(), Some("Maria"));
        assert_eq!(metadata.language.as_deref(), Some("pt-BR"));
    }

    #[test]
    fn the_first_h1_is_the_title_without_front_matter() {
        let parsed = parse("Texto.\n\n# Meu Título\n\n## Outro\n");
        assert_eq!(parsed.metadata().title.as_deref(), Some("Meu Título"));
        assert_eq!(parse("só um parágrafo").metadata().title, None);
    }

    #[test]
    fn headings_form_the_section_hierarchy() {
        let parsed = parse(DOC);
        let sections: Vec<_> = parsed
            .sections()
            .iter()
            .map(|s| (s.title.clone(), s.level, s.path.clone()))
            .collect();
        let path = |parts: &[&str]| parts.iter().map(|p| p.to_string()).collect::<Vec<_>>();
        assert_eq!(
            sections,
            [
                (None, 0, vec![]),
                (Some("Guia".into()), 1, path(&["Guia"])),
                (Some("Instalação".into()), 2, path(&["Guia", "Instalação"])),
                (
                    Some("Detalhe".into()),
                    3,
                    path(&["Guia", "Instalação", "Detalhe"])
                ),
                (Some("Uso".into()), 2, path(&["Guia", "Uso"])),
            ]
        );
        // The preamble is a section of its own, before the first heading.
        assert_eq!(
            parsed.sections()[0].blocks[0].text,
            "Prefácio antes dos títulos."
        );
    }

    #[test]
    fn blocks_keep_their_kind_and_source_lines() {
        let parsed = parse(DOC);
        let found: Vec<_> = parsed
            .blocks()
            .map(|b| (b.text.clone(), lines_of(b)))
            .collect();
        let find = |text: &str| found.iter().find(|(t, _)| t == text).unwrap().1;
        assert_eq!(find("Prefácio antes dos títulos."), (6, 6));
        assert_eq!(find("Guia"), (8, 8));
        assert_eq!(find("Introdução com negrito, link e código."), (10, 10));
        assert_eq!(find("Instalação"), (12, 12));
        assert_eq!(find("Baixe"), (14, 14));
        assert_eq!(
            find("fn main() {}"),
            (21, 23),
            "the fences are part of the block"
        );
        assert_eq!(find("Texto final."), (33, 33));
        assert_eq!(find("Última seção."), (37, 37));
    }

    #[test]
    fn lists_keep_order_depth_and_nesting() {
        let parsed = parse(DOC);
        let items: Vec<_> = parsed
            .blocks()
            .filter_map(|b| match b.kind {
                ContentKind::ListItem { ordered, depth } => Some((b.text.as_str(), ordered, depth)),
                _ => None,
            })
            .collect();
        assert_eq!(
            items,
            [
                ("Baixe", true, 0),
                ("Instale", true, 0),
                ("sub item", false, 1),
                ("Rode", true, 0),
                ("solto", false, 0),
            ]
        );
    }

    #[test]
    fn code_blocks_are_kept_literally_with_their_language() {
        let parsed = parse("# T\n\n```python\nif x:\n    print('a')\n```\n\n    indentado\n");
        let codes: Vec<_> = parsed
            .blocks()
            .filter_map(|b| match &b.kind {
                ContentKind::CodeBlock { language } => Some((language.clone(), b.text.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            codes,
            [
                (
                    Some("python".to_string()),
                    "if x:\n    print('a')".to_string()
                ),
                (None, "indentado".to_string()),
            ]
        );
    }

    #[test]
    fn tables_keep_header_and_rows() {
        let parsed = parse("| nome | qtd |\n|---|---|\n| café | 2 |\n| chá | 5 |\n");
        let table = parsed
            .blocks()
            .find(|b| matches!(b.kind, ContentKind::Table { .. }))
            .unwrap();
        assert_eq!(table.text, "nome | qtd\ncafé | 2\nchá | 5");
        match &table.kind {
            ContentKind::Table { header, rows } => {
                assert_eq!(header, &["nome", "qtd"]);
                assert_eq!(rows, &[vec!["café", "2"], vec!["chá", "5"]]);
            }
            _ => unreachable!(),
        }
        assert_eq!(lines_of(table), (1, 4));
    }

    #[test]
    fn quotes_are_paragraphs_and_html_is_ignored() {
        let parsed = parse("> citado\n\n<div>oculto</div>\n\ntexto");
        let texts: Vec<_> = parsed.blocks().map(|b| b.text.as_str()).collect();
        assert_eq!(texts, ["citado", "texto"]);
    }

    #[test]
    fn task_lists_and_hard_wrapped_paragraphs_read_naturally() {
        let parsed = parse("- [x] feito\n- [ ] pendente\n\nlinha um\nlinha dois\n");
        let texts: Vec<_> = parsed.blocks().map(|b| b.text.as_str()).collect();
        assert_eq!(texts, ["[x] feito", "[ ] pendente", "linha um linha dois"]);
    }

    #[test]
    fn block_locations_carry_the_heading_path() {
        let parsed = parse("# A\n\n## B\n\ntexto\n");
        let last = parsed.blocks().last().unwrap();
        assert_eq!(
            last.location,
            SourceLocation::markdown(vec!["A".into(), "B".into()], Some((5, 5))).unwrap()
        );
    }

    #[test]
    fn empty_documents_fail() {
        assert_eq!(
            MarkdownDocumentParser::parse_bytes(b"  \n"),
            Err(ParseError::Empty)
        );
        // Only front matter and raw HTML: nothing to index.
        assert_eq!(
            MarkdownDocumentParser::parse_bytes(b"---\ntitle: x\n---\n<br>\n"),
            Err(ParseError::Empty)
        );
        assert_eq!(
            MarkdownDocumentParser::parse_bytes(b"a\0b"),
            Err(ParseError::Invalid(DocumentType::Markdown))
        );
    }
}
