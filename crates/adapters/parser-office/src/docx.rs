//! DOCX: `word/document.xml` read in order as headings, paragraphs, list items and tables,
//! located by the enclosing headings and the paragraph number. Headers, footers, footnotes,
//! comments and text boxes' fallbacks are not read.

use std::collections::HashMap;

use nlmx_domain::{
    document_type::DocumentType,
    parsed::{
        ContentBlock, ContentKind, DocumentSection, ParseError, ParseWarning, ParsedDocument,
        RecordField,
    },
    source::SourceLocation,
};
use quick_xml::events::Event;

use crate::{
    archive::{Limits, Package},
    xml::{attr, collapse, core_properties, entity, lower, reader, to_utf8},
};

const MAIN_PART: &str = "word/document.xml";
/// A document of more paragraphs than this is refused rather than half read.
const MAX_BLOCKS: u32 = 2_000_000;

/// Parses the bytes of a DOCX file.
pub fn parse_docx(bytes: &[u8]) -> Result<ParsedDocument, ParseError> {
    parse_docx_with_limits(bytes, Limits::default())
}

pub fn parse_docx_with_limits(bytes: &[u8], limits: Limits) -> Result<ParsedDocument, ParseError> {
    let mut package = Package::open(bytes, limits, DocumentType::Docx)?;
    let main = package.main_part(MAIN_PART)?;
    let body = to_utf8(package.required(&main)?).ok_or(ParseError::Invalid(DocumentType::Docx))?;
    // The parts that only add detail are left out when they cannot be read.
    let mut detail = |name: &str| -> Result<Option<Vec<u8>>, ParseError> {
        Ok(package.optional(name)?.and_then(to_utf8))
    };
    let styles = detail("word/styles.xml")?
        .map(|b| Styles::read(&b))
        .unwrap_or_default();
    let numbering = detail("word/numbering.xml")?
        .map(|b| Numbering::read(&b))
        .unwrap_or_default();
    let metadata = detail("docProps/core.xml")?
        .map(|b| core_properties(&b))
        .unwrap_or_default();

    let sections = Builder::new(&styles, &numbering).run(&body)?;
    if sections.is_empty() {
        return Err(ParseError::Empty);
    }
    ParsedDocument::new(
        DocumentType::Docx,
        metadata,
        Vec::new(),
        sections,
        Vec::<ParseWarning>::new(),
    )
    .map_err(|error| ParseError::Engine(error.to_string()))
}

/// What the paragraph styles say about headings.
#[derive(Debug, Default)]
struct Styles {
    /// style id → (lowercase name, outline level 0-8, parent style id)
    by_id: HashMap<String, (String, Option<u8>, Option<String>)>,
}

impl Styles {
    fn read(bytes: &[u8]) -> Self {
        let mut styles = Self::default();
        let mut reader = reader(bytes);
        let mut current: Option<(String, String, Option<u8>, Option<String>)> = None;
        while let Ok(event) = reader.read_event() {
            match event {
                Event::Start(e) => match lower(e.local_name().as_ref()).as_str() {
                    "style" => {
                        let paragraph = attr(&e, "type").is_none_or(|t| t == "paragraph");
                        current = attr(&e, "styleid")
                            .filter(|_| paragraph)
                            .map(|id| (id, String::new(), None, None));
                    }
                    "name" => {
                        if let (Some(c), Some(v)) = (current.as_mut(), attr(&e, "val")) {
                            c.1 = v.to_lowercase();
                        }
                    }
                    "basedon" => {
                        if let Some(c) = current.as_mut() {
                            c.3 = attr(&e, "val");
                        }
                    }
                    "outlinelvl" => {
                        if let Some(c) = current.as_mut() {
                            c.2 = attr(&e, "val").and_then(|v| v.parse().ok());
                        }
                    }
                    _ => {}
                },
                Event::End(e) if lower(e.local_name().as_ref()) == "style" => {
                    if let Some((id, name, outline, parent)) = current.take() {
                        styles.by_id.insert(id, (name, outline, parent));
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
        styles
    }

    /// Whether a style is the caption style ("Caption", "Legenda"), following `basedOn`.
    fn is_caption(&self, style: &str) -> bool {
        let mut id = style;
        for _ in 0..6 {
            let Some((name, _, parent)) = self.by_id.get(id) else {
                return false;
            };
            if matches!(name.as_str(), "caption" | "legenda") {
                return true;
            }
            match parent.as_deref() {
                Some(parent) => id = parent,
                None => return false,
            }
        }
        false
    }

    /// The heading level (1 is the most prominent) of a style, following `basedOn`.
    fn heading_level(&self, style: &str) -> Option<u8> {
        let mut id = style;
        for _ in 0..6 {
            let (name, outline, parent) = self.by_id.get(id)?;
            if let Some(level) = level_from_name(name) {
                return Some(level);
            }
            if let Some(outline) = outline.filter(|o| *o < 9) {
                return Some(outline + 1);
            }
            id = parent.as_deref()?;
        }
        None
    }
}

/// `heading 2`, `título 2`, `titulo2` → 2; `title`/`título` → 1.
fn level_from_name(name: &str) -> Option<u8> {
    let name = name.trim();
    for prefix in ["heading", "título", "titulo"] {
        if let Some(rest) = name.strip_prefix(prefix) {
            let rest = rest.trim();
            return match rest.parse::<u8>() {
                Ok(level) if (1..=9).contains(&level) => Some(level),
                Ok(_) => None,
                Err(_) if rest.is_empty() && prefix != "heading" => Some(1),
                Err(_) => None,
            };
        }
    }
    (name == "title").then_some(1)
}

/// Which list levels are numbered (as opposed to bulleted).
#[derive(Debug, Default)]
struct Numbering {
    /// numId → abstractNumId
    nums: HashMap<String, String>,
    /// (abstractNumId, ilvl) → ordered
    levels: HashMap<(String, u8), bool>,
}

impl Numbering {
    fn read(bytes: &[u8]) -> Self {
        let mut numbering = Self::default();
        let mut reader = reader(bytes);
        let mut abstract_id: Option<String> = None;
        let mut level: Option<u8> = None;
        let mut num: Option<String> = None;
        while let Ok(event) = reader.read_event() {
            match event {
                Event::Start(e) => match lower(e.local_name().as_ref()).as_str() {
                    "abstractnum" => abstract_id = attr(&e, "abstractnumid"),
                    "lvl" => level = attr(&e, "ilvl").and_then(|v| v.parse().ok()),
                    "numfmt" => {
                        if let (Some(a), Some(l), Some(fmt)) =
                            (abstract_id.as_ref(), level, attr(&e, "val"))
                        {
                            numbering
                                .levels
                                .insert((a.clone(), l), fmt != "bullet" && fmt != "none");
                        }
                    }
                    "num" => num = attr(&e, "numid"),
                    "abstractnumid" => {
                        if let (Some(n), Some(a)) = (num.as_ref(), attr(&e, "val")) {
                            numbering.nums.insert(n.clone(), a);
                        }
                    }
                    _ => {}
                },
                Event::End(e) => match lower(e.local_name().as_ref()).as_str() {
                    "abstractnum" => abstract_id = None,
                    "lvl" => level = None,
                    "num" => num = None,
                    _ => {}
                },
                Event::Eof => break,
                _ => {}
            }
        }
        numbering
    }

    fn ordered(&self, num_id: &str, level: u8) -> bool {
        self.nums
            .get(num_id)
            .and_then(|a| self.levels.get(&(a.clone(), level)))
            .copied()
            .unwrap_or(false)
    }
}

#[derive(Debug, Default)]
struct Paragraph {
    text: String,
    style: Option<String>,
    outline: Option<u8>,
    list: Option<(String, u8)>,
}

#[derive(Debug, Default)]
struct Table {
    rows: Vec<Vec<String>>,
    row: Vec<String>,
    cell: String,
    /// Columns the open cell spans (`w:gridSpan`).
    span: usize,
    /// The row being read is marked as a header row (`w:tblHeader`).
    row_marked: bool,
    /// The first row is declared a header: by `w:tblHeader` on it, or `w:tblLook firstRow`.
    header_marked: bool,
}

/// A section being built.
struct SectionBuf {
    title: Option<String>,
    level: u8,
    path: Vec<String>,
    blocks: Vec<ContentBlock>,
}

struct Builder<'a> {
    styles: &'a Styles,
    numbering: &'a Numbering,
    sections: Vec<DocumentSection>,
    current: Option<SectionBuf>,
    /// Open headings, outermost first: (level, title).
    headings: Vec<(u8, String)>,
    paragraphs: u32,
    /// Tables of the document seen so far (the number of the last one).
    tables: u32,
    /// The table being added, so its blocks are located as table text.
    in_table: Option<u32>,
    /// The caption paragraph right before the next table, if there was one.
    caption: Option<String>,
}

impl<'a> Builder<'a> {
    fn new(styles: &'a Styles, numbering: &'a Numbering) -> Self {
        Self {
            styles,
            numbering,
            sections: Vec::new(),
            current: None,
            headings: Vec::new(),
            paragraphs: 0,
            tables: 0,
            in_table: None,
            caption: None,
        }
    }

    fn path(&self) -> Vec<String> {
        self.headings.iter().map(|(_, t)| t.clone()).collect()
    }

    fn flush(&mut self) {
        if let Some(section) = self.current.take() {
            self.sections.extend(DocumentSection::new(
                section.title,
                section.level,
                section.path,
                section.blocks,
            ));
        }
    }

    fn run(mut self, bytes: &[u8]) -> Result<Vec<DocumentSection>, ParseError> {
        let mut reader = reader(bytes);
        let mut paragraph: Option<Paragraph> = None;
        let mut table: Option<Table> = None;
        let mut table_depth = 0u32;
        let mut in_text = false;
        // Inside `mc:Fallback` the same content is repeated for old readers.
        let mut fallback = 0u32;
        loop {
            let event = reader
                .read_event()
                .map_err(|_| ParseError::Invalid(DocumentType::Docx))?;
            match event {
                Event::Start(e) => {
                    let name = lower(e.local_name().as_ref());
                    match name.as_str() {
                        "fallback" => fallback += 1,
                        _ if fallback > 0 => {}
                        "tbl" => {
                            table_depth += 1;
                            if table_depth == 1 {
                                table = Some(Table::default());
                            }
                        }
                        "tr" if table_depth == 1 => {
                            if let Some(t) = table.as_mut() {
                                t.row.clear();
                                t.row_marked = false;
                            }
                        }
                        "tc" if table_depth == 1 => {
                            if let Some(t) = table.as_mut() {
                                t.cell.clear();
                                t.span = 1;
                            }
                        }
                        "tblheader" if table_depth == 1 => {
                            if let Some(t) = table.as_mut() {
                                t.row_marked = attr(&e, "val")
                                    .is_none_or(|v| !matches!(v.as_str(), "0" | "false" | "off"));
                            }
                        }
                        "tbllook" if table_depth == 1 => {
                            if let Some(t) = table.as_mut() {
                                let first_row = attr(&e, "firstrow").is_some_and(|v| v == "1")
                                    || attr(&e, "val")
                                        .and_then(|v| u32::from_str_radix(&v, 16).ok())
                                        .is_some_and(|bits| bits & 0x20 != 0);
                                t.header_marked |= first_row;
                            }
                        }
                        "gridspan" if table_depth == 1 => {
                            if let Some(t) = table.as_mut() {
                                t.span = attr(&e, "val")
                                    .and_then(|v| v.parse().ok())
                                    .map_or(1, |n: usize| n.clamp(1, 64));
                            }
                        }
                        "p" => paragraph = Some(Paragraph::default()),
                        "pstyle" => {
                            if let Some(p) = paragraph.as_mut() {
                                p.style = attr(&e, "val");
                            }
                        }
                        "outlinelvl" => {
                            if let Some(p) = paragraph.as_mut() {
                                p.outline = attr(&e, "val").and_then(|v| v.parse().ok());
                            }
                        }
                        "numid" => {
                            if let (Some(p), Some(id)) = (paragraph.as_mut(), attr(&e, "val"))
                                && id != "0"
                            {
                                let level = p.list.as_ref().map_or(0, |l| l.1);
                                p.list = Some((id, level));
                            }
                        }
                        "ilvl" => {
                            if let (Some(p), Some(level)) = (
                                paragraph.as_mut(),
                                attr(&e, "val").and_then(|v| v.parse::<u8>().ok()),
                            ) {
                                let id = p.list.as_ref().map(|l| l.0.clone());
                                // `ilvl` may come before `numId`: keep it until the id shows up.
                                p.list = Some((id.unwrap_or_default(), level));
                            }
                        }
                        "t" => in_text = true,
                        "tab" => {
                            if let Some(p) = paragraph.as_mut() {
                                p.text.push(' ');
                            }
                        }
                        "br" | "cr" => {
                            if let Some(p) = paragraph.as_mut() {
                                p.text.push(' ');
                            }
                        }
                        "nobreakhyphen" => {
                            if let Some(p) = paragraph.as_mut() {
                                p.text.push('-');
                            }
                        }
                        _ => {}
                    }
                }
                Event::Text(t) if in_text && fallback == 0 => {
                    if let (Some(p), Ok(text)) = (paragraph.as_mut(), t.decode()) {
                        p.text.push_str(&text);
                    }
                }
                Event::GeneralRef(r) if in_text && fallback == 0 => {
                    if let Some(p) = paragraph.as_mut() {
                        p.text.push_str(&entity(&r).unwrap_or_default());
                    }
                }
                Event::End(e) => {
                    let name = lower(e.local_name().as_ref());
                    match name.as_str() {
                        "fallback" => fallback = fallback.saturating_sub(1),
                        _ if fallback > 0 => {}
                        "t" => in_text = false,
                        "p" => {
                            if let Some(done) = paragraph.take() {
                                if table_depth > 0 {
                                    if let Some(t) = table.as_mut() {
                                        let text = collapse(&done.text);
                                        if !text.is_empty() {
                                            if !t.cell.is_empty() {
                                                t.cell.push(' ');
                                            }
                                            t.cell.push_str(&text);
                                        }
                                    }
                                } else {
                                    self.paragraph(done)?;
                                }
                            }
                        }
                        "tc" if table_depth == 1 => {
                            if let Some(t) = table.as_mut() {
                                let cell = std::mem::take(&mut t.cell);
                                t.row.push(cell);
                                // A merged cell keeps its text in the first column it covers.
                                for _ in 1..t.span {
                                    t.row.push(String::new());
                                }
                                t.span = 1;
                            }
                        }
                        "tr" if table_depth == 1 => {
                            if let Some(t) = table.as_mut() {
                                let row = std::mem::take(&mut t.row);
                                if row.iter().any(|c| !c.is_empty()) {
                                    if t.rows.is_empty() && t.row_marked {
                                        t.header_marked = true;
                                    }
                                    t.rows.push(row);
                                }
                            }
                        }
                        "tbl" => {
                            table_depth = table_depth.saturating_sub(1);
                            if table_depth == 0
                                && let Some(done) = table.take()
                            {
                                self.table(done)?;
                            }
                        }
                        _ => {}
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
        self.flush();
        Ok(self.sections)
    }

    fn next_paragraph(&mut self) -> Result<u32, ParseError> {
        self.paragraphs += 1;
        if self.paragraphs > MAX_BLOCKS {
            return Err(ParseError::TooLarge);
        }
        Ok(self.paragraphs)
    }

    fn location(&self, n: u32) -> Result<SourceLocation, ParseError> {
        SourceLocation::docx_table(self.path(), Some((n, n)), self.in_table)
            .map_err(|error| ParseError::Engine(error.to_string()))
    }

    fn push(&mut self, kind: ContentKind, text: String, n: u32) -> Result<(), ParseError> {
        let location = self.location(n)?;
        let block = ContentBlock {
            kind,
            text,
            location,
        };
        match self.current.as_mut() {
            Some(current) => current.blocks.push(block),
            None => {
                self.current = Some(SectionBuf {
                    title: None,
                    level: 0,
                    path: Vec::new(),
                    blocks: vec![block],
                });
            }
        }
        Ok(())
    }

    fn paragraph(&mut self, paragraph: Paragraph) -> Result<(), ParseError> {
        let text = collapse(&paragraph.text);
        if text.is_empty() {
            return Ok(());
        }
        let level = paragraph
            .outline
            .filter(|o| *o < 9)
            .map(|o| o + 1)
            .or_else(|| {
                paragraph
                    .style
                    .as_deref()
                    .and_then(|s| self.styles.heading_level(s))
            });
        let n = self.next_paragraph()?;
        // Only the paragraph right before a table can be its caption.
        self.caption = paragraph
            .style
            .as_deref()
            .filter(|s| level.is_none() && self.styles.is_caption(s))
            .map(|_| text.clone());
        if let Some(level) = level {
            self.flush();
            while self.headings.last().is_some_and(|(l, _)| *l >= level) {
                self.headings.pop();
            }
            self.headings.push((level, text.clone()));
            self.current = Some(SectionBuf {
                title: Some(text.clone()),
                level,
                path: self.path(),
                blocks: Vec::new(),
            });
            return self.push(ContentKind::Heading { level }, text, n);
        }
        let kind = match paragraph.list.filter(|(id, _)| !id.is_empty()) {
            Some((id, depth)) => ContentKind::ListItem {
                ordered: self.numbering.ordered(&id, depth),
                depth,
            },
            None => ContentKind::Paragraph,
        };
        self.push(kind, text, n)
    }

    /// A table whose first row is a header becomes one `Record` per data row, so each row
    /// reads on its own ("Tabela: Nome | Cidade\nNome: João\nCidade: Fortaleza") and is never
    /// cut in the middle. Without a header that can be trusted, the table is kept as one
    /// `Table` block of lines, so nothing is lost.
    fn table(&mut self, table: Table) -> Result<(), ParseError> {
        self.tables += 1;
        let caption = self.caption.take().and_then(|c| caption_title(&c));
        let Table {
            rows,
            header_marked,
            ..
        } = table;
        if rows.is_empty() {
            return Ok(());
        }
        self.in_table = Some(self.tables);
        let done = self.table_blocks(rows, header_marked, caption);
        self.in_table = None;
        done
    }

    fn table_blocks(
        &mut self,
        mut rows: Vec<Vec<String>>,
        header_marked: bool,
        caption: Option<String>,
    ) -> Result<(), ParseError> {
        if rows.len() > 1 && header_is_reliable(&rows[0], header_marked) {
            let header = rows.remove(0);
            let width = rows
                .iter()
                .map(Vec::len)
                .max()
                .unwrap_or(0)
                .max(header.len());
            let names: Vec<String> = (0..width)
                .map(|i| match header.get(i) {
                    Some(name) if !name.is_empty() => name.clone(),
                    _ => format!("Coluna {}", i + 1),
                })
                .collect();
            let label = match &caption {
                Some(caption) => format!("Tabela {} — {caption}", self.tables),
                None => format!("Tabela {}", self.tables),
            };
            let title = format!("{label}: {}", names.join(" | "));
            for row in rows {
                let fields: Vec<RecordField> = names
                    .iter()
                    .enumerate()
                    .map(|(i, name)| RecordField {
                        name: name.clone(),
                        value: row.get(i).cloned().unwrap_or_default(),
                    })
                    .collect();
                if fields.iter().all(|f| f.value.is_empty()) {
                    continue;
                }
                let mut text = title.clone();
                for field in fields.iter().filter(|f| !f.value.is_empty()) {
                    text.push_str(&format!("\n{}: {}", field.name, field.value));
                }
                let n = self.next_paragraph()?;
                self.push(ContentKind::Record { fields }, text, n)?;
            }
            return Ok(());
        }
        let header = rows.remove(0);
        let mut lines = vec![header.join(" | ")];
        lines.extend(rows.iter().map(|row| row.join(" | ")));
        let n = self.next_paragraph()?;
        self.push(ContentKind::Table { header, rows }, lines.join("\n"), n)
    }
}

/// A caption's own words: "Tabela 2 – Equipe do projeto" → "Equipe do projeto" (the number is
/// the table's index, which the label already says). `None` when nothing is left.
fn caption_title(caption: &str) -> Option<String> {
    let text = caption.trim();
    let lower = text.to_lowercase();
    let rest = ["tabela", "table"]
        .iter()
        .find(|word| lower.starts_with(**word))
        .map_or(text, |word| {
            text[word.len()..]
                .trim_start()
                .trim_start_matches(|c: char| c.is_ascii_digit())
        });
    let rest = rest.trim_matches(|c: char| c.is_whitespace() || ".:-–—".contains(c));
    (!rest.is_empty()).then(|| rest.to_string())
}

/// Longest cell that can still be a column name in a header that is guessed.
const MAX_GUESSED_NAME: usize = 60;

/// Whether the first row names the columns: declared so by the file, or (without a
/// declaration) every cell is a short, distinct text.
fn header_is_reliable(first: &[String], declared: bool) -> bool {
    if first.iter().all(String::is_empty) {
        return false;
    }
    if declared {
        return true;
    }
    first.len() > 1
        && first
            .iter()
            .all(|c| !c.is_empty() && c.chars().count() <= MAX_GUESSED_NAME)
        && {
            let mut seen: Vec<&str> = Vec::new();
            first.iter().all(|c| {
                let new = !seen.contains(&c.as_str());
                seen.push(c);
                new
            })
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_levels_come_from_style_names() {
        assert_eq!(level_from_name("heading 2"), Some(2));
        assert_eq!(level_from_name("título 3"), Some(3));
        assert_eq!(level_from_name("titulo1"), Some(1));
        assert_eq!(level_from_name("title"), Some(1));
        assert_eq!(level_from_name("normal"), None);
        assert_eq!(level_from_name("heading 12"), None);
    }
}
