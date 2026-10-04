//! What any parser produces, whatever the format: metadata plus sections of typed blocks,
//! each with its location. The structure (headings, lists, code, tables, records) is kept
//! until chunking; the rest of the system reads this and never the parser's own types.
//!
//! `document::DocumentMetadata` (the PDF engine's, with `pdf_version` and `page_count`) is a
//! different, PDF-only type; this module's `DocumentMetadata` is the neutral one.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{
    document_type::DocumentType,
    ingestion::{Block, BlockKind, ChunkDraft, DocumentId},
    source::{LocationError, SourceLocation},
    vectors::ChunkId,
};

/// Descriptive fields every format can fill in (all optional; dates are ISO-8601).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentMetadata {
    pub title: Option<String>,
    pub author: Option<String>,
    /// Language tag as declared by the document, e.g. "pt-BR".
    pub language: Option<String>,
    pub created_at: Option<String>,
    pub modified_at: Option<String>,
    pub publisher: Option<String>,
    pub subject: Option<String>,
    /// Pages, for a paged format.
    pub page_count: Option<u32>,
    /// Version of the format as declared by the file, e.g. "1.7" for a PDF.
    pub source_version: Option<String>,
    /// What a table-like file (CSV) holds: columns, delimiter, whether it has a header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dataset: Option<DatasetMetadata>,
}

/// The kind of value a column mostly holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnKind {
    Text,
    Number,
    Date,
    Boolean,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasetColumn {
    pub name: String,
    pub kind: ColumnKind,
}

/// Facts about a table-like file, known before its rows are read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasetMetadata {
    pub columns: Vec<DatasetColumn>,
    pub delimiter: char,
    /// Whether the first row was read as the header (otherwise columns are named by position).
    pub has_header: bool,
    /// Data rows, when the whole file was read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_count: Option<u32>,
}

impl DatasetMetadata {
    pub fn column_names(&self) -> Vec<String> {
        self.columns.iter().map(|c| c.name.clone()).collect()
    }
}

/// An imported document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    pub id: DocumentId,
    /// SHA-256 (hex) of the file.
    pub sha256: String,
    pub document_type: DocumentType,
    pub metadata: DocumentMetadata,
}

/// One cell of a tabular record: the column name and the value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordField {
    pub name: String,
    pub value: String,
}

/// The structural role of a block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContentKind {
    /// Level 1 is the most prominent heading.
    Heading {
        level: u8,
    },
    Paragraph,
    /// `depth` is 0 for a top-level item.
    ListItem {
        ordered: bool,
        depth: u8,
    },
    CodeBlock {
        language: Option<String>,
    },
    Table {
        header: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    /// A row of a table-like file (CSV), with the name of each column.
    Record {
        fields: Vec<RecordField>,
    },
}

/// A unit of content in reading order. `text` is the readable form, the one that is embedded
/// and shown (a record reads "Coluna: valor; Coluna: valor"); `kind` keeps the structure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContentBlock {
    pub kind: ContentKind,
    pub text: String,
    pub location: SourceLocation,
}

impl ContentBlock {
    /// A block of the PDF structure analyzer, read as a format-neutral one. Its page range
    /// runs from the page it starts on to the last page of its boxes.
    pub fn from_block(block: &Block) -> Self {
        let kind = match block.kind {
            BlockKind::Heading { level } => ContentKind::Heading { level },
            BlockKind::Paragraph => ContentKind::Paragraph,
            BlockKind::ListItem => ContentKind::ListItem {
                ordered: false,
                depth: 0,
            },
        };
        let page_end = block
            .boxes
            .iter()
            .map(|b| b.page)
            .max()
            .map_or(block.page, |last| last.max(block.page));
        Self {
            kind,
            text: block.text.clone(),
            location: SourceLocation::Pdf {
                page_start: block.page,
                page_end,
                boxes: block.boxes.clone(),
            },
        }
    }
}

/// A heading and the blocks under it (or the blocks before the first heading).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentSection {
    pub title: Option<String>,
    /// Heading level, 1 being the most prominent; 0 when there is no heading.
    pub level: u8,
    /// Enclosing headings including this section's own, outermost first. Empty for blocks
    /// outside any heading.
    pub path: Vec<String>,
    /// In reading order; the heading itself is the first block of a titled section.
    pub blocks: Vec<ContentBlock>,
    /// Where the section starts (its first block).
    pub location: SourceLocation,
}

impl DocumentSection {
    /// `None` when there are no blocks.
    pub fn new(
        title: Option<String>,
        level: u8,
        path: Vec<String>,
        blocks: Vec<ContentBlock>,
    ) -> Option<Self> {
        let location = blocks.first()?.location.clone();
        Some(Self {
            title,
            level,
            path,
            blocks,
            location,
        })
    }

    /// The readable text of the section: its blocks' text separated by blank lines.
    pub fn text(&self) -> String {
        self.blocks
            .iter()
            .map(|b| b.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

/// Whether a section starts with a heading or is the text outside any heading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionKind {
    Heading,
    Body,
}

impl SectionKind {
    /// Stable name (stored in `document_sections.kind`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Heading => "heading",
            Self::Body => "body",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "heading" => Some(Self::Heading),
            "body" => Some(Self::Body),
            _ => None,
        }
    }
}

/// A section without its text: what is stored of the structure (the text lives in chunks).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SectionOutline {
    pub kind: SectionKind,
    pub title: Option<String>,
    pub level: u8,
    /// Enclosing headings including the section's own, outermost first.
    pub path: Vec<String>,
    /// Where the section starts.
    pub location: SourceLocation,
}

/// A page of a paged document, as the viewer and the text-layer check need it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageSummary {
    /// 1-based.
    pub number: u32,
    pub width: f32,
    pub height: f32,
    pub char_count: u32,
    pub has_text: bool,
}

/// Something a parser worked around. Carries no content of the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ParseWarning {
    /// A unit (e.g. an EPUB chapter, 1-based) could not be read and was left out.
    SkippedUnit { index: u32 },
    /// A table-like file has no header row; columns were named by position.
    NoHeaderRow,
    /// The first row was read as a header although nothing in the data confirms it (every
    /// cell is text).
    UncertainHeader,
    /// The bytes were not UTF-8 and were read as Windows-1252.
    FallbackEncoding,
}

/// A parser's output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParsedDocument {
    document_type: DocumentType,
    metadata: DocumentMetadata,
    pages: Vec<PageSummary>,
    sections: Vec<DocumentSection>,
    warnings: Vec<ParseWarning>,
}

/// A parsed document that contradicts itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParsedDocumentError {
    /// Section `index` (or one of its blocks) points into a document of another format.
    SectionOfOtherType {
        index: usize,
        expected: DocumentType,
        found: DocumentType,
    },
    InvalidLocation {
        index: usize,
        error: LocationError,
    },
    /// Page summaries on a format that has no pages.
    PagesInUnpagedFormat(DocumentType),
}

impl fmt::Display for ParsedDocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SectionOfOtherType {
                index,
                expected,
                found,
            } => write!(
                f,
                "a seção {index} é de um documento {found}, não {expected}"
            ),
            Self::InvalidLocation { index, error } => {
                write!(f, "localização inválida na seção {index}: {error}")
            }
            Self::PagesInUnpagedFormat(kind) => {
                write!(f, "um documento {kind} não tem páginas")
            }
        }
    }
}

impl std::error::Error for ParsedDocumentError {}

impl ParsedDocument {
    /// Every section and block must be located in a document of `document_type` and be
    /// valid; only a paged format has `pages`.
    pub fn new(
        document_type: DocumentType,
        metadata: DocumentMetadata,
        pages: Vec<PageSummary>,
        sections: Vec<DocumentSection>,
        warnings: Vec<ParseWarning>,
    ) -> Result<Self, ParsedDocumentError> {
        let parsed = Self {
            document_type,
            metadata,
            pages,
            sections,
            warnings,
        };
        parsed.validate()?;
        Ok(parsed)
    }

    /// Checks the invariants; call it on a value that was deserialized.
    pub fn validate(&self) -> Result<(), ParsedDocumentError> {
        if !self.pages.is_empty() && !self.document_type.is_paged() {
            return Err(ParsedDocumentError::PagesInUnpagedFormat(
                self.document_type,
            ));
        }
        for (index, section) in self.sections.iter().enumerate() {
            let locations = std::iter::once(&section.location)
                .chain(section.blocks.iter().map(|b| &b.location));
            for location in locations {
                let found = location.document_type();
                if found != self.document_type {
                    return Err(ParsedDocumentError::SectionOfOtherType {
                        index,
                        expected: self.document_type,
                        found,
                    });
                }
                location
                    .validate()
                    .map_err(|error| ParsedDocumentError::InvalidLocation { index, error })?;
            }
        }
        Ok(())
    }

    /// Takes the document apart, to build a changed one with [`ParsedDocument::new`].
    pub fn into_parts(
        self,
    ) -> (
        DocumentType,
        DocumentMetadata,
        Vec<PageSummary>,
        Vec<DocumentSection>,
        Vec<ParseWarning>,
    ) {
        (
            self.document_type,
            self.metadata,
            self.pages,
            self.sections,
            self.warnings,
        )
    }

    pub fn document_type(&self) -> DocumentType {
        self.document_type
    }

    pub fn metadata(&self) -> &DocumentMetadata {
        &self.metadata
    }

    pub fn pages(&self) -> &[PageSummary] {
        &self.pages
    }

    pub fn sections(&self) -> &[DocumentSection] {
        &self.sections
    }

    pub fn warnings(&self) -> &[ParseWarning] {
        &self.warnings
    }

    /// The sections without their text, in reading order.
    pub fn outline(&self) -> Vec<SectionOutline> {
        self.sections
            .iter()
            .map(|section| SectionOutline {
                kind: if section.title.is_some() {
                    SectionKind::Heading
                } else {
                    SectionKind::Body
                },
                title: section.title.clone(),
                level: section.level,
                path: section.path.clone(),
                location: section.location.clone(),
            })
            .collect()
    }

    /// Every block in reading order.
    pub fn blocks(&self) -> impl Iterator<Item = &ContentBlock> {
        self.sections.iter().flat_map(|s| s.blocks.iter())
    }

    /// Whether any block has text other than whitespace.
    pub fn has_text(&self) -> bool {
        self.blocks().any(|b| !b.text.trim().is_empty())
    }

    /// A paged document none of whose pages has a text layer: it would need OCR.
    pub fn needs_ocr(&self) -> bool {
        self.document_type.is_paged() && !self.pages.iter().any(|p| p.has_text)
    }
}

/// Why a document could not be parsed. Messages never contain the document's content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    NotFound,
    /// No parser handles the format.
    Unsupported,
    /// The file is not a valid document of this format.
    Invalid(DocumentType),
    /// Protected by a password (PDF).
    Encrypted,
    /// Protected by DRM (EPUB).
    Drm,
    /// The text could not be decoded.
    Encoding,
    /// A valid document with no text.
    Empty,
    TooLarge,
    /// The engine behind the parser failed.
    Engine(String),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("arquivo não encontrado"),
            Self::Unsupported => f.write_str("formato de documento não suportado"),
            Self::Invalid(kind) => {
                write!(
                    f,
                    "o arquivo não é um documento {} válido",
                    kind.display_name()
                )
            }
            Self::Encrypted => f.write_str("o documento está protegido por senha"),
            Self::Drm => f.write_str("o documento tem proteção DRM"),
            Self::Encoding => f.write_str("não foi possível ler a codificação do texto"),
            Self::Empty => f.write_str("o documento não tem texto"),
            Self::TooLarge => f.write_str("o arquivo é grande demais"),
            Self::Engine(message) => write!(f, "falha ao ler o documento: {message}"),
        }
    }
}

impl std::error::Error for ParseError {}

/// What the chunker is told about the document it chunks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkContext {
    pub document_id: DocumentId,
    pub document_title: Option<String>,
    /// Name of the original file.
    pub file_name: Option<String>,
    pub language: Option<String>,
}

impl ChunkContext {
    /// The metadata every chunk of the document carries.
    pub fn metadata(&self, document_type: DocumentType, columns: Vec<String>) -> ChunkMetadata {
        ChunkMetadata {
            document_type,
            document_title: self.document_title.clone(),
            file_name: self.file_name.clone(),
            language: self.language.clone(),
            columns,
        }
    }
}

/// Facts about the document that travel with each of its chunks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkMetadata {
    pub document_type: DocumentType,
    pub document_title: Option<String>,
    pub file_name: Option<String>,
    pub language: Option<String>,
    /// Column names of a table-like file, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<String>,
}

/// A chunk, located without reference to the format's parser.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentChunk {
    pub document_id: DocumentId,
    /// The stored chunk's id; `None` until the chunk is saved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_id: Option<ChunkId>,
    /// Position within the document, from 0.
    pub index: u32,
    pub text: String,
    /// Approximate token count, when a counter was available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_count: Option<u32>,
    /// Headings or chapters that enclose the chunk, outermost first ("Capítulo 3" › …).
    pub section_path: Vec<String>,
    /// Where in the source the chunk comes from.
    pub location: SourceLocation,
    pub metadata: ChunkMetadata,
    /// SHA-256 (hex) of `text`.
    pub content_hash: String,
}

impl DocumentChunk {
    /// The chunk of today's PDF pipeline, read as a format-neutral one. `ChunkDraft` is
    /// unchanged; this is a one-way view.
    pub fn from_draft(draft: &ChunkDraft, context: &ChunkContext) -> Self {
        Self {
            document_id: context.document_id,
            chunk_id: None,
            index: draft.index,
            text: draft.text.clone(),
            token_count: Some(draft.token_count),
            section_path: draft.section_path.clone(),
            location: SourceLocation::Pdf {
                page_start: draft.page_start,
                page_end: draft.page_end,
                boxes: draft.boxes.clone(),
            },
            metadata: context.metadata(DocumentType::Pdf, Vec::new()),
            content_hash: draft.content_hash.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{document::BoundingBox, ingestion::PageBox};

    fn paragraph(text: &str, location: SourceLocation) -> ContentBlock {
        ContentBlock {
            kind: ContentKind::Paragraph,
            text: text.into(),
            location,
        }
    }

    fn markdown_at(line: u32) -> SourceLocation {
        SourceLocation::markdown(vec!["Guia".into()], Some((line, line + 3))).unwrap()
    }

    fn section(text: &str, location: SourceLocation) -> DocumentSection {
        DocumentSection::new(
            Some("Guia".into()),
            1,
            vec!["Guia".into()],
            vec![paragraph(text, location)],
        )
        .unwrap()
    }

    fn page(number: u32, has_text: bool) -> PageSummary {
        PageSummary {
            number,
            width: 612.0,
            height: 792.0,
            char_count: if has_text { 10 } else { 0 },
            has_text,
        }
    }

    #[test]
    fn a_parsed_document_keeps_its_sections_in_order() {
        let sections = vec![
            section("um", markdown_at(1)),
            section("dois", markdown_at(5)),
        ];
        let parsed = ParsedDocument::new(
            DocumentType::Markdown,
            DocumentMetadata {
                title: Some("Guia".into()),
                ..Default::default()
            },
            vec![],
            sections.clone(),
            vec![ParseWarning::FallbackEncoding],
        )
        .unwrap();
        assert_eq!(parsed.document_type(), DocumentType::Markdown);
        assert_eq!(parsed.metadata().title.as_deref(), Some("Guia"));
        assert_eq!(parsed.sections(), sections);
        assert_eq!(parsed.warnings(), [ParseWarning::FallbackEncoding]);
        assert!(parsed.has_text());
        let texts: Vec<_> = parsed.blocks().map(|b| b.text.as_str()).collect();
        assert_eq!(texts, ["um", "dois"]);
    }

    #[test]
    fn a_section_starts_where_its_first_block_does() {
        let blocks = vec![
            paragraph("a", markdown_at(4)),
            paragraph("b", markdown_at(9)),
        ];
        let section = DocumentSection::new(None, 0, vec![], blocks).unwrap();
        assert_eq!(section.location, markdown_at(4));
        assert_eq!(section.text(), "a\n\nb");
        assert_eq!(DocumentSection::new(None, 0, vec![], vec![]), None);
    }

    #[test]
    fn a_section_or_block_of_another_format_is_rejected() {
        let result = ParsedDocument::new(
            DocumentType::Markdown,
            DocumentMetadata::default(),
            vec![],
            vec![
                section("ok", markdown_at(1)),
                section("linhas", SourceLocation::csv(1, 3).unwrap()),
            ],
            vec![],
        );
        assert_eq!(
            result,
            Err(ParsedDocumentError::SectionOfOtherType {
                index: 1,
                expected: DocumentType::Markdown,
                found: DocumentType::Csv,
            })
        );

        // The section starts in Markdown but one of its blocks does not.
        let mut mixed = section("ok", markdown_at(1));
        mixed
            .blocks
            .push(paragraph("x", SourceLocation::text(0, 1).unwrap()));
        assert!(
            ParsedDocument::new(
                DocumentType::Markdown,
                DocumentMetadata::default(),
                vec![],
                vec![mixed],
                vec![],
            )
            .is_err()
        );
    }

    #[test]
    fn an_invalid_location_is_rejected() {
        let result = ParsedDocument::new(
            DocumentType::Csv,
            DocumentMetadata::default(),
            vec![],
            vec![section(
                "x",
                SourceLocation::Csv {
                    row_start: 4,
                    row_end: 2,
                },
            )],
            vec![],
        );
        assert_eq!(
            result,
            Err(ParsedDocumentError::InvalidLocation {
                index: 0,
                error: LocationError::Reversed("linhas"),
            })
        );
    }

    #[test]
    fn only_a_paged_format_has_pages() {
        let result = ParsedDocument::new(
            DocumentType::Text,
            DocumentMetadata::default(),
            vec![page(1, true)],
            vec![],
            vec![],
        );
        assert_eq!(
            result,
            Err(ParsedDocumentError::PagesInUnpagedFormat(
                DocumentType::Text
            ))
        );
        assert!(
            ParsedDocument::new(
                DocumentType::Pdf,
                DocumentMetadata::default(),
                vec![page(1, true)],
                vec![],
                vec![],
            )
            .is_ok()
        );
    }

    #[test]
    fn whitespace_only_blocks_have_no_text() {
        let parsed = ParsedDocument::new(
            DocumentType::Text,
            DocumentMetadata::default(),
            vec![],
            vec![section(" \n\t", SourceLocation::text(0, 3).unwrap())],
            vec![],
        )
        .unwrap();
        assert!(!parsed.has_text());
        assert!(!parsed.needs_ocr(), "OCR concerns paged formats only");
    }

    #[test]
    fn a_pdf_needs_ocr_when_no_page_has_text() {
        let build = |pages| {
            ParsedDocument::new(
                DocumentType::Pdf,
                DocumentMetadata::default(),
                pages,
                vec![],
                vec![],
            )
            .unwrap()
        };
        assert!(build(vec![page(1, false), page(2, false)]).needs_ocr());
        assert!(!build(vec![page(1, false), page(2, true)]).needs_ocr());
    }

    #[test]
    fn a_structure_block_becomes_a_pdf_content_block() {
        let boxes = vec![
            PageBox {
                page: 2,
                bbox: BoundingBox {
                    left: 1.0,
                    top: 2.0,
                    right: 3.0,
                    bottom: 4.0,
                },
            },
            PageBox {
                page: 3,
                bbox: BoundingBox {
                    left: 1.0,
                    top: 2.0,
                    right: 3.0,
                    bottom: 4.0,
                },
            },
        ];
        let block = Block {
            kind: BlockKind::Heading { level: 2 },
            text: "Resultados".into(),
            page: 2,
            boxes: boxes.clone(),
            section_path: vec!["1 Introdução".into()],
        };
        let content = ContentBlock::from_block(&block);
        assert_eq!(content.kind, ContentKind::Heading { level: 2 });
        assert_eq!(content.text, "Resultados");
        assert_eq!(content.location, SourceLocation::pdf(2, 3, boxes).unwrap());

        let list = Block {
            kind: BlockKind::ListItem,
            text: "item".into(),
            page: 5,
            boxes: vec![],
            section_path: vec![],
        };
        let content = ContentBlock::from_block(&list);
        assert_eq!(
            content.kind,
            ContentKind::ListItem {
                ordered: false,
                depth: 0
            }
        );
        assert_eq!(content.location, SourceLocation::pdf(5, 5, vec![]).unwrap());
    }

    #[test]
    fn a_pdf_draft_becomes_a_pdf_chunk() {
        let boxes = vec![PageBox {
            page: 2,
            bbox: BoundingBox {
                left: 1.0,
                top: 2.0,
                right: 3.0,
                bottom: 4.0,
            },
        }];
        let draft = ChunkDraft {
            index: 7,
            text: "texto do trecho".into(),
            token_count: 12,
            page_start: 2,
            page_end: 3,
            section_path: vec!["1 Introdução".into()],
            boxes: boxes.clone(),
            content_hash: "abc123".into(),
        };
        let context = ChunkContext {
            document_id: 42,
            document_title: Some("Contrato".into()),
            file_name: Some("contrato.pdf".into()),
            language: None,
        };
        let chunk = DocumentChunk::from_draft(&draft, &context);
        assert_eq!(chunk.document_id, 42);
        assert_eq!(chunk.chunk_id, None);
        assert_eq!(chunk.metadata.document_type, DocumentType::Pdf);
        assert_eq!(chunk.metadata.file_name.as_deref(), Some("contrato.pdf"));
        assert_eq!(chunk.index, 7);
        assert_eq!(chunk.text, "texto do trecho");
        assert_eq!(chunk.token_count, Some(12));
        assert_eq!(chunk.section_path, ["1 Introdução"]);
        assert_eq!(chunk.content_hash, "abc123");
        assert_eq!(chunk.location, SourceLocation::pdf(2, 3, boxes).unwrap());
        assert!(chunk.location.previewable());
    }

    #[test]
    fn structured_content_round_trips_through_json() {
        let blocks = vec![
            ContentBlock {
                kind: ContentKind::Record {
                    fields: vec![RecordField {
                        name: "produto".into(),
                        value: "Café".into(),
                    }],
                },
                text: "produto: Café".into(),
                location: SourceLocation::csv(1, 1).unwrap(),
            },
            ContentBlock {
                kind: ContentKind::Table {
                    header: vec!["a".into()],
                    rows: vec![vec!["1".into()]],
                },
                text: "a\n1".into(),
                location: SourceLocation::csv(2, 2).unwrap(),
            },
            ContentBlock {
                kind: ContentKind::CodeBlock {
                    language: Some("rust".into()),
                },
                text: "fn main() {}".into(),
                location: SourceLocation::csv(3, 3).unwrap(),
            },
        ];
        let sections = vec![DocumentSection::new(None, 0, vec![], blocks).unwrap()];
        let parsed = ParsedDocument::new(
            DocumentType::Csv,
            DocumentMetadata::default(),
            vec![],
            sections,
            vec![
                ParseWarning::NoHeaderRow,
                ParseWarning::SkippedUnit { index: 2 },
            ],
        )
        .unwrap();
        let json = serde_json::to_string(&parsed).unwrap();
        assert!(json.contains("\"kind\":\"record\""), "{json}");
        let back: ParsedDocument = serde_json::from_str(&json).unwrap();
        assert_eq!(back, parsed);
        assert_eq!(back.validate(), Ok(()));
    }

    #[test]
    fn documents_and_chunks_round_trip_through_json() {
        let document = Document {
            id: 5,
            sha256: "f".repeat(64),
            document_type: DocumentType::Epub,
            metadata: DocumentMetadata {
                title: Some("Livro".into()),
                language: Some("pt-BR".into()),
                ..Default::default()
            },
        };
        let json = serde_json::to_string(&document).unwrap();
        assert!(json.contains("\"document_type\":\"epub\""), "{json}");
        assert_eq!(serde_json::from_str::<Document>(&json).unwrap(), document);

        let chunk = DocumentChunk {
            document_id: 5,
            chunk_id: Some(77),
            index: 0,
            text: "a,b".into(),
            token_count: Some(3),
            section_path: vec!["Vendas".into()],
            location: SourceLocation::csv(2, 9).unwrap(),
            metadata: ChunkMetadata {
                document_type: DocumentType::Csv,
                document_title: Some("Vendas".into()),
                file_name: Some("vendas.csv".into()),
                language: None,
                columns: vec!["a".into(), "b".into()],
            },
            content_hash: "h".into(),
        };
        let json = serde_json::to_string(&chunk).unwrap();
        assert_eq!(serde_json::from_str::<DocumentChunk>(&json).unwrap(), chunk);
    }

    #[test]
    fn dataset_metadata_round_trips_and_is_optional() {
        let dataset = DatasetMetadata {
            columns: vec![
                DatasetColumn {
                    name: "Nome".into(),
                    kind: ColumnKind::Text,
                },
                DatasetColumn {
                    name: "Idade".into(),
                    kind: ColumnKind::Number,
                },
            ],
            delimiter: ';',
            has_header: true,
            row_count: Some(2),
        };
        assert_eq!(dataset.column_names(), ["Nome", "Idade"]);
        let metadata = DocumentMetadata {
            dataset: Some(dataset),
            ..Default::default()
        };
        let json = serde_json::to_string(&metadata).unwrap();
        assert!(json.contains("\"kind\":\"number\""), "{json}");
        assert_eq!(
            serde_json::from_str::<DocumentMetadata>(&json).unwrap(),
            metadata
        );
        // Metadata written before the field existed still reads.
        let old = r#"{"title":null,"author":null,"language":null,"created_at":null,
            "modified_at":null,"publisher":null,"subject":null,"page_count":null,
            "source_version":null}"#;
        assert_eq!(
            serde_json::from_str::<DocumentMetadata>(old)
                .unwrap()
                .dataset,
            None
        );
    }

    #[test]
    fn a_chunk_not_yet_stored_has_no_id_and_a_pdf_one_no_columns() {
        let json = serde_json::to_string(&DocumentChunk {
            document_id: 1,
            chunk_id: None,
            index: 0,
            text: "t".into(),
            token_count: None,
            section_path: vec![],
            location: SourceLocation::pdf(1, 1, vec![]).unwrap(),
            metadata: ChunkMetadata {
                document_type: DocumentType::Pdf,
                document_title: None,
                file_name: None,
                language: None,
                columns: vec![],
            },
            content_hash: "h".into(),
        })
        .unwrap();
        assert!(
            !json.contains("chunk_id")
                && !json.contains("columns")
                && !json.contains("token_count"),
            "{json}"
        );
    }

    #[test]
    fn the_outline_lists_sections_without_text() {
        let body = DocumentSection::new(
            None,
            0,
            vec![],
            vec![paragraph("antes dos títulos", markdown_at(1))],
        )
        .unwrap();
        let parsed = ParsedDocument::new(
            DocumentType::Markdown,
            DocumentMetadata::default(),
            vec![],
            vec![body, section("um", markdown_at(5))],
            vec![],
        )
        .unwrap();
        let outline = parsed.outline();
        assert_eq!(outline.len(), 2);
        assert_eq!(outline[0].kind, SectionKind::Body);
        assert_eq!(outline[0].title, None);
        assert_eq!(outline[0].location, markdown_at(1));
        assert_eq!(outline[1].kind, SectionKind::Heading);
        assert_eq!(outline[1].title.as_deref(), Some("Guia"));
        assert_eq!(outline[1].path, ["Guia"]);
        assert_eq!(outline[1].level, 1);
        for kind in [SectionKind::Heading, SectionKind::Body] {
            assert_eq!(SectionKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(SectionKind::parse("chapter"), None);
    }

    #[test]
    fn a_parsed_document_can_be_taken_apart_and_rebuilt() {
        let parsed = ParsedDocument::new(
            DocumentType::Markdown,
            DocumentMetadata::default(),
            vec![],
            vec![section("um", markdown_at(1))],
            vec![ParseWarning::FallbackEncoding],
        )
        .unwrap();
        let copy = parsed.clone();
        let (kind, metadata, pages, sections, warnings) = parsed.into_parts();
        assert_eq!(
            ParsedDocument::new(kind, metadata, pages, sections, warnings).unwrap(),
            copy
        );
    }

    #[test]
    fn errors_describe_the_problem_without_content() {
        assert_eq!(
            ParseError::Invalid(DocumentType::Pdf).to_string(),
            "o arquivo não é um documento PDF válido"
        );
        assert_eq!(
            ParseError::Invalid(DocumentType::Epub).to_string(),
            "o arquivo não é um documento EPUB válido"
        );
        for error in [
            ParseError::NotFound,
            ParseError::Unsupported,
            ParseError::Encrypted,
            ParseError::Drm,
            ParseError::Encoding,
            ParseError::Empty,
            ParseError::TooLarge,
        ] {
            assert!(!error.to_string().is_empty());
        }
    }
}
