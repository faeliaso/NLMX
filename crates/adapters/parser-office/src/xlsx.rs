//! XLSX: each visible worksheet becomes one section whose rows are `Record` blocks (the first
//! row is the header when the data confirms it), located by sheet and by the row number shown
//! in Excel. Cached values of formulas are read, never recalculated; dates become ISO text.

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
    archive::{Limits, Package, dir_of, relationships, rels_of, resolve},
    numfmt::{Styles, serial_to_text},
    xml::{attr, collapse, core_properties, entity, lower, reader},
};

const MAIN_PART: &str = "xl/workbook.xml";
/// Rows of all sheets together; a bigger workbook is refused rather than half read.
pub const MAX_ROWS: u32 = 250_000;
/// Columns read per row (Excel's own limit is 16 384).
const MAX_COLUMNS: usize = 16_384;

/// Parses the bytes of an XLSX file.
pub fn parse_xlsx(bytes: &[u8]) -> Result<ParsedDocument, ParseError> {
    parse_xlsx_with_limits(bytes, Limits::default())
}

pub fn parse_xlsx_with_limits(bytes: &[u8], limits: Limits) -> Result<ParsedDocument, ParseError> {
    let mut package = Package::open(bytes, limits, DocumentType::Xlsx)?;
    let main = package.main_part(MAIN_PART)?;
    let workbook = package.required(&main)?;
    let base = dir_of(&main).to_string();
    let rels = package
        .optional(&rels_of(&main))?
        .map(|b| relationships(&b))
        .unwrap_or_default();
    let sheets = read_sheets(&workbook);
    if sheets.is_empty() {
        return Err(package.invalid());
    }
    let shared = match package.optional(&resolve(&base, "sharedStrings.xml"))? {
        Some(bytes) => shared_strings(&bytes),
        None => Vec::new(),
    };
    let styles = package
        .optional(&resolve(&base, "styles.xml"))?
        .map(|b| Styles::read(&b))
        .unwrap_or_default()
        .with_date1904(workbook_uses_1904(&workbook));
    let metadata = package
        .optional("docProps/core.xml")?
        .map(|b| core_properties(&b))
        .unwrap_or_default();

    let mut sections = Vec::new();
    let mut warnings = Vec::new();
    let mut rows_total = 0u32;
    for (position, sheet) in sheets.iter().enumerate() {
        let index = u32::try_from(position + 1).unwrap_or(u32::MAX);
        if sheet.hidden {
            warnings.push(ParseWarning::SkippedUnit { index });
            continue;
        }
        let Some(target) = rels
            .iter()
            .find(|r| r.id == sheet.rel_id && r.kind.ends_with("/worksheet"))
            .map(|r| resolve(&base, &r.target))
        else {
            // A chart sheet or a dangling reference: nothing to read.
            continue;
        };
        let bytes = match package.read(&target) {
            Ok(Some(bytes)) => bytes,
            Ok(None) | Err(crate::archive::ReadFailure::Corrupt) => {
                warnings.push(ParseWarning::SkippedUnit { index });
                continue;
            }
            Err(crate::archive::ReadFailure::TooLarge) => return Err(ParseError::TooLarge),
        };
        let rows = read_rows(&bytes, &shared, &styles, MAX_ROWS - rows_total)?;
        rows_total += u32::try_from(rows.len()).unwrap_or(u32::MAX);
        if let Some(section) = sheet_section(index, &sheet.name, rows, &mut warnings)? {
            sections.push(section);
        }
    }
    if sections.is_empty() {
        return Err(ParseError::Empty);
    }
    warnings.dedup();
    ParsedDocument::new(DocumentType::Xlsx, metadata, Vec::new(), sections, warnings)
        .map_err(|error| ParseError::Engine(error.to_string()))
}

/// Whether `<workbookPr date1904="1">` switches the workbook to the 1904 date system.
fn workbook_uses_1904(workbook: &[u8]) -> bool {
    let mut reader = reader(workbook);
    while let Ok(event) = reader.read_event() {
        match event {
            Event::Start(e) if lower(e.local_name().as_ref()) == "workbookpr" => {
                return attr(&e, "date1904").is_some_and(|v| v == "1" || v == "true");
            }
            Event::Eof => break,
            _ => {}
        }
    }
    false
}

#[derive(Debug)]
struct SheetRef {
    name: String,
    rel_id: String,
    hidden: bool,
}

fn read_sheets(bytes: &[u8]) -> Vec<SheetRef> {
    let mut reader = reader(bytes);
    let mut sheets = Vec::new();
    while let Ok(event) = reader.read_event() {
        match event {
            Event::Start(e) if lower(e.local_name().as_ref()) == "sheet" => {
                if let (Some(name), Some(rel_id)) = (attr(&e, "name"), attr(&e, "id")) {
                    let hidden = attr(&e, "state").is_some_and(|s| s != "visible");
                    sheets.push(SheetRef {
                        name,
                        rel_id,
                        hidden,
                    });
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    sheets
}

/// The shared strings, in order: the plain text of each `<si>` (phonetic runs left out).
fn shared_strings(bytes: &[u8]) -> Vec<String> {
    let mut reader = reader(bytes);
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_si = false;
    let mut in_text = false;
    let mut phonetic = 0u32;
    while let Ok(event) = reader.read_event() {
        match event {
            Event::Start(e) => match lower(e.local_name().as_ref()).as_str() {
                "si" => {
                    in_si = true;
                    current.clear();
                }
                "rph" => phonetic += 1,
                "t" if in_si && phonetic == 0 => in_text = true,
                _ => {}
            },
            Event::Text(t) if in_text => {
                if let Ok(text) = t.decode() {
                    current.push_str(&text);
                }
            }
            Event::GeneralRef(r) if in_text => current.push_str(&entity(&r).unwrap_or_default()),
            Event::End(e) => match lower(e.local_name().as_ref()).as_str() {
                "t" => in_text = false,
                "rph" => phonetic = phonetic.saturating_sub(1),
                "si" => {
                    in_si = false;
                    out.push(std::mem::take(&mut current));
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    out
}

/// A cell's value and whether it is plain text (as opposed to a number or a date).
#[derive(Debug, Clone, Default)]
struct Cell {
    text: String,
    textual: bool,
}

/// A row of the sheet: its Excel number and its cells by column (0-based, gaps are empty).
#[derive(Debug)]
struct Row {
    number: u32,
    cells: Vec<Cell>,
}

fn read_rows(
    bytes: &[u8],
    shared: &[String],
    styles: &Styles,
    budget: u32,
) -> Result<Vec<Row>, ParseError> {
    let mut reader = reader(bytes);
    let mut rows: Vec<Row> = Vec::new();
    let mut row: Option<Row> = None;
    let mut next_row = 0u32;
    // The cell being read: (column, type, style).
    let mut cell: Option<(usize, String, Option<usize>)> = None;
    let mut value = String::new();
    let mut capture = false;
    loop {
        let event = reader
            .read_event()
            .map_err(|_| ParseError::Invalid(DocumentType::Xlsx))?;
        match event {
            Event::Start(e) => match lower(e.local_name().as_ref()).as_str() {
                "row" => {
                    next_row = attr(&e, "r")
                        .and_then(|r| r.parse().ok())
                        .unwrap_or(next_row + 1);
                    row = Some(Row {
                        number: next_row,
                        cells: Vec::new(),
                    });
                }
                "c" if row.is_some() => {
                    let column = attr(&e, "r")
                        .as_deref()
                        .and_then(column_of)
                        .unwrap_or_else(|| row.as_ref().map_or(0, |r| r.cells.len()));
                    cell = Some((
                        column.min(MAX_COLUMNS - 1),
                        attr(&e, "t").unwrap_or_default(),
                        attr(&e, "s").and_then(|s| s.parse().ok()),
                    ));
                    value.clear();
                }
                // `<v>` holds the value; `<is><t>` an inline string.
                "v" | "t" if cell.is_some() => capture = true,
                _ => {}
            },
            Event::Text(t) if capture => {
                if let Ok(text) = t.decode() {
                    value.push_str(&text);
                }
            }
            Event::GeneralRef(r) if capture => value.push_str(&entity(&r).unwrap_or_default()),
            Event::End(e) => match lower(e.local_name().as_ref()).as_str() {
                "v" | "t" => capture = false,
                "c" => {
                    if let (Some((column, kind, style)), Some(r)) = (cell.take(), row.as_mut()) {
                        let parsed = cell_value(&kind, style, &value, shared, styles);
                        if !parsed.text.is_empty() {
                            if r.cells.len() <= column {
                                r.cells.resize(column + 1, Cell::default());
                            }
                            r.cells[column] = parsed;
                        }
                    }
                }
                "row" => {
                    if let Some(done) = row.take()
                        && done.cells.iter().any(|c| !c.text.is_empty())
                    {
                        if u32::try_from(rows.len()).unwrap_or(u32::MAX) >= budget {
                            return Err(ParseError::TooLarge);
                        }
                        rows.push(done);
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(rows)
}

fn cell_value(
    kind: &str,
    style: Option<usize>,
    raw: &str,
    shared: &[String],
    styles: &Styles,
) -> Cell {
    let raw = raw.trim();
    match kind {
        "s" => {
            let text = raw
                .parse::<usize>()
                .ok()
                .and_then(|i| shared.get(i))
                .map(|s| collapse(s))
                .unwrap_or_default();
            Cell {
                text,
                textual: true,
            }
        }
        "inlineStr" | "str" => Cell {
            text: collapse(raw),
            textual: true,
        },
        "b" => Cell {
            text: if raw == "1" { "Sim" } else { "Não" }.to_string(),
            textual: false,
        },
        "e" => Cell::default(),
        _ => {
            if raw.is_empty() {
                return Cell::default();
            }
            let text = match (style.is_some_and(|s| styles.is_date(s)), raw.parse::<f64>()) {
                (true, Ok(serial)) => {
                    serial_to_text(serial, styles.date1904).unwrap_or_else(|| raw.to_string())
                }
                _ => raw.to_string(),
            };
            Cell {
                text,
                textual: false,
            }
        }
    }
}

/// The 0-based column of a cell reference (`C7` → 2).
fn column_of(reference: &str) -> Option<usize> {
    let mut column = 0usize;
    let mut seen = false;
    for c in reference.chars().take_while(char::is_ascii_alphabetic) {
        column = column
            .checked_mul(26)?
            .checked_add((c.to_ascii_uppercase() as u8 - b'A' + 1) as usize)?;
        seen = true;
    }
    (seen && column > 0).then(|| column - 1)
}

/// One sheet as a section of records; `None` when it has no content.
fn sheet_section(
    index: u32,
    name: &str,
    mut rows: Vec<Row>,
    warnings: &mut Vec<ParseWarning>,
) -> Result<Option<DocumentSection>, ParseError> {
    if rows.is_empty() {
        return Ok(None);
    }
    let width = rows.iter().map(|r| r.cells.len()).max().unwrap_or(0);
    // The first row is the header when it is all text and something follows it.
    let header_is_text = rows[0].cells.iter().all(|c| c.text.is_empty() || c.textual);
    let has_header = header_is_text && rows.len() > 1;
    let columns: Vec<String> = if has_header {
        let header = rows.remove(0);
        (0..width)
            .map(|i| match header.cells.get(i) {
                Some(c) if !c.text.is_empty() => c.text.clone(),
                _ => format!("Coluna {}", i + 1),
            })
            .collect()
    } else {
        warnings.push(ParseWarning::NoHeaderRow);
        (0..width).map(|i| format!("Coluna {}", i + 1)).collect()
    };
    if has_header && rows.iter().all(|r| r.cells.iter().all(|c| c.textual)) {
        warnings.push(ParseWarning::UncertainHeader);
    }
    let mut blocks = Vec::with_capacity(rows.len());
    for row in rows {
        let fields: Vec<RecordField> = columns
            .iter()
            .enumerate()
            .map(|(i, column)| RecordField {
                name: column.clone(),
                value: row.cells.get(i).map(|c| c.text.clone()).unwrap_or_default(),
            })
            .collect();
        let mut text = format!("Registro {}:", row.number);
        for field in fields.iter().filter(|f| !f.value.is_empty()) {
            text.push_str(&format!("\n{}: {}", field.name, field.value));
        }
        let location = SourceLocation::xlsx(index, name.to_string(), row.number, row.number)
            .map_err(|error| ParseError::Engine(error.to_string()))?;
        blocks.push(ContentBlock {
            kind: ContentKind::Record { fields },
            text,
            location,
        });
    }
    Ok(DocumentSection::new(
        Some(name.to_string()),
        1,
        vec![name.to_string()],
        blocks,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_are_read_from_references() {
        assert_eq!(column_of("A1"), Some(0));
        assert_eq!(column_of("Z9"), Some(25));
        assert_eq!(column_of("AA10"), Some(26));
        assert_eq!(column_of("c3"), Some(2));
        assert_eq!(column_of("12"), None);
    }

    #[test]
    fn shared_strings_join_rich_runs_and_skip_phonetics() {
        let xml = r#"<sst><si><t>Um</t></si><si><r><t>Do</t></r><r><t>is</t></r><rPh><t>ignorado</t></rPh></si></sst>"#;
        assert_eq!(shared_strings(xml.as_bytes()), ["Um", "Dois"]);
    }
}
