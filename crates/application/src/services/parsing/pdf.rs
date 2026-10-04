//! The PDF parser: PDFium through [`DocumentEngine`] and the structure analysis through
//! [`StructureAnalyzer`], read as a [`ParsedDocument`]. Nothing here reads PDF bytes itself.

use std::sync::Arc;

use nlmx_domain::{
    document::{DocumentError, DocumentHandle, DocumentMetadata as PdfMetadata},
    document_type::DocumentType,
    ingestion::PageLayout,
    parsed::{
        ContentBlock, ContentKind, DocumentMetadata, DocumentSection, PageSummary, ParseError,
        ParsedDocument,
    },
};

use crate::ports::{BoxFuture, DocumentEngine, DocumentParser, DocumentSource, StructureAnalyzer};

/// Reads every page of an open document: its metadata and, per page, size, text-layer facts
/// and text spans (pages without a text layer are not asked for spans).
pub(crate) async fn read_layouts(
    engine: &dyn DocumentEngine,
    handle: DocumentHandle,
) -> Result<(PdfMetadata, Vec<PageLayout>), DocumentError> {
    let metadata = engine.metadata(handle).await?;
    let mut pages = Vec::with_capacity(metadata.page_count as usize);
    for number in 1..=metadata.page_count {
        let info = engine.page_info(handle, number).await?;
        let spans = if info.has_text {
            engine.text_spans(handle, number).await?
        } else {
            Vec::new()
        };
        pages.push(PageLayout {
            number,
            width: info.width,
            height: info.height,
            has_text: info.has_text,
            char_count: info.char_count,
            spans,
        });
    }
    Ok((metadata, pages))
}

pub struct PdfDocumentParser {
    engine: Arc<dyn DocumentEngine>,
    analyzer: Arc<dyn StructureAnalyzer>,
}

impl PdfDocumentParser {
    pub fn new(engine: Arc<dyn DocumentEngine>, analyzer: Arc<dyn StructureAnalyzer>) -> Self {
        Self { engine, analyzer }
    }

    async fn read(&self, source: &DocumentSource) -> Result<ParsedDocument, ParseError> {
        let handle = self.engine.open(&source.path).await.map_err(parse_error)?;
        let read = read_layouts(self.engine.as_ref(), handle).await;
        // Always release the engine's document, even when reading failed.
        let _ = self.engine.close(handle).await;
        let (metadata, layouts) = read.map_err(parse_error)?;

        let structured = self.analyzer.analyze(&layouts);
        let mut sections = Vec::new();
        let mut current: Option<Section> = None;
        for block in &structured.blocks {
            let content = ContentBlock::from_block(block);
            match content.kind {
                ContentKind::Heading { level } => {
                    sections.extend(current.take().and_then(Section::finish));
                    let mut path = block.section_path.clone();
                    path.push(block.text.clone());
                    current = Some(Section {
                        title: Some(block.text.clone()),
                        level,
                        path,
                        blocks: vec![content],
                    });
                }
                _ => match current.as_mut() {
                    Some(section) if section.path == block.section_path => {
                        section.blocks.push(content);
                    }
                    _ => {
                        sections.extend(current.take().and_then(Section::finish));
                        current = Some(Section {
                            title: None,
                            level: 0,
                            path: block.section_path.clone(),
                            blocks: vec![content],
                        });
                    }
                },
            }
        }
        sections.extend(current.and_then(Section::finish));

        let pages = layouts
            .iter()
            .map(|page| PageSummary {
                number: page.number,
                width: page.width,
                height: page.height,
                char_count: page.char_count,
                has_text: page.has_text,
            })
            .collect();
        let metadata = DocumentMetadata {
            title: metadata.title,
            author: metadata.author,
            created_at: metadata.created_at,
            modified_at: metadata.modified_at,
            subject: metadata.subject,
            page_count: Some(metadata.page_count),
            source_version: metadata.pdf_version,
            ..DocumentMetadata::default()
        };
        ParsedDocument::new(DocumentType::Pdf, metadata, pages, sections, Vec::new())
            .map_err(|error| ParseError::Engine(error.to_string()))
    }
}

/// A section being built.
struct Section {
    title: Option<String>,
    level: u8,
    path: Vec<String>,
    blocks: Vec<ContentBlock>,
}

impl Section {
    fn finish(self) -> Option<DocumentSection> {
        DocumentSection::new(self.title, self.level, self.path, self.blocks)
    }
}

fn parse_error(error: DocumentError) -> ParseError {
    match error {
        DocumentError::NotFound => ParseError::NotFound,
        DocumentError::InvalidPdf(_) => ParseError::Invalid(DocumentType::Pdf),
        DocumentError::PasswordRequired => ParseError::Encrypted,
        other => ParseError::Engine(other.to_string()),
    }
}

impl DocumentParser for PdfDocumentParser {
    fn document_type(&self) -> DocumentType {
        DocumentType::Pdf
    }

    fn version(&self) -> u32 {
        self.analyzer.version()
    }

    fn parse<'a>(
        &'a self,
        source: &'a DocumentSource,
    ) -> BoxFuture<'a, Result<ParsedDocument, ParseError>> {
        Box::pin(async move {
            if source
                .document_type
                .is_some_and(|kind| kind != DocumentType::Pdf)
            {
                return Err(ParseError::Unsupported);
            }
            self.read(source).await
        })
    }
}
